#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

// A tiny first-party desktop: a launcher window with buttons that open a custom
// Task Manager (and, in a later phase, a real shell terminal). It is an ordinary
// ring3 program — framebuffer + the ps2mouse IPC channel are the only privileges,
// the same any third-party app could use. Quit with Ctrl+C.
//
// Rendering is double-buffered: everything is drawn into a heap back buffer, then
// copied to the framebuffer in one shot, so dragging windows does not flicker. The
// event loop is tick-driven and polls the mouse non-blocking (SYS_IPC_TRY_RECV) so
// it can also refresh the Task Manager live.

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

// Shared mouse state, written only by the input thread and read by the render thread.
// Single writer + atomics means no lock is needed. The render thread draws the cursor
// from these, so a heavy frame never starves mouse processing or backs up events.
static MOUSE_X: AtomicI32 = AtomicI32::new(0);
static MOUSE_Y: AtomicI32 = AtomicI32::new(0);
static MOUSE_BTN: AtomicU32 = AtomicU32::new(0); // raw PS/2 button byte
static SCR_W: AtomicI32 = AtomicI32::new(0);
static SCR_H: AtomicI32 = AtomicI32::new(0);
static MOUSE_IPC: AtomicU64 = AtomicU64::new(0); // ps2mouse channel id

// Input thread: drain the mouse channel and keep MOUSE_X/Y/BTN current, independent of
// how long a render frame takes. Uses non-blocking TRY_RECV + a short sleep rather than
// a blocking RECV, because IPC blocking is keyed on the process main thread and would
// otherwise park the render thread instead of this one.
fn input_loop() {
    let ipc = MOUSE_IPC.load(Ordering::Relaxed);
    let sw = SCR_W.load(Ordering::Relaxed);
    let sh = SCR_H.load(Ordering::Relaxed);
    let mut msg = [0u8; 5];
    loop {
        loop {
            let n = syscall(SYS_IPC_TRY_RECV, ipc, msg.as_mut_ptr() as u64, 5);
            if n != 5 { break; }
            let dx = i16::from_le_bytes([msg[1], msg[2]]) as i32;
            let dy = i16::from_le_bytes([msg[3], msg[4]]) as i32;
            let nx = (MOUSE_X.load(Ordering::Relaxed) + dx).clamp(0, sw - 1);
            let ny = (MOUSE_Y.load(Ordering::Relaxed) - dy).clamp(0, sh - 1);
            MOUSE_X.store(nx, Ordering::Relaxed);
            MOUSE_Y.store(ny, Ordering::Relaxed);
            MOUSE_BTN.store(msg[0] as u32, Ordering::Relaxed);
        }
        syscall(SYS_SLEEP, 8, SLEEP_UNIT_MS, 0);
    }
}

#[repr(C)]
struct FbInfo {
    base:   u64,
    width:  u32,
    height: u32,
    stride: u32,
    format: u32, // 0=RGB, 1=BGR
}

const NAME_LEN: usize = 32;

// SYS_KEYBOARD_POLL event-mode flag: set in the returned word when the key was released.
const KEY_RELEASE: u16 = 0x100;

#[repr(C)]
struct ProcessInfo {
    pid: u64,
    state: u64,
    image_name: [u8; NAME_LEN],
    start_tsc: u64,
    entry: u64,
    stack_top: u64,
    step: u64,
    cpu_ticks: u64,
    memory_bytes: u64,
    parent: u64,
}

// X11 7x14 BDF font (public domain), ASCII 0x20-0x7E, 14 bytes/glyph.
const GLYPH_W: i32 = 7;
const GLYPH_H: i32 = 14;
const ADVANCE: i32 = 8;  // per-char horizontal step
const LINE_H:  i32 = 16; // per-row vertical step in lists
const FONT_7X14: &[u8] = &[
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x00, 0x10, 0x10, 0x00, 0x00, 0x00, 0x6c, 0x24, 0x24,
    0x48, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0a, 0x0a, 0x0a, 0x7e,
    0x14, 0x14, 0x7e, 0x28, 0x28, 0x28, 0x00, 0x00, 0x00, 0x00, 0x08, 0x3c, 0x4a, 0x4a, 0x28, 0x1c,
    0x0a, 0x4a, 0x4a, 0x3c, 0x08, 0x00, 0x00, 0x00, 0x32, 0x4a, 0x4c, 0x38, 0x08, 0x10, 0x1c, 0x32,
    0x52, 0x4c, 0x00, 0x00, 0x00, 0x00, 0x18, 0x24, 0x24, 0x24, 0x18, 0x32, 0x4a, 0x44, 0x4c, 0x32,
    0x00, 0x00, 0x00, 0x18, 0x08, 0x08, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x02, 0x04, 0x08, 0x08, 0x10, 0x10, 0x10, 0x10, 0x10, 0x08, 0x08, 0x04, 0x02, 0x00, 0x40,
    0x20, 0x10, 0x10, 0x08, 0x08, 0x08, 0x08, 0x08, 0x10, 0x10, 0x20, 0x40, 0x00, 0x00, 0x08, 0x2a,
    0x1c, 0x08, 0x1c, 0x2a, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10, 0x10,
    0x10, 0x7c, 0x10, 0x10, 0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x30, 0x10, 0x10, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7e, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x1c,
    0x08, 0x00, 0x02, 0x02, 0x04, 0x04, 0x08, 0x08, 0x08, 0x10, 0x10, 0x20, 0x20, 0x40, 0x40, 0x00,
    0x00, 0x00, 0x18, 0x24, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x24, 0x18, 0x00, 0x00, 0x00, 0x00,
    0x08, 0x18, 0x28, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x3e, 0x00, 0x00, 0x00, 0x00, 0x3c, 0x42,
    0x42, 0x02, 0x04, 0x04, 0x08, 0x10, 0x20, 0x7e, 0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x02,
    0x1c, 0x02, 0x02, 0x42, 0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x04, 0x0c, 0x14, 0x14, 0x24, 0x24,
    0x44, 0x7e, 0x04, 0x04, 0x00, 0x00, 0x00, 0x00, 0x7e, 0x40, 0x40, 0x7c, 0x42, 0x02, 0x02, 0x42,
    0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x1c, 0x22, 0x42, 0x40, 0x5c, 0x62, 0x42, 0x42, 0x42, 0x3c,
    0x00, 0x00, 0x00, 0x00, 0x7e, 0x42, 0x44, 0x04, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x00, 0x00,
    0x00, 0x00, 0x3c, 0x42, 0x42, 0x24, 0x18, 0x24, 0x42, 0x42, 0x42, 0x3c, 0x00, 0x00, 0x00, 0x00,
    0x3c, 0x42, 0x42, 0x42, 0x46, 0x3a, 0x02, 0x42, 0x44, 0x38, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x08, 0x1c, 0x08, 0x00, 0x00, 0x08, 0x1c, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x18,
    0x18, 0x00, 0x00, 0x18, 0x08, 0x08, 0x10, 0x00, 0x00, 0x00, 0x04, 0x08, 0x10, 0x20, 0x40, 0x20,
    0x10, 0x08, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7e, 0x00, 0x00, 0x7e, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x20, 0x10, 0x08, 0x04, 0x08, 0x10, 0x20, 0x40, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x04, 0x08, 0x08, 0x08, 0x00, 0x08, 0x08, 0x00, 0x00,
    0x00, 0x00, 0x1c, 0x22, 0x4e, 0x52, 0x52, 0x52, 0x52, 0x4e, 0x20, 0x1e, 0x00, 0x00, 0x00, 0x00,
    0x18, 0x24, 0x42, 0x42, 0x42, 0x7e, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00, 0x7c, 0x22,
    0x22, 0x22, 0x3c, 0x22, 0x22, 0x22, 0x22, 0x7c, 0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x40,
    0x40, 0x40, 0x40, 0x42, 0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x7c, 0x22, 0x22, 0x22, 0x22, 0x22,
    0x22, 0x22, 0x22, 0x7c, 0x00, 0x00, 0x00, 0x00, 0x7e, 0x40, 0x40, 0x40, 0x78, 0x40, 0x40, 0x40,
    0x40, 0x7e, 0x00, 0x00, 0x00, 0x00, 0x7e, 0x40, 0x40, 0x40, 0x78, 0x40, 0x40, 0x40, 0x40, 0x40,
    0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x40, 0x40, 0x4e, 0x42, 0x42, 0x46, 0x3a, 0x00, 0x00,
    0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x7e, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00,
    0x3e, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x3e, 0x00, 0x00, 0x00, 0x00, 0x0e, 0x04,
    0x04, 0x04, 0x04, 0x04, 0x04, 0x44, 0x44, 0x38, 0x00, 0x00, 0x00, 0x00, 0x42, 0x44, 0x48, 0x50,
    0x60, 0x50, 0x48, 0x44, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40,
    0x40, 0x40, 0x40, 0x7e, 0x00, 0x00, 0x00, 0x00, 0x42, 0x66, 0x66, 0x5a, 0x5a, 0x42, 0x42, 0x42,
    0x42, 0x42, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x62, 0x62, 0x52, 0x4a, 0x46, 0x46, 0x42, 0x42,
    0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3c, 0x00, 0x00,
    0x00, 0x00, 0x7c, 0x42, 0x42, 0x42, 0x42, 0x7c, 0x40, 0x40, 0x40, 0x40, 0x00, 0x00, 0x00, 0x00,
    0x3c, 0x42, 0x42, 0x42, 0x42, 0x42, 0x72, 0x4a, 0x46, 0x3c, 0x04, 0x02, 0x00, 0x00, 0x7c, 0x42,
    0x42, 0x42, 0x42, 0x7c, 0x48, 0x44, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x40,
    0x30, 0x0c, 0x02, 0x42, 0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0xfe, 0x10, 0x10, 0x10, 0x10, 0x10,
    0x10, 0x10, 0x10, 0x10, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42, 0x42,
    0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x24, 0x24, 0x24, 0x18, 0x18, 0x18,
    0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x42, 0x5a, 0x5a, 0x66, 0x66, 0x42, 0x00, 0x00,
    0x00, 0x00, 0x42, 0x42, 0x24, 0x24, 0x18, 0x18, 0x24, 0x24, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00,
    0x44, 0x44, 0x44, 0x28, 0x28, 0x10, 0x10, 0x10, 0x10, 0x10, 0x00, 0x00, 0x00, 0x00, 0x7e, 0x02,
    0x04, 0x08, 0x08, 0x10, 0x20, 0x20, 0x40, 0x7e, 0x00, 0x00, 0x00, 0x1e, 0x10, 0x10, 0x10, 0x10,
    0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1e, 0x40, 0x40, 0x20, 0x20, 0x10, 0x10, 0x10, 0x08,
    0x08, 0x04, 0x04, 0x02, 0x02, 0x00, 0x00, 0x78, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08, 0x08,
    0x08, 0x08, 0x08, 0x78, 0x00, 0x18, 0x24, 0x42, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x7e,
    0x00, 0x0c, 0x08, 0x08, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x3c, 0x42, 0x0e, 0x32, 0x42, 0x46, 0x3a, 0x00, 0x00, 0x00, 0x00, 0x40, 0x40,
    0x40, 0x5c, 0x62, 0x42, 0x42, 0x42, 0x62, 0x5c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3c,
    0x42, 0x40, 0x40, 0x40, 0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x02, 0x02, 0x02, 0x3a, 0x46, 0x42,
    0x42, 0x42, 0x46, 0x3a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x7e, 0x40,
    0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x12, 0x10, 0x10, 0x7c, 0x10, 0x10, 0x10, 0x10, 0x10,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3a, 0x44, 0x44, 0x44, 0x38, 0x20, 0x5c, 0x42, 0x3c,
    0x00, 0x00, 0x40, 0x40, 0x40, 0x5c, 0x62, 0x42, 0x42, 0x42, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00,
    0x08, 0x08, 0x00, 0x18, 0x08, 0x08, 0x08, 0x08, 0x08, 0x3e, 0x00, 0x00, 0x00, 0x00, 0x04, 0x04,
    0x00, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x04, 0x44, 0x44, 0x38, 0x00, 0x00, 0x40, 0x40, 0x40, 0x44,
    0x48, 0x50, 0x70, 0x48, 0x44, 0x42, 0x00, 0x00, 0x00, 0x00, 0x18, 0x08, 0x08, 0x08, 0x08, 0x08,
    0x08, 0x08, 0x08, 0x3e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x68, 0x54, 0x54, 0x54, 0x54,
    0x54, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5c, 0x62, 0x42, 0x42, 0x42, 0x42, 0x42,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3c, 0x42, 0x42, 0x42, 0x42, 0x42, 0x3c, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x5c, 0x62, 0x42, 0x42, 0x42, 0x62, 0x5c, 0x40, 0x40, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x3a, 0x46, 0x42, 0x42, 0x42, 0x46, 0x3a, 0x02, 0x02, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x5c, 0x62, 0x42, 0x40, 0x40, 0x40, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3c,
    0x42, 0x20, 0x18, 0x04, 0x42, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x10, 0x10, 0x10, 0x7c, 0x10, 0x10,
    0x10, 0x10, 0x12, 0x0c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x42,
    0x46, 0x3a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x44, 0x44, 0x44, 0x28, 0x28, 0x10, 0x10,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x44, 0x44, 0x54, 0x54, 0x54, 0x54, 0x28, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x42, 0x42, 0x24, 0x18, 0x24, 0x42, 0x42, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x42, 0x42, 0x42, 0x42, 0x46, 0x3a, 0x02, 0x42, 0x3c, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x7e, 0x04, 0x08, 0x10, 0x10, 0x20, 0x7e, 0x00, 0x00, 0x00, 0x06, 0x08, 0x08, 0x08, 0x08,
    0x08, 0x10, 0x08, 0x08, 0x08, 0x08, 0x08, 0x06, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x10,
    0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x00, 0x60, 0x10, 0x10, 0x10, 0x10, 0x10, 0x08, 0x10, 0x10,
    0x10, 0x10, 0x10, 0x60, 0x00, 0x20, 0x52, 0x4a, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00,
];

