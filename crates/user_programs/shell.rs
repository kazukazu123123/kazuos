#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

const STDIO_DEFAULT: u64 = 0xFFFF_FFFF;
// Child inherits the shell's stdout (fd 1): default stdin, stdout = our fd 1. So a
// command's output follows the shell's stdout — the console normally, or the GUI
// terminal's pipe when the shell itself is running piped.
const STDIO_INHERIT: u64 = 0xFFFF | (1 << 16);

const KEY_UP:   u64 = 0x82;
const KEY_DOWN: u64 = 0x83;

const MAX_HISTORY: usize = 32;
static mut HISTORY: [[u8; BUF_SIZE]; MAX_HISTORY] = [[0u8; BUF_SIZE]; MAX_HISTORY];
static mut HISTORY_LENS: [usize; MAX_HISTORY] = [0usize; MAX_HISTORY];
static mut HISTORY_COUNT: usize = 0; // total commands ever added (wraps into ring)

const BUF_SIZE: usize = 256;
const NAME_LEN: usize = 32;

#[repr(C)]
struct ProcessInfo {
    pid: u64,
    state: u64,
    image_name: [u8; NAME_LEN],
    start_tsc: u64,
    entry: u64,
    stack_top: u64,
    step: u64,
    cpu_ticks: u64,
    memory_bytes: u64,
}

// When launched with a `--pipe` argument (by the GUI terminal), the shell does its
// I/O over stdin/stdout (fd 0/1) — which the GUI has wired to pipes — instead of the
// kernel console + keyboard. The normal console shell (no flag) is unchanged.
static mut PIPE_MODE: bool = false;
fn pipe_mode() -> bool { unsafe { PIPE_MODE } }

