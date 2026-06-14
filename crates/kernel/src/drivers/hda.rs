use crate::devfs;
use crate::drivers::pci::{self, ScanKind};
use crate::pmm;
use crate::util::SyncUnsafeCell;
use crate::{log_debug, log_info, log_warn};

// HDA controller registers
const HDA_GCAP: u32 = 0x00;
const HDA_GCTL: u32 = 0x08;
const HDA_STATESTS: u32 = 0x0E;
const HDA_INTCTL: u32 = 0x20;
const HDA_CORBWP: u32 = 0x48;
const HDA_CORBRP: u32 = 0x4A;
const HDA_CORBCTL: u32 = 0x4C;
const HDA_RIRBWP: u32 = 0x58;
const HDA_RIRBCTL: u32 = 0x5C;

// Immediate Command Interface registers
const HDA_ICO: u32 = 0x60; // Immediate Command Output
const HDA_IRI: u32 = 0x64; // Immediate Response Input
const HDA_ICS: u32 = 0x68; // Immediate Command Status (bit0=ICB, bit1=IRV)

// Stream descriptor register offsets (relative to stream base)
const SD_CTL: u32 = 0x00;
const SD_STS: u32 = 0x03;
const SD_CBL: u32 = 0x08;
const SD_LVI: u32 = 0x0C;
const SD_FMT: u32 = 0x12;
const SD_BDPL: u32 = 0x18;
const SD_BDPU: u32 = 0x1C;

// Common verbs
const fn get_param(param: u32) -> u32 {
    0xF0000 | param
}
const fn set_stream_channel(sc: u32) -> u32 {
    0x70600 | sc
}
const fn set_converter_format(fmt: u32) -> u32 {
    0x20000 | fmt
}
const fn set_amp_gain_mute(v: u32) -> u32 {
    0x30000 | v
}
const fn set_pin_widget_ctl(v: u32) -> u32 {
    0x70700 | v
}
const fn set_eapd_btl(v: u32) -> u32 {
    0x70C00 | v
}
const fn set_power_state(v: u32) -> u32 {
    0x70500 | v
}
const fn get_conn_list_entry(idx: u32) -> u32 {
    0xF0200 | idx
}
const fn set_conn_select(idx: u32) -> u32 {
    0x70100 | idx
}

// Parameter IDs
const PARAM_SUB_NODE_COUNT: u32 = 0x04;
const PARAM_FN_GROUP_TYPE: u32 = 0x05;
const PARAM_AUDIO_WIDGET_CAP: u32 = 0x09;
const PARAM_PIN_CAP: u32 = 0x0C;
const PARAM_CONN_LIST_LEN: u32 = 0x0E;
const PARAM_OUT_AMP_CAP: u32 = 0x12;

// Widget types (from Audio Widget Capabilities bits 23:20)
const WIDGET_TYPE_OUTPUT: u8 = 0x0;
const WIDGET_TYPE_PIN: u8 = 0x4;

const fn make_verb(cad: u32, nid: u32, verb: u32) -> u32 {
    (cad << 28) | (nid << 20) | verb
}

// Stream config
const SAMPLE_RATE: u32 = 48000;
const CHUNK_SAMPLES: u32 = 1200; // 25ms worth at 48kHz
const NUM_CHUNKS: usize = 4;
const PCM_BUF_SAMPLES: u32 = CHUNK_SAMPLES * NUM_CHUNKS as u32;
const PCM_BUF_BYTES: u32 = PCM_BUF_SAMPLES * 2 * 2; // stereo, 16-bit
const STREAM_ID: u32 = 1;

#[repr(C, packed)]
struct BdlEntry {
    address: u64,
    length: u32,
    ioc: u32, // bit 0 = interrupt on completion
}

static HDA: SyncUnsafeCell<HdAudio> = SyncUnsafeCell::new(HdAudio::new());

// Simple fixed-size queue of PIDs waiting for /dev/audio buffer space.
const MAX_WAITERS: usize = 8;
static HDA_WAITERS: SyncUnsafeCell<[Option<u64>; MAX_WAITERS]> =
    SyncUnsafeCell::new([None; MAX_WAITERS]);

