#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, _arg1: u64, _arg2: u64) -> u64 {
    match number {
    // Memory
            SYS_MEM_INFO => {
                if let Some(stats) = crate::pmm::stats() {
                    ((stats.total_kib() as u64) << 32) | stats.used_kib() as u64
                } else { 0 }
            }
            SYS_HEAP_ALLOC => sys_heap_alloc(arg0),
            SYS_HEAP_FREE => sys_heap_free(arg0),

                _ => u64::MAX,
    }
}