fn has_pipe_flag(argc: u64, argv: u64) -> bool {
    if argv == 0 { return false; }
    let ptrs = argv as *const u64;
    // argv here is the kernel's arg list (no argv[0]=path slot), so start at 0.
    for i in 0..argc {
        let p = unsafe { *ptrs.add(i as usize) } as *const u8;
        if p.is_null() { continue; }
        let target = b"--pipe";
        let mut ok = true;
        for (j, &t) in target.iter().enumerate() {
            if unsafe { *p.add(j) } != t { ok = false; break; }
        }
        if ok && unsafe { *p.add(target.len()) } == 0 { return true; }
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    unsafe { PIPE_MODE = has_pipe_flag(argc, argv); }
    let mut buf = [0u8; BUF_SIZE];
    loop {
        let len = if pipe_mode() { read_line_pipe(&mut buf) } else { read_line(&mut buf) };
        if len > 0 {
            execute(&buf[..len]);
        }
    }
}

// Simple line editor for pipe mode: read bytes from stdin, echo to stdout, handle
// Enter and backspace. No history/cursor movement — the GUI terminal is a plain
// dumb terminal. EOF on stdin (the terminal window closed) exits the shell.
//
// Uses non-blocking SYS_TRY_READ + a short sleep rather than a blocking read: a
// blocking pipe read is completed by the *writer's* wakeup, which copies into our
// buffer using the writer's address space (wrong CR3). Polling keeps the read in
// our own context so the bytes land correctly.
fn read_line_pipe(buf: &mut [u8]) -> usize {
    sys_write(b"KazuOS> ");
    let mut len = 0usize;
    let mut chunk = [0u8; 32];
    loop {
        let n = sys_try_read(0, &mut chunk);
        if n == u64::MAX { sys_exit(0); } // stdin closed → shell exits
        if n == 0 {
            syscall3(SYS_SLEEP, 15, SLEEP_UNIT_MS);
            continue;
        }
        for i in 0..n as usize {
            match chunk[i] {
                b'\r' | b'\n' => { sys_write(b"\r\n"); return len; }
                0x03 => { sys_write(b"^C\r\n"); return 0; } // Ctrl+C: cancel the line
                0x08 | 0x7f => {
                    if len > 0 { len -= 1; sys_write(b"\x08 \x08"); }
                }
                ch => {
                    if len < buf.len() { buf[len] = ch; len += 1; let c = [ch]; sys_write(&c); }
                }
            }
        }
    }
}

fn read_line(buf: &mut [u8]) -> usize {
    let mut len = 0usize;
    let mut pos = 0usize;
    let mut cursor_on = true;
    let mut blink_ticks = 0u32;
    // hist_age=0: live input, 1=most recent, 2=second most recent, ...
    let mut hist_age: usize = 0;
    let mut saved_buf = [0u8; BUF_SIZE];
    let mut saved_len = 0usize;

    redraw(buf, len, pos, cursor_on);

    loop {
        let ch = syscall1(SYS_KEYBOARD_POLL, 0);
        if ch == 0 {
            syscall3(SYS_SLEEP, 50, SLEEP_UNIT_MS);
            blink_ticks += 1;
            if blink_ticks >= 10 {
                blink_ticks = 0;
                cursor_on = !cursor_on;
                redraw(buf, len, pos, cursor_on);
            }
            continue;
        }

        blink_ticks = 0;
        cursor_on = true;

        match ch {
            0x0A | 0x0D => {
                redraw(buf, len, len, false);
                sys_write(b"\n");
                // push non-empty command to history
                if len > 0 {
                    unsafe {
                        let count = *core::ptr::addr_of_mut!(HISTORY_COUNT);
                        let slot = count % MAX_HISTORY;
                        let dst = core::ptr::addr_of_mut!(HISTORY[slot]).cast::<u8>();
                        core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, len);
                        *core::ptr::addr_of_mut!(HISTORY_LENS[slot]) = len;
                        *core::ptr::addr_of_mut!(HISTORY_COUNT) = count + 1;
                    }
                }
                return len;
            }
            0x08 | 0x7F => {
                if pos > 0 {
                    for i in pos - 1..len - 1 {
                        buf[i] = buf[i + 1];
                    }
                    len -= 1;
                    pos -= 1;
                }
            }
            0x80 => { // left
                if pos > 0 { pos -= 1; }
            }
            0x81 => { // right
                if pos < len { pos += 1; }
            }
            KEY_UP => {
                let count = unsafe { *core::ptr::addr_of!(HISTORY_COUNT) };
                let max_age = count.min(MAX_HISTORY);
                if hist_age < max_age {
                    if hist_age == 0 {
                        // save current input
                        saved_buf[..len].copy_from_slice(&buf[..len]);
                        saved_len = len;
                    }
                    hist_age += 1;
                    unsafe {
                        let count = *core::ptr::addr_of!(HISTORY_COUNT);
                        let slot = (count - hist_age) % MAX_HISTORY;
                        len = *core::ptr::addr_of!(HISTORY_LENS[slot]);
                        let src = core::ptr::addr_of!(HISTORY[slot]).cast::<u8>();
                        core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len);
                    }
                    pos = len;
                }
            }
            KEY_DOWN => {
                if hist_age > 0 {
                    hist_age -= 1;
                    if hist_age == 0 {
                        // restore saved input
                        len = saved_len;
                        buf[..len].copy_from_slice(&saved_buf[..len]);
                    } else {
                        unsafe {
                            let count = *core::ptr::addr_of!(HISTORY_COUNT);
                            let slot = (count - hist_age) % MAX_HISTORY;
                            len = *core::ptr::addr_of!(HISTORY_LENS[slot]);
                            let src = core::ptr::addr_of!(HISTORY[slot]).cast::<u8>();
                            core::ptr::copy_nonoverlapping(src, buf.as_mut_ptr(), len);
                        }
                    }
                    pos = len;
                }
            }
            c if c >= 0x20 && c < 0x7F => {
                if len < buf.len() - 1 {
                    for i in (pos..len).rev() {
                        buf[i + 1] = buf[i];
                    }
                    buf[pos] = c as u8;
                    len += 1;
                    pos += 1;
                }
            }
            _ => {}
        }

        redraw(buf, len, pos, cursor_on);
    }
}

