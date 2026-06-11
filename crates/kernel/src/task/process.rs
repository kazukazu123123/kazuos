use alloc::alloc::{Layout, alloc_zeroed};
use alloc::vec::Vec;

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
    Ipc(u64), // channel_id
    Irq(u8),
    PipeRead { pipe_id: u64, buf_ptr: u64, buf_len: u64 },
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

const EMPTY_USER_CONTEXT: UserContext = UserContext {
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
pub(crate) struct Process {
    pub(crate) pid: u64,
    pub(crate) state: ProcessState,
    image_name: [u8; IMAGE_NAME_LEN],
    start_tsc: u64,
    entry: u64,
    stack_top: u64,
    step: u64,
    cpu_ticks: u64,
    memory_bytes: u64,
    user_context: UserContext,
    background: bool,
    kernel_rsp: u64,
    kernel_stack_base: u64,
    pub(crate) kernel_preempted: bool,
    // For blocking syscalls (SYS_KEYBOARD_READ, SYS_WAIT, SYS_SLEEP):
    // blocking_rsp points to the saved int-0x80 frame on the process's kernel stack.
    blocking_rsp: u64,
    blocking_retval: u64,
    pub(crate) wait_target: WaitTarget,
    pub(crate) sigint_catch: bool,
    pub(crate) sigint_pending: bool,
    pub(crate) privilege: PrivilegeLevel,
}

const _: () = assert!(core::mem::size_of::<Process>() == 0x150);

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

const KERNEL_NAME: [u8; IMAGE_NAME_LEN] = [
    b'k', b'e', b'r', b'n', b'e', b'l', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0,
];

const KERNEL_PROCESS: Process = Process {
    pid: 0,
    state: ProcessState::Running,
    image_name: KERNEL_NAME,
    start_tsc: 0,
    entry: 0,
    stack_top: 0,
    step: 0,
    cpu_ticks: 0,
    memory_bytes: 0,
    user_context: EMPTY_USER_CONTEXT,
    background: false,
    kernel_rsp: 0,
    kernel_stack_base: 0,
    kernel_preempted: false,
    blocking_rsp: 0,
    blocking_retval: 0,
    wait_target: WaitTarget::None,
    sigint_catch: false,
    sigint_pending: false,
    privilege: PrivilegeLevel::Driver,
};

pub(crate) static PROCESSES: SyncUnsafeCell<Vec<Process>> = SyncUnsafeCell::new(Vec::new());
static NEXT_PID: SyncUnsafeCell<u64> = SyncUnsafeCell::new(1);
pub(crate) static CURRENT_PID: SyncUnsafeCell<u64> = SyncUnsafeCell::new(0);
static INITIALIZED: SyncUnsafeCell<bool> = SyncUnsafeCell::new(false);

pub fn init() {
    unsafe {
        if *INITIALIZED.0.get() {
            return;
        }
        let processes = &mut *PROCESSES.0.get();
        processes.clear();
        processes.push(KERNEL_PROCESS);
        *CURRENT_PID.0.get() = 0;
        *NEXT_PID.0.get() = 1;
        *INITIALIZED.0.get() = true;
    }
}

pub fn spawn(image_name: &str) -> u64 {
    let pid = spawn_kernel_task(image_name, 0, 0, PrivilegeLevel::System);
    if pid != 0 {
        set_ready(pid);
    }
    pid
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
    let pid = spawn_kernel_task(image_name, entry, 0, privilege);
    if pid == 0 {
        return 0;
    }
    set_user_context(
        pid,
        UserContext {
            rip: entry,
            rsp: user_stack_top,
            cr3,
            user_stack_top,
            // SysV passes argc/argv on the stack, but our _start is a normal extern "C"
            // function reading them from rdi/rsi — so deliver them there too.
            rdi: argc,
            rsi: argv,
            ..EMPTY_USER_CONTEXT
        },
    );
    set_ready(pid);
    pid
}

pub const KERNEL_STACK_SIZE: usize = 65536;

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
        // Layout must match enter_next_process pop order:
        // pop r15,r14,r13,r12,rbp,rbx,r11,r10,r9,r8,rdi,rsi,rdx,rcx,rax, then iretq.
        p.add(0).write(0); // r15
        p.add(1).write(0); // r14
        p.add(2).write(0); // r13
        p.add(3).write(0); // r12
        p.add(4).write(0); // rbp
        p.add(5).write(0); // rbx
        p.add(6).write(0); // r11
        p.add(7).write(0); // r10
        p.add(8).write(0); // r9
        p.add(9).write(0); // r8
        p.add(10).write(arg); // rdi (argument)
        p.add(11).write(0); // rsi
        p.add(12).write(0); // rdx
        p.add(13).write(0); // rcx
        p.add(14).write(0); // rax
        p.add(15).write(entry); // RIP
        p.add(16).write(0x8); // CS (kernel code)
        p.add(17).write(0x202); // RFLAGS (IF set)
        p.add(18).write(stack_top); // RSP
        p.add(19).write(0x10); // SS (kernel data)
    }
    kernel_rsp
}

