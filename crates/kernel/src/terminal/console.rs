use core::fmt::{self, Write};

use ab_glyph::{Font, FontArc, Point, ScaleFont};
use alloc::vec::Vec;

use crate::drivers::framebuffer::FramebufferInfo;
use crate::terminal::bitmap;
use crate::util::{IrqGuard, SpinLock, SyncUnsafeCell};

#[derive(Clone, Copy, PartialEq)]
enum EscapeState {
    Normal,
    Esc,
    Csi,
}

pub struct Console {
    fb: FramebufferInfo,
    font: Option<FontArc>,
    font_size: f32,
    cursor_x: f32,
    cursor_y: f32,
    line_height: f32,
    font_descent: f32,
    text_color: (u8, u8, u8),
    escape_state: EscapeState,
    csi_params: [u16; 4],
    csi_param_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct CursorPosition {
    pub x: f32,
    pub y: f32,
    pub height: f32,
}

impl Console {
    pub fn new(fb: FramebufferInfo, font_data: Vec<u8>) -> Self {
        let font = FontArc::try_from_vec(font_data).ok();
        let (line_height, font_descent) = if let Some(ref font) = font {
            let scaled = font.as_scaled(24.0);
            (scaled.height() + scaled.line_gap(), scaled.descent())
        } else {
            (bitmap::CHAR_HEIGHT as f32, (bitmap::CHAR_HEIGHT - bitmap::ASCENT) as f32)
        };

        Self {
            fb,
            font,
            font_size: 24.0,
            cursor_x: 0.0,
            cursor_y: line_height,
            line_height,
            font_descent,
            text_color: (255, 255, 255),
            escape_state: EscapeState::Normal,
            csi_params: [0; 4],
            csi_param_count: 0,
        }
    }

    pub fn clear(&mut self) {
        unsafe {
            self.fb.clear(0, 0, 0);
        }
        self.cursor_x = 0.0;
        self.cursor_y = self.line_height;
    }

    pub fn println(&mut self, text: &str) {
        self.print(text);
        self.newline();
    }

    pub fn print(&mut self, text: &str) {
        for c in text.chars() {
            self.process_char(c);
        }
    }

    fn process_char(&mut self, c: char) {
        match self.escape_state {
            EscapeState::Normal => match c {
                '\n' => self.newline(),
                '\r' => self.cursor_x = 0.0,
                '\x08' => {
                    let w = if self.font.is_some() {
                        self.char_width(' ')
                    } else {
                        bitmap::CHAR_WIDTH as f32
                    };
                    self.cursor_x = (self.cursor_x - w).max(0.0);
                }
                '\x1b' => self.escape_state = EscapeState::Esc,
                _ => self.draw_char(c),
            },
            EscapeState::Esc => {
                if c == '[' {
                    self.csi_params = [0; 4];
                    self.csi_param_count = 0;
                    self.escape_state = EscapeState::Csi;
                } else {
                    self.escape_state = EscapeState::Normal;
                }
            }
            EscapeState::Csi => {
                if c.is_ascii_digit() {
                    let i = self.csi_param_count.min(3);
                    self.csi_params[i] = self.csi_params[i]
                        .saturating_mul(10)
                        .saturating_add(c as u16 - b'0' as u16);
                } else if c == ';' {
                    if self.csi_param_count < 3 {
                        self.csi_param_count += 1;
                    }
                } else {
                    // finalise last param
                    self.csi_param_count += 1;
                    self.csi_dispatch(c);
                    self.escape_state = EscapeState::Normal;
                }
            }
        }
    }

    fn csi_dispatch(&mut self, cmd: char) {
        let p0 = self.csi_params[0] as f32;
        let p1 = self.csi_params[1] as f32;
        match cmd {
            // \x1b[2J — clear screen,  \x1b[J or \x1b[0J — clear to end of screen
            'J' => {
                let mode = self.csi_params[0];
                if mode == 2 || mode == 3 {
                    unsafe { self.fb.clear(0, 0, 0); }
                    self.cursor_x = 0.0;
                    self.cursor_y = self.line_height;
                } else if mode == 1 {
                    // clear from top to cursor
                    let ascent = self.line_height - self.font_descent;
                    let bottom = (self.cursor_y - ascent).max(0.0) as usize;
                    self.clear_rect(0, 0, self.fb.width, bottom);
                } else {
                    // 0 or default: clear from cursor to end
                    let ascent = self.line_height - self.font_descent;
                    let top = (self.cursor_y - ascent).max(0.0) as usize;
                    self.clear_rect(0, top, self.fb.width, self.fb.height - top);
                }
            }
            // \x1b[K — erase in line
            'K' => self.erase_to_end_of_line(),
            // \x1b[r;cH or \x1b[H — move cursor to row r, col c (1-based)
            'H' | 'f' => {
                let row = (p0.max(1.0) - 1.0) as usize;
                let col = (p1.max(1.0) - 1.0) as usize;
                let char_w = if self.font.is_some() {
                    self.char_width(' ')
                } else {
                    bitmap::CHAR_WIDTH as f32
                };
                self.cursor_x = col as f32 * char_w;
                self.cursor_y = self.line_height + row as f32 * self.line_height;
            }
            // \x1b[nA — cursor up
            'A' => {
                let n = p0.max(1.0);
                self.cursor_y = (self.cursor_y - n * self.line_height).max(self.line_height);
            }
            // \x1b[nB — cursor down
            'B' => {
                let n = p0.max(1.0);
                self.cursor_y = (self.cursor_y + n * self.line_height)
                    .min(self.fb.height as f32 - self.font_descent);
            }
            // \x1b[nC — cursor right
            'C' => {
                let n = p0.max(1.0);
                let char_w = if self.font.is_some() {
                    self.char_width(' ')
                } else {
                    bitmap::CHAR_WIDTH as f32
                };
                self.cursor_x = (self.cursor_x + n * char_w).min(self.fb.width as f32);
            }
            // \x1b[nD — cursor left
            'D' => {
                let n = p0.max(1.0);
                let char_w = if self.font.is_some() {
                    self.char_width(' ')
                } else {
                    bitmap::CHAR_WIDTH as f32
                };
                self.cursor_x = (self.cursor_x - n * char_w).max(0.0);
            }
            _ => {}
        }
    }

