#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    match number {
    // Process / Lifecycle
            SYS_EXIT => {
                if let Some(pid) = crate::scheduler::current_user_pid() {
                    crate::syscall::context::set_exiting_pid_tmp(pid);
                }
                process::exit_current();
                syscall::EXIT_TO_KERNEL
            }
            SYS_EXEC => sys_exec(arg0, arg1, arg2),
            SYS_THREAD_SPAWN => process::spawn_user_thread(arg0, arg1, arg2),
            SYS_THREAD_EXIT => {
                // Last thread of the process? Then exiting it exits the whole process.
                let pid = crate::scheduler::current_user_pid().unwrap_or(0);
                if pid != 0 && process::live_thread_count(pid) <= 1 {
                    crate::syscall::context::set_exiting_pid_tmp(pid);
                    process::exit_current();
                } else {
                    process::exit_current_thread();
                }
                syscall::EXIT_TO_KERNEL
            }
            SYS_THREAD_JOIN => {
                if process::join_current(arg0) {
                    syscall::BLOCK_TO_SCHEDULER
                } else {
                    0 // already exited
                }
            }
            SYS_THREAD_NEXT => crate::task::thread::next_thread_in_pid(arg0, arg1).unwrap_or(u64::MAX),
            SYS_THREAD_INFO => {
                if arg1 != 0 {
                    match crate::task::thread::thread_info(arg0) {
                        Some(info) => {
                            if crate::memory::uaccess::write_value(arg1, info) { 0 } else { u64::MAX }
                        }
                        None => u64::MAX,
                    }
                } else {
                    u64::MAX
                }
            }
            SYS_SIGKILL => { process::kill_pid(arg0); 0 }
            SYS_SIGINT_FG => {
                let leaf = process::foreground_leaf(arg0);
                if leaf != 0 && leaf != arg0 { process::send_sigint(leaf); 1 } else { 0 }
            }
            SYS_WAIT => sys_wait(arg0),
            SYS_PROCESS_INFO => {
                if arg1 != 0 {
                    match process::info(arg0) {
                        Some(info) => {
                            if crate::memory::uaccess::write_value(arg1, info) { 0 } else { u64::MAX }
                        }
                        None => u64::MAX,
                    }
                } else {
                    match arg0 {
                        0 => process::current_pid(),
                        1 => process::count(),
                        2 => process::first_pid().unwrap_or(0),
                        _ => u64::MAX,
                    }
                }
            }
            SYS_PROCESS_NEXT => process::next_pid_after(arg0).unwrap_or(u64::MAX),
            SYS_SLEEP => sys_sleep(arg0, arg1),

                _ => u64::MAX,
    }
}
