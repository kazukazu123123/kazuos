#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

// Drive two concurrent mixer streams from a single process to exercise audiod's
// additive mixing deterministically: each round it sends one chunk of a 440Hz
// square wave and one chunk of a 660Hz square wave (distinct reply channels, so
// audiod treats them as two streams), then waits for both acks. audiod should
// mix both into every output chunk.

const SAMPLE_RATE: u32 = 48000;
const AMP: i16 = 5000;
const CHUNK_SAMPLES: usize = 1200;
const CHUNK_BYTES: usize = CHUNK_SAMPLES * 2 * 2;
const CHUNKS: usize = 120;
const HEADER: usize = 8;
const OP_PCM: u8 = 3;

struct Tone {
    half_period: u32,
    phase: u32,
    level: i16,
    reply: u64,
    msg: alloc::vec::Vec<u8>,
}

impl Tone {
    fn new(freq: u32, reply: u64) -> Self {
        let mut msg = alloc::vec![0u8; HEADER + CHUNK_BYTES];
        msg[0] = OP_PCM;
        msg[2..4].copy_from_slice(&(reply as u16).to_le_bytes());
        Tone { half_period: (SAMPLE_RATE / (2 * freq)).max(1), phase: 0, level: AMP, reply, msg }
    }

    fn fill(&mut self) {
        for i in 0..CHUNK_SAMPLES {
            let s = self.level.to_le_bytes();
            let off = HEADER + i * 4;
            self.msg[off..off + 2].copy_from_slice(&s);
            self.msg[off + 2..off + 4].copy_from_slice(&s);
            self.phase += 1;
            if self.phase >= self.half_period {
                self.phase = 0;
                self.level = -self.level;
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let chan = sys_ipc_open(b"audio");
    if chan == u64::MAX {
        println!("mixtest: 'audio' channel not found (is audiod running?)");
        sys_exit(1);
    }
    let ack_a = sys_ipc_open(b"mixtest.a");
    let ack_b = sys_ipc_open(b"mixtest.b");
    if ack_a == u64::MAX || ack_b == u64::MAX {
        println!("mixtest: cannot open reply channels");
        sys_exit(1);
    }

    println!("mixtest: mixing 440Hz + 660Hz for {} chunks", CHUNKS);

    let mut a = Tone::new(440, ack_a);
    let mut b = Tone::new(660, ack_b);
    let mut ack = [0u8; 4];

    for _ in 0..CHUNKS {
        a.fill();
        b.fill();
        // Submit both streams before waiting, so audiod has both queued and mixes them.
        if sys_ipc_send(chan, &a.msg) == u64::MAX || sys_ipc_send(chan, &b.msg) == u64::MAX {
            println!("mixtest: send error");
            break;
        }
        sys_ipc_recv(ack_a, &mut ack);
        sys_ipc_recv(ack_b, &mut ack);
    }

    sys_ipc_close(ack_a);
    sys_ipc_close(ack_b);
    sys_ipc_close(chan);
    println!("mixtest: done");
    sys_exit(0);
}