pub fn init() -> Option<u8> {
    unsafe { (*HDA.0.get()).initialize() }
}

pub fn play_frequency(frequency: u32) {
    unsafe { (*HDA.0.get()).play_frequency(frequency) }
}

pub fn stop() {
    unsafe { (*HDA.0.get()).stop() }
}

pub fn is_available() -> bool {
    unsafe { (*HDA.0.get()).is_available() }
}

pub fn set_stream_channels(channels: u8) {
    unsafe { (*HDA.0.get()).set_stream_channels(channels) }
}

pub fn write_stream(buf: &[u8]) -> usize {
    unsafe { (*HDA.0.get()).write_stream(buf) }
}

pub fn wait_for_space() -> i64 {
    unsafe { (*HDA.0.get()).wait_for_space() }
}

pub fn on_interrupt() {
    unsafe { (*HDA.0.get()).on_interrupt() }
}

pub fn irq() -> u8 {
    unsafe { (*HDA.0.get()).irq }
}

struct HdAudio {
    available: bool,
    pci_device: Option<pci::Device>,
    mmio: *mut u8,
    bdl: *mut BdlEntry,
    pcm_buffer: *mut i16,
    codec_addr: u8,
    dac_node: u8,
    pin_node: u8,
    osd_base: u32,
    irq: u8,
    stream_channels: u8,
    chunks_written: u64,
    chunks_completed: u64,
    irq_count: u64,
    fifo_errors: u64,
    desc_errors: u64,
}

