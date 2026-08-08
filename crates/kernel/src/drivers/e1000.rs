use crate::drivers::pci::{self, ScanKind};
use crate::pmm;
use crate::util::SyncUnsafeCell;
use crate::{log_info, log_warn};

const VENDOR_INTEL: u16 = 0x8086;
const SUPPORTED_DEVICE_IDS: [u16; 4] = [0x100E, 0x100F, 0x1004, 0x10D3];

const REG_CTRL: u32 = 0x0000;
const REG_STATUS: u32 = 0x0008;
const REG_EERD: u32 = 0x0014;
const REG_ICR: u32 = 0x00C0;
const REG_IMC: u32 = 0x00D8;
const REG_RCTL: u32 = 0x0100;
const REG_TCTL: u32 = 0x0400;
const REG_TIPG: u32 = 0x0410;
const REG_RDBAL: u32 = 0x2800;
const REG_RDBAH: u32 = 0x2804;
const REG_RDLEN: u32 = 0x2808;
const REG_RDH: u32 = 0x2810;
const REG_RDT: u32 = 0x2818;
const REG_TDBAL: u32 = 0x3800;
const REG_TDBAH: u32 = 0x3804;
const REG_TDLEN: u32 = 0x3808;
const REG_TDH: u32 = 0x3810;
const REG_TDT: u32 = 0x3818;
const REG_MTA: u32 = 0x5200;
const REG_RAL0: u32 = 0x5400;
const REG_RAH0: u32 = 0x5404;

const CTRL_SLU: u32 = 1 << 6;
const CTRL_ASDE: u32 = 1 << 5;
const CTRL_RST: u32 = 1 << 26;
const CTRL_LRST: u32 = 1 << 3;
const CTRL_PHY_RST: u32 = 1 << 31;
const CTRL_ILOS: u32 = 1 << 7;
const CTRL_VME: u32 = 1 << 30;

const STATUS_LU: u32 = 1 << 1;

const RAH_AV: u32 = 1 << 31;

const EERD_START: u32 = 1 << 0;
const EERD_DONE: u32 = 1 << 4;

const RCTL_EN: u32 = 1 << 1;
const RCTL_BAM: u32 = 1 << 15;
const RCTL_SECRC: u32 = 1 << 26;
const RCTL_BSIZE_2048: u32 = 0;

const TCTL_EN: u32 = 1 << 1;
const TCTL_PSP: u32 = 1 << 3;
const TCTL_CT_SHIFT: u32 = 4;
const TCTL_COLD_SHIFT: u32 = 12;
const TIPG_DEFAULT: u32 = 0x0060_200A;

const RX_STATUS_DD: u8 = 1 << 0;
const TX_CMD_EOP: u8 = 1 << 0;
const TX_CMD_IFCS: u8 = 1 << 1;
const TX_CMD_RS: u8 = 1 << 3;
const TX_STATUS_DD: u8 = 1 << 0;

const RX_DESC_COUNT: usize = 32;
const TX_DESC_COUNT: usize = 8;
const BUFFER_SIZE: usize = 2048;

#[repr(C)]
#[derive(Clone, Copy)]
struct RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

struct E1000 {
    available: bool,
    mmio: *mut u8,
    mac: [u8; 6],
    rx_ring: *mut RxDesc,
    tx_ring: *mut TxDesc,
    rx_buffers: u64,
    tx_buffers: u64,
    rx_cur: usize,
    tx_cur: usize,
}

unsafe impl Send for E1000 {}

impl E1000 {
    const fn new() -> Self {
        Self {
            available: false,
            mmio: core::ptr::null_mut(),
            mac: [0; 6],
            rx_ring: core::ptr::null_mut(),
            tx_ring: core::ptr::null_mut(),
            rx_buffers: 0,
            tx_buffers: 0,
            rx_cur: 0,
            tx_cur: 0,
        }
    }

    fn reg_read(&self, off: u32) -> u32 {
        unsafe { (self.mmio.add(off as usize) as *const u32).read_volatile() }
    }

    fn reg_write(&self, off: u32, val: u32) {
        unsafe { (self.mmio.add(off as usize) as *mut u32).write_volatile(val) }
    }

    fn find_device(&self) -> Option<pci::Device> {
        let mut found = None;
        pci::scan(ScanKind::Pci, |dev| {
            if found.is_none()
                && dev.vendor_id == VENDOR_INTEL
                && dev.class_code == 0x02
                && dev.subclass == 0x00
                && SUPPORTED_DEVICE_IDS.contains(&dev.device_id)
            {
                found = Some(dev);
            }
        });
        found
    }

