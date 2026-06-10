#![no_std]
#![no_main]
// v2: blocking-syscall aware
include!("../../crates/kernel/src/syscall_numbers.rs");

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

const EMPTY_INFO: ProcessInfo = ProcessInfo {
    pid: 0, state: 0, image_name: [0u8; NAME_LEN],
    start_tsc: 0, entry: 0, stack_top: 0, step: 0,
    cpu_ticks: 0, memory_bytes: 0,
};

#[no_mangle]
pub extern "C" fn _start(_argc: u64, _argv: u64) -> ! {
    let kernel_ticks = syscall(SYS_CPU_INFO, 2, 0, 0);
    let idle_ticks   = syscall(SYS_CPU_INFO, 3, 0, 0);
    let user_ticks   = syscall(SYS_CPU_INFO, 1, 0, 0);
    let grand_total  = kernel_ticks + idle_ticks + user_ticks;

    let mem_info     = syscall(SYS_MEM_INFO, 0, 0, 0);
    let used_kib     = mem_info & 0xffff_ffff;

    sys_write(b"  PID  STATE    %CPU     MEM     NAME\r\n");

    // collect user process rows first so we can subtract their memory from used_kib
    let mut user_mem_kib: u64 = 0;
    let mut pid = syscall(SYS_PROCESS_NEXT, 0, 0, 0);
    while pid != 0 {
        let mut info = EMPTY_INFO;
        let r = syscall(SYS_PROCESS_INFO, pid, &mut info as *mut _ as u64, 0);
        if r == 0 {
            user_mem_kib = user_mem_kib.saturating_add(info.memory_bytes / 1024);
        }
        pid = syscall(SYS_PROCESS_NEXT, pid, 0, 0);
    }
    let kernel_mem_kib = used_kib.saturating_sub(user_mem_kib);

    // kernel row
    let k_pct = pct10(kernel_ticks, grand_total);
    write_row(0, b"Running", k_pct, kernel_mem_kib, b"kernel");

    // user processes
    let mut pid = syscall(SYS_PROCESS_NEXT, 0, 0, 0);
    while pid != 0 {
        let mut info = EMPTY_INFO;
        let r = syscall(SYS_PROCESS_INFO, pid, &mut info as *mut _ as u64, 0);
        if r == 0 {
            let state_str: &[u8] = match info.state {
                1 | 2 => b"Running",
                3 => b"Sleep  ",
                _ => b"?      ",
            };
            let p_pct = pct10(info.cpu_ticks, grand_total);
            let name_len = info.image_name.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
            write_row(pid, state_str, p_pct, info.memory_bytes / 1024, &info.image_name[..name_len]);
        }
        pid = syscall(SYS_PROCESS_NEXT, pid, 0, 0);
    }

    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

fn pct10(ticks: u64, total: u64) -> u64 {
    if total > 0 { (ticks * 1000 / total).min(999) } else { 0 }
}

fn write_row(pid: u64, state: &[u8], pct10: u64, mem_kib: u64, name: &[u8]) {
    sys_write(b"  ");
    write_u64_w4(pid);
    sys_write(b"  ");
    sys_write(state);
    sys_write(b"  ");
    write_u64_w3(pct10 / 10);
    sys_write(b".");
    write_digit(pct10 % 10);
    sys_write(b"%  ");
    write_u64_w6(mem_kib);
    sys_write(b"KiB  ");
    sys_write(name);
    sys_write(b"\r\n");
}

fn sys_write(buf: &[u8]) {
    syscall(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
}

fn write_digit(d: u64) {
    sys_write(&[b'0' + (d % 10) as u8]);
}

fn write_u64(mut n: u64) {
    if n == 0 { sys_write(b"0"); return; }
    let mut buf = [0u8; 20];
    let mut i = 20usize;
    while n > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    sys_write(&buf[i..]);
}

fn write_u64_w4(n: u64) { write_padded(n, 4); }
fn write_u64_w3(n: u64) { write_padded(n, 3); }
fn write_u64_w6(n: u64) { write_padded(n, 6); }

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
