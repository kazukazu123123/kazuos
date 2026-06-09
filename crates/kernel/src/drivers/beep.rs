use core::arch::asm;

unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

unsafe fn inb(port: u16) -> u8 {
    unsafe {
        let val: u8;
        asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack));
        val
    }
}

pub fn off() {
    unsafe {
        let spk = inb(0x61);
        outb(0x61, spk & 0xFC);
    }
}

pub fn start(frequency: u32) {
    if frequency == 0 {
        off();
        return;
    }
    let div = 1193182u32 / frequency;
    unsafe {
        outb(0x43, 0xB6);
        outb(0x42, (div & 0xFF) as u8);
        outb(0x42, ((div >> 8) & 0xFF) as u8);
        let spk = inb(0x61);
        outb(0x61, spk | 0x03);
    }
}
