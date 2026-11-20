#![no_std]
#![no_main]
include!("../runtime/runtime.rs");
include!("../runtime/gui_protocol.rs");
include!("../runtime/gui_client.rs");
include!("../runtime/gui_ui.rs");

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let Some(mut gui) = GuiClient::connect(b"GUI Profiler", 560, 330) else { sys_write_raw(b"profiler: gui is not running\r\n"); sys_exit(1); };
    let mut stats = [0u64; 7]; let mut redraw = true; let mut running = true; let mut tick = 0u32;
    while running {
        while let Some(event) = gui.poll() { match event { GuiEvent::Stats(values) => { stats = values; redraw = true; }, GuiEvent::Close => running = false, _ => {} } }
        tick += 1; if tick >= 30 { tick = 0; let _ = gui.request_stats(); }
        if redraw { if let Some((buffer, address)) = gui.acquire() {
            let mut ui = UiSurface::new(address, gui.width, gui.height, gui.stride, gui.format); let fg = ui.pack(230, 235, 240); let accent = ui.pack(70, 185, 230); ui.clear(ui.pack(22, 28, 38));
            ui.text(16, 16, b"Compositor protocol statistics", fg);
            let labels: [&[u8]; 7] = [b"Frames", b"Composites", b"Dirty pixels", b"Render ticks", b"Composite ticks", b"Present ticks", b"Ticks/second"];
            let mut number = [0u8; 24]; for index in 0..7 { let y = 52 + index as i32 * 34; ui.text(18, y, labels[index], fg); ui.text(190, y, ui_u64(stats[index], &mut number), accent); }
            redraw = !gui.commit(buffer, 0, 0, gui.width, gui.height);
        }}
        sys_sleep(16);
    }
    gui.close(); sys_exit(0);
}
