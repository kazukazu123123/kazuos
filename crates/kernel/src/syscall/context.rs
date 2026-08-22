use crate::arch::x86_64::smp::{MAX_CPUS, current_cpu_index};
use crate::util::SyncUnsafeCell;

pub static mut TSC_PER_MS: u64 = 3_000_000;

static EXITING_PID_TMPS: SyncUnsafeCell<[u64; MAX_CPUS]> = SyncUnsafeCell::new([0; MAX_CPUS]);

pub fn exiting_pid_tmp() -> u64 {
    unsafe { (*EXITING_PID_TMPS.0.get())[current_cpu_index()] }
}

pub fn set_exiting_pid_tmp(value: u64) {
    unsafe {
        (*EXITING_PID_TMPS.0.get())[current_cpu_index()] = value;
    }
}

static KERNEL_RETURN_STACKS: SyncUnsafeCell<[u64; MAX_CPUS]> = SyncUnsafeCell::new([0; MAX_CPUS]);

#[unsafe(no_mangle)]
pub extern "C" fn kernel_return_stack_ptr() -> *mut u64 {
    unsafe { (*KERNEL_RETURN_STACKS.0.get()).as_mut_ptr().add(current_cpu_index()) }
}

pub fn set_kernel_return_stack(value: u64) {
    unsafe {
        (*KERNEL_RETURN_STACKS.0.get())[current_cpu_index()] = value;
    }
}

static BLOCKING_RSP_TMPS: SyncUnsafeCell<[u64; MAX_CPUS]> = SyncUnsafeCell::new([0; MAX_CPUS]);

#[unsafe(no_mangle)]
pub extern "C" fn blocking_rsp_tmp_ptr() -> *mut u64 {
    unsafe { (*BLOCKING_RSP_TMPS.0.get()).as_mut_ptr().add(current_cpu_index()) }
}

pub fn blocking_rsp_tmp() -> u64 {
    unsafe { (*BLOCKING_RSP_TMPS.0.get())[current_cpu_index()] }
}

pub fn set_blocking_rsp_tmp(value: u64) {
    unsafe {
        (*BLOCKING_RSP_TMPS.0.get())[current_cpu_index()] = value;
    }
}
