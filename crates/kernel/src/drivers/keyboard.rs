use crate::util::{IrqGuard, SpinLock, SyncUnsafeCell};
use core::arch::asm;

const BUF_SIZE: usize = 1024;

static BUF:      SyncUnsafeCell<[u8; BUF_SIZE]> = SyncUnsafeCell::new([0; BUF_SIZE]);
static HEAD:     SyncUnsafeCell<usize>           = SyncUnsafeCell::new(0);
static TAIL:     SyncUnsafeCell<usize>           = SyncUnsafeCell::new(0);
static SHIFT:    SyncUnsafeCell<bool>            = SyncUnsafeCell::new(false);
static CTRL:     SyncUnsafeCell<bool>            = SyncUnsafeCell::new(false);
static EXTENDED: SyncUnsafeCell<bool>            = SyncUnsafeCell::new(false);

static LOCK: SpinLock = SpinLock::new();

struct KeyboardGuard {
    _irq: IrqGuard,
}

impl KeyboardGuard {
    fn new() -> Self {
        // Disable interrupts BEFORE taking the lock. If we locked first, an IRQ
        // landing on this CPU in the window before `cli` would re-enter the
        // keyboard handler, which takes the same non-reentrant LOCK and would
        // spin forever (single-CPU self-deadlock). Fields drop in declaration
        // order, so `_irq` (re-enable) runs after the manual unlock in Drop.
        let _irq = IrqGuard::new();
        LOCK.lock();
        Self { _irq }
    }
}

impl Drop for KeyboardGuard {
    fn drop(&mut self) {
        LOCK.unlock();
    }
}

pub fn inject_keys(keys: &[u8]) {
    let _guard = KeyboardGuard::new();
    for &ch in keys {
        unsafe { push_byte(ch); }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Left,
    Right,
}

unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

unsafe fn inb(port: u16) -> u8 {
    unsafe {
        let val: u8;
        asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
        val
    }
}

const fn sc128<const N: usize>(s: [u8; N]) -> [u8; 128] {
    let mut a = [0u8; 128];
    let mut i = 0;
    while i < N { a[i] = s[i]; i += 1; }
    a
}

static SCANCODE: [u8; 128] = sc128([
    0, 0, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b'-', b'=', 8, b'\t',
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0,
    b'a', b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';', b'\'', b'`', 0, b'\\',
    b'z', b'x', b'c', b'v', b'b', b'n', b'm', b',', b'.', b'/', 0, b' ', 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, b'7', b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', b'2',
    b'3', b'0', b'.',
]);

static SCANCODE_SHIFT: [u8; 128] = sc128([
    0, 0, b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*', b'(', b')', b'_', b'+', 8, b'\t',
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0,
    b'A', b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':', b'"', b'~', 0, b'|',
    b'Z', b'X', b'C', b'V', b'B', b'N', b'M', b'<', b'>', b'?', 0, b' ', 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, b'7', b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1', b'2',
    b'3', b'0', b'.',
]);

pub(crate) unsafe fn init() {
    let _guard = KeyboardGuard::new();
    unsafe {
        let mut i = 0u8;
        while inb(0x64) & 0x01 != 0 && i < 32 { inb(0x60); i += 1; }
        outb(0x64, 0xAE);
        *BUF.0.get()      = [0; BUF_SIZE];
        *HEAD.0.get()     = 0;
        *TAIL.0.get()     = 0;
        *SHIFT.0.get()    = false;
        *CTRL.0.get()     = false;
        *EXTENDED.0.get() = false;
    }
}

pub(crate) unsafe fn poll() {
    let _guard = KeyboardGuard::new();
    unsafe {
        let status = inb(0x64);
        if status & 0x01 == 0 { return; }
        if status & 0x20 != 0 { return; }
        let sc = inb(0x60);

        if sc == 0xE0 { *EXTENDED.0.get() = true; return; }

        if *EXTENDED.0.get() {
            *EXTENDED.0.get() = false;
            if sc & 0x80 != 0 { return; }
            let ch = match sc { 0x4B => 0x80, 0x4D => 0x81, 0x48 => 0x82, 0x50 => 0x83, _ => 0 };
            if ch != 0 { push_byte(ch); }
            return;
        }

        if sc == 0x2A || sc == 0x36 { *SHIFT.0.get() = true;  return; }
        if sc == 0xAA || sc == 0xB6 { *SHIFT.0.get() = false; *EXTENDED.0.get() = false; return; }
        if sc == 0x1D { *CTRL.0.get() = true;  return; }
        if sc == 0x9D { *CTRL.0.get() = false; return; }
        if sc & 0x80 != 0 { return; }

        if *CTRL.0.get() {
            let base = SCANCODE[(sc & 0x7F) as usize];
            if base >= b'a' && base <= b'z'      { push_byte(base - b'a' + 1); }
            else if base >= b'A' && base <= b'Z' { push_byte(base - b'A' + 1); }
            return;
        }

        let ch = match sc {
            0x39 => b' ',
            _ if *SHIFT.0.get() => SCANCODE_SHIFT[(sc & 0x7F) as usize],
            _ => SCANCODE[(sc & 0x7F) as usize],
        };
        if ch != 0 { push_byte(ch); }
    }
}

unsafe fn push_byte(ch: u8) {
    unsafe {
        if ch == 0x03 {
            // Ctrl+C goes to the graphical foreground (framebuffer owner) if any,
            // otherwise to the text foreground process (the program the shell is
            // currently waiting on). Falls through to the key buffer only when
            // there is no foreground at all (e.g. shell prompt, background jobs).
            if let Some(pid) =
                crate::drivers::fb_owner::owner().or_else(crate::process::foreground_pid)
            {
                crate::process::send_sigint(pid);
                return;
            }
        }
        if crate::process::wakeup_key_waiters(ch) > 0 { return; }
        let next = (*HEAD.0.get() + 1) % BUF_SIZE;
        if next != *TAIL.0.get() {
            (*BUF.0.get())[*HEAD.0.get()] = ch;
            *HEAD.0.get() = next;
        }
    }
}

pub fn get_key() -> Option<Key> {
    let _guard = KeyboardGuard::new();
    unsafe {
        if *HEAD.0.get() == *TAIL.0.get() { return None; }
        let ch = (*BUF.0.get())[*TAIL.0.get()];
        *TAIL.0.get() = (*TAIL.0.get() + 1) % BUF_SIZE;
        match ch {
            0x80 => Some(Key::Left),
            0x81 => Some(Key::Right),
            _ => Some(Key::Char(ch as char)),
        }
    }
}

pub fn get() -> Option<char> {
    match get_key()? {
        Key::Char(ch) => Some(ch),
        _ => None,
    }
}

pub fn get_raw() -> Option<u8> {
    let _guard = KeyboardGuard::new();
    unsafe {
        if *HEAD.0.get() == *TAIL.0.get() { return None; }
        let ch = (*BUF.0.get())[*TAIL.0.get()];
        *TAIL.0.get() = (*TAIL.0.get() + 1) % BUF_SIZE;
        Some(ch)
    }
}
