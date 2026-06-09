# KazuOS Architecture

## Overview

KazuOS is currently a small monolithic-style x86_64 hobby OS. Most core services live in kernel space: boot initialization, memory management, interrupt handling, shell, early process tracking, syscalls, and drivers.

The long-term direction can still move toward a hybrid architecture by introducing clearer boundaries between the kernel core, VFS, process manager, memory manager, and drivers.

## Boot Flow

1. UEFI bootloader starts.
2. Bootloader loads `kernel.elf` and `font.ttf`.
3. Bootloader collects framebuffer, memory map, RSDP, heap, and command line into `BootInfo`.
4. Bootloader exits UEFI boot services.
5. Kernel `_start` sets the kernel stack and enters `kernel_main`.
6. Kernel initializes GDT, syscalls, console, PMM, VMM, IDT, ACPI, interrupts, VFS/initramfs, then starts the shell.

## Current Kernel Components

### Bootloader

Located in `crates/bootloader`.

Responsibilities:

- Load kernel ELF from the EFI filesystem
- Load font data
- Gather UEFI framebuffer info
- Gather memory map
- Find ACPI RSDP
- Pass `BootInfo` to the kernel

### Kernel Entry

Located in `crates/kernel/src/main.rs`.

Responsibilities:

- Set up initial stack
- Initialize GDT
- Initialize syscall layer
- Run kernel initialization
- Initialize VFS/initramfs
- Start shell

### Initialization

Located in `crates/kernel/src/init.rs`.

Responsibilities:

- Serial init
- Console init
- Heap allocator init
- PMM init
- VMM init
- IDT init
- ACPI / APIC / interrupt init
- TSC calibration

### Memory Management

#### PMM

Located in `crates/kernel/src/pmm.rs`.

Physical Memory Manager tracks frames with a bitmap.

Current features:

- Mark usable/reserved memory from UEFI memory map
- Allocate/free 4 KiB frames
- Allocate/free contiguous frames
- Return memory stats

Limitations:

- No per-process memory accounting yet
- No page cache
- No swapping

#### VMM

Located in `crates/kernel/src/vmm.rs`.

Virtual Memory Manager maps pages into the current page table.

Current features:

- Basic page mapping
- Basic unmapping
- Virtual-to-physical translation
- User demo fixed mapping

Limitations:

- No process-specific address spaces yet
- No demand paging
- No mature page fault recovery

### Interrupts and Exceptions

Located in:

- `crates/kernel/src/idt.rs`
- `crates/kernel/src/handlers/faults.rs`
- `crates/kernel/src/handlers/interrupts.rs`

Current features:

- IDT setup
- Page fault handler
- Double fault handler
- Timer handler
- Keyboard handler
- `int 0x80` syscall entry

### Syscalls

Located in `crates/kernel/src/syscall.rs` and `crates/kernel/src/user.rs`.

Current syscall mechanism uses `int 0x80` from ring 3. The IDT entry is a **trap gate** (type `0xEF`, DPL=3), which preserves IF so timer interrupts can fire during syscall execution.

#### `int 0x80` frame layout (`syscall_int80_asm`)

```
(higher address — CPU frame pushed by hardware on ring3→ring0)
  [rsp+160] ss
  [rsp+152] user_rsp
  [rsp+144] rflags
  [rsp+136] cs
  [rsp+128] rip
(asm-pushed registers, in push order)
  [rsp+120] rbp    ← push rbp (first)
  [rsp+112] rbx
  [rsp+104] rcx
  [rsp+96]  rdx
  [rsp+88]  rsi
  [rsp+80]  rdi
  [rsp+72]  r8
  [rsp+64]  r9
  [rsp+56]  r10
  [rsp+48]  r11
  [rsp+40]  r12
  [rsp+32]  r13
  [rsp+24]  r14
  [rsp+16]  r15    ← push r15 (last)
  [rsp+8]   ← 8-byte alignment pad (sub rsp, 8)
  [rsp+0]   ← rsp at call syscall_handler
```

The 8-byte pad after the 14 register pushes corrects the SysV 16-byte call alignment (14×8 + 5×8 = 152 bytes, which is 8 mod 16 without the pad).

