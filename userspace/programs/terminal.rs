#![no_std]
#![no_main]
include!("../runtime/runtime.rs");
include!("../runtime/gui_protocol.rs");
include!("../runtime/gui_client.rs");
include!("../runtime/gui_ui.rs");

const COLS: usize = 78;
const ROWS: usize = 24;

fn call(number: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let result; unsafe { core::arch::asm!("int 0x80", inlateout("rax") number => result, in("rdi") a0, in("rsi") a1, in("rdx") a2); } result
}

struct Terminal { grid: [[u8; COLS]; ROWS], x: usize, y: usize, input: u64, output: u64, shell: u64, alive: bool, ansi: u8, ansi_a: usize, ansi_b: usize, ansi_param: u8 }
impl Terminal {
    fn new() -> Self {
        let mut input = [0u64; 2]; let mut output = [0u64; 2];
        if call(SYS_PIPE, input.as_mut_ptr() as u64, 0, 0) != 0 { return Self::dead(); }
        if call(SYS_PIPE, output.as_mut_ptr() as u64, 0, 0) != 0 {
            call(SYS_CLOSE, input[0], 0, 0);
            call(SYS_CLOSE, input[1], 0, 0);
            return Self::dead();
        }
        let stdio = (input[0] & 0xffff) | ((output[1] & 0xffff) << 16);
        let path = b"/bin/shell.kxe\0"; let shell = call(SYS_EXEC, path.as_ptr() as u64, path.len() as u64, stdio);
        call(SYS_CLOSE, input[0], 0, 0); call(SYS_CLOSE, output[1], 0, 0);
        if shell == 0 || shell == u64::MAX {
            call(SYS_CLOSE, input[1], 0, 0);
            call(SYS_CLOSE, output[0], 0, 0);
            return Self::dead();
        }
        call(SYS_CONSOLE_SIZE, COLS as u64 | ((ROWS as u64) << 16), shell, 0);
        Self { grid: [[b' '; COLS]; ROWS], x: 0, y: 0, input: input[1], output: output[0], shell, alive: true, ansi: 0, ansi_a: 0, ansi_b: 0, ansi_param: 0 }
    }
    fn dead() -> Self { Self { grid: [[b' '; COLS]; ROWS], x: 0, y: 0, input: 0, output: 0, shell: 0, alive: false, ansi: 0, ansi_a: 0, ansi_b: 0, ansi_param: 0 } }
    fn newline(&mut self) { self.x = 0; if self.y + 1 < ROWS { self.y += 1; } else { for row in 1..ROWS { self.grid[row - 1] = self.grid[row]; } self.grid[ROWS - 1] = [b' '; COLS]; } }
    fn finish_csi(&mut self, byte: u8) {
        let count = self.ansi_a.max(1);
        match byte {
            b'A' => self.y = self.y.saturating_sub(count),
            b'B' => self.y = self.y.saturating_add(count).min(ROWS - 1),
            b'C' => self.x = self.x.saturating_add(count).min(COLS - 1),
            b'D' => self.x = self.x.saturating_sub(count),
            b'G' => self.x = count.saturating_sub(1).min(COLS - 1),
            b'H' | b'f' => {
                self.y = count.saturating_sub(1).min(ROWS - 1);
                self.x = self.ansi_b.max(1).saturating_sub(1).min(COLS - 1);
            }
            b'J' => match self.ansi_a {
                1 => {
                    for row in 0..self.y { self.grid[row] = [b' '; COLS]; }
                    for column in 0..=self.x.min(COLS - 1) { self.grid[self.y][column] = b' '; }
                }
                2 | 3 => self.grid = [[b' '; COLS]; ROWS],
                _ => {
                    for column in self.x.min(COLS)..COLS { self.grid[self.y][column] = b' '; }
                    for row in self.y + 1..ROWS { self.grid[row] = [b' '; COLS]; }
                }
            },
            b'K' => match self.ansi_a {
                1 => for column in 0..=self.x.min(COLS - 1) { self.grid[self.y][column] = b' '; },
                2 => self.grid[self.y] = [b' '; COLS],
                _ => for column in self.x.min(COLS)..COLS { self.grid[self.y][column] = b' '; },
            },
            _ => {}
        }
        self.ansi = 0;
    }
    fn put(&mut self, byte: u8) {
        if self.ansi == 1 {
            if byte == b'[' { self.ansi = 2; self.ansi_a = 0; self.ansi_b = 0; self.ansi_param = 0; }
            else { self.ansi = 0; }
            return;
        }
        if self.ansi == 2 {
            match byte {
                b'0'..=b'9' => {
                    let value = if self.ansi_param == 0 { &mut self.ansi_a } else { &mut self.ansi_b };
                    *value = value.saturating_mul(10).saturating_add((byte - b'0') as usize);
                }
                b';' => self.ansi_param = 1,
                0x40..=0x7e => self.finish_csi(byte),
                _ => self.ansi = 0,
            }
            return;
        }
        match byte { 0x1b => self.ansi = 1, b'\n' => self.newline(), b'\r' => self.x = 0, 8 => self.x = self.x.saturating_sub(1),
            byte if (0x20..=0x7e).contains(&byte) => { if self.x >= COLS { self.newline(); } self.grid[self.y][self.x] = byte; self.x += 1; } _ => {} }
    }
    fn pump(&mut self) -> bool { let mut changed = false; let mut bytes = [0u8; 2048]; loop { let count = call(SYS_TRY_READ, self.output, bytes.as_mut_ptr() as u64, bytes.len() as u64); if count == 0 { break; } if count == u64::MAX { self.alive = false; break; } for byte in &bytes[..count as usize] { self.put(*byte); } changed = true; if count < bytes.len() as u64 { break; } } changed }
    fn contains(&self, needle: &[u8]) -> bool { self.grid.iter().any(|row| row.windows(needle.len()).any(|window| window == needle)) }
    fn key(&self, code: u8) {
        if !self.alive { return; }
        if code == 0x03 && call(SYS_SIGINT_FG, self.shell, 0, 0) == 1 { return; }
        let byte = [code];
        call(SYS_WRITE, self.input, byte.as_ptr() as u64, 1);
    }
    fn send_line(&self, line: &[u8]) { if self.alive { call(SYS_WRITE, self.input, line.as_ptr() as u64, line.len() as u64); } }
    fn stop(&mut self) { if self.shell != 0 && self.shell != u64::MAX { call(SYS_SIGKILL, self.shell, 0, 0); } if self.input != 0 { call(SYS_CLOSE, self.input, 0, 0); } if self.output != 0 { call(SYS_CLOSE, self.output, 0, 0); } }
    fn draw(&self, ui: &mut UiSurface) { let fg = ui.pack(210, 215, 220); ui.clear(ui.pack(8, 10, 16)); for row in 0..ROWS { for column in 0..COLS { let byte = self.grid[row][column]; if byte != b' ' { ui.glyph(6 + column as i32 * 8, 5 + row as i32 * 16, byte, fg); } } } if self.alive { ui.fill(6 + self.x as i32 * 8, 5 + self.y as i32 * 16, 7, 14, ui.pack(55, 195, 85)); } }
}

