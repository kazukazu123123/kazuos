#![no_std]
#![no_main]
// v2: blocking-syscall aware
include!("../../crates/kernel/src/syscall_numbers.rs");

const NAME_LEN:  usize = 32;
const MAX_PROCS: usize = 32;
const BAR_WIDTH: usize = 30;

#[repr(C)]
struct ProcessInfo {
    pid: u64, state: u64,
    image_name: [u8; NAME_LEN],
    start_tsc: u64, entry: u64, stack_top: u64, step: u64,
    cpu_ticks: u64, memory_bytes: u64,
}

const EMPTY_INFO: ProcessInfo = ProcessInfo {
    pid: 0, state: 0, image_name: [0u8; NAME_LEN],
    start_tsc: 0, entry: 0, stack_top: 0, step: 0,
    cpu_ticks: 0, memory_bytes: 0,
};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut prev_ker:   u64 = 0;
    let mut prev_idle:  u64 = 0;
    let mut prev_user:  u64 = 0;
    let mut prev_pids:  [u64; MAX_PROCS]  = [0; MAX_PROCS];
    let mut prev_ticks: [u64; MAX_PROCS]  = [0; MAX_PROCS];
    let mut prev_n:     usize = 0;

    syscall(SYS_FB_ACQUIRE, 0, 0, 0);
    syscall(SYS_SIGNAL_CATCH, 1, 0, 0);

    loop {
        // --- sample ---
        let ker  = syscall(SYS_CPU_INFO, 2, 0, 0);
        let idle = syscall(SYS_CPU_INFO, 3, 0, 0);
        let user = syscall(SYS_CPU_INFO, 1, 0, 0);

        let dk = ker.saturating_sub(prev_ker);
        let di = idle.saturating_sub(prev_idle);
        let du = user.saturating_sub(prev_user);
        let dt = dk + di + du;

        let mem     = syscall(SYS_MEM_INFO, 0, 0, 0);
        let tot_kib = mem >> 32;
        let use_kib = mem & 0xffff_ffff;

        // --- draw ---
        sys_write(b"\x1b[2J\x1b[H");
        sys_write(b"KazuOS ktop                              [q] quit\r\n");
        sys_write(b"--------------------------------------------------\r\n");

        // CPU bars
        let usr_p10 = pct10(du, dt);
        let ker_p10 = pct10(dk, dt);
        let idl_p10 = pct10(di, dt);
        sys_write(b"CPU  ["); draw_bar(usr_p10 + ker_p10, 1000); sys_write(b"] ");
        write_pct(usr_p10 + ker_p10);
        sys_write(b"   usr:");  write_pct(usr_p10);
        sys_write(b" ker:");    write_pct(ker_p10);
        sys_write(b" idle:");   write_pct(idl_p10);
        sys_write(b"\r\n");

        // Memory bar
        let mem_p10 = pct10(use_kib, tot_kib);
        sys_write(b"MEM  ["); draw_bar(mem_p10, 1000); sys_write(b"] ");
        write_pct(mem_p10);
        sys_write(b"   ");
        write_mib(use_kib);
        sys_write(b" / ");
        write_mib(tot_kib);
        sys_write(b"\r\n");

        sys_write(b"--------------------------------------------------\r\n");
        sys_write(b"  PID  STATE    %CPU     MEM     NAME\r\n");

        // kernel row (delta-based)
        write_row(0, b"Running", ker_p10, 0, b"kernel");

        // user processes
        let mut cur_pids:  [u64; MAX_PROCS] = [0; MAX_PROCS];
        let mut cur_ticks: [u64; MAX_PROCS] = [0; MAX_PROCS];
        let mut cur_n = 0usize;

        let mut pid = syscall(SYS_PROCESS_NEXT, 0, 0, 0);
        while pid != 0 && cur_n < MAX_PROCS {
            let mut info = EMPTY_INFO;
            let r = syscall(SYS_PROCESS_INFO, pid, &mut info as *mut _ as u64, 0);
            if r == 0 {
                let prev_t = lookup_prev(&prev_pids, &prev_ticks, prev_n, pid);
                let dp = info.cpu_ticks.saturating_sub(prev_t);
                let p_p10 = pct10(dp, dt);
                let state: &[u8] = match info.state {
                    1 | 2 => b"Running",
                    3 => b"Sleep  ",
                    _ => b"?      ",
                };
                let nlen = info.image_name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
                write_row(pid, state, p_p10, info.memory_bytes / 1024, &info.image_name[..nlen]);
                cur_pids[cur_n]  = pid;
                cur_ticks[cur_n] = info.cpu_ticks;
                cur_n += 1;
            }
            pid = syscall(SYS_PROCESS_NEXT, pid, 0, 0);
        }

        sys_write(b"--------------------------------------------------\r\n");

        // update prev
        prev_ker   = ker;
        prev_idle  = idle;
        prev_user  = user;
        prev_pids  = cur_pids;
        prev_ticks = cur_ticks;
        prev_n     = cur_n;

        // wait ~500ms, check for Ctrl+C every 100ms
        let mut quit = false;
        let mut i = 0u32;
        while i < 5 {
            syscall(SYS_NAP_MS, 100, 0, 0);
            if syscall(SYS_SIGNAL_CHECK, 0, 0, 0) != 0 { quit = true; break; }
            i += 1;
        }
        if quit { break; }
    }

    sys_write(b"\x1b[2J\x1b[H");
    syscall(SYS_FB_RELEASE, 0, 0, 0);
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

