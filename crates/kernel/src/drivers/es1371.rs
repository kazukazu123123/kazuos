use crate::drivers::pci::{self, Device, ScanKind};
use crate::util::{SyncUnsafeCell, ind, outd};
use alloc::alloc::{Layout, alloc_zeroed, dealloc};
use alloc::vec::Vec;

const VENDOR_ENSONIQ: u16 = 0x1274;
const DEVICE_ES1371: u16 = 0x1371;

const REG_CTRL: u16 = 0x00;
const REG_STAT: u16 = 0x04;
const REG_MEM_PAGE: u16 = 0x0C;
const REG_SRC_IF: u16 = 0x10;
const REG_CODEC: u16 = 0x14;
const REG_SER_IF: u16 = 0x20;
const REG_P2_SAMP_CT: u16 = 0x28;
// Page 0x0C mapped:
const REG_P2_BUF_ADDR: u16 = 0x38;
const REG_P2_BUF_DEF: u16 = 0x3C;

const CTRL_DAC2_EN: u32 = 1 << 5;
const CTRL_CDC_EN: u32 = 1 << 11;  // enable AC97 codec interface (ES1371)
const CTRL_BREQ: u32 = 1 << 13;    // DMA bus master request enable (required for DMA)
const CTRL_SYNC_RES: u32 = 1 << 14; // AC97 warm reset

const STAT_P2INT: u32 = 1 << 1;    // DAC2 done interrupt flag
const STAT_CSTAT: u32 = 1 << 29;   // codec ready

// ES1371 SCTRL P2 format field is at bits [5:4] (ES_FMT_16BIT|ES_FMT_STEREO).
// ES1370 used bits [3:2] — do not confuse the two chips.
const SER_P2_16BIT_STEREO: u32 = 0x3 << 4; // bits [5:4] = 0x30
const SER_P2_LOOP_SEL: u32 = 1 << 14;      // 1 = stop mode

const SRC_BUSY: u32 = 1 << 23;
const SRC_WE: u32 = 1 << 24;
const SRC_DIS: u32 = 1 << 22;
const SRC_DDAC1: u32 = 1 << 21;
const SRC_DDAC2: u32 = 1 << 20;
const SRC_DADC: u32 = 1 << 19;

const SRCREG_TRUNC_N: u32 = 0;
const SRCREG_INT_REGS: u32 = 1;
const SRCREG_VFREQ_FRAC: u32 = 3;

const CODEC_WIP: u32 = 1 << 30;

// 256 KiB max hardware buffer (65536 dwords).
const DMA_BYTES: usize = 256 * 1024;

struct Playback {
    base: u16,
    sample_rate: u32,
    dma_virt: *mut u8,
    dma_phys: u32,
    pending: Vec<u8>,
    playing: bool,
}

static PLAYBACK: SyncUnsafeCell<Option<Playback>> = SyncUnsafeCell::new(None);

fn find() -> Option<Device> {
    let mut found: Option<Device> = None;
    pci::scan(ScanKind::Pci, |d| {
        if d.vendor_id == VENDOR_ENSONIQ && d.device_id == DEVICE_ES1371 {
            found = Some(d);
        }
    });
    found
}

fn wait_src_ready(base: u16) -> u32 {
    unsafe {
        for _ in 0..0x1000 {
            let r = ind(base + REG_SRC_IF);
            if r & SRC_BUSY == 0 {
                return r;
            }
        }
        ind(base + REG_SRC_IF)
    }
}

fn src_read(base: u16, reg: u32) -> u16 {
    unsafe {
        let r = wait_src_ready(base) & (SRC_DIS | SRC_DDAC1 | SRC_DDAC2 | SRC_DADC);
        let cmd = r | (reg << 25);
        outd(base + REG_SRC_IF, cmd);
        ((wait_src_ready(base) >> 0) & 0xFFFF) as u16
    }
}

fn src_write(base: u16, addr: u32, data: u16) {
    unsafe {
        let r = wait_src_ready(base) & (SRC_DIS | SRC_DDAC1 | SRC_DDAC2 | SRC_DADC);
        let cmd = r | (addr << 25) | SRC_WE | (data as u32);
        outd(base + REG_SRC_IF, cmd);
    }
}

