# KazuOS Syscall Reference

User programs invoke the kernel via `int 0x80` with the syscall number in `rax`.

## Calling Convention

| Register | Meaning |
| --- | --- |
| `rax` | syscall number |
| `rdi` | arg0 |
| `rsi` | arg1 |
| `rdx` | arg2 |
| `rax` | return value |

Constants live in `crates/kazuos_abi/src/syscall_numbers.rs` and are included by both kernel and user programs via `include!("...")`.

## Syscall Details

### Console / Display

#### `SYS_CONSOLE_WRITE` (1)

Write UTF-8 text to the console (framebuffer + serial).

| arg | Meaning |
| --- | --- |
| arg0 | userspace buffer pointer |
| arg1 | byte length |

Returns 0. If framebuffer ownership is held by another process, the framebuffer write is suppressed; serial write proceeds normally.

#### `SYS_CONSOLE_CLEAR` (2)

Clear the console.

#### `SYS_CURSOR_SAVE` (3)

Save current cursor position for later restoration by `SYS_CURSOR_DRAW`.

#### `SYS_CURSOR_DRAW` (4)

| arg | Meaning |
| --- | --- |
| arg0 | 1=show cursor, 0=hide |

Draws the saved cursor position.

#### `SYS_FB_ACQUIRE` (5)

Acquire exclusive framebuffer access.

| arg | Meaning |
| --- | --- |
| arg0 | `*mut FbInfo` — caller-allocated 24-byte buffer |

On success: saves current FB pixels, maps physical FB into userspace at `USER_FB_VA`, clears FB to black, fills `FbInfo`, returns 0.

On failure (another non-shell process holds it): returns `u64::MAX`.

`FbInfo` layout:
```
base:   u64  — user VA (0x0000_0082_0000_0000)
width:  u32  — pixels per row
height: u32  — rows
stride: u32  — pixels between adjacent rows (may be ≥ width)
format: u32  — 0=RGB, 1=BGR (byte order of red channel)
```

Pixel address: `base + (y * stride + x) * 4`. Each pixel is 4 bytes (B/G/R/unused or R/G/B/unused).

#### `SYS_FB_RELEASE` (6)

Release framebuffer ownership and restore the saved back buffer. Auto-released on process exit.

#### `SYS_CONSOLE_SIZE` (7)

Returns `(rows << 32) | cols` — terminal character dimensions.

#### `SYS_FB_QUERY` (8)

Read-only query of framebuffer parameters and current owner.

| arg | Meaning |
| --- | --- |
| arg0 | `*mut FbInfo` (may be null) |
| arg1 | `*mut u64` receiving owner PID, or `u64::MAX` if unowned (may be null) |

Returns 0 on success, `u64::MAX` if no framebuffer exists. Does not modify ownership.

### Process / Lifecycle

#### `SYS_EXIT` (9)

Terminate the calling process. Does not return.

#### `SYS_EXEC` (10)

Spawn a new process from a KXE binary.

| arg | Meaning |
| --- | --- |
| arg0 | path pointer |
| arg1 | path byte length |
| arg2 | stdio_pack: `(stdout_fd << 16) \| stdin_fd`. Use `0xFFFF_FFFF` for default (console in/out). |

Returns the new PID on success. Returns 1 if the binary has the driver flag set. Returns 0 or `u64::MAX` on error.

#### `SYS_KILL` (11)

Kill a process. Refuses to kill driver processes.

| arg | Meaning |
| --- | --- |
| arg0 | PID |

Returns 0.

#### `SYS_WAIT` (12)

Block until the target process exits.

| arg | Meaning |
| --- | --- |
| arg0 | PID |

Returns 1 when the process has exited. Blocks the caller if the process is still running.

#### `SYS_PROCESS_INFO` (13)

Query process information.

| arg0 | Behaviour |
| --- | --- |
| 0 | Returns current PID |
| 1 | Returns process count |
| 2 | Returns first PID, or 0 if none |
| any PID | Writes `ProcessInfo` to `arg1` buffer (96 bytes) and returns 0. Returns `u64::MAX` if PID not found or buffer is null. |

`ProcessInfo` layout (96 bytes):
```
offset  size  field
 0       8    pid
 8       8    state       — 1=Ready, 2=Running, 3=Sleeping, 4=Exited
16      32    image_name  — NUL-terminated ASCII
48       8    start_tsc
56       8    entry
64       8    stack_top
72       8    step
80       8    cpu_ticks
88       8    memory_bytes
```

#### `SYS_PROCESS_NEXT` (14)

Enumerate processes.

| arg | Meaning |
| --- | --- |
| arg0 | previous PID, or `u64::MAX` for first |

