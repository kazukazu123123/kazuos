use alloc::vec::Vec;

use crate::task::thread;
use crate::util::SyncUnsafeCell;

const IMAGE_NAME_LEN: usize = 32;

#[derive(Clone, Copy)]
#[repr(u64)]
pub enum ProcessState {
    Empty = 0,
    Ready = 1,
    Running = 2,
    Sleeping = 3,
    Exited = 4,
}

impl ProcessState {
    pub const fn name(self) -> &'static str {
        match self {
            ProcessState::Empty => "empty",
            ProcessState::Ready => "ready",
            ProcessState::Running => "running",
            ProcessState::Sleeping => "sleeping",
            ProcessState::Exited => "exited",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrivilegeLevel {
    System = 0,
    Driver = 1,
    User = 2,
}

#[derive(Clone, Copy, PartialEq)]
pub enum WaitTarget {
    None,
    Keyboard,
    Pid(u64),
    Timer(u64),
    Tick,
    Ipc(u64),
    Irq(u8),
    Audio,
    PipeRead { pipe_id: u64, buf_ptr: u64, buf_len: u64 },
}

pub use crate::task::thread::UserContext;

pub(crate) struct Process {
    pub(crate) pid: u64,
    pub(crate) state: ProcessState,
    image_name: [u8; IMAGE_NAME_LEN],
    start_tsc: u64,
    entry: u64,
    stack_top: u64,
    step: u64,
    memory_bytes: u64,
    background: bool,
    pub(crate) sigint_catch: bool,
    pub(crate) sigint_pending: bool,
    // Set when a remote agent requested this process die while its thread was
    // still Running on some CPU. We cannot free its address space out from under
    // a running thread (SMP use-after-free), so the thread self-exits the next
    // time it enters the kernel. See kill_pid / syscall_dispatch.
    pub(crate) kill_pending: bool,
    // pid of the spawning process (0 = kernel). When a parent exits, its children
    // are killed so they don't orphan (e.g. a GUI's terminal shell dies with it).
    pub(crate) parent: u64,
    pub(crate) privilege: PrivilegeLevel,
    pub(crate) main_tid: Option<u64>,
    pub(crate) threads: Vec<u64>,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProcessInfo {
    pub pid: u64,
    pub state: ProcessState,
    pub image_name: [u8; IMAGE_NAME_LEN],
    pub start_tsc: u64,
    pub entry: u64,
    pub stack_top: u64,
    pub step: u64,
    pub cpu_ticks: u64,
    pub memory_bytes: u64,
}

pub(crate) static PROCESSES: SyncUnsafeCell<Vec<Process>> = SyncUnsafeCell::new(Vec::new());
static NEXT_PID: SyncUnsafeCell<u64> = SyncUnsafeCell::new(1);
static INITIALIZED: SyncUnsafeCell<bool> = SyncUnsafeCell::new(false);

pub fn init() {
    unsafe {
        if *INITIALIZED.0.get() {
            return;
        }
        thread::init();
        let processes = &mut *PROCESSES.0.get();
        processes.clear();
        processes.push(kernel_process());
        *NEXT_PID.0.get() = 1;
        *INITIALIZED.0.get() = true;
    }
}

fn kernel_process() -> Process {
    let mut name = [0u8; IMAGE_NAME_LEN];
    let bytes = b"kernel";
    name[..bytes.len()].copy_from_slice(bytes);
    Process {
        pid: 0,
        state: ProcessState::Running,
        image_name: name,
        start_tsc: 0,
        entry: 0,
        stack_top: 0,
        step: 0,
        memory_bytes: 0,
        background: false,
        sigint_catch: false,
        sigint_pending: false,
        kill_pending: false,
        parent: 0,
        privilege: PrivilegeLevel::Driver,
        main_tid: None,
        threads: alloc::vec![],
    }
}

fn allocate_pid() -> u64 {
    unsafe {
        let pid = *NEXT_PID.0.get();
        *NEXT_PID.0.get() = pid + 1;
        pid
    }
}

fn create_process(image_name: &str, privilege: PrivilegeLevel, main_tid: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| unsafe {
        init();
        let pid = allocate_pid();
        let mut name = [0u8; IMAGE_NAME_LEN];
        let bytes = image_name.as_bytes();
        let len = bytes.len().min(IMAGE_NAME_LEN - 1);
        name[..len].copy_from_slice(&bytes[..len]);
        let processes = &mut *PROCESSES.0.get();
        processes.push(Process {
            pid,
            state: ProcessState::Sleeping,
            image_name: name,
            start_tsc: crate::util::rdtsc(),
            entry: 0,
            stack_top: 0,
            step: 0,
            memory_bytes: 0,
            background: false,
            sigint_catch: false,
            sigint_pending: false,
            kill_pending: false,
            parent: 0,
            privilege,
            main_tid: Some(main_tid),
            threads: alloc::vec![main_tid],
        });
        pid
    })
}

pub fn spawn(image_name: &str) -> u64 {
    crate::task::thread::with_threads_lock(|| {
        let tid = thread::create_kernel_thread(0, 0, 0, PrivilegeLevel::System);
        if tid == 0 {
            return 0;
        }
        let pid = create_process(image_name, PrivilegeLevel::System, tid);
        if pid == 0 {
            thread::remove_thread(tid);
            return 0;
        }
        thread::set_thread_pid(tid, pid);
        set_ready(pid);
        pid
    })
}

pub fn spawn_kernel_task(
    image_name: &str,
    entry: u64,
    arg: u64,
    privilege: PrivilegeLevel,
) -> u64 {
    crate::task::thread::with_threads_lock(|| {
        let tid = thread::create_kernel_thread(0, entry, arg, privilege);
        if tid == 0 {
            return 0;
        }
        let pid = create_process(image_name, privilege, tid);
        if pid == 0 {
            thread::remove_thread(tid);
            return 0;
        }
        thread::set_thread_pid(tid, pid);
        thread::set_user_context(tid, thread::UserContext {
            rip: entry,
            ..thread::EMPTY_USER_CONTEXT
        });
        set_ready(pid);
        pid
    })
}

pub fn spawn_user_process(
    image_name: &str,
    entry: u64,
    user_stack_top: u64,
    cr3: u64,
    privilege: PrivilegeLevel,
    argc: u64,
    argv: u64,
) -> u64 {
    crate::task::thread::with_threads_lock(|| {
        let tid = thread::create_kernel_thread(0, entry, 0, privilege);
        if tid == 0 {
            return 0;
        }
        let pid = create_process(image_name, privilege, tid);
        if pid == 0 {
            thread::remove_thread(tid);
            return 0;
        }
        thread::set_thread_pid(tid, pid);
        thread::set_user_context(
            tid,
            UserContext {
                rip: entry,
                rsp: user_stack_top,
                cr3,
                user_stack_top,
                rdi: argc,
                rsi: argv,
                ..thread::EMPTY_USER_CONTEXT
            },
        );
        set_ready(pid);
        pid
    })
}

pub fn set_privilege(pid: u64, privilege: PrivilegeLevel) {
    crate::task::thread::with_threads_lock(|| {
        unsafe {
            let processes = &mut *PROCESSES.0.get();
            if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
                p.privilege = privilege;
            }
        }
        if let Some(tid) = main_tid(pid) {
            thread::set_privilege(tid, privilege);
        }
    })
}

pub fn privilege_level(pid: u64) -> PrivilegeLevel {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &*PROCESSES.0.get();
        processes
            .iter()
            .find(|p| p.pid == pid)
            .map_or(PrivilegeLevel::User, |p| p.privilege)
    })
}