fn redraw(buf: &[u8], len: usize, pos: usize, cursor_on: bool) {
    sys_write(b"\r\x1b[K");
    sys_write(b"KazuOS> ");
    sys_write(&buf[..pos]);
    syscall1(SYS_CURSOR_SAVE, 0);
    sys_write(&buf[pos..len]);
    if cursor_on {
        syscall1(SYS_CURSOR_DRAW, 1);
    }
}

fn execute(cmd: &[u8]) {
    let cmd = trim(cmd);
    if cmd.is_empty() {
        return;
    }
    let (cmd, background) = if cmd.ends_with(b"&") {
        (trim(&cmd[..cmd.len() - 1]), true)
    } else {
        (cmd, false)
    };
    if cmd == b"help" {
        return cmd_help();
    }
    if cmd == b"clear" {
        return cmd_clear();
    }
    if cmd == b"mem" {
        return cmd_mem();
    }
    if cmd == b"ps" {
        return cmd_ps();
    }
    if cmd == b"sysinfo" {
        return cmd_sysinfo();
    }
    if cmd == b"smpinfo" {
        return cmd_smpinfo();
    }
    if cmd == b"shutdown" {
        return cmd_shutdown();
    }
    if cmd == b"reboot" {
        return cmd_reboot();
    }
    if cmd == b"ls" || cmd.starts_with(b"ls ") {
        return cmd_ls(cmd);
    }
    if cmd.starts_with(b"touch ") {
        return cmd_touch(trim(&cmd[6..]));
    }
    if cmd.starts_with(b"rm ") {
        return cmd_rm(trim(&cmd[3..]));
    }
    if cmd.starts_with(b"mkdir ") {
        return cmd_mkdir(trim(&cmd[6..]));
    }
    if cmd.starts_with(b"rmdir ") {
        return cmd_rmdir(trim(&cmd[6..]));
    }
    if cmd.starts_with(b"cat ") {
        return cmd_cat(trim(&cmd[4..]));
    }
    if cmd.starts_with(b"echo ") {
        return cmd_echo(trim(&cmd[5..]));
    }
    if cmd.starts_with(b"exec ") {
        return cmd_exec(&cmd[5..]);
    }
    // check for pipe: cmd1 | cmd2
    if let Some(pipe_pos) = find_pipe(cmd) {
        let cmd1 = trim(&cmd[..pipe_pos]);
        let cmd2 = trim(&cmd[pipe_pos + 1..]);
        return cmd_pipe(cmd1, cmd2);
    }
    // try /bin/<name>.kxe
    let pid = exec_bin(cmd, STDIO_INHERIT);
    if pid == 1 {
        sys_write(b"KazuOS: ");
        sys_write(cmd);
        sys_write(b": cannot execute driver directly\r\n");
        return;
    }
    if pid != 0 && pid != u64::MAX {
        if background {
            sys_write(b"[bg] pid=");
            write_u64(pid);
            sys_write(b"\r\n");
        } else {
            syscall1(SYS_WAIT, pid);
        }
        return;
    }
    sys_write(b"KazuOS: unknown command: ");
    sys_write(cmd);
    sys_write(b"\r\n");
}

fn find_pipe(cmd: &[u8]) -> Option<usize> {
    cmd.iter().position(|&b| b == b'|')
}

// ── RAM rootfs commands ─────────────────────────────────────────────────────

fn need_abs(path: &[u8]) -> bool {
    if path.is_empty() || path[0] != b'/' {
        sys_write(b"path must be absolute (start with /)\r\n");
        return false;
    }
    true
}

fn cmd_touch(path: &[u8]) {
    if !need_abs(path) { return; }
    let fd = sys_create(path);
    if fd == u64::MAX {
        sys_write(b"touch: failed\r\n");
        return;
    }
    sys_close(fd);
}

fn cmd_rm(path: &[u8]) {
    if !need_abs(path) { return; }
    if sys_unlink(path) == u64::MAX {
        sys_write(b"rm: failed (not a file or not found)\r\n");
    }
}

fn cmd_mkdir(path: &[u8]) {
    if !need_abs(path) { return; }
    if sys_mkdir(path) == u64::MAX {
        sys_write(b"mkdir: failed (exists or missing parent)\r\n");
    }
}