    fn read_eeprom(&self, word: u8) -> u16 {
        self.reg_write(REG_EERD, EERD_START | ((word as u32) << 8));
        loop {
            let val = self.reg_read(REG_EERD);
            if val & EERD_DONE != 0 {
                return (val >> 16) as u16;
            }
            crate::util::pause();
        }
    }

    fn read_mac(&mut self) {
        let ral = self.reg_read(REG_RAL0);
        let rah = self.reg_read(REG_RAH0);
        if rah & RAH_AV != 0 || ral != 0 {
            self.mac = [
                ral as u8,
                (ral >> 8) as u8,
                (ral >> 16) as u8,
                (ral >> 24) as u8,
                rah as u8,
                (rah >> 8) as u8,
            ];
            return;
        }
        let w0 = self.read_eeprom(0);
        let w1 = self.read_eeprom(1);
        let w2 = self.read_eeprom(2);
        self.mac = [
            w0 as u8,
            (w0 >> 8) as u8,
            w1 as u8,
            (w1 >> 8) as u8,
            w2 as u8,
            (w2 >> 8) as u8,
        ];
    }

    fn setup_rx(&mut self) -> bool {
        let ring_frame = match pmm::alloc_frame() {
            Some(a) => a,
            None => return false,
        };
        let buf_pages = (RX_DESC_COUNT * BUFFER_SIZE).div_ceil(4096);
        let buf_base = match pmm::alloc_frames(buf_pages) {
            Some(a) => a,
            None => return false,
        };
        self.rx_ring = ring_frame as usize as *mut RxDesc;
        self.rx_buffers = buf_base;
        unsafe {
            core::ptr::write_bytes(self.rx_ring as *mut u8, 0, 4096);
            for i in 0..RX_DESC_COUNT {
                let desc = self.rx_ring.add(i);
                (*desc).addr = buf_base + (i * BUFFER_SIZE) as u64;
                (*desc).status = 0;
            }
        }
        self.reg_write(REG_RDBAL, ring_frame as u32);
        self.reg_write(REG_RDBAH, (ring_frame >> 32) as u32);
        self.reg_write(REG_RDLEN, (RX_DESC_COUNT * core::mem::size_of::<RxDesc>()) as u32);
        self.reg_write(REG_RDH, 0);
        self.reg_write(REG_RDT, (RX_DESC_COUNT - 1) as u32);
        self.reg_write(
            REG_RCTL,
            RCTL_EN | RCTL_BAM | RCTL_SECRC | RCTL_BSIZE_2048,
        );
        self.rx_cur = 0;
        true
    }

    fn setup_tx(&mut self) -> bool {
        let ring_frame = match pmm::alloc_frame() {
            Some(a) => a,
            None => return false,
        };
        let buf_pages = (TX_DESC_COUNT * BUFFER_SIZE).div_ceil(4096);
        let buf_base = match pmm::alloc_frames(buf_pages) {
            Some(a) => a,
            None => return false,
        };
        self.tx_ring = ring_frame as usize as *mut TxDesc;
        self.tx_buffers = buf_base;
        unsafe {
            core::ptr::write_bytes(self.tx_ring as *mut u8, 0, 4096);
            for i in 0..TX_DESC_COUNT {
                let desc = self.tx_ring.add(i);
                (*desc).addr = buf_base + (i * BUFFER_SIZE) as u64;
                (*desc).status = TX_STATUS_DD;
            }
        }
        self.reg_write(REG_TDBAL, ring_frame as u32);
        self.reg_write(REG_TDBAH, (ring_frame >> 32) as u32);
        self.reg_write(REG_TDLEN, (TX_DESC_COUNT * core::mem::size_of::<TxDesc>()) as u32);
        self.reg_write(REG_TDH, 0);
        self.reg_write(REG_TDT, 0);
        self.reg_write(REG_TIPG, TIPG_DEFAULT);
        self.reg_write(
            REG_TCTL,
            TCTL_EN | TCTL_PSP | (0x10 << TCTL_CT_SHIFT) | (0x40 << TCTL_COLD_SHIFT),
        );
        self.tx_cur = 0;
        true
    }

