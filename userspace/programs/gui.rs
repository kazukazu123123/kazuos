#![no_std]
#![no_main]
include!("../runtime/runtime.rs");
include!("../runtime/gui_protocol.rs");

// Ring3 desktop compositor with a built-in launcher. Applications are independent
// processes that submit SHM surfaces over directed IPC. Quit with Ctrl+Alt+Esc.
// Rendering is double-buffered and the event loop polls input and clients non-blocking.

use core::sync::atomic::{fence, AtomicU64, Ordering};

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
const MOD_CTRL: u16 = 0x400;
const MOD_ALT: u16 = 0x800;

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
    const fn empty() -> Rect { Rect { x0: i32::MAX, y0: i32::MAX, x1: i32::MIN, y1: i32::MIN } }
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

const MAX_DIRTY_RECTS: usize = 16;
struct DirtyRects { rects: [Rect; MAX_DIRTY_RECTS], len: usize }
impl DirtyRects {
    fn new() -> Self { Self { rects: [Rect::empty(); MAX_DIRTY_RECTS], len: 0 } }
    fn is_empty(&self) -> bool { self.len == 0 }
    fn add(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if w <= 0 || h <= 0 { return; }
        let r = Rect { x0: x, y0: y, x1: x + w, y1: y + h };
        if self.len < MAX_DIRTY_RECTS { self.rects[self.len] = r; self.len += 1; }
        else { self.rects[0].add(x, y, w, h); }
    }
    fn iter(&self) -> core::slice::Iter<'_, Rect> { self.rects[..self.len].iter() }
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

fn blit_shared(dst: &mut Screen, address: u64, width: i32, height: i32,
               dx: i32, dy: i32, clip: Rect) {
    let x0 = dx.max(clip.x0).max(0);
    let y0 = dy.max(clip.y0).max(0);
    let x1 = (dx + width).min(clip.x1).min(dst.width);
    let y1 = (dy + height).min(clip.y1).min(dst.height);
    if address == u64::MAX || x0 >= x1 || y0 >= y1 { return; }
    for y in y0..y1 {
        let src_row = ((y - dy) * width + (x0 - dx)) as usize;
        let dst_row = (y * dst.stride + x0) as usize;
        for x in 0..(x1 - x0) as usize {
            dst.bb[dst_row + x] = unsafe { (address as *const u32).add(src_row + x).read_volatile() };
        }
    }
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
const TASKBAR_BLUR: bool = true;
const BLUR_R: i32 = 8;
const BLUR_PASSES: usize = 3;
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

struct ExternalClient {
    pid: u64,
    channel: u64,
    buffers: [(u64, u64); GUI_BUFFER_COUNT],
    width: i32,
    height: i32,
    front: Option<usize>,
    held: [bool; GUI_BUFFER_COUNT],
    title: [u8; GUI_TITLE_MAX],
    title_len: usize,
    close_frames: u16,
    transport_failed: bool,
}

impl ExternalClient {
    fn send(&mut self, message: GuiMessage) -> bool {
        let mut envelope = [0u8; IPC_PID_PREFIX_SIZE + GUI_MESSAGE_SIZE];
        envelope[..IPC_PID_PREFIX_SIZE].copy_from_slice(&self.pid.to_le_bytes());
        envelope[IPC_PID_PREFIX_SIZE..].copy_from_slice(message.as_bytes());
        let ok = sys_ipc_try_send_to_envelope(self.channel, &envelope) == GUI_MESSAGE_SIZE as u64;
        if !ok { self.transport_failed = true; }
        ok
    }

    fn send_mouse(&mut self, x: i32, y: i32, buttons: u8, changed: u32) {
        let _ = self.send(GuiMessage::mouse(x, y, buttons as u32, changed));
    }

    fn cleanup(&mut self, kill: bool) -> bool {
        if kill && self.pid != 0 { let _ = sys_kill(self.pid); }
        self.pid = 0;
        let mut ok = true;
        for buffer in self.buffers.iter_mut() {
            if buffer.0 != u64::MAX && sys_shm_close(buffer.0) != 0 { ok = false; }
            *buffer = (u64::MAX, u64::MAX);
        }
        ok
    }
}

enum Content {
    Launcher(Launcher),
    External(alloc::boxed::Box<ExternalClient>),
}

impl Content {
    fn title(&self) -> &[u8] {
        match self {
            Content::Launcher(_) => b"KazuOS",
            Content::External(client) => &client.title[..client.title_len],
        }
    }
    fn has_close(&self) -> bool { !matches!(self, Content::Launcher(_)) }

    // Input is delegated to the focused/captured window's content. Coordinates are
    // body-local (origin = content canvas top-left); `bw` is the body width and
    // `buttons` is the raw mouse button byte. Each handler returns whether its content
    // changed (so the compositor re-renders the canvas) plus any window-level action.
    fn on_keydown(&mut self, b: u8) -> (bool, InputAction) {
        match self {
            Content::External(client) => { let _ = client.send(GuiMessage::key(b, false)); (false, InputAction::None) }
            _ => (false, InputAction::None),
        }
    }

    fn on_keyup(&mut self, b: u8) -> (bool, InputAction) {
        if let Content::External(client) = self { let _ = client.send(GuiMessage::key(b, true)); }
        (false, InputAction::None)
    }

    fn on_mouse_down(&mut self, x: i32, y: i32, _bw: i32, buttons: u8, changed: u8) -> (bool, InputAction) {
        match self {
            Content::Launcher(l) => l.on_mouse_down(x, y, buttons),
            Content::External(client) => { client.send_mouse(x, y, buttons, changed as u32); (false, InputAction::None) }
            _ => (false, InputAction::None),
        }
    }

    fn on_mouse_up(&mut self, x: i32, y: i32, bw: i32, buttons: u8, changed: u8) -> (bool, InputAction) {
        match self {
            Content::Launcher(l) => l.on_mouse_up(x, y),
            Content::External(client) => { client.send_mouse(x, y, buttons, changed as u32); (false, InputAction::None) }
        }
    }

    fn on_mouse_buttons(&mut self, x: i32, y: i32, buttons: u8, changed: u8) {
        if let Content::External(client) = self {
            client.send_mouse(x, y, buttons, changed as u32);
        }
    }

    fn on_mouse_move(&mut self, x: i32, y: i32, _bw: i32, buttons: u8) -> (bool, InputAction) {
        match self {
            Content::Launcher(l) => l.on_mouse_move(x, y, buttons),
            Content::External(client) => { client.send_mouse(x, y, buttons, 0); (false, InputAction::None) }
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
        for i in 0..4 {
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
    draw_button(c, BTN_X - 1, launcher_btn_y(3), b"Shared App", pressed_btn == 4);
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

    if TASKBAR_BLUR {
        blur_region(s, 0, top, sw, sh, BLUR_R);
    }
    blend_region(s, 0, top, sw, sh, pack_fmt(fmt, 0x1E, 0x2A, 0x3C), 120);
    blend_region(s, 0, top, sw, top + 1, pack_fmt(fmt, 0x9C, 0xC4, 0xF0), 90); // top hairline

    let mut buf = [0u8; 24];
    let len = fmt_uptime(uptime_secs, &mut buf);
    let clock_w = len as i32 * ADVANCE;
    s.text(sw - TB_PAD - clock_w, top + (TASKBAR_H - GLYPH_H) / 2, &buf[..len], pack_fmt(fmt, 0xDA, 0xE4, 0xF0));

    let order = taskbar_order(windows);
    let n = order.len();
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
fn taskbar_order(windows: &[Window]) -> alloc::vec::Vec<usize> {
    let mut order = (0..windows.len()).collect::<alloc::vec::Vec<_>>();
    order.sort_unstable_by_key(|index| windows[*index].id);
    order
}

enum TbHit { Window(u64), PageUp, PageDown, Empty }

// What the taskbar strip click at (mx,my) hit. Mirrors draw_taskbar's layout.
fn taskbar_hit(windows: &[Window], mx: i32, my: i32, sw: i32, sh: i32, tb_page: usize) -> TbHit {
    let top = sh - TASKBAR_H;
    let order = taskbar_order(windows);
    let n = order.len();
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
        if let Content::External(client) = &wn.content {
            if let Some(front) = client.front {
                blit_shared(s, client.buffers[front].1, client.width, client.height,
                            g.x + BORDER, g.y + TITLE_H, paint);
            }
        } else {
            blit(s, &wn.canvas, g.x + BORDER, g.y + TITLE_H, paint);
        }
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

const MIN_CLIENT_WIDTH: u32 = 64;
const MIN_CLIENT_HEIGHT: u32 = 64;
const MAX_CLIENT_WIDTH: u32 = 1024;
const MAX_CLIENT_HEIGHT: u32 = 768;

fn close_buffer_pairs(buffers: &mut [(u64, u64); GUI_BUFFER_COUNT]) {
    for buffer in buffers.iter_mut() {
        if buffer.0 != u64::MAX { let _ = sys_shm_close(buffer.0); }
        *buffer = (u64::MAX, u64::MAX);
    }
}

fn create_external(pid: u64, channel: u64, width: u32, height: u32, title: &[u8], next_id: &mut u64,
                   cascade: &mut i32, format: u32) -> Option<Window> {
    if !(MIN_CLIENT_WIDTH..=MAX_CLIENT_WIDTH).contains(&width)
        || !(MIN_CLIENT_HEIGHT..=MAX_CLIENT_HEIGHT).contains(&height) {
        return None;
    }
    let canvas = Screen::offscreen(1, 1, format)?;
    let bytes = (width as u64).checked_mul(height as u64)?.checked_mul(4)?;
    let mut buffers = [(u64::MAX, u64::MAX); GUI_BUFFER_COUNT];
    for buffer in buffers.iter_mut() {
        let id = sys_shm_create(bytes);
        if id == u64::MAX { close_buffer_pairs(&mut buffers); return None; }
        let address = sys_shm_map(id);
        if address == u64::MAX {
            let _ = sys_shm_close(id);
            close_buffer_pairs(&mut buffers);
            return None;
        }
        *buffer = (id, address);
    }
    for buffer in buffers {
        if sys_shm_grant(buffer.0, pid) != 0 {
            let _ = sys_kill(pid);
            close_buffer_pairs(&mut buffers);
            return None;
        }
    }
    let mut stored_title = [0u8; GUI_TITLE_MAX];
    stored_title[..title.len()].copy_from_slice(title);
    let mut client = ExternalClient {
        pid, channel, buffers, width: width as i32, height: height as i32,
        front: None, held: [false; GUI_BUFFER_COUNT], title: stored_title,
        title_len: title.len(), close_frames: 0, transport_failed: false,
    };
    if !client.send(GuiMessage::init(width, height, width, format,
                                     [buffers[0].0, buffers[1].0])) {
        client.cleanup(true);
        return None;
    }
    let off = (*cascade % 6) * 24;
    *cascade += 1;
    let geo = Win {
        x: 150 + off,
        y: 80 + off,
        w: width as i32 + 2 * BORDER,
        h: height as i32 + TITLE_H + BORDER,
    };
    let id = *next_id;
    *next_id += 1;
    Some(Window { id, geo, content: Content::External(alloc::boxed::Box::new(client)),
                  canvas, dirty: Rect::empty() })
}

#[derive(Default)]
struct ExternalPoll {
    added: Option<u64>,
    commits: u32,
    releases: u32,
    removed: u32,
    destroyed: u32,
    cleanup_failed: bool,
}

fn remove_external(windows: &mut alloc::vec::Vec<Window>, index: usize, kill: bool,
                   dr: &mut DirtyRects, result: &mut ExternalPoll) {
    if let Content::External(client) = &mut windows[index].content {
        if !client.cleanup(kill) { result.cleanup_failed = true; }
    }
    let old = windows.remove(index).geo;
    result.removed += 1;
    dr.add(old.x, old.y, old.w, old.h);
    for window in windows.iter() { dr.add(window.geo.x, window.geo.y, window.geo.w, window.geo.h); }
}

fn poll_external(channel: u64, windows: &mut alloc::vec::Vec<Window>, next_id: &mut u64,
                 cascade: &mut i32, format: u32, dr: &mut DirtyRects) -> ExternalPoll {
    let mut result = ExternalPoll::default();
    let mut envelope = [0u8; IPC_MAX_ENVELOPE_SIZE];
    for _ in 0..32 {
        let received = sys_ipc_try_recv_from_envelope(channel, &mut envelope);
        if received == 0 { break; }
        if received == u64::MAX || received < IPC_PID_PREFIX_SIZE as u64
            || received as usize > envelope.len() { break; }
        let sender = u64::from_le_bytes(envelope[..IPC_PID_PREFIX_SIZE].try_into().unwrap());
        let payload_len = received as usize - IPC_PID_PREFIX_SIZE;
        let existing = windows.iter().position(|window| matches!(
            &window.content, Content::External(client) if client.pid == sender));
        if payload_len != GUI_MESSAGE_SIZE {
            if let Some(index) = existing { remove_external(windows, index, true, dr, &mut result); }
            continue;
        }
        let mut bytes = [0u8; GUI_MESSAGE_SIZE];
        bytes.copy_from_slice(&envelope[IPC_PID_PREFIX_SIZE..IPC_PID_PREFIX_SIZE + GUI_MESSAGE_SIZE]);
        let Some(message) = GuiMessage::from_bytes(bytes) else {
            if let Some(index) = existing { remove_external(windows, index, true, dr, &mut result); }
            continue;
        };
        if message.kind() == GUI_MSG_CONNECT {
            if existing.is_none() {
                if let Some(title) = message.title() {
                    if let Some(window) = create_external(sender, channel, message.width(), message.height(), title,
                                                          next_id, cascade, format) {
                        dr.add(window.geo.x, window.geo.y, window.geo.w, window.geo.h);
                        result.added = Some(window.id);
                        windows.push(window);
                    }
                }
            }
            continue;
        }
        let Some(index) = existing else { continue; };
        let geo = windows[index].geo;
        let client = match &mut windows[index].content {
            Content::External(client) => client,
            _ => unreachable!(),
        };
        let mut remove = false;
        let mut kill = false;
        let mut destroyed = false;
        match message.kind() {
            GUI_MSG_COMMIT => {
                let buffer = message.buffer() as usize;
                let x = message.damage_x();
                let y = message.damage_y();
                let w = message.damage_width();
                let h = message.damage_height();
                let valid_damage = w != 0 && h != 0
                    && x.checked_add(w).is_some_and(|end| end <= client.width as u32)
                    && y.checked_add(h).is_some_and(|end| end <= client.height as u32);
                if buffer >= GUI_BUFFER_COUNT || client.held[buffer] || !valid_damage {
                    remove = true;
                    kill = true;
                } else {
                    fence(Ordering::Acquire);
                    if let Some(old) = client.front {
                        if old != buffer {
                            client.held[old] = false;
                            fence(Ordering::Release);
                            if client.send(GuiMessage::buffer_release(old as u8)) {
                                result.releases += 1;
                            } else {
                                remove = true;
                            }
                        }
                    }
                    if !remove {
                        client.held[buffer] = true;
                        client.front = Some(buffer);
                        result.commits += 1;
                        dr.add(geo.x + BORDER, geo.y + TITLE_H, client.width, client.height);
                    }
                }
            }
            GUI_MSG_STATS_REQUEST => {
                let _ = client.send(GuiMessage::stats_response(
                    GUI_FRAMES.load(Ordering::Relaxed),
                    GUI_COMPOSITES.load(Ordering::Relaxed),
                    GUI_DIRTY_PIXELS.load(Ordering::Relaxed),
                    GUI_RENDER_TICKS.load(Ordering::Relaxed),
                    GUI_COMPOSITE_TICKS.load(Ordering::Relaxed),
                    GUI_PRESENT_TICKS.load(Ordering::Relaxed),
                    GUI_TICKS_PER_SEC.load(Ordering::Relaxed),
                ));
            }
            GUI_MSG_DESTROY => { remove = true; destroyed = true; }
            _ => { remove = true; kill = true; }
        }
        if remove {
            remove_external(windows, index, kill, dr, &mut result);
            if destroyed { result.destroyed += 1; }
        }
    }

    let mut index = 0usize;
    while index < windows.len() {
        let mut remove = false;
        if let Content::External(client) = &mut windows[index].content {
            if !gui_client_alive(client.pid) {
                remove = true;
            } else if client.close_frames != 0 {
                client.close_frames = client.close_frames.saturating_add(1);
                if client.close_frames > 120 { remove = true; }
            }
            if client.transport_failed { remove = true; }
        }
        if remove { remove_external(windows, index, true, dr, &mut result); } else { index += 1; }
    }
    result
}

fn gui_client_alive(pid: u64) -> bool {
    let mut info = ProcessInfo::ZERO;
    sys_proc_info(pid, &mut info as *mut ProcessInfo as *mut u64) == 0 && info.state != 4
}

fn notify_focus(windows: &mut [Window], old: Option<u64>, new: Option<u64>) {
    if old == new { return; }
    if let Some(index) = old.and_then(|id| win_pos(windows, id)) {
        if let Content::External(client) = &mut windows[index].content { let _ = client.send(GuiMessage::focus(false)); }
    }
    if let Some(index) = new.and_then(|id| win_pos(windows, id)) {
        if let Content::External(client) = &mut windows[index].content { let _ = client.send(GuiMessage::focus(true)); }
    }
}

fn has_arg(argc: u64, argv: u64, wanted: &[u8]) -> bool {
    if argv == 0 { return false; }
    for index in 0..argc as usize {
        let pointer = unsafe { *(argv as *const u64).add(index) };
        if pointer == 0 { continue; }
        let mut length = 0usize;
        while length <= 64 && unsafe { *(pointer as *const u8).add(length) } != 0 { length += 1; }
        if length == wanted.len() && unsafe { core::slice::from_raw_parts(pointer as *const u8, length) } == wanted {
            return true;
        }
    }
    false
}

static FRAME: AtomicU64 = AtomicU64::new(0);
static GUI_FRAMES: AtomicU64 = AtomicU64::new(0);
static GUI_COMPOSITES: AtomicU64 = AtomicU64::new(0);
static GUI_FULL_REDRAWS: AtomicU64 = AtomicU64::new(0);
static GUI_PARTIAL_REDRAWS: AtomicU64 = AtomicU64::new(0);
static GUI_DIRTY_PIXELS: AtomicU64 = AtomicU64::new(0);
static GUI_RENDER_TICKS: AtomicU64 = AtomicU64::new(0);
static GUI_COMPOSITE_TICKS: AtomicU64 = AtomicU64::new(0);
static GUI_PRESENT_TICKS: AtomicU64 = AtomicU64::new(0);
static GUI_TICKS_PER_SEC: AtomicU64 = AtomicU64::new(1);

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    let mut info = FbInfo { base: 0, width: 0, height: 0, stride: 0, format: 0 };
    if syscall(SYS_FB_ACQUIRE, &mut info as *mut FbInfo as u64, 0, 0) == u64::MAX {
        sys_write(b"gui: failed to acquire framebuffer\r\n");
        sys_exit(1);
    }
    let ipc = syscall(SYS_IPC_OPEN, b"module_mouse".as_ptr() as u64, 12, 0);
    if ipc == u64::MAX {
        sys_write(b"gui: IPC module_mouse not found (is ps2mouse.kkm loaded?)\r\n");
        syscall(SYS_FB_RELEASE, 0, 0, 0);
        sys_exit(1);
    }

    let control = sys_ipc_open(b"gui-control");
    if control == u64::MAX {
        sys_write(b"gui: failed to open gui-control\r\n");
        syscall(SYS_IPC_CLOSE, ipc, 0, 0);
        syscall(SYS_FB_RELEASE, 0, 0, 0);
        sys_exit(1);
    }

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
    let mut btn = 0u8;
    let mut mouse_total: Option<(u64, i64, i64)> = None;
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
    let test_many = has_arg(argc, argv, b"--test-many");
    let test_terminal = has_arg(argc, argv, b"--test-terminal");
    let test_path: Option<&[u8]> = if has_arg(argc, argv, b"--test-taskmgr") {
        Some(b"/bin/taskmgr.kxe")
    } else if test_terminal {
        Some(b"/bin/terminal.kxe")
    } else if has_arg(argc, argv, b"--test-profiler") {
        Some(b"/bin/profiler.kxe")
    } else if has_arg(argc, argv, b"--test-client") || test_many {
        Some(b"/bin/guidemo.kxe")
    } else {
        None
    };
    let test_mode = test_path.is_some();
    let test_expected = if test_many { 12usize } else { 1usize };
    if let Some(path) = test_path {
        for _ in 0..test_expected {
            let pid = if test_terminal {
                sys_exec_with(path, &[b"--test-nested-shell"], 1 << 16)
            } else {
                sys_exec(path, 1 << 16)
            };
            if pid == 0 || pid == u64::MAX {
                sys_write(b"gui-test: FAIL spawn\r\n");
                syscall(SYS_IPC_CLOSE, control, 0, 0);
                syscall(SYS_IPC_CLOSE, ipc, 0, 0);
                syscall(SYS_FB_RELEASE, 0, 0, 0);
                sys_exit(1);
            }
        }
    }
    let mut notified_focus = None;
    notify_focus(&mut windows, notified_focus, focused_id);
    notified_focus = focused_id;

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
    let mut exit_code = 0u64;
    let mut test_commits = 0u32;
    let mut test_releases = 0u32;
    let mut test_redraw_sent = false;
    let mut test_close_sent = false;
    let mut test_frames = 0u32;

    // The kernel timer-tick counter (SYS_CPU_INFO 0) is monotonic but its rate depends on
    // the LAPIC/PIT path and isn't otherwise exposed, so measure ticks-per-second once with
    // a known sleep. Uptime is then counted from here (desktop session start).
    let start_ticks = syscall(SYS_CPU_INFO, 0, 0, 0);
    syscall(SYS_SLEEP, 500, SLEEP_UNIT_MS, 0);
    let ticks_per_sec = (syscall(SYS_CPU_INFO, 0, 0, 0).saturating_sub(start_ticks) * 2).max(1);
    GUI_TICKS_PER_SEC.store(ticks_per_sec, Ordering::Relaxed);
    let mut uptime_secs: u64 = 0;
    // (window count, focused id, uptime secs) — when this changes, repaint the taskbar.
    let mut last_tb_sig: (usize, u64, u64, u64) = (usize::MAX, 0, 0, 0);

    loop {
        let old_mx = mx;
        let old_my = my;
        let old_buttons = btn;
        let mut mouse_message = [0u8; 25];
        for _ in 0..64 {
            let n = syscall(SYS_IPC_TRY_RECV, ipc, mouse_message.as_mut_ptr() as u64, mouse_message.len() as u64);
            if n != mouse_message.len() as u64 { break; }
            let epoch = u64::from_le_bytes(mouse_message[1..9].try_into().unwrap());
            let total_x = i64::from_le_bytes(mouse_message[9..17].try_into().unwrap());
            let total_y = i64::from_le_bytes(mouse_message[17..25].try_into().unwrap());
            if let Some((last_epoch, last_x, last_y)) = mouse_total {
                if epoch == last_epoch {
                    let dx = total_x.wrapping_sub(last_x).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                    let dy = total_y.wrapping_sub(last_y).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
                    mx = mx.saturating_add(dx).clamp(0, sw - 1);
                    my = my.saturating_sub(dy).clamp(0, sh - 1);
                }
            }
            mouse_total = Some((epoch, total_x, total_y));
            let next_buttons = mouse_message[0];
            let changed = next_buttons != btn;
            btn = next_buttons;
            if changed { break; }
        }
        let moved = old_mx != mx || old_my != my;
        let button_changes = old_buttons ^ btn;

        let left = btn & 1 != 0;
        let just_pressed = left && !prev_left;
        let just_released = !left && prev_left;
        prev_left = left;

        let mut dr = DirtyRects::new();

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
                    if let Content::External(client) = &mut windows[p].content {
                        if client.close_frames == 0 {
                            let _ = client.send(GuiMessage::close_request());
                            client.close_frames = 1;
                        }
                    } else {
                        windows.remove(p);
                        focused_id = None;
                    }
                } else {
                    focused_id = Some(id);
                    notify_focus(&mut windows, notified_focus, focused_id);
                    notified_focus = focused_id;
                    if in_rect(mx, my, geo.x, geo.y, geo.w, TITLE_H) {
                        dragging = Some(id);
                        grab_dx = mx - geo.x;
                        grab_dy = my - geo.y;
                    } else {
                        mouse_capture = Some(id);
                        let bx = mx - (geo.x + BORDER);
                        let by = my - (geo.y + TITLE_H);
                        let bw = body_w(&geo);
                        let (dirty, _) = windows[p].content.on_mouse_down(bx, by, bw, btn, button_changes);
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
                    let (dirty, action) = windows[p].content.on_mouse_up(bx, by, bw, btn, button_changes);
                    if dirty { windows[p].dirty_full(); }
                    match action {
                        InputAction::Open(k) => {
                            let path: &[u8] = match k {
                                1 => b"/bin/taskmgr.kxe",
                                2 => b"/bin/terminal.kxe",
                                3 => b"/bin/profiler.kxe",
                                _ => b"/bin/guidemo.kxe",
                            };
                            let _ = sys_exec(path, 1 << 16);
                            for wn in &windows { dr.add(wn.geo.x, wn.geo.y, wn.geo.w, wn.geo.h); }
                        }
                        InputAction::Quit => quit = true,
                        InputAction::None => {}
                    }
                }
            }
        }

        if button_changes & !1 != 0 && !just_pressed && !just_released {
            let target = mouse_capture.and_then(|id| win_pos(&windows, id)).or_else(|| topmost_at(&windows, mx, my));
            if let Some(p) = target {
                let geo = windows[p].geo;
                if mouse_capture.is_some() || in_rect(mx, my, geo.x + BORDER, geo.y + TITLE_H, body_w(&geo), body_h(&geo)) {
                    let bx = mx - (geo.x + BORDER);
                    let by = my - (geo.y + TITLE_H);
                    windows[p].content.on_mouse_buttons(bx, by, btn, button_changes);
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
            } else if let Some(p) = topmost_at(&windows, mx, my) {
                let geo = windows[p].geo;
                if in_rect(mx, my, geo.x + BORDER, geo.y + TITLE_H, body_w(&geo), body_h(&geo)) {
                    let bx = mx - (geo.x + BORDER);
                    let by = my - (geo.y + TITLE_H);
                    let bw = body_w(&geo);
                    let (dirty, _) = windows[p].content.on_mouse_move(bx, by, bw, 0);
                    if dirty { windows[p].dirty_full(); }
                }
            }
        }

        let external = poll_external(control, &mut windows, &mut next_id, &mut cascade, fmt, &mut dr);
        test_commits = test_commits.saturating_add(external.commits);
        test_releases = test_releases.saturating_add(external.releases);
        if let Some(id) = external.added { focused_id = Some(id); }
        if focused_id.is_some_and(|id| win_pos(&windows, id).is_none()) { focused_id = None; }
        notify_focus(&mut windows, notified_focus, focused_id);
        notified_focus = focused_id;

        if test_mode {
            test_frames = test_frames.saturating_add(1);
            let external_count = windows.iter().filter(|window| matches!(window.content, Content::External(_))).count();
            if test_frames > 600 && (!test_close_sent || external_count != 0) {
                sys_write(b"gui-test: FAIL connect timeout\r\n");
                exit_code = 1;
                quit = true;
            } else if external.cleanup_failed {
                sys_write(b"gui-test: FAIL cleanup\r\n");
                exit_code = 1;
                quit = true;
            } else if external.removed != 0 && !test_close_sent {
                if test_terminal && external_count == 0 && external.destroyed != 0 {
                    sys_write(b"terminal-kill-test: PASS\r\n");
                    sys_write(b"gui-test: PASS\r\n");
                } else {
                    sys_write(b"gui-test: FAIL early exit\r\n");
                    exit_code = 1;
                }
                quit = true;
            } else if test_close_sent && external_count == 0 {
                sys_write(b"gui-test: PASS\r\n");
                quit = true;
            } else if !test_redraw_sent && external_count == test_expected
                && test_commits >= test_expected as u32 {
                let mut sent = true;
                for window in windows.iter_mut() {
                    if let Content::External(client) = &mut window.content {
                        let message = if test_terminal { GuiMessage::key(b'x', false) }
                                      else { GuiMessage::mouse(96, 96, 0, 0) };
                        sent &= client.send(message);
                    }
                }
                test_redraw_sent = sent;
            } else if !test_close_sent
                && test_commits >= (test_expected as u32).saturating_mul(2)
                && test_releases >= test_expected as u32 {
                let mut sent = true;
                for window in windows.iter_mut() {
                    if let Content::External(client) = &mut window.content {
                        if client.send(GuiMessage::close_request()) {
                            client.close_frames = 1;
                        } else {
                            sent = false;
                        }
                    }
                }
                test_close_sent = sent;
            }
        }

        FRAME.fetch_add(1, Ordering::Relaxed);

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
            if !release && b == 27 && k & MOD_CTRL as u64 != 0 && k & MOD_ALT as u64 != 0 {
                quit = true;
                break;
            }
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

        if quit { break; }

        // Re-render the content canvas of every window whose content changed (only the
        // dirty region), and mark that region of the screen dirty for compositing.
        let render_start = syscall(SYS_CPU_INFO, 0, 0, 0);
        for wn in windows.iter_mut() {
            if wn.dirty.is_empty() { continue; }
            let mut d = wn.dirty;
            wn.dirty = Rect::empty();
            d.x0 = d.x0.max(0); d.y0 = d.y0.max(0);
            d.x1 = d.x1.min(wn.canvas.width); d.y1 = d.y1.min(wn.canvas.height);
            if d.x0 >= d.x1 || d.y0 >= d.y1 { continue; }
            let area = (d.x1 - d.x0) as u64 * (d.y1 - d.y0) as u64;
            GUI_DIRTY_PIXELS.fetch_add(area, Ordering::Relaxed);
            if d.x0 == 0 && d.y0 == 0 && d.x1 == wn.canvas.width && d.y1 == wn.canvas.height {
                GUI_FULL_REDRAWS.fetch_add(1, Ordering::Relaxed);
            } else {
                GUI_PARTIAL_REDRAWS.fetch_add(1, Ordering::Relaxed);
            }
            let g = wn.geo;
            {
                let Window { canvas, content, .. } = &mut *wn;
                canvas.set_clip(d);
                match &*content {
                    Content::Launcher(l) => draw_launcher_body(canvas, l.pressed),
                    Content::External(_) => {}
                }
            }
            dr.add(g.x + BORDER + d.x0, g.y + TITLE_H + d.y0, d.x1 - d.x0, d.y1 - d.y0);
        }

        GUI_RENDER_TICKS.fetch_add(syscall(SYS_CPU_INFO, 0, 0, 0).saturating_sub(render_start), Ordering::Relaxed);

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
        GUI_FRAMES.fetch_add(1, Ordering::Relaxed);
        if composite_ran { GUI_COMPOSITES.fetch_add(1, Ordering::Relaxed); }
        if moved || composite_ran || !cur_shown {
            if cur_shown { s.restore_under(&cur_save, cur_x, cur_y, cursor_w, cursor_h); }
            if composite_ran {
                let composite_start = syscall(SYS_CPU_INFO, 0, 0, 0);
                for &region in dr.iter() {
                    composite(&mut s, &windows, focused_id, tb_page, uptime_secs, &bg, region);
                }
                GUI_COMPOSITE_TICKS.fetch_add(syscall(SYS_CPU_INFO, 0, 0, 0).saturating_sub(composite_start), Ordering::Relaxed);
            }
            let moved_cursor = !cur_shown || cur_x != mx || cur_y != my;
            s.save_under(&mut cur_save, mx, my, cursor_w, cursor_h);
            let fr = s.full_rect();
            s.set_clip(fr);
            draw_cursor(&mut s, mx, my);
            if cur_shown && moved_cursor {
                s.present(Rect { x0: cur_x, y0: cur_y, x1: cur_x + cursor_w, y1: cur_y + cursor_h });
            }
            let present_start = syscall(SYS_CPU_INFO, 0, 0, 0);
            s.present(Rect { x0: mx, y0: my, x1: mx + cursor_w, y1: my + cursor_h });
            GUI_PRESENT_TICKS.fetch_add(syscall(SYS_CPU_INFO, 0, 0, 0).saturating_sub(present_start), Ordering::Relaxed);
            cur_x = mx;
            cur_y = my;
            cur_shown = true;
        }

        // Cap near 60 Hz. Rendering is dirty-rect only, so an idle frame is cheap.
        syscall(SYS_SLEEP, 16, SLEEP_UNIT_MS, 0);
    }

    // Tear down any live shells before releasing the framebuffer.
    for wn in windows.iter_mut() {
        if let Content::External(client) = &mut wn.content {
            if !client.cleanup(true) { exit_code = 1; }
        }
    }
    syscall(SYS_IPC_CLOSE, control, 0, 0);
    syscall(SYS_IPC_CLOSE, ipc, 0, 0);
    syscall(SYS_FB_RELEASE, 0, 0, 0);
    sys_exit(exit_code);
}
