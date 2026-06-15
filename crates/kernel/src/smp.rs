use crate::drivers::{acpi, lapic};
use crate::util::SyncUnsafeCell;
use alloc::vec::Vec;
use core::arch::global_asm;

pub const MAX_CPUS: usize = 16;

const TRAMPOLINE_PHYS: u64 = 0x8000;

pub struct CpuInfo {
    pub apic_id: u8,
    pub cpu_index: u8,
    pub is_bsp: bool,
}

static CPU_COUNT: SyncUnsafeCell<usize> = SyncUnsafeCell::new(1);
static BSP_APIC_ID: SyncUnsafeCell<u8> = SyncUnsafeCell::new(0);
static APIC_IDS: SyncUnsafeCell<Vec<u8>> = SyncUnsafeCell::new(Vec::new());

#[repr(C, align(4096))]
struct ApStack([u8; 32768]);

static mut AP_STACKS: [ApStack; MAX_CPUS - 1] = [const { ApStack([0; 32768]) }; MAX_CPUS - 1];

pub fn cpu_count() -> usize {
    unsafe { *CPU_COUNT.0.get() }
}

pub fn bsp_apic_id() -> u8 {
    unsafe { *BSP_APIC_ID.0.get() }
}

pub fn apic_id_to_cpu_index(apic_id: u8) -> Option<usize> {
    unsafe {
        (*APIC_IDS.0.get())
            .iter()
            .position(|&id| id == apic_id)
    }
}

#[repr(C, align(64))]
pub struct CpuData {
    pub cpu_index: usize,
    pub apic_id: u8,
    pub is_bsp: bool,
    pub current_tid: core::sync::atomic::AtomicU64,
    pub idle: core::sync::atomic::AtomicBool,
}

static CPU_DATA: SyncUnsafeCell<[CpuData; MAX_CPUS]> = SyncUnsafeCell::new(
    [const {
        CpuData {
            cpu_index: 0,
            apic_id: 0,
            is_bsp: false,
            current_tid: core::sync::atomic::AtomicU64::new(0),
            idle: core::sync::atomic::AtomicBool::new(false),
        }
    }; MAX_CPUS]
);

pub fn cpu_data(cpu_index: usize) -> &'static CpuData {
    unsafe { &(*CPU_DATA.0.get())[cpu_index] }
}

pub fn current_cpu_data() -> &'static CpuData {
    cpu_data(current_cpu_index())
}

pub fn current_cpu_index() -> usize {
    let apic_id = lapic::local_apic_id();
    apic_id_to_cpu_index(apic_id).unwrap_or(0)
}

pub fn cpu_kernel_stack_top(cpu_index: usize) -> Option<u64> {
    if cpu_index == 0 || cpu_index >= MAX_CPUS {
        return None;
    }
    unsafe {
        Some(core::ptr::addr_of!(AP_STACKS[cpu_index - 1]) as u64 + 32768)
    }
}

pub fn apic_id_for_cpu_index(index: usize) -> Option<u8> {
    unsafe {
        (&(*APIC_IDS.0.get())).get(index).copied()
    }
}

