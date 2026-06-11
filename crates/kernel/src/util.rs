use core::cell::UnsafeCell;

pub struct SyncUnsafeCell<T>(pub UnsafeCell<T>);

unsafe impl<T> Sync for SyncUnsafeCell<T> {}

impl<T> SyncUnsafeCell<T> {
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }
}

pub fn rdtsc() -> u64 {
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | (lo as u64)
    }
}

pub(crate) unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

pub(crate) unsafe fn inb(port: u16) -> u8 {
    unsafe {
        let value: u8;
        core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack));
        value
    }
}

pub(crate) unsafe fn outw(port: u16, value: u16) {
    unsafe {
        core::arch::asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack));
    }
}

pub(crate) unsafe fn outd(port: u16, value: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack));
    }
}

pub(crate) unsafe fn ind(port: u16) -> u32 {
    unsafe {
        let value: u32;
        core::arch::asm!("in eax, dx", out("eax") value, in("dx") port, options(nomem, nostack));
        value
    }
}

pub fn pause() {
    core::arch::x86_64::_mm_pause();
}

pub fn hlt() {
    unsafe {
        core::arch::asm!("sti; hlt", options(nostack));
    }
}

pub fn wait_ms(ms: u64, tsc_per_ms: u64) {
    let end = rdtsc() + tsc_per_ms * ms;
    while rdtsc() < end {
        pause();
    }
}