fn cmd_rmdir(path: &[u8]) {
    if !need_abs(path) { return; }
    if sys_rmdir(path) == u64::MAX {
        sys_write(b"rmdir: failed (not empty or not a dir)\r\n");
    }
}

fn cmd_cat(path: &[u8]) {
    if !need_abs(path) { return; }
    let fd = sys_open(path);
    if fd == u64::MAX {
        sys_write(b"cat: not found\r\n");
        return;
    }
    let mut buf = [0u8; 256];
    loop {
        let n = sys_read(fd, &mut buf);
        if n == 0 || n == u64::MAX { break; }
        sys_write(&buf[..n as usize]);
    }
    sys_write(b"\r\n");
    sys_close(fd);
}

fn cmd_echo(args: &[u8]) {
    // "echo TEXT"          -> stdout
    // "echo TEXT > /path"  -> write TEXT to a file (create/overwrite)
    if let Some(pos) = args.iter().position(|&b| b == b'>') {
        let text = trim(&args[..pos]);
        let path = trim(&args[pos + 1..]);
        if !need_abs(path) { return; }
        let fd = sys_create(path);
        if fd == u64::MAX {
            sys_write(b"echo: cannot create file\r\n");
            return;
        }
        sys_write_fd(fd, text);
        sys_close(fd);
    } else {
        sys_write(args);
        sys_write(b"\r\n");
    }
}

fn exec_bin(cmd: &[u8], stdio_pack: u64) -> u64 {
    let name = cmd.split(|&b| b == b' ').next().unwrap_or(cmd);
    let args_part = if cmd.len() > name.len() + 1 {
        trim(&cmd[name.len() + 1..])
    } else {
        &[][..]
    };

    try_exec_path(name, args_part, stdio_pack)
}

fn try_exec_path(name: &[u8], args_part: &[u8], stdio_pack: u64) -> u64 {
    let mut buf = [0u8; 128];

    // Build path in buf
    let path_section;
    if name.contains(&b'/') {
        if name.starts_with(b"/") {
            if name.len() + 1 > buf.len() { return u64::MAX; }
            buf[..name.len()].copy_from_slice(name);
            path_section = name.len();
        } else {
            if 1 + name.len() + 1 > buf.len() { return u64::MAX; }
            buf[0] = b'/';
            buf[1..1 + name.len()].copy_from_slice(name);
            path_section = 1 + name.len();
        }
    } else {
        let prefix = b"/bin/";
        let suffix = b".kxe";
        let path_total = prefix.len() + name.len() + suffix.len();
        if path_total + 1 > buf.len() { return u64::MAX; }
        buf[..prefix.len()].copy_from_slice(prefix);
        buf[prefix.len()..prefix.len() + name.len()].copy_from_slice(name);
        buf[prefix.len() + name.len()..path_total].copy_from_slice(suffix);
        path_section = path_total;
    }

    // Null-terminate path
    buf[path_section] = 0;

    // Write args as null-separated tokens
    let mut pos = path_section + 1;
    if !args_part.is_empty() {
        for arg in args_part.split(|&b| b == b' ') {
            let arg = trim(arg);
            if arg.is_empty() { continue; }
            if pos + arg.len() + 1 > buf.len() { return u64::MAX; }
            buf[pos..pos + arg.len()].copy_from_slice(arg);
            pos += arg.len();
            buf[pos] = 0;
            pos += 1;
        }
    }

    let total = if pos > path_section + 1 { pos } else { path_section + 1 };
    let pid = syscall4(SYS_EXEC, buf.as_ptr() as u64, total as u64, stdio_pack);
    if pid != 0 && pid != u64::MAX { return pid; }
    u64::MAX
}