Returns the next PID, or `u64::MAX` if none remain.

#### `SYS_SLEEP` (15)

Block the calling process for a duration.

| arg | Meaning |
| --- | --- |
| arg0 | duration |
| arg1 | unit: `SLEEP_UNIT_MS` (0) for milliseconds, `SLEEP_UNIT_US` (1) for microseconds |

Returns 0. Uses TSC-based deadline; minimum resolution is ~1µs.

### Memory

#### `SYS_MEM_INFO` (16)

Returns `(total_kib << 32) | used_kib`. Returns 0 if PMM is not available.

#### `SYS_HEAP_ALLOC` (17)

Allocate zeroed pages in the calling process's address space.

| arg | Meaning |
| --- | --- |
| arg0 | size in bytes |

Returns user VA (page-aligned, zeroed), or `u64::MAX` on error. Max 128 MiB per allocation.

#### `SYS_HEAP_FREE` (18)

Free memory allocated by `SYS_HEAP_ALLOC`.

| arg | Meaning |
| --- | --- |
| arg0 | VA returned by `SYS_HEAP_ALLOC` |

Returns 0 on success, `u64::MAX` on error.

### Signals

#### `SYS_SIGNAL_CATCH` (19)

Opt in or out of handling Ctrl+C.

| arg | Meaning |
| --- | --- |
| arg0 | 1=catch, 0=reset to default (kill) |

#### `SYS_SIGNAL_CHECK` (20)

Check if Ctrl+C was received since the last call. Returns 1 if pending (flag is cleared), else 0.

### IPC

#### `SYS_IPC_OPEN` (21)

Open or create a named IPC channel.

| arg | Meaning |
| --- | --- |
| arg0 | name pointer |
| arg1 | name length |

Returns channel ID (1-based), or `u64::MAX` on error. Max 32 channels system-wide.

#### `SYS_IPC_SEND` (22)

Send a message to a channel. Blocks if the queue is full.

| arg | Meaning |
| --- | --- |
| arg0 | channel ID |
| arg1 | data pointer |
| arg2 | data length |

Returns 0 on success, `u64::MAX` on error. Max message size: 4096 bytes. Max queue depth: 8.

#### `SYS_IPC_RECV` (23)

Receive a message from a channel. Blocks if the queue is empty.

| arg | Meaning |
| --- | --- |
| arg0 | channel ID |
| arg1 | buffer pointer |
| arg2 | buffer max length |

Returns the number of bytes written, or `u64::MAX` on error.

#### `SYS_IPC_CLOSE` (24)

Close a channel (decrements refcount; destroyed when it reaches 0).

| arg | Meaning |
| --- | --- |
| arg0 | channel ID |

Returns 0.

### File I/O

#### `SYS_OPEN` (25)

Open a file or device.

| arg | Meaning |
| --- | --- |
| arg0 | path pointer |
| arg1 | path length |

Returns fd (1-based), or `u64::MAX` on error.

#### `SYS_CLOSE` (26)

Close a file descriptor.

| arg | Meaning |
| --- | --- |
| arg0 | fd |

Returns 0, or `u64::MAX` on error.

#### `SYS_READ` (27)

Read from a file descriptor.

| arg | Meaning |
| --- | --- |
| arg0 | fd |
| arg1 | buffer pointer |
| arg2 | buffer length |

Returns bytes read, 0 on EOF, or `u64::MAX` on error. Blocking for `ConsoleIn` and pipes.

#### `SYS_WRITE` (28)

Write to a file descriptor.

| arg | Meaning |
| --- | --- |
| arg0 | fd |
| arg1 | buffer pointer |
| arg2 | buffer length |

Returns bytes written, or `u64::MAX` on error. Write to `ConsoleOut` is equivalent to `SYS_CONSOLE_WRITE`.

#### `SYS_IOCTL` (29)

Device-specific control.

| arg | Meaning |
| --- | --- |
| arg0 | fd |
| arg1 | request |
| arg2 | argument |

Returns device-specific, or `u64::MAX` on error.

#### `SYS_PIPE` (30)

Create a pipe.

| arg | Meaning |
| --- | --- |
| arg0 | `*mut [u64; 2]` — receives [read_fd, write_fd] |

Returns 0 on success, `u64::MAX` on error.

### Hardware / Driver

#### `SYS_PCI_INFO` (31)

Query PCI device list.

| arg | Meaning |
| --- | --- |
| arg0 | device index (0-based) |
| arg1 | `*mut PciDeviceInfo` (may be null to get count) |

Returns the total device count, or `u64::MAX` on error.

