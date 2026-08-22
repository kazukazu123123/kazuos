#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    match number {
    // System / Misc
            SYS_CPU_INFO => match arg0 {
                0 => crate::handlers::interrupts::timer_ticks(),
                1 => crate::handlers::interrupts::user_cpu_ticks(),
                2 => crate::handlers::interrupts::kernel_cpu_ticks(),
                3 => crate::handlers::interrupts::idle_cpu_ticks(),
                4 => crate::arch::x86_64::smp::cpu_count() as u64,
                5 => crate::arch::x86_64::smp::bsp_apic_id() as u64,
                6 => crate::arch::x86_64::smp::current_cpu_index() as u64,
                7 => crate::arch::x86_64::smp::apic_id_for_cpu_index(arg1 as usize).unwrap_or(0xff) as u64,
                8 => crate::handlers::interrupts::idle_cpu_ticks_for_cpu(arg1 as usize),
                9 => crate::handlers::interrupts::kernel_cpu_ticks_for_cpu(arg1 as usize),
                10 => crate::handlers::interrupts::user_cpu_ticks_for_cpu(arg1 as usize),
                pid => process::cpu_ticks(pid).unwrap_or(u64::MAX),
            },
            SYS_SHUTDOWN => crate::drivers::power::shutdown(),
            SYS_REBOOT => crate::drivers::power::reboot(),
            SYS_READDIR => sys_readdir(arg0, arg1, arg2),

                _ => u64::MAX,
    }
}
