pub const GUI_PROTOCOL_MAGIC: u32 = u32::from_le_bytes(*b"KGUI");
pub const GUI_PROTOCOL_VERSION: u16 = 2;
pub const GUI_MESSAGE_SIZE: usize = 64;
pub const GUI_BUFFER_COUNT: usize = 2;
pub const GUI_TITLE_MAX: usize = 32;
pub const GUI_PIXEL_FORMAT_RGBX8888: u32 = 0;
pub const GUI_PIXEL_FORMAT_BGRX8888: u32 = 1;

pub const GUI_MSG_CONNECT: u16 = 1;
pub const GUI_MSG_INIT: u16 = 2;
pub const GUI_MSG_COMMIT: u16 = 3;
pub const GUI_MSG_BUFFER_RELEASE: u16 = 4;
pub const GUI_MSG_MOUSE: u16 = 5;
pub const GUI_MSG_KEY: u16 = 6;
pub const GUI_MSG_FOCUS: u16 = 7;
pub const GUI_MSG_CLOSE_REQUEST: u16 = 8;
pub const GUI_MSG_DESTROY: u16 = 9;
pub const GUI_MSG_STATS_REQUEST: u16 = 10;
pub const GUI_MSG_STATS_RESPONSE: u16 = 11;

#[derive(Clone, Copy)]
pub struct GuiMessage {
    bytes: [u8; GUI_MESSAGE_SIZE],
}

impl GuiMessage {
    fn new(kind: u16) -> Self {
        let mut message = Self { bytes: [0; GUI_MESSAGE_SIZE] };
        message.put_u32(0, GUI_PROTOCOL_MAGIC);
        message.put_u16(4, GUI_PROTOCOL_VERSION);
        message.put_u16(6, kind);
        message
    }

    pub fn connect(width: u32, height: u32, title: &[u8]) -> Option<Self> {
        if title.is_empty() || title.len() > GUI_TITLE_MAX || title.iter().any(|byte| !(0x20..=0x7e).contains(byte)) {
            return None;
        }
        let mut message = Self::new(GUI_MSG_CONNECT);
        message.put_u32(8, width);
        message.put_u32(12, height);
        message.bytes[16] = title.len() as u8;
        message.bytes[17..17 + title.len()].copy_from_slice(title);
        Some(message)
    }

    pub fn init(width: u32, height: u32, stride: u32, format: u32, buffers: [u64; 2]) -> Self {
        let mut message = Self::new(GUI_MSG_INIT);
        message.put_u32(8, width);
        message.put_u32(12, height);
        message.put_u32(16, stride);
        message.put_u32(20, format);
        message.put_u64(24, buffers[0]);
        message.put_u64(32, buffers[1]);
        message
    }

    pub fn commit(buffer: u8, x: u32, y: u32, width: u32, height: u32) -> Self {
        let mut message = Self::new(GUI_MSG_COMMIT);
        message.bytes[8] = buffer;
        message.put_u32(12, x);
        message.put_u32(16, y);
        message.put_u32(20, width);
        message.put_u32(24, height);
        message
    }

    pub fn buffer_release(buffer: u8) -> Self {
        let mut message = Self::new(GUI_MSG_BUFFER_RELEASE);
        message.bytes[8] = buffer;
        message
    }

    pub fn mouse(x: i32, y: i32, buttons: u32, changed: u32) -> Self {
        let mut message = Self::new(GUI_MSG_MOUSE);
        message.put_u32(8, x as u32);
        message.put_u32(12, y as u32);
        message.put_u32(16, buttons);
        message.put_u32(20, changed);
        message
    }

    pub fn key(code: u8, released: bool) -> Self {
        let mut message = Self::new(GUI_MSG_KEY);
        message.bytes[8] = code;
        message.bytes[9] = released as u8;
        message
    }

    pub fn focus(focused: bool) -> Self {
        let mut message = Self::new(GUI_MSG_FOCUS);
        message.bytes[8] = focused as u8;
        message
    }

