use core::arch::global_asm;

use crate::{idt, util::pause};

#[repr(C)]
pub struct FaultRegisters {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rbp: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
}

#[unsafe(no_mangle)]
unsafe extern "C" fn fault_handler_inner(
    vector: u64,
    error_code: u64,
    registers: *const FaultRegisters,
    stack_frame: *const idt::InterruptStackFrame,
) -> ! {
    let frame = unsafe { &*stack_frame };
    let regs = unsafe { &*registers };
    let cr2 = if vector == 14 { read_cr2() } else { 0 };
    let name = exception_name(vector);

    // User-mode exception (ring 3): kill the faulting process and reschedule.
    if frame.code_segment & 3 == 3 {
        let pid = crate::scheduler::current_user_pid().unwrap_or(0);
        crate::log_error!(
            "USER FAULT: {} pid={} vector={} error={:#x} rip={:#x}",
            name, pid, vector, error_code, frame.instruction_pointer
        );
        if pid != 0 {
            unsafe {
                let processes = &mut *crate::task::process::PROCESSES.0.get();
                crate::drivers::fb_owner::release(pid);
                crate::scheduler::clear_current_user(pid);
                processes.retain(|p| p.pid != pid);
            }
        }
        crate::scheduler::enter_next_process();
    }

    crate::log_fatal!(
        "KERNEL PANIC: {} vector={} error={:#x} rip={:#x} cs={:#x} rflags={:#x}",
        name, vector, error_code, frame.instruction_pointer, frame.code_segment, frame.cpu_flags
    );
    crate::serial_println!(
        "rsp={:#x} ss={:#x} cr2={:#x}",
        frame.stack_pointer,
        frame.stack_segment,
        cr2
    );
    crate::serial_println!(
        "rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x}",
        regs.rax,
        regs.rbx,
        regs.rcx,
        regs.rdx
    );
    crate::serial_println!(
        "rsi={:#x} rdi={:#x} rbp={:#x}",
        regs.rsi,
        regs.rdi,
        regs.rbp
    );
    crate::serial_println!(
        "r8={:#x} r9={:#x} r10={:#x} r11={:#x}",
        regs.r8,
        regs.r9,
        regs.r10,
        regs.r11
    );
    crate::serial_println!(
        "r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        regs.r12,
        regs.r13,
        regs.r14,
        regs.r15
    );
    if vector == 14 {
        unsafe {
            dump_page_table(cr2);
        }
    }
    loop {
        pause();
    }
}

unsafe fn dump_page_table(virt: u64) {
    let cr3: u64;
    unsafe {
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack));
    }
    crate::serial_println!("--- Page table walk for {:#x} (cr3={:#x}) ---", virt, cr3);
    let pml4_base = cr3 & 0x000f_ffff_ffff_f000;
    let pml4_i = (virt >> 39) & 0x1ff;
    let pml4_entry = unsafe { *(pml4_base as *const u64).add(pml4_i as usize) };
    crate::serial_println!("PML4[{:#x}] = {:#018x}", pml4_i, pml4_entry);
    if pml4_entry & 1 == 0 {
        crate::serial_println!("  -> NOT PRESENT");
        return;
    }
    crate::serial_println!(
        "  -> present={} user={} writable={} nx={}",
        (pml4_entry >> 0) & 1,
        (pml4_entry >> 2) & 1,
        (pml4_entry >> 1) & 1,
        (pml4_entry >> 63) & 1
    );
    let pdpt_base = pml4_entry & 0x000f_ffff_ffff_f000;
    let pdpt_i = (virt >> 30) & 0x1ff;
    let pdpt_entry = unsafe { *(pdpt_base as *const u64).add(pdpt_i as usize) };
    crate::serial_println!("PDPT[{:#x}] = {:#018x}", pdpt_i, pdpt_entry);
    if pdpt_entry & 1 == 0 {
        crate::serial_println!("  -> NOT PRESENT");
        return;
    }
    crate::serial_println!(
        "  -> present={} user={} writable={} nx={}",
        (pdpt_entry >> 0) & 1,
        (pdpt_entry >> 2) & 1,
        (pdpt_entry >> 1) & 1,
        (pdpt_entry >> 63) & 1
    );
    if pdpt_entry & (1 << 7) != 0 {
        crate::serial_println!("  -> 1G page (huge)");
        return;
    }
    let pd_base = pdpt_entry & 0x000f_ffff_ffff_f000;
    let pd_i = (virt >> 21) & 0x1ff;
    let pd_entry = unsafe { *(pd_base as *const u64).add(pd_i as usize) };
    crate::serial_println!("PD[{:#x}] = {:#018x}", pd_i, pd_entry);
    if pd_entry & 1 == 0 {
        crate::serial_println!("  -> NOT PRESENT");
        return;
    }
    crate::serial_println!(
        "  -> present={} user={} writable={} nx={}",
        (pd_entry >> 0) & 1,
        (pd_entry >> 2) & 1,
        (pd_entry >> 1) & 1,
        (pd_entry >> 63) & 1
    );
    if pd_entry & (1 << 7) != 0 {
        crate::serial_println!("  -> 2M page (huge)");
        return;
    }
    let pt_base = pd_entry & 0x000f_ffff_ffff_f000;
    let pt_i = (virt >> 12) & 0x1ff;
    let pt_entry = unsafe { *(pt_base as *const u64).add(pt_i as usize) };
    crate::serial_println!("PT[{:#x}] = {:#018x}", pt_i, pt_entry);
    if pt_entry & 1 == 0 {
        crate::serial_println!("  -> NOT PRESENT");
        return;
    }
    crate::serial_println!(
        "  -> present={} user={} writable={} nx={}",
        (pt_entry >> 0) & 1,
        (pt_entry >> 2) & 1,
        (pt_entry >> 1) & 1,
        (pt_entry >> 63) & 1
    );
}

