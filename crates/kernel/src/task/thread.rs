use alloc::alloc::{Layout, alloc_zeroed};
use alloc::vec::Vec;

use crate::task::process::{PrivilegeLevel, WaitTarget};
use crate::util::SyncUnsafeCell;

#[derive(Clone, Copy)]
#[repr(u64)]
pub enum ThreadState {
    Empty = 0,
    Ready = 1,
    Running = 2,
    Sleeping = 3,
    Exited = 4,
}

impl ThreadState {
    pub const fn name(self) -> &'static str {
        match self {
            ThreadState::Empty => "empty",
            ThreadState::Ready => "ready",
            ThreadState::Running => "running",
            ThreadState::Sleeping => "sleeping",
            ThreadState::Exited => "exited",
        }
    }
}

#[derive(Clone, Copy)]
pub struct UserContext {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub cr3: u64,
    pub kernel_rsp: u64,
    pub user_stack_top: u64,
}

pub(crate) const EMPTY_USER_CONTEXT: UserContext = UserContext {
    rax: 0,
    rbx: 0,
    rcx: 0,
    rdx: 0,
    rsi: 0,
    rdi: 0,
    rbp: 0,
    r8: 0,
    r9: 0,
    r10: 0,
    r11: 0,
    r12: 0,
    r13: 0,
    r14: 0,
    r15: 0,
    rip: 0,
    rsp: 0,
    rflags: 0x202,
    cr3: 0,
    kernel_rsp: 0,
    user_stack_top: 0,
};

#[derive(Clone, Copy)]
pub(crate) struct Thread {
    pub(crate) tid: u64,
    pub(crate) pid: u64,
    pub(crate) state: ThreadState,
    pub(crate) cpu_ticks: u64,
    pub(crate) user_context: UserContext,
    pub(crate) kernel_rsp: u64,
    pub(crate) kernel_stack_base: u64,
    pub(crate) kernel_preempted: bool,
    pub(crate) blocking_rsp: u64,
    pub(crate) blocking_retval: u64,
    pub(crate) wait_target: WaitTarget,
    pub(crate) privilege: PrivilegeLevel,
    pub(crate) assigned_cpu: usize,
}

pub const KERNEL_STACK_SIZE: usize = 65536;

pub(crate) static THREADS: SyncUnsafeCell<Vec<Thread>> = SyncUnsafeCell::new(Vec::new());
static NEXT_TID: SyncUnsafeCell<u64> = SyncUnsafeCell::new(1);
static INITIALIZED: SyncUnsafeCell<bool> = SyncUnsafeCell::new(false);
static NEXT_ASSIGN_CPU: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

static THREADS_LOCK: crate::util::ReentrantSpinLock = crate::util::ReentrantSpinLock::new();

pub fn with_threads_lock<F: FnOnce() -> R, R>(f: F) -> R {
    let _guard = crate::util::ReentrantIrqGuard::new(&THREADS_LOCK);
    f()
}

pub fn init() {
    unsafe {
        if *INITIALIZED.0.get() {
            return;
        }
        let threads = &mut *THREADS.0.get();
        threads.clear();
        *NEXT_TID.0.get() = 1;
        *INITIALIZED.0.get() = true;
    }
}

unsafe fn alloc_kernel_stack() -> Option<u64> {
    let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 4096).ok()?;
    let ptr = unsafe { alloc_zeroed(layout) };
    if ptr.is_null() {
        None
    } else {
        Some(ptr as u64)
    }
}

unsafe fn setup_kernel_task_stack(stack_base: u64, entry: u64, arg: u64) -> u64 {
    let stack_top = stack_base + KERNEL_STACK_SIZE as u64;
    let kernel_rsp = stack_top - 160;
    let p = kernel_rsp as *mut u64;
    unsafe {
        p.add(0).write(0);
        p.add(1).write(0);
        p.add(2).write(0);
        p.add(3).write(0);
        p.add(4).write(0);
        p.add(5).write(0);
        p.add(6).write(0);
        p.add(7).write(0);
        p.add(8).write(0);
        p.add(9).write(0);
        p.add(10).write(arg);
        p.add(11).write(0);
        p.add(12).write(0);
        p.add(13).write(0);
        p.add(14).write(0);
        p.add(15).write(entry);
        p.add(16).write(0x8);
        p.add(17).write(0x202);
        p.add(18).write(stack_top);
        p.add(19).write(0x10);
    }
    kernel_rsp
}