global_asm!(
    r#"
    .code16
    .global ap_trampoline_start
    .global ap_trampoline_end
    .global ap_gdt
    .global ap_gdt_desc
    .global ap_jump16
    .global ap_jump32
    .global ap_protected
    .global ap_long
    .global ap_pml4
    .global ap_stack
    .global ap_started
    .global ap_in_protected
    .global ap_in_long
    .global ap_done
    .align 4096
ap_trampoline_start:
    jmp ap_code_start

    # Offset table filled in by BSP before startup. Fixed offsets from 0x8000.
    # 0x02 ap_started_off
    # 0x04 ap_gdt_desc_off
    # 0x06 ap_jump16_off
    # 0x08 ap_in_protected_off
    # 0x0A ap_pml4_off
    # 0x0C ap_jump32_off
    # 0x0E ap_in_long_off
    # 0x10 ap_stack_off
    # 0x12 ap_done_off
ap_started_off:     .word 0
ap_gdt_desc_off:    .word 0
ap_jump16_off:      .word 0
ap_in_protected_off:.word 0
ap_pml4_off:        .word 0
ap_jump32_off:      .word 0
ap_in_long_off:     .word 0
ap_stack_off:       .word 0
ap_done_off:        .word 0

ap_code_start:
    cli
    cld
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov sp, 0x7000

    # Early progress flag: AP reached trampoline (use 0x8000 copy)
    mov bx, 0x8000
    add bx, [0x8002]
    mov word ptr [bx], 1

    # Enable A20 fast gate
    in al, 0x92
    or al, 2
    out 0x92, al

    # Load GDT. Compute runtime address (0x8000 + offset) in bx.
    mov bx, 0x8000
    add bx, [0x8004]
    lgdt [bx]

    # Protected mode
    mov eax, cr0
    or al, 1
    mov cr0, eax

    # Far jump to 32-bit code (use 32-bit operand-size override)
    mov bx, 0x8000
    add bx, [0x8006]
    .byte 0x66
    jmp fword ptr [bx]

    .code32
ap_protected:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    # Progress flag: AP in protected mode
    mov eax, 0x8000
    add ax, [0x8008]
    mov dword ptr [eax], 1

    # Enable PAE
    mov eax, cr4
    or eax, 0x20
    mov cr4, eax

    # Load PML4 from 0x8000 copy
    mov eax, 0x8000
    add ax, [0x800A]
    mov eax, [eax]
    mov cr3, eax

    # Enable long mode
    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x100
    wrmsr

    # Enable paging
    mov eax, cr0
    or eax, 0x80000001
    mov cr0, eax

    # Far jump to long mode (use 0x8000 copy of descriptor)
    mov eax, 0x8000
    add ax, [0x800C]
    jmp fword ptr [eax]

    .code64
ap_long:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    # Progress flag: AP in long mode
    mov rax, 0x8000
    add ax, [0x800E]
    mov qword ptr [rax], 1

    # Load stack from 0x8000 copy
    mov rax, 0x8000
    add ax, [0x8010]
    mov rsp, [rax]

    # Set done flag
    mov rax, 0x8000
    add ax, [0x8012]
    mov qword ptr [rax], 1

    # Call Rust AP main
    movabs rax, offset ap_main
    call rax

    cli
1:
    hlt
    jmp 1b

    .align 16
ap_gdt:
    .quad 0
    .quad 0x00CF9A000000FFFF   # 0x08: 32-bit code (D=1, L=0)
    .quad 0x00CF92000000FFFF   # 0x10: data
    .quad 0x00AF9A000000FFFF   # 0x18: 64-bit code (D=0, L=1)
ap_gdt_desc:
    .word 0
    .long 0

    .align 8
ap_jump16:
    .long 0
    .word 0

ap_jump32:
    .long 0
    .word 0

ap_pml4: .quad 0
ap_stack: .quad 0
ap_started: .word 0
ap_in_protected: .quad 0
ap_in_long: .quad 0
ap_done: .quad 0
ap_trampoline_end:
    "#
);

unsafe extern "C" {
    fn ap_trampoline_start();
    fn ap_trampoline_end();
    static ap_gdt: u64;
    static ap_gdt_desc: u16;
    static ap_jump16: u64;
    static ap_jump32: u64;
    static ap_protected: u8;
    static ap_long: u8;
    static ap_pml4: u64;
    static ap_stack: u64;
    static ap_started: u16;
    static ap_in_protected: u64;
    static ap_in_long: u64;
    static ap_done: u64;
}

pub unsafe fn detect_cpus(rsdp: u64) {
    crate::vserial_println!("SMP: detect_cpus called, rsdp={:#x}", rsdp);
    unsafe {
        if rsdp == 0 {
            crate::vserial_println!("SMP: no RSDP, assuming single core");
            *CPU_COUNT.0.get() = 1;
            *BSP_APIC_ID.0.get() = 0;
            (*APIC_IDS.0.get()).clear();
            (*APIC_IDS.0.get()).push(0);
            return;
        }

        let parsed = acpi::parse_rsdp(rsdp);
        if let Some((sdt_phys, is_xsdt)) = parsed {
            if let Some(madt_phys) = acpi::find_madt(sdt_phys, is_xsdt) {
                let ids = acpi::parse_cpu_lapic_ids(madt_phys);
                *CPU_COUNT.0.get() = ids.len().max(1);
                (*APIC_IDS.0.get()).clear();
                (*APIC_IDS.0.get()).extend_from_slice(&ids);
                if let Some(&bsp) = ids.first() {
                    *BSP_APIC_ID.0.get() = bsp;
                }

                let cpu_data = &mut *CPU_DATA.0.get();
                for (i, &id) in ids.iter().enumerate() {
                    cpu_data[i].cpu_index = i;
                    cpu_data[i].apic_id = id;
                    cpu_data[i].is_bsp = id == *BSP_APIC_ID.0.get();
                    cpu_data[i].current_tid.store(0, core::sync::atomic::Ordering::Relaxed);
                    cpu_data[i].idle.store(id != *BSP_APIC_ID.0.get(), core::sync::atomic::Ordering::Relaxed);
                }

                crate::vserial_println!("SMP: detected {} CPU(s), BSP apic_id={}", ids.len(), *BSP_APIC_ID.0.get());
                for (i, &id) in ids.iter().enumerate() {
                    crate::vserial_println!("SMP: CPU[{}] apic_id={}", i, id);
                }
            } else {
                crate::vserial_println!("SMP: MADT not found, assuming single core");
                (*APIC_IDS.0.get()).clear();
                (*APIC_IDS.0.get()).push(0);
            }
        } else {
            crate::vserial_println!("SMP: RSDP invalid, assuming single core");
            (*APIC_IDS.0.get()).clear();
            (*APIC_IDS.0.get()).push(0);
        }
    }
}