fn exception_name(vector: u64) -> &'static str {
    match vector {
        0 => "DIVIDE ERROR",
        1 => "DEBUG EXCEPTION",
        2 => "NMI",
        3 => "BREAKPOINT",
        4 => "OVERFLOW",
        5 => "BOUND RANGE EXCEEDED",
        6 => "INVALID OPCODE",
        7 => "DEVICE NOT AVAILABLE",
        8 => "DOUBLE FAULT",
        10 => "INVALID TSS",
        11 => "SEGMENT NOT PRESENT",
        12 => "STACK SEGMENT FAULT",
        13 => "GENERAL PROTECTION FAULT",
        14 => "PAGE FAULT",
        16 => "X87 FLOATING POINT EXCEPTION",
        17 => "ALIGNMENT CHECK",
        18 => "MACHINE CHECK",
        19 => "SIMD FLOATING POINT EXCEPTION",
        20 => "VIRTUALIZATION EXCEPTION",
        21 => "CONTROL PROTECTION EXCEPTION",
        _ => "CPU EXCEPTION",
    }
}

fn read_cr2() -> u64 {
    let cr2: u64;
    unsafe {
        core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack));
    }
    cr2
}

macro_rules! no_error_stub {
    ($name:ident, $vector:literal) => {
        global_asm!(concat!(
            ".global ",
            stringify!($name),
            "\n",
            stringify!($name),
            ":\n",
            "    push 0\n",
            "    push rax\n",
            "    push rbx\n",
            "    push rcx\n",
            "    push rdx\n",
            "    push rbp\n",
            "    push rsi\n",
            "    push rdi\n",
            "    push r8\n",
            "    push r9\n",
            "    push r10\n",
            "    push r11\n",
            "    push r12\n",
            "    push r13\n",
            "    push r14\n",
            "    push r15\n",
            "    mov rdi, ",
            stringify!($vector),
            "\n",
            "    mov rsi, [rsp + 120]\n",
            "    mov rdx, rsp\n",
            "    lea rcx, [rsp + 128]\n",
            "    call fault_handler_inner\n"
        ));
    };
}

macro_rules! error_stub {
    ($name:ident, $vector:literal) => {
        global_asm!(concat!(
            ".global ",
            stringify!($name),
            "\n",
            stringify!($name),
            ":\n",
            "    push rax\n",
            "    push rbx\n",
            "    push rcx\n",
            "    push rdx\n",
            "    push rbp\n",
            "    push rsi\n",
            "    push rdi\n",
            "    push r8\n",
            "    push r9\n",
            "    push r10\n",
            "    push r11\n",
            "    push r12\n",
            "    push r13\n",
            "    push r14\n",
            "    push r15\n",
            "    mov rdi, ",
            stringify!($vector),
            "\n",
            "    mov rsi, [rsp + 120]\n",
            "    mov rdx, rsp\n",
            "    lea rcx, [rsp + 128]\n",
            "    call fault_handler_inner\n"
        ));
    };
}

no_error_stub!(fault0_asm, 0);
no_error_stub!(fault1_asm, 1);
no_error_stub!(fault2_asm, 2);
no_error_stub!(fault3_asm, 3);
no_error_stub!(fault4_asm, 4);
no_error_stub!(fault5_asm, 5);
no_error_stub!(fault6_asm, 6);
no_error_stub!(fault7_asm, 7);
error_stub!(fault8_asm, 8);
error_stub!(fault10_asm, 10);
error_stub!(fault11_asm, 11);
error_stub!(fault12_asm, 12);
error_stub!(fault13_asm, 13);
error_stub!(fault14_asm, 14);
no_error_stub!(fault16_asm, 16);
error_stub!(fault17_asm, 17);
no_error_stub!(fault18_asm, 18);
no_error_stub!(fault19_asm, 19);
no_error_stub!(fault20_asm, 20);
error_stub!(fault21_asm, 21);

unsafe extern "C" {
    fn fault0_asm();
    fn fault1_asm();
    fn fault2_asm();
    fn fault3_asm();
    fn fault4_asm();
    fn fault5_asm();
    fn fault6_asm();
    fn fault7_asm();
    fn fault8_asm();
    fn fault10_asm();
    fn fault11_asm();
    fn fault12_asm();
    fn fault13_asm();
    fn fault14_asm();
    fn fault16_asm();
    fn fault17_asm();
    fn fault18_asm();
    fn fault19_asm();
    fn fault20_asm();
    fn fault21_asm();
}

pub fn handler_addr(vector: u8) -> Option<u64> {
    let handler = match vector {
        0 => fault0_asm,
        1 => fault1_asm,
        2 => fault2_asm,
        3 => fault3_asm,
        4 => fault4_asm,
        5 => fault5_asm,
        6 => fault6_asm,
        7 => fault7_asm,
        8 => fault8_asm,
        10 => fault10_asm,
        11 => fault11_asm,
        12 => fault12_asm,
        13 => fault13_asm,
        14 => fault14_asm,
        16 => fault16_asm,
        17 => fault17_asm,
        18 => fault18_asm,
        19 => fault19_asm,
        20 => fault20_asm,
        21 => fault21_asm,
        _ => return None,
    };
    Some(handler as *const () as usize as u64)
}
