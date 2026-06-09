use crate::drivers::pci::{self, Device, ScanKind};
use crate::util::{SyncUnsafeCell, inb, inw, outb, outd, outw};

#[repr(C, align(16))]
struct BufferDescriptorList([BufferDescriptor; BDL_COUNT]);

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct BufferDescriptor {
    addr: u32,
    samples: u16,
    flags: u16,
}

const TEST_FRAMES: usize = 48_000;
const PCM_WORDS: usize = TEST_FRAMES * 2;
const BDL_COUNT: usize = 32;
const STREAM_CHUNK_FRAMES: usize = 8192;
const STREAM_FRAMES: usize = BDL_COUNT * STREAM_CHUNK_FRAMES;

static mut BDL: BufferDescriptorList = BufferDescriptorList(
    [BufferDescriptor {
        addr: 0,
        samples: 0,
        flags: 0,
    }; BDL_COUNT],
);
static mut PCM: [i16; PCM_WORDS] = [0; PCM_WORDS];
static mut STREAM_PCM: [i16; STREAM_FRAMES * 2] = [0; STREAM_FRAMES * 2];
static BACKGROUND: SyncUnsafeCell<Option<BackgroundPlayback>> = SyncUnsafeCell::new(None);

struct BackgroundPlayback {
    po: u16,
    wav: crate::audio::WavInfo<'static>,
    total_frames: usize,
    played_frames: usize,
    next_frame: usize,
    next_desc: usize,
    last_civ: usize,
    done: bool,
}

pub fn test() {
    unsafe {
        fill_pcm();
        play_pcm(core::ptr::addr_of!(PCM) as *const i16, TEST_FRAMES, 48_000);
    }
}

pub fn play_wav(data: &[u8]) {
    crate::audio::play_wav(data);
}

pub fn play_wav_background(data: &'static [u8]) {
    let Some(wav) = crate::audio::parse_wav_public(data) else {
        crate::println!("unsupported WAV");
        return;
    };
    unsafe {
        start_background(wav);
    }
}

pub(crate) fn play_wav_parsed(wav: &crate::audio::WavInfo<'_>) {
    unsafe {
        play_wav_stream(wav);
    }
}

pub fn poll_background() {
    unsafe {
        let bg_opt = &mut *BACKGROUND.0.get();
        let Some(bg) = bg_opt.as_mut() else {
            return;
        };
        if bg.done {
            return;
        }
        let civ = (inb(bg.po + 0x04) as usize) & (BDL_COUNT - 1);
        if civ != bg.last_civ {
            let mut steps = if civ > bg.last_civ {
                civ - bg.last_civ
            } else {
                BDL_COUNT - bg.last_civ + civ
            };
            while steps > 0 {
                let desc = bg.last_civ;
                bg.played_frames =
                    (bg.played_frames + descriptor_frames(desc)).min(bg.total_frames);
                if bg.next_frame < bg.total_frames {
                    let refill_desc = bg.next_desc % BDL_COUNT;
                    let frames = (bg.total_frames - bg.next_frame).min(STREAM_CHUNK_FRAMES);
                    let last = bg.next_frame + frames >= bg.total_frames;
                    fill_stream_chunk_public(
                        &bg.wav,
                        bg.next_frame,
                        frames,
                        refill_desc * STREAM_CHUNK_FRAMES * 2,
                    );
                    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                    set_stream_bdl(refill_desc, frames, last);
                    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                    outb(bg.po + 0x05, refill_desc as u8);
                    bg.next_frame += frames;
                    bg.next_desc += 1;
                }
                bg.last_civ = (bg.last_civ + 1) % BDL_COUNT;
                steps -= 1;
            }
        }
        let picb = inw(bg.po + 0x08);
        if picb == 0 && bg.played_frames >= bg.total_frames {
            outb(bg.po + 0x0b, 0x00);
            outb(bg.po + 0x0b, 0x02);
            bg.done = true;
            crate::println!("background WAV done");
        }
    }
}