// dirty rectangle (x1/y1 exclusive)
#[derive(Clone, Copy)]
struct Rect { x0: i32, y0: i32, x1: i32, y1: i32 }

impl Rect {
    fn empty() -> Rect { Rect { x0: i32::MAX, y0: i32::MAX, x1: i32::MIN, y1: i32::MIN } }
    fn is_empty(&self) -> bool { self.x0 >= self.x1 || self.y0 >= self.y1 }
    fn add(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 { return; }
        self.x0 = self.x0.min(x);
        self.y0 = self.y0.min(y);
        self.x1 = self.x1.max(x + w);
        self.y1 = self.y1.max(y + h);
    }
    fn intersects(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        !self.is_empty() && x < self.x1 && x + w > self.x0 && y < self.y1 && y + h > self.y0
    }
}

// double-buffered screen with a clip rect for partial redraws
struct Screen {
    base:   *mut u32,
    width:  i32,
    height: i32,
    stride: i32,
    format: u32,
    clip:   Rect, // drawing is restricted to this rect (in screen coords)
    bb:     alloc::vec::Vec<u32>,
}

impl Screen {
    fn new(info: &FbInfo) -> Screen {
        let mut bb = alloc::vec::Vec::new();
        bb.resize((info.stride * info.height) as usize, 0u32);
        Screen {
            base: info.base as *mut u32,
            width: info.width as i32,
            height: info.height as i32,
            stride: info.stride as i32,
            format: info.format,
            clip: Rect { x0: 0, y0: 0, x1: info.width as i32, y1: info.height as i32 },
            bb,
        }
    }

    // An off-screen buffer (no framebuffer behind it) used as a window's content
    // canvas. present() is never called on these; they are blitted onto the screen.
    // Returns None if the backing memory can't be allocated (so opening a window can
    // fail gracefully instead of aborting when memory runs out).
    fn offscreen(w: i32, h: i32, format: u32) -> Option<Screen> {
        let w = w.max(1);
        let h = h.max(1);
        let n = (w * h) as usize;
        let mut bb = alloc::vec::Vec::new();
        bb.try_reserve_exact(n).ok()?;
        bb.resize(n, 0u32); // capacity is reserved, so this won't reallocate
        Some(Screen {
            base: core::ptr::null_mut(),
            width: w, height: h, stride: w, format,
            clip: Rect { x0: 0, y0: 0, x1: w, y1: h },
            bb,
        })
    }

    fn full_rect(&self) -> Rect { Rect { x0: 0, y0: 0, x1: self.width, y1: self.height } }

    fn set_clip(&mut self, r: Rect) {
        self.clip = Rect {
            x0: r.x0.max(0),
            y0: r.y0.max(0),
            x1: r.x1.min(self.width),
            y1: r.y1.min(self.height),
        };
    }

    fn pack(&self, r: u8, g: u8, b: u8) -> u32 {
        if self.format == 1 {
            (b as u32) << 16 | (g as u32) << 8 | (r as u32)
        } else {
            (r as u32) << 16 | (g as u32) << 8 | (b as u32)
        }
    }

    fn put(&mut self, x: i32, y: i32, color: u32) {
        if x < self.clip.x0 || x >= self.clip.x1 || y < self.clip.y0 || y >= self.clip.y1 { return; }
        self.bb[(y * self.stride + x) as usize] = color;
    }

    fn fill(&mut self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        // Iterate only the part that overlaps the current clip rect so a clipped
        // redraw doesn't loop over the whole (off-clip) area.
        let x0 = x.max(self.clip.x0);
        let y0 = y.max(self.clip.y0);
        let x1 = (x + w).min(self.clip.x1);
        let y1 = (y + h).min(self.clip.y1);
        for yy in y0..y1 {
            for xx in x0..x1 {
                self.put(xx, yy, color);
            }
        }
    }

    fn glyph(&mut self, x: i32, y: i32, c: u8, color: u32) {
        if !(0x20..=0x7E).contains(&c) { return; }
        let base = (c - 0x20) as usize * GLYPH_H as usize;
        for row in 0..GLYPH_H {
            let bits = FONT_7X14[base + row as usize];
            for col in 0..GLYPH_W {
                if bits & (0x80 >> col) != 0 {
                    self.put(x + col, y + row, color);
                }
            }
        }
    }

    fn text(&mut self, mut x: i32, y: i32, s: &[u8], color: u32) {
        for &c in s {
            self.glyph(x, y, c, color);
            x += ADVANCE;
        }
    }

    // Copy only the given rect from the back buffer to the framebuffer.
    fn present(&self, r: Rect) {
        let x0 = r.x0.max(0);
        let y0 = r.y0.max(0);
        let x1 = r.x1.min(self.width);
        let y1 = r.y1.min(self.height);
        if x0 >= x1 || y0 >= y1 { return; }
        let w = (x1 - x0) as usize;
        for y in y0..y1 {
            let off = (y * self.stride + x0) as usize;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    self.bb.as_ptr().add(off),
                    self.base.add(off),
                    w,
                );
            }
        }
    }

    // Save the back-buffer pixels of a w*h rect into `buf` (for the software cursor's
    // save-under), and restore them. Out-of-screen cells are skipped consistently.
    fn save_under(&self, buf: &mut [u32], x: i32, y: i32, w: i32, h: i32) {
        for r in 0..h {
            let yy = y + r;
            if yy < 0 || yy >= self.height { continue; }
            let base = (yy * self.stride) as usize;
            for c in 0..w {
                let xx = x + c;
                if xx < 0 || xx >= self.width { continue; }
                buf[(r * w + c) as usize] = self.bb[base + xx as usize];
            }
        }
    }

    fn restore_under(&mut self, buf: &[u32], x: i32, y: i32, w: i32, h: i32) {
        for r in 0..h {
            let yy = y + r;
            if yy < 0 || yy >= self.height { continue; }
            let base = (yy * self.stride) as usize;
            for c in 0..w {
                let xx = x + c;
                if xx < 0 || xx >= self.width { continue; }
                self.bb[base + xx as usize] = buf[(r * w + c) as usize];
            }
        }
    }
}

// Copy a window's content canvas onto the screen back buffer at (dx, dy), restricted to
// `clip` (the dirty screen rect). This is a plain RAM->RAM row copy — far cheaper than
// re-running the glyph/fill drawing, which is the whole point of caching window content.
fn blit(dst: &mut Screen, src: &Screen, dx: i32, dy: i32, clip: Rect) {
    let x0 = dx.max(clip.x0).max(0);
    let y0 = dy.max(clip.y0).max(0);
    let x1 = (dx + src.width).min(clip.x1).min(dst.width);
    let y1 = (dy + src.height).min(clip.y1).min(dst.height);
    if x0 >= x1 || y0 >= y1 { return; }
    let n = (x1 - x0) as usize;
    for y in y0..y1 {
        let s_off = ((y - dy) * src.stride + (x0 - dx)) as usize;
        let d_off = (y * dst.stride + x0) as usize;
        dst.bb[d_off..d_off + n].copy_from_slice(&src.bb[s_off..s_off + n]);
    }
}

// Shift a terminal canvas's text region up by `rows_up` lines (a single block memmove),
// so a scrolling terminal reuses the already-rendered rows and only repaints the newly
// exposed bottom rows. The text region starts at y=4 to match Term::draw's origin.
fn scroll_term_canvas_up(canvas: &mut Screen, rows_up: usize) {
    let dy = rows_up as i32 * LINE_H;
    let top = 4;
    let bottom = (4 + TROWS as i32 * LINE_H).min(canvas.height);
    if dy <= 0 || top + dy >= bottom { return; }
    let stride = canvas.stride as usize;
    let src = ((top + dy) as usize) * stride..(bottom as usize) * stride;
    let dst = (top as usize) * stride;
    canvas.bb.copy_within(src, dst);
}

// Format a u64 as decimal into `out`, returning the written slice.
fn fmt_u64(v: u64, out: &mut [u8; 24]) -> usize {
    if v == 0 { out[0] = b'0'; return 1; }
    let mut tmp = [0u8; 24];
    let mut n = v;
    let mut i = 0;
    while n > 0 { tmp[i] = b'0' + (n % 10) as u8; n /= 10; i += 1; }
    for j in 0..i { out[j] = tmp[i - 1 - j]; }
    i
}

// window manager
// Windows are a dynamic list, not a fixed per-kind array, so several terminals and
// task managers can be open at once. The Vec order *is* the z-order (front = last);
// each window has a stable id so raising/closing/dragging survive index shuffles.
const TITLE_H: i32 = 28;
const CLOSE_SZ: i32 = 18;
const BORDER: i32 = 4; // window frame thickness (left/right/bottom); the title bar is the top

// Taskbar along the bottom edge: window buttons on the left, an uptime clock on the right.
// It's a frosted-glass strip: the content behind it is blurred and a translucent tint laid
// over it. When the buttons overflow, a vertical pager flips between pages.
const TASKBAR_H: i32 = 40;
const TB_BTN_W: i32 = 132;
const TB_BTN_GAP: i32 = 4;
const TB_PAD: i32 = 6;
const TB_CLOCK_W: i32 = 100; // fixed reserve on the right so button geometry is clock-independent
const BLUR_R: i32 = 8;       // taskbar backdrop blur radius (per box-blur pass)
const BLUR_PASSES: usize = 3; // box-blur passes; 3 approximates a Gaussian
const PAGER_W: i32 = 24;     // width of the up/down page switcher when buttons overflow

// Launcher buttons (relative to the window body).
const BTN_W: i32 = 150;
const BTN_H: i32 = 24;
const BTN_X: i32 = 12;

// A window's screen geometry (the content state lives in `Content`).
#[derive(Clone, Copy)]
struct Win {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

// What a content handler asks the compositor to do after consuming an input event.
// Content can't reach the window list itself, so window-level effects (open a window,
// quit the desktop) are returned for the compositor to apply.
enum InputAction {
    None,
    Open(u8), // open a window of this launcher kind (1=Task Manager, 2=Terminal, 3=Profiler)
    Quit,     // request the desktop to exit
}

enum Content {
    Launcher(Launcher),
    // TaskMon and Term are large; box them so a Window (and the `content` temporaries
    // built when opening one) stay small on the 16 KB-ish main stack.
    TaskMgr(alloc::boxed::Box<TaskMon>),
    Terminal(alloc::boxed::Box<Term>),
    Profiler(Profiler),
}

impl Content {
    fn title(&self) -> &'static [u8] {
        match self {
            Content::Launcher(_) => b"KazuOS",
            Content::TaskMgr(_) => b"Task Manager",
            Content::Terminal(_) => b"Terminal",
            Content::Profiler(_) => b"Profiler",
        }
    }
    fn has_close(&self) -> bool { !matches!(self, Content::Launcher(_)) }

    // Input is delegated to the focused/captured window's content. Coordinates are
    // body-local (origin = content canvas top-left); `bw` is the body width and
    // `buttons` is the raw mouse button byte. Each handler returns whether its content
    // changed (so the compositor re-renders the canvas) plus any window-level action.
    fn on_keydown(&mut self, b: u8) -> (bool, InputAction) {
        match self {
            Content::Terminal(t) => t.on_keydown(b),
            // Windows without text input don't consume keys; Ctrl+C quits the desktop.
            _ => if b == 0x03 { (false, InputAction::Quit) } else { (false, InputAction::None) },
        }
    }

    // Key release (the compositor polls SYS_KEYBOARD_POLL in event mode). No window needs
    // releases yet, so this is a no-op, but the path is live for windows that do.
    fn on_keyup(&mut self, _b: u8) -> (bool, InputAction) { (false, InputAction::None) }

    fn on_mouse_down(&mut self, x: i32, y: i32, _bw: i32, buttons: u8) -> (bool, InputAction) {
        match self {
            Content::Launcher(l) => l.on_mouse_down(x, y, buttons),
            _ => (false, InputAction::None),
        }
    }

    fn on_mouse_up(&mut self, x: i32, y: i32, bw: i32, _buttons: u8) -> (bool, InputAction) {
        match self {
            Content::Launcher(l) => l.on_mouse_up(x, y),
            Content::Profiler(p) => p.on_mouse_up(x, y, bw),
            Content::TaskMgr(t) => t.on_mouse_up(x, y),
            _ => (false, InputAction::None),
        }
    }

    fn on_mouse_move(&mut self, x: i32, y: i32, _bw: i32, buttons: u8) -> (bool, InputAction) {
        match self {
            Content::Launcher(l) => l.on_mouse_move(x, y, buttons),
            _ => (false, InputAction::None),
        }
    }
}

