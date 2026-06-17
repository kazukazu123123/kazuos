#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");
// v3: per-core CPU + clean framebuffer hand-off

const NAME_LEN:    usize = 32;
const MAX_PROCS:   usize = 25;
const MAX_THREADS: usize = 64; // total threads tracked across all processes (for %CPU deltas)
const MAX_TPP:     usize = 16; // threads enumerated per process
const MAX_CPUS:    usize = 16;

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

#[repr(C)]
struct ThreadInfo {
    tid: u64, pid: u64, state: u64, cpu_ticks: u64, assigned_cpu: u64,
}

const EMPTY_TINFO: ThreadInfo = ThreadInfo {
    tid: 0, pid: 0, state: 0, cpu_ticks: 0, assigned_cpu: 0,
};

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let mut prev_ker:   u64 = 0;
    let mut prev_idle:  u64 = 0;
    let mut prev_user:  u64 = 0;
    let mut prev_idle_cpu:  [u64; MAX_CPUS] = [0; MAX_CPUS];
    let mut prev_ker_cpu:   [u64; MAX_CPUS] = [0; MAX_CPUS];
    let mut prev_user_cpu:  [u64; MAX_CPUS] = [0; MAX_CPUS];
    let mut prev_pids:  [u64; MAX_PROCS]  = [0; MAX_PROCS];
    let mut prev_ticks: [u64; MAX_PROCS]  = [0; MAX_PROCS];
    let mut prev_n:     usize = 0;
    let mut prev_tids:   [u64; MAX_THREADS] = [0; MAX_THREADS];
    let mut prev_tticks: [u64; MAX_THREADS] = [0; MAX_THREADS];
    let mut prev_tn:     usize = 0;

    // ktop is a plain text program: it draws with ANSI escape sequences over fd 1, so it
    // works on the console and inside a GUI terminal alike, and must NOT grab the
    // framebuffer. (Owning the framebuffer on the console suppresses the kernel's Ctrl+C
    // -> SIGINT path, which is gated on there being no framebuffer owner, so ktop could
    // never be interrupted.)
    syscall(SYS_SIGNAL_CATCH, 1, 0, 0);

    let cpu_count = syscall(SYS_CPU_INFO, 4, 0, 0) as usize;
    let cpu_count = cpu_count.min(MAX_CPUS);

    // Console size: height drives how many process rows fit (the rest scrolls); width
    // drives the bar length and whether the verbose usr/ker/idle breakdown fits. GUI
    // terminals are small (64x18), so everything must adapt rather than wrap.
    let console = syscall(SYS_CONSOLE_SIZE, 0, 0, 0);
    let rows = (console >> 32) as usize;
    let cols = { let c = (console & 0xffff_ffff) as usize; if c == 0 { 80 } else { c } };
    let wide = cols >= 72; // room for the per-source (usr/ker/idle) breakdown
    // Bar width: reserve room for label + "] " + percentage (+ breakdown when wide).
    let reserve = if wide { 44 } else { 18 };
    let bar_w = cols.saturating_sub(reserve).clamp(6, 30);
    // Lines used by the fixed header (title..column header) and footer.
    let header_lines = 7 + cpu_count;
    let footer_lines = 2;
    let visible = rows.saturating_sub(header_lines + footer_lines).max(1);
    let mut scroll: usize = 0;
    let mut total_rows: usize = 1; // actual list rows drawn last frame (for clamping)

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
        sys_write(b"KazuOS ktop  [Ctrl+C] quit\r\n");
        write_sep(cols);

        // Overall CPU bar
        let usr_p10 = pct10(du, dt);
        let ker_p10 = pct10(dk, dt);
        let idl_p10 = pct10(di, dt);
        sys_write(b"CPU ["); draw_bar(usr_p10 + ker_p10, 1000, bar_w); sys_write(b"] ");
        write_pct(usr_p10 + ker_p10);
        if wide {
            sys_write(b" usr:");  write_pct(usr_p10);
            sys_write(b" ker:");  write_pct(ker_p10);
            sys_write(b" idle:"); write_pct(idl_p10);
        }
        sys_write(b"\r\n");

        // Per-core usage
        sys_write(b"Cores:\r\n");
        let mut i = 0usize;
        while i < cpu_count {
            let ci = i as u64;
            let idle_c = syscall(SYS_CPU_INFO, 8, ci, 0);
            let ker_c  = syscall(SYS_CPU_INFO, 9, ci, 0);
            let user_c = syscall(SYS_CPU_INFO, 10, ci, 0);

            let dic = idle_c.saturating_sub(prev_idle_cpu[i]);
            let dkc = ker_c.saturating_sub(prev_ker_cpu[i]);
            let duc = user_c.saturating_sub(prev_user_cpu[i]);
            let dtc = dic + dkc + duc;

            let u_p10 = pct10(duc, dtc);
            let k_p10 = pct10(dkc, dtc);
            let i_p10 = pct10(dic, dtc);

            sys_write(b" C");
            write_u64(ci);
            sys_write(b" ["); draw_bar(u_p10 + k_p10, 1000, bar_w); sys_write(b"] ");
            write_pct(u_p10 + k_p10);
            if wide {
                sys_write(b" u:"); write_pct(u_p10);
                sys_write(b" k:"); write_pct(k_p10);
                sys_write(b" i:"); write_pct(i_p10);
            }
            sys_write(b"\r\n");

            prev_idle_cpu[i] = idle_c;
            prev_ker_cpu[i]  = ker_c;
            prev_user_cpu[i] = user_c;
            i += 1;
        }

        // Memory bar
        let mem_p10 = pct10(use_kib, tot_kib);
        sys_write(b"MEM ["); draw_bar(mem_p10, 1000, bar_w); sys_write(b"] ");
        write_pct(mem_p10);
        sys_write(b" ");
        write_mib(use_kib);
        sys_write(b"/");
        write_mib(tot_kib);
        sys_write(b"\r\n");

        write_sep(cols);
        sys_write(b"  PID STATE    %CPU    MEM   NAME\r\n");

        // Clamp scroll using the actual number of rows drawn last frame.
        let max_scroll = total_rows.saturating_sub(visible);
        if scroll > max_scroll { scroll = max_scroll; }
        let mut row_idx = 0usize; // index into the (kernel + users) list

        // kernel row (delta-based)
        let mut kinfo = EMPTY_INFO;
        let k_mem = if syscall(SYS_PROCESS_INFO, 0, &mut kinfo as *mut _ as u64, 0) == 0 {
            kinfo.memory_bytes / 1024
        } else {
            0
        };
        if row_idx >= scroll && row_idx < scroll + visible {
            write_row(0, b"Running", pct10_n(dk, dt, cpu_count as u64), k_mem, b"kernel");
        }
        row_idx += 1;

        // user processes
        let mut cur_pids:  [u64; MAX_PROCS] = [0; MAX_PROCS];
        let mut cur_ticks: [u64; MAX_PROCS] = [0; MAX_PROCS];
        let mut cur_n = 0usize;
        let mut cur_tids:   [u64; MAX_THREADS] = [0; MAX_THREADS];
        let mut cur_tticks: [u64; MAX_THREADS] = [0; MAX_THREADS];
        let mut cur_tn = 0usize;

        let mut pid = syscall(SYS_PROCESS_NEXT, 0, 0, 0);
        while pid != u64::MAX && cur_n < MAX_PROCS {
            let mut info = EMPTY_INFO;
            let r = syscall(SYS_PROCESS_INFO, pid, &mut info as *mut _ as u64, 0);
            if r == 0 {
                let prev_t = lookup_prev(&prev_pids, &prev_ticks, prev_n, pid);
                let dp = info.cpu_ticks.saturating_sub(prev_t);
                let p_p10 = pct10_n(dp, dt, cpu_count as u64);
                let state: &[u8] = match info.state {
                    1 | 2 => b"Running",
                    3 => b"Sleep  ",
                    _ => b"?      ",
                };
                let nlen = info.image_name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
                if row_idx >= scroll && row_idx < scroll + visible {
                    write_row(pid, state, p_p10, info.memory_bytes / 1024, &info.image_name[..nlen]);
                }
                row_idx += 1;
                cur_pids[cur_n]  = pid;
                cur_ticks[cur_n] = info.cpu_ticks;
                cur_n += 1;

                // Threads of this process. Only worth showing when there is more than one
                // (a single-threaded process is already fully described by its row above).
                let mut tids = [0u64; MAX_TPP];
                let mut tn = 0usize;
                let mut t = syscall(SYS_THREAD_NEXT, pid, 0, 0);
                while t != u64::MAX && tn < MAX_TPP {
                    tids[tn] = t;
                    tn += 1;
                    t = syscall(SYS_THREAD_NEXT, pid, t, 0);
                }
                if tn > 1 {
                    let mut k = 0usize;
                    while k < tn {
                        let mut tinfo = EMPTY_TINFO;
                        if syscall(SYS_THREAD_INFO, tids[k], &mut tinfo as *mut _ as u64, 0) == 0 {
                            let prev_tt =
                                lookup_prev(&prev_tids, &prev_tticks, prev_tn, tinfo.tid);
                            let dtp = tinfo.cpu_ticks.saturating_sub(prev_tt);
                            let tp10 = pct10_n(dtp, dt, cpu_count as u64);
                            let tstate: &[u8] = match tinfo.state {
                                1 | 2 => b"run ",
                                3 => b"slp ",
                                _ => b"?   ",
                            };
                            if row_idx >= scroll && row_idx < scroll + visible {
                                write_thread_row(tinfo.tid, tstate, tp10, tinfo.assigned_cpu);
                            }
                            row_idx += 1;
                            if cur_tn < MAX_THREADS {
                                cur_tids[cur_tn]   = tinfo.tid;
                                cur_tticks[cur_tn] = tinfo.cpu_ticks;
                                cur_tn += 1;
                            }
                        }
                        k += 1;
                    }
                }
            }
            pid = syscall(SYS_PROCESS_NEXT, pid, 0, 0);
        }

        // Remember the real row count and re-clamp so the footer is accurate.
        total_rows = row_idx;
        let max_scroll = total_rows.saturating_sub(visible);
        if scroll > max_scroll { scroll = max_scroll; }

        write_sep(cols);
        // Footer: scroll position / hint.
        sys_write(b"  ");
        write_u64((scroll + 1) as u64);
        sys_write(b"-");
        write_u64((scroll + visible).min(row_idx) as u64);
        sys_write(b"/");
        write_u64(row_idx as u64);
        sys_write(b"  [Up/Down] scroll  [Ctrl+C] quit\r\n");

        // update prev
        prev_ker   = ker;
        prev_idle  = idle;
        prev_user  = user;
        prev_pids  = cur_pids;
        prev_ticks = cur_ticks;
        prev_n     = cur_n;
        prev_tids   = cur_tids;
        prev_tticks = cur_tticks;
        prev_tn     = cur_tn;

        // wait ~500ms, check for Ctrl+C and arrow-key scrolling every 100ms
        let mut quit = false;
        let mut i = 0u32;
        while i < 5 {
            syscall(SYS_SLEEP, 100, SLEEP_UNIT_MS, 0);
            if syscall(SYS_SIGNAL_CHECK, 0, 0, 0) != 0 { quit = true; break; }
            // Drain pending keys from stdin (fd 0), not the console keyboard directly,
            // so this works both on the console and inside a GUI terminal (where the
            // compositor owns the keyboard and forwards keys down our stdin pipe).
            let mut moved = false;
            let mut kb = [0u8; 16];
            loop {
                let n = syscall(SYS_TRY_READ, 0, kb.as_mut_ptr() as u64, kb.len() as u64);
                if n == 0 || n == u64::MAX { break; }
                for &key in &kb[..n as usize] {
                    match key {
                        0x82 => { scroll = scroll.saturating_sub(1); moved = true; } // Up
                        0x83 => { scroll += 1; moved = true; }                       // Down
                        _ => {}
                    }
                }
            }
            if moved { break; }
            i += 1;
        }
        if quit { break; }
    }

    // Leave the screen clean for whatever drew before us (the shell reprints its prompt).
    sys_write(b"\x1b[2J\x1b[H");
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

