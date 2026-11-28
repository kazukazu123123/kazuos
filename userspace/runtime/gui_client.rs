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

/// A connection to the compositor. One connection can own any number of
/// windows; messages for all of them arrive on this single channel, tagged
/// with the window id the compositor assigned (GuiMessage::window_id).
pub struct GuiConnection {
    channel: u64,
    compositor: u64,
    // Messages received while polling a different window (or waiting for an
    // INIT), kept until the window they belong to polls.
    pending: alloc::vec::Vec<GuiMessage>,
}

impl GuiConnection {
    pub fn connect() -> Option<Self> {
        let compositor = find_gui_compositor()?;
        let channel = sys_ipc_open(GUI_CHANNEL);
        if channel == u64::MAX { return None; }
        Some(Self { channel, compositor, pending: alloc::vec::Vec::new() })
    }

    pub fn create_window(&mut self, title: &[u8], width: u32, height: u32) -> Option<GuiWindow> {
        let message = GuiMessage::connect(width, height, title)?;
        let mut connected = false;
        for _ in 0..GUI_CONNECT_ATTEMPTS {
            match gui_send(self.channel, self.compositor, message) {
                result if result == GUI_MESSAGE_SIZE as u64 => { connected = true; break; }
                0 => sys_sleep(10),
                _ if gui_process_alive(self.compositor) => sys_sleep(10),
                _ => break,
            }
        }
        if !connected { return None; }
        // Wait for the INIT answering this request. Anything else that
        // arrives meanwhile belongs to another window: stash it.
        let mut init = None;
        for _ in 0..GUI_INIT_ATTEMPTS {
            if let Some(index) = self.pending.iter().position(|m| m.kind() == GUI_MSG_INIT) {
                init = Some(self.pending.remove(index));
                break;
            }
            let mut envelope = [0u8; IPC_MAX_ENVELOPE_SIZE];
            match gui_receive(self.channel, &mut envelope) {
                None => sys_sleep(8),
                Some((sender, message)) if sender == self.compositor && message.kind() == GUI_MSG_INIT => {
                    init = Some(message);
                    break;
                }
                Some((sender, message)) if sender == self.compositor => { self.pending.push(message); }
                _ => {}
            }
        }
        let Some(init) = init else { return None; };
        let window_id = init.window_id();
        let width = init.width();
        let height = init.height();
        let stride = init.stride();
        let format = init.format();
        if width == 0 || height == 0 || stride != width || width > 1024 || height > 768
            || width.checked_mul(height).and_then(|pixels| pixels.checked_mul(4)).is_none()
            || (format != GUI_PIXEL_FORMAT_RGBX8888 && format != GUI_PIXEL_FORMAT_BGRX8888) {
            self.send_destroy(window_id);
            return None;
        }
        let ids = [init.shm_id(0), init.shm_id(1)];
        if ids[0] == ids[1] {
            self.send_destroy(window_id);
            return None;
        }
        let mut addresses = [u64::MAX; GUI_BUFFER_COUNT];
        for index in 0..GUI_BUFFER_COUNT {
            addresses[index] = sys_shm_map(ids[index]);
            if addresses[index] == u64::MAX {
                self.send_destroy(window_id);
                for id in ids { if id != u64::MAX { let _ = sys_shm_close(id); } }
                return None;
            }
        }
        Some(GuiWindow { id: window_id, ids, addresses, available: [true; GUI_BUFFER_COUNT],
                         next: 0, width, height, stride, format })
    }

    fn send_destroy(&self, window_id: u64) {
        let mut message = GuiMessage::destroy();
        message.set_window_id(window_id);
        let _ = gui_send(self.channel, self.compositor, message);
    }

    pub fn request_stats(&self) -> bool {
        gui_send(self.channel, self.compositor, GuiMessage::stats_request()) == GUI_MESSAGE_SIZE as u64
    }

    pub fn close(self) {
        let _ = sys_ipc_close(self.channel);
    }
}

/// One window owned by a GuiConnection. Drawing state (the SHM buffer pair)
/// lives here; the transport lives in the connection, so every operation
/// that talks to the compositor takes it as a parameter.
pub struct GuiWindow {
    id: u64,
    ids: [u64; GUI_BUFFER_COUNT],
    addresses: [u64; GUI_BUFFER_COUNT],
    available: [bool; GUI_BUFFER_COUNT],
    next: usize,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
}

