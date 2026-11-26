#![no_std]
#![no_main]
include!("../runtime/driver_runtime.rs");

static CLAIMED_IRQ: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub fn kdm_info() -> KdmInfo {
    KdmInfo { name: "xhci_irqprobe", depends: &[] }
}

pub fn kdm_init() -> bool {
    let mut index = 0;
    loop {
        let mut dev = PciDeviceInfo::default();
        let count = sys_pci_info(index, &mut dev);
        if count == u64::MAX || index >= count {
            println!("xhci_irqprobe: no xHCI controller");
            return false;
        }
        if dev.class_code == 0x0c && dev.subclass == 0x03 && dev.prog_if == 0x30 {
            let bdf = ((dev.bus as u32) << 16) | ((dev.device as u32) << 8) | dev.function as u32;
            let line = sys_pci_enable(bdf);
            println!("xhci_irqprobe: PCI IRQ line {}", line);
            let Some(irq) = sys_irq_claim(bdf) else {
                println!("xhci_irqprobe: IRQ claim failed");
                sys_pci_disable(bdf);
                return false;
            };
            CLAIMED_IRQ.store(irq, core::sync::atomic::Ordering::Release);
            println!("xhci_irqprobe: claimed IRQ {} alongside HDA", irq);
            return true;
        }
        index += 1;
    }
}

pub fn kdm_run() {
    while !sys_signal_check() {
        sys_sleep(100);
    }
}

pub fn kdm_exit() {
    let irq = CLAIMED_IRQ.swap(0, core::sync::atomic::Ordering::AcqRel);
    if irq != 0 { sys_irq_release(irq); }
}
