#![no_std]
#![no_main]
include!("../runtime/driver_runtime.rs");

const HDA_GCAP: u32 = 0x00;
const HDA_GCTL: u32 = 0x08;
const HDA_STATESTS: u32 = 0x0e;
const HDA_INTCTL: u32 = 0x20;
const HDA_INTSTS: u32 = 0x24;
const HDA_ICO: u32 = 0x60;
const HDA_IRI: u32 = 0x64;
const HDA_ICS: u32 = 0x68;
const SD_CTL: u32 = 0x00;
const SD_STS: u32 = 0x03;
const SD_LPIB: u32 = 0x04;
const SD_CBL: u32 = 0x08;
const SD_LVI: u32 = 0x0c;
const SD_FMT: u32 = 0x12;
const SD_BDPL: u32 = 0x18;
const SD_BDPU: u32 = 0x1c;
const CHUNK_FRAMES: usize = 1200;
const CHUNKS: usize = 4;
const CHUNK_BYTES: usize = CHUNK_FRAMES * 4;
const PCM_BYTES: usize = CHUNK_BYTES * CHUNKS;
const STREAM_ID: u32 = 1;

const PARAM_SUB_NODE_COUNT: u32 = 0x04;
const PARAM_FN_GROUP_TYPE: u32 = 0x05;
const PARAM_AUDIO_WIDGET_CAP: u32 = 0x09;
const PARAM_PIN_CAP: u32 = 0x0c;
const PARAM_CONN_LIST_LEN: u32 = 0x0e;
const PARAM_OUT_AMP_CAP: u32 = 0x12;

#[repr(C, packed)]
struct BdlEntry {
    address: u64,
    length: u32,
    ioc: u32,
}

struct Hda {
    bdf: u32,
    mmio: *mut u8,
    bdl: *mut BdlEntry,
    bdl_phys: u64,
    pcm: *mut i16,
    pcm_phys: u64,
    codec: u8,
    dac: u8,
    pin: u8,
    stream_base: u32,
    channels: u8,
    written: u64,
    completed: u64,
    last_lpib: u32,
    wraps: u64,
}

static mut HDA: Option<Hda> = None;
static mut COMMAND_CH: u64 = u64::MAX;
static IRQ_NUMBER: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
static IRQ_RUNNING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static IRQ_MMIO: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static IRQ_STREAM_BASE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static IRQ_TID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static IRQ_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

pub fn kdm_info() -> KdmInfo {
    KdmInfo { name: "hda", depends: &[] }
}

fn bdf(info: &PciDeviceInfo) -> u32 {
    ((info.bus as u32) << 16) | ((info.device as u32) << 8) | info.function as u32
}

fn find_hda() -> Option<u32> {
    let mut index = 0;
    loop {
        let mut info = PciDeviceInfo::default();
        let total = sys_pci_info(index, &mut info);
        if total == u64::MAX { return None; }
        if info.class_code == 0x04 && info.subclass == 0x03 { return Some(bdf(&info)); }
        index += 1;
        if index >= total { return None; }
    }
}

fn sleep_us(us: u64) {
    let ms = (us + 999) / 1000;
    sys_sleep(ms.max(1));
}

impl Hda {
    fn read8(&self, off: u32) -> u8 { unsafe { self.mmio.add(off as usize).read_volatile() } }
    fn read16(&self, off: u32) -> u16 { unsafe { (self.mmio.add(off as usize) as *const u16).read_volatile() } }
    fn read32(&self, off: u32) -> u32 { unsafe { (self.mmio.add(off as usize) as *const u32).read_volatile() } }
    fn write8(&self, off: u32, value: u8) { unsafe { self.mmio.add(off as usize).write_volatile(value) } }
    fn write16(&self, off: u32, value: u16) { unsafe { (self.mmio.add(off as usize) as *mut u16).write_volatile(value) } }
    fn write32(&self, off: u32, value: u32) { unsafe { (self.mmio.add(off as usize) as *mut u32).write_volatile(value) } }

    fn reset(&self) -> bool {
        self.write32(HDA_GCTL, self.read32(HDA_GCTL) & !1);
        sleep_us(100);
        for _ in 0..100 {
            if self.read32(HDA_GCTL) & 1 == 0 { break; }
            sleep_us(1000);
        }
        self.write32(HDA_GCTL, self.read32(HDA_GCTL) | 1);
        sleep_us(100);
        for _ in 0..100 {
            if self.read32(HDA_GCTL) & 1 != 0 { break; }
            sleep_us(1000);
        }
        if self.read32(HDA_GCTL) & 1 == 0 { return false; }
        sleep_us(500_000);
        self.read16(HDA_STATESTS) != 0
    }

