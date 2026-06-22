# KazuOS Architecture

## Overview

KazuOS is currently a small monolithic-style x86_64 hobby OS. Core services live in kernel space: boot initialization, memory management, interrupt handling, processes/threads/scheduling, syscalls, VFS, and IPC. The shell and apps run in ring3, and device drivers are increasingly ring3 `.kkm` modules (see Driver Policy below).

The long-term direction can still move toward a hybrid architecture by introducing clearer boundaries between the kernel core, VFS, process manager, memory manager, and drivers.

## Boot Flow

1. UEFI bootloader starts.
2. Bootloader loads `kernel.elf` and `font.ttf`.
3. Bootloader collects framebuffer, memory map, RSDP, heap, and command line into `BootInfo`.
4. Bootloader exits UEFI boot services.
5. Kernel `_start` sets the kernel stack and enters `kernel_main`.
6. Kernel initializes GDT, syscalls, console, PMM, VMM, IDT, ACPI, interrupts, SMP (APs), VFS/initramfs, then spawns the initial user process, which brings up the shell.

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
- Spawn the initial user process (which starts the shell)

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

Located in `crates/kernel/src/memory/pmm.rs` (re-exported via `crate::pmm`).

Physical Memory Manager tracks frames with a bitmap.

Current features:

- Mark usable/reserved memory from UEFI memory map
- Allocate/free 4 KiB frames
- Allocate/free contiguous frames
- Return memory stats
- Per-process memory accounting (`ProcessInfo.memory_bytes`)

Limitations:

- No page cache
- No swapping

#### VMM

Located in `crates/kernel/src/memory/vmm.rs` (re-exported via `crate::vmm`).

Virtual Memory Manager maps pages into per-process page tables.

Current features:

- Page mapping / unmapping
- Virtual-to-physical translation
- Per-process address spaces (`create_address_space`, `free_user_address_space`) with CR3 switching
- User heap and DMA mappings

Limitations:

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

The full syscall list and ABI live in `crates/kazuos_abi/src/syscall_numbers.rs` (source of
truth for numbers) and `docs/USER_ABI.md` (human-readable). Do not duplicate the numeric
table here — it drifts.

Limitations:

- No userspace pointer validation yet
- No syscall table abstraction yet

### User Mode

Located in `crates/kernel/src/user.rs` and `crates/kernel/src/exec.rs`.

Current status:

- Full ring-3 user processes via KXE binary format
- `int 0x80` trap gate with SysV-compatible register save/restore
- Preemptive SMP scheduler with per-thread kernel stacks and TSS RSP0 updates
- User-space threads (`SYS_THREAD_SPAWN`/`EXIT`/`JOIN`) sharing one address space
- Blocking syscalls (e.g. a console `SYS_READ`) suspend the calling thread and resume on wakeup
- `SYS_EXIT` terminates the process and returns to the scheduler
- `SYS_EXEC` loads a KXE binary from VFS and spawns a new process

### Process Tracking and Scheduler

Located in `crates/kernel/src/task/process.rs`, `crates/kernel/src/task/thread.rs`, and
`crates/kernel/src/task/scheduler.rs`.

Current status:

- The scheduling unit is the **thread**; a process may own several threads in one address
  space. Each thread has its own 64 KiB kernel stack and `user_context`.
- Dynamic process table; `pid=0` is kernel, user processes start at pid=1
- Preemptive per-CPU round-robin scheduler driven by each CPU's LAPIC timer (no priority —
  strict priority was tried and removed because it starved threads)
- Timer can preempt both ring-3 (user) and ring-0 (kernel mid-syscall) contexts
- Per-thread `user_context` saves all GP registers + rip/rsp/rflags/cr3
- `blocking_rsp` saves the full `int 0x80` register frame for direct blocking resume
- `kernel_preempted` flag distinguishes mid-syscall preemption from user-mode preemption

Three resume paths in `enter_next_process`:
1. **blocking_rsp set**: process was blocked in a syscall — restore directly from `int 0x80` frame (14 pop + iretq)
2. **kernel_preempted set**: process was preempted during a syscall — resume via saved kernel stack (timer frame pops + iretq)
3. **user_context only**: process was preempted in user mode — rebuild frame on `USER_RETURN_STACK` and iretq

### Shell

Located in `crates/user_programs/shell.rs` (a ring3 user-space KXE binary — **not** a kernel
module). It communicates with the kernel only via `int 0x80` syscalls.

Built-in commands include `help`, `clear`, `ls`, `cat`, `mem`, `ps`, `sysinfo`, `smpinfo`,
`exec`, the filesystem mutations (`touch`/`rm`/`mkdir`/`rmdir`), `shutdown`, and `reboot`,
plus `cmd1 | cmd2` pipelines and `&` background jobs. Other programs in
`crates/user_programs/` (`ps`, `ktop`, `cpuburner`, `gui`, …) are launched by name.

Planned:

- richer job control

### SMP

Located in `crates/kernel/src/smp.rs`.

Current status:

- MADT parsing enumerates local APIC IDs and detects CPU count
- BSP sends INIT-SIPI-SIPI to start APs via the local APIC ICR
- 16→32→64-bit trampoline copied to physical `0x8000` prepares each AP
- Each AP sets up its own LAPIC, per-CPU GDT, TSS/RSP0, and IDT, starts its own
  LAPIC timer, and enters the scheduler (`enter_next_process`) — every CPU runs
  the scheduler, not just the BSP
- Per-CPU `CpuData` array tracks `cpu_index`, `apic_id`, `current_tid`, and `idle` state
- Threads are assigned a home CPU round-robin at creation (`assigned_cpu`) and are
  pinned there; the scheduler only runs threads assigned to the current CPU
- A CPU with no runnable thread sets its idle flag and `hlt`s until its next timer tick
- `smpinfo` shell command reports CPU topology via `SYS_CPU_INFO`

Limitations:

- Thread→CPU assignment is round-robin at creation and fixed; there is no load
  balancing or thread migration between CPUs, so work can pile up unevenly
- No affinity API and no IPI-based cross-CPU rescheduling/wakeup
- AP fault/panic handling is minimal

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
- PCI / PCIe enumeration
- Intel HD Audio (HDA)
- e1000 (Intel 82540EM) network controller

### Networking

Located in `crates/kernel/src/net.rs`.

A minimal polling-mode IPv4 stack on top of the `e1000` driver: Ethernet, ARP,
IPv4, ICMP, UDP, a DHCP client, a DNS resolver, and an active-open TCP client.
On top of TCP it runs TLS via rustls (no_std) with the rustls-rustcrypto
provider; server certificates are verified against the webpki-roots trust
anchors using the CMOS RTC for the current time, and entropy comes from
`rng.rs` (RDRAND with a TSC fallback, also exposed as `/dev/random`).

It has no socket layer yet; each shell command runs a fixed sequence and is
driven synchronously by polling the NIC receive ring with TSC-based timeouts:

- `nettest [host]` (`SYS_NETTEST`) — DHCP, DNS/IPv4 literal, four ICMP echoes.
- `http [host]` (`SYS_HTTPGET`) — TCP connect to port 80 and an HTTP/1.1 GET.
- `https [host]` (`SYS_HTTPSGET`) — verified TLS GET on port 443.

Interrupt-driven RX/TX and a general socket API are future work.

## Driver Policy / Kernel Scope

The long-term direction is a **minimal kernel**: ring0 *arbitrates* hardware but
should not *operate* devices itself. The deciding question for "does this belong
in the kernel?" is **not** "is it important?" (everything matters to the user) but
**"does the kernel itself depend on it to run, boot, or report a failure?"**

**Stays in ring0 (genuinely core):** memory management (PMM/VMM), scheduler,
syscalls, IDT/GDT/TSS, interrupt controllers (LAPIC/IOAPIC/PIC), PIT, ACPI boot,
serial debug output, exec/process, IPC, VFS core — plus the plumbing that lets
ring3 drivers reach hardware: `SYS_IOPORT_REQUEST` (TSS I/O permission bitmap),
`SYS_DMA_ALLOC`, `SYS_IRQ_WAIT`, `SYS_PCI_BAR_MAP`. A **minimal framebuffer blit**
is also justified in ring0 so the kernel can show panic / early-boot diagnostics.

**Belongs in ring3:** device drivers. They run as kernel modules — `.kkm` files
built from `crates/user_modules/*.rs`, loaded via `SYS_MODULE_LOAD`, running as
ring3 processes at `PrivilegeLevel::Driver` (which only gates *which syscalls are
allowed*; all processes run ring3). The format, source contract, build pipeline,
and load/unload lifecycle are documented in `docs/MODULES.md`. `ps2mouse` is the reference example: it does
PS/2 `in`/`out` from ring3 after requesting the ports, and delivers events over
IPC. Rich console/terminal rendering (font shaping, scrollback) is also a ring3
concern, distinct from the minimal panic blit above.

**Rule of thumb for new drivers:** write them as ring3 `.kkm` modules, not as new
files under `crates/kernel/src/drivers/`. The existing in-kernel drivers (`hda`,
framebuffer rendering, `beep`, `pci` enumeration, `power`) predate this policy; they
work and need not be moved urgently, but they are candidates to migrate to ring3 over
time. Large protocol/library stacks (e.g. a TLS library) must never live in ring0 —
`panic = "abort"` means a driver panic in ring0 takes down the whole kernel.

**`keyboard` is deliberately kept in-kernel** and is *not* a migration candidate. It
is the recovery input path: a ring3 keyboard module that is accidentally unloaded
(`SYS_MODULE_UNLOAD`) or crashes would leave no way to type to recover — the system
is bricked. This is asymmetric with the mouse (losing the mouse is survivable via the
keyboard; losing the keyboard is not). Keyboard is also the stdin source for every
program (`FdEntry::ConsoleIn` reads it directly). Keeping it in ring0 is an
intentional, pragmatic exception with a concrete reason — not a violation of the
policy above.