impl HdAudio {
    const fn new() -> Self {
        Self {
            available: false,
            pci_device: None,
            mmio: core::ptr::null_mut(),
            bdl: core::ptr::null_mut(),
            pcm_buffer: core::ptr::null_mut(),
            codec_addr: 0,
            dac_node: 0,
            pin_node: 0,
            osd_base: 0,
            irq: 0,
            stream_channels: 2,
            chunks_written: 0,
            chunks_completed: 0,
            irq_count: 0,
            fifo_errors: 0,
            desc_errors: 0,
        }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    // ---- MMIO register access ----

    fn reg_read32(&self, off: u32) -> u32 {
        unsafe { (self.mmio.add(off as usize) as *const u32).read_volatile() }
    }
    fn reg_read16(&self, off: u32) -> u16 {
        unsafe { (self.mmio.add(off as usize) as *const u16).read_volatile() }
    }
    fn reg_read8(&self, off: u32) -> u8 {
        unsafe { self.mmio.add(off as usize).read_volatile() }
    }
    fn reg_write32(&self, off: u32, val: u32) {
        unsafe { (self.mmio.add(off as usize) as *mut u32).write_volatile(val) }
    }
    fn reg_write16(&self, off: u32, val: u16) {
        unsafe { (self.mmio.add(off as usize) as *mut u16).write_volatile(val) }
    }
    fn reg_write8(&self, off: u32, val: u8) {
        unsafe { self.mmio.add(off as usize).write_volatile(val) }
    }

    // ---- Controller discovery / init ----

    fn find_controller(&mut self) -> bool {
        let mut found: Option<pci::Device> = None;
        pci::scan(ScanKind::Pci, |dev| {
            if dev.class_code == 0x04 && dev.subclass == 0x03 && found.is_none() {
                found = Some(dev);
            }
        });
        if let Some(dev) = found {
            self.pci_device = Some(dev);
            true
        } else {
            false
        }
    }

    fn reset_controller(&self) -> bool {
        // Put controller in reset
        self.reg_write32(HDA_GCTL, self.reg_read32(HDA_GCTL) & !1u32);
        stall_us(100);
        for _ in 0..100 {
            if self.reg_read32(HDA_GCTL) & 1 == 0 {
                break;
            }
            stall_us(1000);
        }

        // Bring controller out of reset
        self.reg_write32(HDA_GCTL, self.reg_read32(HDA_GCTL) | 1);
        stall_us(100);
        for _ in 0..100 {
            if self.reg_read32(HDA_GCTL) & 1 != 0 {
                break;
            }
            stall_us(1000);
        }
        if self.reg_read32(HDA_GCTL) & 1 == 0 {
            return false;
        }

        // Wait for codecs to enumerate
        stall_us(500_000);

        self.reg_read16(HDA_STATESTS) != 0
    }

    fn alloc_dma_buffers(&mut self) -> bool {
        // BDL: one page, zeroed
        let bdl_addr = pmm::alloc_frame().unwrap_or(0);
        if bdl_addr == 0 {
            return false;
        }
        self.bdl = bdl_addr as usize as *mut BdlEntry;
        unsafe {
            core::ptr::write_bytes(self.bdl as *mut u8, 0, 4096);
        }

        // PCM buffer: two halves for ping-pong streaming
        let pcm_pages = ((PCM_BUF_BYTES * 2 + 4095) / 4096) as usize;
        let pcm_addr = pmm::alloc_frames(pcm_pages).unwrap_or(0);
        if pcm_addr == 0 {
            return false;
        }
        self.pcm_buffer = pcm_addr as usize as *mut i16;
        unsafe {
            core::ptr::write_bytes(self.pcm_buffer as *mut u8, 0, (PCM_BUF_BYTES * 2) as usize);
        }

        true
    }

    // ---- Codec communication via Immediate Command Interface ----

    fn send_verb(&self, verb: u32) -> u32 {
        // Wait for ICB (Immediate Command Busy) to clear
        for _ in 0..1000 {
            if self.reg_read16(HDA_ICS) & 0x01 == 0 {
                break;
            }
            stall_us(100);
        }
        if self.reg_read16(HDA_ICS) & 0x01 != 0 {
            return 0; // still busy, timeout
        }

        // Clear IRV (Immediate Response Valid)
        self.reg_write16(HDA_ICS, 0x02);

        // Write the verb and set ICB to start processing
        self.reg_write32(HDA_ICO, verb);
        self.reg_write16(HDA_ICS, self.reg_read16(HDA_ICS) | 0x01);

        // Wait for ICB to clear and IRV to be set
        for _ in 0..10000 {
            stall_us(100);
            let ics = self.reg_read16(HDA_ICS);
            if ics & 0x01 == 0 && ics & 0x02 != 0 {
                let response = self.reg_read32(HDA_IRI);
                self.reg_write16(HDA_ICS, 0x02);
                return response;
            }
        }
        0 // timeout
    }

    fn codec_command(&self, node: u8, verb: u32) -> u32 {
        self.send_verb(make_verb(self.codec_addr as u32, node as u32, verb))
    }

    // ---- Codec discovery & configuration ----

    fn discover_codec(&mut self) -> bool {
        let statests = self.reg_read16(HDA_STATESTS);
        self.codec_addr = 0xFF;
        for i in 0..15u8 {
            if statests & (1 << i) != 0 {
                self.codec_addr = i;
                break;
            }
        }
        if self.codec_addr == 0xFF {
            return false;
        }

        log_debug!("HDA: CORBCTL = {:#x}", self.reg_read8(HDA_CORBCTL));
        log_debug!("HDA: RIRBCTL = {:#x}", self.reg_read8(HDA_RIRBCTL));
        log_debug!("HDA: CORBWP  = {:#x}", self.reg_read16(HDA_CORBWP));
        log_debug!("HDA: CORBRP  = {:#x}", self.reg_read16(HDA_CORBRP));
        log_debug!("HDA: RIRBWP  = {:#x}", self.reg_read16(HDA_RIRBWP));

        // Get root node subordinate nodes
        let sub_nodes = self.codec_command(0, get_param(PARAM_SUB_NODE_COUNT));
        log_debug!("HDA: Root SubNodes = {:#x}", sub_nodes);
        let start_node = ((sub_nodes >> 16) & 0xFF) as u8;
        let total_nodes = (sub_nodes & 0xFF) as u8;

        // Find the Audio Function Group (AFG)
        let mut afg_node = 0u8;
        for i in 0..total_nodes {
            let nid = start_node + i;
            let fg_type = self.codec_command(nid, get_param(PARAM_FN_GROUP_TYPE));
            log_debug!("HDA: Node FGType = {:#x}", fg_type);
            if fg_type & 0xFF == 0x01 {
                afg_node = nid;
                break;
            }
        }
        if afg_node == 0 {
            log_warn!("HDA: No AFG found!");
            return false;
        }
        log_debug!("HDA: AFG node = {:#x}", afg_node);

        // Power on the AFG
        self.codec_command(afg_node, set_power_state(0x00)); // D0
        stall_us(10000);

        // Enumerate AFG subnodes
        let sub_nodes = self.codec_command(afg_node, get_param(PARAM_SUB_NODE_COUNT));
        log_debug!("HDA: AFG SubNodes = {:#x}", sub_nodes);
        let start_node = ((sub_nodes >> 16) & 0xFF) as u8;
        let total_nodes = (sub_nodes & 0xFF) as u8;

        self.dac_node = 0;
        self.pin_node = 0;

        // First pass: find a DAC (audio output converter)
        for i in 0..total_nodes {
            if self.dac_node != 0 {
                break;
            }
            let nid = start_node + i;
            let widget_cap = self.codec_command(nid, get_param(PARAM_AUDIO_WIDGET_CAP));
            let widget_type = ((widget_cap >> 20) & 0xF) as u8;
            log_debug!("HDA: Widget NID={:#x} Cap={:#x}", nid, widget_cap);
            if widget_type == WIDGET_TYPE_OUTPUT {
                self.dac_node = nid;
            }
        }

        // Second pass: find an output-capable pin
        for i in 0..total_nodes {
            if self.pin_node != 0 {
                break;
            }
            let nid = start_node + i;
            let widget_cap = self.codec_command(nid, get_param(PARAM_AUDIO_WIDGET_CAP));
            let widget_type = ((widget_cap >> 20) & 0xF) as u8;
            if widget_type == WIDGET_TYPE_PIN {
                let pin_cap = self.codec_command(nid, get_param(PARAM_PIN_CAP));
                log_debug!("HDA: Pin NID={:#x} PinCap={:#x}", nid, pin_cap);
                if pin_cap & (1 << 4) != 0 {
                    self.pin_node = nid;
                }
            }
        }

        if self.dac_node == 0 || self.pin_node == 0 {
            log_debug!("HDA: dacNode={:#x}", self.dac_node);
            log_debug!("HDA: pinNode={:#x}", self.pin_node);
            return false;
        }

        true
    }

    fn configure_codec(&self) -> bool {
        // Power on DAC and Pin
        self.codec_command(self.dac_node, set_power_state(0x00));
        self.codec_command(self.pin_node, set_power_state(0x00));
        stall_us(10000);

        // Pin Widget Control: HP amp enable (bit 7) + output enable (bit 6)
        self.codec_command(self.pin_node, set_pin_widget_ctl(0xC0));

        // Try to enable EAPD if supported
        self.codec_command(self.pin_node, set_eapd_btl(0x02));

        // Connect pin to our DAC if it has a connection list
        let conn_list_len = self.codec_command(self.pin_node, get_param(PARAM_CONN_LIST_LEN));
        let num_conns = (conn_list_len & 0x7F) as u8;
        if num_conns > 1 {
            'outer: for ci in 0..num_conns {
                let conn_entry = self.codec_command(self.pin_node, get_conn_list_entry(ci as u32));
                // Each response holds up to 4 connection NIDs (short form)
                for j in 0..4u8 {
                    if ci + j >= num_conns {
                        break;
                    }
                    let conn_nid = ((conn_entry >> (j * 8)) & 0xFF) as u8;
                    if conn_nid == self.dac_node {
                        self.codec_command(self.pin_node, set_conn_select((ci + j) as u32));
                        break 'outer;
                    }
                }
            }
        }

        // Stream format on DAC: 48kHz, 16-bit, stereo
        let fmt = 0x0011u32;
        self.codec_command(self.dac_node, set_converter_format(fmt));

        // Stream/channel: stream tag = STREAM_ID, channel = 0
        self.codec_command(self.dac_node, set_stream_channel((STREAM_ID << 4) | 0));

        // Unmute DAC output amp at max gain
        let amp_cap = self.codec_command(self.dac_node, get_param(PARAM_OUT_AMP_CAP));
        let mut num_steps = ((amp_cap >> 8) & 0x7F) as u32;
        if num_steps == 0 {
            num_steps = 0x7F;
        }
        // bit 15=output, bit 13=left, bit 12=right, bits 6:0=gain
        self.codec_command(self.dac_node, set_amp_gain_mute(0xB000 | num_steps));

        // Also unmute the pin's output amp if present
        let pin_widget_cap = self.codec_command(self.pin_node, get_param(PARAM_AUDIO_WIDGET_CAP));
        if pin_widget_cap & (1 << 2) != 0 {
            let pin_amp_cap = self.codec_command(self.pin_node, get_param(PARAM_OUT_AMP_CAP));
            let mut pin_steps = ((pin_amp_cap >> 8) & 0x7F) as u32;
            if pin_steps == 0 {
                pin_steps = 0x7F;
            }
            self.codec_command(self.pin_node, set_amp_gain_mute(0xB000 | pin_steps));
        }

        true
    }

