#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, _arg2: u64) -> u64 {
    match number {
    // Hardware / Driver
            SYS_PCI_INFO => sys_pci_info(arg0, arg1),
            SYS_IOPORT_REQUEST => {
                let caller = crate::scheduler::current_user_pid().unwrap_or(0);
                if process::privilege_level(caller) > process::PrivilegeLevel::Driver { return u64::MAX; }
                let port  = arg0 as u16;
                let count = arg1 as u16;
                for i in 0..count { crate::arch::x86_64::gdt::iopb_allow_port(port + i); }
                0
            }
            SYS_IRQ_WAIT => {
                let caller = crate::scheduler::current_user_pid().unwrap_or(0);
                if process::privilege_level(caller) > process::PrivilegeLevel::Driver { return u64::MAX; }
                let irq = arg0 as u8;
                process::block_current(process::WaitTarget::Irq(irq));
                syscall::BLOCK_TO_SCHEDULER
            }
            SYS_DMA_ALLOC => sys_dma_alloc(arg0, arg1),
            SYS_DMA_FREE => sys_dma_free(arg0),
            SYS_PCI_BAR_MAP => sys_pci_bar_map(arg0, arg1),
            SYS_PCI_BAR_UNMAP => sys_pci_bar_unmap(arg0),

        
    // Keyboard (non-blocking). Returns the next key event word for the graphical focus
            // owner, or 0 if none. Low byte is the key code, KEY_RELEASE (0x100) is set on
            // release, and MOD_SHIFT/MOD_CTRL/MOD_ALT report the live modifier state. Every
            // key (characters, arrows, Ctrl/Shift/Alt/Caps, Esc, F1-F12) is reported. The
            // text/console read path (fd0) is separate and unaffected. Args are ignored.
            SYS_KEYBOARD_POLL => {
                if kbd_locked_out() { 0 } else { crate::drivers::keyboard::get_event().map(|e| e as u64).unwrap_or(0) }
            }

                _ => u64::MAX,
    }
}