fn lookup_prev(keys: &[u64], ticks: &[u64], n: usize, key: u64) -> u64 {
    let mut i = 0usize;
    while i < n {
        if keys[i] == key { return ticks[i]; }
        i += 1;
    }
    0
}

fn pct10(part: u64, total: u64) -> u64 {
    if total > 0 { (part * 1000 / total).min(1000) } else { 0 }
}

// Per-process CPU%, top-style: scaled so one fully-used core = 100% and the cap is
// cpu_count * 100% (e.g. 400% on 4 cores). `total` is the system-wide tick delta across
// all cores, so multiplying by `n` normalises back to per-core utilisation.
fn pct10_n(part: u64, total: u64, n: u64) -> u64 {
    if total > 0 { (part * 1000 * n / total).min(n * 1000) } else { 0 }
}

fn draw_bar(pct10: u64, max: u64, width: usize) {
    let filled = ((pct10 * width as u64 / max) as usize).min(width);
    let mut i = 0usize;
    while i < filled { sys_write(b"#"); i += 1; }
    while i < width  { sys_write(b" "); i += 1; }
}

// A separator line of dashes as wide as the terminal (capped so a huge width can't
// blow the small stack buffer).
fn write_sep(cols: usize) {
    let n = cols.min(96);
    let dashes = [b'-'; 96];
    sys_write(&dashes[..n]);
    sys_write(b"\r\n");
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

fn write_thread_row(tid: u64, state: &[u8], pct10: u64, cpu: u64) {
    sys_write(b"      `- t");
    write_padded(tid, 3);
    sys_write(b"  ");
    sys_write(state);
    sys_write(b"  ");
    write_padded(pct10 / 10, 3);
    sys_write(b".");
    sys_write(&[b'0' + (pct10 % 10) as u8]);
    sys_write(b"%  cpu");
    write_u64(cpu);
    sys_write(b"\r\n");
}

fn sys_write(buf: &[u8]) {
    syscall(SYS_WRITE, 1, buf.as_ptr() as u64, buf.len() as u64);
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
