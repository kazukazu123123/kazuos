#![no_std]
#![no_main]

include!("../../crates/kernel/src/syscall_numbers.rs");

#[repr(C)]
struct PciDeviceInfo {
    bus: u8,
    device: u8,
    function: u8,
    _pad: u8,
    vendor_id: u16,
    device_id: u16,
    class_code: u8,
    subclass: u8,
    prog_if: u8,
    header_type: u8,
}

// (class_code, subclass) -> name
const CLASS_NAMES: &[(u8, u8, &str)] = &[
    (0x00, 0x00, "Non-VGA unclassified device"),
    (0x00, 0x01, "VGA compatible unclassified device"),
    (0x01, 0x00, "SCSI storage controller"),
    (0x01, 0x01, "IDE interface"),
    (0x01, 0x05, "ATA controller"),
    (0x01, 0x06, "SATA controller"),
    (0x01, 0x08, "Non-Volatile memory controller"),
    (0x02, 0x00, "Ethernet controller"),
    (0x02, 0x80, "Network controller"),
    (0x03, 0x00, "VGA compatible controller"),
    (0x03, 0x80, "Display controller"),
    (0x04, 0x00, "Multimedia video controller"),
    (0x04, 0x01, "Multimedia audio controller"),
    (0x04, 0x03, "Audio device"),
    (0x05, 0x00, "RAM memory"),
    (0x06, 0x00, "Host bridge"),
    (0x06, 0x01, "ISA bridge"),
    (0x06, 0x04, "PCI bridge"),
    (0x06, 0x80, "Bridge"),
    (0x07, 0x00, "Serial controller"),
    (0x07, 0x80, "Communication controller"),
    (0x08, 0x00, "PIC"),
    (0x08, 0x06, "IOMMU"),
    (0x09, 0x00, "Keyboard controller"),
    (0x09, 0x03, "USB controller"),
    (0x0b, 0x00, "386 Processor"),
    (0x0c, 0x03, "USB controller"),
    (0x0c, 0x05, "SMBus"),
    (0x0c, 0x80, "Serial bus controller"),
    (0x0d, 0x00, "IRDA controller"),
    (0x0d, 0x80, "Wireless controller"),
    (0x0f, 0x00, "Satellite communications controller"),
    (0x10, 0x00, "Network and computing encryption device"),
    (0x11, 0x00, "DPIO module"),
    (0x12, 0x00, "Processing accelerator"),
    (0x13, 0x00, "Non-Essential Instrumentation"),
    (0x40, 0x00, "Co-processor"),
    (0xff, 0x00, "Unassigned class"),
];

// (vendor_id, device_id) -> (vendor_name, device_name)
const DEVICE_NAMES: &[(u16, u16, &str, &str)] = &[
    // Intel
    (0x8086, 0x1237, "Intel Corporation", "440FX - 82441FX PMC [Natoma]"),
    (0x8086, 0x7000, "Intel Corporation", "82371SB PIIX3 ISA [Natoma/Triton II]"),
    (0x8086, 0x7010, "Intel Corporation", "82371SB PIIX3 IDE [Natoma/Triton II]"),
    (0x8086, 0x7020, "Intel Corporation", "82371SB PIIX3 USB [Natoma/Triton II]"),
    (0x8086, 0x7113, "Intel Corporation", "82371AB/EB/MB PIIX4 ACPI"),
    (0x8086, 0x100e, "Intel Corporation", "82540EM Gigabit Ethernet Controller"),
    (0x8086, 0x10d3, "Intel Corporation", "82574L Gigabit Network Connection"),
    (0x8086, 0x2918, "Intel Corporation", "82801IB (ICH9) LPC Interface Controller"),
    (0x8086, 0x2922, "Intel Corporation", "82801IR/IO/IH (ICH9R/DO/DH) 6 port SATA"),
    (0x8086, 0x2930, "Intel Corporation", "82801I (ICH9 Family) SMBus Controller"),
    (0x8086, 0x29c0, "Intel Corporation", "82G33/G31/P35/P31 Express DRAM Controller"),
    // QEMU / Red Hat
    (0x1234, 0x1111, "QEMU", "Standard VGA"),
    (0x1b36, 0x0001, "Red Hat, Inc.", "QEMU PCI-PCI bridge"),
    (0x1b36, 0x0002, "Red Hat, Inc.", "QEMU PCI 16550A Adapter"),
    (0x1b36, 0x0005, "Red Hat, Inc.", "QEMU PCI SYSBUS FLASH"),
    (0x1b36, 0x000d, "Red Hat, Inc.", "QEMU XHCI Host Controller"),
    // VirtIO
    (0x1af4, 0x1000, "Red Hat, Inc.", "Virtio network device"),
    (0x1af4, 0x1001, "Red Hat, Inc.", "Virtio block device"),
    (0x1af4, 0x1002, "Red Hat, Inc.", "Virtio memory balloon"),
    (0x1af4, 0x1003, "Red Hat, Inc.", "Virtio console"),
    (0x1af4, 0x1004, "Red Hat, Inc.", "Virtio SCSI"),
    (0x1af4, 0x1005, "Red Hat, Inc.", "Virtio RNG"),
    (0x1af4, 0x1009, "Red Hat, Inc.", "Virtio filesystem"),
    (0x1af4, 0x1050, "Red Hat, Inc.", "Virtio GPU"),
    (0x1af4, 0x1052, "Red Hat, Inc.", "Virtio input"),
    // Realtek
    (0x10ec, 0x8029, "Realtek Semiconductor Co., Ltd.", "RTL-8029(AS)"),
    (0x10ec, 0x8139, "Realtek Semiconductor Co., Ltd.", "RTL-8139/8139C/8139C+"),
    (0x10ec, 0x8168, "Realtek Semiconductor Co., Ltd.", "RTL8111/8168/8411 PCIe Gigabit Ethernet"),
    // AMD
    (0x1002, 0x4385, "Advanced Micro Devices, Inc.", "SBx00 SMBus Controller"),
    (0x1002, 0x4396, "Advanced Micro Devices, Inc.", "SB7x0/SB8x0/SB9x0 USB EHCI Controller"),
    // NVIDIA
    (0x10de, 0x0041, "NVIDIA Corporation", "NV40 [GeForce 6800]"),
    // VMware
    (0x15ad, 0x0405, "VMware", "SVGA II Adapter"),
    (0x15ad, 0x0740, "VMware", "Virtual Machine Communication Interface"),
    (0x15ad, 0x07a0, "VMware", "PCI Express Root Port"),
    // AC97
    (0x8086, 0x2415, "Intel Corporation", "82801AA AC'97 Audio Controller"),
    (0x1274, 0x5000, "Ensoniq", "ES1370 [AudioPCI]"),
    (0x1274, 0x1371, "Ensoniq", "ES1371/ES1373 / Creative Labs CT2518"),
];

