#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

// IPC-based audio server with additive mixing.
//
// Owns the "audio" channel and is the sole writer of /dev/audio. Any number of
// clients (up to MAX_STREAMS) send PCM concurrently; audiod sums their samples
// (with clipping) into one output chunk per round and writes the mix to the HDA
// driver. Each contributing client is ack'd on its reply channel once its chunk
// has been mixed, which paces every producer to real time and keeps concurrent
// streams roughly aligned — so they play together instead of taking turns.
//
// Message layout (client -> "audio"):
//   [0]     op: 1=TONE, 2=STOP, 3=PCM, 4=SETCH
//   [1]     reserved
//   [2..4]  reply channel id (u16 LE) — also the stream id; 0 = no ack / no mixing
//   [4..8]  arg (u32 LE): TONE = frequency Hz, SETCH = channel count
//   [8..]   PCM payload (op=PCM), s16le, 48kHz, stereo, up to one chunk
//
// A PCM message with reply != 0 joins the mix; reply == 0 is written straight
// through (single-stream, un-acked). TONE is a kernel square wave and bypasses
// the mix. Ack (audiod -> reply channel): single byte [1].

const OP_TONE: u8 = 1;
const OP_STOP: u8 = 2;
const OP_PCM: u8 = 3;
const OP_SETCH: u8 = 4;

const HEADER: usize = 8;
const MSG_MAX: usize = 8192;
const CHUNK_BYTES: usize = 4800; // 1200 stereo frames @ s16le
const SAMPLES: usize = CHUNK_BYTES / 2; // 2400 i16 samples
const MAX_STREAMS: usize = 8;
const MAX_MISS: u32 = 4; // rounds a silent stream lingers before its slot is freed
const ALIGN_TRIES: u32 = 4; // short waits to let concurrent streams catch up

struct Stream {
    reply: u64, // stream id / ack channel; 0 = free slot
    has: bool,  // a chunk is buffered for this round
    miss: u32,  // consecutive rounds with no chunk
    buf: alloc::vec::Vec<u8>,
}

fn write_all(fd: u64, buf: &[u8]) -> bool {
    let mut written = 0usize;
    while written < buf.len() {
        let n = sys_write_fd(fd, &buf[written..]);
        if n == u64::MAX {
            return false;
        }
        if n == 0 {
            // Ring buffer full: block until the DMA engine frees a chunk.
            sys_ioctl(fd, 3, 0);
            continue;
        }
        written += n as usize;
    }
    true
}

/// Slot index for `reply`: its existing slot, or a newly claimed free one, or None if full.
fn slot_for(streams: &mut [Stream], reply: u64) -> Option<usize> {
    if let Some(i) = streams.iter().position(|s| s.reply == reply) {
        return Some(i);
    }
    let i = streams.iter().position(|s| s.reply == 0)?;
    streams[i].reply = reply;
    streams[i].miss = 0;
    Some(i)
}

/// Handle one request message. PCM with a reply id is buffered for mixing; everything
/// else acts immediately.
fn ingest(streams: &mut [Stream], fd: u64, msg: &[u8], n: usize) {
    if n < HEADER {
        return;
    }
    let op = msg[0];
    let reply = u16::from_le_bytes([msg[2], msg[3]]) as u64;
    let arg = u32::from_le_bytes([msg[4], msg[5], msg[6], msg[7]]);
    let payload = &msg[HEADER..n];

    match op {
        OP_PCM => {
            if reply == 0 {
                // Legacy single-stream path: write straight through, no ack.
                write_all(fd, payload);
                return;
            }
            if let Some(i) = slot_for(streams, reply) {
                let s = &mut streams[i];
                let len = payload.len().min(CHUNK_BYTES);
                for b in s.buf.iter_mut() {
                    *b = 0;
                }
                s.buf[..len].copy_from_slice(&payload[..len]);
                s.has = true;
                s.miss = 0;
            }
            // No free slot: drop this chunk (server is at capacity).
        }
        OP_TONE => {
            sys_ioctl(fd, 1, arg as u64);
            if reply != 0 {
                sys_ipc_send(reply, &[1u8]);
            }
        }
        OP_STOP => {
            sys_ioctl(fd, 0, 0);
            if reply != 0 {
                sys_ipc_send(reply, &[1u8]);
            }
        }
        OP_SETCH => {
            sys_ioctl(fd, 2, arg as u64);
            if reply != 0 {
                sys_ipc_send(reply, &[1u8]);
            }
        }
        _ => {}
    }
}