pub fn spawn_kernel_task(
    image_name: &str,
    entry: u64,
    arg: u64,
    privilege: PrivilegeLevel,
) -> u64 {
    unsafe {
        init();
        let stack_base = match alloc_kernel_stack() {
            Some(base) => base,
            None => return 0,
        };
        let kernel_rsp = setup_kernel_task_stack(stack_base, entry, arg);
        let pid = *NEXT_PID.0.get();
        *NEXT_PID.0.get() = pid + 1;
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
            entry,
            stack_top: 0,
            step: 0,
            cpu_ticks: 0,
            memory_bytes: 0,
            user_context: EMPTY_USER_CONTEXT,
            background: false,
            kernel_rsp,
            kernel_stack_base: stack_base,
            kernel_preempted: false,
            blocking_rsp: 0,
            blocking_retval: 0,
            wait_target: WaitTarget::None,
            sigint_catch: false,
            sigint_pending: false,
            privilege,
        });
        pid
    }
}

pub fn set_privilege(pid: u64, privilege: PrivilegeLevel) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.privilege = privilege;
        }
    }
}

pub fn privilege_level(pid: u64) -> PrivilegeLevel {
    unsafe {
        let processes = &*PROCESSES.0.get();
        processes
            .iter()
            .find(|p| p.pid == pid)
            .map_or(PrivilegeLevel::User, |p| p.privilege)
    }
}

pub fn exit_current() {
    unsafe {
        let pid = *CURRENT_PID.0.get();
        let processes = &mut *PROCESSES.0.get();
        if pid == 0 {
            if let Some(p) = processes.iter_mut().find(|p| p.pid == 0) {
                p.state = ProcessState::Running;
            }
        } else {
            crate::drivers::fb_owner::release(pid);
            crate::scheduler::clear_current_user(pid);
            crate::fd::close_all(pid);
            crate::user::free_dma_for_pid(pid);
            crate::user::free_heap_for_pid(pid);
            processes.retain(|p| p.pid != pid);
            wakeup_pid_waiters(pid);
        }
        *CURRENT_PID.0.get() = 0;
    }
}

pub fn send_sigint(pid: u64) {
    let catch = unsafe {
        let processes = &mut *PROCESSES.0.get();
        processes.iter_mut().find(|p| p.pid == pid).map(|p| {
            if p.privilege <= PrivilegeLevel::Driver {
                return true; // drivers are immune to SIGINT
            }
            if p.sigint_catch {
                p.sigint_pending = true;
                true
            } else {
                false
            }
        })
    };
    if catch == Some(false) {
        kill_pid(pid);
    }
}

/// Signal a kernel module to exit gracefully. Unlike send_sigint, this bypasses
/// the driver immunity so kmod::unload() can reach module processes. It also
/// wakes the process if it is blocked in a sleep or IRQ-wait so it can check
/// the pending signal without waiting for the next event.
pub fn send_module_exit(pid: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.sigint_pending = true;
            if matches!(p.state, ProcessState::Sleeping) {
                restore_ctx_from_blocking_frame(p, 0);
                p.state = ProcessState::Ready;
                p.wait_target = WaitTarget::None;
            }
        }
    }
}

pub fn sigint_set_catch(pid: u64, catch: bool) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.sigint_catch = catch;
        }
    }
}

pub fn sigint_check_and_clear(pid: u64) -> bool {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            let pending = p.sigint_pending;
            p.sigint_pending = false;
            return pending;
        }
        false
    }
}