pub fn exit_current() {
    crate::task::thread::with_threads_lock(|| unsafe {
        let pid = current_pid();
        let tid = current_tid();
        if pid == 0 {
            if let Some(p) = (&mut *PROCESSES.0.get()).iter_mut().find(|p| p.pid == 0) {
                p.state = ProcessState::Running;
            }
        } else {
            kill_children(pid);
            crate::drivers::fb_owner::release(pid);
            crate::scheduler::clear_current_user(pid);
            crate::fd::close_all(pid);
            crate::user::free_dma_for_pid(pid);
            crate::user::free_heap_for_pid(pid);
            if let Some(cr3) = user_cr3(pid) {
                crate::vmm::switch_cr3(crate::vmm::kernel_cr3());
                crate::vmm::free_user_address_space(cr3);
            }
            remove_process(pid);
            wakeup_pid_waiters(pid);
        }
        clear_current_pid();
        if tid != 0 {
            thread::set_state(tid, thread::ThreadState::Exited);
        }
    })
}

fn remove_process(pid: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(pos) = processes.iter().position(|p| p.pid == pid) {
            let p = processes.swap_remove(pos);
            for tid in p.threads {
                thread::remove_thread(tid);
            }
        }
    })
}

pub fn send_sigint(pid: u64) {
    crate::task::thread::with_threads_lock(|| {
        let catch = unsafe {
            let processes = &mut *PROCESSES.0.get();
            processes.iter_mut().find(|p| p.pid == pid).map(|p| {
                if p.privilege <= PrivilegeLevel::Driver {
                    return true;
                }
                if p.sigint_catch {
                    p.sigint_pending = true;
                    if let Some(tid) = p.main_tid {
                        if matches!(thread::state(tid), Some(thread::ThreadState::Sleeping))
                            && thread::take_blocking_rsp(tid).is_some()
                        {
                            thread::apply_blocking_return_if_pending(tid);
                            thread::set_ready(tid);
                            thread::set_wait_target(tid, WaitTarget::None);
                        }
                    }
                    true
                } else {
                    false
                }
            })
        };
        if catch == Some(false) {
            kill_pid(pid);
        }
    })
}

