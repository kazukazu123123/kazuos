#![no_std]
#![no_main]

include!("../../crates/kernel/src/syscall_numbers.rs");

const AUDIO_PATH: &[u8] = b"/dev/audio";
const WAV_PATH: &[u8] = b"/audio/test.wav";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    sys_write(b"wavtestplay: starting\r\n");
    let audio_fd = open(AUDIO_PATH);
    if audio_fd == u64::MAX {
        sys_write(b"wavtestplay: failed to open /dev/audio\r\n");
        syscall(SYS_EXIT, 0, 0, 0);
        loop {}
    }
    sys_write(b"wavtestplay: /dev/audio opened\r\n");

    let file_fd = open(WAV_PATH);
    if file_fd == u64::MAX {
        sys_write(b"wavtestplay: failed to open /audio/test.wav\r\n");
        close(audio_fd);
        syscall(SYS_EXIT, 0, 0, 0);
        loop {}
    }
    sys_write(b"wavtestplay: /audio/test.wav opened\r\n");

    let mut header = [0u8; 512];
    let n = read(file_fd, &mut header);
    if n < 44 {
        sys_write(b"wavtestplay: wav too small\r\n");
        close(file_fd);
        close(audio_fd);
        syscall(SYS_EXIT, 0, 0, 0);
        loop {}
    }

    let Some(wav) = parse_wav_header(&header[..n as usize]) else {
        sys_write(b"wavtestplay: invalid wav header\r\n");
        close(file_fd);
        close(audio_fd);
        syscall(SYS_EXIT, 0, 0, 0);
        loop {}
    };
    sys_write(b"wavtestplay: wav parsed ok\r\n");

    ioctl(audio_fd, 0, wav.sample_rate as u64);

    let mut skipped = n as usize;
    while skipped < wav.data_offset {
        let need = (wav.data_offset - skipped).min(512);
        let got = read(file_fd, &mut header[..need]);
        if got == 0 {
            break;
        }
        skipped += got as usize;
    }

    let mut pcm_buf = [0u8; 4096];
    let frame_size = wav.channels as usize * (wav.bits as usize / 8);
    let mut remaining = wav.data_len;
    while remaining > 0 {
        let to_read = remaining.min(1024 * frame_size);
        let got = read(file_fd, &mut pcm_buf[..to_read]);
        if got == 0 {
            break;
        }
        let out_buf: &[u8] = if wav.channels == 1 && wav.bits == 16 {
            let frames = got as usize / 2;
            let mut out = [0u8; 4096];
            let mut j = 0;
            for i in 0..frames {
                let sample = i16::from_le_bytes([
                    pcm_buf[i * 2],
                    pcm_buf[i * 2 + 1],
                ]);
                out[j..j + 2].copy_from_slice(&sample.to_le_bytes());
                out[j + 2..j + 4].copy_from_slice(&sample.to_le_bytes());
                j += 4;
            }
            // Write in a loop to handle partial writes.
            let mut offset = 0usize;
            while offset < j {
                let n = write(audio_fd, &out[offset..j]);
                if n == 0 {
                    syscall(SYS_SLEEP, 1, SLEEP_UNIT_MS, 0);
                    continue;
                }
                offset += n as usize;
            }
            remaining -= got as usize;
            continue;
        } else if wav.bits == 8 {
            let frames = got as usize / (wav.channels as usize);
            let mut out = [0u8; 4096];
            let mut j = 0;
            for i in 0..frames {
                let left = if wav.channels == 1 {
                    ((pcm_buf[i] as i16) - 128) << 8
                } else {
                    ((pcm_buf[i * 2] as i16) - 128) << 8
                };
                let right = if wav.channels == 1 {
                    left
                } else {
                    ((pcm_buf[i * 2 + 1] as i16) - 128) << 8
                };
                out[j..j + 2].copy_from_slice(&left.to_le_bytes());
                out[j + 2..j + 4].copy_from_slice(&right.to_le_bytes());
                j += 4;
            }
            let mut offset = 0usize;
            while offset < j {
                let n = write(audio_fd, &out[offset..j]);
                if n == 0 {
                    syscall(SYS_SLEEP, 1, SLEEP_UNIT_MS, 0);
                    continue;
                }
                offset += n as usize;
            }
            remaining -= got as usize;
            continue;
        } else {
            &pcm_buf[..got as usize]
        };
        let mut offset = 0usize;
        while offset < out_buf.len() {
            let n = write(audio_fd, &out_buf[offset..]);
            if n == 0 {
                syscall(SYS_SLEEP, 1, SLEEP_UNIT_MS, 0);
                continue;
            }
            offset += n as usize;
        }
        remaining -= got as usize;
    }

    close(file_fd);
    sys_write(b"wavtestplay: draining audio...\r\n");
    ioctl(audio_fd, 1, 0);
    close(audio_fd);
    sys_write(b"wavtestplay: playback finished\r\n");
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

struct WavInfo {
    sample_rate: u32,
    channels: u16,
    bits: u16,
    data_offset: usize,
    data_len: usize,
}

fn parse_wav_header(data: &[u8]) -> Option<WavInfo> {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let mut offset = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data_start = 0usize;
    let mut data_len = 0usize;
    while offset + 8 <= data.len() {
        let id = &data[offset..offset + 4];
        let len = le_u32(data, offset + 4) as usize;
        offset += 8;
        if id == b"fmt " {
            if offset + 16 > data.len() || le_u16(data, offset) != 1 {
                return None;
            }
            channels = le_u16(data, offset + 2);
            sample_rate = le_u32(data, offset + 4);
            bits = le_u16(data, offset + 14);
        } else if id == b"data" {
            data_start = offset;
            data_len = len;
        }
        offset = offset.wrapping_add((len + 1) & !1);
        if offset < 8 || offset > data.len() {
            break;
        }
    }
    if !(channels == 1 || channels == 2)
        || !(bits == 8 || bits == 16)
        || data_start == 0
        || data_len == 0
    {
        return None;
    }
    Some(WavInfo {
        sample_rate,
        channels,
        bits,
        data_offset: data_start,
        data_len,
    })
}

fn le_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

fn le_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn open(path: &[u8]) -> u64 {
    syscall(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0)
}

fn close(fd: u64) -> u64 {
    syscall(SYS_CLOSE, fd, 0, 0)
}

fn read(fd: u64, buf: &mut [u8]) -> u64 {
    syscall(SYS_READ, fd, buf.as_mut_ptr() as u64, buf.len() as u64)
}

fn write(fd: u64, buf: &[u8]) -> u64 {
    syscall(SYS_WRITE, fd, buf.as_ptr() as u64, buf.len() as u64)
}

fn ioctl(fd: u64, cmd: u64, arg: u64) -> u64 {
    syscall(SYS_IOCTL, fd, cmd, arg)
}

fn sys_write(buf: &[u8]) {
    syscall(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
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
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