unsafe fn start_background(wav: crate::audio::WavInfo<'static>) {
    unsafe {
        let Some((nam, po)) = init_device(wav.sample_rate) else {
            return;
        };
        let total_frames = wav.frames();
        let mut next_frame = 0usize;
        let mut next_desc = 0usize;
        while next_desc < BDL_COUNT && next_frame < total_frames {
            let frames = (total_frames - next_frame).min(STREAM_CHUNK_FRAMES);
            let last = next_frame + frames >= total_frames;
            fill_stream_chunk_public(
                &wav,
                next_frame,
                frames,
                next_desc * STREAM_CHUNK_FRAMES * 2,
            );
            set_stream_bdl(next_desc, frames, last);
            next_frame += frames;
            next_desc += 1;
        }
        let lvi = if next_desc == 0 {
            BDL_COUNT - 1
        } else {
            next_desc - 1
        } as u8;
        outb(po + 0x05, lvi);
        outb(po + 0x0b, 0x01);
        *BACKGROUND.0.get() = Some(BackgroundPlayback {
            po,
            wav,
            total_frames,
            played_frames: 0,
            next_frame,
            next_desc,
            last_civ: (inb(po + 0x04) as usize) & (BDL_COUNT - 1),
            done: false,
        });
        let _ = nam;
        crate::println!("background WAV started");
    }
}

unsafe fn play_wav_stream(wav: &crate::audio::WavInfo<'_>) {
    unsafe {
        let Some((_nam, po)) = init_device(wav.sample_rate) else {
            return;
        };
        let total_frames = wav.frames();
        let mut next_frame = 0usize;
        let mut next_desc = 0usize;
        while next_desc < BDL_COUNT && next_frame < total_frames {
            let frames = (total_frames - next_frame).min(STREAM_CHUNK_FRAMES);
            let last = next_frame + frames >= total_frames;
            fill_stream_chunk_public(wav, next_frame, frames, next_desc * STREAM_CHUNK_FRAMES * 2);
            set_stream_bdl(next_desc, frames, last);
            next_frame += frames;
            next_desc += 1;
        }
        let lvi = if next_desc == 0 {
            BDL_COUNT - 1
        } else {
            next_desc - 1
        } as u8;
        outb(po + 0x05, lvi);
        outb(po + 0x0b, 0x01);
        let mut last_civ = (inb(po + 0x04) as usize) & (BDL_COUNT - 1);
        let mut played_frames = 0usize;
        let mut idle_ms = 0usize;
        let max_idle_ms =
            ((total_frames as u64 * 1000) / wav.sample_rate.max(1) as u64) as usize + 1000;
        while played_frames < total_frames && idle_ms < max_idle_ms {
            let civ = (inb(po + 0x04) as usize) & (BDL_COUNT - 1);
            if civ != last_civ {
                let mut steps = if civ > last_civ {
                    civ - last_civ
                } else {
                    BDL_COUNT - last_civ + civ
                };
                while steps > 0 {
                    let desc = last_civ;
                    played_frames = (played_frames + descriptor_frames(desc)).min(total_frames);
                    if next_frame < total_frames {
                        let refill_desc = next_desc % BDL_COUNT;
                        let frames = (total_frames - next_frame).min(STREAM_CHUNK_FRAMES);
                        let last = next_frame + frames >= total_frames;
                        fill_stream_chunk_public(
                            wav,
                            next_frame,
                            frames,
                            refill_desc * STREAM_CHUNK_FRAMES * 2,
                        );
                        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                        set_stream_bdl(refill_desc, frames, last);
                        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                        outb(po + 0x05, refill_desc as u8);
                        next_frame += frames;
                        next_desc += 1;
                    }
                    last_civ = (last_civ + 1) % BDL_COUNT;
                    steps -= 1;
                }
                idle_ms = 0;
            } else {
                let sr = inw(po + 0x06);
                if sr & 0x1C != 0 {
                    outw(po + 0x06, sr & 0x1C);
                }
                crate::drivers::pit::sleep_ms(1);
                idle_ms += 1;
            }
        }
        outb(po + 0x0b, 0x00);
        outb(po + 0x0b, 0x02);
        crate::println!("AC97 playback done");
    }
}