pub fn kill_pid(pid: u64) {
    if pid == 0 {
        return;
    }
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter().find(|p| p.pid == pid) {
            if p.privilege <= PrivilegeLevel::Driver {
                return; // driver processes cannot be killed
            }
            crate::drivers::fb_owner::release(pid);
            crate::scheduler::clear_current_user(pid);
            crate::fd::close_all(pid);
            crate::user::free_dma_for_pid(pid);
            crate::user::free_heap_for_pid(pid);
            processes.retain(|p| p.pid != pid);
            wakeup_pid_waiters(pid);
        }
    }
}

pub fn set_running(pid: u64) {
    unsafe {
        *CURRENT_PID.0.get() = pid;
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid)
            && !matches!(process.state, ProcessState::Empty | ProcessState::Exited)
        {
            process.state = ProcessState::Running;
        }
    }
}

pub fn set_ready(pid: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid)
            && matches!(
                process.state,
                ProcessState::Ready | ProcessState::Running | ProcessState::Sleeping
            )
        {
            process.state = ProcessState::Ready;
        }
    }
}

pub fn set_sleeping(pid: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid)
            && !matches!(process.state, ProcessState::Empty | ProcessState::Exited)
        {
            process.state = ProcessState::Sleeping;
        }
    }
}

pub fn step(pid: u64) -> u64 {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid) {
            process.step = process.step.wrapping_add(1);
            return process.step;
        }
        0
    }
}

pub fn add_cpu_ticks(pid: u64, ticks: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid) {
            process.cpu_ticks = process.cpu_ticks.saturating_add(ticks);
        }
    }
}

pub fn set_memory_bytes(pid: u64, bytes: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid) {
            process.memory_bytes = bytes;
        }
    }
}

pub fn add_memory_bytes(pid: u64, bytes: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid) {
            process.memory_bytes = process.memory_bytes.saturating_add(bytes);
        }
    }
}

pub fn memory_bytes(pid: u64) -> Option<u64> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.memory_bytes)
    }
}

pub fn cpu_ticks(pid: u64) -> Option<u64> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.cpu_ticks)
    }
}

pub fn cpu_usage_permille(pid: u64, total_ticks: u64) -> Option<u64> {
    cpu_ticks(pid).map(|ticks| {
        ticks
            .saturating_mul(1000)
            .checked_div(total_ticks)
            .unwrap_or(0)
    })
}

pub fn total_cpu_ticks() -> u64 {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|process| process.pid != 0)
            .map(|process| process.cpu_ticks)
            .sum()
    }
}

pub fn user_cpu_ticks_total() -> u64 {
    total_cpu_ticks()
}

pub fn active_pid_after(pid: u64) -> Option<u64> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|process| {
                matches!(process.state, ProcessState::Ready | ProcessState::Running)
                    && process.pid > pid
            })
            .map(|process| process.pid)
            .min()
    }
}

pub fn set_user_context(pid: u64, context: UserContext) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid) {
            process.user_context = context;
        }
    }
}

pub fn user_context(pid: u64) -> Option<UserContext> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.user_context)
    }
}

pub fn user_cr3(pid: u64) -> Option<u64> {
    user_context(pid).map(|context| context.cr3)
}

pub fn user_stack_top(pid: u64) -> Option<u64> {
    user_context(pid).map(|context| context.user_stack_top)
}

pub fn kernel_rsp(pid: u64) -> Option<u64> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.kernel_rsp)
    }
}

pub fn set_kernel_rsp(pid: u64, rsp: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid) {
            process.kernel_rsp = rsp;
        }
    }
}

pub fn kernel_stack_base(pid: u64) -> Option<u64> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.kernel_stack_base)
    }
}

pub fn kernel_stack_top(pid: u64) -> Option<u64> {
    kernel_stack_base(pid).map(|base| base + KERNEL_STACK_SIZE as u64)
}

pub fn set_background(pid: u64, background: bool) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|process| process.pid == pid) {
            process.background = background;
        }
    }
}

pub fn is_background(pid: u64) -> bool {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| process.pid == pid)
            .map(|process| process.background)
            .unwrap_or(false)
    }
}

pub fn current_pid() -> u64 {
    unsafe { *CURRENT_PID.0.get() }
}

pub fn clear_current_pid() {
    unsafe {
        *CURRENT_PID.0.get() = 0;
    }
}

pub fn count() -> u64 {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|process| !matches!(process.state, ProcessState::Empty))
            .count() as u64
    }
}

