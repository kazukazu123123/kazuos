use crate::drivers::acpi;
use crate::util::{ind, outd};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

static mut RSDP: u64 = 0;

#[derive(Clone, Copy)]
pub struct Device {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

#[derive(Clone, Copy)]
pub enum ScanKind {
    Pci,
    Pcie,
}

pub fn init(rsdp: u64) {
    unsafe {
        RSDP = rsdp;
    }
}

pub fn scan(kind: ScanKind, mut f: impl FnMut(Device)) {
    match kind {
        ScanKind::Pci => scan_pci(&mut f),
        ScanKind::Pcie => scan_pcie(&mut f),
    }
}

pub fn pcie_available() -> bool {
    unsafe {
        let Some((sdt, is_xsdt)) = acpi::parse_rsdp(RSDP) else {
            return false;
        };
        acpi::find_table(sdt, is_xsdt, *b"MCFG").is_some()
    }
}

pub fn read_bar(bus: u8, device: u8, function: u8, bar_index: u8) -> u32 {
    read_u32(bus, device, function, 0x10 + bar_index * 4)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarType {
    Io,
    Mmio32,
    Mmio64,
}

pub fn bar_type(bar: u32) -> BarType {
    if bar & 0x1 == 0x1 {
        BarType::Io
    } else {
        match (bar >> 1) & 0x3 {
            0x2 => BarType::Mmio64,
            _ => BarType::Mmio32,
        }
    }
}

/// Returns the physical base address for a PCI BAR, or None if the BAR is invalid.
pub fn bar_phys_addr(bus: u8, device: u8, function: u8, bar_index: u8) -> Option<u64> {
    let low = read_u32(bus, device, function, 0x10 + bar_index * 4);
    if low == 0 {
        return None;
    }
    match bar_type(low) {
        BarType::Io => Some((low & 0xFFFFFFFC) as u64),
        BarType::Mmio32 => Some((low & 0xFFFFFFF0) as u64),
        BarType::Mmio64 => {
            let high = read_u32(bus, device, function, 0x10 + (bar_index + 1) * 4);
            Some(((high as u64) << 32) | ((low & 0xFFFFFFF0) as u64))
        }
    }
}

/// Decode the size of a PCI BAR by writing all-ones and reading back the mask.
/// Returns 0 if the BAR is unimplemented or decoding fails.
pub fn bar_size(bus: u8, device: u8, function: u8, bar_index: u8) -> u64 {
    let offset = 0x10 + bar_index * 4;
    let original = read_u32(bus, device, function, offset);
    if original == 0 {
        return 0;
    }
    let ty = bar_type(original);
    let mask = if ty == BarType::Io {
        0xFFFFFFFC
    } else {
        0xFFFFFFF0
    };
    write_u32(bus, device, function, offset, 0xFFFFFFFF);
    let decoded = read_u32(bus, device, function, offset);
    write_u32(bus, device, function, offset, original);
    let size_low = (!(decoded & mask)).wrapping_add(1) as u64;
    if size_low == 0 {
        return 0;
    }
    if ty == BarType::Mmio64 {
        let offset_high = offset + 4;
        let original_high = read_u32(bus, device, function, offset_high);
        write_u32(bus, device, function, offset_high, 0xFFFFFFFF);
        let decoded_high = read_u32(bus, device, function, offset_high);
        write_u32(bus, device, function, offset_high, original_high);
        let size_high = (!(decoded_high as u64)).wrapping_add(1);
        if size_high != 0 {
            (size_high << 32) | size_low
        } else {
            size_low
        }
    } else {
        size_low
    }
}

pub fn read_command(bus: u8, device: u8, function: u8) -> u16 {
    (read_u32(bus, device, function, 0x04) & 0xFFFF) as u16
}

pub fn read_interrupt_line(bus: u8, device: u8, function: u8) -> u8 {
    (read_u32(bus, device, function, 0x3C) & 0xFF) as u8
}

pub fn write_command(bus: u8, device: u8, function: u8, value: u16) {
    let address = read_u32(bus, device, function, 0x04) & 0xFFFF_0000;
    write_u32(bus, device, function, 0x04, address | value as u32);
}

fn scan_pci(f: &mut impl FnMut(Device)) {
    for bus in 0..=255u8 {
        for device in 0..32u8 {
            for function in 0..8u8 {
                if let Some(dev) = read_config_device(bus, device, function) {
                    f(dev);
                }
            }
        }
    }
}

fn scan_pcie(f: &mut impl FnMut(Device)) {
    let Some((sdt, is_xsdt)) = (unsafe { acpi::parse_rsdp(RSDP) }) else {
        return;
    };
    let Some(mcfg_phys) = (unsafe { acpi::find_table(sdt, is_xsdt, *b"MCFG") }) else {
        return;
    };
    let header: acpi::SdtHeader = unsafe { acpi::read_phys(mcfg_phys) };
    let length = unsafe { core::ptr::read_unaligned(core::ptr::addr_of!(header.length)) } as usize;
    let header_size = core::mem::size_of::<acpi::SdtHeader>();
    if length < header_size + 8 + 16 {
        return;
    }
    let entries_start = mcfg_phys + header_size as u64 + 8;
    let entry_count = (length - header_size - 8) / 16;
    for index in 0..entry_count {
        let entry = entries_start + index as u64 * 16;
        let base = unsafe { core::ptr::read_unaligned(entry as *const u64) };
        let start_bus = unsafe { core::ptr::read_unaligned((entry + 10) as *const u8) };
        let end_bus = unsafe { core::ptr::read_unaligned((entry + 11) as *const u8) };
        for bus in start_bus..=end_bus {
            for device in 0..32u8 {
                for function in 0..8u8 {
                    if let Some(dev) = read_ecam_device(base, start_bus, bus, device, function) {
                        f(dev);
                    }
                }
            }
        }
    }
}

fn read_config_device(bus: u8, device: u8, function: u8) -> Option<Device> {
    let vendor_device = read_u32(bus, device, function, 0x00);
    let vendor_id = (vendor_device & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return None;
    }
    Some(build_device(bus, device, function, vendor_device))
}

fn read_ecam_device(base: u64, start_bus: u8, bus: u8, device: u8, function: u8) -> Option<Device> {
    let bus_offset = bus.saturating_sub(start_bus) as u64;
    let addr = base + (bus_offset << 20) + ((device as u64) << 15) + ((function as u64) << 12);
    let vendor_id = unsafe { core::ptr::read_volatile(addr as *const u16) };
    if vendor_id == 0xFFFF {
        return None;
    }
    let vendor_device = unsafe { core::ptr::read_volatile(addr as *const u32) };
    Some(build_device(bus, device, function, vendor_device))
}

fn build_device(bus: u8, device: u8, function: u8, vendor_device: u32) -> Device {
    let device_id = (vendor_device >> 16) as u16;
    let class_reg = read_u32(bus, device, function, 0x08);
    let header_reg = read_u32(bus, device, function, 0x0C);
    Device {
        bus,
        device,
        function,
        vendor_id: (vendor_device & 0xFFFF) as u16,
        device_id,
        class_code: (class_reg >> 24) as u8,
        subclass: (class_reg >> 16) as u8,
        prog_if: (class_reg >> 8) as u8,
        header_type: (header_reg >> 16) as u8,
    }
}

// The CONFIG_ADDRESS/CONFIG_DATA pair is a stateful two-step protocol: write the target
// address, then read/write the data port. If two CPUs interleave, or an interrupt slips
// in between the two ports, the data access lands on whatever address the other party
// last wrote — returning garbage. Serialize every config access (IRQs off + a global
// spinlock) so the address+data steps are atomic.
static PCI_CONFIG_LOCK: crate::util::SpinLock = crate::util::SpinLock::new();

fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    (1u32 << 31)
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC)
}

fn read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address = config_address(bus, device, function, offset);
    let flags = crate::util::irq_save();
    PCI_CONFIG_LOCK.lock();
    let v = unsafe {
        outd(CONFIG_ADDRESS, address);
        ind(CONFIG_DATA)
    };
    PCI_CONFIG_LOCK.unlock();
    crate::util::restore_flags(flags);
    v
}

fn write_u32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address = config_address(bus, device, function, offset);
    let flags = crate::util::irq_save();
    PCI_CONFIG_LOCK.lock();
    unsafe {
        outd(CONFIG_ADDRESS, address);
        outd(CONFIG_DATA, value);
    }
    PCI_CONFIG_LOCK.unlock();
    crate::util::restore_flags(flags);
}