fn cmd_pipe(cmd1: &[u8], cmd2: &[u8]) {
    let mut fds = [0u64; 2]; // [read_fd, write_fd]
    if syscall1(SYS_PIPE, fds.as_mut_ptr() as u64) != 0 {
        sys_write(b"pipe failed\r\n");
        return;
    }
    let read_fd  = fds[0];
    let write_fd = fds[1];

    // spawn cmd1 with stdout = write_fd
    let stdio1 = 0xFFFF | (write_fd << 16);
    let pid1 = exec_bin(cmd1, stdio1);

    // shell closes write end so cmd2 sees EOF when cmd1 exits
    syscall1(SYS_CLOSE, write_fd);

    // spawn cmd2 with stdin = read_fd
    let stdio2 = read_fd | (0xFFFF << 16);
    let pid2 = exec_bin(cmd2, stdio2);

    // shell closes read end
    syscall1(SYS_CLOSE, read_fd);

    if pid2 != 0 && pid2 != 1 && pid2 != u64::MAX {
        syscall1(SYS_WAIT, pid2);
    }
}

fn cmd_help() {
    sys_write(b"commands: help clear ls mem ps sysinfo smpinfo shutdown reboot\r\n");
}

fn cmd_clear() {
    // Clear via an in-band ANSI sequence (clear screen + home cursor) so it works on
    // whatever the shell's stdout is — the kernel console and the GUI terminal both
    // interpret it — instead of an out-of-band console-only syscall.
    sys_write(b"\x1b[2J\x1b[H");
}

fn cmd_mem() {
    let info = syscall1(SYS_MEM_INFO, 0);
    if info == 0 {
        sys_write(b"PMM unavailable\r\n");
        return;
    }
    let total_kib = info >> 32;
    let used_kib = info & 0xffffffff;
    let free_kib = total_kib.saturating_sub(used_kib);
    sys_write(b"total: ");
    write_u64(total_kib);
    sys_write(b" KiB used: ");
    write_u64(used_kib);
    sys_write(b" KiB free: ");
    write_u64(free_kib);
    sys_write(b" KiB\r\n");
}

fn cmd_ps() {
    // Print kernel placeholder (pid 0) first so it is always visible.
    print_process_info(0, b"kernel");

    let mut pid = syscall2(SYS_PROCESS_NEXT, 0);
    while pid != u64::MAX {
        print_process_info(pid, &[]);
        pid = syscall2(SYS_PROCESS_NEXT, pid);
    }
}

fn print_process_info(pid: u64, force_name: &[u8]) {
    let mut info = ProcessInfo {
        pid: 0,
        state: 0,
        image_name: [0u8; NAME_LEN],
        start_tsc: 0,
        entry: 0,
        stack_top: 0,
        step: 0,
        cpu_ticks: 0,
        memory_bytes: 0,
    };
    let r = syscall3(SYS_PROCESS_INFO, pid, &mut info as *mut _ as u64);
    if r != 0 {
        return;
    }
    let state_name = match info.state {
        1 => b"ready   ",
        2 => b"running ",
        3 => b"sleeping",
        4 => b"exited  ",
        _ => b"unknown ",
    };
    let name_len = info
        .image_name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(NAME_LEN);
    sys_write(b"pid=");
    write_u64(info.pid);
    sys_write(b" state=");
    sys_write(state_name);
    sys_write(b" cpu=");
    write_u64(info.cpu_ticks);
    sys_write(b" mem=");
    write_u64(info.memory_bytes / 1024);
    sys_write(b"KiB name=");
    if force_name.is_empty() {
        sys_write(&info.image_name[..name_len]);
    } else {
        sys_write(force_name);
    }
    sys_write(b"\r\n");
}

fn cmd_sysinfo() {
    cmd_mem();
    let timer_ticks = syscall1(SYS_CPU_INFO, 0);
    let count = syscall1(SYS_PROCESS_INFO, 1);
    sys_write(b"processes: ");
    write_u64(count);
    sys_write(b" timer_ticks: ");
    write_u64(timer_ticks);
    sys_write(b"\r\n");
}

