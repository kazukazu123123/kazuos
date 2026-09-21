#![no_std]
#![no_main]
include!("../runtime/runtime.rs");
include!("../runtime/gui_protocol.rs");
include!("../runtime/gui_client.rs");
include!("../runtime/gui_ui.rs");

const MAX_ROWS: usize = 24;

fn refresh(rows: &mut [ProcessInfo; MAX_ROWS]) -> usize {
    let mut count = 0; let mut previous = 0;
    while count < MAX_ROWS {
        let pid = sys_proc_next(previous); if pid == u64::MAX { break; } previous = pid;
        if sys_proc_info(pid, &mut rows[count] as *mut ProcessInfo as *mut u64) == 0 { count += 1; }
    }
    count
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let Some(mut gui) = GuiClient::connect(b"Task Manager", 620, 430) else { sys_write_raw(b"taskmgr: gui is not running\r\n"); sys_exit(1); };
    let mut rows = [ProcessInfo::ZERO; MAX_ROWS]; let mut count = refresh(&mut rows);
    let mut selected: Option<u64> = None; let mut redraw = true; let mut running = true; let mut ticks = 0;
    while running {
        while let Some(event) = gui.poll() { match event {
            GuiEvent::Mouse { x, y, buttons, changed } if changed & 1 != 0 && buttons & 1 == 0 => {
                if x >= 12 && y >= 52 { let row = ((y - 52) / 16) as usize; if row < count { selected = Some(rows[row].pid); } }
                if (500..604).contains(&x) && (14..42).contains(&y) { if let Some(pid) = selected { let _ = sys_kill(pid); selected = None; count = refresh(&mut rows); } }
                redraw = true;
            }
            GuiEvent::Close => running = false, _ => {}
        }}
        ticks += 1; if ticks >= 60 { ticks = 0; count = refresh(&mut rows); redraw = true; }
        if redraw { if let Some((buffer, address)) = gui.acquire() {
            let mut ui = UiSurface::new(address, gui.width, gui.height, gui.stride, gui.format);
            let fg = ui.pack(230, 235, 240); let dim = ui.pack(150, 165, 180); ui.clear(ui.pack(22, 28, 38));
            ui.text(12, 18, b"PID     Memory       CPU ticks    Image", fg); ui.button(500, 10, 104, 30, b"Kill", false);
            for index in 0..count { let y = 52 + index as i32 * 16; if selected == Some(rows[index].pid) { ui.fill(8, y - 1, 600, 16, ui.pack(45, 75, 105)); }
                let mut number = [0u8; 24]; ui.text(12, y, ui_u64(rows[index].pid, &mut number), fg);
                ui.text(76, y, ui_u64(rows[index].memory_bytes, &mut number), dim);
                ui.text(188, y, ui_u64(rows[index].cpu_ticks, &mut number), dim);
                let length = rows[index].image_name.iter().position(|byte| *byte == 0).unwrap_or(PROC_NAME_LEN); ui.text(300, y, &rows[index].image_name[..length], fg);
            }
            redraw = !gui.commit(buffer, 0, 0, gui.width, gui.height);
        }}
        sys_sleep(16);
    }
    gui.close(); sys_exit(0);
}
