use core::arch::global_asm;

use crate::drivers::{keyboard, lapic, mouse, pic};
use crate::util::rdtsc;

static mut TIMER_TICKS: u64 = 0;
static mut KERNEL_TICKS: u64 = 0;
static mut IDLE_TICKS: u64 = 0;
static mut USE_IOAPIC: bool = false;
static mut KEYBOARD_POLLING: bool = false;
static mut LAST_TSC: u64 = 0;

pub fn timer_ticks() -> u64 {
    unsafe { TIMER_TICKS }
}

pub fn kernel_cpu_ticks() -> u64 {
    unsafe { KERNEL_TICKS }
}

pub fn idle_cpu_ticks() -> u64 {
    unsafe { IDLE_TICKS }
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

#[unsafe(no_mangle)]
pub extern "C" fn timer_handler_inner(saved_rsp: u64, cs_ring: u64) -> u64 {
    unsafe {
        let now = rdtsc();
        let delta = if LAST_TSC == 0 {
            0
        } else {
            now.saturating_sub(LAST_TSC)
        };
        LAST_TSC = now;
        TIMER_TICKS += 1;
        if delta != 0 {
            if let Some(pid) = crate::scheduler::current_user_pid() {
                crate::process::add_cpu_ticks(pid, delta);
            } else if crate::scheduler::is_idle() {
                IDLE_TICKS = IDLE_TICKS.saturating_add(delta);
            } else {
                KERNEL_TICKS = KERNEL_TICKS.saturating_add(delta);
                crate::process::add_cpu_ticks(0, delta);
            }
        }
        if KEYBOARD_POLLING {
            keyboard::poll();
            mouse::poll();
        }
        // Wake any processes sleeping on a timer whose deadline has passed.
        crate::process::wake_timer_sleepers(now);

        let current_pid = if cs_ring == 3 {
            crate::scheduler::current_user_pid().unwrap_or(0)
        } else {
            crate::process::current_pid()
        };

        if current_pid != 0 {
            if cs_ring == 3 {
                crate::scheduler::save_user_context(current_pid, saved_rsp);
                crate::process::set_kernel_preempted(current_pid, false);
            } else {
                crate::scheduler::save_kernel_context(current_pid, saved_rsp);
                crate::process::set_kernel_preempted(current_pid, true);
            }
            crate::process::set_ready(current_pid);
        }

        let next_pid = crate::scheduler::schedule_next(current_pid);

        lapic::eoi();

        if next_pid == 0 {
            crate::scheduler::set_current_user_pid(None);
            return 0;
        }

        crate::scheduler::set_idle(false);

        if cs_ring == 0 && next_pid == current_pid {
            return 0;
        }

        // If next_pid was preempted mid-syscall (kernel mode), resume via kernel stack.
        if crate::process::is_kernel_preempted(next_pid) {
            if let Some(kernel_rsp) = crate::process::kernel_rsp(next_pid) {
                if let Some(ctx) = crate::process::user_context(next_pid) {
                    if ctx.cr3 != 0 {
                        crate::vmm::switch_cr3(ctx.cr3);
                    }
                    crate::scheduler::set_current_user_pid(Some(next_pid));
                } else {
                    crate::scheduler::set_current_user_pid(None);
                }
                if let Some(top) = crate::process::kernel_stack_top(next_pid) {
                    crate::gdt::set_kernel_stack_top(top);
                }
                crate::process::set_running(next_pid);
                return kernel_rsp;
            }
        }

        // If next_pid was woken from a blocking syscall, update its user_context from the
        // saved int-0x80 frame so iretq lands at the correct return address.
        crate::process::apply_blocking_return_if_pending(next_pid);

        if let Some(ctx) = crate::process::user_context(next_pid) {
            crate::scheduler::set_current_user_pid(Some(next_pid));
            if let Some(top) = crate::process::kernel_stack_top(next_pid) {
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
        } else if let Some(kernel_rsp) = crate::process::kernel_rsp(next_pid) {
            crate::scheduler::set_current_user_pid(None);
            crate::process::set_running(next_pid);
            kernel_rsp
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn keyboard_handler_inner() {
    unsafe {
        keyboard::poll();
        if USE_IOAPIC {
            lapic::eoi();
        } else {
            pic::eoi(1);
        }
    }
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

pub fn timer_handler_addr() -> u64 {
    timer_handler_asm as *const () as usize as u64
}

pub fn keyboard_handler_addr() -> u64 {
    keyboard_handler_asm as *const () as usize as u64
}
