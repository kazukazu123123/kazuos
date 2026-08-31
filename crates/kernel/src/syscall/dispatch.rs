#[allow(unused_imports)]
use crate::{process, syscall};
use kazuos_abi::*;

pub(crate) extern "C" fn syscall_dispatch(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    // A remote kill that arrived while this thread was running was deferred (the
    // killer could not safely free our address space underneath us). Now that we
    // are back in the kernel on our own CPU, honor it: exit cleanly instead of
    // servicing the syscall against soon-to-be-freed memory.
    if let Some(pid) = crate::scheduler::current_user_pid() {
        if process::is_kill_pending(pid) {
            crate::syscall::context::set_exiting_pid_tmp(pid);
            process::exit_current();
            return syscall::EXIT_TO_KERNEL;
        }
    }
    match number {
        SYS_CONSOLE_WRITE
        | SYS_CURSOR_SAVE
        | SYS_CURSOR_RESTORE
        | SYS_CURSOR_DRAW
        | SYS_FB_ACQUIRE
        | SYS_FB_RELEASE
        | SYS_CONSOLE_SIZE => crate::syscall::console::handle(number, arg0, arg1, arg2),
        SYS_EXIT
        | SYS_EXEC
        | SYS_THREAD_SPAWN
        | SYS_THREAD_EXIT
        | SYS_THREAD_JOIN
        | SYS_THREAD_NEXT
        | SYS_THREAD_INFO
        | SYS_SIGKILL
        | SYS_SIGINT_FG
        | SYS_WAIT
        | SYS_PROCESS_INFO
        | SYS_PROCESS_NEXT
        | SYS_SLEEP => crate::syscall::process::handle(number, arg0, arg1, arg2),
        SYS_MEM_INFO
        | SYS_HEAP_ALLOC
        | SYS_HEAP_FREE => crate::syscall::memory::handle(number, arg0, arg1, arg2),
        SYS_SIGNAL_CATCH
        | SYS_SIGNAL_CHECK
        | SYS_SIGTERM => crate::syscall::signals::handle(number, arg0, arg1, arg2),
        SYS_IPC_OPEN
        | SYS_IPC_SEND
        | SYS_IPC_RECV
        | SYS_IPC_TRY_RECV
        | SYS_IPC_CLOSE => crate::syscall::ipc::handle(number, arg0, arg1, arg2),
        SYS_OPEN
        | SYS_CLOSE
        | SYS_READ
        | SYS_TRY_READ
        | SYS_WRITE
        | SYS_IOCTL
        | SYS_PIPE
        | SYS_CREATE
        | SYS_UNLINK
        | SYS_MKDIR
        | SYS_RMDIR => crate::syscall::fs::handle(number, arg0, arg1, arg2),
        SYS_PCI_INFO
        | SYS_IOPORT_REQUEST
        | SYS_IRQ_WAIT
        | SYS_DMA_ALLOC
        | SYS_DMA_FREE
        | SYS_PCI_BAR_MAP
        | SYS_PCI_BAR_UNMAP
        | SYS_KEYBOARD_POLL => crate::syscall::device::handle(number, arg0, arg1, arg2),
        SYS_CPU_INFO
        | SYS_SHUTDOWN
        | SYS_REBOOT
        | SYS_READDIR => crate::syscall::system::handle(number, arg0, arg1, arg2),
        SYS_MODULE_LOAD
        | SYS_MODULE_UNLOAD
        | SYS_MODULE_LIST
        | SYS_MODULE_INFO => crate::syscall::module::handle(number, arg0, arg1, arg2),
        _ => u64::MAX,
    }
}
