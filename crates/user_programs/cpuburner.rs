#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

use core::sync::atomic::{AtomicBool, Ordering};

// cpuburner — saturate CPUs using worker threads.
//
//   cpuburner          burn with 1 thread until Ctrl+C
//   cpuburner <N>      burn with N threads (one per core to use all of them)
//
// All burning happens in spawned threads of this process; Ctrl+C sets a shared stop
// flag, the threads fall out of their loops, and we join them.

const MAX_THREADS: usize = 64;

static STOP: AtomicBool = AtomicBool::new(false);

fn burn() {
    let mut acc: u64 = 0x1234_5678_9ABC_DEF0;
    let mut i: u64 = 0;
    loop {
        // Burn a chunk, then check the stop flag (cheap relative to the chunk).
        for _ in 0..2_000_000u64 {
            acc = acc.wrapping_mul(6364136223846793005).wrapping_add(i);
            acc ^= acc >> 29;
            core::hint::black_box(acc);
            i = i.wrapping_add(1);
        }
        if STOP.load(Ordering::Relaxed) {
            break;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    let n = match nth_arg(argc, argv, 0) {
        Some(a) => parse_u64(a).clamp(1, MAX_THREADS as u64) as usize,
        None => 1,
    };

    sys_write(b"cpuburner: burning with ");
    write_u64(n as u64);
    sys_write(b" thread(s) - press Ctrl+C to stop\r\n");

    // Catch Ctrl+C so we stop the threads cleanly instead of being killed.
    syscall(SYS_SIGNAL_CATCH, 1, 0, 0);

    let mut handles: alloc::vec::Vec<JoinHandle> = alloc::vec::Vec::new();
    for _ in 0..n {
        if let Some(h) = thread_create(burn) {
            handles.push(h);
        }
    }

    // Foreground: idle until Ctrl+C (the burning is in the threads).
    loop {
        if syscall(SYS_SIGNAL_CHECK, 0, 0, 0) == 1 {
            break;
        }
        syscall(SYS_SLEEP, 50, SLEEP_UNIT_MS, 0);
    }

    // Stop the burn loops and wait for the threads to finish.
    STOP.store(true, Ordering::Relaxed);
    for h in handles {
        h.join();
    }

    sys_write(b"\r\ncpuburner: stopped\r\n");
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
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
    syscall(SYS_WRITE, 1, buf.as_ptr() as u64, buf.len() as u64);
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
