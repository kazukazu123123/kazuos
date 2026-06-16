# AGENTS.md

## Project

KazuOS is a small x86_64 hobby OS written in Rust.

Current architecture is monolithic-style, with a possible long-term hybrid direction. Keep boundaries clear so subsystems can be split later.

**Not stable:** Filesystem formats (KFS), KXE binary format, and syscall ABI are all experimental and subject to change.

## Required Commands

Use nightly for kernel checks.

```powershell
cargo +nightly check
```

For QEMU validation:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "auto_shell_qemu.ps1"
```

The pipeline accepts `-SendLines` (array of shell commands, default `ret`) and `-ExpectPattern` (regex to search serial.log, exits 1 on fail).

Use `-Verbose` to boot with serial output enabled (required for pattern matching on shell output).

Examples:
```powershell
# boot and press Enter
.\auto_shell_qemu.ps1

# boot verbose and run shell commands (Enter is appended automatically)
.\auto_shell_qemu.ps1 -Verbose -SendLines help,mem,ps

# check serial output for a specific pattern
.\auto_shell_qemu.ps1 -Verbose -SendLines help,ps -AfterWaitSeconds 6 -ExpectPattern "commands:"
```

Do not use plain `cargo check` for kernel validation because stable Rust fails on current nightly features.

## Code Style

- Keep code `no_std` compatible in kernel crates.
- Do not add comments unless explicitly requested.
- Do not use decorative divider/banner comments: rows of dashes, box-drawing characters, or ASCII art (e.g. `// ──── section ────` or `// ==== FOO ====`). Plain `//` line comments and `/* ... */` block comments are both fine.
- Prefer small modules with clear ownership.
- Keep unsafe blocks as small as practical.
- Prefer existing `crate::util::{inb,outb,inw,outw,ind,outd,pause,rdtsc}` helpers over inline asm duplicates.
- Use `SyncUnsafeCell` for mutable statics that must be shared globally.
- Avoid heap allocation in low-level handlers, interrupt paths, allocator paths, and panic/alloc handlers.
- Keep serial logging available for fault paths.

## Module Layout

### `main.rs`

Only kernel entry and top-level module declarations should live here.

Allowed:

- `_start`
- `kernel_main`
- `panic_handler`
- module declarations

Do not add subsystem logic here.

### `init.rs`

Boot-time kernel initialization.

Allowed:

- console init
- allocator init
- PMM/VMM init
- IDT init
- ACPI/APIC/interrupt init
- TSC calibration

Do not add runtime service logic here.

### `handlers/`

Exception, interrupt, panic-like, and allocator/fault handlers.

Current modules:

- `handlers/alloc.rs`
- `handlers/faults.rs`
- `handlers/interrupts.rs`

Do not allocate in handlers unless absolutely unavoidable.

### `drivers/`

Hardware-facing code.

Current modules include:

- `acpi.rs`
- `beep.rs`
- `framebuffer.rs`
- `ioapic.rs`
- `keyboard.rs`
- `lapic.rs`
- `pci.rs`
- `pic.rs`
- `pit.rs`
- `power.rs`
- `serial.rs`

Drivers should expose small safe wrappers when possible and keep port/MMIO/asm details inside the driver module.

### `pmm.rs`

Physical memory manager.

Responsibilities:

- frame bitmap
- frame allocation/free
- memory stats

Do not add virtual-memory logic here.

### `vmm.rs`

Virtual memory manager.

Responsibilities:

- page mapping/unmapping
- address translation
- user page mapping helpers until process address spaces exist

Future process page table management should be kept here or in a dedicated address-space module.

### `process.rs`

Process table and process metadata.

Current responsibilities:

- `pid=0` kernel process
- dynamic process table backed by heap allocation after allocator init
- PID allocation
- image name
- state tracking
- lightweight scheduling metadata
- per-process CPU tick accounting

Future responsibilities:

- parent/child
- exit code
- process address space
- scheduling metadata