pub unsafe fn start_aps() {
    unsafe {
        let ids = &*APIC_IDS.0.get();
        if ids.len() > 1 {
            start_aps_inner(ids);
        }
    }
}

unsafe fn start_aps_inner(ids: &[u8]) {
    unsafe {
        crate::pmm::mark_used(TRAMPOLINE_PHYS, 4096);
        copy_trampoline();

        let bsp_id = lapic::local_apic_id();
        let pml4 = crate::vmm::kernel_cr3();
        let kernel_pml4_addr = pml4 & 0x000F_FFFF_FFFF_F000;

        for (i, &apic_id) in ids.iter().enumerate() {
            if apic_id == bsp_id {
                continue;
            }
            let ap_index = i - 1; // BSP is index 0
            if ap_index >= MAX_CPUS - 1 {
                crate::vserial_println!("SMP: too many CPUs, skipping apic_id={}", apic_id);
                continue;
            }
            let stack_top = core::ptr::addr_of!(AP_STACKS[ap_index]) as u64 + 32768;
            start_ap(apic_id, kernel_pml4_addr, stack_top);
        }
    }
}

unsafe fn copy_trampoline() {
    unsafe {
        let src = ap_trampoline_start as *const u8;
        let dst = TRAMPOLINE_PHYS as *mut u8;
        let size = ap_trampoline_end as *const () as usize - ap_trampoline_start as *const () as usize;
        core::ptr::copy_nonoverlapping(src, dst, size);
    }
}