When a syscall blocks (`BLOCK_TO_SCHEDULER`), `blocking_rsp` is set to `rsp+8` (the r15 slot), skipping the alignment pad. The blocking resume path in `enter_next_process` pops r15..rbp then `iretq` directly from that pointer.

Current syscall IDs:

- `1`: console write
- `2`: console clear
- `3`: exit process (`SYS_EXIT`)
- `4`: memory info
- `5`: CPU info
- `6`: process info
- `7`: VFS `ls`
- `8`: file-backed `exec`
- `9`: keyboard read (blocking)
- `16`: framebuffer acquire
- `17`: framebuffer release

Limitations:

- No userspace pointer validation yet
- No syscall table abstraction yet

### User Mode

Located in `crates/kernel/src/user.rs` and `crates/kernel/src/exec.rs`.

Current status:

- Full ring-3 user processes via KXE binary format
- `int 0x80` trap gate with SysV-compatible register save/restore
- Preemptive scheduler with per-process kernel stacks and TSS RSP0 updates
- Blocking syscalls (`SYS_KEYBOARD_READ`) suspend the process and resume on wakeup
- `SYS_EXIT` terminates the process and returns to the scheduler
- `SYS_EXEC` loads a KXE binary from VFS and spawns a new process

### Process Tracking and Scheduler

Located in `crates/kernel/src/task/process.rs` and `crates/kernel/src/task/scheduler.rs`.

Current status:

- Dynamic process table with per-process kernel stacks (64 KiB each)
- `pid=0` is kernel; user processes start at pid=1
- Preemptive round-robin scheduler driven by LAPIC timer
- Timer can preempt both ring-3 (user) and ring-0 (kernel mid-syscall) contexts
- Per-process `user_context` saves all GP registers + rip/rsp/rflags/cr3
- `blocking_rsp` saves the full `int 0x80` register frame for direct blocking resume
- `kernel_preempted` flag distinguishes mid-syscall preemption from user-mode preemption

Three resume paths in `enter_next_process`:
1. **blocking_rsp set**: process was blocked in a syscall — restore directly from `int 0x80` frame (14 pop + iretq)
2. **kernel_preempted set**: process was preempted during a syscall — resume via saved kernel stack (timer frame pops + iretq)
3. **user_context only**: process was preempted in user mode — rebuild frame on `USER_RETURN_STACK` and iretq

### Shell

Located in `crates/kernel/src/user_programs/shell.rs` (user-space KXE binary).

The shell runs entirely in ring 3 and communicates with the kernel via `int 0x80` syscalls.

Current commands:

- `help`
- `clear`
- `ls [path]`
- `mem`
- `ps` (spawns `/bin/ps.kxe` via `SYS_EXEC`)
- `sysinfo`
- `exec <path>`
- `shutdown`
- `reboot`

Planned commands:

- `cat`
- background execution with `&`

### Drivers

Located in `crates/kernel/src/drivers`.

Current drivers/support code:

- Serial
- Framebuffer
- Keyboard
- PIT/LAPIC/IOAPIC/PIC pieces
- ACPI parsing
- Power shutdown/reboot
- PC speaker beep

## Long-Term Architecture Direction

Current architecture is monolithic-style. A future hybrid direction could split responsibilities like this:

- Kernel core: scheduler, low-level memory, interrupts, syscall dispatch
- Process manager: process lifecycle, exec, wait, signals/events
- VFS layer: path resolution and filesystem abstraction
- Filesystem services: ramfs, disk FS, procfs
- Driver boundary: device abstraction, later some drivers could become userspace services

## Roadmap

- [x] VFS core (initramfs-backed, path lookup, `ls`)
- [x] shell `ls [path]` via `SYS_LS`
- [x] preemptive scheduler with per-process kernel stacks
- [x] blocking syscalls (`SYS_KEYBOARD_READ`)
- [ ] shell `cat`
- [ ] ramfs / procfs (`/proc/meminfo`, `/proc/ps`)
- [ ] stronger syscall userspace pointer validation
- [ ] background shell jobs with `&`
- [ ] signals / kill
- [ ] userspace allocator / mmap-like API