    fn verb(&self, value: u32) -> u32 {
        for _ in 0..1000 {
            if self.read16(HDA_ICS) & 1 == 0 { break; }
            sleep_us(100);
        }
        if self.read16(HDA_ICS) & 1 != 0 { return 0; }
        self.write16(HDA_ICS, 2);
        self.write32(HDA_ICO, value);
        self.write16(HDA_ICS, self.read16(HDA_ICS) | 1);
        for _ in 0..10_000 {
            sleep_us(100);
            let status = self.read16(HDA_ICS);
            if status & 1 == 0 && status & 2 != 0 {
                let response = self.read32(HDA_IRI);
                self.write16(HDA_ICS, 2);
                return response;
            }
        }
        0
    }

    fn command(&self, node: u8, verb: u32) -> u32 {
        self.verb(((self.codec as u32) << 28) | ((node as u32) << 20) | verb)
    }

    fn discover(&mut self) -> bool {
        let states = self.read16(HDA_STATESTS);
        self.codec = (0..15).find(|i| states & (1 << i) != 0).unwrap_or(0xff) as u8;
        if self.codec == 0xff { return false; }
        let root = self.command(0, 0xf0000 | PARAM_SUB_NODE_COUNT);
        let start = (root >> 16) as u8;
        let count = root as u8;
        let mut afg = 0;
        for node in start..start.saturating_add(count) {
            if self.command(node, 0xf0000 | PARAM_FN_GROUP_TYPE) & 0xff == 1 {
                afg = node;
                break;
            }
        }
        if afg == 0 { return false; }
        self.command(afg, 0x70500);
        sleep_us(10_000);
        let nodes = self.command(afg, 0xf0000 | PARAM_SUB_NODE_COUNT);
        let start = (nodes >> 16) as u8;
        let count = nodes as u8;
        for node in start..start.saturating_add(count) {
            let cap = self.command(node, 0xf0000 | PARAM_AUDIO_WIDGET_CAP);
            match (cap >> 20) & 0xf {
                0 if self.dac == 0 => self.dac = node,
                4 if self.pin == 0 && self.command(node, 0xf0000 | PARAM_PIN_CAP) & (1 << 4) != 0 => self.pin = node,
                _ => {}
            }
        }
        self.dac != 0 && self.pin != 0
    }

    fn configure_codec(&self) {
        self.command(self.dac, 0x70500);
        self.command(self.pin, 0x70500);
        sleep_us(10_000);
        self.command(self.pin, 0x707c0);
        self.command(self.pin, 0x70c02);
        let connections = (self.command(self.pin, 0xf0000 | PARAM_CONN_LIST_LEN) & 0x7f) as u8;
        for index in 0..connections {
            let entries = self.command(self.pin, 0xf0200 | index as u32);
            for offset in 0..4u8 {
                if ((entries >> (offset * 8)) & 0xff) as u8 == self.dac {
                    self.command(self.pin, 0x70100 | (index + offset) as u32);
                }
            }
        }
        self.command(self.dac, 0x20011);
        self.command(self.dac, 0x70600 | (STREAM_ID << 4));
        let steps = ((self.command(self.dac, 0xf0000 | PARAM_OUT_AMP_CAP) >> 8) & 0x7f).max(1);
        self.command(self.dac, 0x30000 | 0xb000 | steps);
    }

    fn setup_stream(&mut self) {
        let inputs = ((self.read16(HDA_GCAP) >> 8) & 0xf) as u32;
        self.stream_base = 0x80 + inputs * 0x20;
        self.write8(self.stream_base + SD_CTL, 0);
        sleep_us(10_000);
        self.write8(self.stream_base + SD_CTL, 1);
        sleep_us(10_000);
        self.write8(self.stream_base + SD_CTL, 0);
        sleep_us(10_000);
        unsafe {
            for index in 0..CHUNKS {
                let entry = self.bdl.add(index);
                (*entry).address = self.pcm_phys + (index * CHUNK_BYTES) as u64;
                (*entry).length = CHUNK_BYTES as u32;
                (*entry).ioc = 1;
            }
        }
        self.write32(self.stream_base + SD_CBL, PCM_BYTES as u32);
        self.write16(self.stream_base + SD_LVI, (CHUNKS - 1) as u16);
        self.write16(self.stream_base + SD_FMT, 0x11);
        self.write32(self.stream_base + SD_BDPL, self.bdl_phys as u32);
        self.write32(self.stream_base + SD_BDPU, (self.bdl_phys >> 32) as u32);
        self.write8(self.stream_base + SD_CTL + 2, (STREAM_ID << 4) as u8);
    }