pub fn create_kernel_thread(
    pid: u64,
    entry: u64,
    arg: u64,
    privilege: PrivilegeLevel,
) -> u64 {
    with_threads_lock(|| unsafe {
        init();
        let stack_base = match alloc_kernel_stack() {
            Some(base) => base,
            None => return 0,
        };
        let kernel_rsp = setup_kernel_task_stack(stack_base, entry, arg);
        let tid = *NEXT_TID.0.get();
        *NEXT_TID.0.get() = tid + 1;
        let cpu_count = crate::smp::cpu_count().max(1);
        let assigned_cpu = NEXT_ASSIGN_CPU.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % cpu_count;
        crate::vserial_println!("THREAD: create tid={} pid={} assigned_cpu={}", tid, pid, assigned_cpu);
        let threads = &mut *THREADS.0.get();
        threads.push(Thread {
            tid,
            pid,
            state: ThreadState::Sleeping,
            cpu_ticks: 0,
            user_context: EMPTY_USER_CONTEXT,
            kernel_rsp,
            kernel_stack_base: stack_base,
            kernel_preempted: false,
            blocking_rsp: 0,
            blocking_retval: 0,
            wait_target: WaitTarget::None,
            privilege,
            assigned_cpu,
        });
        tid
    })
}

pub fn create_user_thread(
    pid: u64,
    entry: u64,
    user_stack_top: u64,
    cr3: u64,
    privilege: PrivilegeLevel,
    argc: u64,
    argv: u64,
) -> u64 {
    with_threads_lock(|| {
        let tid = create_kernel_thread(pid, entry, 0, privilege);
        if tid == 0 {
            return 0;
        }
        set_user_context(
            tid,
            UserContext {
                rip: entry,
                rsp: user_stack_top,
                cr3,
                user_stack_top,
                rdi: argc,
                rsi: argv,
                ..EMPTY_USER_CONTEXT
            },
        );
        tid
    })
}

pub fn set_user_context(tid: u64, context: UserContext) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            thread.user_context = context;
        }
    })
}

pub fn user_context(tid: u64) -> Option<UserContext> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.user_context)
    })
}

pub fn user_cr3(tid: u64) -> Option<u64> {
    with_threads_lock(|| user_context(tid).map(|ctx| ctx.cr3))
}

pub fn kernel_rsp(tid: u64) -> Option<u64> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.kernel_rsp)
    })
}

pub fn set_kernel_rsp(tid: u64, rsp: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            thread.kernel_rsp = rsp;
        }
    })
}

pub fn kernel_stack_base(tid: u64) -> Option<u64> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.kernel_stack_base)
    })
}

pub fn kernel_stack_top(tid: u64) -> Option<u64> {
    with_threads_lock(|| kernel_stack_base(tid).map(|base| base + KERNEL_STACK_SIZE as u64))
}

pub fn set_running(tid: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid)
            && !matches!(thread.state, ThreadState::Empty | ThreadState::Exited)
        {
            thread.state = ThreadState::Running;
        }
    })
}

pub fn set_ready(tid: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid)
            && matches!(
                thread.state,
                ThreadState::Ready | ThreadState::Running | ThreadState::Sleeping
            )
        {
            thread.state = ThreadState::Ready;
        }
    })
}

pub fn set_sleeping(tid: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid)
            && !matches!(thread.state, ThreadState::Empty | ThreadState::Exited)
        {
            thread.state = ThreadState::Sleeping;
        }
    })
}

pub fn set_state(tid: u64, state: ThreadState) {
    with_threads_lock(|| match state {
        ThreadState::Ready => set_ready(tid),
        ThreadState::Running => set_running(tid),
        ThreadState::Sleeping => set_sleeping(tid),
        _ => {}
    })
}

pub fn state(tid: u64) -> Option<ThreadState> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.state)
    })
}

pub fn set_kernel_preempted(tid: u64, val: bool) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            thread.kernel_preempted = val;
        }
    })
}

pub fn is_kernel_preempted(tid: u64) -> bool {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.kernel_preempted)
            .unwrap_or(false)
    })
}

pub fn set_blocking_rsp(tid: u64, rsp: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            thread.blocking_rsp = rsp;
        }
    })
}

pub fn take_blocking_rsp(tid: u64) -> Option<u64> {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            if thread.blocking_rsp != 0 {
                let rsp = thread.blocking_rsp;
                thread.blocking_rsp = 0;
                return Some(rsp);
            }
        }
        None
    })
}

pub fn take_blocking_retval(tid: u64) -> u64 {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            let v = thread.blocking_retval;
            thread.blocking_retval = 0;
            return v;
        }
        0
    })
}

pub fn set_wait_target(tid: u64, target: WaitTarget) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            thread.wait_target = target;
        }
    })
}

pub fn wait_target(tid: u64) -> Option<WaitTarget> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.wait_target)
    })
}