    // ---- Output stream setup ----

    fn setup_output_stream(&mut self) -> bool {
        // GCAP bits: [15:12]=OSS, [11:8]=ISS; output stream 0 base = 0x80 + ISS * 0x20
        let gcap = self.reg_read16(HDA_GCAP);
        let num_iss = ((gcap >> 8) & 0x0F) as u32;
        self.osd_base = 0x80 + num_iss * 0x20;

        // Stop stream if running
        self.reg_write8(self.osd_base + SD_CTL, 0);
        stall_us(10000);

        // Reset stream
        self.reg_write8(self.osd_base + SD_CTL, 0x01); // SRST
        stall_us(10000);
        for _ in 0..100 {
            if self.reg_read8(self.osd_base + SD_CTL) & 0x01 != 0 {
                break;
            }
            stall_us(1000);
        }

        // Clear reset
        self.reg_write8(self.osd_base + SD_CTL, 0x00);
        stall_us(10000);
        for _ in 0..100 {
            if self.reg_read8(self.osd_base + SD_CTL) & 0x01 == 0 {
                break;
            }
            stall_us(1000);
        }

        // BDL with NUM_CHUNKS entries pointing at each chunk of the PCM buffer
        unsafe {
            let pcm = self.pcm_buffer as u64;
            for i in 0..NUM_CHUNKS {
                let entry = self.bdl.add(i);
                (*entry).address = pcm + (i as u64) * CHUNK_SAMPLES as u64 * 4;
                (*entry).length = CHUNK_SAMPLES * 4; // stereo s16le
                (*entry).ioc = 1;
            }
        }

        // Cyclic Buffer Length = total bytes across all BDL entries
        self.reg_write32(self.osd_base + SD_CBL, PCM_BUF_BYTES);

        // Last Valid Index = NUM_CHUNKS - 1
        self.reg_write16(self.osd_base + SD_LVI, (NUM_CHUNKS - 1) as u16);

        // Format: 48kHz, 16-bit, stereo
        self.reg_write16(self.osd_base + SD_FMT, 0x0011);

        // BDL address
        let bdl_phys = self.bdl as u64;
        self.reg_write32(self.osd_base + SD_BDPL, (bdl_phys & 0xFFFF_FFFF) as u32);
        self.reg_write32(self.osd_base + SD_BDPU, (bdl_phys >> 32) as u32);

        // Stream number in CTL byte 2 (bits 7:4)
        self.reg_write8(self.osd_base + SD_CTL + 2, ((STREAM_ID & 0x0F) << 4) as u8);

        true
    }

