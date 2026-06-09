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

const P1C: u16 = 0x20;
const P1D: u16 = 0x21;
const P2C: u16 = 0xA0;
const P2D: u16 = 0xA1;

pub(crate) unsafe fn init() {
    unsafe {
        let m1 = inb(P1D);
        let m2 = inb(P2D);

        // ICW1: start init, expect ICW4
        outb(P1C, 0x11);
        outb(P2C, 0x11);
        // ICW2: vector offsets
        outb(P1D, 0x20);
        outb(P2D, 0x28);
        // ICW3: master/slave wiring
        outb(P1D, 0x04);
        outb(P2D, 0x02);
        // ICW4: 8086 mode
        outb(P1D, 0x01);
        outb(P2D, 0x01);
        // Restore masks (all masked by default)
        outb(P1D, m1 | 0xFF);
        outb(P2D, m2 | 0xFF);
    }
}

pub(crate) unsafe fn mask_all() {
    unsafe {
        outb(P1D, 0xFF);
        outb(P2D, 0xFF);
    }
}

pub(crate) unsafe fn unmask_irq(irq: u8) {
    unsafe {
        if irq < 8 {
            outb(P1D, inb(P1D) & !(1 << irq));
        } else {
            outb(P2D, inb(P2D) & !(1 << (irq - 8)));
        }
    }
}

pub(crate) unsafe fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            outb(P2C, 0x20);
        }
        outb(P1C, 0x20);
    }
}