fn src_init(base: u16) {
    unsafe {
        outd(base + REG_SRC_IF, SRC_DIS);
        for i in 0..0x80 {
            src_write(base, i, 0);
        }
        // Default init values from Linux es1371 driver.
        src_write(base, 0x70 + SRCREG_TRUNC_N, 16 << 4);
        src_write(base, 0x70 + SRCREG_INT_REGS, 16 << 10);
        src_write(base, 0x74 + SRCREG_TRUNC_N, 16 << 4);
        src_write(base, 0x74 + SRCREG_INT_REGS, 16 << 10);
        src_write(base, 0x6C, 1 << 12);
        src_write(base, 0x6D, 1 << 12);
        src_write(base, 0x7C, 1 << 12);
        src_write(base, 0x7D, 1 << 12);
        src_write(base, 0x7E, 1 << 12);
        src_write(base, 0x7F, 1 << 12);
        set_dac2_rate(base, 22050);
        wait_src_ready(base);
        outd(base + REG_SRC_IF, 0);
    }
}

fn set_dac2_rate(base: u16, rate: u32) {
    let freq = (rate << 15) / 3000;
    let r = wait_src_ready(base) & (SRC_DIS | SRC_DDAC1 | SRC_DADC) | SRC_DDAC2;
    unsafe {
        outd(base + REG_SRC_IF, r);
    }
    // Preserve lower 8 bits of INT_REGS and update upper bits with (freq >> 5) & 0xfc00.
    let int_regs = src_read(base, 0x74 + SRCREG_INT_REGS);
    src_write(base, 0x74 + SRCREG_INT_REGS, (int_regs & 0x00ff) | (((freq >> 5) & 0xfc00) as u16));
    src_write(base, 0x74 + SRCREG_VFREQ_FRAC, (freq & 0x7fff) as u16);
    // Re-enable accumulator updates.
    let r2 = wait_src_ready(base) & (SRC_DIS | SRC_DDAC1 | SRC_DADC);
    unsafe {
        outd(base + REG_SRC_IF, r2);
    }
}

fn codec_write(base: u16, reg: u8, value: u16) {
    unsafe {
        for _ in 0..0x1000 {
            if ind(base + REG_CODEC) & CODEC_WIP == 0 {
                break;
            }
        }
        outd(base + REG_CODEC, ((reg as u32) << 16) | (value as u32));
        for _ in 0..0x1000 {
            if ind(base + REG_CODEC) & CODEC_WIP == 0 {
                break;
            }
        }
    }
}

fn init_hardware() -> Option<u16> {
    unsafe {
        let Some(device) = find() else {
            crate::serial_println!("ES1371 not found");
            return None;
        };

        let base = (pci::read_bar(device.bus, device.device, device.function, 0) & !1) as u16;
        if base == 0 {
            crate::serial_println!("ES1371 BAR0 unavailable");
            return None;
        }
        crate::serial_println!("ES1371 init: base=0x{:x}", base);

        let cmd = pci::read_command(device.bus, device.device, device.function);
        pci::write_command(device.bus, device.device, device.function, cmd | 0x0007);

        // Enable AC97 link and DMA bus requests (FreeBSD sequence):
        //   1. CDC_EN + BREQ to bring up AC97 link
        //   2. Pulse SYNC_RES for AC97 warm reset
        //   3. Back to CDC_EN + BREQ (clear SYNC_RES)
        outd(base + REG_CTRL, CTRL_CDC_EN | CTRL_BREQ);
        crate::drivers::pit::sleep_ms(20);
        outd(base + REG_CTRL, CTRL_CDC_EN | CTRL_BREQ | CTRL_SYNC_RES);
        crate::drivers::pit::sleep_ms(1);
        outd(base + REG_CTRL, CTRL_CDC_EN | CTRL_BREQ);
        crate::drivers::pit::sleep_ms(20);

        // Wait up to 200 ms for codec ready (CSTAT).  VMware may not implement
        // this bit; we log the result and continue regardless.
        let mut codec_ready = false;
        for _ in 0..200 {
            if ind(base + REG_STAT) & STAT_CSTAT != 0 {
                codec_ready = true;
                break;
            }
            crate::drivers::pit::sleep_ms(1);
        }
        crate::serial_println!("ES1371: codec_ready={} stat=0x{:x}", codec_ready, ind(base + REG_STAT));

        // AC97 soft reset via register 0x00.
        codec_write(base, 0x00, 0x0000);
        crate::drivers::pit::sleep_ms(10);
        crate::serial_println!("ES1371: codec reset done");

        // Init sample rate converter.
        src_init(base);
        crate::serial_println!("ES1371: SRC init done");

        // Unmute master volume (AC97 register 0x02) and headphone (0x04).
        codec_write(base, 0x02, 0x0000);
        codec_write(base, 0x04, 0x0000);
        // PCM out volume max, unmuted (AC97 register 0x18).
        codec_write(base, 0x18, 0x0000);
        crate::serial_println!("ES1371: volumes set");

        Some(base)
    }
}