    // ---- Waveform generation ----

    fn fill_tone_buffer(&self, frequency: u32) {
        if self.pcm_buffer.is_null() || frequency == 0 {
            return;
        }
        for i in 0..NUM_CHUNKS {
            self.fill_tone_chunk(i, frequency);
        }
    }

    fn fill_tone_chunk(&self, chunk: usize, frequency: u32) {
        let mut half_period = SAMPLE_RATE / (frequency * 2);
        if half_period == 0 {
            half_period = 1;
        }
        let amplitude: i16 = 8000; // moderate volume
        let base = unsafe { self.pcm_buffer.add(chunk * CHUNK_SAMPLES as usize * 2) };

        for i in 0..CHUNK_SAMPLES {
            let pos_in_period = i % (half_period * 2);
            let sample = if pos_in_period < half_period { amplitude } else { -amplitude };
            unsafe {
                // Stereo: left and right channel
                base.add((i * 2) as usize).write(sample);
                base.add((i * 2 + 1) as usize).write(sample);
            }
        }
    }

    // ---- Public interface ----

    pub fn initialize(&mut self) -> Option<u8> {
        self.available = false;

        log_info!("HDA: Scanning PCI...");
        if !self.find_controller() {
            log_info!("HDA: No controller found");
            return None;
        }
        let dev = self.pci_device.expect("HDA controller missing after find_controller");
        log_info!("HDA: Found at PCI {:02x}:{:02x}.{}", dev.bus, dev.device, dev.function);

        // Capture the PCI interrupt line before we start the controller.
        self.irq = pci::read_interrupt_line(dev.bus, dev.device, dev.function);
        log_debug!("HDA: IRQ = {}", self.irq);

        // BAR0 (MMIO base)
        let bar0 = pci::read_bar(dev.bus, dev.device, dev.function, 0);
        if pci::bar_type(bar0) == pci::BarType::Io {
            log_warn!("HDA: BAR0 is I/O space, not MMIO");
            return None;
        }
        let mmio_base = pci::bar_phys_addr(dev.bus, dev.device, dev.function, 0)
            .expect("HDA: BAR0 invalid");
        self.mmio = mmio_base as usize as *mut u8;
        log_debug!("HDA: MMIO base = {:#x}", mmio_base);

        // Enable PCI memory space + bus mastering
        let cmd = pci::read_command(dev.bus, dev.device, dev.function);
        pci::write_command(dev.bus, dev.device, dev.function, cmd | 0x06);

        if !self.reset_controller() {
            log_warn!("HDA: Reset failed");
            return None;
        }
        let gcap = self.reg_read16(HDA_GCAP);
        log_debug!("HDA: GCAP = {:#x}", gcap);
        log_debug!("HDA: STATESTS = {:#x}", self.reg_read16(HDA_STATESTS));

        // Enable controller interrupts: global interrupt enable (bit 31) plus
        // output stream 0 interrupt. The actual stream IOC enable is in SD_CTL.
        let iss = ((gcap >> 8) & 0x0F) as u32;
        self.reg_write32(HDA_INTCTL, (1u32 << 31) | (1u32 << iss));

        if !self.alloc_dma_buffers() {
            log_warn!("HDA: DMA buffer setup failed");
            return None;
        }
        log_info!("HDA: DMA buffers ok");

        if !self.discover_codec() {
            log_warn!("HDA: Codec discovery failed");
            return None;
        }
        log_debug!("HDA: Codec addr = {:#x}", self.codec_addr);
        log_debug!("HDA: DAC node   = {:#x}", self.dac_node);
        log_debug!("HDA: Pin node   = {:#x}", self.pin_node);

        if !self.configure_codec() {
            log_warn!("HDA: Codec config failed");
            return None;
        }
        log_info!("HDA: Codec configured");

        if !self.setup_output_stream() {
            log_warn!("HDA: Stream setup failed");
            return None;
        }
        log_debug!("HDA: Stream base = {:#x}", self.osd_base);

        self.available = true;
        register_device();
        log_info!("HDA: Initialized successfully!");
        Some(self.irq)
    }

