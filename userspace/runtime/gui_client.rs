use core::sync::atomic::{fence, Ordering};

pub const GUI_CHANNEL: &[u8] = b"gui-control";
const GUI_CONNECT_ATTEMPTS: usize = 100;
const GUI_INIT_ATTEMPTS: usize = 250;

#[derive(Clone, Copy)]
pub enum GuiEvent {
    Mouse { x: i32, y: i32, buttons: u32, changed: u32 },
    Key { code: u8, released: bool },
    Focus(bool),
    Close,
    Stats([u64; 7]),
}

pub struct GuiClient {
    channel: u64,
    compositor: u64,
    ids: [u64; GUI_BUFFER_COUNT],
    addresses: [u64; GUI_BUFFER_COUNT],
    available: [bool; GUI_BUFFER_COUNT],
    next: usize,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
}

impl GuiClient {
    pub fn connect(title: &[u8], width: u32, height: u32) -> Option<Self> {
        let message = GuiMessage::connect(width, height, title)?;
        let compositor = find_gui_compositor()?;
        let channel = sys_ipc_open(GUI_CHANNEL);
        if channel == u64::MAX { return None; }
        let mut connected = false;
        for _ in 0..GUI_CONNECT_ATTEMPTS {
            match gui_send(channel, compositor, message) {
                result if result == GUI_MESSAGE_SIZE as u64 => { connected = true; break; }
                0 => sys_sleep(10),
                _ if gui_process_alive(compositor) => sys_sleep(10),
                _ => break,
            }
        }
        if !connected { let _ = sys_ipc_close(channel); return None; }
        let mut envelope = [0u8; IPC_MAX_ENVELOPE_SIZE];
        let mut init = None;
        for _ in 0..GUI_INIT_ATTEMPTS {
            match gui_receive(channel, &mut envelope) {
                None => sys_sleep(8),
                Some((sender, message)) if sender == compositor && message.kind() == GUI_MSG_INIT => {
                    init = Some(message);
                    break;
                }
                _ => {}
            }
        }
        let Some(init) = init else { let _ = sys_ipc_close(channel); return None; };
        let width = init.width();
        let height = init.height();
        let stride = init.stride();
        let format = init.format();
        if width == 0 || height == 0 || stride != width || width > 1024 || height > 768
            || width.checked_mul(height).and_then(|pixels| pixels.checked_mul(4)).is_none()
            || (format != GUI_PIXEL_FORMAT_RGBX8888 && format != GUI_PIXEL_FORMAT_BGRX8888) {
            let _ = gui_send(channel, compositor, GuiMessage::destroy());
            let _ = sys_ipc_close(channel);
            return None;
        }
        let ids = [init.shm_id(0), init.shm_id(1)];
        let mut addresses = [u64::MAX; GUI_BUFFER_COUNT];
        for index in 0..GUI_BUFFER_COUNT {
            addresses[index] = sys_shm_map(ids[index]);
            if addresses[index] == u64::MAX {
                let _ = gui_send(channel, compositor, GuiMessage::destroy());
                for id in ids { if id != u64::MAX { let _ = sys_shm_close(id); } }
                let _ = sys_ipc_close(channel);
                return None;
            }
        }
        Some(Self { channel, compositor, ids, addresses, available: [true; GUI_BUFFER_COUNT],
                    next: 0, width, height, stride, format })
    }

    pub fn acquire(&mut self) -> Option<(usize, u64)> {
        let index = if self.available[self.next] { self.next }
            else if self.available[self.next ^ 1] { self.next ^ 1 } else { return None; };
        Some((index, self.addresses[index]))
    }

    pub fn commit(&mut self, buffer: usize, x: u32, y: u32, width: u32, height: u32) -> bool {
        if buffer >= GUI_BUFFER_COUNT || !self.available[buffer] { return false; }
        fence(Ordering::Release);
        if gui_send(self.channel, self.compositor,
                    GuiMessage::commit(buffer as u8, x, y, width, height)) != GUI_MESSAGE_SIZE as u64 {
            return false;
        }
        self.available[buffer] = false;
        self.next = buffer ^ 1;
        true
    }