fn alloc_dma() -> Option<(*mut u8, u32)> {
    unsafe {
        let layout = Layout::from_size_align(DMA_BYTES, 4096).ok()?;
        let ptr = alloc_zeroed(layout);
        if ptr.is_null() {
            return None;
        }
        Some((ptr, ptr as u32))
    }
}

fn free_dma(ptr: *mut u8) {
    unsafe {
        if ptr.is_null() {
            return;
        }
        let layout = Layout::from_size_align(DMA_BYTES, 4096).unwrap();
        dealloc(ptr, layout);
    }
}

unsafe fn copy_to_dma(dst: *mut u8, src: &[u8]) {
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst, src.len());
    }
}

fn play_chunk(base: u16, phys: u32, frames: usize) {
    unsafe {
        crate::serial_println!(
            "ES1371 play_chunk: phys=0x{:x} frames={} ctrl=0x{:x} ser=0x{:x}",
            phys,
            frames,
            ind(base + REG_CTRL),
            ind(base + REG_SER_IF)
        );
        // Select page 0x0C for DAC2 buffer registers.
        outd(base + REG_MEM_PAGE, 0x0C);
        outd(base + REG_P2_BUF_ADDR, phys);
        // Buffer size in dwords minus one.
        let dwords = (DMA_BYTES / 4) as u32;
        outd(base + REG_P2_BUF_DEF, dwords - 1);
        // Sample count (frames minus one).
        outd(base + REG_P2_SAMP_CT, (frames as u32) - 1);
        // 16-bit stereo, stop-mode single-shot (no interrupt: no handler registered).
        outd(base + REG_SER_IF, SER_P2_16BIT_STEREO | SER_P2_LOOP_SEL);
        // Enable DAC2 (preserve CDC_EN and BREQ).
        outd(base + REG_CTRL, ind(base + REG_CTRL) | CTRL_DAC2_EN | CTRL_BREQ);
        crate::serial_println!(
            "ES1371 play_chunk: after enable ctrl=0x{:x} sampct=0x{:x}",
            ind(base + REG_CTRL),
            ind(base + REG_P2_SAMP_CT)
        );
    }
}

fn stop(base: u16) {
    unsafe {
        outd(base + REG_CTRL, ind(base + REG_CTRL) & !CTRL_DAC2_EN);
    }
}

/// Play `data` (stereo 16-bit) via DMA and block until done.
unsafe fn play_and_wait(base: u16, sample_rate: u32, dma_virt: *mut u8, dma_phys: u32, data: &[u8]) {
    unsafe {
        const FRAME_SIZE: usize = 4; // stereo 16-bit
        let chunk_frames = data.len() / FRAME_SIZE;
        if chunk_frames == 0 {
            return;
        }
        copy_to_dma(dma_virt, data);
        play_chunk(base, dma_phys, chunk_frames);

        let timeout_ms = (chunk_frames as u64 * 1500 / sample_rate as u64) as usize + 500;
        let mut poll_count = 0usize;
        let mut seen_nonzero = false;
        loop {
            let samp_ct = ind(base + REG_P2_SAMP_CT);
            let curr = (samp_ct >> 16) as u16;
            let stat = ind(base + REG_STAT);

            if curr != 0 {
                seen_nonzero = true;
            }
            if stat & STAT_P2INT != 0 {
                crate::serial_println!("ES1371 play_and_wait: P2INT done");
                outd(base + REG_STAT, STAT_P2INT);
                break;
            }
            if curr == 0 && seen_nonzero {
                crate::serial_println!("ES1371 play_and_wait: curr=0 done");
                break;
            }
            crate::drivers::pit::sleep_ms(1);
            poll_count += 1;
            if poll_count % 1000 == 0 {
                crate::serial_println!(
                    "ES1371 play_and_wait: t={}ms curr={} seen_nz={} stat=0x{:x}",
                    poll_count, curr, seen_nonzero, stat
                );
            }
            if poll_count >= timeout_ms {
                crate::serial_println!(
                    "ES1371 play_and_wait: timeout {}ms curr={} seen_nz={}",
                    poll_count, curr, seen_nonzero
                );
                break;
            }
        }
        stop(base);
    }
}