/// The "foreground" process for terminal signals (Ctrl+C): the leaf of the wait
/// chain — a process that another process is blocked waiting on
/// (`WaitTarget::Pid`) and that is not itself waiting on a child. With the
/// shell's exec+wait model this is the program currently running in the
/// foreground. Returns `None` when nothing is being waited on (e.g. only
/// background jobs at the prompt), so Ctrl+C then has no foreground target.
pub fn foreground_pid() -> Option<u64> {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &*PROCESSES.0.get();
        let waits_on_pid = |p: &Process| -> bool {
            p.main_tid
                .and_then(thread::wait_target)
                .map(|t| matches!(t, WaitTarget::Pid(_)))
                .unwrap_or(false)
        };
        for p in processes.iter() {
            if p.pid == 0 || matches!(p.state, ProcessState::Empty | ProcessState::Exited) {
                continue;
            }
            let waited_on = processes.iter().any(|q| {
                q.main_tid
                    .and_then(thread::wait_target)
                    .map(|t| matches!(t, WaitTarget::Pid(target) if target == p.pid))
                    .unwrap_or(false)
            });
            if waited_on && !waits_on_pid(p) {
                return Some(p.pid);
            }
        }
        None
    })
}

pub fn send_module_exit(pid: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.sigint_pending = true;
            if let Some(tid) = p.main_tid {
                if matches!(thread::state(tid), Some(thread::ThreadState::Sleeping)) {
                    thread::apply_blocking_return_if_pending(tid);
                    thread::set_ready(tid);
                    thread::set_wait_target(tid, WaitTarget::None);
                }
            }
        }
    })
}

pub fn sigint_set_catch(pid: u64, catch: bool) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.sigint_catch = catch;
        }
    })
}

pub fn sigint_check_and_clear(pid: u64) -> bool {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            let pending = p.sigint_pending;
            p.sigint_pending = false;
            return pending;
        }
        false
    })
}

/// Pick an out-of-memory victim: the killable user process (not the kernel, not
/// a driver, not `exclude`) currently using the most memory.
pub fn oom_victim(exclude: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &*PROCESSES.0.get();
        let mut best: Option<(u64, u64)> = None; // (pid, memory_bytes)
        for p in processes.iter() {
            if p.pid == 0 || p.pid == exclude {
                continue;
            }
            if p.privilege <= PrivilegeLevel::Driver {
                continue;
            }
            if matches!(p.state, ProcessState::Empty | ProcessState::Exited) {
                continue;
            }
            if best.map_or(true, |(_, bm)| p.memory_bytes > bm) {
                best = Some((p.pid, p.memory_bytes));
            }
        }
        best.map(|(pid, _)| pid)
    })
}

/// Consume the deferred-kill flag for `pid`. Returns true if a kill was pending,
/// in which case the caller (the process's own thread, on its own CPU) should
/// exit itself via the normal self-exit path.
pub fn take_kill_pending(pid: u64) -> bool {
    crate::task::thread::with_threads_lock(|| unsafe {
        if let Some(p) = (&mut *PROCESSES.0.get()).iter_mut().find(|p| p.pid == pid) {
            let pending = p.kill_pending;
            p.kill_pending = false;
            pending
        } else {
            false
        }
    })
}