unsafe fn init_device(sample_rate: u32) -> Option<(u16, u16)> {
    unsafe {
        let Some(device) = find() else {
            crate::println!("AC97 not found");
            return None;
        };
        let nam = (pci::read_bar(device.bus, device.device, device.function, 0) & !1) as u16;
        let nabm = (pci::read_bar(device.bus, device.device, device.function, 1) & !1) as u16;
        if nam == 0 || nabm == 0 {
            crate::println!("AC97 BAR unavailable");
            return None;
        }
        let command = pci::read_command(device.bus, device.device, device.function);
        pci::write_command(device.bus, device.device, device.function, command | 0x0007);
        let po = nabm + 0x10;
        outw(nam + 0x02, 0x0000);
        outw(nam + 0x18, 0x0808);
        outw(nam + 0x1a, 0x0808);
        outw(nam + 0x2a, inw(nam + 0x2a) | 0x0001);
        outw(nam + 0x2c, sample_rate as u16);
        outb(po + 0x0b, 0x00);
        for _ in 0..100 {
            if inb(po + 0x0b) & 0x01 == 0 {
                break;
            }
            crate::drivers::pit::sleep_ms(1);
        }
        outw(po + 0x06, inw(po + 0x06) & 0x001C);
        outd(po, core::ptr::addr_of!(BDL) as u32);
        outb(po + 0x05, 0);
        Some((nam, po))
    }
}

fn descriptor_frames(index: usize) -> usize {
    unsafe { (BDL.0[index].samples as usize) / 2 }
}

unsafe fn play_pcm(samples_ptr: *const i16, frames: usize, sample_rate: u32) {
    unsafe {
        let Some((_nam, po)) = init_device(sample_rate) else {
            return;
        };
        setup_single_bdl(samples_ptr, frames * 2);
        outb(po + 0x05, 0);
        outb(po + 0x0b, 0x01);
        let ms = ((frames as u64 * 1000) / sample_rate.max(1) as u64) as u32 + 100;
        crate::drivers::pit::sleep_ms(ms);
        outb(po + 0x0b, 0x00);
        outb(po + 0x0b, 0x02);
    }
    crate::println!("AC97 playback done");
}

unsafe fn setup_single_bdl(samples_ptr: *const i16, words: usize) {
    unsafe {
        BDL.0[0] = BufferDescriptor {
            addr: samples_ptr as u32,
            samples: words as u16,
            flags: 0x8000,
        };
    }
}

unsafe fn set_stream_bdl(index: usize, frames: usize, last: bool) {
    unsafe {
        BDL.0[index] = BufferDescriptor {
            addr: (core::ptr::addr_of!(STREAM_PCM) as *const i16)
                .add(index * STREAM_CHUNK_FRAMES * 2) as u32,
            samples: (frames * 2) as u16,
            flags: if last { 0x8000 } else { 0 },
        };
    }
}

fn fill_stream_chunk_public(
    wav: &crate::audio::WavInfo<'_>,
    start_frame: usize,
    chunk_frames: usize,
    out_offset: usize,
) {
    unsafe {
        let ptr = (core::ptr::addr_of_mut!(STREAM_PCM) as *mut i16).add(out_offset);
        let out = core::slice::from_raw_parts_mut(ptr, chunk_frames * 2);
        wav.fill_stereo_i16(start_frame, chunk_frames, out);
    }
}

unsafe fn fill_pcm() {
    unsafe {
        let mut i = 0usize;
        while i < TEST_FRAMES {
            let phase = (i / 54) & 1;
            let sample = if phase == 0 { 8000 } else { -8000 };
            PCM[i * 2] = sample;
            PCM[i * 2 + 1] = sample;
            i += 1;
        }
    }
}

fn find() -> Option<Device> {
    let mut found = None;
    pci::scan(ScanKind::Pci, |dev| {
        if found.is_none() && dev.class_code == 0x04 && dev.subclass == 0x01 {
            found = Some(dev);
        }
    });
    found
}

// ========================================================================
// /dev/audio streaming interface
// ========================================================================

struct AudioStream {
    po: u16,
    nam: u16,
    sample_rate: u32,
    // Next descriptor to fill.
    next_desc: usize,
    // Last observed CIV (descriptor currently being played).
    last_civ: usize,
    running: bool,
}

static STREAM: SyncUnsafeCell<Option<AudioStream>> = SyncUnsafeCell::new(None);