    pub fn poll(&mut self) -> Option<GuiEvent> {
        let mut envelope = [0u8; IPC_MAX_ENVELOPE_SIZE];
        loop {
            let (sender, message) = gui_receive(self.channel, &mut envelope)?;
            if sender != self.compositor { continue; }
            match message.kind() {
                GUI_MSG_BUFFER_RELEASE if (message.buffer() as usize) < GUI_BUFFER_COUNT => {
                    fence(Ordering::Acquire);
                    self.available[message.buffer() as usize] = true;
                }
                GUI_MSG_MOUSE => return Some(GuiEvent::Mouse { x: message.x(), y: message.y(), buttons: message.buttons(), changed: message.changed() }),
                GUI_MSG_KEY => return Some(GuiEvent::Key { code: message.buffer(), released: message.released() }),
                GUI_MSG_FOCUS => return Some(GuiEvent::Focus(message.flag())),
                GUI_MSG_CLOSE_REQUEST => return Some(GuiEvent::Close),
                GUI_MSG_STATS_RESPONSE => return Some(GuiEvent::Stats([
                    message.stat(0), message.stat(1), message.stat(2), message.stat(3),
                    message.stat(4), message.stat(5), message.stat(6),
                ])),
                _ => {}
            }
        }
    }

    pub fn request_stats(&self) -> bool {
        gui_send(self.channel, self.compositor, GuiMessage::stats_request()) == GUI_MESSAGE_SIZE as u64
    }

    pub fn close(mut self) {
        for _ in 0..100 {
            match gui_send(self.channel, self.compositor, GuiMessage::destroy()) {
                result if result == GUI_MESSAGE_SIZE as u64 => break,
                0 => sys_sleep(5),
                _ => break,
            }
        }
        for id in self.ids.iter_mut() {
            if *id != u64::MAX { let _ = sys_shm_close(*id); *id = u64::MAX; }
        }
        let _ = sys_ipc_close(self.channel);
    }
}

fn find_gui_compositor() -> Option<u64> {
    let mut previous = 0u64;
    loop {
        let pid = sys_proc_next(previous);
        if pid == u64::MAX { return None; }
        previous = pid;
        let mut info = ProcessInfo::ZERO;
        if sys_proc_info(pid, &mut info as *mut ProcessInfo as *mut u64) != 0 { continue; }
        let length = info.image_name.iter().position(|byte| *byte == 0).unwrap_or(PROC_NAME_LEN);
        if &info.image_name[..length] == b"/bin/gui.kxe" { return Some(pid); }
    }
}

fn gui_process_alive(pid: u64) -> bool {
    let mut info = ProcessInfo::ZERO;
    sys_proc_info(pid, &mut info as *mut ProcessInfo as *mut u64) == 0
}

fn gui_send(channel: u64, target: u64, message: GuiMessage) -> u64 {
    let mut envelope = [0u8; IPC_PID_PREFIX_SIZE + GUI_MESSAGE_SIZE];
    envelope[..IPC_PID_PREFIX_SIZE].copy_from_slice(&target.to_le_bytes());
    envelope[IPC_PID_PREFIX_SIZE..].copy_from_slice(message.as_bytes());
    sys_ipc_try_send_to_envelope(channel, &envelope)
}

fn gui_receive(channel: u64, envelope: &mut [u8; IPC_MAX_ENVELOPE_SIZE]) -> Option<(u64, GuiMessage)> {
    let result = sys_ipc_try_recv_from_envelope(channel, envelope);
    if result == 0 { return None; }
    if result != (IPC_PID_PREFIX_SIZE + GUI_MESSAGE_SIZE) as u64 { return None; }
    let sender = u64::from_le_bytes(envelope[..IPC_PID_PREFIX_SIZE].try_into().ok()?);
    let mut bytes = [0u8; GUI_MESSAGE_SIZE];
    bytes.copy_from_slice(&envelope[IPC_PID_PREFIX_SIZE..IPC_PID_PREFIX_SIZE + GUI_MESSAGE_SIZE]);
    GuiMessage::from_bytes(bytes).map(|message| (sender, message))
}