/// Request that `pid` die. We never free its fds/address space here: a remote
/// caller (another CPU, or an IRQ) cannot safely tear down a process whose
/// thread may still run — or be resumed from a saved blocking frame — against
/// the freed memory (SMP use-after-free). Instead we just flag it and make it
/// Ready. Because nothing is freed, the thread keeps running on its still-valid
/// address space until it next enters the kernel, where `syscall_dispatch` sees
/// `kill_pending` and self-exits via `exit_current()` on its own CPU (safe).
/// Follow `root`'s wait chain (root waits on X, X waits on Y, ...) to the running
/// leaf — the foreground process. Returns `root` itself if it isn't waiting on a pid.
pub fn foreground_leaf(root: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &*PROCESSES.0.get();
        let waits_on = |pid: u64| -> Option<u64> {
            processes
                .iter()
                .find(|p| p.pid == pid)
                .and_then(|p| p.main_tid)
                .and_then(thread::wait_target)
                .and_then(|t| if let WaitTarget::Pid(target) = t { Some(target) } else { None })
        };
        let mut cur = root;
        for _ in 0..64 {
            match waits_on(cur) {
                Some(next) => cur = next,
                None => break,
            }
        }
        cur
    })
}

pub fn set_parent(pid: u64, parent: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        if let Some(p) = (&mut *PROCESSES.0.get()).iter_mut().find(|p| p.pid == pid) {
            p.parent = parent;
        }
    })
}

/// Kill every process whose parent is `pid` (used when the parent exits).
fn kill_children(pid: u64) {
    let children: alloc::vec::Vec<u64> = unsafe {
        (*PROCESSES.0.get()).iter().filter(|p| p.parent == pid).map(|p| p.pid).collect()
    };
    for child in children {
        kill_pid(child);
    }
}

pub fn kill_pid(pid: u64) {
    crate::task::thread::with_threads_lock(|| {
        if pid == 0 {
            return;
        }
        unsafe {
            let processes = &mut *PROCESSES.0.get();
            if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
                if p.privilege <= PrivilegeLevel::Driver {
                    return;
                }
                p.kill_pending = true;
            } else {
                return;
            }
        }
        // Wake it (if blocked) so its CPU schedules it and reaps it promptly.
        if let Some(main) = main_tid(pid) {
            thread::set_ready(main);
        }
    })
}

pub fn set_running(pid: u64) {
    crate::task::thread::with_threads_lock(|| {
        crate::scheduler::set_current_user_pid(Some(pid));
        unsafe {
            let processes = &mut *PROCESSES.0.get();
            if let Some(p) = processes.iter_mut().find(|p| p.pid == pid)
                && !matches!(p.state, ProcessState::Empty | ProcessState::Exited)
            {
                p.state = ProcessState::Running;
            }
        }
        if let Some(tid) = main_tid(pid) {
            thread::set_running(tid);
            crate::scheduler::set_current_user_tid(Some(tid));
        }
    })
}

pub fn set_ready(pid: u64) {
    crate::task::thread::with_threads_lock(|| {
        unsafe {
            let processes = &mut *PROCESSES.0.get();
            if let Some(p) = processes.iter_mut().find(|p| p.pid == pid)
                && matches!(
                    p.state,
                    ProcessState::Ready | ProcessState::Running | ProcessState::Sleeping
                )
            {
                p.state = ProcessState::Ready;
            }
        }
        if let Some(tid) = main_tid(pid) {
            thread::set_ready(tid);
        }
    })
}

pub fn set_sleeping(pid: u64) {
    crate::task::thread::with_threads_lock(|| {
        unsafe {
            let processes = &mut *PROCESSES.0.get();
            if let Some(p) = processes.iter_mut().find(|p| p.pid == pid)
                && !matches!(p.state, ProcessState::Empty | ProcessState::Exited)
            {
                p.state = ProcessState::Sleeping;
            }
        }
        if let Some(tid) = main_tid(pid) {
            thread::set_sleeping(tid);
        }
    })
}

pub fn step(pid: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.step = p.step.wrapping_add(1);
            return p.step;
        }
        0
    })
}

