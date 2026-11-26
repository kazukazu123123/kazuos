use crate::util::{IrqGuard, SpinLock, SyncUnsafeCell};

// Unified mouse event queue. Every mouse driver (PS/2, USB xHCI, ...) injects
// relative motion through inject(), and userspace reaches the queue through
// /dev/input/mouse: drivers write event records, the graphical consumer
// reads one merged stream.
// Drivers keep no per-driver totals or epochs; the queue order is the event
// order, so consumers just apply each delta as it comes.
//
// An event carries the buttons state after the change and the relative delta
// since the previous event. dy is up-positive, the convention every producer
// already converts to. Events are only buffered while a framebuffer owner
// exists, mirroring the keyboard event stream: mouse input belongs to the
// graphical session, and motion produced while nobody graphical is running
// must not leak into the next session.

const EBUF_SIZE: usize = 512;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct MouseEvent {
    pub buttons: u8,
    pub _pad: [u8; 3],
    pub dx: i32,
    pub dy: i32,
}

static EBUF:  SyncUnsafeCell<[MouseEvent; EBUF_SIZE]> = SyncUnsafeCell::new([MouseEvent { buttons: 0, _pad: [0; 3], dx: 0, dy: 0 }; EBUF_SIZE]);
static EHEAD: SyncUnsafeCell<usize> = SyncUnsafeCell::new(0);
static ETAIL: SyncUnsafeCell<usize> = SyncUnsafeCell::new(0);

static LOCK: SpinLock = SpinLock::new();

struct MouseGuard {
    _irq: IrqGuard,
}

impl MouseGuard {
    fn new() -> Self {
        // Disable interrupts BEFORE taking the lock (same reasoning as the
        // keyboard guard: an IRQ landing in between could re-enter and spin
        // forever on the non-reentrant LOCK).
        let _irq = IrqGuard::new();
        LOCK.lock();
        Self { _irq }
    }
}

impl Drop for MouseGuard {
    fn drop(&mut self) {
        LOCK.unlock();
    }
}

pub fn inject(buttons: u8, dx: i32, dy: i32) {
    let _guard = MouseGuard::new();
    unsafe {
        if crate::drivers::fb_owner::owner().is_none() { return; }
        let next = (*EHEAD.0.get() + 1) % EBUF_SIZE;
        if next != *ETAIL.0.get() {
            (*EBUF.0.get())[*EHEAD.0.get()] = MouseEvent { buttons, _pad: [0; 3], dx, dy };
            *EHEAD.0.get() = next;
        }
    }
}

/// Pull the next mouse event, or None when the queue is empty.
pub fn get_event() -> Option<MouseEvent> {
    let _guard = MouseGuard::new();
    unsafe {
        if *EHEAD.0.get() == *ETAIL.0.get() { return None; }
        let ev = (*EBUF.0.get())[*ETAIL.0.get()];
        *ETAIL.0.get() = (*ETAIL.0.get() + 1) % EBUF_SIZE;
        Some(ev)
    }
}

/// Discard any buffered events. Called on framebuffer acquire/release so a
/// graphical program never inherits motion produced under the previous owner.
pub fn flush() {
    let _guard = MouseGuard::new();
    unsafe {
        *EHEAD.0.get() = 0;
        *ETAIL.0.get() = 0;
    }
}

// ── /dev/input/mouse ─────────────────────────────────────────────────────────
//
// The userspace face of this queue. read returns whole 12-byte MouseEvent
// records (as many as fit), write injects records (driver processes only).
// Reads are focus-gated like the keyboard event stream: while a framebuffer
// owner exists, only the owner may drain the queue.

use crate::fs::devfs::DeviceOps;

static MOUSE_OPS: DeviceOps = DeviceOps {
    open: mouse_open,
    close: mouse_close,
    read: mouse_read,
    write: mouse_write,
    ioctl: mouse_ioctl,
};

pub fn init_devfs() {
    crate::fs::devfs::register("/dev/input/mouse", &MOUSE_OPS);
}

fn mouse_open() -> u64 { 1 }
fn mouse_close(_handle: u64) {}

fn mouse_read(_handle: u64, buf: &mut [u8]) -> usize {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    if matches!(crate::drivers::fb_owner::owner(), Some(o) if o != caller) { return 0; }
    let mut written = 0;
    while written + 12 <= buf.len() {
        let Some(ev) = get_event() else { break; };
        buf[written] = ev.buttons;
        buf[written + 1..written + 4].fill(0);
        buf[written + 4..written + 8].copy_from_slice(&ev.dx.to_le_bytes());
        buf[written + 8..written + 12].copy_from_slice(&ev.dy.to_le_bytes());
        written += 12;
    }
    written
}

fn mouse_write(_handle: u64, buf: &[u8]) -> usize {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    if caller == 0 || crate::process::privilege_level(caller) > crate::process::PrivilegeLevel::Driver {
        return 0;
    }
    let mut read = 0;
    while read + 12 <= buf.len() {
        let dx = i32::from_le_bytes(buf[read + 4..read + 8].try_into().unwrap()).clamp(-4096, 4096);
        let dy = i32::from_le_bytes(buf[read + 8..read + 12].try_into().unwrap()).clamp(-4096, 4096);
        inject(buf[read], dx, dy);
        read += 12;
    }
    read
}

fn mouse_ioctl(_handle: u64, _cmd: u64, _arg: u64) -> i64 { -1 }