/// Mix every buffered stream into one chunk, write it, then ack contributors and age
/// out silent streams. Returns false on a hardware write error.
fn mix_flush(streams: &mut [Stream], fd: u64, out: &mut [u8], acc: &mut [i32]) -> bool {
    let contributors = streams.iter().filter(|s| s.has).count();
    let mut ok = true;

    if contributors == 1 {
        let s = streams.iter().find(|s| s.has).unwrap();
        ok = write_all(fd, &s.buf);
    } else if contributors > 1 {
        for a in acc.iter_mut() {
            *a = 0;
        }
        for s in streams.iter().filter(|s| s.has) {
            for i in 0..SAMPLES {
                let v = i16::from_le_bytes([s.buf[i * 2], s.buf[i * 2 + 1]]);
                acc[i] += v as i32;
            }
        }
        for i in 0..SAMPLES {
            let clamped = acc[i].clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let b = clamped.to_le_bytes();
            out[i * 2] = b[0];
            out[i * 2 + 1] = b[1];
        }
        ok = write_all(fd, out);
    }

    // Ack contributors (releasing them to send the next chunk); age out silent streams.
    for s in streams.iter_mut() {
        if s.reply == 0 {
            continue;
        }
        if s.has {
            sys_ipc_send(s.reply, &[1u8]);
            s.has = false;
            s.miss = 0;
        } else {
            s.miss += 1;
            if s.miss >= MAX_MISS {
                s.reply = 0;
            }
        }
    }
    ok
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let fd = sys_open(b"/dev/audio");
    if fd == u64::MAX {
        println!("audiod: /dev/audio not found");
        sys_exit(1);
    }

    let chan = sys_ipc_open(b"audio");
    if chan == u64::MAX {
        println!("audiod: cannot open 'audio' channel");
        sys_close(fd);
        sys_exit(1);
    }

    sys_signal_catch(1);
    println!("audiod: ready (channel 'audio', mixing up to {} streams)", MAX_STREAMS);

    let mut streams: alloc::vec::Vec<Stream> = alloc::vec::Vec::new();
    for _ in 0..MAX_STREAMS {
        streams.push(Stream { reply: 0, has: false, miss: 0, buf: alloc::vec![0u8; CHUNK_BYTES] });
    }
    let mut msg = alloc::vec![0u8; MSG_MAX];
    let mut out = alloc::vec![0u8; CHUNK_BYTES];
    let mut acc = alloc::vec![0i32; SAMPLES];

    loop {
        if sys_signal_check() != 0 {
            break;
        }

        // Block for the first request of the round, then drain everything else queued.
        let n = sys_ipc_recv(chan, &mut msg);
        if n == u64::MAX {
            continue;
        }
        ingest(&mut streams, fd, &msg, n as usize);
        drain(&mut streams, fd, chan, &mut msg);

        // Give concurrent streams a moment to deliver this round's chunk so they mix
        // together instead of alternating. Bounded so a departed client can't stall us.
        let mut tries = 0;
        loop {
            let active = streams.iter().filter(|s| s.reply != 0).count();
            let ready = streams.iter().filter(|s| s.has).count();
            if active == 0 || ready >= active || tries >= ALIGN_TRIES {
                break;
            }
            sys_sleep(1);
            drain(&mut streams, fd, chan, &mut msg);
            tries += 1;
        }

        if !mix_flush(&mut streams, fd, &mut out, &mut acc) {
            println!("audiod: /dev/audio write error");
        }
    }

    sys_ioctl(fd, 0, 0);
    sys_close(fd);
    sys_ipc_close(chan);
    println!("audiod: exiting");
    sys_exit(0);
}

/// Pull every immediately-available message off the channel into stream buffers.
fn drain(streams: &mut [Stream], fd: u64, chan: u64, msg: &mut [u8]) {
    for _ in 0..MAX_STREAMS * 2 {
        let m = sys_ipc_try_recv(chan, msg);
        if m == 0 || m == u64::MAX {
            break;
        }
        ingest(streams, fd, msg, m as usize);
    }
}
