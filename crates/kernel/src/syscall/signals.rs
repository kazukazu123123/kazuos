#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, _arg1: u64, _arg2: u64) -> u64 {
    match number {
    // Signals
            SYS_SIGNAL_CATCH => {
                if let Some(pid) = crate::scheduler::current_user_pid() { process::signal_set_catch(pid, arg0); }
                0
            }
            SYS_SIGNAL_CHECK => crate::scheduler::current_user_pid().map(process::signal_check_and_clear).unwrap_or(0),
            SYS_SIGTERM => {
                if arg0 == 0 { u64::MAX } else { process::send_sigterm(arg0); 0 }
            }
            _ => u64::MAX,
    }
}
