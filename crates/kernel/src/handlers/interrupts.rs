use core::arch::global_asm;

use crate::drivers::{keyboard, lapic, pic};
use crate::util::{SyncUnsafeCell, rdtsc};

static mut TIMER_TICKS: u64 = 0;
static mut KERNEL_TICKS: u64 = 0;
static mut IDLE_TICKS: u64 = 0;
static mut USE_IOAPIC: bool = false;
static mut KEYBOARD_POLLING: bool = false;
static mut LAST_TSC: u64 = 0;

static KERNEL_TICKS_PER_CPU: SyncUnsafeCell<[u64; crate::smp::MAX_CPUS]> =
    SyncUnsafeCell::new([0; crate::smp::MAX_CPUS]);
static IDLE_TICKS_PER_CPU: SyncUnsafeCell<[u64; crate::smp::MAX_CPUS]> =
    SyncUnsafeCell::new([0; crate::smp::MAX_CPUS]);
static USER_TICKS_PER_CPU: SyncUnsafeCell<[u64; crate::smp::MAX_CPUS]> =
    SyncUnsafeCell::new([0; crate::smp::MAX_CPUS]);

pub fn timer_ticks() -> u64 {
    unsafe { TIMER_TICKS }
}

pub fn kernel_cpu_ticks() -> u64 {
    unsafe { KERNEL_TICKS }
}

pub fn idle_cpu_ticks() -> u64 {
    unsafe { IDLE_TICKS }
}

pub fn kernel_cpu_ticks_for_cpu(cpu: usize) -> u64 {
    unsafe { (*KERNEL_TICKS_PER_CPU.0.get()).get(cpu).copied().unwrap_or(0) }
}

pub fn idle_cpu_ticks_for_cpu(cpu: usize) -> u64 {
    unsafe { (*IDLE_TICKS_PER_CPU.0.get()).get(cpu).copied().unwrap_or(0) }
}

pub fn user_cpu_ticks_for_cpu(cpu: usize) -> u64 {
    unsafe { (*USER_TICKS_PER_CPU.0.get()).get(cpu).copied().unwrap_or(0) }
}

pub fn set_use_ioapic(value: bool) {
    unsafe {
        USE_IOAPIC = value;
    }
}

pub fn set_keyboard_polling(value: bool) {
    unsafe {
        KEYBOARD_POLLING = value;
    }
}