    fn erase_to_end_of_line(&self) {
        let x = self.cursor_x as usize;
        let ascent = self.line_height - self.font_descent;
        let top = (self.cursor_y - ascent).max(0.0) as usize;
        let bottom = (self.cursor_y as usize + self.font_descent as usize + 1).min(self.fb.height);
        let height = bottom.saturating_sub(top).max(1);
        if x < self.fb.width {
            self.clear_rect(x, top, self.fb.width - x, height);
        }
    }

    pub fn position(&self) -> CursorPosition {
        CursorPosition {
            x: self.cursor_x,
            y: self.cursor_y,
            height: self.line_height,
        }
    }

    pub fn set_position(&mut self, position: CursorPosition) {
        self.cursor_x = position.x;
        self.cursor_y = position.y;
    }

    pub fn line_descent(&self) -> f32 {
        self.font_descent
    }

    pub fn char_width(&self, c: char) -> f32 {
        if let Some(ref font) = self.font {
            let scaled = font.as_scaled(self.font_size);
            scaled.h_advance(scaled.glyph_id(c))
        } else {
            bitmap::CHAR_WIDTH as f32
        }
    }

    pub fn clear_rect(&self, x: usize, y: usize, width: usize, height: usize) {
        let max_y = (y + height).min(self.fb.height);
        let max_x = (x + width).min(self.fb.width);
        for py in y..max_y {
            for px in x..max_x {
                unsafe {
                    self.fb.write_pixel(px, py, 0, 0, 0);
                }
            }
        }
    }

    pub fn draw_cursor(&self, position: CursorPosition, visible: bool) {
        let x = position.x as usize;
        let ascent = position.height - self.font_descent;
        let top = (position.y - ascent).max(0.0) as usize;
        let height = position.height as usize;
        let color = if visible { 255 } else { 0 };
        for py in top..(top + height).min(self.fb.height) {
            for px in x..(x + 2).min(self.fb.width) {
                unsafe {
                    self.fb.write_pixel(px, py, color, color, color);
                }
            }
        }
    }

    fn newline(&mut self) {
        self.cursor_x = 0.0;
        self.cursor_y += self.line_height;
        let max_y = self.fb.height as f32;
        if self.cursor_y >= max_y {
            let scroll_px = self.line_height as usize + 1;
            unsafe {
                self.fb.scroll_up(scroll_px);
            }
            self.cursor_y -= self.line_height;
        }
    }

    fn draw_char(&mut self, c: char) {
        let advance = if self.font.is_some() {
            let scaled = self.font.as_ref().unwrap().as_scaled(self.font_size);
            scaled.h_advance(scaled.glyph_id(c))
        } else {
            bitmap::CHAR_WIDTH as f32
        };

        if self.cursor_x + advance >= self.fb.width as f32 {
            self.newline();
        }
        if self.cursor_y + self.font_descent >= self.fb.height as f32 {
            self.newline();
        }

        if let Some(ref font) = self.font {
            let scaled = font.as_scaled(self.font_size);
            let glyph_id = scaled.glyph_id(c);
            let glyph = glyph_id.with_scale_and_position(
                self.font_size,
                Point {
                    x: self.cursor_x,
                    y: self.cursor_y,
                },
            );
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                let (r, g, b) = self.text_color;
                outlined.draw(|offset_x, offset_y, coverage| {
                    let px = bounds.min.x as i32 + offset_x as i32;
                    let py = bounds.min.y as i32 + offset_y as i32;
                    if px < 0 || py < 0 { return; }
                    let alpha = (coverage * 255.0) as u8;
                    unsafe {
                        self.fb.blend_pixel(px as usize, py as usize, r, g, b, alpha);
                    }
                });
            }
        } else {
            let (r, g, b) = self.text_color;
            bitmap::draw_glyph(
                &self.fb,
                c,
                self.cursor_x as usize,
                self.cursor_y as usize,
                r,
                g,
                b,
            );
        }

