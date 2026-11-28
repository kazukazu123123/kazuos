#![no_std]
#![no_main]
include!("../runtime/runtime.rs");
include!("../runtime/gui_protocol.rs");
include!("../runtime/gui_client.rs");
include!("../runtime/gui_ui.rs");

// Per-window demo state: one process drives two windows over a single
// GuiConnection, and each window keeps its own input state and buffers.
struct DemoWindow {
    gui: GuiWindow,
    mouse: (i32, i32),
    buttons: u32,
    key: u8,
    focused: bool,
    frame: u64,
    redraw: bool,
    close_requested: bool,
}

impl DemoWindow {
    fn new(gui: GuiWindow, mouse: (i32, i32)) -> Self {
        Self { gui, mouse, buttons: 0, key: 0, focused: false, frame: 0,
               redraw: true, close_requested: false }
    }

    fn on_event(&mut self, event: GuiEvent) {
        match event {
            GuiEvent::Mouse { x, y, buttons, .. } => { self.mouse = (x, y); self.buttons = buttons; self.redraw = true; }
            GuiEvent::Key { code, released: false } => {
                self.key = code;
                self.redraw = true;
            }
            GuiEvent::Focus(value) => { self.focused = value; self.redraw = true; }
            GuiEvent::Close => self.close_requested = true,
            _ => {}
        }
    }

    fn draw(&mut self, conn: &GuiConnection, label: &[u8]) {
        if !self.redraw { return; }
        if let Some((buffer, address)) = self.gui.acquire() {
            let mut ui = UiSurface::new(address, self.gui.width(), self.gui.height(), self.gui.stride(), self.gui.format());
            let bg = if self.focused { ui.pack(28, 48, 76) } else { ui.pack(48, 52, 60) };
            ui.clear(bg);
            ui.text(18, 18, label, ui.pack(240, 245, 250));
            ui.text(18, 44, b"One process, multiple windows", ui.pack(155, 205, 235));
            let accent = if self.buttons & 1 != 0 { ui.pack(245, 105, 75) } else { ui.pack(65, 185, 230) };
            ui.fill(self.mouse.0 - 12, self.mouse.1 - 12, 24, 24, accent);
            let mut number = [0u8; 24];
            ui.text(18, 76, b"Last key: ", ui.pack(220, 225, 230));
            ui.text(98, 76, ui_u64(self.key as u64, &mut number), accent);
            ui.text(18, 100, b"Frame: ", ui.pack(220, 225, 230));
            ui.text(74, 100, ui_u64(self.frame, &mut number), accent);
            let committed = self.gui.commit(conn, buffer, 0, 0, self.gui.width(), self.gui.height());
            if committed { self.frame = self.frame.wrapping_add(1); }
            self.redraw = !committed;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let Some(mut conn) = GuiConnection::connect() else {
        sys_write_raw(b"guidemo: gui is not running\r\n");
        sys_exit(1);
    };
    let Some(gui) = conn.create_window(b"GUI Demo", 420, 260) else {
        sys_write_raw(b"guidemo: gui is not running\r\n");
        sys_exit(1);
    };
    // Both windows are independent: closing one leaves the other (and the
    // process) running, and the process exits once the last window is gone.
    let mut main = Some(DemoWindow::new(gui, (210, 130)));
    let mut sub = conn.create_window(b"GUI Demo Sub", 420, 200)
        .map(|gui| DemoWindow::new(gui, (210, 100)));
    let mut running = true;
    while running {
        let mut close_main = false;
        if let Some(main) = &mut main {
            while let Some(event) = main.gui.poll(&mut conn) { main.on_event(event); }
            close_main = main.close_requested;
        }
        if close_main {
            if let Some(main) = main.take() { main.gui.close(&conn); }
        }
        let mut close_sub = false;
        if let Some(sub) = &mut sub {
            while let Some(event) = sub.gui.poll(&mut conn) { sub.on_event(event); }
            close_sub = sub.close_requested;
        }
        if close_sub {
            if let Some(sub) = sub.take() { sub.gui.close(&conn); }
        }
        if main.is_none() && sub.is_none() { running = false; }
        if let Some(main) = &mut main { main.draw(&conn, b"Main window (ring3 GUI client)"); }
        if let Some(sub) = &mut sub { sub.draw(&conn, b"Sub window (same process)"); }
        sys_sleep(8);
    }
    if let Some(main) = main.take() { main.gui.close(&conn); }
    if let Some(sub) = sub.take() { sub.gui.close(&conn); }
    conn.close();
    sys_exit(0);
}
