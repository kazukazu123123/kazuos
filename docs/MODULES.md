# Kernel Modules (`.kkm`)

KazuOS device drivers run in **ring3 as kernel modules** — `.kkm` files compiled
from `crates/user_modules/*.rs`. This is the companion to `docs/KXE.md` (which
covers ordinary ring3 *programs*); read that first for the executable basics.

For *why* drivers live in ring3 (and which subsystems deliberately stay in the
kernel, e.g. the keyboard), see the **Driver Policy** section of
`docs/ARCHITECTURE.md`. This document covers the **how**: the binary format, the
module source contract, the build pipeline, and the load/unload lifecycle.

## What a `.kkm` actually is

A `.kkm` is a **KXE binary with the module flag set**. It uses the exact same
header as a normal program (see `docs/KXE.md`); the only difference is the
`flags` field:

```
offset  size  field        .kkm value
 0       4    magic        "KXE\0"
 8       8    entry        virtual entry point
16       8    code_offset  offset to code (0x24)
24       8    code_size    size of code
32       4    flags        1  (KXE_FLAG_MODULE; a normal program is 0)
36       4    reserved
36+     ...   code         raw binary, relocated to USER_BASE (0x8000000000)
```

`exec::spawn_module` refuses to load a KXE whose `flags & KXE_FLAG_MODULE == 0`,
so a `.kkm` and a `.kxe` are not interchangeable even though the container is the
same. The module is loaded at `USER_BASE`, `R_X86_64_RELATIVE` relocations are
applied, and `_start` is called — identical to a program.

The difference at runtime is **privilege**: a module process is created at
`PrivilegeLevel::Driver`, not `PrivilegeLevel::User` (see "Privileges" below).

## The module source contract

A module is a `#![no_std] #![no_main]` crate that `include!`s the module runtime
and implements **four lifecycle functions**. The runtime supplies `_start`, the
panic handler, the global allocator, and all the syscall wrappers.

```rust
#![no_std]
#![no_main]
include!("../../crates/user_rt/module_runtime.rs");

pub fn kkm_info() -> KkmInfo {
    KkmInfo { name: "mydrv", depends: &[] }
}

pub fn kkm_init() -> bool {
    // Acquire hardware (ports / IRQ / BARs), open IPC channels.
    // Return false to abort the load (the process exits with code 1).
    true
}

pub fn kkm_run() {
    loop {
        sys_sleep_tick();              // or sys_irq_wait(irq)
        if sys_signal_check() { return; } // unload requested → leave the loop
        // ... do work, e.g. drain the device and sys_ipc_send(...) events ...
    }
}

pub fn kkm_exit() {
    // Quiesce hardware and close IPC channels. Always runs after kkm_run returns.
}
```

`_start` (in `module_runtime.rs`) ties these together:

```
sys_signal_catch(true);     // so unload can signal us
if !kkm_init() { sys_exit(1); }
kkm_run();                  // returns when sys_signal_check() is true
kkm_exit();
sys_exit(0);
```

### `KkmInfo`

```rust
pub struct KkmInfo {
    pub name:    &'static str,
    pub depends: &'static [&'static str], // other module names this one needs
}
```

> **Note (current behaviour):** `kkm_info()` is defined by convention but is **not
> consulted by the build or the loader today**. The module name comes from the
> *filename* (`kmod::load` derives it from the path via `extract_name`), and
> `depends` is **not enforced** — load order is whatever `modules.list` specifies.
> Treat `kkm_info` as forward-looking metadata until tooling reads it.

### Available syscall helpers

`module_runtime.rs` exposes (among others): `sys_ioport_request`, `sys_irq_wait`,
`sys_pci_bar_map`/`sys_pci_bar_unmap`, `sys_ipc_open`/`sys_ipc_send`/`sys_ipc_recv`,
`sys_heap_alloc`/`sys_heap_free` (also wired as the global allocator, so `alloc`
works), `sys_sleep`/`sys_sleep_tick`, `sys_signal_catch`/`sys_signal_check`, and a
raw `syscall(n, a0, a1, a2)` escape hatch. `print!`/`println!` macros route to
`SYS_CONSOLE_WRITE`.

## Privileges

Process privilege is ordered `System (0) < Driver (1) < User (2)` — **lower is more
privileged**. Modules run at `Driver`. The hardware-facing syscalls check
`privilege_level(caller) > Driver` and reject `User` callers, so these are
**module-only**:

