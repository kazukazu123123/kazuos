#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    match number {
    // IPC
            SYS_IPC_OPEN => {
                if arg0 == 0 || arg1 == 0 { u64::MAX }
                else {
                    match crate::memory::uaccess::read_bytes(arg0, arg1) {
                        Some(name) => ipc::open(&name),
                        None => u64::MAX,
                    }
                }
            }
            SYS_IPC_SEND => {
                let channel_id = arg0;
                let buf_ptr    = arg1;
                let buf_len    = arg2 as usize;
                if buf_ptr == 0 || buf_len == 0 { return u64::MAX; }
                let Some(data) = crate::memory::uaccess::read_bytes(buf_ptr, buf_len as u64) else {
                    return u64::MAX;
                };
                let sender = crate::scheduler::current_user_pid().unwrap_or(0);
                match ipc::try_send(channel_id, sender, &data) {
                    ipc::SendResult::Ok => 0,
                    ipc::SendResult::Error => u64::MAX,
                    ipc::SendResult::Block => {
                        if let Some(pid) = crate::scheduler::current_user_pid() {
                            ipc::add_send_waiter(channel_id, pid);
                            process::set_wait_target(pid, process::WaitTarget::Ipc(channel_id));
                            process::set_sleeping(pid);
                        }
                        syscall::BLOCK_TO_SCHEDULER
                    }
                }
            }
            SYS_IPC_RECV => {
                let channel_id = arg0;
                let buf_ptr    = arg1;
                let buf_len    = arg2 as usize;
                if buf_ptr == 0 || buf_len == 0 { return u64::MAX; }
                if !crate::memory::uaccess::validate_range(buf_ptr, buf_len as u64, true) { return u64::MAX; }
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };
                let pid = crate::scheduler::current_user_pid().unwrap_or(0);
                // try_recv registers the waiter and marks it sleeping atomically on Block.
                match ipc::try_recv(channel_id, buf, pid) {
                    ipc::RecvResult::Ok(len) => len as u64,
                    ipc::RecvResult::Error   => u64::MAX,
                    ipc::RecvResult::Block   => syscall::BLOCK_TO_SCHEDULER,
                }
            }
            SYS_IPC_TRY_RECV => {
                let channel_id = arg0;
                let buf_ptr    = arg1;
                let buf_len    = arg2 as usize;
                if buf_ptr == 0 || buf_len == 0 { return u64::MAX; }
                if !crate::memory::uaccess::validate_range(buf_ptr, buf_len as u64, true) { return u64::MAX; }
                let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };
                match ipc::try_recv_nonblock(channel_id, buf) {
                    ipc::RecvResult::Ok(len) => len as u64,
                    ipc::RecvResult::Error   => u64::MAX,
                    // No message available right now; caller polls again later.
                    ipc::RecvResult::Block   => 0,
                }
            }
            SYS_IPC_CLOSE => { ipc::close(arg0); 0 }

                _ => u64::MAX,
    }
}