/// Restore the CR3 that was active when an interrupt preempted code we are
/// about to resume unchanged. No-op (no TLB flush) when it never changed.
#[inline]
fn restore_entry_cr3(entry_cr3: u64) {
    if crate::vmm::active_cr3() != entry_cr3 {
        unsafe { crate::vmm::switch_cr3(entry_cr3) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn timer_handler_inner(saved_rsp: u64, cs_ring: u64) -> u64 {
    unsafe {
        // CR3 active when we interrupted. A ring0 syscall runs on the caller's
        // (user) CR3 because int 0x80 does not switch address spaces. If the
        // bookkeeping below (process reaping, wakeups, etc.) switches CR3 to the
        // kernel's, any path that resumes the *interrupted* context unchanged
        // must restore this value first — otherwise the syscall continues a
        // copy-from-user on the wrong CR3 and page-faults. Thread-switch paths
        // install the target thread's CR3 themselves and are unaffected.
        let entry_cr3 = crate::vmm::active_cr3();
        let now = rdtsc();
        let delta = if LAST_TSC == 0 {
            0
        } else {
            now.saturating_sub(LAST_TSC)
        };
        LAST_TSC = now;
        TIMER_TICKS += 1;
        // Verbose-only liveness beat for the headless test harness. The timer
        // keeps firing while idle (the idle loop only `hlt`s), so this continues
        // during legitimate quiet periods but STOPS on a real total hang (a held
        // serial/thread lock blocks every CPU's beat) — letting the pipeline tell
        // "idle" from "frozen" instead of guessing from raw serial silence.
        let hb_ticks = TIMER_TICKS;
        if hb_ticks % 4000 == 0 && crate::init::heartbeat_log() {
            crate::serial_println!("HEARTBEAT ticks={} cpu={}", hb_ticks, crate::smp::current_cpu_index());
        }
        if delta != 0 {
            let cpu = crate::smp::current_cpu_index();
            if let Some(pid) = crate::scheduler::current_user_pid() {
                crate::process::add_cpu_ticks(pid, delta);
                if let Some(v) = (*USER_TICKS_PER_CPU.0.get()).get_mut(cpu) {
                    *v = v.saturating_add(delta);
                }
            } else if crate::scheduler::is_idle() {
                IDLE_TICKS = IDLE_TICKS.saturating_add(delta);
                if let Some(v) = (*IDLE_TICKS_PER_CPU.0.get()).get_mut(cpu) {
                    *v = v.saturating_add(delta);
                }
            } else {
                KERNEL_TICKS = KERNEL_TICKS.saturating_add(delta);
                crate::process::add_cpu_ticks(0, delta);
                if let Some(v) = (*KERNEL_TICKS_PER_CPU.0.get()).get_mut(cpu) {
                    *v = v.saturating_add(delta);
                }
            }
        }
        if KEYBOARD_POLLING {
            keyboard::poll();
        }
        // Wake any processes sleeping on a timer whose deadline has passed.
        crate::process::wake_timer_sleepers(now);
        // Wake any processes waiting for the next tick (SLEEP_UNIT_TICK).
        crate::process::wake_tick_sleepers();

        let current_tid = if cs_ring == 3 {
            crate::scheduler::current_user_tid().unwrap_or(0)
        } else {
            crate::process::current_tid()
        };
        let current_pid = if current_tid != 0 {
            crate::task::thread::thread_pid(current_tid).unwrap_or(0)
        } else {
            0
        };

        if current_tid != 0 {
            if cs_ring == 3 {
                crate::scheduler::save_user_context(current_tid, saved_rsp);
                crate::task::thread::set_kernel_preempted(current_tid, false);
            } else {
                crate::scheduler::save_kernel_context(current_tid, saved_rsp);
                crate::task::thread::set_kernel_preempted(current_tid, true);
            }
            crate::task::thread::set_ready(current_tid);
            if current_pid != 0 {
                crate::process::set_ready(current_pid);
            }
        }

        let next_tid = crate::scheduler::schedule_next(current_tid);

        lapic::eoi();

        if next_tid == 0 {
            crate::scheduler::set_current_user_tid(None);
            restore_entry_cr3(entry_cr3);
            return 0;
        }

        let next_pid = crate::task::thread::thread_pid(next_tid).unwrap_or(0);
        crate::scheduler::set_idle(false);

        if cs_ring == 0 && next_tid == current_tid {
            restore_entry_cr3(entry_cr3);
            return 0;
        }

        // Don't enter a brand-new user thread directly from a kernel-mode
        // interrupt: its per-CPU kernel return stack has not been initialized
        // yet. Let the kernel code path call enter_next_process(), which sets
        // the return stack before the first iretq to userspace.
        if cs_ring == 0
            && next_tid != current_tid
            && crate::task::thread::user_context(next_tid).is_some()
            && *crate::user::kernel_return_stack_ptr() == 0
        {
            restore_entry_cr3(entry_cr3);
            return 0;
        }

        if crate::task::thread::is_kernel_preempted(next_tid) {
            if let Some(kernel_rsp) = crate::task::thread::kernel_rsp(next_tid) {
                if let Some(ctx) = crate::task::thread::user_context(next_tid) {
                    if ctx.cr3 != 0 {
                        crate::vmm::switch_cr3(ctx.cr3);
                    }
                    crate::scheduler::set_current_user_tid(Some(next_tid));
                } else {
                    crate::scheduler::set_current_user_tid(None);
                }
                if let Some(top) = crate::task::thread::kernel_stack_top(next_tid) {
                    crate::gdt::set_kernel_stack_top(top);
                }
                crate::process::set_running(next_pid);
                return kernel_rsp;
            }
        }

        crate::task::thread::apply_blocking_return_if_pending(next_tid);

        if let Some(ctx) = crate::task::thread::user_context(next_tid) {
            crate::scheduler::set_current_user_tid(Some(next_tid));
            if let Some(top) = crate::task::thread::kernel_stack_top(next_tid) {
                crate::gdt::set_kernel_stack_top(top);
            }
            crate::process::set_running(next_pid);
            if ctx.cr3 != 0 {
                crate::vmm::switch_cr3(ctx.cr3);
            }
            if cs_ring == 3 {
                crate::scheduler::restore_user_frame(saved_rsp, ctx);
                0
            } else {
                let temp_rsp = crate::scheduler::setup_user_frame_for_timer_on_temp_stack(ctx);
                temp_rsp
            }
        } else if let Some(kernel_rsp) = crate::task::thread::kernel_rsp(next_tid) {
            crate::scheduler::set_current_user_tid(None);
            crate::process::set_running(next_pid);
            kernel_rsp
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn keyboard_handler_inner() {
    let entry_cr3 = crate::vmm::active_cr3();
    unsafe {
        keyboard::poll();
        if USE_IOAPIC {
            lapic::eoi();
        } else {
            pic::eoi(1);
        }
    }
    restore_entry_cr3(entry_cr3);
}

#[unsafe(no_mangle)]
pub extern "C" fn mouse_irq_handler_inner() {
    let entry_cr3 = crate::vmm::active_cr3();
    unsafe {
        crate::process::wakeup_irq_waiter(12);
        if USE_IOAPIC {
            lapic::eoi();
        } else {
            pic::eoi(12);
        }
    }
    restore_entry_cr3(entry_cr3);
}

#[unsafe(no_mangle)]
pub extern "C" fn hda_irq_handler_inner() {
    let entry_cr3 = crate::vmm::active_cr3();
    unsafe {
        crate::drivers::hda::on_interrupt();
        if USE_IOAPIC {
            lapic::eoi();
        } else {
            let irq = crate::drivers::hda::irq();
            if irq != 0 && irq != 255 {
                pic::eoi(irq);
            }
        }
    }
    restore_entry_cr3(entry_cr3);
}

global_asm!(
    ".global timer_handler_asm",
    "timer_handler_asm:",
    // Push all registers in the order expected by InterruptFrame.
    // r11 is pushed BEFORE any modification so the interrupted code's r11 is saved correctly.
    // CS lives at [rsp + 15*8 + 8] = [rsp + 128] after all 15 pushes.
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    mov rdi, rsp",           // saved_rsp after our pushes
    "    mov rsi, [rsp + 128]",   // CS at rsp+15*8+8 (CPU frame rip+cs+rflags[+rsp+ss])
    "    and rsi, 3",
    "    call timer_handler_inner",
    "    test rax, rax",
    "    jz 1f",
    "    mov rsp, rax",
    "1:",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
);

unsafe extern "C" {
    fn timer_handler_asm();
}

global_asm!(
    ".global keyboard_handler_asm",
    "keyboard_handler_asm:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    call keyboard_handler_inner",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
);

unsafe extern "C" {
    fn keyboard_handler_asm();
}

global_asm!(
    ".global mouse_irq_handler_asm",
    "mouse_irq_handler_asm:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    call mouse_irq_handler_inner",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
);

unsafe extern "C" {
    fn mouse_irq_handler_asm();
}

global_asm!(
    ".global hda_irq_handler_asm",
    "hda_irq_handler_asm:",
    "    push rax",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push rbx",
    "    push rbp",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    "    call hda_irq_handler_inner",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop rbp",
    "    pop rbx",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rax",
    "    iretq",
);

unsafe extern "C" {
    fn hda_irq_handler_asm();
}

pub fn timer_handler_addr() -> u64 {
    timer_handler_asm as *const () as usize as u64
}

pub fn keyboard_handler_addr() -> u64 {
    keyboard_handler_asm as *const () as usize as u64
}

pub fn mouse_handler_addr() -> u64 {
    mouse_irq_handler_asm as *const () as usize as u64
}

pub fn hda_handler_addr() -> u64 {
    hda_irq_handler_asm as *const () as usize as u64
}