pub fn add_cpu_ticks(pid: u64, ticks: u64) {
    crate::task::thread::with_threads_lock(|| {
        if let Some(tid) = main_tid(pid) {
            thread::add_cpu_ticks(tid, ticks);
        }
    })
}

pub fn set_memory_bytes(pid: u64, bytes: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.memory_bytes = bytes;
        }
    })
}

pub fn add_memory_bytes(pid: u64, bytes: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.memory_bytes = p.memory_bytes.saturating_add(bytes);
        }
    })
}

pub fn sub_memory_bytes(pid: u64, bytes: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.memory_bytes = p.memory_bytes.saturating_sub(bytes);
        }
    })
}

pub fn memory_bytes(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.memory_bytes)
    })
}

pub fn cpu_ticks(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| main_tid(pid).and_then(thread::cpu_ticks))
}

pub fn cpu_usage_permille(pid: u64, total_ticks: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| {
        cpu_ticks(pid).map(|ticks| {
            ticks
                .saturating_mul(1000)
                .checked_div(total_ticks)
                .unwrap_or(0)
        })
    })
}

pub fn total_cpu_ticks() -> u64 {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|p| p.pid != 0)
            .filter_map(|p| main_tid(p.pid).and_then(thread::cpu_ticks))
            .sum()
    })
}

pub fn user_cpu_ticks_total() -> u64 {
    crate::task::thread::with_threads_lock(|| total_cpu_ticks())
}

pub fn active_pid_after(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|p| {
                matches!(p.state, ProcessState::Ready | ProcessState::Running) && p.pid > pid
            })
            .map(|p| p.pid)
            .min()
    })
}

pub fn set_user_context(pid: u64, context: UserContext) {
    crate::task::thread::with_threads_lock(|| {
        if let Some(tid) = main_tid(pid) {
            thread::set_user_context(tid, context);
        }
    })
}

pub fn user_context(pid: u64) -> Option<UserContext> {
    crate::task::thread::with_threads_lock(|| main_tid(pid).and_then(thread::user_context))
}

pub fn user_cr3(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| main_tid(pid).and_then(thread::user_cr3))
}

pub fn user_stack_top(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| {
        main_tid(pid).and_then(|tid| thread::user_context(tid).map(|ctx| ctx.user_stack_top))
    })
}

pub fn kernel_rsp(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| main_tid(pid).and_then(thread::kernel_rsp))
}

pub fn set_kernel_rsp(pid: u64, rsp: u64) {
    crate::task::thread::with_threads_lock(|| {
        if let Some(tid) = main_tid(pid) {
            thread::set_kernel_rsp(tid, rsp);
        }
    })
}

pub fn kernel_stack_base(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| main_tid(pid).and_then(thread::kernel_stack_base))
}

pub fn kernel_stack_top(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| main_tid(pid).and_then(thread::kernel_stack_top))
}

pub fn set_background(pid: u64, background: bool) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.background = background;
        }
    })
}

pub fn is_background(pid: u64) -> bool {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.background)
            .unwrap_or(false)
    })
}

pub fn current_pid() -> u64 {
    crate::scheduler::current_user_pid().unwrap_or(0)
}

pub fn current_tid() -> u64 {
    crate::scheduler::current_user_tid().unwrap_or(0)
}

pub fn clear_current_pid() {
    crate::scheduler::set_current_user_pid(None);
}

pub fn count() -> u64 {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|p| !matches!(p.state, ProcessState::Empty))
            .count() as u64
    })
}

pub fn user_memory_total() -> u64 {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|p| p.pid != 0 && !matches!(p.state, ProcessState::Empty))
            .map(|p| p.memory_bytes)
            .sum()
    })
}

pub fn info(pid: u64) -> Option<ProcessInfo> {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|p| p.pid == pid && !matches!(p.state, ProcessState::Empty))
            .map(|p| {
                let memory_bytes = if p.pid == 0 {
                    let pmm_used = crate::pmm::stats()
                        .map(|s| s.used_kib() as u64 * 1024)
                        .unwrap_or(0);
                    pmm_used.saturating_sub(user_memory_total())
                } else {
                    p.memory_bytes
                };
                let cpu_ticks = if p.pid == 0 {
                    crate::handlers::interrupts::kernel_cpu_ticks()
                } else {
                    main_tid(p.pid).and_then(thread::cpu_ticks).unwrap_or(0)
                };
                ProcessInfo {
                    pid: p.pid,
                    state: p.state,
                    image_name: p.image_name,
                    start_tsc: p.start_tsc,
                    entry: p.entry,
                    stack_top: p.stack_top,
                    step: p.step,
                    cpu_ticks,
                    memory_bytes,
                }
            })
    })
}

