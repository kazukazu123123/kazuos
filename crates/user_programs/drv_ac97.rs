#![no_std]
#![no_main]

include!("../../crates/kernel/src/syscall_numbers.rs");

// PCI config ports
const PCI_ADDR: u16 = 0xCF8;
const PCI_DATA: u16 = 0xCFC;

const BDL_COUNT:          usize = 32;
const STREAM_CHUNK_FRAMES: usize = 8192;
const STREAM_FRAMES:      usize = BDL_COUNT * STREAM_CHUNK_FRAMES;
const MAX_WAV_SIZE:       usize = 32 * 1024 * 1024; // 32 MiB
const IPC_BUF:            usize = 256;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Bde {
    addr:    u32,
    samples: u16,
    flags:   u16,
}

struct Ac97 {
    nam:     u16,
    po:      u16,
    bdl_va:  u64,
    bdl_pa:  u32,
    pcm_va:  u64,
    pcm_pa:  u32,
    wav_va:  u64, // large read buffer (not DMA, just mapped memory)
}

#[no_mangle]
pub extern "C" fn _start(_argc: u64, _argv: u64) -> ! {
    // Allow PCI config ports.
    let r0 = syscall(SYS_IOPORT_REQUEST, PCI_ADDR as u64, 4, 0);
    let r1 = syscall(SYS_IOPORT_REQUEST, PCI_DATA as u64, 4, 0);
    if r0 == u64::MAX || r1 == u64::MAX {
        loop { syscall(SYS_SLEEP, 10000, SLEEP_UNIT_MS, 0); }
    }

    // Find AC97 (class=0x04 subclass=0x01).
    let loc = match pci_find(0x04, 0x01) {
        Some(l) => l,
        None => {
            loop { syscall(SYS_SLEEP, 10000, SLEEP_UNIT_MS, 0); }
        }
    };

    let (bus, dev, func) = (loc >> 16, (loc >> 8) & 0xFF, loc & 0xFF);
    let nam  = (pci_read_bar(bus, dev, func, 0) & !1) as u16;
    let nabm = (pci_read_bar(bus, dev, func, 1) & !1) as u16;

    if nam == 0 || nabm == 0 {
        loop { syscall(SYS_SLEEP, 10000, SLEEP_UNIT_MS, 0); }
    }

    // Allow AC97 I/O ports (NAM and NABM, each up to 256 ports wide).
    syscall(SYS_IOPORT_REQUEST, nam as u64,  256, 0);
    syscall(SYS_IOPORT_REQUEST, nabm as u64, 256, 0);

    // Enable bus mastering + I/O space.
    let cmd = pci_read_u16(bus, dev, func, 0x04);
    pci_write_u16(bus, dev, func, 0x04, cmd | 0x0005);

    let po = nabm + 0x10;

    // Allocate DMA buffers.
    let bdl_size = (BDL_COUNT * core::mem::size_of::<Bde>()) as u64;
    let pcm_size = (STREAM_FRAMES * 2 * 2) as u64; // stereo i16

    let mut bdl_pa: u64 = 0;
    let bdl_va = syscall(SYS_DMA_ALLOC, bdl_size, &mut bdl_pa as *mut u64 as u64, 0);
    let mut pcm_pa: u64 = 0;
    let pcm_va = syscall(SYS_DMA_ALLOC, pcm_size, &mut pcm_pa as *mut u64 as u64, 0);
    // Read buffer for WAV files (not DMA — no phys addr needed, pass 0 for phys_out).
    let wav_va = syscall(SYS_DMA_ALLOC, MAX_WAV_SIZE as u64, 0, 0);

    if bdl_va == u64::MAX || pcm_va == u64::MAX || wav_va == u64::MAX {
        loop { syscall(SYS_SLEEP, 10000, SLEEP_UNIT_MS, 0); }
    }
    let ac97 = Ac97 {
        nam,
        po,
        bdl_va,
        bdl_pa: bdl_pa as u32,
        pcm_va,
        pcm_pa: pcm_pa as u32,
        wav_va,
    };

    // Open IPC channel.
    let ch_name = b"ac97";
    let ch = syscall(SYS_IPC_OPEN, ch_name.as_ptr() as u64, ch_name.len() as u64, 0);
    if ch == u64::MAX {
        loop { syscall(SYS_SLEEP, 10000, SLEEP_UNIT_MS, 0); }
    }

    let mut msg = [0u8; IPC_BUF];
    loop {
        let len = syscall(SYS_IPC_RECV, ch, msg.as_mut_ptr() as u64, IPC_BUF as u64);
        if len == 0 || len == u64::MAX {
            continue;
        }
        // Message is a filename (not NUL-terminated, length = len).
        let path_len = len as usize;
        let fd = syscall(SYS_OPEN, msg.as_ptr() as u64, path_len as u64, 0);
        if fd == u64::MAX {
            continue;
        }
        let mut wav_len = 0usize;
        const CHUNK: usize = 4096;
        while wav_len < MAX_WAV_SIZE {
            let want = CHUNK.min(MAX_WAV_SIZE - wav_len);
            let n = syscall(SYS_READ, fd, ac97.wav_va + wav_len as u64, want as u64);
            if n == 0 || n == u64::MAX {
                break;
            }
            wav_len += n as usize;
        }
        syscall(SYS_CLOSE, fd, 0, 0);
        if wav_len == 0 {
            continue;
        }
        let wav_slice = unsafe {
            core::slice::from_raw_parts(ac97.wav_va as *const u8, wav_len)
        };
        if let Some(wav) = parse_wav(wav_slice) {
            play(&ac97, &wav);
        }
    }

    #[allow(unreachable_code)]
    syscall(SYS_DMA_FREE, ac97.bdl_va, 0, 0);
    syscall(SYS_DMA_FREE, ac97.pcm_va, 0, 0);
    syscall(SYS_DMA_FREE, ac97.wav_va, 0, 0);
    syscall(SYS_EXIT, 0, 0, 0);
    loop {}
}