// The launcher's content: just the pressed-button highlight state. Buttons fire on
// release; the highlight follows the cursor while the mouse is held down.
struct Launcher {
    pressed: u8, // 0 = none, else 1/2/3 while that button is held
}

impl Launcher {
    fn new() -> Launcher { Launcher { pressed: 0 } }

    // Body-local hit test for the stacked buttons; mirrors draw_launcher_body's layout.
    fn button_at(x: i32, y: i32) -> u8 {
        for i in 0..3 {
            if in_rect(x, y, BTN_X - 1, launcher_btn_y(i), BTN_W, BTN_H) { return (i + 1) as u8; }
        }
        0
    }

    fn on_mouse_down(&mut self, x: i32, y: i32, buttons: u8) -> (bool, InputAction) {
        let p = if buttons & 1 != 0 { Launcher::button_at(x, y) } else { 0 };
        let dirty = p != self.pressed;
        self.pressed = p;
        (dirty, InputAction::None)
    }

    fn on_mouse_move(&mut self, x: i32, y: i32, buttons: u8) -> (bool, InputAction) {
        // Keep the highlight under the cursor while held; clear it if dragged off.
        let p = if buttons & 1 != 0 { Launcher::button_at(x, y) } else { 0 };
        let dirty = p != self.pressed;
        self.pressed = p;
        (dirty, InputAction::None)
    }

    fn on_mouse_up(&mut self, x: i32, y: i32) -> (bool, InputAction) {
        let b = Launcher::button_at(x, y);
        self.pressed = 0;
        let action = if b != 0 { InputAction::Open(b) } else { InputAction::None };
        (true, action) // clear the highlight
    }
}

struct Window {
    id: u64,
    geo: Win,
    content: Content,
    // The window's body pixels, drawn only when content changes; the compositor blits
    // this onto the screen each frame. `dirty` (canvas-local, empty = clean) is the
    // region of the canvas that needs re-rendering before the next composite.
    canvas: Screen,
    dirty: Rect,
}

impl Window {
    fn dirty_full(&mut self) {
        self.dirty.add(0, 0, self.canvas.width, self.canvas.height);
    }
    fn dirty_rect(&mut self, x: i32, y: i32, w: i32, h: i32) {
        self.dirty.add(x, y, w, h);
    }
}

fn make_canvas(geo: &Win, format: u32) -> Option<Screen> {
    Screen::offscreen(body_w(geo), body_h(geo), format)
}

fn close_rect(win: &Win) -> (i32, i32, i32, i32) {
    (win.x + win.w - CLOSE_SZ - BORDER, win.y + (TITLE_H - CLOSE_SZ) / 2, CLOSE_SZ, CLOSE_SZ)
}