pub fn first_pid() -> Option<u64> {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|p| !matches!(p.state, ProcessState::Empty))
            .map(|p| p.pid)
    })
}

pub fn next_pid_after(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|p| !matches!(p.state, ProcessState::Empty) && p.pid > pid)
            .map(|p| p.pid)
            .min()
    })
}

pub fn active_count() -> u64 {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|p| {
                matches!(
                    p.state,
                    ProcessState::Ready | ProcessState::Running | ProcessState::Sleeping
                )
            })
            .count() as u64
    })
}

pub fn set_kernel_preempted(pid: u64, val: bool) {
    crate::task::thread::with_threads_lock(|| {
        if let Some(tid) = main_tid(pid) {
            thread::set_kernel_preempted(tid, val);
        }
    })
}

pub fn is_kernel_preempted(pid: u64) -> bool {
    crate::task::thread::with_threads_lock(|| main_tid(pid).map_or(false, thread::is_kernel_preempted))
}

pub fn set_blocking_rsp(pid: u64, rsp: u64) {
    crate::task::thread::with_threads_lock(|| {
        if let Some(tid) = main_tid(pid) {
            thread::set_blocking_rsp(tid, rsp);
        }
    })
}

pub fn take_blocking_rsp(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| main_tid(pid).and_then(thread::take_blocking_rsp))
}

pub fn take_blocking_retval(pid: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| main_tid(pid).map_or(0, thread::take_blocking_retval))
}

pub fn apply_blocking_return_if_pending(pid: u64) -> bool {
    crate::task::thread::with_threads_lock(|| main_tid(pid).map_or(false, thread::apply_blocking_return_if_pending))
}

pub fn set_wait_target(pid: u64, target: WaitTarget) {
    crate::task::thread::with_threads_lock(|| {
        if let Some(tid) = main_tid(pid) {
            thread::set_wait_target(tid, target);
        }
    })
}

pub fn main_tid(pid: u64) -> Option<u64> {
    crate::task::thread::with_threads_lock(|| unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|p| p.pid == pid)
            .and_then(|p| p.main_tid)
    })
}

pub fn wakeup_key_waiters(key: u8) -> usize {
    crate::task::thread::with_threads_lock(|| thread::wakeup_key_waiters(key))
}

pub fn wakeup_key_waiter_for_pid(pid: u64, key: u8) -> usize {
    crate::task::thread::with_threads_lock(|| thread::wakeup_key_waiter_for_pid(pid, key))
}

pub fn wakeup_pid_waiters(exited_pid: u64) {
    crate::task::thread::with_threads_lock(|| thread::wakeup_pid_waiters(exited_pid))
}

pub fn wakeup_ipc_waiter(pid: u64, retval: u64) {
    crate::task::thread::with_threads_lock(|| {
        if let Some(tid) = main_tid(pid) {
            thread::wakeup_ipc_waiter(tid, retval);
        }
    })
}

pub fn wakeup_irq_waiter(irq: u8) -> bool {
    crate::task::thread::with_threads_lock(|| thread::wakeup_irq_waiter(irq))
}

pub fn notify_pipe_readers(pipe_id: u64) {
    crate::task::thread::with_threads_lock(|| thread::notify_pipe_readers(pipe_id))
}

pub fn wakeup_audio_waiters() {
    crate::task::thread::with_threads_lock(|| thread::wakeup_audio_waiters())
}

pub fn wake_timer_sleepers(now_tsc: u64) {
    crate::task::thread::with_threads_lock(|| thread::wake_timer_sleepers(now_tsc))
}

pub fn wake_tick_sleepers() {
    crate::task::thread::with_threads_lock(|| thread::wake_tick_sleepers())
}

pub fn add_thread_to_process(pid: u64, tid: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.threads.push(tid);
        }
    })
}

pub fn remove_thread_from_process(pid: u64, tid: u64) {
    crate::task::thread::with_threads_lock(|| unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.threads.retain(|&t| t != tid);
        }
    })
}
