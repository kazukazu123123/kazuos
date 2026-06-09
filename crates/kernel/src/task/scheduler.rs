use crate::util::SyncUnsafeCell;

#[repr(C, align(16))]
struct UserReturnStack([u8; 16384]);

static mut USER_RETURN_STACK: UserReturnStack = UserReturnStack([0; 16384]);

static CURRENT_USER_PID: SyncUnsafeCell<Option<u64>> = SyncUnsafeCell::new(None);
static NEXT_USER_PID: SyncUnsafeCell<Option<u64>> = SyncUnsafeCell::new(None);
static IS_IDLE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

type ExitHandler = Option<fn()>;
static EXIT_HANDLER: SyncUnsafeCell<ExitHandler> = SyncUnsafeCell::new(None);

#[repr(C)]
pub struct InterruptFrame {
    // Matches timer_handler_asm push order (last pushed = lowest address):
    // push rax, rcx, rdx, rsi, rdi, r8, r9, r10, r11, rbx, rbp, r12, r13, r14, r15
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub rbp: u64,
    pub rbx: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rax: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

pub fn on_user_exit(handler: fn()) {
    unsafe {
        *EXIT_HANDLER.0.get() = Some(handler);
    }
}

pub fn run_exit_handler() {
    unsafe {
        if let Some(handler) = *EXIT_HANDLER.0.get() {
            handler();
        }
    }
}

pub fn current_user_pid() -> Option<u64> {
    unsafe { *CURRENT_USER_PID.0.get() }
}

pub fn set_current_user_pid(pid: Option<u64>) {
    unsafe {
        *CURRENT_USER_PID.0.get() = pid;
    }
}

pub fn set_idle(idle: bool) {
    IS_IDLE.store(idle, core::sync::atomic::Ordering::Release);
}

pub fn is_idle() -> bool {
    IS_IDLE.load(core::sync::atomic::Ordering::Acquire)
}

pub fn clear_current_user(pid: u64) {
    unsafe {
        if *CURRENT_USER_PID.0.get() == Some(pid) {
            *CURRENT_USER_PID.0.get() = None;
            *NEXT_USER_PID.0.get() = None;
        }
    }
}

pub fn schedule_next(current: u64) -> u64 {
    unsafe {
        let processes = &*crate::task::process::PROCESSES.0.get();
        if let Some(next) = processes
            .iter()
            .filter(|p| {
                matches!(p.state, crate::task::process::ProcessState::Ready) && p.pid > current
            })
            .map(|p| p.pid)
            .min()
        {
            return next;
        }
        if let Some(next) = processes
            .iter()
            .filter(|p| matches!(p.state, crate::task::process::ProcessState::Ready) && p.pid > 0)
            .map(|p| p.pid)
            .min()
        {
            return next;
        }
        0
    }
}

pub fn save_user_context(pid: u64, frame_ptr: u64) {
    unsafe {
        let frame = &*(frame_ptr as *const InterruptFrame);
        if let Some(mut ctx) = crate::process::user_context(pid) {
            ctx.rax = frame.rax;
            ctx.rcx = frame.rcx;
            ctx.rdx = frame.rdx;
            ctx.rsi = frame.rsi;
            ctx.rdi = frame.rdi;
            ctx.r8 = frame.r8;
            ctx.r9 = frame.r9;
            ctx.r10 = frame.r10;
            ctx.r11 = frame.r11;
            ctx.rbx = frame.rbx;
            ctx.rbp = frame.rbp;
            ctx.r12 = frame.r12;
            ctx.r13 = frame.r13;
            ctx.r14 = frame.r14;
            ctx.r15 = frame.r15;
            ctx.rip = frame.rip;
            ctx.rsp = frame.rsp;
            ctx.rflags = frame.rflags;
            ctx.kernel_rsp = frame_ptr;
            crate::process::set_user_context(pid, ctx);
        }
    }
}

pub fn save_kernel_context(pid: u64, saved_rsp: u64) {
    crate::process::set_kernel_rsp(pid, saved_rsp);
}

pub unsafe fn setup_user_frame_on_temp_stack(ctx: crate::process::UserContext) -> u64 {
    let stack_top = (core::ptr::addr_of!(USER_RETURN_STACK) as u64) + 16384;
    let frame_bottom = stack_top - 160;
    unsafe {
        let p = frame_bottom as *mut u64;
        p.add(0).write(ctx.rax);
        p.add(1).write(ctx.rcx);
        p.add(2).write(ctx.rdx);
        p.add(3).write(ctx.rsi);
        p.add(4).write(ctx.rdi);
        p.add(5).write(ctx.r8);
        p.add(6).write(ctx.r9);
        p.add(7).write(ctx.r10);
        p.add(8).write(ctx.r11);
        p.add(9).write(ctx.rbx);
        p.add(10).write(ctx.rbp);
        p.add(11).write(ctx.r12);
        p.add(12).write(ctx.r13);
        p.add(13).write(ctx.r14);
        p.add(14).write(ctx.r15);
        p.add(15).write(ctx.rip);
        p.add(16).write(crate::gdt::USER_CODE as u64);
        p.add(17).write(ctx.rflags | 0x200);
        p.add(18).write(ctx.rsp);
        p.add(19).write(crate::gdt::USER_DATA as u64);
    }
    frame_bottom
}

// Layout matches timer_handler_asm pop order:
// pop r15, r14, r13, r12, rbp, rbx, r11, r10, r9, r8, rdi, rsi, rdx, rcx, rax, iretq
pub unsafe fn setup_user_frame_for_timer_on_temp_stack(ctx: crate::process::UserContext) -> u64 {
    let stack_top = (core::ptr::addr_of!(USER_RETURN_STACK) as u64) + 16384;
    let frame_bottom = stack_top - 160;
    unsafe {
        let p = frame_bottom as *mut u64;
        p.add(0).write(ctx.r15);
        p.add(1).write(ctx.r14);
        p.add(2).write(ctx.r13);
        p.add(3).write(ctx.r12);
        p.add(4).write(ctx.rbp);
        p.add(5).write(ctx.rbx);
        p.add(6).write(ctx.r11);
        p.add(7).write(ctx.r10);
        p.add(8).write(ctx.r9);
        p.add(9).write(ctx.r8);
        p.add(10).write(ctx.rdi);
        p.add(11).write(ctx.rsi);
        p.add(12).write(ctx.rdx);
        p.add(13).write(ctx.rcx);
        p.add(14).write(ctx.rax);
        p.add(15).write(ctx.rip);
        p.add(16).write(crate::gdt::USER_CODE as u64);
        p.add(17).write(ctx.rflags | 0x200);
        p.add(18).write(ctx.rsp);
        p.add(19).write(crate::gdt::USER_DATA as u64);
    }
    frame_bottom
}

pub unsafe fn restore_user_frame(frame_ptr: u64, ctx: crate::process::UserContext) {
    unsafe {
        let frame = &mut *(frame_ptr as *mut InterruptFrame);
        frame.rax = ctx.rax;
        frame.rcx = ctx.rcx;
        frame.rdx = ctx.rdx;
        frame.rsi = ctx.rsi;
        frame.rdi = ctx.rdi;
        frame.r8 = ctx.r8;
        frame.r9 = ctx.r9;
        frame.r10 = ctx.r10;
        frame.r11 = ctx.r11;
        frame.rbx = ctx.rbx;
        frame.rbp = ctx.rbp;
        frame.r12 = ctx.r12;
        frame.r13 = ctx.r13;
        frame.r14 = ctx.r14;
        frame.r15 = ctx.r15;
        frame.rip = ctx.rip;
        frame.rsp = ctx.rsp;
        frame.rflags = ctx.rflags;
        frame.cs = crate::gdt::USER_CODE as u64;
        frame.ss = crate::gdt::USER_DATA as u64;
    }
}

#[unsafe(no_mangle)]
extern "C" fn idle_loop() -> ! {
    loop {
        crate::util::hlt();
    }
}

pub fn enter_next_process() -> ! {
    unsafe {
        let next_pid = schedule_next(0);
        if next_pid == 0 {
            set_idle(true);
            crate::process::clear_current_pid();
            set_current_user_pid(None);
            loop {
                crate::util::hlt();
            }
        }

        // If next_pid was blocked in a syscall (blocking_rsp set), resume the int-0x80 frame.
        // Frame layout: [r15][r14][r13][r12][r11][r10][r9][r8][rdi][rsi][rdx][rcx][rbx][rbp] | iretq_frame
        if let Some(blocking_rsp) = crate::process::take_blocking_rsp(next_pid) {
            let retval = crate::process::take_blocking_retval(next_pid);
            if let Some(ctx) = crate::process::user_context(next_pid) {
                if ctx.cr3 != 0 {
                    crate::vmm::switch_cr3(ctx.cr3);
                }
            }
            set_current_user_pid(Some(next_pid));
            if let Some(top) = crate::process::kernel_stack_top(next_pid) {
                crate::gdt::set_kernel_stack_top(top);
            }
            crate::process::set_running(next_pid);
            core::arch::asm!(
                "mov rsp, {rsp}",
                "mov rax, {retval}",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rbx",
                "pop rbp",
                "iretq",
                rsp    = in(reg) blocking_rsp,
                retval = in(reg) retval,
                options(noreturn),
            );
        }

        // If next_pid was preempted mid-syscall (kernel mode), resume via kernel stack.
        if crate::process::is_kernel_preempted(next_pid) {
            if let Some(kernel_rsp) = crate::process::kernel_rsp(next_pid) {
                if let Some(ctx) = crate::process::user_context(next_pid) {
                    if ctx.cr3 != 0 {
                        crate::vmm::switch_cr3(ctx.cr3);
                    }
                    set_current_user_pid(Some(next_pid));
                } else {
                    set_current_user_pid(None);
                }
                if let Some(top) = crate::process::kernel_stack_top(next_pid) {
                    crate::gdt::set_kernel_stack_top(top);
                }
                crate::process::set_running(next_pid);
                core::arch::asm!(
                    "mov rsp, {rsp}",
                    "pop r15",
                    "pop r14",
                    "pop r13",
                    "pop r12",
                    "pop rbp",
                    "pop rbx",
                    "pop r11",
                    "pop r10",
                    "pop r9",
                    "pop r8",
                    "pop rdi",
                    "pop rsi",
                    "pop rdx",
                    "pop rcx",
                    "pop rax",
                    "iretq",
                    rsp = in(reg) kernel_rsp,
                    options(noreturn),
                );
            }
        }

        if let Some(ctx) = crate::process::user_context(next_pid) {
            if ctx.cr3 != 0 {
                crate::vmm::switch_cr3(ctx.cr3);
            }
            set_current_user_pid(Some(next_pid));
            if let Some(top) = crate::process::kernel_stack_top(next_pid) {
                crate::gdt::set_kernel_stack_top(top);
            }
            crate::process::set_running(next_pid);
            let rsp: u64;
            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack));
            crate::user::KERNEL_RETURN_STACK = rsp;
            // Use setup_user_frame_on_temp_stack so ALL registers (including rax) are
            // restored correctly — essential when resuming a process that was woken from
            // a blocking syscall (blocking_rsp was cleared before reaching this path).
            let temp_rsp = setup_user_frame_on_temp_stack(ctx);
            core::arch::asm!(
                "mov ax, dx",
                "mov ds, ax",
                "mov es, ax",
                "mov rsp, r11",
                "pop rax",
                "pop rcx",
                "pop rdx",
                "pop rsi",
                "pop rdi",
                "pop r8",
                "pop r9",
                "pop r10",
                "pop r11",
                "pop rbx",
                "pop rbp",
                "pop r12",
                "pop r13",
                "pop r14",
                "pop r15",
                "iretq",
                in("r11") temp_rsp,
                in("rdx") crate::gdt::USER_DATA as u64,
                options(noreturn),
            );
        }

        if let Some(kernel_rsp) = crate::process::kernel_rsp(next_pid) {
            set_current_user_pid(None);
            crate::process::set_running(next_pid);
            core::arch::asm!(
                "mov rsp, {rsp}",
                "pop r15",
                "pop r14",
                "pop r13",
                "pop r12",
                "pop rbp",
                "pop rbx",
                "pop r11",
                "pop r10",
                "pop r9",
                "pop r8",
                "pop rdi",
                "pop rsi",
                "pop rdx",
                "pop rcx",
                "pop rax",
                "iretq",
                rsp = in(reg) kernel_rsp,
                options(noreturn),
            );
        }

        loop {
            crate::util::hlt();
        }
    }
}
