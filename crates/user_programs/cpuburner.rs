#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

// cpuburner — saturate the CPU.
//
//   cpuburner          burn one core in the foreground until Ctrl+C
//   cpuburner <N>      spawn N worker processes (one per extra core) plus burn
//                      in the foreground; Ctrl+C stops everything
//   cpuburner worker   pure burn loop; meant to be spawned + killed by a parent

const MAX_WORKERS: usize = 64;

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    let arg0 = nth_arg(argc, argv, 0);

    // Worker mode: just burn until the parent kills us. No signal handling.
    if matches!(arg0, Some(a) if eq(a, b"worker")) {
        burn_forever();
    }

    // Determine how many worker processes to spawn (0 = foreground only).
    let workers = match arg0 {
        Some(a) => parse_u64(a).min(MAX_WORKERS as u64) as usize,
        None => 0,
    };

    let mut pids = [0u64; MAX_WORKERS];
    for slot in pids.iter_mut().take(workers) {
        // "/bin/cpuburner\0worker\0\0"
        let spec = b"/bin/cpuburner\0worker\0\0";
        let pid = syscall(SYS_EXEC, spec.as_ptr() as u64, spec.len() as u64, 0xFFFF_FFFF);
        *slot = pid;
    }

    sys_write(b"cpuburner: burning CPU");
    if workers > 0 {
        sys_write(b" (");
        write_u64(workers as u64);
        sys_write(b" worker(s) + foreground)");
    }
    sys_write(b" - press Ctrl+C to stop\r\n");

    // Catch Ctrl+C so we can clean up the workers instead of being killed.
    syscall(SYS_SIGNAL_CATCH, 1, 0, 0);

    // Foreground burn loop, periodically checking for Ctrl+C.
    loop {
        let mut acc: u64 = 0x9E3779B97F4A7C15;
        for i in 0..50_000_000u64 {
            // Mix of arithmetic to keep the ALU busy and resist being optimised out.
            acc = acc.wrapping_mul(6364136223846793005).wrapping_add(i);
            acc ^= acc >> 33;
            core::hint::black_box(acc);
        }
        core::hint::black_box(acc);
        if syscall(SYS_SIGNAL_CHECK, 0, 0, 0) == 1 {
            break;
        }
    }

    // Tear down workers.
    for &pid in pids.iter().take(workers) {
        if pid != 0 && pid != u64::MAX {
            syscall(SYS_KILL, pid, 0, 0);
        }
    }

    sys_write(b"\r\ncpuburner: stopped\r\n");
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

fn burn_forever() -> ! {
    let mut acc: u64 = 0x1234_5678_9ABC_DEF0;
    let mut i: u64 = 0;
    loop {
        acc = acc.wrapping_mul(6364136223846793005).wrapping_add(i);
        acc ^= acc >> 29;
        core::hint::black_box(acc);
        i = i.wrapping_add(1);
    }
}

/// Return argv[n] as a byte slice, or None if out of range.
fn nth_arg(argc: u64, argv: u64, n: usize) -> Option<&'static [u8]> {
    if argv == 0 || (n as u64) >= argc {
        return None;
    }
    unsafe {
        let ptr = *((argv as *const u64).add(n));
        if ptr == 0 {
            return None;
        }
        let mut len = 0usize;
        while *((ptr as *const u8).add(len)) != 0 {
            len += 1;
        }
        Some(core::slice::from_raw_parts(ptr as *const u8, len))
    }
}

fn eq(a: &[u8], b: &[u8]) -> bool {
    a == b
}

fn parse_u64(s: &[u8]) -> u64 {
    let mut n = 0u64;
    for &c in s {
        if !c.is_ascii_digit() {
            break;
        }
        n = n.wrapping_mul(10).wrapping_add((c - b'0') as u64);
    }
    n
}

fn sys_write(buf: &[u8]) {
    syscall(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
}

fn write_u64(mut n: u64) {
    if n == 0 {
        sys_write(b"0");
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 20usize;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
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
