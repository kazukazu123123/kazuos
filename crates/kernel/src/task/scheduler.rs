use crate::util::{IrqGuard, SyncUnsafeCell};

#[repr(C, align(16))]
struct UserReturnStack([u8; 16384]);

static mut USER_RETURN_STACKS: [UserReturnStack; MAX_CPUS] =
    [const { UserReturnStack([0; 16384]) }; MAX_CPUS];

unsafe fn user_return_stack_top() -> u64 {
    let idx = cpu_idx();
    unsafe { core::ptr::addr_of!(USER_RETURN_STACKS[idx]) as u64 + 16384 }
}

use crate::smp::{MAX_CPUS, current_cpu_index};

static CURRENT_USER_PID: SyncUnsafeCell<[Option<u64>; MAX_CPUS]> = SyncUnsafeCell::new([None; MAX_CPUS]);
static CURRENT_USER_TID: SyncUnsafeCell<[Option<u64>; MAX_CPUS]> = SyncUnsafeCell::new([None; MAX_CPUS]);
static NEXT_USER_PID: SyncUnsafeCell<[Option<u64>; MAX_CPUS]> = SyncUnsafeCell::new([None; MAX_CPUS]);
static IS_IDLE: SyncUnsafeCell<[core::sync::atomic::AtomicBool; MAX_CPUS]> =
    SyncUnsafeCell::new([const { core::sync::atomic::AtomicBool::new(false) }; MAX_CPUS]);

type ExitHandler = Option<fn()>;
static EXIT_HANDLER: SyncUnsafeCell<ExitHandler> = SyncUnsafeCell::new(None);

#[repr(C)]
pub struct InterruptFrame {
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

fn cpu_idx() -> usize {
    current_cpu_index()
}

pub fn current_user_pid() -> Option<u64> {
    unsafe { (*CURRENT_USER_PID.0.get())[cpu_idx()] }
}

pub fn current_user_tid() -> Option<u64> {
    unsafe { (*CURRENT_USER_TID.0.get())[cpu_idx()] }
}

pub fn set_current_user_pid(pid: Option<u64>) {
    let _irq = IrqGuard::new();
    unsafe {
        let idx = cpu_idx();
        (*CURRENT_USER_PID.0.get())[idx] = pid;
        (*CURRENT_USER_TID.0.get())[idx] = pid.and_then(|p| crate::process::main_tid(p));
    }
}

pub fn set_current_user_tid(tid: Option<u64>) {
    let _irq = IrqGuard::new();
    unsafe {
        let idx = cpu_idx();
        (*CURRENT_USER_TID.0.get())[idx] = tid;
        (*CURRENT_USER_PID.0.get())[idx] = tid.and_then(|t| crate::task::thread::thread_pid(t));
    }
}

pub fn set_idle(idle: bool) {
    unsafe {
        (*IS_IDLE.0.get())[cpu_idx()].store(idle, core::sync::atomic::Ordering::Release);
    }
}

pub fn is_idle() -> bool {
    unsafe { (*IS_IDLE.0.get())[cpu_idx()].load(core::sync::atomic::Ordering::Acquire) }
}

pub fn clear_current_user(pid: u64) {
    let _irq = IrqGuard::new();
    unsafe {
        let idx = cpu_idx();
        if (*CURRENT_USER_PID.0.get())[idx] == Some(pid) {
            (*CURRENT_USER_PID.0.get())[idx] = None;
            (*CURRENT_USER_TID.0.get())[idx] = None;
            (*NEXT_USER_PID.0.get())[idx] = None;
        }
    }
}

pub fn schedule_next(current_tid: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| {
        let cpu = cpu_idx();
        unsafe {
            let threads = &*crate::task::thread::THREADS.0.get();
            if let Some(next) = threads
                .iter()
                .filter(|t| {
                    t.assigned_cpu == cpu
                        && matches!(t.state, crate::task::thread::ThreadState::Ready)
                        && t.tid > current_tid
                })
                .map(|t| t.tid)
                .min()
            {
                return next;
            }
            if let Some(next) = threads
                .iter()
                .filter(|t| {
                    t.assigned_cpu == cpu
                        && matches!(t.state, crate::task::thread::ThreadState::Ready)
                        && t.tid > 0
                })
                .map(|t| t.tid)
                .min()
            {
                return next;
            }
            0
        }
    })
}

