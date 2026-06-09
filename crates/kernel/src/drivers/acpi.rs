// Minimal ACPI parser for RSDP -> XSDT/RSDT -> MADT

use core::mem::size_of;

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Rsdp {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_addr: u32,
    pub length: u32,
    pub xsdt_addr: u64,
    pub ext_checksum: u8,
    _reserved: [u8; 3],
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SdtHeader {
    pub signature: [u8; 4],
    pub length: u32,
    pub revision: u8,
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub oem_table_id: [u8; 8],
    pub oem_revision: u32,
    pub creator_id: u32,
    pub creator_revision: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct Madt {
    pub header: SdtHeader,
    pub lapic_addr: u32,
    pub flags: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtIoApic {
    pub header: MadtEntryHeader,
    pub ioapic_id: u8,
    _reserved: u8,
    pub ioapic_addr: u32,
    pub global_irq_base: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtEntryHeader {
    pub ty: u8,
    pub len: u8,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtLocalApic {
    pub header: MadtEntryHeader,
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
}

#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct MadtInterruptSourceOverride {
    pub header: MadtEntryHeader,
    pub bus_source: u8,
    pub irq_source: u8,
    pub global_irq: u32,
    pub flags: u16,
}

fn sig_eq(a: &[u8; 4], b: &[u8]) -> bool {
    a == b
}

fn phys_ptr<T>(addr: u64) -> *const T {
    addr as *const T
}

pub(crate) unsafe fn read_phys<T: Copy>(addr: u64) -> T {
    unsafe { core::ptr::read_unaligned(phys_ptr::<T>(addr) as *const u8 as *const T) }
}

unsafe fn read_unaligned<T: Copy>(ptr: *const T) -> T {
    unsafe { core::ptr::read_unaligned(ptr) }
}

pub(crate) unsafe fn parse_rsdp(rsdp_phys: u64) -> Option<(u64, bool)> {
    if rsdp_phys == 0 {
        return None;
    }
    let rsdp: Rsdp = unsafe { read_phys(rsdp_phys) };
    if &rsdp.signature != b"RSD PTR " {
        return None;
    }
    let revision = rsdp.revision;
    let xsdt_addr = unsafe { read_unaligned(core::ptr::addr_of!(rsdp.xsdt_addr)) };
    let rsdt_addr = unsafe { read_unaligned(core::ptr::addr_of!(rsdp.rsdt_addr)) };
    if revision >= 2 && xsdt_addr != 0 {
        Some((xsdt_addr, true))
    } else {
        Some((rsdt_addr as u64, false))
    }
}

unsafe fn table_entries_32(table_phys: u64) -> &'static [u32] {
    let header: SdtHeader = unsafe { read_phys(table_phys) };
    let length = unsafe { read_unaligned(core::ptr::addr_of!(header.length)) } as usize;
    let entries_start = table_phys + size_of::<SdtHeader>() as u64;
    let entry_count = (length - size_of::<SdtHeader>()) / 4;
    unsafe { core::slice::from_raw_parts(entries_start as *const u32, entry_count) }
}

unsafe fn table_entries_64(table_phys: u64) -> &'static [u64] {
    let header: SdtHeader = unsafe { read_phys(table_phys) };
    let length = unsafe { read_unaligned(core::ptr::addr_of!(header.length)) } as usize;
    let entries_start = table_phys + size_of::<SdtHeader>() as u64;
    let entry_count = (length - size_of::<SdtHeader>()) / 8;
    unsafe { core::slice::from_raw_parts(entries_start as *const u64, entry_count) }
}

pub(crate) unsafe fn find_madt(sdt_phys: u64, is_xsdt: bool) -> Option<u64> {
    let entries: &[u64] = if is_xsdt {
        unsafe { table_entries_64(sdt_phys) }
    } else {
        let e32 = unsafe { table_entries_32(sdt_phys) };
        let count = e32.len();
        let mut i = 0;
        while i < count {
            let entry_phys = e32[i] as u64;
            let entry_header: SdtHeader = unsafe { read_phys(entry_phys) };
            if sig_eq(&entry_header.signature, b"APIC") {
                return Some(entry_phys);
            }
            i += 1;
        }
        return None;
    };

    for &entry_phys in entries {
        let entry_header: SdtHeader = unsafe { read_phys(entry_phys) };
        if sig_eq(&entry_header.signature, b"APIC") {
            return Some(entry_phys);
        }
    }
    None
}

pub(crate) unsafe fn find_table(sdt_phys: u64, is_xsdt: bool, sig: [u8; 4]) -> Option<u64> {
    if is_xsdt {
        for &entry_phys in unsafe { table_entries_64(sdt_phys) } {
            let entry_header: SdtHeader = unsafe { read_phys(entry_phys) };
            if entry_header.signature == sig {
                return Some(entry_phys);
            }
        }
    } else {
        for &entry_phys in unsafe { table_entries_32(sdt_phys) } {
            let entry_header: SdtHeader = unsafe { read_phys(entry_phys as u64) };
            if entry_header.signature == sig {
                return Some(entry_phys as u64);
            }
        }
    }
    None
}

#[derive(Debug, Clone, Copy)]
pub struct IoApicInfo {
    pub id: u8,
    pub addr: u32,
    pub global_irq_base: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct IrqOverride {
    pub bus_irq: u8,
    pub global_irq: u32,
    pub flags: u16,
}

pub(crate) unsafe fn parse_madt(
    madt_phys: u64,
) -> (u32, Option<IoApicInfo>, Option<IrqOverride>, u8) {
    let madt: Madt = unsafe { read_phys(madt_phys) };
    let lapic_addr = unsafe { read_unaligned(core::ptr::addr_of!(madt.lapic_addr)) };
    let header_length = unsafe { read_unaligned(core::ptr::addr_of!(madt.header.length)) };

    let data_start = madt_phys + size_of::<Madt>() as u64;
    let data_end = madt_phys + header_length as u64;
    let mut ptr = data_start;

    let mut ioapic = None;
    let mut irq_override = None;
    let mut bsp_apic_id = 0u8;

    while ptr + 2 <= data_end {
        let header: MadtEntryHeader = unsafe { read_phys(ptr) };
        if header.len == 0 {
            break;
        }
        match header.ty {
            0 => {
                if header.len as usize >= size_of::<MadtLocalApic>() {
                    let entry: MadtLocalApic = unsafe { read_phys(ptr) };
                    let flags = unsafe { read_unaligned(core::ptr::addr_of!(entry.flags)) };
                    if entry.acpi_processor_id == 0 && (flags & 1) != 0 {
                        bsp_apic_id = entry.apic_id;
                    }
                }
            }
            1 => {
                if header.len as usize >= size_of::<MadtIoApic>() {
                    let entry: MadtIoApic = unsafe { read_phys(ptr) };
                    ioapic = Some(IoApicInfo {
                        id: entry.ioapic_id,
                        addr: unsafe { read_unaligned(core::ptr::addr_of!(entry.ioapic_addr)) },
                        global_irq_base: unsafe {
                            read_unaligned(core::ptr::addr_of!(entry.global_irq_base))
                        },
                    });
                }
            }
            2 if header.len as usize >= size_of::<MadtInterruptSourceOverride>() => {
                let entry: MadtInterruptSourceOverride = unsafe { read_phys(ptr) };
                irq_override = Some(IrqOverride {
                    bus_irq: entry.irq_source,
                    global_irq: unsafe { read_unaligned(core::ptr::addr_of!(entry.global_irq)) },
                    flags: unsafe { read_unaligned(core::ptr::addr_of!(entry.flags)) },
                });
            }
            _ => {}
        }
        ptr += header.len as u64;
    }

    (lapic_addr, ioapic, irq_override, bsp_apic_id)
}