pub fn user_memory_total() -> u64 {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|p| p.pid != 0 && !matches!(p.state, ProcessState::Empty))
            .map(|p| p.memory_bytes)
            .sum()
    }
}

pub fn info(pid: u64) -> Option<ProcessInfo> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| process.pid == pid && !matches!(process.state, ProcessState::Empty))
            .map(|process| {
                let memory_bytes = if process.pid == 0 {
                    // PMM used - user processes = kernel memory
                    let pmm_used = crate::pmm::stats()
                        .map(|s| s.used_kib() as u64 * 1024)
                        .unwrap_or(0);
                    pmm_used.saturating_sub(user_memory_total())
                } else {
                    process.memory_bytes
                };
                ProcessInfo {
                    pid: process.pid,
                    state: process.state,
                    image_name: process.image_name,
                    start_tsc: process.start_tsc,
                    entry: process.entry,
                    stack_top: process.stack_top,
                    step: process.step,
                    cpu_ticks: process.cpu_ticks,
                    memory_bytes,
                }
            })
    }
}

pub fn first_pid() -> Option<u64> {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|process| !matches!(process.state, ProcessState::Empty))
            .map(|process| process.pid)
    }
}

pub fn next_pid_after(pid: u64) -> Option<u64> {
    // u64::MAX is the "before first" sentinel — return the minimum pid including 0
    let filter_pid = if pid == u64::MAX { u64::MAX } else { pid };
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|process| {
                !matches!(process.state, ProcessState::Empty)
                    && if filter_pid == u64::MAX {
                        true
                    } else {
                        process.pid > filter_pid
                    }
            })
            .map(|process| process.pid)
            .min()
    }
}

pub fn active_count() -> u64 {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .filter(|process| {
                matches!(
                    process.state,
                    ProcessState::Ready | ProcessState::Running | ProcessState::Sleeping
                )
            })
            .count() as u64
    }
}

pub fn set_kernel_preempted(pid: u64, val: bool) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(process) = processes.iter_mut().find(|p| p.pid == pid) {
            process.kernel_preempted = val;
        }
    }
}

pub fn is_kernel_preempted(pid: u64) -> bool {
    unsafe {
        (*PROCESSES.0.get())
            .iter()
            .find(|p| p.pid == pid)
            .map(|p| p.kernel_preempted)
            .unwrap_or(false)
    }
}

/// Read the saved int-0x80 frame at `rsp` and update the process's user_context so the
/// process can be resumed via the normal user_context iretq path with `retval` in rax.
/// Also clears blocking_rsp and kernel_preempted.
///
/// Frame layout (syscall_int80_asm, 14 pushes):
///   [rsp+0]   = r15, [+8]=r14, [+16]=r13, [+24]=r12, [+32]=r11, [+40]=r10,
///   [+48]=r9, [+56]=r8, [+64]=rdi, [+72]=rsi, [+80]=rdx, [+88]=rcx, [+96]=rbx,
///   [+104]=rbp
///   [+112]=user_rip, [+120]=user_cs, [+128]=user_rflags, [+136]=user_rsp, [+144]=user_ss
unsafe fn restore_ctx_from_blocking_frame(p: &mut Process, retval: u64) {
    let rsp = p.blocking_rsp;
    if rsp == 0 {
        return;
    }
    unsafe {
        let f = rsp as *const u64;
        let user_rip = *f.add(14);
        let user_rflags = *f.add(16);
        let user_rsp = *f.add(17);
        let ctx = &mut p.user_context;
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
    // cr3 unchanged
    p.blocking_rsp = 0;
    p.kernel_preempted = false;
}

pub fn set_blocking_rsp(pid: u64, rsp: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.blocking_rsp = rsp;
        }
    }
}

/// Returns the saved syscall frame RSP and clears it (consumed once).
pub fn take_blocking_rsp(pid: u64) -> Option<u64> {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            if p.blocking_rsp != 0 {
                let rsp = p.blocking_rsp;
                p.blocking_rsp = 0;
                return Some(rsp);
            }
        }
        None
    }
}

pub fn take_blocking_retval(pid: u64) -> u64 {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            let v = p.blocking_retval;
            p.blocking_retval = 0;
            return v;
        }
        0
    }
}

