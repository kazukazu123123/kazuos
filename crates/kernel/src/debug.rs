use core::fmt::{self, Write};

use crate::drivers::serial;

pub fn serial_print(args: fmt::Arguments) {
    let _ = SerialWriter.write_fmt(args);
}

pub fn qemu_exit(code: u32) -> ! {
    unsafe {
        outl(0xF4, code);
    }
    loop {
        crate::util::pause();
    }
}

pub fn qemu_debug_break() {
    unsafe {
        core::arch::asm!("xchg bx, bx", options(nomem, nostack));
    }
}

struct SerialWriter;

impl Write for SerialWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        serial::write_str(s);
        Ok(())
    }
}

unsafe fn outl(port: u16, value: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack));
    }
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::debug::serial_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! serial_println {
    () => {
        $crate::serial_print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::serial_print!("{}\n", format_args!($($arg)*))
    };
}
