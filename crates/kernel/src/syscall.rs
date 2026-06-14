use core::arch::global_asm;

static mut SYS_HANDLER: Option<extern "C" fn(u64, u64, u64, u64) -> u64> = None;
static mut SYS_STDOUT: u64 = 0;

pub const EXIT_TO_KERNEL: u64 = u64::MAX - 1;
pub const BLOCK_TO_SCHEDULER: u64 = u64::MAX - 2;

pub fn init() {}

pub fn register(handler: extern "C" fn(u64, u64, u64, u64) -> u64, stdout: u64) {
    unsafe {
        SYS_HANDLER = Some(handler);
        SYS_STDOUT = stdout;
    }
}

pub fn stdout() -> u64 {
    unsafe { SYS_STDOUT }
}

pub fn invoke(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    syscall_handler(number, arg0, arg1, arg2)
}

#[unsafe(no_mangle)]
extern "C" fn syscall_handler(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    unsafe {
        if let Some(handler) = SYS_HANDLER {
            handler(number, arg0, arg1, arg2)
        } else {
            0
        }
    }
}

global_asm!(
    ".global syscall_int80_asm",
    "syscall_int80_asm:",
    "    push rbp",
    "    push rbx",
    "    push rcx",
    "    push rdx",
    "    push rsi",
    "    push rdi",
    "    push r8",
    "    push r9",
    "    push r10",
    "    push r11",
    "    push r12",
    "    push r13",
    "    push r14",
    "    push r15",
    // 14 regs pushed + 5 CPU frame values = 152 bytes; need RSP%16==0 before call.
    // RSP0 is 16-byte aligned, so RSP0-152 ≡ 8(mod16).  Pad by 8 to realign.
    "    sub rsp, 8",
    "    mov rcx, rdx",
    "    mov rdx, rsi",
    "    mov rsi, rdi",
    "    mov rdi, rax",
    "    call syscall_handler",
    // check EXIT_TO_KERNEL (0xfffffffffffffffe)
    "    mov rbx, 0xfffffffffffffffe",
    "    cmp rax, rbx",
    "    je 2f",
    // check BLOCK_TO_SCHEDULER (0xfffffffffffffffd)
    "    mov rbx, 0xfffffffffffffffd",
    "    cmp rax, rbx",
    "    je 3f",
    // normal return: undo alignment pad, then restore all 14 regs
    "    add rsp, 8",
    "    pop r15",
    "    pop r14",
    "    pop r13",
    "    pop r12",
    "    pop r11",
    "    pop r10",
    "    pop r9",
    "    pop r8",
    "    pop rdi",
    "    pop rsi",
    "    pop rdx",
    "    pop rcx",
    "    pop rbx",
    "    pop rbp",
    "    iretq",
    // EXIT_TO_KERNEL path
    "2:",
    "    call {get_return_stack}",
    "    mov rsp, [rax]",
    "    jmp {return_fn}",
    // BLOCK_TO_SCHEDULER path: save frame RSP, switch to kernel stack, call block handler
    // rsp still has the 8-byte alignment pad below r15; skip it so blocking_rsp → r15 slot.
    "3:",
    "    lea rax, [rsp+8]",
    "    push rax",
    "    call {get_blocking_rsp_tmp}",
    "    pop rdi",
    "    mov [rax], rdi",
    "    call {get_return_stack}",
    "    mov rsp, [rax]",
    "    jmp {block_fn}",
    get_return_stack = sym crate::user::kernel_return_stack_ptr,
    get_blocking_rsp_tmp = sym crate::user::blocking_rsp_tmp_ptr,
    return_fn = sym syscall_return_to_kernel,
    block_fn = sym syscall_block_fn,
);

unsafe extern "C" {
    fn syscall_int80_asm();
}

pub fn handler_addr() -> u64 {
    syscall_int80_asm as *const () as usize as u64
}

/// Called when a syscall returns BLOCK_TO_SCHEDULER.
/// The process's int-0x80 frame is already saved to BLOCKING_RSP_TMP by the asm stub.
#[unsafe(no_mangle)]
extern "C" fn syscall_block_fn() -> ! {
    unsafe {
        crate::vmm::switch_cr3(crate::vmm::kernel_cr3());
        core::arch::asm!(
            "mov ds, ax",
            "mov es, ax",
            in("ax") crate::gdt::KERNEL_DATA,
            options(nostack, preserves_flags),
        );
        let blocking_rsp = crate::user::blocking_rsp_tmp();
        if let Some(pid) = crate::scheduler::current_user_pid() {
            crate::process::set_blocking_rsp(pid, blocking_rsp);
        }
        crate::scheduler::set_current_user_pid(None);
        crate::process::clear_current_pid();
        core::arch::asm!("pop rax", options(nostack, preserves_flags));
    }
    crate::scheduler::enter_next_process()
}

#[unsafe(no_mangle)]
extern "C" fn syscall_return_to_kernel() -> ! {
    unsafe {
        crate::vmm::switch_cr3(crate::vmm::kernel_cr3());
        core::arch::asm!(
            "mov ds, ax",
            "mov es, ax",
            in("ax") crate::gdt::KERNEL_DATA,
            options(nostack, preserves_flags),
        );
        let exiting_pid = crate::user::exiting_pid_tmp();
        crate::user::set_exiting_pid_tmp(0);
        crate::scheduler::set_current_user_pid(None);
        if exiting_pid != 0 {
            crate::kmod::on_process_exit(exiting_pid);
        }
        crate::scheduler::run_exit_handler();
        core::arch::asm!("pop rax", options(nostack, preserves_flags),);
    }
    crate::scheduler::enter_next_process();
}
