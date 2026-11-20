#![no_std]
#![no_main]
include!("../runtime/runtime.rs");
include!("../runtime/gui_protocol.rs");
include!("../runtime/gui_client.rs");
include!("../runtime/gui_ui.rs");

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let Some(mut gui) = GuiClient::connect(b"GUI Demo", 420, 260) else {
        sys_write_raw(b"guidemo: gui is not running\r\n");
        sys_exit(1);
    };
    let mut mouse = (210, 130);
    let mut buttons = 0u32;
    let mut key = 0u8;
    let mut focused = false;
    let mut frame = 0u64;
    let mut redraw = true;
    let mut running = true;
    while running {
        while let Some(event) = gui.poll() {
            match event {
                GuiEvent::Mouse { x, y, buttons: value, .. } => { mouse = (x, y); buttons = value; redraw = true; }
                GuiEvent::Key { code, released: false } => { key = code; redraw = true; if code == 27 { running = false; } }
                GuiEvent::Focus(value) => { focused = value; redraw = true; }
                GuiEvent::Close => running = false,
                _ => {}
            }
        }
        if redraw {
            if let Some((buffer, address)) = gui.acquire() {
                frame = frame.wrapping_add(1);
                let mut ui = UiSurface::new(address, gui.width, gui.height, gui.stride, gui.format);
                let bg = if focused { ui.pack(28, 48, 76) } else { ui.pack(48, 52, 60) };
                ui.clear(bg);
                ui.text(18, 18, b"Independent ring3 GUI client", ui.pack(240, 245, 250));
                ui.text(18, 44, b"Mouse, keyboard, focus and SHM buffers", ui.pack(155, 205, 235));
                let accent = if buttons & 1 != 0 { ui.pack(245, 105, 75) } else { ui.pack(65, 185, 230) };
                ui.fill(mouse.0 - 12, mouse.1 - 12, 24, 24, accent);
                let mut number = [0u8; 24];
                ui.text(18, 76, b"Last key: ", ui.pack(220, 225, 230));
                ui.text(98, 76, ui_u64(key as u64, &mut number), accent);
                ui.text(18, 100, b"Frame: ", ui.pack(220, 225, 230));
                ui.text(74, 100, ui_u64(frame, &mut number), accent);
                redraw = !gui.commit(buffer, 0, 0, gui.width, gui.height);
            }
        }
        sys_sleep(8);
    }
    gui.close();
    sys_exit(0);
}
