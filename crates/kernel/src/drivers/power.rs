use crate::util::{inb, outb, outw, pause};

#[derive(Debug, Clone, Copy)]
pub struct PowerInfo {
    pub smi_cmd: u16,
    pub acpi_enable: u8,
    pub pm1a_cnt: u16,
    pub pm1b_cnt: u16,
    pub slp_typa: u16,
    pub slp_typb: u16,
}

static mut POWER: Option<PowerInfo> = None;

pub fn init(rsdp: u64) {
    unsafe {
        POWER = find_power_info(rsdp);
    }
}

pub fn shutdown() -> ! {
    unsafe {
        if let Some(info) = POWER {
            if info.smi_cmd != 0 && info.acpi_enable != 0 {
                outb(info.smi_cmd, info.acpi_enable);
                for _ in 0..1_000_000 {
                    pause();
                }
            }
            let slp_en = 1u16 << 13;
            outw(info.pm1a_cnt, info.slp_typa | slp_en);
            if info.pm1b_cnt != 0 {
                outw(info.pm1b_cnt, info.slp_typb | slp_en);
            }
            for _ in 0..10_000_000 {
                pause();
            }
        }
        qemu_shutdown();
    }
    loop {
        pause();
    }
}

pub fn reboot() -> ! {
    unsafe {
        for _ in 0..100000 {
            if inb(0x64) & 0x02 == 0 {
                break;
            }
            pause();
        }
        outb(0x64, 0xFE);
    }
    loop {
        pause();
    }
}

pub fn poll_power_button() {}

unsafe fn qemu_shutdown() {
    unsafe {
        outw(0x604, 0x2000);
        outw(0xB004, 0x2000);
    }
}

unsafe fn find_power_info(rsdp: u64) -> Option<PowerInfo> {
    let (sdt, is_xsdt) = unsafe { super::acpi::parse_rsdp(rsdp)? };
    let fadt = unsafe { find_table(sdt, is_xsdt, *b"FACP")? };
    let dsdt = unsafe { read_u32(fadt + 40) as u64 };
    let smi_cmd = unsafe { read_u32(fadt + 48) as u16 };
    let acpi_enable = unsafe { read_u8(fadt + 52) };
    let pm1a_cnt = unsafe { read_u32(fadt + 64) as u16 };
    let pm1b_cnt = unsafe { read_u32(fadt + 68) as u16 };
    let (slp_typa, slp_typb) = unsafe { find_s5(dsdt)? };
    Some(PowerInfo {
        smi_cmd,
        acpi_enable,
        pm1a_cnt,
        pm1b_cnt,
        slp_typa,
        slp_typb,
    })
}

unsafe fn find_table(sdt: u64, is_xsdt: bool, sig: [u8; 4]) -> Option<u64> {
    let length = unsafe { read_u32(sdt + 4) as usize };
    let entry_size = if is_xsdt { 8 } else { 4 };
    let count = (length - 36) / entry_size;
    let mut i = 0;
    while i < count {
        let entry = if is_xsdt {
            unsafe { read_u64(sdt + 36 + (i * 8) as u64) }
        } else {
            unsafe { read_u32(sdt + 36 + (i * 4) as u64) as u64 }
        };
        if unsafe { read_sig(entry) } == sig {
            return Some(entry);
        }
        i += 1;
    }
    None
}

unsafe fn find_s5(dsdt: u64) -> Option<(u16, u16)> {
    let length = unsafe { read_u32(dsdt + 4) as usize };
    let start = dsdt + 36;
    let end = dsdt + length as u64;
    let mut ptr = start;
    while ptr + 8 < end {
        if unsafe {
            read_u8(ptr)     == b'_'
            && read_u8(ptr + 1) == b'S'
            && read_u8(ptr + 2) == b'5'
            && read_u8(ptr + 3) == b'_'
        } {
            let scan_end = (ptr + 128).min(end);
            let mut p = ptr + 4;

            // Skip to Package opcode (0x12)
            while p < scan_end && unsafe { read_u8(p) } != 0x12 {
                p += 1;
            }
            if p >= scan_end { ptr += 1; continue; }
            p += 1; // skip Package opcode

            // Skip PkgLength (variable-length encoding)
            if p >= scan_end { ptr += 1; continue; }
            let pkg_lead = unsafe { read_u8(p) };
            let extra = (pkg_lead >> 6) as u64;
            p += 1 + extra;

            // Skip NumElements
            p += 1;

            // Read SLP_TYP_A and SLP_TYP_B
            let mut values = [0u16; 2];
            let mut count = 0usize;
            while p < scan_end && count < 2 {
                let op = unsafe { read_u8(p) };
                if op == 0x0A {
                    // ByteData
                    values[count] = unsafe { read_u8(p + 1) } as u16;
                    count += 1;
                    p += 2;
                } else if op == 0x0B {
                    // WordData
                    values[count] = unsafe { read_u16(p + 1) };
                    count += 1;
                    p += 3;
                } else if op <= 0x0F {
                    // Small integer (ZeroOp=0x00, OneOp=0x01, or literal byte)
                    values[count] = op as u16;
                    count += 1;
                    p += 1;
                } else {
                    break;
                }
            }
            if count >= 1 {
                let a = values[0] << 10;
                let b = if count >= 2 { values[1] << 10 } else { a };
                return Some((a, b));
            }
        }
        ptr += 1;
    }
    None
}

unsafe fn read_sig(addr: u64) -> [u8; 4] {
    [
        unsafe { read_u8(addr) },
        unsafe { read_u8(addr + 1) },
        unsafe { read_u8(addr + 2) },
        unsafe { read_u8(addr + 3) },
    ]
}

unsafe fn read_u8(addr: u64) -> u8 {
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

unsafe fn read_u16(addr: u64) -> u16 {
    unsafe { core::ptr::read_unaligned(addr as *const u16) }
}

unsafe fn read_u32(addr: u64) -> u32 {
    unsafe { core::ptr::read_unaligned(addr as *const u32) }
}

unsafe fn read_u64(addr: u64) -> u64 {
    unsafe { core::ptr::read_unaligned(addr as *const u64) }
}