fn class_name(class: u8, sub: u8) -> &'static str {
    for &(c, s, name) in CLASS_NAMES {
        if c == class && s == sub {
            return name;
        }
    }
    // fallback: match class only
    for &(c, s, name) in CLASS_NAMES {
        if c == class && s == 0x80 {
            return name;
        }
    }
    "Unknown device"
}

fn device_names(vendor: u16, device: u16) -> (&'static str, &'static str) {
    for &(v, d, vname, dname) in DEVICE_NAMES {
        if v == vendor && d == device {
            return (vname, dname);
        }
    }
    ("Unknown vendor", "Unknown device")
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut index: u64 = 0;
    loop {
        let mut dev = PciDeviceInfo {
            bus: 0, device: 0, function: 0, _pad: 0,
            vendor_id: 0, device_id: 0,
            class_code: 0, subclass: 0, prog_if: 0, header_type: 0,
        };
        let total = syscall(SYS_PCI_INFO, index, &mut dev as *mut _ as u64, 0);
        if total == u64::MAX {
            break;
        }

        // BB:DD.F
        let mut buf = [0u8; 8];
        let mut pos = 0usize;
        hex2(dev.bus,    &mut buf, &mut pos);
        buf[pos] = b':'; pos += 1;
        hex2(dev.device, &mut buf, &mut pos);
        buf[pos] = b'.'; pos += 1;
        buf[pos] = b'0' + (dev.function & 7); pos += 1;
        sys_write(&buf[..pos]);
        sys_write(b" ");

        // class name
        sys_write(class_name(dev.class_code, dev.subclass).as_bytes());
        sys_write(b": ");

        // vendor + device name
        let (vname, dname) = device_names(dev.vendor_id, dev.device_id);
        if vname == "Unknown vendor" {
            // print raw IDs
            let mut id_buf = [0u8; 9];
            let mut p = 0usize;
            hex4(dev.vendor_id,  &mut id_buf, &mut p);
            id_buf[p] = b':'; p += 1;
            hex4(dev.device_id,  &mut id_buf, &mut p);
            sys_write(&id_buf[..p]);
        } else {
            sys_write(vname.as_bytes());
            sys_write(b" ");
            sys_write(dname.as_bytes());
        }
        sys_write(b"\r\n");

        index += 1;
        if index >= total {
            break;
        }
    }
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

fn sys_write(buf: &[u8]) {
    syscall(SYS_WRITE, 1, buf.as_ptr() as u64, buf.len() as u64);
}

fn hex2(v: u8, buf: &mut [u8], pos: &mut usize) {
    const H: &[u8] = b"0123456789abcdef";
    buf[*pos]     = H[(v >> 4) as usize];
    buf[*pos + 1] = H[(v & 0xf) as usize];
    *pos += 2;
}

fn hex4(v: u16, buf: &mut [u8], pos: &mut usize) {
    hex2((v >> 8) as u8, buf, pos);
    hex2((v & 0xff) as u8, buf, pos);
}

fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0, in("rsi") a1, in("rdx") a2,
        );
    }
    r
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