    fn refresh(&mut self) {
        let position = self.read32(self.stream_base + SD_LPIB) % PCM_BYTES as u32;
        if position < self.last_lpib { self.wraps += 1; }
        self.last_lpib = position;
        self.completed = (self.wraps * PCM_BYTES as u64 + position as u64) / CHUNK_BYTES as u64;
    }

    fn write_pcm(&mut self, data: &[u8]) -> usize {
        self.refresh();
        if self.written.saturating_sub(self.completed) >= (CHUNKS - 1) as u64 { return 0; }
        let input_channels = self.channels.clamp(1, 2) as usize;
        let frame_bytes = input_channels * 2;
        let frames = (data.len() / frame_bytes).min(CHUNK_FRAMES);
        let chunk = (self.written % CHUNKS as u64) as usize;
        let target = unsafe { self.pcm.add(chunk * CHUNK_FRAMES * 2) };
        for frame in 0..frames {
            let sample = i16::from_le_bytes([data[frame * frame_bytes], data[frame * frame_bytes + 1]]);
            unsafe {
                if input_channels == 1 {
                    target.add(frame * 2).write(sample);
                    target.add(frame * 2 + 1).write(sample);
                } else {
                    core::ptr::copy_nonoverlapping(data.as_ptr().add(frame * 4), target.add(frame * 2) as *mut u8, 4);
                }
            }
        }
        unsafe { core::ptr::write_bytes(target.add(frames * 2), 0, (CHUNK_FRAMES - frames) * 2); }
        self.written += 1;
        if self.read8(self.stream_base + SD_CTL) & 2 == 0 {
            self.write8(self.stream_base + SD_STS, 7);
            self.write8(self.stream_base + SD_CTL, 6);
        }
        frames * frame_bytes
    }

    fn stop(&mut self) {
        if self.stream_base == 0 { return; }
        let control = self.read8(self.stream_base + SD_CTL);
        self.write8(self.stream_base + SD_CTL, control & !2);
        for _ in 0..100 {
            if self.read8(self.stream_base + SD_CTL) & 2 == 0 { break; }
            sleep_us(1000);
        }
        self.written = 0;
        self.completed = 0;
        self.last_lpib = 0;
        self.wraps = 0;
    }

    fn release(mut self) {
        self.stop();
        self.write32(HDA_INTCTL, 0);
        sys_pci_disable(self.bdf);
        sys_pci_bar_unmap(self.mmio as u64);
        sys_dma_free(self.bdl as *mut u8);
        sys_dma_free(self.pcm as *mut u8);
    }
}

extern "C" fn irq_worker(_arg: u64) -> ! {
    use core::sync::atomic::Ordering;
    loop {
        let irq = IRQ_NUMBER.load(Ordering::Acquire);
        if irq == 0 || !IRQ_RUNNING.load(Ordering::Acquire) { break; }
        syscall(SYS_IRQ_WAIT, irq as u64, 0, 0);
        if !IRQ_RUNNING.load(Ordering::Acquire) { break; }
        IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        let mmio = IRQ_MMIO.load(Ordering::Acquire) as *mut u8;
        let stream = IRQ_STREAM_BASE.load(Ordering::Acquire);
        if !mmio.is_null() && stream != 0 {
            unsafe {
                let intsts = (mmio.add(HDA_INTSTS as usize) as *const u32).read_volatile();
                (mmio.add(HDA_INTSTS as usize) as *mut u32).write_volatile(intsts);
                mmio.add((stream + SD_STS) as usize).write_volatile(7);
            }
            sys_irq_ack(irq);
        }
    }
    sys_thread_exit()
}