pub fn audio_open() -> u64 {
    unsafe {
        let stream_opt = &mut *STREAM.0.get();
        if stream_opt.is_some() {
            return u64::MAX; // EBUSY
        }
        let Some((nam, po)) = init_device(48_000) else {
            return u64::MAX;
        };
        outd(po, core::ptr::addr_of!(BDL) as u32);
        *stream_opt = Some(AudioStream {
            po,
            nam,
            sample_rate: 48_000,
            next_desc: 0,
            last_civ: 0,
            running: false,
        });
        0
    }
}

pub fn audio_close(_handle: u64) {
    unsafe {
        let stream_opt = &mut *STREAM.0.get();
        if let Some(stream) = stream_opt {
            outb(stream.po + 0x0b, 0x00);
            outb(stream.po + 0x0b, 0x02);
        }
        *stream_opt = None;
    }
}

pub fn audio_write(_handle: u64, buf: &[u8]) -> usize {
    unsafe {
        let stream_opt = &mut *STREAM.0.get();
        let Some(stream) = stream_opt else {
            return 0;
        };
        if buf.is_empty() {
            return 0;
        }

        let chunk_bytes = STREAM_CHUNK_FRAMES * 4;
        let mut written = 0usize;
        let mut src_offset = 0usize;

        while written < buf.len() {
            if stream.running {
                let civ = (inb(stream.po + 0x04) as usize) & (BDL_COUNT - 1);
                if civ != stream.last_civ {
                    let steps = if civ > stream.last_civ {
                        civ - stream.last_civ
                    } else {
                        BDL_COUNT - stream.last_civ + civ
                    };
                    stream.last_civ = (stream.last_civ + steps) % BDL_COUNT;
                }
                // Buffer full when next_desc == last_civ (don't overwrite
                // the descriptor currently being played).
                if stream.next_desc == stream.last_civ {
                    break;
                }
            }

            let desc = stream.next_desc;
            let dst = (core::ptr::addr_of_mut!(STREAM_PCM) as *mut u8)
                .add(desc * chunk_bytes);
            let space = chunk_bytes.min(buf.len() - written);
            core::ptr::copy_nonoverlapping(
                buf.as_ptr().add(src_offset),
                dst,
                space,
            );
            written += space;
            src_offset += space;

            let frames = space / 4;
            set_stream_bdl(desc, frames, false);

            stream.next_desc = (desc + 1) % BDL_COUNT;
        }

        if written > 0 {
            let lvi = if stream.next_desc == 0 {
                BDL_COUNT - 1
            } else {
                stream.next_desc - 1
            } as u8;
            outb(stream.po + 0x05, lvi);

            if !stream.running {
                outb(stream.po + 0x0b, 0x01);
                stream.running = true;
            } else {
                // Only restart DMA if it has halted.
                // Writing CR_RUN on QEMU resets CIV to PIV, aborting
                // the descriptor currently being played.
                let sr = inw(stream.po + 0x06);
                if sr & 0x01 != 0 {
                    outb(stream.po + 0x0b, 0x01);
                }
            }
        }

        written
    }
}

pub fn audio_ioctl(_handle: u64, cmd: u64, arg: u64) -> i64 {
    unsafe {
        let stream_opt = &mut *STREAM.0.get();
        let Some(stream) = stream_opt else {
            return -1;
        };
        match cmd {
            0 => {
                // Set sample rate.
                stream.sample_rate = arg as u32;
                outw(stream.nam + 0x2c, stream.sample_rate as u16);
                0
            }
            1 => {
                // Drain: wait until all queued audio has been played.
                if !stream.running {
                    return 0;
                }
                let mut idle_ms = 0usize;
                while idle_ms < 60_000 {
                    let sr = inw(stream.po + 0x06);
                    // SR_DCH (bit 0) means DMA halted.
                    if sr & 0x01 != 0 {
                        let picb = inw(stream.po + 0x08);
                        if picb == 0 {
                            break;
                        }
                    }
                    if sr & 0x1C != 0 {
                        outw(stream.po + 0x06, sr & 0x1C);
                    }
                    crate::drivers::pit::sleep_ms(1);
                    idle_ms += 1;
                }
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