    pub fn play_frequency(&mut self, frequency: u32) {
        if !self.available {
            return;
        }
        if !(20..=20000).contains(&frequency) {
            self.stop();
            return;
        }

        self.fill_tone_buffer(frequency);

        // Stop stream if already running
        let ctl0 = self.reg_read8(self.osd_base + SD_CTL);
        if ctl0 & 0x02 != 0 {
            self.reg_write8(self.osd_base + SD_CTL, ctl0 & !0x02u8);
            stall_us(5000);
        }

        // Clear status bits
        self.reg_write8(self.osd_base + SD_STS, 0x07);

        // Ensure stream number is set in CTL byte 2 before starting
        self.reg_write8(self.osd_base + SD_CTL + 2, ((STREAM_ID & 0x0F) << 4) as u8);

        // Start stream: RUN (bit 1) + IOC interrupt enable (bit 2)
        self.reg_write8(self.osd_base + SD_CTL, 0x06);
    }

    pub fn stop(&mut self) {
        if !self.available {
            return;
        }
        let ctl = self.reg_read8(self.osd_base + SD_CTL);
        self.reg_write8(self.osd_base + SD_CTL, ctl & !0x02u8);
    }

    // ---- Streaming interface ----

    pub fn set_stream_channels(&mut self, channels: u8) {
        self.stream_channels = if channels == 1 { 1 } else { 2 };
    }

