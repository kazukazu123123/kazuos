#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

const SAMPLE_RATE: u32 = 48000;
const AMPLITUDE: i16 = 8000;
const CHUNK_SAMPLES: usize = 1200; // 25ms @ 48kHz stereo
const CHUNK_BYTES: usize = CHUNK_SAMPLES * 2 * 2; // s16le stereo
const CHUNKS_TO_PLAY: usize = 120; // 3 seconds

fn sys_ioctl(fd: u64, cmd: u64, arg: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_IOCTL => r,
            in("rdi") fd,
            in("rsi") cmd,
            in("rdx") arg,
        );
    }
    r
}

/// Fixed-point sine oscillator for 440 Hz @ 48 kHz.
/// Recurrence: y[n] = A*y[n-1] - y[n-2],  A = 2*cos(w).
/// cos(2*pi*440/48000) ~= 0.998341698,  A/2 in Q30 ~= 1_071_961_363.
fn sine_sample() -> i16 {
    const A_OVER_2_Q30: i64 = 1_071_961_363i64;
    const SIN_W_AMP: i32 = 460; // AMPLITUDE * sin(2*pi*440/48000)

    static mut PREV: i32 = -SIN_W_AMP;
    static mut CURR: i32 = 0;

    unsafe {
        // next = A*curr - prev  (A = 2*(A/2))
        let next = ((2 * A_OVER_2_Q30 * CURR as i64) >> 30) as i32 - PREV;
        PREV = CURR;
        CURR = next;
        next as i16
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let fd = sys_open(b"/dev/audio");
    if fd == u64::MAX {
        println!("hdatest: /dev/audio not found");
        sys_exit(1);
    }

    println!("hdatest: playing 440Hz sine wave for {} chunks", CHUNKS_TO_PLAY);

    let mut buf = alloc::vec![0u8; CHUNK_BYTES];

    for _ in 0..CHUNKS_TO_PLAY {
        // Fill one chunk with sine wave.
        for i in 0..CHUNK_SAMPLES {
            let sample = sine_sample();
            let off = i * 4;
            buf[off..off + 2].copy_from_slice(&sample.to_le_bytes());
            buf[off + 2..off + 4].copy_from_slice(&sample.to_le_bytes());
        }

        // Write to /dev/audio; block on ioctl(3) until space is available.
        let mut written = 0usize;
        while written < CHUNK_BYTES {
            let n = sys_write_fd(fd, &buf[written..]);
            if n == u64::MAX {
                println!("hdatest: write error");
                sys_close(fd);
                sys_exit(1);
            }
            if n == 0 {
                sys_ioctl(fd, 3, 0);
                continue;
            }
            written += n as usize;
        }
    }

    sys_close(fd);
    println!("hdatest: done");
    sys_exit(0);
}
