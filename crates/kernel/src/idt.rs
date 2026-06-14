use crate::util::SyncUnsafeCell;
use core::arch::asm;

static IDT: SyncUnsafeCell<Idt> = SyncUnsafeCell::new(Idt {
    entries: [IdtEntry::missing(); 256],
});

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

#[derive(Clone, Copy)]
#[repr(C, align(16))]
pub struct IdtEntry {
    pub low: u64,
    pub high: u64,
}

impl IdtEntry {
    pub const fn missing() -> Self {
        Self { low: 0, high: 0 }
    }

    pub fn new(handler: u64, selector: u16, ist: u8, type_attr: u8) -> Self {
        let low = (handler & 0xFFFF)
            | ((selector as u64) << 16)
            | ((ist as u64 & 0x7) << 32)
            | ((type_attr as u64) << 40)
            | (((handler >> 16) & 0xFFFF) << 48);
        let high = handler >> 32;
        Self { low, high }
    }
}

#[repr(C, packed)]
pub struct IdtDescriptor {
    limit: u16,
    base: u64,
}

pub struct Idt {
    pub entries: [IdtEntry; 256],
}

impl Idt {
    pub fn set_handler(&mut self, index: u8, handler: u64) {
        self.entries[index as usize] = IdtEntry::new(handler, crate::gdt::KERNEL_CODE, 0, 0x8E);
    }

    pub fn set_handler_with_ist(&mut self, index: u8, handler: u64, ist: u8) {
        self.entries[index as usize] = IdtEntry::new(handler, crate::gdt::KERNEL_CODE, ist, 0x8E);
    }

    /// Interrupt gate with DPL=3 (0xEE) clears IF during syscall handling
    /// so timer and keyboard interrupts cannot preempt the kernel, while
    /// still allowing the handler to be invoked from ring 3 via int 0x80.
    pub fn set_user_handler(&mut self, index: u8, handler: u64) {
        self.entries[index as usize] = IdtEntry::new(handler, crate::gdt::KERNEL_CODE, 0, 0xEE);
    }

    pub fn load(&self) {
        let desc = IdtDescriptor {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: self.entries.as_ptr() as u64,
        };
        unsafe {
            asm!("lidt [{}]", in(reg) &desc, options(nostack, preserves_flags));
        }
    }
}

pub unsafe fn load_idt() {
    unsafe {
        let idt = &*IDT.0.get();
        idt.load();
    }
}

pub(crate) unsafe fn init(keyboard: u64, timer: u64, syscall: u64, mouse: u64, hda: u64) {
    unsafe {
        let idt = &mut *IDT.0.get();
        for vector in 0u8..=31 {
            if let Some(handler) = crate::handlers::faults::handler_addr(vector) {
                if vector == 8 {
                    idt.set_handler_with_ist(vector, handler, 1);
                } else {
                    idt.set_handler(vector, handler);
                }
            }
        }
        idt.set_handler(0x21, keyboard);
        idt.set_handler(0x2C, mouse);
        idt.set_handler(0x30, timer);
        idt.set_handler(0x31, hda);
        idt.set_user_handler(0x80, syscall);
        idt.load();
    }
}
