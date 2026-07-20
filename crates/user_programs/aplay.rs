#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

// Stream a square-wave tone THROUGH the audiod IPC server, proving the audio
// path (aplay -> "audio" channel -> audiod mixer -> /dev/audio -> HDA). Each
// chunk is ack'd by audiod before the next is sent, so nothing is dropped and
// the producer is paced to real time. Run several at once (e.g.
// `audiod &`, `aplay 440 &`, `aplay 660`) to hear them mixed together.
//
// Usage: aplay [frequency_hz]   (default 440)

const SAMPLE_RATE: u32 = 48000;
const AMPLITUDE: i16 = 7000;
const CHUNK_SAMPLES: usize = 1200; // 25ms @ 48kHz stereo
const CHUNK_BYTES: usize = CHUNK_SAMPLES * 2 * 2; // s16le stereo
const CHUNKS_TO_PLAY: usize = 120; // 3 seconds

const HEADER: usize = 8;
const OP_PCM: u8 = 3;

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
        let p = ptr as *const u8;
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
        }
        Some(core::slice::from_raw_parts(p, len))
    }
}

fn parse_u32(s: &[u8]) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    let mut v: u32 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as u32)?;
    }
    Some(v)
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    let freq = nth_arg(argc, argv, 0)
        .and_then(parse_u32)
        .filter(|&f| f >= 20 && f <= 20000)
        .unwrap_or(440);

    let chan = sys_ipc_open(b"audio");
    if chan == u64::MAX {
        println!("aplay: 'audio' channel not found (is audiod running?)");
        sys_exit(1);
    }

    // Per-client reply channel for backpressure acks. Name it per frequency so two
    // concurrent players get distinct channels (and thus distinct mixer streams).
    let mut name = *b"aplay.ack.00000";
    let mut f = freq;
    for i in (10..15).rev() {
        name[i] = b'0' + (f % 10) as u8;
        f /= 10;
    }
    let reply = sys_ipc_open(&name);
    if reply == u64::MAX {
        println!("aplay: cannot open reply channel");
        sys_exit(1);
    }

    println!("aplay: streaming {}Hz for {} chunks via audiod", freq, CHUNKS_TO_PLAY);

    let mut msg = alloc::vec![0u8; HEADER + CHUNK_BYTES];
    msg[0] = OP_PCM;
    msg[2..4].copy_from_slice(&(reply as u16).to_le_bytes());

    // Square wave: hold +A for half a period, -A for the other half.
    let half_period = (SAMPLE_RATE / (2 * freq)).max(1);
    let mut phase = 0u32;
    let mut level = AMPLITUDE;

    let mut ack = [0u8; 4];
    for _ in 0..CHUNKS_TO_PLAY {
        for i in 0..CHUNK_SAMPLES {
            let s = level.to_le_bytes();
            let off = HEADER + i * 4;
            msg[off..off + 2].copy_from_slice(&s);
            msg[off + 2..off + 4].copy_from_slice(&s);
            phase += 1;
            if phase >= half_period {
                phase = 0;
                level = -level;
            }
        }
        if sys_ipc_send(chan, &msg) == u64::MAX {
            println!("aplay: send error");
            break;
        }
        // Wait for audiod to mix this chunk before producing the next.
        sys_ipc_recv(reply, &mut ack);
    }

    sys_ipc_close(reply);
    sys_ipc_close(chan);
    println!("aplay: done {}Hz", freq);
    sys_exit(0);
}