fn in_rect(px: i32, py: i32, x: i32, y: i32, w: i32, h: i32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

// Body-local y of launcher button `i` (0-based), stacked top-down. The hit test lives in
// Launcher::button_at (body-local), matching draw_launcher_body's layout.
fn launcher_btn_y(i: i32) -> i32 { 12 + i * (BTN_H + 8) }

fn draw_button(s: &mut Screen, x: i32, y: i32, label: &[u8], pressed: bool) {
    let border = s.pack(0x00, 0x00, 0x00);
    let face = if pressed { s.pack(0x90, 0x90, 0x90) } else { s.pack(0xC8, 0xC8, 0xC8) };
    s.fill(x, y, BTN_W, BTN_H, border);
    s.fill(x + 1, y + 1, BTN_W - 2, BTN_H - 2, face);
    s.text(x + 8, y + (BTN_H - GLYPH_H) / 2, label, s.pack(0x00, 0x00, 0x00));
}

// Draw the launcher's body (buttons) into its content canvas at body-local coords. The
// button positions mirror launcher_button()'s hit test (screen body origin = x+1,y+TITLE_H).
fn draw_launcher_body(c: &mut Screen, pressed_btn: u8) {
    let bg = c.pack(0xE8, 0xE8, 0xE8);
    let (w, h) = (c.width, c.height);
    c.fill(0, 0, w, h, bg);
    draw_button(c, BTN_X - 1, launcher_btn_y(0), b"Task Manager", pressed_btn == 1);
    draw_button(c, BTN_X - 1, launcher_btn_y(1), b"Terminal", pressed_btn == 2);
    draw_button(c, BTN_X - 1, launcher_btn_y(2), b"Profiler", pressed_btn == 3);
}

// Window body geometry (the area the content canvas covers, inside the frame and below the
// title bar). The content canvas is this size and is blitted at (x+BORDER, y+TITLE_H).
fn body_w(geo: &Win) -> i32 { (geo.w - 2 * BORDER).max(1) }
fn body_h(geo: &Win) -> i32 { (geo.h - TITLE_H - BORDER).max(1) }

// Draw the (opaque) window chrome: a solid frame border and title bar. Drawing it is
// backdrop-independent, so the compositor can safely redraw it on any partial repaint that
// touches it — which is what keeps a window's border from being erased by a neighbour's update.
fn draw_frame(s: &mut Screen, win: &Win, title: &[u8], focused: bool, has_close: bool) {
    let fmt = s.format;
    let x = win.x; let y = win.y; let w = win.w; let h = win.h;
    let border = if focused { pack_fmt(fmt, 0x3A, 0x6A, 0xB8) } else { pack_fmt(fmt, 0x44, 0x4E, 0x5E) };
    let titlebar = if focused { pack_fmt(fmt, 0x28, 0x60, 0xC0) } else { pack_fmt(fmt, 0x4A, 0x55, 0x66) };
    // Whole frame, then the title bar; the body is overdrawn by the content blit next.
    s.fill(x, y, w, h, border);
    s.fill(x, y, w, TITLE_H, titlebar);
    if focused { s.fill(x, y + TITLE_H - 2, w, 2, pack_fmt(fmt, 0x80, 0xB4, 0xFF)); }
    let txt = if focused { pack_fmt(fmt, 0xFF, 0xFF, 0xFF) } else { pack_fmt(fmt, 0xCE, 0xD6, 0xE0) };
    s.text(x + BORDER + 4, y + (TITLE_H - GLYPH_H) / 2, title, txt);
    if has_close {
        let (cx, cy, cw, ch) = close_rect(win);
        s.fill(cx, cy, cw, ch, pack_fmt(fmt, 0xC0, 0x40, 0x40));
        s.text(cx + (cw - ADVANCE) / 2, cy + (ch - GLYPH_H) / 2, b"x", pack_fmt(fmt, 0xFF, 0xFF, 0xFF));
    }
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

fn sys_write(buf: &[u8]) {
    syscall(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
}

// A scrolling column graph: history is a ring buffer of permille values (0..=1000),
// newest written at `head`. Each graph pixel column maps to a sample (oldest on the left).
fn draw_graph(s: &mut Screen, x: i32, y: i32, w: i32, h: i32,
              hist: &[u16], head: usize, count: usize, cap: usize, color: u32) {
    if w <= 0 || h <= 0 { return; }
    s.fill(x, y, w, h, s.pack(0x10, 0x12, 0x18));
    let grid = s.pack(0x30, 0x36, 0x40);
    for q in 1..4 {
        let yy = y + h - q * h / 4;
        s.fill(x, yy, w, 1, grid);
    }
    // The history ring is kept permanently full (zero-padded before real data arrives),
    // so the trace always spans the whole graph width and simply scrolls left as new
    // samples push in on the right — no rescaling as it fills up.
    if count > 0 {
        for px in 0..w {
            let i = px as usize * count / w as usize;
            let idx = (head + cap - count + i) % cap;
            let bh = hist[idx] as i32 * h / 1000;
            if bh > 0 { s.fill(x + px, y + h - bh, 1, bh, color); }
        }
    }
    let bd = s.pack(0x50, 0x58, 0x64);
    s.fill(x, y, w, 1, bd);
    s.fill(x, y + h - 1, w, 1, bd);
    s.fill(x, y, 1, h, bd);
    s.fill(x + w - 1, y, 1, h, bd);
}

// Render a permille value as "NN.N%" and return the x just past it.
fn text_permille(s: &mut Screen, x: i32, y: i32, permille: u16, color: u32) -> i32 {
    let mut buf = [0u8; 24];
    let ni = fmt_u64((permille / 10) as u64, &mut buf);
    s.text(x, y, &buf[..ni], color);
    let mut xx = x + ni as i32 * ADVANCE;
    s.text(xx, y, b".", color); xx += ADVANCE;
    let mut f = [0u8; 24];
    let nf = fmt_u64((permille % 10) as u64, &mut f);
    s.text(xx, y, &f[..nf], color); xx += nf as i32 * ADVANCE;
    s.text(xx, y, b"%", color); xx += ADVANCE;
    xx
}

// Task Manager tab strip.
const TAB_H: i32 = 22;
const TAB_W: i32 = 110;
const PERF_SAMPLES: usize = 30;

// Tab under a point in body-local coords: Some(0)=Processes, Some(1)=Performance.
fn taskmgr_tab(px: i32, py: i32) -> Option<u8> {
    if py < 0 || py >= TAB_H { return None; }
    if px >= 0 && px < TAB_W { Some(0) }
    else if px >= TAB_W && px < 2 * TAB_W { Some(1) }
    else { None }
}

fn draw_tabs(s: &mut Screen, active: u8) {
    let w = s.width;
    s.fill(0, 0, w, TAB_H, s.pack(0xC0, 0xC6, 0xCE));
    let labels: [&[u8]; 2] = [b"Processes", b"Performance"];
    for (i, l) in labels.iter().enumerate() {
        let x = i as i32 * TAB_W;
        let on = active == i as u8;
        let face = if on { s.pack(0xE8, 0xE8, 0xE8) } else { s.pack(0xB0, 0xB6, 0xBE) };
        s.fill(x, 0, TAB_W, TAB_H, face);
        s.fill(x + TAB_W - 1, 0, 1, TAB_H, s.pack(0x80, 0x86, 0x90));
        if on { s.fill(x, 0, TAB_W, 2, s.pack(0x20, 0x60, 0xC0)); }
        s.text(x + 10, (TAB_H - GLYPH_H) / 2, l, s.pack(0x00, 0x00, 0x00));
    }
}

// Task Manager content
// Task Manager state. %CPU is computed from the *delta* in each process's cpu_ticks
// between refreshes (≈0.5 s) over the delta in total ticks — an instantaneous load,
// like a real task manager — instead of the lifetime average (which reads absurdly
// high right after boot when the tick denominator is tiny).
const MAXT: usize = 48;

#[derive(Clone, Copy)]
struct TaskRow {
    pid: u64,
    ppid: u64,
    permille: u64,
    mem_kb: u64,
    name: [u8; NAME_LEN],
    name_len: usize,
}

struct TaskMon {
    rows: [TaskRow; MAXT],
    n: usize,
    prev_pid: [u64; MAXT],
    prev_ticks: [u64; MAXT],
    nprev: usize,
    prev_total: u64,
    primed: bool, // false until the first baseline sample is taken
    // Performance tab: which tab is shown, plus scrolling CPU/memory history (permille).
    tab: u8,
    prev_busy: u64,
    cpu_hist: [u16; PERF_SAMPLES],
    mem_hist: [u16; PERF_SAMPLES],
    phead: usize,
    pcount: usize,
    cpu_cur: u16,
    mem_cur: u16,
    mem_used_kib: u64,
    mem_total_kib: u64,
    // Per-core CPU load (permille), same cadence/ring index as cpu_hist. Sized to the
    // actual core count on the first refresh (one ring buffer of PERF_SAMPLES per core).
    core_hist: alloc::vec::Vec<alloc::vec::Vec<u16>>,
    core_cur: alloc::vec::Vec<u16>,
    core_prev_busy: alloc::vec::Vec<u64>,
    core_prev_total: alloc::vec::Vec<u64>,
}

impl TaskMon {
    fn new() -> TaskMon {
        let blank = TaskRow { pid: 0, ppid: 0, permille: 0, mem_kb: 0, name: [0u8; NAME_LEN], name_len: 0 };
        TaskMon {
            rows: [blank; MAXT],
            n: 0,
            prev_pid: [0u64; MAXT],
            prev_ticks: [0u64; MAXT],
            nprev: 0,
            prev_total: 0,
            primed: false,
            tab: 0,
            prev_busy: 0,
            cpu_hist: [0u16; PERF_SAMPLES],
            mem_hist: [0u16; PERF_SAMPLES],
            phead: 0,
            pcount: PERF_SAMPLES, // ring kept full (zero-padded) so the graph spans full width

            cpu_cur: 0,
            mem_cur: 0,
            mem_used_kib: 0,
            mem_total_kib: 0,
            core_hist: alloc::vec::Vec::new(),
            core_cur: alloc::vec::Vec::new(),
            core_prev_busy: alloc::vec::Vec::new(),
            core_prev_total: alloc::vec::Vec::new(),
        }
    }

    fn prev_lookup(&self, pid: u64) -> u64 {
        for i in 0..self.nprev {
            if self.prev_pid[i] == pid { return self.prev_ticks[i]; }
        }
        0
    }

    // Re-baseline: the next refresh records ticks without reporting a (bogus) %CPU.
    fn baseline(&mut self) {
        self.primed = false;
        self.refresh();
    }

    // Click in the tab strip switches between the Processes and Performance views.
    fn on_mouse_up(&mut self, x: i32, y: i32) -> (bool, InputAction) {
        if let Some(t) = taskmgr_tab(x, y) {
            if self.tab != t { self.tab = t; return (true, InputAction::None); }
        }
        (false, InputAction::None)
    }

    fn refresh(&mut self) {
        // Denominator must be the *whole* elapsed CPU time so a row's share is real:
        // user (1) + kernel (2) + idle (3). SYS_CPU_INFO(1) alone is user-only, which
        // makes the kernel row read ~95% when the system is otherwise idle.
        let user = syscall(SYS_CPU_INFO, 1, 0, 0);
        let kernel = syscall(SYS_CPU_INFO, 2, 0, 0);
        let idle = syscall(SYS_CPU_INFO, 3, 0, 0);
        let total = user + kernel + idle;
        let busy = user + kernel;
        let dt = total.saturating_sub(self.prev_total).max(1);
        // 100% means one fully-used core, so the scale tops out at ncpu*100%.
        let ncpu = syscall(SYS_CPU_INFO, 4, 0, 0).max(1);
        let scale = 1000 * ncpu;
        let primed = self.primed;
        let mut n = 0usize;
        let mut snap_pid = [0u64; MAXT];
        let mut snap_ticks = [0u64; MAXT];
        let mut ns = 0usize;

        // Kernel (pid 0).
        let mut info = empty_info();
        if syscall(SYS_PROCESS_INFO, 0, &mut info as *mut _ as u64, 0) == 0 && n < MAXT {
            let ticks = info.cpu_ticks;
            let permille = if primed { ticks.saturating_sub(self.prev_lookup(0)).saturating_mul(scale) / dt } else { 0 };
            let mut name = [0u8; NAME_LEN];
            let kn = b"kernel";
            name[..kn.len()].copy_from_slice(kn);
            self.rows[n] = TaskRow { pid: 0, ppid: 0, permille, mem_kb: info.memory_bytes / 1024, name, name_len: kn.len() };
            n += 1;
            snap_pid[ns] = 0; snap_ticks[ns] = ticks; ns += 1;
        }

        // User processes.
        let mut pid = syscall(SYS_PROCESS_NEXT, 0, 0, 0);
        while pid != u64::MAX && n < MAXT {
            let mut info = empty_info();
            if syscall(SYS_PROCESS_INFO, pid, &mut info as *mut _ as u64, 0) == 0 {
                let ticks = info.cpu_ticks;
                let permille = if primed { ticks.saturating_sub(self.prev_lookup(pid)).saturating_mul(scale) / dt } else { 0 };
                let nlen = info.image_name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
                self.rows[n] = TaskRow { pid, ppid: info.parent, permille, mem_kb: info.memory_bytes / 1024, name: info.image_name, name_len: nlen };
                n += 1;
                if ns < MAXT { snap_pid[ns] = pid; snap_ticks[ns] = ticks; ns += 1; }
            }
            pid = syscall(SYS_PROCESS_NEXT, pid, 0, 0);
        }

        // Performance-tab sampling: whole-system CPU load and memory usage (permille).
        let cpu_permille = if primed {
            (busy.saturating_sub(self.prev_busy).saturating_mul(1000) / dt).min(1000) as u16
        } else { 0 };
        let mem = syscall(SYS_MEM_INFO, 0, 0, 0);
        let mem_total = mem >> 32;
        let mem_used = mem & 0xffff_ffff;
        let mem_permille = if mem_total > 0 {
            (mem_used.saturating_mul(1000) / mem_total).min(1000) as u16
        } else { 0 };
        self.cpu_cur = cpu_permille;
        self.mem_cur = mem_permille;
        self.mem_used_kib = mem_used;
        self.mem_total_kib = mem_total;

        // Per-core load (busy = user+kernel over the core's whole elapsed time). The
        // per-core buffers are allocated lazily here, sized to the real core count.
        let ncores = ncpu as usize;
        if self.core_cur.len() != ncores {
            self.core_hist = alloc::vec![alloc::vec![0u16; PERF_SAMPLES]; ncores];
            self.core_cur = alloc::vec![0u16; ncores];
            self.core_prev_busy = alloc::vec![0u64; ncores];
            self.core_prev_total = alloc::vec![0u64; ncores];
        }
        for c in 0..ncores {
            let cu = syscall(SYS_CPU_INFO, 10, c as u64, 0);
            let ck = syscall(SYS_CPU_INFO, 9, c as u64, 0);
            let ci = syscall(SYS_CPU_INFO, 8, c as u64, 0);
            let cbusy = cu + ck;
            let ctotal = cbusy + ci;
            let cdt = ctotal.saturating_sub(self.core_prev_total[c]).max(1);
            self.core_cur[c] = if primed {
                (cbusy.saturating_sub(self.core_prev_busy[c]).saturating_mul(1000) / cdt).min(1000) as u16
            } else { 0 };
            self.core_prev_busy[c] = cbusy;
            self.core_prev_total[c] = ctotal;
        }

        if primed {
            self.cpu_hist[self.phead] = cpu_permille;
            self.mem_hist[self.phead] = mem_permille;
            for c in 0..ncores { self.core_hist[c][self.phead] = self.core_cur[c]; }
            self.phead = (self.phead + 1) % PERF_SAMPLES;
            if self.pcount < PERF_SAMPLES { self.pcount += 1; }
        }

        self.n = n;
        self.prev_pid = snap_pid;
        self.prev_ticks = snap_ticks;
        self.nprev = ns;
        self.prev_total = total;
        self.prev_busy = busy;
        self.primed = true;
    }

    // Draw into the window's content canvas at body-local coords (origin = body top-left).
    // A tab strip at the top switches between the process list and the performance graphs.
    fn draw(&self, s: &mut Screen) {
        let body = s.pack(0xE8, 0xE8, 0xE8);
        s.fill(0, 0, s.width, s.height, body);
        draw_tabs(s, self.tab);
        if self.tab == 0 { self.draw_processes(s); } else { self.draw_performance(s); }
    }

    fn draw_processes(&self, s: &mut Screen) {
        let black = s.pack(0x00, 0x00, 0x00);
        let head_bg = s.pack(0xD0, 0xD8, 0xE0);
        let row_alt = s.pack(0xF4, 0xF4, 0xF4);
        let w = s.width;
        let bx = 8;
        let mut y = TAB_H + 6;

        s.fill(0, y - 2, w, LINE_H, head_bg);
        s.text(bx,       y, b"PID",  black);
        s.text(bx + 48,  y, b"PPID", black);
        s.text(bx + 104, y, b"%CPU", black);
        s.text(bx + 168, y, b"MEM(KB)", black);
        s.text(bx + 256, y, b"NAME", black);
        y += LINE_H + 2;

        let bottom = s.height - LINE_H;
        let mut alt = false;
        for i in 0..self.n {
            if y >= bottom { break; }
            let r = &self.rows[i];
            if alt { s.fill(0, y - 2, w, LINE_H, row_alt); }
            let mut buf = [0u8; 24];
            let nb = fmt_u64(r.pid, &mut buf); s.text(bx, y, &buf[..nb], black);
            let mut pp = [0u8; 24];
            let np = fmt_u64(r.ppid, &mut pp); s.text(bx + 48, y, &pp[..np], black);
            // %CPU permille -> "NN.N"
            let mut c = [0u8; 24];
            let ni = fmt_u64(r.permille / 10, &mut c);
            s.text(bx + 104, y, &c[..ni], black);
            s.text(bx + 104 + ni as i32 * ADVANCE, y, b".", black);
            let mut f = [0u8; 24];
            let nf = fmt_u64(r.permille % 10, &mut f);
            s.text(bx + 104 + (ni as i32 + 1) * ADVANCE, y, &f[..nf], black);
            let mut m = [0u8; 24];
            let nm = fmt_u64(r.mem_kb, &mut m); s.text(bx + 168, y, &m[..nm], black);
            s.text(bx + 256, y, &r.name[..r.name_len], black);
            y += LINE_H; alt = !alt;
        }
    }

    fn draw_performance(&self, s: &mut Screen) {
        let black = s.pack(0x10, 0x10, 0x10);
        let cpu_col = s.pack(0x40, 0xC0, 0x80);
        let mem_col = s.pack(0x50, 0x90, 0xE0);
        let w = s.width;
        let gx = 8;
        let gw = (w - 16).max(1);
        let mut y = TAB_H + 8;
        let bottom = s.height - 8;

        // Memory block is pinned to the bottom; everything else stacks from the top.
        let mem_h = 56;
        let mem_label_y = bottom - mem_h - LINE_H;

        // Total CPU (full width).
        s.text(gx, y, b"CPU Total", black);
        text_permille(s, gx + 10 * ADVANCE, y, self.cpu_cur, black);
        y += LINE_H;
        let total_h = 64;
        draw_graph(s, gx, y, gw, total_h, &self.cpu_hist, self.phead, self.pcount,
                   PERF_SAMPLES, cpu_col);
        y += total_h + 10;

        // Per-core grid in the space between the total graph and the memory block.
        let n = self.core_cur.len();
        let grid_bottom = mem_label_y - 8;
        if n > 0 && grid_bottom > y + LINE_H + 12 {
            let cols = if n <= 1 { 1 } else if n <= 4 { 2 } else { 4 };
            let rows = (n + cols - 1) / cols;
            let gap = 8;
            let cell_w = (gw - (cols as i32 - 1) * gap) / cols as i32;
            let cell_h = (grid_bottom - y - (rows as i32 - 1) * gap) / rows as i32;
            let chart_h = cell_h - LINE_H;
            if cell_w > 8 && chart_h >= 8 {
                for c in 0..n {
                    let col = (c % cols) as i32;
                    let row = (c / cols) as i32;
                    let cx = gx + col * (cell_w + gap);
                    let cy = y + row * (cell_h + gap);
                    let mut lbl = [0u8; 8];
                    lbl[0] = b'C'; lbl[1] = b'P'; lbl[2] = b'U';
                    let mut nb = [0u8; 24];
                    let ln = fmt_u64(c as u64, &mut nb);
                    lbl[3..3 + ln].copy_from_slice(&nb[..ln]);
                    s.text(cx, cy, &lbl[..3 + ln], black);
                    text_permille(s, cx + (5 + ln as i32) * ADVANCE, cy, self.core_cur[c], black);
                    draw_graph(s, cx, cy + LINE_H, cell_w, chart_h, &self.core_hist[c],
                               self.phead, self.pcount, PERF_SAMPLES, cpu_col);
                }
            }
        }

        // Memory (full width, bottom).
        let mut y = mem_label_y;
        s.text(gx, y, b"Memory", black);
        let mut xx = text_permille(s, gx + 8 * ADVANCE, y, self.mem_cur, black);
        xx += ADVANCE;
        let mut buf = [0u8; 24];
        let used_mb = self.mem_used_kib / 1024;
        let total_mb = self.mem_total_kib / 1024;
        let nu = fmt_u64(used_mb, &mut buf); s.text(xx, y, &buf[..nu], black);
        xx += nu as i32 * ADVANCE;
        s.text(xx, y, b"/", black); xx += ADVANCE;
        let nt = fmt_u64(total_mb, &mut buf); s.text(xx, y, &buf[..nt], black);
        xx += nt as i32 * ADVANCE;
        s.text(xx, y, b" MB", black);
        y += LINE_H;
        draw_graph(s, gx, y, gw, mem_h, &self.mem_hist, self.phead, self.pcount,
                   PERF_SAMPLES, mem_col);
    }
}

fn empty_info() -> ProcessInfo {
    ProcessInfo {
        pid: 0, state: 0, image_name: [0u8; NAME_LEN],
        start_tsc: 0, entry: 0, stack_top: 0, step: 0, cpu_ticks: 0, memory_bytes: 0, parent: 0,
    }
}

// terminal: runs the real shell over pipes and emulates a dumb text terminal
const TCOLS: usize = 64;
const TROWS: usize = 18;
const TPAD: i32 = 6;

struct Term {
    grid: [[u8; TCOLS]; TROWS],
    cx: usize,
    cy: usize,
    esc: u8,      // ANSI parse state: 0=normal, 1=ESC seen, 2=in CSI
    eparam: u32,  // first numeric CSI parameter
    spawned: bool,
    alive: bool,
    in_write: u64,  // GUI -> shell stdin
    out_read: u64,  // shell stdout -> GUI
    shell_pid: u64, // the shell process, so we can kill it on close
    // Rows changed since the last draw, so only those are repainted/copied instead of
    // the whole grid every frame (dy0 >= dy1 means nothing changed). last_cur_y is the
    // row the cursor block was last drawn at, so we can erase it when the cursor moves.
    dy0: usize,
    dy1: usize,
    last_cur_y: usize,
    // Lines scrolled this frame (capped at TROWS). The compositor shifts the rendered
    // canvas pixels up by this many rows instead of repainting every row.
    scrolled: usize,
}

impl Term {
    fn new() -> Term {
        Term {
            grid: [[b' '; TCOLS]; TROWS],
            cx: 0, cy: 0,
            esc: 0, eparam: 0,
            spawned: false, alive: false,
            in_write: 0, out_read: 0, shell_pid: 0,
            dy0: TROWS, dy1: 0, last_cur_y: 0,
            scrolled: 0,
        }
    }

    fn mark(&mut self, r: usize) {
        if r < TROWS {
            self.dy0 = self.dy0.min(r);
            self.dy1 = self.dy1.max(r + 1);
        }
    }
    fn mark_all(&mut self) { self.dy0 = 0; self.dy1 = TROWS; }

    // Take the changed row range (unioned with the cursor's old/new rows so the block is
    // erased and redrawn), clearing it. None = nothing to repaint this frame.
    fn take_dirty(&mut self) -> Option<(usize, usize)> {
        let cur_lo = self.cy.min(self.last_cur_y);
        let cur_hi = self.cy.max(self.last_cur_y) + 1;
        let (mut y0, mut y1) = (self.dy0, self.dy1);
        if y0 >= y1 {
            if self.cy == self.last_cur_y { return None; }
            y0 = cur_lo; y1 = cur_hi;
        } else {
            y0 = y0.min(cur_lo); y1 = y1.max(cur_hi);
        }
        self.last_cur_y = self.cy;
        self.dy0 = TROWS; self.dy1 = 0;
        Some((y0.min(TROWS), y1.min(TROWS)))
    }

    // Take this frame's scroll count plus the rows to repaint. When scrolled, the
    // compositor shifts the canvas up by `scrolled` rows, so we only need to repaint the
    // newly exposed bottom rows — and the row the old cursor block landed on after the
    // shift, so it doesn't leave a ghost.
    fn take_frame(&mut self) -> (usize, Option<(usize, usize)>) {
        let scrolled = self.scrolled;
        self.scrolled = 0;
        let old_cur = self.last_cur_y;
        let dirty = self.take_dirty();
        if scrolled == 0 {
            return (0, dirty);
        }
        let lo = TROWS.saturating_sub(scrolled);
        let ghost = old_cur.saturating_sub(scrolled);
        let r0 = lo.min(ghost);
        let merged = match dirty {
            Some((a, b)) => (a.min(r0), b.max(TROWS)),
            None => (r0, TROWS),
        };
        (scrolled, Some(merged))
    }

    fn spawn(&mut self) {
        if self.spawned { return; }
        self.grid = [[b' '; TCOLS]; TROWS];
        self.cx = 0; self.cy = 0;
        self.esc = 0; self.eparam = 0;
        self.dy0 = TROWS; self.dy1 = 0; self.last_cur_y = 0;
        self.scrolled = 0;

        let mut pin = [0u64; 2];   // [read, write] : GUI writes pin[1] -> shell stdin pin[0]
        let mut pout = [0u64; 2];  // shell stdout pout[1] -> GUI reads pout[0]
        if syscall(SYS_PIPE, pin.as_mut_ptr() as u64, 0, 0) != 0 { return; }
        if syscall(SYS_PIPE, pout.as_mut_ptr() as u64, 0, 0) != 0 { return; }
        let (in_read, in_write) = (pin[0], pin[1]);
        let (out_read, out_write) = (pout[0], pout[1]);

        let stdio = (in_read & 0xFFFF) | ((out_write & 0xFFFF) << 16);
        let cmd = b"/bin/shell.kxe\0";
        let pid = syscall(SYS_EXEC, cmd.as_ptr() as u64, cmd.len() as u64, stdio);

        // Drop the shell's ends so EOF propagates: when the shell exits its stdout
        // write end closes and our out_read sees EOF; closing in_write later makes
        // the shell's stdin read hit EOF and exit.
        syscall(SYS_CLOSE, in_read, 0, 0);
        syscall(SYS_CLOSE, out_write, 0, 0);

        self.in_write = in_write;
        self.out_read = out_read;
        self.shell_pid = pid;
        self.spawned = true;
        self.alive = pid != 0 && pid != u64::MAX;
        if self.alive {
            // Tell the shell its terminal size (SYS_CONSOLE_SIZE set form) so it and
            // its children see our grid via SYS_CONSOLE_SIZE.
            syscall(SYS_CONSOLE_SIZE, (TCOLS as u64) | ((TROWS as u64) << 16), pid, 0);
        }
    }

    fn shutdown(&mut self) {
        if !self.spawned { return; }
        // Kill the shell outright: closing the pipes alone won't stop it if it's
        // blocked waiting on a child (e.g. cpuburner), and killing it cascades to its
        // children via the kernel's parent/child teardown.
        if self.shell_pid != 0 && self.shell_pid != u64::MAX {
            syscall(SYS_KILL, self.shell_pid, 0, 0);
        }
        syscall(SYS_CLOSE, self.in_write, 0, 0);
        syscall(SYS_CLOSE, self.out_read, 0, 0);
        self.spawned = false;
        self.alive = false;
    }

    fn forward_key(&self, b: u8) {
        let c = [b];
        syscall(SYS_WRITE, self.in_write, c.as_ptr() as u64, 1);
    }

    // Ctrl+C: if the shell is running a command, SIGINT that foreground child; if it's
    // idle at its prompt, send 0x03 down stdin so the line editor cancels the line.
    fn interrupt(&self) {
        if self.shell_pid == 0 || self.shell_pid == u64::MAX { return; }
        if syscall(SYS_SIGINT_FG, self.shell_pid, 0, 0) == 0 {
            self.forward_key(0x03);
        }
    }

    // A key press for the focused terminal. Ctrl+C interrupts the running command (or
    // cancels the prompt line); if the shell has already exited, Ctrl+C falls through to
    // quit the desktop. Editable/printable keys are forwarded to the shell's stdin. The
    // grid is repainted from pump()'s shell output, so no redraw is requested here.
    fn on_keydown(&mut self, b: u8) -> (bool, InputAction) {
        if b == 0x03 {
            if self.alive { self.interrupt(); return (false, InputAction::None); }
            return (false, InputAction::Quit);
        }
        let editable = (0x20..=0x7e).contains(&b)
            || b == 0x0d || b == 0x0a || b == 0x08 || b == 0x7f
            || (0x80..=0x83).contains(&b); // arrow keys (left/right/up/down)
        if editable && self.alive { self.forward_key(b); }
        (false, InputAction::None)
    }

    // Drain shell output into the grid. Returns true if anything changed; sets
    // alive=false on EOF (shell exited).
    fn pump(&mut self) -> bool {
        if !self.alive { return false; }
        let mut changed = false;
        // Drain in big chunks: each SYS_TRY_READ takes the kernel's global thread lock
        // (shared with the scheduler), so reading a ~2 KB frame 128 bytes at a time meant
        // ~16 lock acquisitions per frame and visible micro-stalls. One grid is 64*18 plus
        // ANSI, so 2 KB swallows a whole frame in one or two reads.
        let mut b = [0u8; 2048];
        loop {
            let n = syscall(SYS_TRY_READ, self.out_read, b.as_mut_ptr() as u64, b.len() as u64);
            if n == 0 { break; }
            if n == u64::MAX { self.alive = false; self.mark_all(); changed = true; break; }
            for &ch in &b[..n as usize] { self.put_char(ch); }
            changed = true;
            if (n as usize) < b.len() { break; }
        }
        changed
    }

    fn newline(&mut self) {
        self.cx = 0;
        if self.cy + 1 >= TROWS {
            for r in 1..TROWS { self.grid[r - 1] = self.grid[r]; }
            self.grid[TROWS - 1] = [b' '; TCOLS];
            // The grid scrolled up one row. Instead of repainting every row, record the
            // scroll so the compositor shifts the rendered pixels up and repaints only the
            // exposed bottom row. Move pending dirty marks up with the content so they keep
            // pointing at the right rows.
            if self.dy1 > self.dy0 {
                self.dy0 = self.dy0.saturating_sub(1);
                self.dy1 = self.dy1.saturating_sub(1);
            }
            self.scrolled = (self.scrolled + 1).min(TROWS);
            self.mark(TROWS - 1);
        } else {
            self.cy += 1;
        }
    }

    fn put_char(&mut self, ch: u8) {
        match self.esc {
            1 => { self.esc = if ch == b'[' { self.eparam = 0; 2 } else { 0 }; return; }
            2 => { self.csi(ch); return; }
            _ => {}
        }
        match ch {
            0x1b => self.esc = 1,
            b'\n' => self.newline(),
            b'\r' => { self.cx = 0; self.mark(self.cy); }
            0x08 => { if self.cx > 0 { self.cx -= 1; } self.mark(self.cy); }
            0x09 => { self.cx = (((self.cx / 4) + 1) * 4).min(TCOLS - 1); }
            c if (0x20..=0x7e).contains(&c) => {
                if self.cx >= TCOLS { self.newline(); }
                self.grid[self.cy][self.cx] = c;
                self.cx += 1;
                self.mark(self.cy);
            }
            _ => {}
        }
    }

    // Minimal CSI handler (the bytes after ESC [). Enough for the shell's output:
    // 2J clear screen, H cursor home, K erase to end of line.
    fn csi(&mut self, ch: u8) {
        if ch.is_ascii_digit() {
            self.eparam = self.eparam.saturating_mul(10) + (ch - b'0') as u32;
            return;
        }
        if ch == b';' { return; } // more params follow; we only track the first
        let n = if self.eparam == 0 { 1 } else { self.eparam as usize };
        match ch {
            b'J' => { if self.eparam == 2 { self.grid = [[b' '; TCOLS]; TROWS]; self.scrolled = 0; self.mark_all(); } }
            b'H' => { self.mark(self.cy); self.cx = 0; self.cy = 0; self.mark(0); }
            b'K' => { for c in self.cx..TCOLS { self.grid[self.cy][c] = b' '; } self.mark(self.cy); }
            b'D' => { self.cx = self.cx.saturating_sub(n); self.mark(self.cy); } // cursor back
            b'C' => { self.cx = (self.cx + n).min(TCOLS - 1); self.mark(self.cy); } // cursor forward
            _ => {}
        }
        self.esc = 0;
    }

    // Draw into the window's content canvas at body-local coords (origin = body top-left).
    // Honours the canvas clip, so re-rendering just the dirty rows stays cheap.
    fn draw(&self, s: &mut Screen) {
        let bg = s.pack(0x0a, 0x0a, 0x14);
        let fg = s.pack(0xD0, 0xD0, 0xD0);
        let cur = s.pack(0x40, 0xC0, 0x40);
        let (w, h) = (s.width, s.height);
        s.fill(0, 0, w, h, bg);
        let x0 = TPAD - 1;
        let y0 = 4;
        // Only walk the cells overlapping the clip rect (cheap on partial redraws).
        let c_start = (((s.clip.x0 - x0) / ADVANCE).max(0) as usize).min(TCOLS);
        let c_end = ((((s.clip.x1 - x0) + ADVANCE - 1) / ADVANCE).max(0) as usize).min(TCOLS);
        let r_start = (((s.clip.y0 - y0) / LINE_H).max(0) as usize).min(TROWS);
        let r_end = ((((s.clip.y1 - y0) + LINE_H - 1) / LINE_H).max(0) as usize).min(TROWS);
        for r in r_start..r_end {
            for c in c_start..c_end {
                let ch = self.grid[r][c];
                if ch != b' ' {
                    s.glyph(x0 + c as i32 * ADVANCE, y0 + r as i32 * LINE_H, ch, fg);
                }
            }
        }
        if self.alive {
            s.fill(x0 + self.cx as i32 * ADVANCE, y0 + self.cy as i32 * LINE_H, GLYPH_W, GLYPH_H, cur);
        } else if self.spawned {
            s.text(x0, y0 + (TROWS as i32) * LINE_H - LINE_H, b"[shell exited]", s.pack(0x90, 0x90, 0x90));
        }
    }
}

// profiler: a scrolling CPU-usage graph with an on/off toggle button
// Samples whole-system CPU load (busy = user+kernel, over busy+idle) once per refresh
// tick into a ring buffer and draws it as a filled column graph (newest on the right).
// The Stop/Start button in the window freezes/resumes sampling.
const PROF_SAMPLES: usize = 30;
const PROF_BTN_W: i32 = 64;
const PROF_BTN_H: i32 = 20;

struct Profiler {
    samples: [u16; PROF_SAMPLES], // CPU load in permille (0..=1000), ring buffer
    head: usize,                  // next write index
    count: usize,                 // valid samples
    running: bool,
    primed: bool,                 // false until the first baseline sample is taken
    prev_busy: u64,
    prev_total: u64,
    cur: u16,                     // latest sample (permille), for the header readout
}

impl Profiler {
    fn new() -> Profiler {
        Profiler {
            samples: [0u16; PROF_SAMPLES],
            head: 0,
            count: PROF_SAMPLES, // ring kept full (zero-padded) so the graph spans full width
            running: true,
            primed: false,
            prev_busy: 0,
            prev_total: 0,
            cur: 0,
        }
    }

    // Re-baseline so the first sample after a resume isn't a huge delta.
    fn rebaseline(&mut self) { self.primed = false; }

    // Click on the top-right toggle freezes/resumes sampling.
    fn on_mouse_up(&mut self, x: i32, y: i32, bw: i32) -> (bool, InputAction) {
        if prof_button(x, y, bw) {
            self.running = !self.running;
            if self.running { self.rebaseline(); }
            return (true, InputAction::None);
        }
        (false, InputAction::None)
    }

    fn sample(&mut self) {
        let user = syscall(SYS_CPU_INFO, 1, 0, 0);
        let kernel = syscall(SYS_CPU_INFO, 2, 0, 0);
        let idle = syscall(SYS_CPU_INFO, 3, 0, 0);
        let busy = user.saturating_add(kernel);
        let total = busy.saturating_add(idle);
        let dbusy = busy.saturating_sub(self.prev_busy);
        let dtotal = total.saturating_sub(self.prev_total).max(1);
        self.prev_busy = busy;
        self.prev_total = total;
        if !self.primed { self.primed = true; return; }
        let permille = (dbusy.saturating_mul(1000) / dtotal).min(1000) as u16;
        self.cur = permille;
        self.samples[self.head] = permille;
        self.head = (self.head + 1) % PROF_SAMPLES;
        if self.count < PROF_SAMPLES { self.count += 1; }
    }

    // Draw into the window's content canvas at body-local coords (origin = body top-left).
    fn draw(&self, s: &mut Screen) {
        let bg = s.pack(0x20, 0x24, 0x2c);
        let white = s.pack(0xe8, 0xe8, 0xe8);
        let (w, h) = (s.width, s.height);
        s.fill(0, 0, w, h, bg);

        s.text(8, 8, b"CPU load", white);
        // current load "NN.N%"
        let tx = 8 + 10 * ADVANCE;
        let mut buf = [0u8; 24];
        let ni = fmt_u64((self.cur / 10) as u64, &mut buf);
        s.text(tx, 8, &buf[..ni], white);
        s.text(tx + ni as i32 * ADVANCE, 8, b".", white);
        let mut f = [0u8; 24];
        let nf = fmt_u64((self.cur % 10) as u64, &mut f);
        s.text(tx + (ni as i32 + 1) * ADVANCE, 8, &f[..nf], white);
        s.text(tx + (ni as i32 + 1 + nf as i32) * ADVANCE, 8, b"%", white);

        // toggle button (top-right): red "Stop" while running, green "Start" while paused
        let bx = w - PROF_BTN_W - 8;
        let by = 6;
        let border = s.pack(0x00, 0x00, 0x00);
        let face = if self.running { s.pack(0xC0, 0x50, 0x50) } else { s.pack(0x40, 0xA0, 0x50) };
        s.fill(bx, by, PROF_BTN_W, PROF_BTN_H, border);
        s.fill(bx + 1, by + 1, PROF_BTN_W - 2, PROF_BTN_H - 2, face);
        let label: &[u8] = if self.running { b"Stop" } else { b"Start" };
        let lx = bx + (PROF_BTN_W - label.len() as i32 * ADVANCE) / 2;
        s.text(lx, by + (PROF_BTN_H - GLYPH_H) / 2, label, white);

        // graph area
        let gx0 = 8;
        let gy0 = 34;
        let gx1 = w - 8;
        let gy1 = h - 8;
        if gx1 <= gx0 || gy1 <= gy0 { return; }
        let gw = gx1 - gx0;
        let gh = gy1 - gy0;
        s.fill(gx0, gy0, gw, gh, s.pack(0x10, 0x12, 0x18));

        // 25/50/75% gridlines
        let grid = s.pack(0x30, 0x36, 0x40);
        for q in 1..4 {
            let yy = gy1 - q * gh / 4;
            s.fill(gx0, yy, gw, 1, grid);
        }

        // filled columns: map each graph pixel column to a sample (newest on the right)
        let bar = if self.running { s.pack(0x40, 0xC0, 0x80) } else { s.pack(0x70, 0x78, 0x84) };
        if self.count > 0 {
            for px in 0..gw {
                let i = px as usize * self.count / gw as usize;
                let idx = (self.head + PROF_SAMPLES - self.count + i) % PROF_SAMPLES;
                let bh = self.samples[idx] as i32 * gh / 1000;
                if bh > 0 { s.fill(gx0 + px, gy1 - bh, 1, bh, bar); }
            }
        }

        // graph border
        let bd = s.pack(0x50, 0x58, 0x64);
        s.fill(gx0, gy0, gw, 1, bd);
        s.fill(gx0, gy1 - 1, gw, 1, bd);
        s.fill(gx0, gy0, 1, gh, bd);
        s.fill(gx1 - 1, gy0, 1, gh, bd);
    }
}

// The profiler's toggle button hit test, in body-local coords (bw = body width).
fn prof_button(px: i32, py: i32, bw: i32) -> bool {
    in_rect(px, py, bw - PROF_BTN_W - 8, 6, PROF_BTN_W, PROF_BTN_H)
}

// arrow cursor
const CW: usize = 12;
const CH: usize = 16;
const CURSOR_ROWS: [&[u8; CW]; CH] = [
    b"o           ", b"oo          ", b"owo         ", b"owwo        ",
    b"owwwo       ", b"owwwwo      ", b"owwwwwo     ", b"owwwwwwo    ",
    b"owwwwwwwo   ", b"owwwwwwwwo  ", b"owwwwooooo  ", b"owwowwo     ",
    b"owo owwo    ", b"oo  owwo    ", b"o    owwo   ", b"      ooo   ",
];

fn draw_cursor(s: &mut Screen, cx: i32, cy: i32) {
    let white = s.pack(0xFF, 0xFF, 0xFF);
    let black = s.pack(0x00, 0x00, 0x00);
    for (row, line) in CURSOR_ROWS.iter().enumerate() {
        for (col, &cell) in line.iter().enumerate() {
            let color = match cell { b'o' => black, b'w' => white, _ => continue };
            s.put(cx + col as i32, cy + row as i32, color);
        }
    }
}

// Pack r,g,b in the framebuffer's pixel order. Free fn so per-pixel loops needn't borrow a
// whole &Screen.
fn pack_fmt(fmt: u32, r: u8, g: u8, b: u8) -> u32 {
    if fmt == 1 { (b as u32) << 16 | (g as u32) << 8 | r as u32 }
    else { (r as u32) << 16 | (g as u32) << 8 | b as u32 }
}

fn clamp8(v: i32) -> u8 { v.max(0).min(255) as u8 }

// Desktop background: a diagonal indigo->teal gradient with a bright diamond lattice. The
// sharp lattice lines are what make the taskbar's blur obvious.
fn desktop_px(fmt: u32, x: i32, y: i32, sw: i32, sh: i32) -> u32 {
    let span = (sw + sh).max(1);
    let t = (x + y) * 255 / span;
    let u = (x - y + sh) * 255 / span;
    let mut r = 0x12 + t * 0x22 / 255;
    let mut g = 0x1E + u * 0x2C / 255;
    let mut b = 0x42 + t * 0x26 / 255;
    let d1 = (x + y).rem_euclid(72);
    let d2 = (x - y).rem_euclid(72);
    if d1 < 2 || d2 < 2 { r += 0x18; g += 0x20; b += 0x26; }
    else if d1 < 4 || d2 < 4 { r += 0x09; g += 0x0D; b += 0x11; }
    pack_fmt(fmt, clamp8(r), clamp8(g), clamp8(b))
}

// Precompute the (static) desktop background once, so repaints are a cheap row copy instead
// of per-pixel gradient + lattice math.
fn build_desktop(sw: i32, sh: i32, fmt: u32) -> alloc::vec::Vec<u32> {
    let mut v = alloc::vec![0u32; (sw * sh) as usize];
    for y in 0..sh {
        let row = (y * sw) as usize;
        for x in 0..sw {
            v[row + x as usize] = desktop_px(fmt, x, y, sw, sh);
        }
    }
    v
}

fn paint_desktop(s: &mut Screen, r: Rect, bg: &[u32], sw: i32) {
    let x0 = r.x0.max(s.clip.x0).max(0);
    let y0 = r.y0.max(s.clip.y0).max(0);
    let x1 = r.x1.min(s.clip.x1).min(s.width);
    let y1 = r.y1.min(s.clip.y1).min(s.height);
    if x0 >= x1 { return; }
    let (xs, xe) = (x0 as usize, x1 as usize);
    for y in y0..y1 {
        let dst = (y * s.stride) as usize;
        let src = (y * sw) as usize;
        s.bb[dst + xs..dst + xe].copy_from_slice(&bg[src + xs..src + xe]);
    }
}

// 1D clamped box blur of a strided line: reads `a`, writes `b`. Channels are the three low
// bytes, blurred independently (order-agnostic, so it works for either pixel format).
fn blur_line(a: &[u32], b: &mut [u32], start: usize, stride: usize, count: usize, radius: i32) {
    let r = radius.max(1) as usize;
    let n = count;
    let mut sr = 0u32; let mut sg = 0u32; let mut sb = 0u32;
    let hi0 = r.min(n - 1);
    for i in 0..=hi0 {
        let p = a[start + i * stride];
        sr += p & 0xFF; sg += (p >> 8) & 0xFF; sb += (p >> 16) & 0xFF;
    }
    let mut lo = 0usize; let mut hi = hi0;
    for x in 0..n {
        let cnt = (hi - lo + 1) as u32;
        let rr = sr / cnt; let gg = sg / cnt; let bbv = sb / cnt;
        b[start + x * stride] = (bbv << 16) | (gg << 8) | rr;
        let nlo = (x + 1).saturating_sub(r);
        let nhi = (x + 1 + r).min(n - 1);
        while lo < nlo {
            let p = a[start + lo * stride];
            sr -= p & 0xFF; sg -= (p >> 8) & 0xFF; sb -= (p >> 16) & 0xFF;
            lo += 1;
        }
        while hi < nhi {
            hi += 1;
            let p = a[start + hi * stride];
            sr += p & 0xFF; sg += (p >> 8) & 0xFF; sb += (p >> 16) & 0xFF;
        }
    }
}

// Approximate a Gaussian blur of a screen rect, in place on the back buffer. Three passes of
// a separable box blur converge to a Gaussian (central limit theorem) while staying O(n).
fn blur_region(s: &mut Screen, x0: i32, y0: i32, x1: i32, y1: i32, radius: i32) {
    let x0 = x0.max(0); let y0 = y0.max(0);
    let x1 = x1.min(s.width); let y1 = y1.min(s.height);
    let w = (x1 - x0).max(0) as usize;
    let h = (y1 - y0).max(0) as usize;
    if w == 0 || h == 0 { return; }
    let mut a = alloc::vec![0u32; w * h];
    for yy in 0..h {
        let row = ((y0 + yy as i32) * s.stride + x0) as usize;
        for xx in 0..w { a[yy * w + xx] = s.bb[row + xx]; }
    }
    let mut b = alloc::vec![0u32; w * h];
    for _ in 0..BLUR_PASSES {
        for yy in 0..h { blur_line(&a, &mut b, yy * w, 1, w, radius); }   // horizontal: a -> b
        for xx in 0..w { blur_line(&b, &mut a, xx, w, h, radius); }       // vertical:   b -> a
    }
    for yy in 0..h {
        let row = ((y0 + yy as i32) * s.stride + x0) as usize;
        for xx in 0..w { s.bb[row + xx] = a[yy * w + xx]; }
    }
}

// Alpha-blend a flat `color` (packed) over a screen rect. `alpha` is 0..256.
fn blend_region(s: &mut Screen, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, alpha: u32) {
    let x0 = x0.max(s.clip.x0).max(0);
    let y0 = y0.max(s.clip.y0).max(0);
    let x1 = x1.min(s.clip.x1).min(s.width);
    let y1 = y1.min(s.clip.y1).min(s.height);
    let ia = 256 - alpha;
    let tr = color & 0xFF; let tg = (color >> 8) & 0xFF; let tb = (color >> 16) & 0xFF;
    for y in y0..y1 {
        let row = (y * s.stride) as usize;
        for x in x0..x1 {
            let i = row + x as usize;
            let p = s.bb[i];
            let r = ((p & 0xFF) * ia + tr * alpha) >> 8;
            let g = (((p >> 8) & 0xFF) * ia + tg * alpha) >> 8;
            let b = (((p >> 16) & 0xFF) * ia + tb * alpha) >> 8;
            s.bb[i] = (b << 16) | (g << 8) | r;
        }
    }
}

// Format an uptime in seconds as "up H:MM:SS" into `out`, returning the byte length.
fn fmt_uptime(secs: u64, out: &mut [u8; 24]) -> usize {
    let h = secs / 3600;
    let m = ((secs % 3600) / 60) as u32;
    let s = (secs % 60) as u32;
    let mut n = 0;
    for &c in b"up " { out[n] = c; n += 1; }
    let mut hb = [0u8; 20];
    let mut hl = 0;
    let mut hv = h;
    if hv == 0 { hb[0] = b'0'; hl = 1; }
    else { while hv > 0 { hb[hl] = b'0' + (hv % 10) as u8; hv /= 10; hl += 1; } }
    while hl > 0 { hl -= 1; out[n] = hb[hl]; n += 1; }
    out[n] = b':'; n += 1;
    out[n] = b'0' + (m / 10) as u8; n += 1;
    out[n] = b'0' + (m % 10) as u8; n += 1;
    out[n] = b':'; n += 1;
    out[n] = b'0' + (s / 10) as u8; n += 1;
    out[n] = b'0' + (s % 10) as u8; n += 1;
    n
}

// A translucent taskbar button so the frosted glass shows through; the active one gets a
// brighter wash and an accent underline.
fn draw_tb_button(s: &mut Screen, x: i32, y: i32, w: i32, h: i32, label: &[u8], active: bool) {
    let fmt = s.format;
    if active {
        blend_region(s, x, y, x + w, y + h, pack_fmt(fmt, 0x4C, 0x8C, 0xE0), 150);
        s.fill(x, y + h - 2, w, 2, pack_fmt(fmt, 0x80, 0xB4, 0xFF));
    } else {
        blend_region(s, x, y, x + w, y + h, pack_fmt(fmt, 0x38, 0x44, 0x56), 70);
    }
    let txt = if active { pack_fmt(fmt, 0xFF, 0xFF, 0xFF) } else { pack_fmt(fmt, 0xC6, 0xD0, 0xDC) };
    let maxch = ((w - 12) / ADVANCE).max(0) as usize;
    let n = label.len().min(maxch);
    s.text(x + 6, y + (h - GLYPH_H) / 2, &label[..n], txt);
}

// Vertical page switcher (shown only when buttons overflow): an up arrow (previous page) and
// a down arrow (next page), dimmed at the ends of the range.
fn draw_pager(s: &mut Screen, x: i32, top: i32, page: usize, pages: usize) {
    let fmt = s.format;
    let cx = x + (PAGER_W - ADVANCE) / 2;
    let on = pack_fmt(fmt, 0xFF, 0xFF, 0xFF);
    let off = pack_fmt(fmt, 0x5A, 0x66, 0x76);
    s.text(cx, top + 4, b"^", if page > 0 { on } else { off });
    s.text(cx, top + TASKBAR_H - GLYPH_H - 4, b"v", if page + 1 < pages { on } else { off });
}

struct TbLayout { per_page: usize, pages: usize, btn_max_x: i32, overflow: bool }

// How the buttons fit: everything on one page, or paginated with a pager reserved on the
// right when they overflow.
fn taskbar_layout(n: usize, sw: i32) -> TbLayout {
    let slot = TB_BTN_W + TB_BTN_GAP;
    let full_max = sw - TB_CLOCK_W;
    let fit_full = ((full_max - TB_PAD) / slot).max(0) as usize;
    if n <= fit_full || n == 0 {
        return TbLayout { per_page: fit_full.max(1), pages: 1, btn_max_x: full_max, overflow: false };
    }
    let btn_max_x = full_max - PAGER_W - TB_BTN_GAP;
    let per_page = (((btn_max_x - TB_PAD) / slot).max(1)) as usize;
    let pages = n.div_ceil(per_page);
    TbLayout { per_page, pages, btn_max_x, overflow: true }
}

// Draw the frosted-glass taskbar: blur the strip's backdrop, lay a translucent tint over it,
// then the window buttons (one page worth), pager, and uptime clock.
fn draw_taskbar(s: &mut Screen, windows: &[Window], focused_id: Option<u64>, tb_page: usize, uptime_secs: u64) {
    let sw = s.width;
    let sh = s.height;
    let top = sh - TASKBAR_H;
    let fmt = s.format;

    blur_region(s, 0, top, sw, sh, BLUR_R);
    blend_region(s, 0, top, sw, sh, pack_fmt(fmt, 0x1E, 0x2A, 0x3C), 120);
    blend_region(s, 0, top, sw, top + 1, pack_fmt(fmt, 0x9C, 0xC4, 0xF0), 90); // top hairline

    let mut buf = [0u8; 24];
    let len = fmt_uptime(uptime_secs, &mut buf);
    let clock_w = len as i32 * ADVANCE;
    s.text(sw - TB_PAD - clock_w, top + (TASKBAR_H - GLYPH_H) / 2, &buf[..len], pack_fmt(fmt, 0xDA, 0xE4, 0xF0));

    let (order, n) = taskbar_order(windows);
    let lay = taskbar_layout(n, sw);
    let page = tb_page.min(lay.pages.saturating_sub(1));
    let start = page * lay.per_page;
    let end = (start + lay.per_page).min(n);
    let by = top + 5;
    let bh = TASKBAR_H - 10;
    let mut bx = TB_PAD;
    for si in start..end {
        let wn = &windows[order[si]];
        draw_tb_button(s, bx, by, TB_BTN_W, bh, wn.content.title(), Some(wn.id) == focused_id);
        bx += TB_BTN_W + TB_BTN_GAP;
    }
    if lay.overflow {
        draw_pager(s, lay.btn_max_x + TB_BTN_GAP, top, page, lay.pages);
    }
}

// Button layout order: by stable id (creation order), independent of the z-order, so a
// button never moves when a window is raised/focused — only when one opens or closes.
fn taskbar_order(windows: &[Window]) -> ([usize; 64], usize) {
    let n = windows.len().min(64);
    let mut idx = [0usize; 64];
    for i in 0..n { idx[i] = i; }
    for i in 1..n {
        let mut j = i;
        while j > 0 && windows[idx[j - 1]].id > windows[idx[j]].id {
            idx.swap(j - 1, j);
            j -= 1;
        }
    }
    (idx, n)
}

enum TbHit { Window(u64), PageUp, PageDown, Empty }

// What the taskbar strip click at (mx,my) hit. Mirrors draw_taskbar's layout.
fn taskbar_hit(windows: &[Window], mx: i32, my: i32, sw: i32, sh: i32, tb_page: usize) -> TbHit {
    let top = sh - TASKBAR_H;
    let (order, n) = taskbar_order(windows);
    let lay = taskbar_layout(n, sw);
    if lay.overflow {
        let px = lay.btn_max_x + TB_BTN_GAP;
        if mx >= px && mx < px + PAGER_W {
            return if my < top + TASKBAR_H / 2 { TbHit::PageUp } else { TbHit::PageDown };
        }
    }
    let page = tb_page.min(lay.pages.saturating_sub(1));
    let start = page * lay.per_page;
    let end = (start + lay.per_page).min(n);
    let mut bx = TB_PAD;
    for si in start..end {
        if mx >= bx && mx < bx + TB_BTN_W { return TbHit::Window(windows[order[si]].id); }
        bx += TB_BTN_W + TB_BTN_GAP;
    }
    TbHit::Empty
}

// cursor, and present. Window *content* is re-rendered separately (only when it changes),
// so dragging/restacking here is just cheap blits — no glyph redrawing.
fn composite(s: &mut Screen, windows: &[Window], focused_id: Option<u64>, tb_page: usize, uptime_secs: u64, bg: &[u32], dirty: Rect) {
    if dirty.is_empty() { return; }
    let sw = s.width;
    let sh = s.height;
    let tb_top = sh - TASKBAR_H;
    let mut paint = dirty;
    // The glass taskbar needs its whole strip, not a sub-rect.
    let touch_tb = paint.intersects(0, tb_top, sw, TASKBAR_H);
    if touch_tb { paint.add(0, tb_top, sw, TASKBAR_H); }
    s.set_clip(paint);
    paint_desktop(s, paint, bg, sw);
    // Redraw the chrome of every window overlapping the repaint (clipped to it) — the opaque
    // frame is cheap and backdrop-independent, so a neighbour's update can't erase a border.
    for wn in windows.iter() {
        let g = wn.geo;
        if !paint.intersects(g.x, g.y, g.w, g.h) { continue; }
        let focused = Some(wn.id) == focused_id;
        draw_frame(s, &g, wn.content.title(), focused, wn.content.has_close());
        blit(s, &wn.canvas, g.x + BORDER, g.y + TITLE_H, paint);
    }
    if touch_tb { draw_taskbar(s, windows, focused_id, tb_page, uptime_secs); }
    s.present(paint);
}

fn win_pos(windows: &[Window], id: u64) -> Option<usize> {
    windows.iter().position(|w| w.id == id)
}

// Index of the topmost window (front of the z-order) covering the point, or None.
fn topmost_at(windows: &[Window], px: i32, py: i32) -> Option<usize> {
    for i in (0..windows.len()).rev() {
        let g = windows[i].geo;
        if in_rect(px, py, g.x, g.y, g.w, g.h) { return Some(i); }
    }
    None
}

// Bring the window with `id` to the front of the z-order (end of the list).
fn raise(windows: &mut alloc::vec::Vec<Window>, id: u64) {
    if let Some(p) = win_pos(windows, id) {
        if p + 1 != windows.len() {
            let w = windows.remove(p);
            windows.push(w);
        }
    }
}

// Open a launcher-kind window (1=Task Manager, 2=Terminal, 3=Profiler), staggered by the
// cascade counter. The Task Manager is single-instance: a whole-system view doesn't need
// duplicates (and many at once is needless load), so an existing one is raised instead.
// Newly affected screen rects are unioned into `dr` for compositing.
fn open_or_raise(windows: &mut alloc::vec::Vec<Window>, next_id: &mut u64, cascade: &mut i32,
                 fmt: u32, kind: u8, dr: &mut Rect) {
    if kind == 1 {
        if let Some(p) = windows.iter().position(|w| matches!(w.content, Content::TaskMgr(_))) {
            let id = windows[p].id;
            let g = windows[p].geo;
            raise(windows, id);
            dr.add(g.x, g.y, g.w, g.h);
            for wn in windows.iter() { dr.add(wn.geo.x, wn.geo.y, wn.geo.w, wn.geo.h); }
            return;
        }
    }
    let off = (*cascade % 6) * 24;
    let geo = match kind {
        1 => Win { x: 90 + off, y: 60 + off, w: 540, h: 440 },
        2 => Win { x: 130 + off, y: 96 + off, w: 540, h: 320 },
        _ => Win { x: 110 + off, y: 90 + off, w: 440, h: 260 },
    };
    // Allocate the content canvas first; if we're out of memory, just don't open the
    // window (and don't spawn a shell for a terminal).
    if let Some(canvas) = make_canvas(&geo, fmt) {
        *cascade += 1;
        let id = *next_id;
        *next_id += 1;
        let content = match kind {
            1 => {
                let mut tm = alloc::boxed::Box::new(TaskMon::new());
                tm.baseline(); // start the %CPU window fresh on open
                Content::TaskMgr(tm)
            }
            2 => {
                let mut t = alloc::boxed::Box::new(Term::new());
                t.spawn();
                Content::Terminal(t)
            }
            _ => Content::Profiler(Profiler::new()),
        };
        let mut win = Window { id, geo, content, canvas, dirty: Rect::empty() };
        win.dirty_full(); // render its content before the first composite
        windows.push(win);
        dr.add(geo.x, geo.y, geo.w, geo.h);
    }
}

static FRAME: AtomicU64 = AtomicU64::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let mut info = FbInfo { base: 0, width: 0, height: 0, stride: 0, format: 0 };
    if syscall(SYS_FB_ACQUIRE, &mut info as *mut FbInfo as u64, 0, 0) == u64::MAX {
        sys_write(b"gui: failed to acquire framebuffer\r\n");
        sys_exit(1);
    }
    syscall(SYS_SIGNAL_CATCH, 1, 0, 0);
    let ipc = syscall(SYS_IPC_OPEN, b"module_mouse".as_ptr() as u64, 12, 0);
    if ipc == u64::MAX {
        sys_write(b"gui: IPC module_mouse not found (is ps2mouse.kkm loaded?)\r\n");
        syscall(SYS_FB_RELEASE, 0, 0, 0);
        sys_exit(1);
    }

    // A dedicated input thread keeps the cursor position current independently of how
    // long a render frame takes; both run at normal scheduler priority (the kernel uses
    // round-robin, so every thread gets a fair turn even when the cores are saturated).
    let mut s = Screen::new(&info);
    let sw = s.width;
    let sh = s.height;

    // The window list starts with just the launcher (id 1). Its order is the z-order.
    let fmt = s.format;
    let mut windows: alloc::vec::Vec<Window> = alloc::vec::Vec::new();
    let launcher_geo = Win { x: (sw - 240) / 2, y: (sh - 200) / 2, w: 240, h: 200 };
    let Some(launcher_canvas) = make_canvas(&launcher_geo, fmt) else {
        sys_write(b"gui: out of memory\r\n");
        syscall(SYS_FB_RELEASE, 0, 0, 0);
        sys_exit(1);
    };
    windows.push(Window {
        id: 1,
        geo: launcher_geo,
        content: Content::Launcher(Launcher::new()),
        canvas: launcher_canvas,
        dirty: Rect::empty(),
    });
    let mut next_id: u64 = 2;
    let mut cascade: i32 = 0; // staggers each new window so they don't stack exactly

    let mut mx = sw / 2;
    let mut my = sh / 2;
    let mut prev_left = false;
    let mut dragging: Option<u64> = None;
    let mut grab_dx = 0i32;
    let mut grab_dy = 0i32;
    // The window that captured the current mouse press (a body click); move/up events go
    // to it until the button is released, so a content's input stays with one window.
    let mut mouse_capture: Option<u64> = None;
    // The active window (drawn with a lit title bar / highlighted taskbar button), tracked
    // separately from the z-order: clicking the empty desktop or taskbar clears it to None.
    let mut focused_id: Option<u64> = Some(1);
    let mut tb_page: usize = 0; // current taskbar page when window buttons overflow

    // Initial paint: render the launcher's content canvas, then composite the whole screen.
    {
        let canvas = &mut windows[0].canvas;
        let d = canvas.full_rect();
        canvas.set_clip(d);
        draw_launcher_body(canvas, 0);
    }
    let bg = build_desktop(sw, sh, fmt);
    let full = s.full_rect();
    composite(&mut s, &windows, focused_id, tb_page, 0, &bg, full);

    let cursor_w = CW as i32;
    let cursor_h = CH as i32;
    // Software-cursor save-under: the pixels currently behind the cursor, so it can move
    // without forcing the (expensive) glass chrome/taskbar to repaint.
    let mut cur_save = alloc::vec![0u32; (cursor_w * cursor_h) as usize];
    let mut cur_x = mx;
    let mut cur_y = my;
    let mut cur_shown = false;
    let mut quit = false;

    // Hand the mouse channel + screen bounds to the input thread and start it. From here
    // the render loop reads the cursor position from the atomics it keeps current.
    MOUSE_X.store(mx, Ordering::Relaxed);
    MOUSE_Y.store(my, Ordering::Relaxed);
    SCR_W.store(sw, Ordering::Relaxed);
    SCR_H.store(sh, Ordering::Relaxed);
    MOUSE_IPC.store(ipc, Ordering::Relaxed);
    let _input = thread_create(input_loop);

    // The kernel timer-tick counter (SYS_CPU_INFO 0) is monotonic but its rate depends on
    // the LAPIC/PIT path and isn't otherwise exposed, so measure ticks-per-second once with
    // a known sleep. Uptime is then counted from here (desktop session start).
    let start_ticks = syscall(SYS_CPU_INFO, 0, 0, 0);
    syscall(SYS_SLEEP, 500, SLEEP_UNIT_MS, 0);
    let ticks_per_sec = (syscall(SYS_CPU_INFO, 0, 0, 0).saturating_sub(start_ticks) * 2).max(1);
    let mut uptime_secs: u64 = 0;
    // (window count, focused id, uptime secs) — when this changes, repaint the taskbar.
    let mut last_tb_sig: (usize, u64, u64, u64) = (usize::MAX, 0, 0, 0);

    loop {
        if syscall(SYS_SIGNAL_CHECK, 0, 0, 0) != 0 { break; }

        // Read the latest mouse state the input thread published (no IPC here).
        let nmx = MOUSE_X.load(Ordering::Relaxed);
        let nmy = MOUSE_Y.load(Ordering::Relaxed);
        let btn = MOUSE_BTN.load(Ordering::Relaxed) as u8;
        let moved = nmx != mx || nmy != my;
        mx = nmx;
        my = nmy;

        let left = btn & 1 != 0;
        let just_pressed = left && !prev_left;
        let just_released = !left && prev_left;
        prev_left = left;

        let mut dr = Rect::empty();

        // Press: window management (raise / close / title-bar drag) is the compositor's
        // job. A press that lands in the body captures the window and is delegated to its
        // content via on_mouse_down; subsequent move/up go to that captured window.
        if just_pressed && my >= sh - TASKBAR_H {
            // Taskbar strip (above the windows): a button raises+focuses its window, the pager
            // flips pages, and empty space deactivates the focused window.
            match taskbar_hit(&windows, mx, my, sw, sh, tb_page) {
                TbHit::Window(id) => { focused_id = Some(id); raise(&mut windows, id); }
                TbHit::PageUp => { tb_page = tb_page.saturating_sub(1); }
                TbHit::PageDown => { tb_page += 1; }
                TbHit::Empty => { focused_id = None; }
            }
            // Focus/z may change: redraw every window's chrome and the taskbar.
            for wn in &windows { dr.add(wn.geo.x, wn.geo.y, wn.geo.w, wn.geo.h); }
            dr.add(0, sh - TASKBAR_H, sw, TASKBAR_H);
        } else if just_pressed {
            if let Some(p) = topmost_at(&windows, mx, my) {
                let id = windows[p].id;
                raise(&mut windows, id);
                let p = windows.len() - 1; // raised window is now at the front
                let geo = windows[p].geo;
                let (cx, cy, cw, ch) = close_rect(&geo);
                if windows[p].content.has_close() && in_rect(mx, my, cx, cy, cw, ch) {
                    if let Content::Terminal(t) = &mut windows[p].content { t.shutdown(); }
                    windows.remove(p);
                    focused_id = None;
                } else {
                    focused_id = Some(id);
                    if in_rect(mx, my, geo.x, geo.y, geo.w, TITLE_H) {
                        dragging = Some(id);
                        grab_dx = mx - geo.x;
                        grab_dy = my - geo.y;
                    } else {
                        mouse_capture = Some(id);
                        let bx = mx - (geo.x + BORDER);
                        let by = my - (geo.y + TITLE_H);
                        let bw = body_w(&geo);
                        let (dirty, _) = windows[p].content.on_mouse_down(bx, by, bw, btn);
                        if dirty { windows[p].dirty_full(); }
                    }
                }
                // A click can change focus, stacking, or close a window; redraw the clicked
                // area plus every window's chrome (and reclaim a closed window's pixels).
                dr.add(geo.x, geo.y, geo.w, geo.h);
                for wn in &windows { dr.add(wn.geo.x, wn.geo.y, wn.geo.w, wn.geo.h); }
            } else {
                // Click on the empty desktop: deactivate the focused window.
                focused_id = None;
                for wn in &windows { dr.add(wn.geo.x, wn.geo.y, wn.geo.w, wn.geo.h); }
                dr.add(0, sh - TASKBAR_H, sw, TASKBAR_H);
            }
        }

        if !left { dragging = None; }

        // Release: deliver on_mouse_up to the captured window and apply its action (open a
        // window, quit), then drop the capture.
        if just_released {
            if let Some(id) = mouse_capture.take() {
                if let Some(p) = win_pos(&windows, id) {
                    let geo = windows[p].geo;
                    let bx = mx - (geo.x + BORDER);
                    let by = my - (geo.y + TITLE_H);
                    let bw = body_w(&geo);
                    let (dirty, action) = windows[p].content.on_mouse_up(bx, by, bw, btn);
                    if dirty { windows[p].dirty_full(); }
                    match action {
                        InputAction::Open(k) => {
                            open_or_raise(&mut windows, &mut next_id, &mut cascade, fmt, k, &mut dr);
                            focused_id = windows.last().map(|w| w.id); // the new/raised window
                            for wn in &windows { dr.add(wn.geo.x, wn.geo.y, wn.geo.w, wn.geo.h); }
                        }
                        InputAction::Quit => quit = true,
                        InputAction::None => {}
                    }
                }
            }
        }

        // Motion: dragging a title bar moves the window (compositor); otherwise the
        // captured window's content gets on_mouse_move (e.g. launcher button highlight).
        if moved {
            if let Some(id) = dragging {
                if let Some(p) = win_pos(&windows, id) {
                    let old = windows[p].geo;
                    let nx = mx - grab_dx;
                    // Keep the title bar reachable: don't let it slide under the taskbar.
                    let ny = (my - grab_dy).min(sh - TASKBAR_H - TITLE_H);
                    if nx != old.x || ny != old.y {
                        dr.add(old.x, old.y, old.w, old.h); // vacated
                        windows[p].geo.x = nx;
                        windows[p].geo.y = ny;
                        dr.add(nx, ny, old.w, old.h);       // new
                    }
                }
            } else if let Some(id) = mouse_capture {
                if let Some(p) = win_pos(&windows, id) {
                    let geo = windows[p].geo;
                    let bx = mx - (geo.x + BORDER);
                    let by = my - (geo.y + TITLE_H);
                    let bw = body_w(&geo);
                    let (dirty, _) = windows[p].content.on_mouse_move(bx, by, bw, btn);
                    if dirty { windows[p].dirty_full(); }
                }
            }
        }

        // Refresh every open Task Manager about twice a second so %CPU stays live.
        let frame = FRAME.fetch_add(1, Ordering::Relaxed);
        if frame % 32 == 0 {
            for wn in windows.iter_mut() {
                match &mut wn.content {
                    Content::TaskMgr(tm) => { tm.refresh(); wn.dirty_full(); }
                    Content::Profiler(p) if p.running => { p.sample(); wn.dirty_full(); }
                    _ => {}
                }
            }
        }

        // Keyboard: the desktop owns the keyboard; route each press to the focused window
        // (the front of the z-order) via on_keydown. The content decides what to do —
        // terminals forward/interrupt, others quit on Ctrl+C — and may return an action
        // (the kernel no longer turns Ctrl+C into a signal for us, so it can't kill the
        // compositor by accident).
        loop {
            // Each word is a key code in the low byte (KEY_RELEASE set on release, plus
            // MOD_* modifier-state bits), so the focused window sees every press/release —
            // characters, arrows, Ctrl/Shift/Alt/Caps, Esc, F-keys. `b` keeps just the code.
            let k = syscall(SYS_KEYBOARD_POLL, 0, 0, 0);
            if k == 0 { break; }
            let b = k as u8;
            let release = k & KEY_RELEASE as u64 != 0;
            // Route only to the active window; with nothing focused, keys are discarded.
            if let Some(p) = focused_id.and_then(|fid| win_pos(&windows, fid)) {
                let wn = &mut windows[p];
                let (dirty, action) = if release {
                    wn.content.on_keyup(b)
                } else {
                    wn.content.on_keydown(b)
                };
                if dirty { wn.dirty_full(); }
                if let InputAction::Quit = action { quit = true; }
            }
        }

        // Drain every open terminal's shell output into its grid, marking only the rows
        // that changed dirty (in canvas-local coords) so a busy terminal re-renders just
        // those rows of its content canvas, not the whole window.
        for wn in windows.iter_mut() {
            let cw = wn.canvas.width;
            let mut rows: Option<(usize, usize)> = None;
            let mut scrolled = 0usize;
            if let Content::Terminal(t) = &mut wn.content {
                if t.pump() {
                    let (s, d) = t.take_frame();
                    scrolled = s;
                    rows = d;
                }
            }
            // Scroll the rendered pixels up before repainting the exposed bottom rows.
            if scrolled > 0 { scroll_term_canvas_up(&mut wn.canvas, scrolled); }
            if let Some((r0, r1)) = rows {
                let y_top = (4 + r0 as i32 * LINE_H - 2).max(0);
                let y_bot = 4 + r1 as i32 * LINE_H + 2;
                wn.dirty_rect(0, y_top, cw, y_bot - y_top);
            }
        }
        if quit { break; }

        // Re-render the content canvas of every window whose content changed (only the
        // dirty region), and mark that region of the screen dirty for compositing.
        for wn in windows.iter_mut() {
            if wn.dirty.is_empty() { continue; }
            let mut d = wn.dirty;
            wn.dirty = Rect::empty();
            d.x0 = d.x0.max(0); d.y0 = d.y0.max(0);
            d.x1 = d.x1.min(wn.canvas.width); d.y1 = d.y1.min(wn.canvas.height);
            if d.x0 >= d.x1 || d.y0 >= d.y1 { continue; }
            let g = wn.geo;
            {
                let Window { canvas, content, .. } = &mut *wn;
                canvas.set_clip(d);
                match &*content {
                    Content::Launcher(l) => draw_launcher_body(canvas, l.pressed),
                    Content::TaskMgr(tm) => tm.draw(canvas),
                    Content::Terminal(t) => t.draw(canvas),
                    Content::Profiler(p) => p.draw(canvas),
                }
            }
            dr.add(g.x + BORDER + d.x0, g.y + TITLE_H + d.y0, d.x1 - d.x0, d.y1 - d.y0);
        }

        // Keep the page in range as windows open/close (or after a PageDown past the end).
        let pages = taskbar_layout(windows.len(), sw).pages;
        if tb_page >= pages { tb_page = pages - 1; }

        uptime_secs = syscall(SYS_CPU_INFO, 0, 0, 0).saturating_sub(start_ticks) / ticks_per_sec;
        let tb_sig = (windows.len(), focused_id.unwrap_or(0), tb_page as u64, uptime_secs);
        if tb_sig != last_tb_sig {
            last_tb_sig = tb_sig;
            dr.add(0, sh - TASKBAR_H, sw, TASKBAR_H);
        }

        // Cursor as a software overlay with save-under: erase it (restore the saved pixels)
        // before compositing so blurs never sample the cursor, composite the dirty region,
        // then save under the new position and redraw. Skipped entirely when nothing changed.
        let composite_ran = !dr.is_empty();
        if moved || composite_ran || !cur_shown {
            if cur_shown { s.restore_under(&cur_save, cur_x, cur_y, cursor_w, cursor_h); }
            if composite_ran {
                composite(&mut s, &windows, focused_id, tb_page, uptime_secs, &bg, dr);
            }
            let moved_cursor = !cur_shown || cur_x != mx || cur_y != my;
            s.save_under(&mut cur_save, mx, my, cursor_w, cursor_h);
            let fr = s.full_rect();
            s.set_clip(fr);
            draw_cursor(&mut s, mx, my);
            if cur_shown && moved_cursor {
                s.present(Rect { x0: cur_x, y0: cur_y, x1: cur_x + cursor_w, y1: cur_y + cursor_h });
            }
            s.present(Rect { x0: mx, y0: my, x1: mx + cursor_w, y1: my + cursor_h });
            cur_x = mx;
            cur_y = my;
            cur_shown = true;
        }

        // Cap near 60 Hz. Rendering is dirty-rect only, so an idle frame is cheap; the
        // input thread keeps the cursor position current on its own core meanwhile.
        syscall(SYS_SLEEP, 16, SLEEP_UNIT_MS, 0);
    }

    // Tear down any live shells before releasing the framebuffer.
    for wn in windows.iter_mut() {
        if let Content::Terminal(t) = &mut wn.content { t.shutdown(); }
    }
    syscall(SYS_IPC_CLOSE, ipc, 0, 0);
    syscall(SYS_FB_RELEASE, 0, 0, 0);
    sys_exit(0);
}
