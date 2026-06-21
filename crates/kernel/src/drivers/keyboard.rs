use crate::util::{IrqGuard, SpinLock, SyncUnsafeCell};
use core::arch::asm;

const BUF_SIZE: usize = 1024;
const EBUF_SIZE: usize = 512;

// Key-event word layout (returned by SYS_KEYBOARD_POLL). Low 8 bits are the key code:
// a translated character for text keys, or one of the KEY_* codes below for non-text
// keys (arrows, modifiers, Esc, F-keys). The high bits are flags.
pub const KEY_RELEASE: u16 = 0x100; // set on release (clear on press)
pub const MOD_SHIFT:   u16 = 0x200; // modifier state at event time
pub const MOD_CTRL:    u16 = 0x400;
pub const MOD_ALT:     u16 = 0x800;

// Non-text key codes (low byte). 0x80..0x83 are the arrows (kept stable).
pub const KEY_LEFT:   u16 = 0x80;
pub const KEY_RIGHT:  u16 = 0x81;
pub const KEY_UP:     u16 = 0x82;
pub const KEY_DOWN:   u16 = 0x83;
pub const KEY_LSHIFT: u16 = 0x84;
pub const KEY_RSHIFT: u16 = 0x85;
pub const KEY_LCTRL:  u16 = 0x86;
pub const KEY_RCTRL:  u16 = 0x87;
pub const KEY_LALT:   u16 = 0x88;
pub const KEY_RALT:   u16 = 0x89;
pub const KEY_CAPS:   u16 = 0x8A;
pub const KEY_ESC:    u16 = 0x8B;
pub const KEY_F1:     u16 = 0x90; // F1..F12 are KEY_F1 + (n - 1), i.e. 0x90..0x9B

static BUF:      SyncUnsafeCell<[u8; BUF_SIZE]> = SyncUnsafeCell::new([0; BUF_SIZE]);
static HEAD:     SyncUnsafeCell<usize>           = SyncUnsafeCell::new(0);
static TAIL:     SyncUnsafeCell<usize>           = SyncUnsafeCell::new(0);
static SHIFT:    SyncUnsafeCell<bool>            = SyncUnsafeCell::new(false);
static CTRL:     SyncUnsafeCell<bool>            = SyncUnsafeCell::new(false);
static ALT:      SyncUnsafeCell<bool>            = SyncUnsafeCell::new(false);
static EXTENDED: SyncUnsafeCell<bool>            = SyncUnsafeCell::new(false);

// Event stream (press + release), consumed only by the graphical focus owner. The plain
// BUF above stays a press-only stream of translated characters for text/console readers
// (the shell), which have no use for key-up events. An event is the translated key code
// in the low byte, OR'd with KEY_RELEASE on release.
static EBUF:  SyncUnsafeCell<[u16; EBUF_SIZE]> = SyncUnsafeCell::new([0; EBUF_SIZE]);
static EHEAD: SyncUnsafeCell<usize>            = SyncUnsafeCell::new(0);
static ETAIL: SyncUnsafeCell<usize>            = SyncUnsafeCell::new(0);

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
        *EBUF.0.get()     = [0; EBUF_SIZE];
        *EHEAD.0.get()    = 0;
        *ETAIL.0.get()    = 0;
        *SHIFT.0.get()    = false;
        *CTRL.0.get()     = false;
        *ALT.0.get()      = false;
        *EXTENDED.0.get() = false;
    }
}

pub(crate) unsafe fn poll() {
    let _guard = KeyboardGuard::new();
    unsafe {
        // Drain every keyboard byte currently pending, not just one: in polling
        // mode poll() runs once per timer tick, so reading a single byte loses
        // keystrokes when typing faster than the tick — the 8042 output buffer
        // overflows. Cap the loop so a stuck controller can't wedge us here.
        let mut iters = 0;
        while iters < 32 {
            iters += 1;
            let status = inb(0x64);
            if status & 0x01 == 0 { break; }  // no byte pending
            if status & 0x20 != 0 { break; }  // mouse byte at head — leave it for the mouse
            let sc = inb(0x60);

            if sc == 0xE0 { *EXTENDED.0.get() = true; continue; }

            let release = sc & 0x80 != 0;
            let base = sc & 0x7F;

            if *EXTENDED.0.get() {
                *EXTENDED.0.get() = false;
                // Extended (E0-prefixed) keys: arrows, plus the right-hand Ctrl/Alt.
                match base {
                    0x1D => { *CTRL.0.get() = !release; emit_key(KEY_RCTRL, release); continue; }
                    0x38 => { *ALT.0.get()  = !release; emit_key(KEY_RALT,  release); continue; }
                    _ => {}
                }
                let code = match base { 0x4B => KEY_LEFT, 0x4D => KEY_RIGHT, 0x48 => KEY_UP, 0x50 => KEY_DOWN, _ => 0 };
                if code != 0 {
                    if !release { push_byte(code as u8); }
                    emit_key(code, release);
                }
                continue;
            }

            // Modifier keys: update state, then report the key itself as an event so apps
            // can track Ctrl/Shift/Alt/Caps. They are never written to the text BUF.
            match base {
                0x2A => { *SHIFT.0.get() = !release; emit_key(KEY_LSHIFT, release); continue; }
                0x36 => { *SHIFT.0.get() = !release; emit_key(KEY_RSHIFT, release); continue; }
                0x1D => { *CTRL.0.get()  = !release; emit_key(KEY_LCTRL,  release); continue; }
                0x38 => { *ALT.0.get()   = !release; emit_key(KEY_LALT,   release); continue; }
                0x3A => { if !release { emit_key(KEY_CAPS, false); } else { emit_key(KEY_CAPS, true); } continue; }
                0x01 => { emit_key(KEY_ESC, release); continue; }
                0x3B..=0x44 => { emit_key(KEY_F1 + (base - 0x3B) as u16, release); continue; }
                0x57 => { emit_key(KEY_F1 + 10, release); continue; } // F11
                0x58 => { emit_key(KEY_F1 + 11, release); continue; } // F12
                _ => {}
            }

            // Ordinary character key. Releases carry the plain character (Ctrl ignored);
            // presses translate to a control char while Ctrl is held, matching the text
            // stream. The text BUF only ever gets presses.
            let plain = match base {
                0x39 => b' ',
                _ if *SHIFT.0.get() => SCANCODE_SHIFT[base as usize],
                _ => SCANCODE[base as usize],
            };
            if release {
                if plain != 0 { emit_key(plain as u16, true); }
                continue;
            }
            if *CTRL.0.get() {
                let ctrl_ch = if plain >= b'a' && plain <= b'z'      { plain - b'a' + 1 }
                              else if plain >= b'A' && plain <= b'Z' { plain - b'A' + 1 }
                              else { 0 };
                if ctrl_ch != 0 { push_byte(ctrl_ch); emit_key(ctrl_ch as u16, false); }
                continue;
            }
            if plain != 0 { push_byte(plain); emit_key(plain as u16, false); }
        }
    }
}