        self.cursor_x += advance;
    }
}

impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.print(text);
        Ok(())
    }
}

static CONSOLE: SyncUnsafeCell<Option<Console>> = SyncUnsafeCell::new(None);
static SAVED_CURSOR: SyncUnsafeCell<CursorPosition> =
    SyncUnsafeCell::new(CursorPosition { x: 0.0, y: 0.0, height: 0.0 });
static CONSOLE_LOCK: SpinLock = SpinLock::new();

struct ConsoleGuard {
    _irq: IrqGuard,
}

impl ConsoleGuard {
    fn new() -> Self {
        // Interrupts off before locking: prevents a same-CPU IRQ from re-entering
        // and self-deadlocking on this non-reentrant lock (see KeyboardGuard).
        let _irq = IrqGuard::new();
        CONSOLE_LOCK.lock();
        Self { _irq }
    }
}

impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        CONSOLE_LOCK.unlock();
    }
}

pub fn init(console: Console) {
    let _guard = ConsoleGuard::new();
    unsafe {
        *CONSOLE.0.get() = Some(console);
    }
}

pub fn clear() {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            console.clear();
        }
    }
}

pub fn screen_print(text: &str) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            // Auto-cursor: erase the old caret, print, then draw a caret at the new
            // position. This makes the console a self-cursoring terminal, so a shell
            // line editor only needs to emit text + ANSI (no cursor syscalls) and works
            // the same here as in the GUI terminal.
            console.draw_cursor(*SAVED_CURSOR.0.get(), false);
            console.print(text);
            let pos = console.position();
            *SAVED_CURSOR.0.get() = pos;
            console.draw_cursor(pos, true);
        }
    }
}

pub fn screen_println(text: &str) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            console.println(text);
        }
    }
}

pub fn position() -> CursorPosition {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            return console.position();
        }
    }
    CursorPosition {
        x: 0.0,
        y: 0.0,
        height: 0.0,
    }
}

pub fn set_position(position: CursorPosition) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            console.set_position(position);
        }
    }
}

pub fn font_descent() -> f32 {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &*CONSOLE.0.get() {
            return console.line_descent();
        }
    }
    0.0
}

/// Returns (cols, rows) — number of character columns and rows on screen.
pub fn console_size() -> (u32, u32) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &*CONSOLE.0.get() {
            let cw = console.char_width(' ');
            let lh = console.line_height;
            if cw > 0.0 && lh > 0.0 {
                let cols = (console.fb.width as f32 / cw) as u32;
                let rows = (console.fb.height as f32 / lh) as u32;
                return (cols, rows);
            }
        }
    }
    (80, 25)
}

pub fn text_width(text: &[u8]) -> f32 {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            let mut width = 0.0;
            for &byte in text {
                width += console.char_width(byte as char);
            }
            return width;
        }
    }
    0.0
}

pub fn clear_rect(x: usize, y: usize, width: usize, height: usize) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            console.clear_rect(x, y, width, height);
        }
    }
}

pub fn draw_cursor(position: CursorPosition, visible: bool) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            console.draw_cursor(position, visible);
        }
    }
}

pub fn save_cursor_pos() {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            // Erase old cursor block before saving new position so stale
            // blocks don't persist when cursor_y changes between redraws.
            console.draw_cursor(*SAVED_CURSOR.0.get(), false);
            *SAVED_CURSOR.0.get() = console.position();
        }
    }
}

pub fn draw_saved_cursor(visible: bool) {
    let _guard = ConsoleGuard::new();
    unsafe {
        let pos = *SAVED_CURSOR.0.get();
        if let Some(console) = &*CONSOLE.0.get() {
            console.draw_cursor(pos, visible);
        }
    }
}

pub fn restore_cursor_pos() {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            console.set_position(*SAVED_CURSOR.0.get());
        }
    }
}

pub struct FbParams {
    pub base: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32, // 0=RGB, 1=BGR
}

pub fn fb_params() -> Option<FbParams> {
    let _guard = ConsoleGuard::new();
    unsafe {
        (*CONSOLE.0.get()).as_ref().map(|c| FbParams {
            base: c.fb.base as u64,
            width: c.fb.width as u32,
            height: c.fb.height as u32,
            stride: c.fb.stride as u32,
            format: match c.fb.pixel_format {
                crate::drivers::framebuffer::PixelFormat::Rgb => 0,
                crate::drivers::framebuffer::PixelFormat::Bgr => 1,
            },
        })
    }
}

pub fn _print(args: fmt::Arguments) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            let _ = console.write_fmt(args);
        }
    }
}

pub fn _println(args: fmt::Arguments) {
    let _guard = ConsoleGuard::new();
    unsafe {
        if let Some(console) = &mut *CONSOLE.0.get() {
            let _ = console.write_fmt(args);
            console.println("");
        }
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(core::format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        if $crate::init::is_verbose() {
            $crate::println!($($arg)*);
        }
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::console::_println(core::format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::console::_println(core::format_args!($($arg)*))
    };
}
