use crate::console;
use crate::drivers::keyboard::Key;
use crate::util::rdtsc;

const INPUT_CAP: usize = 128;

pub struct Line {
    buf: [u8; INPUT_CAP],
    len: usize,
}

impl Line {
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

pub struct Tty {
    input: [u8; INPUT_CAP],
    len: usize,
    cursor: usize,
    prompt_start: console::CursorPosition,
    cursor_visible: bool,
    next_blink: u64,
    tsc_per_ms: u64,
}

impl Tty {
    pub fn new(tsc_per_ms: u64) -> Self {
        Self {
            input: [0; INPUT_CAP],
            len: 0,
            cursor: 0,
            prompt_start: console::position(),
            cursor_visible: false,
            next_blink: 0,
            tsc_per_ms,
        }
    }

    pub fn prompt(&mut self, prompt: &str) {
        crate::print!("{}", prompt);
        self.prompt_start = console::position();
        self.len = 0;
        self.cursor = 0;
        self.cursor_visible = false;
        self.next_blink = rdtsc() + self.tsc_per_ms * 500;
    }

    pub fn handle_key(&mut self, key: Key) -> Option<Line> {
        self.hide_cursor();
        let line = match key {
            Key::Char('\n') => self.submit(),
            Key::Char('\x08') => {
                self.backspace();
                None
            }
            Key::Char(c) if !c.is_control() => {
                self.push(c);
                None
            }
            Key::Left => {
                self.move_left();
                None
            }
            Key::Right => {
                self.move_right();
                None
            }
            _ => None,
        };
        if line.is_none() {
            self.show_cursor();
        }
        line
    }

    pub fn update_cursor(&mut self) {
        if rdtsc() >= self.next_blink {
            self.cursor_visible = !self.cursor_visible;
            console::draw_cursor(self.cursor_position(), self.cursor_visible);
            self.next_blink = rdtsc() + self.tsc_per_ms * 500;
        }
    }

    fn show_cursor(&mut self) {
        self.cursor_visible = true;
        console::draw_cursor(self.cursor_position(), true);
        self.next_blink = rdtsc() + self.tsc_per_ms * 500;
    }

    fn hide_cursor(&mut self) {
        if self.cursor_visible {
            console::draw_cursor(self.cursor_position(), false);
            self.cursor_visible = false;
        }
    }

    fn cursor_position(&self) -> console::CursorPosition {
        let mut position = self.prompt_start;
        position.x += console::text_width(&self.input[..self.cursor]);
        position
    }

    fn push(&mut self, ch: char) {
        if self.len < INPUT_CAP - 1 && ch.is_ascii() {
            for i in (self.cursor..self.len).rev() {
                self.input[i + 1] = self.input[i];
            }
            self.input[self.cursor] = ch as u8;
            self.len += 1;
            self.cursor += 1;
            self.redraw_input();
        }
    }

    fn backspace(&mut self) {
        if self.cursor > 0 {
            for i in self.cursor..self.len {
                self.input[i - 1] = self.input[i];
            }
            self.len -= 1;
            self.cursor -= 1;
            self.redraw_input();
        }
    }

    fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_right(&mut self) {
        if self.cursor < self.len {
            self.cursor += 1;
        }
    }

    fn redraw_input(&self) {
        let width = console::text_width(&self.input[..self.len]) as usize + 32;
        let top = (self.prompt_start.y - (self.prompt_start.height - console::font_descent())).max(0.0) as usize;
        let clear_h = self.prompt_start.height as usize + console::font_descent() as usize + 2;
        console::clear_rect(
            self.prompt_start.x as usize,
            top,
            width.max(64),
            clear_h,
        );
        console::set_position(self.prompt_start);
        if let Ok(text) = core::str::from_utf8(&self.input[..self.len]) {
            crate::print!("{}", text);
        }
        console::set_position(self.cursor_position());
    }

    fn submit(&mut self) -> Option<Line> {
        console::set_position({
            let mut position = self.prompt_start;
            position.x += console::text_width(&self.input[..self.len]);
            position
        });
        crate::println!("");
        let mut line = Line {
            buf: [0; INPUT_CAP],
            len: self.len,
        };
        line.buf[..self.len].copy_from_slice(&self.input[..self.len]);
        self.len = 0;
        self.cursor = 0;
        self.cursor_visible = false;
        Some(line)
    }
}