fn lookup_prev(pids: &[u64; MAX_PROCS], ticks: &[u64; MAX_PROCS], n: usize, pid: u64) -> u64 {
    let mut i = 0usize;
    while i < n {
        if pids[i] == pid { return ticks[i]; }
        i += 1;
    }
    0
}

fn pct10(part: u64, total: u64) -> u64 {
    if total > 0 { (part * 1000 / total).min(1000) } else { 0 }
}

fn draw_bar(pct10: u64, max: u64) {
    let filled = (pct10 * BAR_WIDTH as u64 / max) as usize;
    let filled = filled.min(BAR_WIDTH);
    let mut i = 0usize;
    while i < filled        { sys_write(b"#"); i += 1; }
    while i < BAR_WIDTH     { sys_write(b" "); i += 1; }
}

fn write_pct(p10: u64) {
    write_padded(p10 / 10, 3);
    sys_write(b".");
    sys_write(&[b'0' + (p10 % 10) as u8]);
    sys_write(b"%");
}

fn write_mib(kib: u64) {
    write_u64(kib / 1024);
    sys_write(b"MiB");
}

fn write_row(pid: u64, state: &[u8], pct10: u64, mem_kib: u64, name: &[u8]) {
    sys_write(b"  ");
    write_padded(pid, 4);
    sys_write(b"  ");
    sys_write(state);
    sys_write(b"  ");
    write_padded(pct10 / 10, 3);
    sys_write(b".");
    sys_write(&[b'0' + (pct10 % 10) as u8]);
    sys_write(b"%  ");
    write_padded(mem_kib, 6);
    sys_write(b"KiB  ");
    sys_write(name);
    sys_write(b"\r\n");
}

fn sys_write(buf: &[u8]) {
    syscall(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
}

fn write_u64(mut n: u64) {
    if n == 0 { sys_write(b"0"); return; }
    let mut buf = [0u8; 20];
    let mut i = 20usize;
    while n > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    sys_write(&buf[i..]);
}

fn write_padded(n: u64, width: usize) {
    let mut buf = [b' '; 20];
    if n == 0 {
        buf[width - 1] = b'0';
    } else {
        let mut v = n;
        let mut i = width;
        while v > 0 && i > 0 { i -= 1; buf[i] = b'0' + (v % 10) as u8; v /= 10; }
    }
    sys_write(&buf[..width]);
}

fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0, in("rsi") a1, in("rdx") a2,
        );
    }
    r
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
