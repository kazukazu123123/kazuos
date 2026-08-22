#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

pub mod allocator;
pub mod arch;
pub mod fs;
pub mod kmod;
pub mod boot;
pub mod console;
pub mod debug;
pub mod drivers;
pub mod handlers;
pub mod ipc;
pub mod init;
pub mod log;
pub mod memory;
pub mod pmm;
pub mod process;
pub mod scheduler;
pub mod syscall;
pub mod task;
pub mod terminal;
pub mod tty;
pub mod user_programs;
pub mod util;
pub mod vmm;

pub use kazuos_shared::{BootInfo, FramebufferInfo, MemoryMapEntry};

#[repr(C, align(4096))]
struct Stack([u8; 1024 * 1024]);
static mut STACK: Stack = Stack([0; 1024 * 1024]);

core::arch::global_asm!(
    ".global _start",
    "_start:",
    "    cli",
    "    lea rsp, [rip + {0} + 0x100000]",
    "    call kernel_main",
    "    cli",
    "    hlt",
    sym STACK,
);

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_info: &'static BootInfo) {
    unsafe {
        crate::arch::x86_64::gdt::init((core::ptr::addr_of!(STACK) as u64) + 1024 * 1024);
    }
    boot::init(boot_info);
    syscall::init();
    syscall::runtime::init();
    let _state = init::run(boot_info);
    let initramfs = boot_info.initrd_slice();
    if initramfs.is_empty() {
        panic!("no initrd provided by bootloader");
    }
    if let Err(error) = crate::fs::vfs::init(initramfs) {
        panic!("initramfs invalid: {:?}", error);
    }
    // Disable interrupts while spawning processes so the timer cannot preempt the
    // kernel before all processes are fully configured. This MUST cover module
    // loading too: a module (e.g. ps2mouse.kkm) entered via the timer path before
    // enter_next_process() runs would make its first blocking syscall with
    // KERNEL_RETURN_STACK still 0 → rsp=0 → double fault.
    unsafe { core::arch::asm!("cli"); }
    kmod::load_from_list("/modules/modules.list");
    let pid = crate::task::exec::spawn("/bin/shell.kxe");
    if pid == 0 {
        panic!("shell spawn failed");
    }
    // The boot shell talks to the kernel console over standard fds (it shares the one
    // unified line editor with the GUI terminal, reading fd 0 and writing fd 1), so it
    // needs the default console stdio the same way a user-spawned process gets it.
    crate::fs::fd::alloc_fd_at(pid, 0, crate::fs::fd::FdEntry::ConsoleIn);
    crate::fs::fd::alloc_fd_at(pid, 1, crate::fs::fd::FdEntry::ConsoleOut);
    crate::fs::fd::alloc_fd_at(pid, 2, crate::fs::fd::FdEntry::ConsoleOut);
    unsafe {
        crate::arch::x86_64::smp::start_aps();
    }

    // Install the BSP's scheduler restart stack: the fixed top of the boot stack.
    // Every blocking syscall / process exit resets rsp to this and re-runs the
    // scheduler. It must be set before the first user thread runs, because some
    // entry paths (timer, blocking-resume) do not set it themselves.
    crate::syscall::context::set_kernel_return_stack(core::ptr::addr_of!(STACK) as u64 + 1024 * 1024);

    crate::scheduler::enter_next_process();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    crate::log_fatal!("{}", info);
    loop {
        util::pause();
    }
}
