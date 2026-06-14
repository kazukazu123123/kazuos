use crate::util::SyncUnsafeCell;
use core::arch::asm;

const IA32_APIC_BASE_MSR: u32 = 0x1B;

static BASE: SyncUnsafeCell<u64> = SyncUnsafeCell::new(0);

unsafe fn read_msr(msr: u32) -> u64 {
    unsafe {
        let low: u32;
        let high: u32;
        asm!("rdmsr", in("ecx") msr, out("eax") low, out("edx") high, options(nomem, nostack));
        ((high as u64) << 32) | (low as u64)
    }
}

unsafe fn reg(offset: usize) -> *mut u32 {
    unsafe { (*BASE.0.get() as *mut u32).add(offset / 4) }
}

unsafe fn read(offset: usize) -> u32 {
    unsafe { reg(offset).read_volatile() }
}

unsafe fn write(offset: usize, value: u32) {
    unsafe { reg(offset).write_volatile(value) }
}

pub(crate) unsafe fn init() {
    unsafe {
        let base = read_msr(IA32_APIC_BASE_MSR) & 0xFFFFF000;
        *BASE.0.get() = base;
    }
}

pub(crate) unsafe fn enable() {
    unsafe {
        write(0xF0, read(0xF0) | 0x100);
    }
}

pub(crate) unsafe fn set_timer(vector: u8, initial_count: u32) {
    unsafe {
        // Stop any running timer first (mask + clear count)
        write(0x320, 0x10000); // Masked
        write(0x380, 0); // Initial Count = 0
        core::arch::asm!("mfence");

        // Now configure
        write(0x320, (vector as u32) | 0x20000); // Periodic mode
        write(0x3E0, 0x3); // Divide by 16
        write(0x380, initial_count);
    }
}

pub(crate) unsafe fn eoi() {
    unsafe {
        write(0xB0, 0);
    }
}

pub fn local_apic_id() -> u8 {
    unsafe { (read(0x20) >> 24) as u8 }
}

pub fn icr_low() -> u32 {
    unsafe { read(0x300) }
}

pub fn icr_high() -> u32 {
    unsafe { read(0x310) }
}

pub unsafe fn send_ipi(destination: u8, vector: u8, flags: u32) {
    unsafe {
        // Wait for ICR idle
        while read(0x300) & 0x1000 != 0 {}
        write(0x310, (destination as u32) << 24);
        core::arch::asm!("mfence");
        write(0x300, flags | (vector as u32));
        core::arch::asm!("mfence");
    }
}