    fn stream_is_running(&self) -> bool {
        if self.osd_base == 0 {
            return false;
        }
        self.reg_read8(self.osd_base + SD_CTL) & 0x02 != 0
    }

    fn space_available(&self) -> bool {
        if !self.available {
            return false;
        }
        // Keep one chunk ahead of the hardware: allow up to NUM_CHUNKS - 1
        // chunks to be queued.
        self.chunks_written - self.chunks_completed < (NUM_CHUNKS - 1) as u64
    }

    pub fn write_stream(&mut self, buf: &[u8]) -> usize {
        if !self.available || buf.is_empty() {
            return 0;
        }

        let channels = self.stream_channels.max(1).min(2);
        let bytes_per_frame = 2 * channels as usize; // s16le
        if buf.len() < bytes_per_frame {
            return 0;
        }

        if !self.space_available() {
            return 0;
        }

        let chunk = (self.chunks_written % NUM_CHUNKS as u64) as usize;
        let target_base = unsafe { self.pcm_buffer.add(chunk * CHUNK_SAMPLES as usize * 2) };

        let input_frames = (buf.len() / bytes_per_frame).min(CHUNK_SAMPLES as usize);

        if channels == 1 {
            // Mono input: duplicate to stereo output.
            for i in 0..input_frames {
                let sample = i16::from_le_bytes([buf[i * 2], buf[i * 2 + 1]]);
                unsafe {
                    target_base.add(i * 2).write(sample);
                    target_base.add(i * 2 + 1).write(sample);
                }
            }
        } else {
            // Stereo input: copy directly.
            unsafe {
                core::ptr::copy_nonoverlapping(
                    buf.as_ptr(),
                    target_base as *mut u8,
                    input_frames * 4,
                );
            }
        }

        // Pad any remaining frames with silence.
        for i in input_frames..CHUNK_SAMPLES as usize {
            unsafe {
                target_base.add(i * 2).write(0);
                target_base.add(i * 2 + 1).write(0);
            }
        }

        unsafe {
            flush_pcm_chunk(target_base, CHUNK_SAMPLES as usize * 4);
        }

        // Diagnostics: log every 40 chunks to see streaming health.
        if self.chunks_written % 40 == 0 {
            log_debug!(
                "HDA: written={} completed={} irq={} fifo_err={} desc_err={}",
                self.chunks_written,
                self.chunks_completed,
                self.irq_count,
                self.fifo_errors,
                self.desc_errors
            );
        }

        self.chunks_written += 1;

        if !self.stream_is_running() {
            // Ensure stream number is set before starting.
            self.reg_write8(self.osd_base + SD_CTL + 2, ((STREAM_ID & 0x0F) << 4) as u8);
            // Start stream: RUN (bit 1) + IOC interrupt enable (bit 2)
            self.reg_write8(self.osd_base + SD_CTL, 0x06);
            log_debug!(
                "HDA: stream started chunk={} ctl={:#x}",
                chunk,
                self.reg_read8(self.osd_base + SD_CTL)
            );
        }

        input_frames * bytes_per_frame
    }

    pub fn wait_for_space(&mut self) -> i64 {
        if !self.available {
            return 0;
        }

        let caller = crate::scheduler::current_user_pid().unwrap_or(0);
        if caller == 0 {
            return 0;
        }

        // Register as a waiter first, then check. This avoids the lost-wake-up
        // race where an interrupt fires between the check and going to sleep.
        unsafe {
            let waiters = &mut *HDA_WAITERS.0.get();
            let mut added = false;
            for slot in waiters.iter_mut() {
                if slot.is_none() {
                    *slot = Some(caller);
                    added = true;
                    break;
                }
            }
            if !added {
                return 0;
            }
        }

        if self.space_available() {
            // Space is already available: remove ourselves and return.
            unsafe {
                let waiters = &mut *HDA_WAITERS.0.get();
                for slot in waiters.iter_mut() {
                    if *slot == Some(caller) {
                        *slot = None;
                        break;
                    }
                }
            }
            return 0;
        }

        crate::process::set_wait_target(caller, crate::process::WaitTarget::Audio);
        crate::process::set_sleeping(caller);
        crate::syscall::BLOCK_TO_SCHEDULER as i64
    }