fn cmd_smpinfo() {
    let cpu_count = syscall1(SYS_CPU_INFO, 4);
    let bsp_apic = syscall1(SYS_CPU_INFO, 5);
    let current_index = syscall1(SYS_CPU_INFO, 6);
    sys_write(b"cpus: ");
    write_u64(cpu_count);
    sys_write(b" bsp_apic_id: ");
    write_u64(bsp_apic);
    sys_write(b" current_cpu: ");
    write_u64(current_index);
    sys_write(b"\r\n");
    for i in 0..cpu_count {
        let apic_id = syscall3(SYS_CPU_INFO, 7, i);
        sys_write(b"  cpu[");
        write_u64(i);
        sys_write(b"] apic_id=");
        write_u64(apic_id);
        sys_write(b"\r\n");
    }
}

fn cmd_ls(cmd: &[u8]) {
    let path_raw = if cmd.len() > 3 {
        trim(&cmd[3..])
    } else {
        b"/"
    };
    let mut norm_buf = [0u8; 256];
    let path = if path_raw.starts_with(b"/") {
        path_raw
    } else {
        if 1 + path_raw.len() > norm_buf.len() {
            sys_write(b"ls: path too long\r\n");
            return;
        }
        norm_buf[0] = b'/';
        norm_buf[1..1 + path_raw.len()].copy_from_slice(path_raw);
        &norm_buf[..1 + path_raw.len()]
    };
    const CAP: usize = 64;
    let mut ents = [DirEnt { kind: 0, name: [0u8; 32] }; CAP];
    let r = syscall4(SYS_READDIR, path.as_ptr() as u64, path.len() as u64, ents.as_mut_ptr() as u64);
    if r == u64::MAX {
        sys_write(b"ls: failed\r\n");
        return;
    }
    let count = (r as usize).min(CAP);
    for e in &ents[..count] {
        sys_write(match e.kind {
            1 => b"dir   ",
            2 => b"dev   ",
            _ => b"file  ",
        });
        let nlen = e.name.iter().position(|&b| b == 0).unwrap_or(e.name.len());
        sys_write(&e.name[..nlen]);
        sys_write(b"\r\n");
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DirEnt {
    kind: u8,
    name: [u8; 32],
}

fn cmd_exec(args: &[u8]) {
    let path = trim(args);
    if path.is_empty() {
        sys_write(b"usage: exec <path>\r\n");
        return;
    }
    let pid = syscall4(SYS_EXEC, path.as_ptr() as u64, path.len() as u64, STDIO_INHERIT);
    if pid == 1 {
        sys_write(b"KazuOS: ");
        sys_write(path);
        sys_write(b": cannot execute driver directly\r\n");
    } else if pid == 0 || pid == u64::MAX {
        sys_write(b"exec failed: ");
        sys_write(path);
        sys_write(b"\r\n");
    } else {
        sys_write(b"spawned pid=");
        write_u64(pid);
        sys_write(b"\r\n");
    }
}

fn cmd_shutdown() -> ! {
    sys_write(b"Shutting down...\r\n");
    syscall1(SYS_SHUTDOWN, 0);
    loop {}
}

fn cmd_reboot() -> ! {
    sys_write(b"Rebooting...\r\n");
    syscall1(SYS_REBOOT, 0);
    loop {}
}

fn trim(s: &[u8]) -> &[u8] {
    let start = s.iter().position(|&b| b != b' ').unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&b| b != b' ')
        .map(|i| i + 1)
        .unwrap_or(start);
    &s[start..end]
}

fn write_u64(mut n: u64) {
    if n == 0 {
        sys_write(b"0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut i = 20;
    while n > 0 {
        i -= 1;
        digits[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    sys_write(&digits[i..]);
}

fn sys_write(buf: &[u8]) {
    if pipe_mode() {
        sys_write_fd(1, buf); // GUI terminal: stdout is a pipe
    } else {
        // Console shell: write straight to the console.
        syscall3(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, buf.len() as u64);
    }
}

fn syscall1(n: u64, a0: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0,
            in("rsi") 0u64,
            in("rdx") 0u64,
        );
    }
    r
}

fn syscall2(n: u64, a0: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0,
            in("rsi") 0u64,
            in("rdx") 0u64,
        );
    }
    r
}

fn syscall3(n: u64, a0: u64, a1: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") 0u64,
        );
    }
    r
}

fn syscall4(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
        );
    }
    r
}