pub fn kdm_init() -> bool {
    let Some(bdf) = find_hda() else {
        println!("hda: no controller");
        return false;
    };
    let mmio = sys_pci_bar_map(bdf, 0);
    if mmio == u64::MAX { return false; }
    if sys_pci_enable(bdf) == u64::MAX {
        sys_pci_bar_unmap(mmio);
        return false;
    }
    let Some((bdl, bdl_phys)) = sys_dma_alloc(4096) else {
        sys_pci_bar_unmap(mmio);
        return false;
    };
    let Some((pcm, pcm_phys)) = sys_dma_alloc(PCM_BYTES) else {
        sys_dma_free(bdl);
        sys_pci_bar_unmap(mmio);
        return false;
    };
    let mut hda = Hda {
        bdf,
        mmio: mmio as *mut u8,
        bdl: bdl as *mut BdlEntry,
        bdl_phys,
        pcm: pcm as *mut i16,
        pcm_phys,
        codec: 0xff,
        dac: 0,
        pin: 0,
        stream_base: 0,
        channels: 2,
        written: 0,
        completed: 0,
        last_lpib: 0,
        wraps: 0,
    };
    hda.write32(HDA_INTCTL, 0);
    if !hda.reset() || !hda.discover() {
        println!("hda: controller initialization failed");
        hda.release();
        return false;
    }
    hda.configure_codec();
    hda.setup_stream();
    let Some(irq) = sys_irq_claim(bdf) else {
        hda.release();
        return false;
    };
    use core::sync::atomic::Ordering;
    IRQ_NUMBER.store(irq, Ordering::Release);
    IRQ_MMIO.store(mmio, Ordering::Release);
    IRQ_STREAM_BASE.store(hda.stream_base, Ordering::Release);
    IRQ_COUNT.store(0, Ordering::Release);
    IRQ_RUNNING.store(true, Ordering::Release);
    let stack = sys_heap_alloc(64 * 1024);
    if stack == u64::MAX {
        IRQ_RUNNING.store(false, Ordering::Release);
        sys_irq_release(irq);
        hda.release();
        return false;
    }
    let stack_top = ((stack + 64 * 1024) & !0xf) - 8;
    let irq_tid = sys_thread_spawn(irq_worker, 0, stack_top);
    if irq_tid == 0 {
        IRQ_RUNNING.store(false, Ordering::Release);
        sys_irq_release(irq);
        hda.release();
        return false;
    }
    IRQ_TID.store(irq_tid, Ordering::Release);
    let stream_index = ((hda.read16(HDA_GCAP) >> 8) & 0x0f) as u32;
    hda.write32(HDA_INTCTL, (1 << 31) | (1 << stream_index));
    let channel = sys_ipc_open(b"driver_audio");
    if channel == u64::MAX {
        IRQ_RUNNING.store(false, Ordering::Release);
        sys_irq_release(irq);
        sys_thread_join(irq_tid);
        hda.release();
        return false;
    }
    unsafe {
        HDA = Some(hda);
        COMMAND_CH = channel;
    }
    true
}

pub fn kdm_run() {
    let mut message = [0u8; 8192];
    loop {
        if sys_signal_check() { return; }
        let channel = unsafe { COMMAND_CH };
        let length = sys_ipc_recv(channel, &mut message);
        if length == u64::MAX { return; }
        let data = &message[..length as usize];
        if data.is_empty() { continue; }
        unsafe {
            let state = core::ptr::addr_of_mut!(HDA);
            let Some(hda) = (&mut *state).as_mut() else { return; };
            match data[0] {
                0 => hda.stop(),
                1 if data.len() >= 2 => hda.channels = data[1].clamp(1, 2),
                2 => {
                    while hda.write_pcm(&data[1..]) == 0 {
                        if sys_signal_check() { return; }
                        sys_sleep(1);
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn kdm_exit() {
    unsafe {
        let state = core::ptr::addr_of_mut!(HDA);
        let current = core::ptr::read(state);
        core::ptr::write(state, None);
        let mut current = current;
        if let Some(hda) = current.as_mut() {
            hda.stop();
            hda.write32(HDA_INTCTL, 0);
        }
        use core::sync::atomic::Ordering;
        IRQ_RUNNING.store(false, Ordering::Release);
        let irq = IRQ_NUMBER.swap(0, Ordering::AcqRel);
        if irq != 0 { sys_irq_release(irq); }
        let tid = IRQ_TID.swap(0, Ordering::AcqRel);
        if tid != 0 { sys_thread_join(tid); }
        IRQ_MMIO.store(0, Ordering::Release);
        IRQ_STREAM_BASE.store(0, Ordering::Release);
        if let Some(hda) = current { hda.release(); }
        if COMMAND_CH != u64::MAX { syscall(SYS_IPC_CLOSE, COMMAND_CH, 0, 0); }
    }
}