/// If the process was woken from a blocking syscall, update its user_context from the
/// saved int-0x80 frame (consuming blocking_rsp) so the normal user_context iretq path
/// resumes at the correct return address with the correct registers.
pub fn apply_blocking_return_if_pending(pid: u64) -> bool {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            if p.blocking_rsp != 0 {
                let retval = p.blocking_retval;
                p.blocking_retval = 0;
                restore_ctx_from_blocking_frame(p, retval);
                return true;
            }
        }
        false
    }
}

pub fn set_wait_target(pid: u64, target: WaitTarget) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            p.wait_target = target;
        }
    }
}


/// Wake one process sleeping waiting for a keyboard key; returns 1 if a waiter was found.
/// When a waiter is found the key is delivered directly (no buffer) to avoid double delivery.
pub fn wakeup_key_waiters(key: u8) -> usize {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        for p in processes.iter_mut() {
            if matches!(p.state, ProcessState::Sleeping)
                && matches!(p.wait_target, WaitTarget::Keyboard)
            {
                restore_ctx_from_blocking_frame(p, key as u64);
                p.state = ProcessState::Ready;
                p.wait_target = WaitTarget::None;
                return 1;
            }
        }
        0
    }
}

/// Wake all processes sleeping waiting for `exited_pid` to finish.
pub fn wakeup_pid_waiters(exited_pid: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        for p in processes.iter_mut() {
            if matches!(p.state, ProcessState::Sleeping)
                && matches!(p.wait_target, WaitTarget::Pid(target_pid) if target_pid == exited_pid)
            {
                restore_ctx_from_blocking_frame(p, 1);
                p.state = ProcessState::Ready;
                p.wait_target = WaitTarget::None;
            }
        }
    }
}

/// Wake a specific process sleeping on IPC.
pub fn wakeup_ipc_waiter(pid: u64, retval: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        if let Some(p) = processes.iter_mut().find(|p| p.pid == pid) {
            if matches!(p.state, ProcessState::Sleeping)
                && matches!(p.wait_target, WaitTarget::Ipc(_))
            {
                restore_ctx_from_blocking_frame(p, retval);
                p.state = ProcessState::Ready;
                p.wait_target = WaitTarget::None;
            }
        }
    }
}

/// Wake the process sleeping on the given IRQ number. Returns true if a waiter was found.
pub fn wakeup_irq_waiter(irq: u8) -> bool {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        for p in processes.iter_mut() {
            if matches!(p.state, ProcessState::Sleeping)
                && matches!(p.wait_target, WaitTarget::Irq(n) if n == irq)
            {
                restore_ctx_from_blocking_frame(p, 0);
                p.state = ProcessState::Ready;
                p.wait_target = WaitTarget::None;
                return true;
            }
        }
        false
    }
}

/// Wake all processes sleeping waiting to read from a pipe.
/// Performs the actual read so the syscall returns the correct byte count.
pub fn notify_pipe_readers(pipe_id: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        for p in processes.iter_mut() {
            if matches!(p.state, ProcessState::Sleeping) {
                if let WaitTarget::PipeRead { pipe_id: id, buf_ptr, buf_len } = p.wait_target {
                    if id == pipe_id {
                        let n = crate::pipe::read_raw(pipe_id, buf_ptr, buf_len as usize);
                        restore_ctx_from_blocking_frame(p, n as u64);
                        p.state = ProcessState::Ready;
                        p.wait_target = WaitTarget::None;
                    }
                }
            }
        }
    }
}

/// Wake any Timer-sleeping processes whose deadline has passed.
pub fn wake_timer_sleepers(now_tsc: u64) {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        for p in processes.iter_mut() {
            if matches!(p.state, ProcessState::Sleeping) {
                if let WaitTarget::Timer(deadline) = p.wait_target {
                    if now_tsc >= deadline {
                        restore_ctx_from_blocking_frame(p, 0);
                        p.state = ProcessState::Ready;
                        p.wait_target = WaitTarget::None;
                    }
                }
            }
        }
    }
}

/// Wake all processes sleeping with WaitTarget::Tick. Called on every timer tick.
pub fn wake_tick_sleepers() {
    unsafe {
        let processes = &mut *PROCESSES.0.get();
        for p in processes.iter_mut() {
            if matches!(p.state, ProcessState::Sleeping)
                && matches!(p.wait_target, WaitTarget::Tick)
            {
                restore_ctx_from_blocking_frame(p, 0);
                p.state = ProcessState::Ready;
                p.wait_target = WaitTarget::None;
            }
        }
    }
}


