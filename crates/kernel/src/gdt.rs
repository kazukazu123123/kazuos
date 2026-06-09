use core::arch::asm;

use crate::util::SyncUnsafeCell;

pub const KERNEL_CODE: u16 = 0x08;
pub const KERNEL_DATA: u16 = 0x10;
pub const USER_CODE: u16 = 0x1b;
pub const USER_DATA: u16 = 0x23;
pub const TSS_SELECTOR: u16 = 0x28;

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

#[repr(C, packed)]
struct Tss {
    reserved0: u32,
    rsp: [u64; 3],
    reserved1: u64,
    ist: [u64; 7],
    reserved2: u64,
    reserved3: u16,
    iopb_offset: u16,
}

// IOPB: 1 bit per I/O port (65536 ports = 8192 bytes).
// 1 = blocked (default), 0 = allowed for ring-3.
// The extra trailing 0xFF byte is required by the CPU spec.
#[repr(C)]
struct TssIopb {
    tss: Tss,
    iopb: [u8; 8192],
    iopb_end: u8,
}

#[repr(C, align(16))]
struct Gdt {
    entries: [u64; 7],
}

#[repr(C, align(16))]
struct IstStack([u8; 16384]);

static mut IST_STACK: IstStack = IstStack([0; 16384]);

static TSS: SyncUnsafeCell<TssIopb> = SyncUnsafeCell::new(TssIopb {
    tss: Tss {
        reserved0: 0,
        rsp: [0; 3],
        reserved1: 0,
        ist: [0; 7],
        reserved2: 0,
        reserved3: 0,
        iopb_offset: core::mem::size_of::<Tss>() as u16,
    },
    iopb: [0xFF; 8192],
    iopb_end: 0xFF,
});

static GDT: SyncUnsafeCell<Gdt> = SyncUnsafeCell::new(Gdt { entries: [0; 7] });

pub fn set_kernel_stack_top(rsp0: u64) {
    unsafe {
        (*TSS.0.get()).tss.rsp[0] = rsp0;
    }
}

/// Allow ring-3 access to a single I/O port via the IOPB.
pub fn iopb_allow_port(port: u16) {
    unsafe {
        let byte = (port / 8) as usize;
        let bit = port % 8;
        (*TSS.0.get()).iopb[byte] &= !(1 << bit);
    }
}

pub(crate) unsafe fn init(kernel_stack_top: u64) {
    unsafe {
        (*TSS.0.get()).tss.rsp[0] = kernel_stack_top;
        let ist_top = core::ptr::addr_of!(IST_STACK) as u64 + 16384;
        (*TSS.0.get()).tss.ist[0] = ist_top;
        let tss_base = TSS.0.get() as u64;
        let tss_limit = core::mem::size_of::<TssIopb>() as u64 - 1;
        let gdt = &mut *GDT.0.get();
        gdt.entries[0] = 0;
        gdt.entries[1] = 0x00af9a000000ffff;
        gdt.entries[2] = 0x00af92000000ffff;
        gdt.entries[3] = 0x00affa000000ffff;
        gdt.entries[4] = 0x00aff2000000ffff;
        gdt.entries[5] = tss_descriptor_low(tss_base, tss_limit);
        gdt.entries[6] = tss_base >> 32;
        let ptr = DescriptorTablePointer {
            limit: (core::mem::size_of::<Gdt>() - 1) as u16,
            base: gdt.entries.as_ptr() as u64,
        };
        asm!("lgdt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags));
        asm!(
            "push {code}",
            "lea rax, [rip + 2f]",
            "push rax",
            "retfq",
            "2:",
            code = const KERNEL_CODE as u64,
            out("rax") _,
        );
        asm!(
            "mov ds, ax",
            "mov es, ax",
            "mov ss, ax",
            in("ax") KERNEL_DATA,
            options(nostack, preserves_flags),
        );
        asm!("ltr ax", in("ax") TSS_SELECTOR, options(nostack, preserves_flags));
    }
}

fn tss_descriptor_low(base: u64, limit: u64) -> u64 {
    (limit & 0xffff)
        | ((base & 0xffff) << 16)
        | (((base >> 16) & 0xff) << 32)
        | (0x89 << 40)
        | (((limit >> 16) & 0xf) << 48)
        | (((base >> 24) & 0xff) << 56)
}