pub fn audio_open() -> u64 {
    unsafe {
        let pb_opt = &mut *PLAYBACK.0.get();
        if pb_opt.is_some() {
            crate::serial_println!("ES1371 audio_open: EBUSY");
            return u64::MAX;
        }
        crate::serial_println!("ES1371 audio_open: initializing...");
        let Some(base) = init_hardware() else {
            crate::serial_println!("ES1371 audio_open: init_hardware failed");
            return u64::MAX;
        };
        let Some((dma_virt, dma_phys)) = alloc_dma() else {
            crate::serial_println!("ES1371 audio_open: alloc_dma failed");
            return u64::MAX;
        };
        crate::serial_println!("ES1371 audio_open: success base=0x{:x} dma=0x{:x}", base, dma_phys);
        // Align hardware rate with the default sample_rate stored below.
        set_dac2_rate(base, 48_000);
        *pb_opt = Some(Playback {
            base,
            sample_rate: 48_000,
            dma_virt,
            dma_phys,
            pending: Vec::new(),
            playing: false,
        });
        0
    }
}

pub fn audio_close(_handle: u64) {
    unsafe {
        let pb_opt = &mut *PLAYBACK.0.get();
        if let Some(pb) = pb_opt.take() {
            stop(pb.base);
            free_dma(pb.dma_virt);
        }
    }
}

pub fn audio_write(_handle: u64, buf: &[u8]) -> usize {
    unsafe {
        let pb_opt = &mut *PLAYBACK.0.get();
        let Some(pb) = pb_opt.as_mut() else {
            crate::serial_println!("ES1371 audio_write: no playback context");
            return 0;
        };
        if buf.is_empty() {
            return 0;
        }
        // If the pending buffer is full, play it now to make room.
        if pb.pending.len() >= DMA_BYTES {
            crate::serial_println!("ES1371 audio_write: pending full, auto-playing");
            let data = core::slice::from_raw_parts(pb.pending.as_ptr(), pb.pending.len());
            play_and_wait(pb.base, pb.sample_rate, pb.dma_virt, pb.dma_phys, data);
            pb.pending.clear();
            pb.playing = false;
        }
        let available = DMA_BYTES.saturating_sub(pb.pending.len());
        if available == 0 {
            return 0;
        }
        let to_queue = buf.len().min(available);
        pb.pending.extend_from_slice(&buf[..to_queue]);
        to_queue
    }
}

pub fn audio_ioctl(_handle: u64, cmd: u64, arg: u64) -> i64 {
    unsafe {
        let pb_opt = &mut *PLAYBACK.0.get();
        let Some(pb) = pb_opt.as_mut() else {
            return -1;
        };
        match cmd {
            0 => {
                // Set sample rate.
                let rate = arg as u32;
                if rate == 0 {
                    return -1;
                }
                pb.sample_rate = rate;
                crate::serial_println!("ES1371 ioctl: set sample rate {} Hz", rate);
                set_dac2_rate(pb.base, rate);
                0
            }
            1 => {
                // Drain: play all pending data and wait.
                crate::serial_println!("ES1371 ioctl: drain pending={}", pb.pending.len());
                if pb.pending.is_empty() {
                    return 0;
                }
                if pb.playing {
                    stop(pb.base);
                    pb.playing = false;
                }

                const FRAME_SIZE: usize = 4; // stereo 16-bit
                let mut offset = 0usize;
                while offset < pb.pending.len() {
                    let chunk = (pb.pending.len() - offset).min(DMA_BYTES);
                    if chunk < FRAME_SIZE {
                        break;
                    }
                    crate::serial_println!("ES1371 drain: chunk offset={} bytes={}", offset, chunk);
                    play_and_wait(
                        pb.base,
                        pb.sample_rate,
                        pb.dma_virt,
                        pb.dma_phys,
                        &pb.pending[offset..offset + chunk],
                    );
                    pb.playing = false;
                    offset += chunk;
                }
                pb.pending.clear();
                crate::serial_println!("ES1371 drain: done");
                0
            }
            _ => -1,
        }
    }
}

pub static AUDIO_OPS: crate::devfs::DeviceOps = crate::devfs::DeviceOps {
    open: audio_open,
    close: audio_close,
    read: |_handle, _buf| 0,
    write: audio_write,
    ioctl: audio_ioctl,
};