| Syscall | Purpose |
| --- | --- |
| `SYS_IOPORT_REQUEST` | Add ports to the process's TSS I/O permission bitmap (then use `in`/`out` directly) |
| `SYS_IRQ_WAIT` | Block until an IRQ fires (or a stop signal wakes us) |
| `SYS_DMA_ALLOC` / `SYS_DMA_FREE` | Physically-contiguous DMA buffers |
| `SYS_PCI_BAR_MAP` / `SYS_PCI_BAR_UNMAP` | Map a PCI BAR into the module's address space |

`SYS_MODULE_LOAD`/`UNLOAD`/`LIST`/`INFO` are **not** privileged — any process
(including the shell) may load and unload modules.

## Build pipeline

`crates/kernel/build.rs` compiles every `*.rs` in `crates/user_modules/` (sorted by
filename) during the kernel build:

1. `rustc` → ELF: `--edition 2024 --target x86_64-unknown-none -C panic=abort
   -C opt-level=3` linked with `crates/user_programs/link.ld`.
2. Collect `R_X86_64_RELATIVE` relocations (fixed up to `USER_BASE`) and the ELF
   entry point; compute the load mem size.
3. `objcopy -O binary` → flat code, resized to mem size, with relocations patched in.
4. Wrap the code in a KXE header with `flags = KXE_FLAG_MODULE (1)` → `<stem>.kkm`.
5. Place it in the initramfs (KFS) at `/modules/<stem>.kkm`, alongside
   `/modules/modules.list`.

Requires `rustc` and `objcopy` (LLVM) on `PATH`, same as the program build. No
manual step is needed — adding a `.rs` file under `crates/user_modules/` and
rebuilding the kernel is enough to produce its `.kkm`.

## Loading, listing, and unloading

### At boot

`main.rs` calls `kmod::load_from_list("/modules/modules.list")`. Each non-blank,
non-`#` line is a module path to load in order. The current list:

```
/modules/ps2mouse.kkm
```

### At runtime

```rust
let id = sys_module_load(b"/modules/mydrv.kkm"); // module id, or u64::MAX on failure
sys_module_unload(id);                           // true on success
```

`load` spawns the module process (`exec::spawn_module`) at `Driver` privilege and
records it in a fixed table (max **16** modules). **Duplicate module names are
rejected.** `unload` marks the entry `Unloading` and calls
`process::send_module_exit(pid)`, which raises the stop signal the module observes
via `sys_signal_check()`; the module then leaves `kkm_run`, runs `kkm_exit`, and
exits. `kmod::on_process_exit` clears the table slot when the process is gone.

The `modules` user program (`crates/user_programs/modules.rs`) wraps these
syscalls as a CLI:

```
modules list             List loaded kernel modules (ID, NAME, PID, STATUS)
modules load <path>      Load a .kkm by path
modules unload <id>      Unload a module by id
modules help             Usage
```

The `gui` desktop depends on `ps2mouse` being loaded — it opens the
`module_mouse` IPC channel that the driver publishes.

### List / info ABI

`SYS_MODULE_LIST(buf, len)` fills `buf` with **48-byte** entries and returns the
count; `SYS_MODULE_INFO(id, buf)` fills one entry. Layout:

```
offset  size  field
 0       4    id        (u32)
 4       4    pid       (u32)
 8       4    status    (u32: 0=Running, 1=Unloading, 2=Failed)
12      32    name      (bytes, not NUL-guaranteed; use name_len)
44       4    name_len  (u32)
```

## Reference module: `ps2mouse`

`crates/user_modules/ps2mouse.rs` is the canonical example:

- `kkm_init`: `sys_ioport_request(0x60, 1)` and `(0x64, 1)`, opens the
  `module_mouse` IPC channel, enables the PS/2 auxiliary port and streaming.
- `kkm_run`: each `sys_sleep_tick()`, drains available PS/2 bytes, assembles
  3-byte packets, and `sys_ipc_send`s a 5-byte `(buttons, dx:i16, dy:i16)` event.
- `kkm_exit`: disables streaming, drains leftover bytes, disables the aux port,
  closes the IPC channel.

It does raw `in`/`out` from ring3 after being granted the ports — the model for
any new driver. Write new drivers this way rather than adding files under
`crates/kernel/src/drivers/`.