unsafe fn push_byte(ch: u8) {
    unsafe {
        if ch == 0x03 && crate::drivers::fb_owner::owner().is_none() {
            // Console mode: Ctrl+C interrupts the text foreground process (the program
            // the shell is waiting on). With a graphical owner, Ctrl+C is NOT a signal
            // to the compositor — it falls through as an ordinary key so the focused
            // app (e.g. a terminal) can forward it to its own child. Killing the
            // framebuffer owner here would take the whole desktop down.
            if let Some(pid) = crate::process::foreground_pid() {
                crate::process::send_sigint(pid);
                return;
            }
        }
        // Keyboard focus: a graphical program that owns the framebuffer also owns
        // the keyboard. Deliver only to it — wake a blocking read, otherwise buffer
        // for its non-blocking polls — and never wake other readers such as the
        // shell. The ring buffer is flushed on FB acquire/release (see flush()) so
        // keys typed under one focus never resurface under another.
        if let Some(owner) = crate::drivers::fb_owner::owner() {
            if crate::process::wakeup_key_waiter_for_pid(owner, ch) > 0 { return; }
            buffer_byte(ch);
            return;
        }

        if crate::process::wakeup_key_waiters(ch) > 0 { return; }
        buffer_byte(ch);
    }
}

unsafe fn buffer_byte(ch: u8) {
    unsafe {
        let next = (*HEAD.0.get() + 1) % BUF_SIZE;
        if next != *TAIL.0.get() {
            (*BUF.0.get())[*HEAD.0.get()] = ch;
            *HEAD.0.get() = next;
        }
    }
}

// Current modifier flags for the event word.
unsafe fn mods() -> u16 {
    unsafe {
        let mut m = 0;
        if *SHIFT.0.get() { m |= MOD_SHIFT; }
        if *CTRL.0.get()  { m |= MOD_CTRL; }
        if *ALT.0.get()   { m |= MOD_ALT; }
        m
    }
}

// Emit a key event: code in the low byte, KEY_RELEASE on release, plus the live modifier
// flags so any key event also reports whether Ctrl/Shift/Alt are held.
unsafe fn emit_key(code: u16, release: bool) {
    unsafe {
        let mut ev = (code & 0xFF) | mods();
        if release { ev |= KEY_RELEASE; }
        push_event(ev);
    }
}

// Buffer a press/release event, but only while a graphical program owns the framebuffer
// (it is the only consumer of key-up events). In text/console mode there is no keyup
// reader, so the event stream stays empty and costs nothing.
unsafe fn push_event(ev: u16) {
    unsafe {
        if crate::drivers::fb_owner::owner().is_none() { return; }
        let next = (*EHEAD.0.get() + 1) % EBUF_SIZE;
        if next != *ETAIL.0.get() {
            (*EBUF.0.get())[*EHEAD.0.get()] = ev;
            *EHEAD.0.get() = next;
        }
    }
}

/// Discard any buffered keystrokes. Called on framebuffer acquire/release so a
/// graphical program and the shell never inherit each other's typed-but-unread
/// keys across a focus change.
pub fn flush() {
    let _guard = KeyboardGuard::new();
    unsafe {
        *HEAD.0.get() = 0;
        *TAIL.0.get() = 0;
        *EHEAD.0.get() = 0;
        *ETAIL.0.get() = 0;
    }
}

/// Pull the next key event (press or release), or None. The low byte is the translated
/// key code; KEY_RELEASE is set for releases. Only populated for the graphical focus
/// owner (see push_event).
pub fn get_event() -> Option<u16> {
    let _guard = KeyboardGuard::new();
    unsafe {
        if *EHEAD.0.get() == *ETAIL.0.get() { return None; }
        let ev = (*EBUF.0.get())[*ETAIL.0.get()];
        *ETAIL.0.get() = (*ETAIL.0.get() + 1) % EBUF_SIZE;
        Some(ev)
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
