#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

// keyboard.rs extended key codes
const KEY_UP:    u8 = 0x82;
const KEY_DOWN:  u8 = 0x83;
const BUF_SIZE: usize = 65536;

static mut INPUT_BUF: [u8; BUF_SIZE] = [0u8; BUF_SIZE];
static mut INPUT_LEN: usize = 0;
static mut LINE_STARTS: [u32; 4096] = [0u32; 4096];
static mut LINE_COUNT: usize = 0;

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    // Read all of stdin
    unsafe {
        let mut total = 0usize;
        loop {
            if total >= BUF_SIZE { break; }
            let n = syscall(SYS_READ, 0, core::ptr::addr_of_mut!(INPUT_BUF).cast::<u8>().add(total) as u64, (BUF_SIZE - total) as u64);
            if n == 0 || n == u64::MAX { break; }
            total += n as usize;
        }
        *core::ptr::addr_of_mut!(INPUT_LEN) = total;
    }

    // Build line index
    unsafe {
        let data = core::slice::from_raw_parts(core::ptr::addr_of!(INPUT_BUF).cast::<u8>(), INPUT_LEN);
        let mut lc = 0usize;
        *core::ptr::addr_of_mut!(LINE_STARTS[0]) = 0;
        lc = 1;
        let mut i = 0usize;
        while i < data.len() && lc < 4096 {
            if data[i] == b'\n' && i + 1 < data.len() {
                *core::ptr::addr_of_mut!(LINE_STARTS[lc]) = (i + 1) as u32;
                lc += 1;
            }
            i += 1;
        }
        *core::ptr::addr_of_mut!(LINE_COUNT) = lc;
    }

    let total_lines = unsafe { LINE_COUNT };
    if total_lines == 0 {
        sys_write(b"(empty)\r\n");
        syscall(SYS_EXIT, 0, 0, 0);
        loop {}
    }

    let console_size = syscall(SYS_CONSOLE_SIZE, 0, 0, 0);
    let rows = {
        let r = (console_size >> 32) as usize;
        if r >= 2 { r - 1 } else { 23 }
    };

    let max_top = total_lines.saturating_sub(rows);
    let mut top: usize = 0;

    loop {
        redraw(top, total_lines, rows);

        // Wait for a key
        let key = loop {
            let k = syscall(SYS_KEYBOARD_POLL, 0, 0, 0);
            if k != 0 { break k as u8; }
            syscall(SYS_SLEEP, 30, SLEEP_UNIT_MS, 0);
        };

        match key {
            b'q' | b'Q' | 0x03 /* Ctrl-C */ => {
                sys_write(b"\x1b[2J\x1b[H");
                syscall(SYS_EXIT, 0, 0, 0);
                loop {}
            }
            b' ' | b'f' | b'F' => {
                top = (top + rows).min(max_top);
            }
            b'b' | b'B' => {
                top = top.saturating_sub(rows);
            }
            KEY_DOWN | b'j' | b'\r' | b'\n' => {
                top = (top + 1).min(max_top);
            }
            KEY_UP | b'k' => {
                top = top.saturating_sub(1);
            }
            _ => {}
        }
    }
}

fn redraw(top: usize, total_lines: usize, rows: usize) {
    sys_write(b"\x1b[2J\x1b[H");

    let end = (top + rows).min(total_lines);
    for line_idx in top..end {
        print_line(line_idx);
    }

    // Status bar
    let at_end = end >= total_lines;
    let pct = if total_lines <= rows { 100u64 } else { (end as u64 * 100) / total_lines as u64 };
    sys_write(b"\x1b[7m");
    if at_end {
        sys_write(b" (END)");
    }
    sys_write(b"  lines ");
    write_u64(end as u64);
    sys_write(b"/");
    write_u64(total_lines as u64);
    sys_write(b"  ");
    write_u64(pct);
    sys_write(b"%  q:quit  space/b:page  j/k:line \x1b[0m");
}

fn print_line(line_idx: usize) {
    unsafe {
        let start = (*core::ptr::addr_of!(LINE_STARTS).cast::<u32>().add(line_idx)) as usize;
        let data = core::slice::from_raw_parts(core::ptr::addr_of!(INPUT_BUF).cast::<u8>(), INPUT_LEN);
        let end = data[start..].iter().position(|&b| b == b'\n')
            .map(|p| start + p)
            .unwrap_or(INPUT_LEN);
        let line_end = if end > start && data[end - 1] == b'\r' { end - 1 } else { end };
        sys_write(&data[start..line_end]);
        sys_write(b"\r\n");
    }
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