// ---------- playback ----------

struct WavInfo<'a> {
    data:        &'a [u8],
    channels:    u16,
    sample_rate: u32,
    bits:        u16,
}

impl WavInfo<'_> {
    fn frames(&self) -> usize {
        self.data.len() / (self.channels as usize * (self.bits as usize / 8))
    }

    fn fill_stereo_i16(&self, start: usize, count: usize, out: &mut [i16]) {
        let frame_size = self.channels as usize * (self.bits as usize / 8);
        let total = self.frames();
        let mut i = 0usize;
        while i < count {
            let frame = start + i;
            if frame >= total { out[i*2] = 0; out[i*2+1] = 0; i += 1; continue; }
            let base = frame * frame_size;
            let (l, r) = if self.bits == 16 {
                if self.channels == 1 {
                    let s = i16::from_le_bytes([self.data[base], self.data[base+1]]);
                    (s, s)
                } else {
                    (i16::from_le_bytes([self.data[base],   self.data[base+1]]),
                     i16::from_le_bytes([self.data[base+2], self.data[base+3]]))
                }
            } else {
                if self.channels == 1 {
                    let s = ((self.data[base] as i16) - 128) << 8;
                    (s, s)
                } else {
                    (((self.data[base]   as i16) - 128) << 8,
                     ((self.data[base+1] as i16) - 128) << 8)
                }
            };
            out[i*2] = l; out[i*2+1] = r;
            i += 1;
        }
    }
}

