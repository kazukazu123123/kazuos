#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    match number {
    // File I/O
            SYS_OPEN => sys_open(arg0, arg1),
            SYS_CLOSE => sys_close(arg0),
            SYS_READ => sys_read(arg0, arg1, arg2),
            SYS_TRY_READ => sys_try_read(arg0, arg1, arg2),
            SYS_WRITE => sys_write(arg0, arg1, arg2),
            SYS_IOCTL => sys_ioctl(arg0, arg1, arg2),
            SYS_PIPE => sys_pipe(arg0),
            SYS_CREATE => sys_create(arg0, arg1),
            SYS_UNLINK => sys_unlink(arg0, arg1),
            SYS_MKDIR => sys_mkdir(arg0, arg1),
            SYS_RMDIR => sys_rmdir(arg0, arg1),

                _ => u64::MAX,
    }
}