### `syscall.rs`

Syscall entry path and dispatch trampoline.

Keep arch-specific syscall assembly here. High-level syscall behavior can dispatch to subsystem modules.

### `user.rs`

High-level syscall dispatch (core logic).

Syscall number constants live in the `kazuos-abi` crate. Keep real exec/loading code in `exec.rs`, `process.rs`, or VFS/filesystem modules.

### `kazuos-abi` crate

Single source of truth for all `SYS_*` constants — the kernel/user ABI.

Location: `crates/kazuos_abi/src/syscall_numbers.rs`. The kernel depends on it via Cargo (`user.rs` does `pub use kazuos_abi::*;`). Standalone-compiled code that can't use Cargo deps — the user-space runtimes (`crates/user_rt/*.rs`) and, transitively, all user programs/modules — pulls the same file in with `include!("../kazuos_abi/src/syscall_numbers.rs")`. Update this file when adding or renumbering syscalls, then update `docs/USER_ABI.md`.

### `shell.rs`

Interactive shell commands.

Shell should call subsystem APIs. Do not implement hardware/protocol logic directly in shell.

### `exec.rs`

User program loading and process address space creation.

Responsibilities:

- KXE binary parsing
- process address space setup (`create_process_space`)
- code/stack mapping for user processes
- `spawn()` for initramfs paths

### `user_programs.rs`

KXE format definitions and embedded user binaries.

Responsibilities:

- `KxeHeader` struct
- `INIT_KXE`, `STRESS_EXIT_KXE` minimal test binaries
- auto-generated `*_KXE` blobs (built from `crates/user_programs/*.rs` via `build.rs`)

The `build.rs` compiles all `.rs` files in `crates/user_programs/` (except `syscall_numbers.rs`) to ELF, parses `.rela.dyn` for `R_X86_64_RELATIVE` relocations, applies `USER_BASE` fixup, builds KXE blobs, and emits `user_programs_generated.rs` into `OUT_DIR`. It also builds `initrd.kfs` containing all binaries.

### `vfs.rs`

Current VFS responsibilities:

- initramfs image parsing
- path lookup
- file metadata
- read/readdir operations

Do not implement filesystem parsing in shell commands. Shell should call VFS/syscall APIs.

### Future filesystem modules

Suggested:

- `ramfs.rs`
- `procfs.rs`
- later disk-backed FS modules

## Architectural Direction

Preferred implementation order for major features:

1. VFS core
2. ramfs/procfs
3. shell `ls` / `cat`
4. executable abstraction under `/bin`
5. process address spaces
6. scheduler
7. shell background jobs with `&`
8. richer device drivers

## Error and Debug Output

Use consistent prefixes for serious kernel messages:

- `[KazuOS] ALLOC ERROR ...`
- `KERNEL PANIC: ...`
- `PAGE FAULT ...`
- `DOUBLE FAULT ...`

For debugging QEMU, prefer serial output as well as framebuffer output.

## Safety Rules

- Never trust userspace pointers; validate before reading/writing once validation helpers exist.
- Do not let user pages share kernel writable mappings unnecessarily.
- Keep page table modifications explicit and minimal.
- Do not enable interrupts around fragile ring transition code unless intentional.
- Avoid unbounded loops that print to the framebuffer; they can trigger allocator pressure or make debugging impossible.

## Testing Expectations

After code changes, run:

```powershell
cargo +nightly check
```

When touching ring3, syscalls, page tables, interrupts, boot, or shell commands used during boot demos, also run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File "auto_shell_qemu.ps1"
```

If QEMU fails, inspect:

- `serial.log`
- `qemu-debug.log`
- `qemu-stderr.log`

## Documentation

Keep `ARCHITECTURE.md` updated when subsystem boundaries or roadmap change.
Keep `TODO.md` updated when adding planned kernel features.
Keep `docs/USER_ABI.md` updated when adding or changing syscalls, syscall arguments, return values, or process/user ABI behavior.
