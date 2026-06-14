use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub struct SyncUnsafeCell<T>(pub UnsafeCell<T>);

pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    pub fn lock(&self) {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::arch::x86_64::_mm_pause();
        }
    }

    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub fn save_flags() -> u64 {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq; pop {}", out(reg) flags, options(nomem, preserves_flags));
    }
    flags
}

pub fn restore_flags(flags: u64) {
    unsafe {
        core::arch::asm!("push {}; popfq", in(reg) flags, options(nomem, preserves_flags));
    }
}

pub fn cli() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack));
    }
}

pub fn sti() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack));
    }
}

pub fn irq_save() -> u64 {
    let flags = save_flags();
    cli();
    flags
}

pub struct IrqGuard {
    flags: u64,
}

impl IrqGuard {
    pub fn new() -> Self {
        Self { flags: irq_save() }
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        restore_flags(self.flags);
    }
}

use core::sync::atomic::AtomicUsize;

pub struct ReentrantSpinLock {
    locked: AtomicBool,
    owner: AtomicUsize,
    depth: AtomicUsize,
}

impl ReentrantSpinLock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            owner: AtomicUsize::new(usize::MAX),
            depth: AtomicUsize::new(0),
        }
    }

    pub fn lock(&self) {
        let cpu = crate::smp::current_cpu_index();
        if self.owner.load(Ordering::Relaxed) == cpu && self.locked.load(Ordering::Relaxed) {
            self.depth.fetch_add(1, Ordering::Relaxed);
            return;
        }
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::arch::x86_64::_mm_pause();
        }
        self.owner.store(cpu, Ordering::Relaxed);
        self.depth.store(1, Ordering::Relaxed);
    }

    pub fn unlock(&self) {
        if self.depth.fetch_sub(1, Ordering::Relaxed) == 1 {
            self.owner.store(usize::MAX, Ordering::Relaxed);
            self.locked.store(false, Ordering::Release);
        }
    }
}

pub struct ReentrantIrqGuard {
    _irq: IrqGuard,
    lock: &'static ReentrantSpinLock,
}

impl ReentrantIrqGuard {
    pub fn new(lock: &'static ReentrantSpinLock) -> Self {
        let irq = IrqGuard::new();
        lock.lock();
        Self { _irq: irq, lock }
    }
}

impl Drop for ReentrantIrqGuard {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}

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