`PciDeviceInfo` layout (16 bytes):
```
bus: u8, device: u8, function: u8, _pad: u8,
vendor_id: u16, device_id: u16,
class_code: u8, subclass: u8, prog_if: u8, header_type: u8
```

#### `SYS_IOPORT_REQUEST` (32)

Allow the calling driver process to access a range of I/O ports.

| arg | Meaning |
| --- | --- |
| arg0 | base port |
| arg1 | count |

Returns 0. Driver-only; returns `u64::MAX` for non-driver processes.

#### `SYS_IRQ_WAIT` (33)

Block until the given IRQ fires.

| arg | Meaning |
| --- | --- |
| arg0 | IRQ number |

Blocks the calling driver process. Driver-only; returns `u64::MAX` for non-driver processes.

#### `SYS_DMA_ALLOC` (34)

Allocate physically-contiguous DMA memory.

| arg | Meaning |
| --- | --- |
| arg0 | size in bytes |
| arg1 | `*mut u64` receiving the physical address (may be null) |

Returns user VA, or `u64::MAX` on error. Driver-only; max 128 MiB.

#### `SYS_DMA_FREE` (35)

Free DMA memory.

| arg | Meaning |
| --- | --- |
| arg0 | VA from `SYS_DMA_ALLOC` |

Returns 0 on success, `u64::MAX` on error. Driver-only.

### PCI

#### `SYS_PCI_BAR_MAP` (36)

Map a PCI MMIO BAR into the calling driver's address space.

| arg | Meaning |
| --- | --- |
| `arg0` | BDF encoded as `(bus << 16) \| (device << 8) \| function` |
| `arg1` | BAR index (`0`–`5`) |

Returns the user-space virtual address on success, or `u64::MAX` on error. Driver-only. I/O BARs are not supported.

#### `SYS_PCI_BAR_UNMAP` (37)

Unmap a PCI BAR previously mapped by `SYS_PCI_BAR_MAP`.

| arg | Meaning |
| --- | --- |
| `arg0` | user VA returned by `SYS_PCI_BAR_MAP` |

Returns `0` on success, or `u64::MAX` on error. Driver-only.

### Keyboard

#### `SYS_KEYBOARD_READ` (38)

Read a keypress. Blocks until a key is available.

Returns the key byte. Extended keys: `0x80`=Left, `0x81`=Right, `0x82`=Up, `0x83`=Down.

#### `SYS_KEYBOARD_POLL` (39)

Non-blocking key check.

Returns key byte if available, or 0 if no key is pending.

### System / Misc

#### `SYS_CPU_INFO` (40)

| arg0 | Returns |
| --- | --- |
| 0 | total timer ticks |
| 1 | total user CPU ticks |
| 2 | kernel CPU ticks |
| 3 | idle CPU ticks |
| any PID | CPU ticks for that PID, or `u64::MAX` if not found |

#### `SYS_SHUTDOWN` (41)

Shut down the system. Does not return.

#### `SYS_REBOOT` (42)

Reboot the system. Does not return.

#### `SYS_LS` (43)

List directory contents.

| arg | Meaning |
| --- | --- |
| arg0 | path pointer (0 for `/`) |
| arg1 | path length |

Returns entry count, or `u64::MAX` on error.

### Kernel modules

#### `SYS_MODULE_LOAD` (44)

Load a kernel module by path.

| arg | Meaning |
| --- | --- |
| `arg0` | path pointer |
| `arg1` | path length |

Returns module id on success, or `u64::MAX` on error.

#### `SYS_MODULE_UNLOAD` (45)

Unload a kernel module by id.

| arg | Meaning |
| --- | --- |
| `arg0` | module id |

Returns `0` on success, `u64::MAX` on error.

#### `SYS_MODULE_LIST` (46)

List loaded modules.

| arg | Meaning |
| --- | --- |
| `arg0` | buffer pointer (48-byte entries) |
| `arg1` | buffer length |

Returns entry count.

#### `SYS_MODULE_INFO` (47)

Query info for a specific module.

| arg | Meaning |
| --- | --- |
| `arg0` | module id |
| `arg1` | buffer pointer (48 bytes) |

Returns `0` on success, `u64::MAX` on error.

## CPU Accounting

CPU time is measured in TSC ticks sampled at each timer interrupt.

- **kernel ticks**: timer fired while no user process was on CPU and not idle
- **idle ticks**: timer fired in the idle `hlt` loop
- **user ticks**: timer fired while a user-mode process was on CPU

## `int 0x80` Gate Type

The IDT entry for `int 0x80` is a **trap gate** (type `0xEF`, DPL=3). Unlike an interrupt gate, IF is **not** cleared on entry, so timer and keyboard interrupts can be delivered during syscall execution.