impl GuiWindow {
    pub fn id(&self) -> u64 { self.id }
    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn stride(&self) -> u32 { self.stride }
    pub fn format(&self) -> u32 { self.format }

    pub fn acquire(&mut self) -> Option<(usize, u64)> {
        let index = if self.available[self.next] { self.next }
            else if self.available[self.next ^ 1] { self.next ^ 1 } else { return None; };
        Some((index, self.addresses[index]))
    }

    pub fn commit(&mut self, conn: &GuiConnection, buffer: usize, x: u32, y: u32, width: u32, height: u32) -> bool {
        if buffer >= GUI_BUFFER_COUNT || !self.available[buffer] { return false; }
        // Validate the damage rect here: the compositor rejects invalid damage
        // without ever releasing the buffer, which would strand it for good.
        if width == 0 || height == 0
            || x.checked_add(width).map_or(true, |end| end > self.width)
            || y.checked_add(height).map_or(true, |end| end > self.height) {
            return false;
        }
        fence(Ordering::Release);
        let mut message = GuiMessage::commit(buffer as u8, x, y, width, height);
        message.set_window_id(self.id);
        if gui_send(conn.channel, conn.compositor, message) != GUI_MESSAGE_SIZE as u64 {
            return false;
        }
        self.available[buffer] = false;
        self.next = buffer ^ 1;
        true
    }

    pub fn poll(&mut self, conn: &mut GuiConnection) -> Option<GuiEvent> {
        // Stashed messages first: another window's poll may have received
        // ours (and vice versa, which is what the stash is for).
        let mut index = 0;
        while index < conn.pending.len() {
            let message = conn.pending[index];
            if message.window_id() == self.id || message.window_id() == 0 {
                conn.pending.remove(index);
                if let Some(event) = self.handle(message) { return Some(event); }
            } else {
                index += 1;
            }
        }
        let mut envelope = [0u8; IPC_MAX_ENVELOPE_SIZE];
        loop {
            let (sender, message) = gui_receive(conn.channel, &mut envelope)?;
            if sender != conn.compositor { continue; }
            if message.window_id() != self.id && message.window_id() != 0 {
                // Another window's message: stash it for that window's poll
                // and report nothing this call; the caller's loop re-polls.
                conn.pending.push(message);
                return None;
            }
            if let Some(event) = self.handle(message) { return Some(event); }
        }
    }

    // Apply a message addressed to this window (or connection-scoped, id 0).
    // Buffer releases are consumed here; real events are returned.
    fn handle(&mut self, message: GuiMessage) -> Option<GuiEvent> {
        match message.kind() {
            GUI_MSG_BUFFER_RELEASE if (message.buffer() as usize) < GUI_BUFFER_COUNT => {
                fence(Ordering::Acquire);
                self.available[message.buffer() as usize] = true;
                None
            }
            GUI_MSG_MOUSE => Some(GuiEvent::Mouse { x: message.x(), y: message.y(), buttons: message.buttons(), changed: message.changed() }),
            GUI_MSG_KEY => Some(GuiEvent::Key { code: message.buffer(), released: message.released() }),
            GUI_MSG_FOCUS => Some(GuiEvent::Focus(message.flag())),
            GUI_MSG_CLOSE_REQUEST => Some(GuiEvent::Close),
            GUI_MSG_STATS_RESPONSE => Some(GuiEvent::Stats([
                message.stat(0), message.stat(1), message.stat(2), message.stat(3),
                message.stat(4), message.stat(5), message.stat(6),
            ])),
            _ => None,
        }
    }

    pub fn close(mut self, conn: &GuiConnection) {
        for _ in 0..100 {
            let mut message = GuiMessage::destroy();
            message.set_window_id(self.id);
            match gui_send(conn.channel, conn.compositor, message) {
                result if result == GUI_MESSAGE_SIZE as u64 => break,
                0 => sys_sleep(5),
                _ => break,
            }
        }
        for id in self.ids.iter_mut() {
            if *id != u64::MAX { let _ = sys_shm_close(*id); *id = u64::MAX; }
        }
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
    sys_proc_info(pid, &mut info as *mut ProcessInfo as *mut u64) == 0 && info.state != 4
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