    pub fn on_interrupt(&mut self) {
        if self.irq_count < 5 {
            log_debug!("HDA: on_interrupt entered available={} osd_base={:#x}", self.available, self.osd_base);
        }
        if !self.available || self.osd_base == 0 {
            return;
        }
        // Clear controller interrupt status.
        const HDA_INTSTS: u32 = 0x24;
        self.reg_write32(HDA_INTSTS, self.reg_read32(HDA_INTSTS));

        let sts = self.reg_read8(self.osd_base + SD_STS);
        // Clear stream status bits (FIFOE=0, BCIS=1, DESE=2). Writing a 1 to
        // R/WC bits clears them; write all known status bits so the IRQ can
        // re-trigger on the next buffer completion.
        self.reg_write8(self.osd_base + SD_STS, 0x07);

        // BCIS (buffer completion interrupt status) is the normal IOC path.
        // Only count a completion when BCIS is set; other status bits are
        // errors we want to track.
        if sts & 0x02 != 0 {
            self.chunks_completed += 1;
        }
        if sts & 0x01 != 0 {
            self.fifo_errors += 1;
        }
        if sts & 0x04 != 0 {
            self.desc_errors += 1;
        }

        self.irq_count += 1;
        if self.irq_count % 40 == 0 {
            log_debug!(
                "HDA IRQ: written={} completed={} irq={} sts={:#x} fifo_err={} desc_err={}",
                self.chunks_written,
                self.chunks_completed,
                self.irq_count,
                sts,
                self.fifo_errors,
                self.desc_errors
            );
        }

        unsafe {
            let waiters = &mut *HDA_WAITERS.0.get();
            for slot in waiters.iter_mut() {
                if let Some(pid) = *slot {
                    crate::process::set_ready(pid);
                    *slot = None;
                }
            }
        }
    }
}

fn stall_us(us: u32) {
    crate::drivers::pit::sleep_us(us);
}

#[cfg(target_arch = "x86_64")]
unsafe fn clflush(addr: *const u8) {
    unsafe { core::arch::x86_64::_mm_clflush(addr); }
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn clflush(_addr: *const u8) {}

/// Flush the cache lines covering a PCM chunk so the DMA engine sees the
/// latest samples. x86 DMA is normally snooped, but explicit flushing removes
/// any remaining coherency doubts on real hardware / QEMU edge cases.
unsafe fn flush_pcm_chunk(target: *mut i16, bytes: usize) {
    unsafe {
        let base = target as *const u8;
        let mut off = 0usize;
        while off < bytes {
            clflush(base.add(off));
            off += 64;
        }
    }
}

// ---- /dev/audio device interface ----

static AUDIO_OPS: devfs::DeviceOps = devfs::DeviceOps {
    open: audio_open,
    close: audio_close,
    read: audio_read,
    write: audio_write,
    ioctl: audio_ioctl,
};

fn audio_open() -> u64 {
    0
}

fn audio_close(_handle: u64) {
    stop();
}

fn audio_read(_handle: u64, _buf: &mut [u8]) -> usize {
    0
}

fn audio_write(_handle: u64, buf: &[u8]) -> usize {
    write_stream(buf)
}

fn audio_ioctl(_handle: u64, cmd: u64, arg: u64) -> i64 {
    match cmd {
        0 => {
            stop();
            0
        }
        1 => {
            play_frequency(arg as u32);
            0
        }
        2 => {
            set_stream_channels(arg as u8);
            0
        }
        3 => wait_for_space(),
        _ => -1,
    }
}

pub fn register_device() {
    devfs::register("/dev/audio", &AUDIO_OPS);
}