pub fn add_cpu_ticks(tid: u64, ticks: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            thread.cpu_ticks = thread.cpu_ticks.saturating_add(ticks);
        }
    })
}

pub fn cpu_ticks(tid: u64) -> Option<u64> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.cpu_ticks)
    })
}

pub fn assigned_cpu(tid: u64) -> Option<usize> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.assigned_cpu)
    })
}

pub fn set_assigned_cpu(tid: u64, cpu: usize) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(thread) = threads.iter_mut().find(|t| t.tid == tid) {
            thread.assigned_cpu = cpu;
        }
    })
}

pub fn remove_thread(tid: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        threads.retain(|t| t.tid != tid);
    })
}

pub fn first_tid() -> Option<u64> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| !matches!(t.state, ThreadState::Empty))
            .map(|t| t.tid)
    })
}

pub fn next_tid_after(tid: u64) -> Option<u64> {
    with_threads_lock(|| {
        let filter_tid = if tid == u64::MAX { u64::MAX } else { tid };
        unsafe {
            (*THREADS.0.get())
                .iter()
                .filter(|t| {
                    !matches!(t.state, ThreadState::Empty)
                        && if filter_tid == u64::MAX {
                            true
                        } else {
                            t.tid > filter_tid
                        }
                })
                .map(|t| t.tid)
                .min()
        }
    })
}

pub fn active_tid_after(tid: u64) -> Option<u64> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .filter(|t| {
                matches!(t.state, ThreadState::Ready | ThreadState::Running) && t.tid > tid
            })
            .map(|t| t.tid)
            .min()
    })
}

pub fn count() -> u64 {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .filter(|t| !matches!(t.state, ThreadState::Empty))
            .count() as u64
    })
}

pub fn active_count() -> u64 {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .filter(|t| {
                matches!(
                    t.state,
                    ThreadState::Ready | ThreadState::Running | ThreadState::Sleeping
                )
            })
            .count() as u64
    })
}

pub fn thread_pid(tid: u64) -> Option<u64> {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid && !matches!(t.state, ThreadState::Empty))
            .map(|t| t.pid)
    })
}

pub fn thread_exists(tid: u64) -> bool {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .any(|t| t.tid == tid && !matches!(t.state, ThreadState::Empty))
    })
}

unsafe fn restore_ctx_from_blocking_frame(t: &mut Thread, retval: u64) {
    let rsp = t.blocking_rsp;
    if rsp == 0 {
        return;
    }
    unsafe {
        let f = rsp as *const u64;
        let user_rip = *f.add(14);
        let user_rflags = *f.add(16);
        let user_rsp = *f.add(17);
        let ctx = &mut t.user_context;
        ctx.rax = retval;
        ctx.rbx = *f.add(12);
        ctx.rcx = *f.add(11);
        ctx.rdx = *f.add(10);
        ctx.rsi = *f.add(9);
        ctx.rdi = *f.add(8);
        ctx.rbp = *f.add(13);
        ctx.r8 = *f.add(7);
        ctx.r9 = *f.add(6);
        ctx.r10 = *f.add(5);
        ctx.r11 = *f.add(4);
        ctx.r12 = *f.add(3);
        ctx.r13 = *f.add(2);
        ctx.r14 = *f.add(1);
        ctx.r15 = *f.add(0);
        ctx.rip = user_rip;
        ctx.rsp = user_rsp;
        ctx.rflags = user_rflags;
    }
    t.blocking_rsp = 0;
    t.kernel_preempted = false;
}

pub fn apply_blocking_return_if_pending(tid: u64) -> bool {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(t) = threads.iter_mut().find(|t| t.tid == tid) {
            if t.blocking_rsp != 0 {
                let retval = t.blocking_retval;
                t.blocking_retval = 0;
                restore_ctx_from_blocking_frame(t, retval);
                return true;
            }
        }
        false
    })
}

pub fn wakeup_key_waiters(key: u8) -> usize {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if matches!(t.state, ThreadState::Sleeping)
                && matches!(t.wait_target, WaitTarget::Keyboard)
            {
                restore_ctx_from_blocking_frame(t, key as u64);
                t.state = ThreadState::Ready;
                t.wait_target = WaitTarget::None;
                return 1;
            }
        }
        0
    })
}

/// Like `wakeup_key_waiters` but only wakes a keyboard waiter belonging to `pid`.
/// Used for keyboard focus: while a graphical program owns the framebuffer, keys
/// go only to it, never to other readers (e.g. the shell at its prompt).
pub fn wakeup_key_waiter_for_pid(pid: u64, key: u8) -> usize {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if t.pid == pid
                && matches!(t.state, ThreadState::Sleeping)
                && matches!(t.wait_target, WaitTarget::Keyboard)
            {
                restore_ctx_from_blocking_frame(t, key as u64);
                t.state = ThreadState::Ready;
                t.wait_target = WaitTarget::None;
                return 1;
            }
        }
        0
    })
}