unsafe fn start_ap(apic_id: u8, pml4: u64, stack_top: u64) {
    unsafe {
        // Patch trampoline data and descriptors
        let base = TRAMPOLINE_PHYS as *mut u8;
        let offset = |sym: *const ()| -> usize {
            sym as usize - ap_trampoline_start as *const () as usize
        };

        let gdt_offset = offset(&ap_gdt as *const u64 as *const ());
        let gdt_desc_offset = offset(&ap_gdt_desc as *const u16 as *const ());
        let gdt_limit = gdt_desc_offset - gdt_offset - 1;
        let gdt_base = (TRAMPOLINE_PHYS as usize + gdt_offset) as u32;
        (base.add(gdt_desc_offset) as *mut u16).write_volatile(gdt_limit as u16);
        (base.add(gdt_desc_offset + 2) as *mut u32).write_volatile(gdt_base);

        let protected_offset = offset(&ap_protected as *const u8 as *const ());
        let long_offset = offset(&ap_long as *const u8 as *const ());
        let jump16_offset = offset(&ap_jump16 as *const u64 as *const ());
        let jump32_offset = offset(&ap_jump32 as *const u64 as *const ());
        (base.add(jump16_offset) as *mut u32).write_volatile((TRAMPOLINE_PHYS as usize + protected_offset) as u32);
        (base.add(jump16_offset + 4) as *mut u16).write_volatile(0x08);
        (base.add(jump32_offset) as *mut u32).write_volatile((TRAMPOLINE_PHYS as usize + long_offset) as u32);
        (base.add(jump32_offset + 4) as *mut u16).write_volatile(0x18);

        let pml4_offset = offset(&ap_pml4 as *const u64 as *const ());
        let stack_offset = offset(&ap_stack as *const u64 as *const ());
        let started_offset = offset(&ap_started as *const u16 as *const ());
        let in_protected_offset = offset(&ap_in_protected as *const u64 as *const ());
        let in_long_offset = offset(&ap_in_long as *const u64 as *const ());
        let done_offset = offset(&ap_done as *const u64 as *const ());

        // Fill in offset table at fixed offsets from TRAMPOLINE_PHYS.
        let write_off = |idx: usize, off: usize| {
            (base.add(2 + idx * 2) as *mut u16).write_volatile(off as u16);
        };
        write_off(0, started_offset);
        write_off(1, gdt_desc_offset);
        write_off(2, jump16_offset);
        write_off(3, in_protected_offset);
        write_off(4, pml4_offset);
        write_off(5, jump32_offset);
        write_off(6, in_long_offset);
        write_off(7, stack_offset);
        write_off(8, done_offset);

        (base.add(pml4_offset) as *mut u64).write_volatile(pml4);
        (base.add(stack_offset) as *mut u64).write_volatile(stack_top);
        (base.add(started_offset) as *mut u16).write_volatile(0);
        (base.add(in_protected_offset) as *mut u64).write_volatile(0);
        (base.add(in_long_offset) as *mut u64).write_volatile(0);
        (base.add(done_offset) as *mut u64).write_volatile(0);

        crate::vserial_println!("SMP: starting AP apic_id={}", apic_id);

        // INIT (level-triggered)
        lapic::send_ipi(apic_id, 0, 0x0000C500);
        crate::util::wait_ms(10, crate::user::TSC_PER_MS);

        // SIPI
        let vector = (TRAMPOLINE_PHYS >> 12) as u8;
        lapic::send_ipi(apic_id, vector, 0x00004600);
        crate::util::wait_ms(2, crate::user::TSC_PER_MS);

        // Second SIPI
        lapic::send_ipi(apic_id, vector, 0x00004600);

        // Wait for AP to signal ready (with timeout)
        let deadline = crate::util::rdtsc() + crate::user::TSC_PER_MS * 100;
        while crate::util::rdtsc() < deadline {
            if base.add(done_offset).read_volatile() != 0 {
                crate::vserial_println!("SMP: AP apic_id={} started", apic_id);
                return;
            }
            crate::util::pause();
        }
        crate::vserial_println!("SMP: AP apic_id={} start timeout", apic_id);
    }
}

#[unsafe(no_mangle)]
extern "C" fn ap_main() -> ! {
    unsafe {
        // Match BSP's CR0/CR4 settings: no cache disable/write-through, enable SSE.
        core::arch::asm!(
            "mov rax, cr0",
            "and rax, {cr0_mask}",
            "mov cr0, rax",
            "mov rax, cr4",
            "or rax, {cr4_bits}",
            "mov cr4, rax",
            cr0_mask = const !(0x6000_0000u64),
            cr4_bits = const (1u64 << 9) | (1u64 << 10),
            options(nostack, preserves_flags),
        );
        // Ensure LAPIC base MSR has the global enable bit set and the same base as BSP.
        core::arch::asm!(
            "mov ecx, {msr}",
            "rdmsr",
            "or eax, {enable_mask}",
            "and eax, {base_mask}",
            "wrmsr",
            msr = const 0x1Bu32,
            enable_mask = const 0x800u32,
            base_mask = const 0xFFFFF000u32,
            options(nostack, preserves_flags),
        );
        lapic::init();
        lapic::enable();
        // Enable EFER.NXE so user no-execute mappings work on this CPU.
        crate::vmm::init();
    }

    let apic_id = lapic::local_apic_id();
    let cpu_index = apic_id_to_cpu_index(apic_id).unwrap_or(0);
    let data = cpu_data(cpu_index);
    data.current_tid.store(0, core::sync::atomic::Ordering::Relaxed);
    data.idle.store(true, core::sync::atomic::Ordering::Relaxed);

    unsafe {
        crate::gdt::load_for_cpu(cpu_index);
        if let Some(top) = cpu_kernel_stack_top(cpu_index) {
            crate::gdt::set_kernel_stack_top_for_cpu(top, cpu_index);
        }
        crate::idt::load_idt();
        lapic::set_timer(0x30, 0x20000);
    }

    crate::vserial_println!("SMP: AP apic_id={} cpu_index={} ready", apic_id, cpu_index);

    // Install this AP's scheduler restart stack (fixed top of its kernel stack)
    // before entering the scheduler, so blocking syscalls on threads pinned to
    // this CPU always unwind to a valid stack regardless of which entry path
    // first ran the thread.
    if let Some(top) = cpu_kernel_stack_top(cpu_index) {
        crate::user::set_kernel_return_stack(top);
    }

    crate::scheduler::enter_next_process();
}