fn play(ac: &Ac97, wav: &WavInfo<'_>) {
    // Initialise AC97 mixer and sample rate.
    outw(ac.nam + 0x02, 0x0000);
    outw(ac.nam + 0x18, 0x0808);
    outw(ac.nam + 0x1a, 0x0808);
    outw(ac.nam + 0x2a, inw(ac.nam + 0x2a) | 0x0001);
    outw(ac.nam + 0x2c, wav.sample_rate as u16);

    // Stop and reset PCM-out.
    outb(ac.po + 0x0b, 0x00);
    let mut i = 0u32;
    while i < 100 {
        if inb(ac.po + 0x0b) & 0x01 == 0 { break; }
        syscall(SYS_SLEEP, 1, SLEEP_UNIT_MS, 0);
        i += 1;
    }
    outw(ac.po + 0x06, inw(ac.po + 0x06) & 0x001C);

    // BDL base address.
    outd(ac.po, ac.bdl_pa);

    let total = wav.frames();
    let mut next_frame = 0usize;
    let mut next_desc  = 0usize;

    // Fill initial BDL entries.
    while next_desc < BDL_COUNT && next_frame < total {
        let frames = (total - next_frame).min(STREAM_CHUNK_FRAMES);
        let last   = next_frame + frames >= total;
        fill_chunk(ac, wav, next_frame, frames, next_desc);
        set_bde(ac, next_desc, frames, last);
        next_frame += frames;
        next_desc  += 1;
    }

    let lvi = (if next_desc == 0 { BDL_COUNT - 1 } else { next_desc - 1 }) as u8;
    outb(ac.po + 0x05, lvi);
    outb(ac.po + 0x0b, 0x01); // run

    let mut last_civ     = inb(ac.po + 0x04) as usize & (BDL_COUNT - 1);
    let mut played       = 0usize;
    let max_idle_ms      = ((total as u64 * 1000) / wav.sample_rate.max(1) as u64) as usize + 2000;
    let mut idle_ms      = 0usize;

    while played < total && idle_ms < max_idle_ms {
        let civ = inb(ac.po + 0x04) as usize & (BDL_COUNT - 1);
        if civ != last_civ {
            let steps = if civ > last_civ { civ - last_civ } else { BDL_COUNT - last_civ + civ };
            let mut s = 0usize;
            while s < steps {
                let desc_frames = bde_frames(ac, last_civ);
                played = (played + desc_frames).min(total);
                if next_frame < total {
                    let rd = next_desc % BDL_COUNT;
                    let frames = (total - next_frame).min(STREAM_CHUNK_FRAMES);
                    let last   = next_frame + frames >= total;
                    fill_chunk(ac, wav, next_frame, frames, rd);
                    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                    set_bde(ac, rd, frames, last);
                    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                    outb(ac.po + 0x05, rd as u8);
                    next_frame += frames;
                    next_desc  += 1;
                }
                last_civ = (last_civ + 1) % BDL_COUNT;
                s += 1;
            }
            idle_ms = 0;
        } else {
            let sr = inw(ac.po + 0x06);
            if sr & 0x1C != 0 { outw(ac.po + 0x06, sr & 0x1C); }
            syscall(SYS_SLEEP, 1, SLEEP_UNIT_MS, 0);
            idle_ms += 1;
        }
    }

    outb(ac.po + 0x0b, 0x00);
    outb(ac.po + 0x0b, 0x02); // reset
}

fn fill_chunk(ac: &Ac97, wav: &WavInfo<'_>, start: usize, frames: usize, desc: usize) {
    let out_offset = desc * STREAM_CHUNK_FRAMES * 2;
    let ptr = (ac.pcm_va + (out_offset * 2) as u64) as *mut i16;
    let out = unsafe { core::slice::from_raw_parts_mut(ptr, frames * 2) };
    wav.fill_stereo_i16(start, frames, out);
}

fn set_bde(ac: &Ac97, index: usize, frames: usize, last: bool) {
    let bde = Bde {
        addr:    ac.pcm_pa + (index * STREAM_CHUNK_FRAMES * 4) as u32,
        samples: (frames * 2) as u16,
        flags:   if last { 0x8000 } else { 0 },
    };
    let ptr = (ac.bdl_va + (index * core::mem::size_of::<Bde>()) as u64) as *mut Bde;
    unsafe { core::ptr::write_unaligned(ptr, bde); }
}

fn bde_frames(ac: &Ac97, index: usize) -> usize {
    let ptr = (ac.bdl_va + (index * core::mem::size_of::<Bde>()) as u64) as *const Bde;
    let bde = unsafe { core::ptr::read_unaligned(ptr) };
    bde.samples as usize / 2
}

// ---------- WAV parser ----------