    fn initialize(&mut self) -> bool {
        let dev = match self.find_device() {
            Some(d) => d,
            None => return false,
        };
        let bar0 = pci::read_bar(dev.bus, dev.device, dev.function, 0);
        if pci::bar_type(bar0) == pci::BarType::Io {
            log_warn!("e1000: BAR0 is I/O space, MMIO required");
            return false;
        }
        let mmio_base = match pci::bar_phys_addr(dev.bus, dev.device, dev.function, 0) {
            Some(a) => a,
            None => {
                log_warn!("e1000: BAR0 invalid");
                return false;
            }
        };
        self.mmio = mmio_base as usize as *mut u8;

        let cmd = pci::read_command(dev.bus, dev.device, dev.function);
        pci::write_command(dev.bus, dev.device, dev.function, cmd | 0x06);

        self.reg_write(REG_IMC, 0xFFFF_FFFF);
        self.reg_write(REG_CTRL, self.reg_read(REG_CTRL) | CTRL_RST);
        for _ in 0..1_000_000 {
            if self.reg_read(REG_CTRL) & CTRL_RST == 0 {
                break;
            }
            crate::util::pause();
        }
        self.reg_write(REG_IMC, 0xFFFF_FFFF);
        let _ = self.reg_read(REG_ICR);

        let ctrl = self.reg_read(REG_CTRL);
        let ctrl = (ctrl | CTRL_SLU | CTRL_ASDE) & !(CTRL_LRST | CTRL_PHY_RST | CTRL_ILOS | CTRL_VME);
        self.reg_write(REG_CTRL, ctrl);

        for i in 0..128u32 {
            self.reg_write(REG_MTA + i * 4, 0);
        }

        self.read_mac();

        if !self.setup_rx() {
            log_warn!("e1000: RX ring setup failed");
            return false;
        }
        if !self.setup_tx() {
            log_warn!("e1000: TX ring setup failed");
            return false;
        }

        self.available = true;
        let m = self.mac;
        let link = self.reg_read(REG_STATUS) & STATUS_LU != 0;
        log_info!(
            "e1000: {:04x}:{:04x} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} link={}",
            dev.vendor_id,
            dev.device_id,
            m[0],
            m[1],
            m[2],
            m[3],
            m[4],
            m[5],
            if link { "up" } else { "down" }
        );
        true
    }

    fn transmit(&mut self, data: &[u8]) -> bool {
        if !self.available || data.is_empty() || data.len() > BUFFER_SIZE {
            return false;
        }
        let idx = self.tx_cur;
        unsafe {
            let desc = self.tx_ring.add(idx);
            if (*desc).status & TX_STATUS_DD == 0 {
                return false;
            }
            let buf = (self.tx_buffers + (idx * BUFFER_SIZE) as u64) as usize as *mut u8;
            core::ptr::copy_nonoverlapping(data.as_ptr(), buf, data.len());
            (*desc).length = data.len() as u16;
            (*desc).cmd = TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS;
            (*desc).status = 0;
        }
        self.tx_cur = (idx + 1) % TX_DESC_COUNT;
        self.reg_write(REG_TDT, self.tx_cur as u32);

        for _ in 0..1_000_000 {
            unsafe {
                if (*self.tx_ring.add(idx)).status & TX_STATUS_DD != 0 {
                    return true;
                }
            }
            crate::util::pause();
        }
        false
    }

    fn receive(&mut self, out: &mut [u8]) -> Option<usize> {
        if !self.available {
            return None;
        }
        let idx = self.rx_cur;
        unsafe {
            let desc = self.rx_ring.add(idx);
            if (*desc).status & RX_STATUS_DD == 0 {
                return None;
            }
            let len = (*desc).length as usize;
            let copy = len.min(out.len());
            let buf = (self.rx_buffers + (idx * BUFFER_SIZE) as u64) as usize as *const u8;
            core::ptr::copy_nonoverlapping(buf, out.as_mut_ptr(), copy);
            (*desc).status = 0;
            self.rx_cur = (idx + 1) % RX_DESC_COUNT;
            self.reg_write(REG_RDT, idx as u32);
            Some(len)
        }
    }
}

static E1000: SyncUnsafeCell<E1000> = SyncUnsafeCell::new(E1000::new());

pub fn init() -> bool {
    unsafe { (*E1000.0.get()).initialize() }
}

pub fn is_available() -> bool {
    unsafe { (*E1000.0.get()).available }
}

pub fn mac() -> Option<[u8; 6]> {
    unsafe {
        let dev = &*E1000.0.get();
        if dev.available { Some(dev.mac) } else { None }
    }
}

pub fn transmit(data: &[u8]) -> bool {
    unsafe { (*E1000.0.get()).transmit(data) }
}

pub fn receive(out: &mut [u8]) -> Option<usize> {
    unsafe { (*E1000.0.get()).receive(out) }
}
