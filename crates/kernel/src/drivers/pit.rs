use crate::util::{inb, outb};

const PIT_HZ: u32 = 1_193_182;
const CHANNEL_2: u16 = 0x42;
const COMMAND: u16 = 0x43;
const SPEAKER: u16 = 0x61;

pub fn sleep_ms(ms: u32) {
    for _ in 0..ms {
        sleep_one_ms();
    }
}

pub fn sleep_oneshot_ms(ms: u32) {
    let count = ((PIT_HZ as u64 * ms as u64) / 1000).min(u16::MAX as u64) as u16;
    unsafe {
        let speaker = inb(SPEAKER);
        outb(SPEAKER, (speaker & !0x02) | 0x01);
        outb(COMMAND, 0xB0);
        outb(CHANNEL_2, (count & 0xFF) as u8);
        outb(CHANNEL_2, (count >> 8) as u8);
        while inb(SPEAKER) & 0x20 == 0 {}
        outb(SPEAKER, speaker);
    }
}

fn sleep_one_ms() {
    let count = (PIT_HZ / 1000) as u16;
    unsafe {
        let speaker = inb(SPEAKER);
        outb(SPEAKER, (speaker & !0x02) | 0x01);
        outb(COMMAND, 0xB0);
        outb(CHANNEL_2, (count & 0xFF) as u8);
        outb(CHANNEL_2, (count >> 8) as u8);
        loop {
            if inb(SPEAKER) & 0x20 != 0 {
                break;
            }
        }
        outb(SPEAKER, speaker);
    }
}