fn has_arg(argc: u64, argv: u64, wanted: &[u8]) -> bool {
    if argv == 0 { return false; }
    for index in 0..argc as usize {
        let pointer = unsafe { *(argv as *const u64).add(index) };
        if pointer == 0 { continue; }
        let mut length = 0usize;
        while length <= 64 && unsafe { *(pointer as *const u8).add(length) } != 0 { length += 1; }
        if length == wanted.len() && unsafe { core::slice::from_raw_parts(pointer as *const u8, length) } == wanted { return true; }
    }
    false
}

fn live_child(parent: u64) -> Option<u64> {
    let mut pid = 0;
    loop {
        pid = sys_proc_next(pid);
        if pid == 0 || pid == u64::MAX { return None; }
        let mut info = ProcessInfo::ZERO;
        if sys_proc_info(pid, &mut info as *mut ProcessInfo as *mut u64) == 0
            && info.parent == parent && info.state != 4 { return Some(pid); }
    }
}

fn wait_child(parent: u64) -> Option<u64> {
    for _ in 0..250 {
        if let Some(pid) = live_child(parent) { return Some(pid); }
        sys_sleep(20);
    }
    None
}

fn wait_no_child(parent: u64) -> bool {
    for _ in 0..250 {
        if live_child(parent).is_none() { return true; }
        sys_sleep(20);
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    let Some(mut gui) = GuiClient::connect(b"Terminal", 636, 394) else { sys_write_raw(b"terminal: gui is not running\r\n"); sys_exit(1); }; let mut terminal = Terminal::new();
    if has_arg(argc, argv, b"--test-nested-shell") {
        terminal.send_line(b"clear\r");
        sys_sleep(200);
        terminal.pump();
        let clear_ok = terminal.x == 8 && terminal.y == 0
            && &terminal.grid[0][..8] == b"KazuOS> "
            && terminal.grid[0][8..].iter().all(|byte| *byte == b' ');
        terminal.send_line(b"shell\r");
        let nested = wait_child(terminal.shell);
        if nested.is_none() { sys_write_raw(b"terminal-nest-test: nested spawn failed\r\n"); }
        sys_sleep(200);
        terminal.key(0x03);
        sys_sleep(100);
        terminal.pump();
        let prompt_interrupt = terminal.contains(b"^C")
            && nested.is_some_and(|pid| live_child(terminal.shell) == Some(pid));
        let mut command_interrupt = false;
        if let Some(nested_pid) = nested {
            terminal.send_line(b"cpuburner\r");
            if wait_child(nested_pid).is_some() {
                sys_sleep(200);
                terminal.key(0x03);
                command_interrupt = wait_no_child(nested_pid)
                    && live_child(terminal.shell) == Some(nested_pid);
            }
        }
        terminal.key(0x04);
        let returned = wait_no_child(terminal.shell);
        sys_sleep(100);
        terminal.pump();
        let ctrl_d_visible = terminal.contains(b"exit");
        if clear_ok { sys_write_raw(b"terminal-clear-test: PASS\r\n"); }
        else { sys_write_raw(b"terminal-clear-test: FAIL\r\n"); }
        if !prompt_interrupt { sys_write_raw(b"terminal-nest-test: prompt interrupt failed\r\n"); }
        if !command_interrupt { sys_write_raw(b"terminal-nest-test: command interrupt failed\r\n"); }
        if !returned { sys_write_raw(b"terminal-nest-test: Ctrl+D return failed\r\n"); }
        if !ctrl_d_visible { sys_write_raw(b"terminal-nest-test: Ctrl+D exit text missing\r\n"); }
        if prompt_interrupt && command_interrupt && returned && ctrl_d_visible { sys_write_raw(b"terminal-nest-test: PASS\r\n"); }
        else { sys_write_raw(b"terminal-nest-test: FAIL\r\n"); }
        call(SYS_SIGKILL, terminal.shell, 0, 0);
    }
    let mut redraw = true; let mut running = true;
    while running { while let Some(event) = gui.poll() { match event { GuiEvent::Key { code, released: false } => { terminal.key(code); }, GuiEvent::Close => running = false, _ => {} } }
        if terminal.pump() { redraw = true; }
        if !terminal.alive { running = false; }
        if redraw { if let Some((buffer, address)) = gui.acquire() { let mut ui = UiSurface::new(address, gui.width, gui.height, gui.stride, gui.format); terminal.draw(&mut ui); redraw = !gui.commit(buffer, 0, 0, gui.width, gui.height); } } sys_sleep(8); }
    terminal.stop(); gui.close(); sys_exit(0);
}