fn parse_wav(data: &[u8]) -> Option<WavInfo<'_>> {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let mut off = 12usize;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits = 0u16;
    let mut data_start = 0usize;
    let mut data_len = 0usize;
    while off + 8 <= data.len() {
        let id  = &data[off..off+4];
        let len = le_u32(data, off+4) as usize;
        off += 8;
        if off + len > data.len() { return None; }
        if id == b"fmt " {
            if len < 16 || le_u16(data, off) != 1 { return None; }
            channels    = le_u16(data, off+2);
            sample_rate = le_u32(data, off+4);
            bits        = le_u16(data, off+14);
        } else if id == b"data" {
            data_start = off;
            data_len   = len;
        }
        off += (len + 1) & !1;
    }
    if !(channels == 1 || channels == 2) || !(bits == 8 || bits == 16)
        || data_start == 0 || data_len == 0 { return None; }
    Some(WavInfo { data: &data[data_start..data_start+data_len], channels, sample_rate, bits })
}

// ---------- PCI ----------

fn pci_addr(bus: u64, dev: u64, func: u64, off: u8) -> u32 {
    0x8000_0000
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) <<  8)
        | (off as u32 & 0xFC)
}

fn pci_read_u32(bus: u64, dev: u64, func: u64, off: u8) -> u32 {
    outd(PCI_ADDR as u16, pci_addr(bus, dev, func, off));
    ind(PCI_DATA)
}

fn pci_read_u16(bus: u64, dev: u64, func: u64, off: u8) -> u16 {
    let d = pci_read_u32(bus, dev, func, off & !2);
    if off & 2 != 0 { (d >> 16) as u16 } else { d as u16 }
}

fn pci_write_u16(bus: u64, dev: u64, func: u64, off: u8, val: u16) {
    let d = pci_read_u32(bus, dev, func, off & !2);
    let new = if off & 2 != 0 {
        (d & 0x0000_FFFF) | ((val as u32) << 16)
    } else {
        (d & 0xFFFF_0000) | val as u32
    };
    outd(PCI_ADDR as u16, pci_addr(bus, dev, func, off & !2));
    outd(PCI_DATA, new);
}

fn pci_read_bar(bus: u64, dev: u64, func: u64, bar: u8) -> u32 {
    pci_read_u32(bus, dev, func, 0x10 + bar * 4)
}

fn pci_find(class: u8, subclass: u8) -> Option<u64> {
    let mut bus = 0u64;
    while bus < 256 {
        let mut dev = 0u64;
        while dev < 32 {
            let id = pci_read_u32(bus, dev, 0, 0);
            if id == 0xFFFF_FFFF { dev += 1; continue; }
            let cc = pci_read_u32(bus, dev, 0, 0x08);
            let c  = (cc >> 24) as u8;
            let sc = (cc >> 16) as u8;
            if c == class && sc == subclass {
                return Some((bus << 16) | (dev << 8));
            }
            dev += 1;
        }
        bus += 1;
    }
    None
}

// ---------- I/O helpers ----------

fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem,nostack)) }
    v
}
fn inw(port: u16) -> u16 {
    let v: u16;
    unsafe { core::arch::asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem,nostack)) }
    v
}
fn ind(port: u16) -> u32 {
    let v: u32;
    unsafe { core::arch::asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem,nostack)) }
    v
}
fn outb(port: u16, v: u8)  {
    unsafe { core::arch::asm!("out dx, al",  in("dx") port, in("al")  v, options(nomem,nostack)) }
}
fn outw(port: u16, v: u16) {
    unsafe { core::arch::asm!("out dx, ax",  in("dx") port, in("ax")  v, options(nomem,nostack)) }
}
fn outd(port: u16, v: u32) {
    unsafe { core::arch::asm!("out dx, eax", in("dx") port, in("eax") v, options(nomem,nostack)) }
}

fn le_u16(d: &[u8], o: usize) -> u16 { u16::from_le_bytes([d[o], d[o+1]]) }
fn le_u32(d: &[u8], o: usize) -> u32 { u32::from_le_bytes([d[o], d[o+1], d[o+2], d[o+3]]) }

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
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