    pub fn close_request() -> Self { Self::new(GUI_MSG_CLOSE_REQUEST) }
    pub fn destroy() -> Self { Self::new(GUI_MSG_DESTROY) }
    pub fn stats_request() -> Self { Self::new(GUI_MSG_STATS_REQUEST) }

    pub fn stats_response(frames: u64, composites: u64, dirty_pixels: u64,
                          render_ticks: u64, composite_ticks: u64, present_ticks: u64,
                          ticks_per_sec: u64) -> Self {
        let mut message = Self::new(GUI_MSG_STATS_RESPONSE);
        message.put_u64(8, frames);
        message.put_u64(16, composites);
        message.put_u64(24, dirty_pixels);
        message.put_u64(32, render_ticks);
        message.put_u64(40, composite_ticks);
        message.put_u64(48, present_ticks);
        message.put_u64(56, ticks_per_sec);
        message
    }

    pub fn from_bytes(bytes: [u8; GUI_MESSAGE_SIZE]) -> Option<Self> {
        let message = Self { bytes };
        if message.u32(0) != GUI_PROTOCOL_MAGIC || message.u16(4) != GUI_PROTOCOL_VERSION { return None; }
        match message.kind() {
            GUI_MSG_CONNECT | GUI_MSG_INIT | GUI_MSG_COMMIT | GUI_MSG_BUFFER_RELEASE | GUI_MSG_MOUSE
            | GUI_MSG_KEY | GUI_MSG_FOCUS | GUI_MSG_CLOSE_REQUEST | GUI_MSG_DESTROY
            | GUI_MSG_STATS_REQUEST | GUI_MSG_STATS_RESPONSE => Some(message),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> &[u8; GUI_MESSAGE_SIZE] { &self.bytes }
    pub fn kind(&self) -> u16 { self.u16(6) }
    pub fn buffer(&self) -> u8 { self.bytes[8] }
    pub fn released(&self) -> bool { self.bytes[9] != 0 }
    pub fn flag(&self) -> bool { self.bytes[8] != 0 }
    pub fn x(&self) -> i32 { self.u32(8) as i32 }
    pub fn y(&self) -> i32 { self.u32(12) as i32 }
    pub fn buttons(&self) -> u32 { self.u32(16) }
    pub fn changed(&self) -> u32 { self.u32(20) }
    pub fn damage_x(&self) -> u32 { self.u32(12) }
    pub fn damage_y(&self) -> u32 { self.u32(16) }
    pub fn damage_width(&self) -> u32 { self.u32(20) }
    pub fn damage_height(&self) -> u32 { self.u32(24) }
    pub fn width(&self) -> u32 { self.u32(8) }
    pub fn height(&self) -> u32 { self.u32(12) }
    pub fn stride(&self) -> u32 { self.u32(16) }
    pub fn format(&self) -> u32 { self.u32(20) }
    pub fn shm_id(&self, index: usize) -> u64 { self.u64(24 + index * 8) }
    pub fn title(&self) -> Option<&[u8]> {
        let length = self.bytes[16] as usize;
        if length == 0 || length > GUI_TITLE_MAX { return None; }
        let title = &self.bytes[17..17 + length];
        if title.iter().any(|byte| !(0x20..=0x7e).contains(byte)) { None } else { Some(title) }
    }
    pub fn stat(&self, index: usize) -> u64 { self.u64(8 + index * 8) }

    fn u16(&self, offset: usize) -> u16 { u16::from_le_bytes([self.bytes[offset], self.bytes[offset + 1]]) }
    fn u32(&self, offset: usize) -> u32 { u32::from_le_bytes(self.bytes[offset..offset + 4].try_into().unwrap()) }
    fn u64(&self, offset: usize) -> u64 { u64::from_le_bytes(self.bytes[offset..offset + 8].try_into().unwrap()) }
    fn put_u16(&mut self, offset: usize, value: u16) { self.bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes()); }
    fn put_u32(&mut self, offset: usize, value: u32) { self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes()); }
    fn put_u64(&mut self, offset: usize, value: u64) { self.bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes()); }
}