pub fn wakeup_pid_waiters(exited_pid: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if matches!(t.state, ThreadState::Sleeping)
                && matches!(t.wait_target, WaitTarget::Pid(target_pid) if target_pid == exited_pid)
            {
                restore_ctx_from_blocking_frame(t, 1);
                t.state = ThreadState::Ready;
                t.wait_target = WaitTarget::None;
            }
        }
    })
}

pub fn wakeup_ipc_waiter(tid: u64, retval: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(t) = threads.iter_mut().find(|t| t.tid == tid) {
            if matches!(t.state, ThreadState::Sleeping)
                && matches!(t.wait_target, WaitTarget::Ipc(_))
            {
                restore_ctx_from_blocking_frame(t, retval);
                t.state = ThreadState::Ready;
                t.wait_target = WaitTarget::None;
            }
        }
    })
}

pub fn wakeup_irq_waiter(irq: u8) -> bool {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if matches!(t.state, ThreadState::Sleeping)
                && matches!(t.wait_target, WaitTarget::Irq(n) if n == irq)
            {
                restore_ctx_from_blocking_frame(t, 0);
                t.state = ThreadState::Ready;
                t.wait_target = WaitTarget::None;
                return true;
            }
        }
        false
    })
}

pub fn notify_pipe_readers(pipe_id: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if matches!(t.state, ThreadState::Sleeping) {
                if let WaitTarget::PipeRead { pipe_id: id, buf_ptr, buf_len } = t.wait_target {
                    if id == pipe_id {
                        // The blocked reader's buffer lives in *its* address space, but we
                        // run in the writer's. Switch to the reader's CR3 to copy into the
                        // right pages, then restore. (Kernel mappings are global, so the
                        // switch is safe inside this IRQ-disabled, locked section.)
                        let reader_cr3 = t.user_context.cr3;
                        let cur_cr3 = crate::vmm::active_cr3();
                        let switch = reader_cr3 != 0 && reader_cr3 != cur_cr3;
                        if switch { crate::vmm::switch_cr3(reader_cr3); }
                        let n = crate::pipe::read_raw(pipe_id, buf_ptr, buf_len as usize);
                        if switch { crate::vmm::switch_cr3(cur_cr3); }
                        restore_ctx_from_blocking_frame(t, n as u64);
                        t.state = ThreadState::Ready;
                        t.wait_target = WaitTarget::None;
                    }
                }
            }
        }
    })
}

pub fn wakeup_audio_waiters() {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if matches!(t.state, ThreadState::Sleeping)
                && matches!(t.wait_target, WaitTarget::Audio)
            {
                restore_ctx_from_blocking_frame(t, 0);
                t.state = ThreadState::Ready;
                t.wait_target = WaitTarget::None;
            }
        }
    })
}

pub fn wake_timer_sleepers(now_tsc: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if matches!(t.state, ThreadState::Sleeping) {
                if let WaitTarget::Timer(deadline) = t.wait_target {
                    if now_tsc >= deadline {
                        restore_ctx_from_blocking_frame(t, 0);
                        t.state = ThreadState::Ready;
                        t.wait_target = WaitTarget::None;
                    }
                }
            }
        }
    })
}

pub fn wake_tick_sleepers() {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        for t in threads.iter_mut() {
            if matches!(t.state, ThreadState::Sleeping)
                && matches!(t.wait_target, WaitTarget::Tick)
            {
                restore_ctx_from_blocking_frame(t, 0);
                t.state = ThreadState::Ready;
                t.wait_target = WaitTarget::None;
            }
        }
    })
}

pub fn set_thread_pid(tid: u64, pid: u64) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(t) = threads.iter_mut().find(|t| t.tid == tid) {
            t.pid = pid;
        }
    })
}

pub fn set_privilege(tid: u64, privilege: PrivilegeLevel) {
    with_threads_lock(|| unsafe {
        let threads = &mut *THREADS.0.get();
        if let Some(t) = threads.iter_mut().find(|t| t.tid == tid) {
            t.privilege = privilege;
        }
    })
}

pub fn privilege_level(tid: u64) -> PrivilegeLevel {
    with_threads_lock(|| unsafe {
        (*THREADS.0.get())
            .iter()
            .find(|t| t.tid == tid)
            .map(|t| t.privilege)
            .unwrap_or(PrivilegeLevel::User)
    })
}