pub fn save_user_context(tid: u64, frame_ptr: u64) {
    unsafe {
        let frame = &*(frame_ptr as *const InterruptFrame);
        if let Some(mut ctx) = crate::task::thread::user_context(tid) {
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
            crate::task::thread::set_user_context(tid, ctx);
        }
    }
}

pub fn save_kernel_context(tid: u64, saved_rsp: u64) {
    crate::task::thread::set_kernel_rsp(tid, saved_rsp);
}

pub unsafe fn setup_user_frame_on_temp_stack(ctx: crate::process::UserContext) -> u64 {
    let stack_top = unsafe { user_return_stack_top() };
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

pub unsafe fn setup_user_frame_for_timer_on_temp_stack(ctx: crate::process::UserContext) -> u64 {
    let stack_top = unsafe { user_return_stack_top() };
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
        // Wait until there is work for this CPU. The loop runs with interrupts
        // enabled so the timer can wake us; once a thread is selected we disable
        // them until the iretq to userspace/kernel-thread installs the new
        // RFLAGS. A timer interrupt between set_current_user_tid() and the
        // iretq would mark the thread kernel-preempted and skip setting the
        // per-CPU kernel return stack, which makes the next blocking syscall
        // load RSP=0 and double fault.
        let next_tid = loop {
            let t = schedule_next(0);
            if t != 0 {
                if let Some(ac) = crate::task::thread::assigned_cpu(t) {
                    if ac != cpu_idx() {
                        crate::serial_println!("SCHED BUG: tid={} assigned_cpu={} but cpu={}", t, ac, cpu_idx());
                        loop { crate::util::hlt(); }
                    }
                }
                break t;
            }
            set_idle(true);
            crate::process::clear_current_pid();
            set_current_user_tid(None);
            crate::util::hlt();
        };

        let _irq = crate::util::IrqGuard::new();
        core::mem::forget(_irq);

        let next_pid = crate::task::thread::thread_pid(next_tid).unwrap_or(0);

        if let Some(blocking_rsp) = crate::task::thread::take_blocking_rsp(next_tid) {
            let retval = crate::task::thread::take_blocking_retval(next_tid);
            if let Some(ctx) = crate::task::thread::user_context(next_tid) {
                if ctx.cr3 != 0 {
                    crate::vmm::switch_cr3(ctx.cr3);
                }
            }
            set_current_user_tid(Some(next_tid));
            if let Some(top) = crate::task::thread::kernel_stack_top(next_tid) {
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

        if crate::task::thread::is_kernel_preempted(next_tid) {
            if let Some(kernel_rsp) = crate::task::thread::kernel_rsp(next_tid) {
                if let Some(ctx) = crate::task::thread::user_context(next_tid) {
                    if ctx.cr3 != 0 {
                        crate::vmm::switch_cr3(ctx.cr3);
                    }
                    set_current_user_tid(Some(next_tid));
                } else {
                    set_current_user_tid(None);
                }
                if let Some(top) = crate::task::thread::kernel_stack_top(next_tid) {
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

        if let Some(ctx) = crate::task::thread::user_context(next_tid) {
            if ctx.cr3 != 0 {
                crate::vmm::switch_cr3(ctx.cr3);
            }
            set_current_user_tid(Some(next_tid));
            if let Some(top) = crate::task::thread::kernel_stack_top(next_tid) {
                crate::gdt::set_kernel_stack_top(top);
            }
            crate::process::set_running(next_pid);
            let rsp: u64;
            core::arch::asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack));
            crate::user::set_kernel_return_stack(rsp);
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

        if let Some(kernel_rsp) = crate::task::thread::kernel_rsp(next_tid) {
            set_current_user_tid(None);
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
