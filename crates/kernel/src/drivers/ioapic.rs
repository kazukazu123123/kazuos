use core::sync::atomic::{AtomicU64, AtomicU8, Ordering};

static DRIVER_IOAPIC_BASE: AtomicU64 = AtomicU64::new(0);
static DRIVER_LAPIC_ID: AtomicU8 = AtomicU8::new(0);

pub fn configure_driver_irqs(base: u64, lapic_id: u8) {
    DRIVER_LAPIC_ID.store(lapic_id, Ordering::Release);
    DRIVER_IOAPIC_BASE.store(base, Ordering::Release);
}

pub fn mask_driver_irq(irq: u8, masked: bool) -> bool {
    let base = DRIVER_IOAPIC_BASE.load(Ordering::Acquire);
    if base == 0 { return false; }
    let ioapic = unsafe { IoApic::new(base) };
    unsafe {
        if masked { ioapic.mask_irq(irq); } else { ioapic.unmask_irq(irq); }
    }
    true
}

pub fn route_driver_irq(irq: u8, enable: bool) -> bool {
    let base = DRIVER_IOAPIC_BASE.load(Ordering::Acquire);
    if base == 0 { return false; }
    let ioapic = unsafe { IoApic::new(base) };
    unsafe {
        if enable {
            ioapic.set_irq_ext(irq, 0x31, DRIVER_LAPIC_ID.load(Ordering::Acquire), 0x0f);
            ioapic.unmask_irq(irq);
        } else {
            ioapic.mask_irq(irq);
        }
    }
    true
}

/// IOAPIC driver.
/// NOTE: `base` is a **physical address**. Under UEFI, identity mapping is
/// typically active so phys == virt, but this is not guaranteed once a custom
/// page table is installed. Keep this in mind when enabling a VMM later.
pub struct IoApic {
    base: u64,
}

impl IoApic {
    pub(crate) unsafe fn new(base: u64) -> Self {
        Self { base }
    }

    unsafe fn read_reg(&self, reg: u8) -> u32 {
        let sel = self.base as *mut u32;
        let win = (self.base + 0x10) as *mut u32;
        unsafe {
            sel.write_volatile(reg as u32);
            win.read_volatile()
        }
    }

    unsafe fn write_reg(&self, reg: u8, value: u32) {
        let sel = self.base as *mut u32;
        let win = (self.base + 0x10) as *mut u32;
        unsafe {
            sel.write_volatile(reg as u32);
            win.write_volatile(value);
        }
    }

    pub(crate) unsafe fn max_redirection_entry(&self) -> u8 {
        unsafe { ((self.read_reg(0x01) >> 16) & 0xFF) as u8 }
    }

    pub(crate) unsafe fn mask_all(&self) {
        unsafe {
            let max = self.max_redirection_entry();
            for i in 0..=max {
                let low_reg = 0x10 + i * 2;
                let high_reg = low_reg + 1;
                self.write_reg(high_reg, 0);
                let mut low = self.read_reg(low_reg);
                low |= 1 << 16;
                self.write_reg(low_reg, low);
            }
        }
    }

    /// flags: bit 0-1 = polarity (0=conform, 1=active high, 3=active low)
    ///        bit 2-3 = trigger mode (0=conform, 1=edge, 3=level)
    pub(crate) unsafe fn set_irq_ext(&self, irq: u8, vector: u8, lapic_id: u8, flags: u16) {
        unsafe {
            let low_reg = 0x10 + irq * 2;
            let high_reg = low_reg + 1;
            self.write_reg(high_reg, (lapic_id as u32) << 24);
            let mut low = self.read_reg(low_reg);
            low &= !0x1F7FF;
            low |= vector as u32;
            let polarity = flags & 0x3;
            let trigger = (flags >> 2) & 0x3;
            if polarity != 0 {
                low &= !(1 << 13);
                low |= ((polarity & 1) as u32) << 13;
            }
            if trigger != 0 {
                low &= !(1 << 15);
                low |= ((trigger & 1) as u32) << 15;
            }
            self.write_reg(low_reg, low);
        }
    }

    pub(crate) unsafe fn unmask_irq(&self, irq: u8) {
        unsafe {
            let low_reg = 0x10 + irq * 2;
            let mut low = self.read_reg(low_reg);
            low &= !(1 << 16);
            self.write_reg(low_reg, low);
        }
    }

    pub(crate) unsafe fn mask_irq(&self, irq: u8) {
        unsafe {
            let low_reg = 0x10 + irq * 2;
            let mut low = self.read_reg(low_reg);
            low |= 1 << 16;
            self.write_reg(low_reg, low);
        }
    }
}
