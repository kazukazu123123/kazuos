# KazuOS User ABI

User programs call the kernel with `int 0x80`.

## Calling convention

| Register | Meaning |
| --- | --- |
| `rax` | syscall number |
| `rdi` | arg0 |
| `rsi` | arg1 |
| `rdx` | arg2 |
| `rax` | return value |

## Syscalls

The source of truth for these numbers is `crates/kazuos_abi/src/syscall_numbers.rs`.

| Number | Name | Arguments | Return |
| --- | --- | --- | --- |
| `1` | `SYS_CONSOLE_WRITE` | `arg0 = buffer pointer`, `arg1 = byte length` | `0` (writes UTF-8 text to the framebuffer console) |
| `2` | `SYS_CURSOR_SAVE` | none | `0` (saves console cursor position) |
| `3` | `SYS_CURSOR_DRAW` | none | `0` (draws the text cursor) |
| `4` | `SYS_CURSOR_RESTORE` | none | `0` (restores saved cursor position) |
| `5` | `SYS_FB_ACQUIRE` | `arg0 = *mut FbInfo` | `0` on success; `u64::MAX` if framebuffer already held |
| `6` | `SYS_FB_RELEASE` | none | `0` |
| `7` | `SYS_CONSOLE_SIZE` | `arg0 = 0` gets; `arg0 != 0` sets (`cols = arg0 & 0xFFFF`, `rows = arg0 >> 16`, `arg1 = target pid`, 0 = self) | get: `(rows << 32) \| cols` |
| `8` | `SYS_EXIT` | `arg0 = exit code` | does not return |
| `9` | `SYS_EXEC` | `arg0 = "path\0arg1\0arg2\0"` pointer, `arg1 = byte length`, `arg2 = stdio pack (`stdin_fd \| (stdout_fd << 16)`, `0xFFFF` = console)` | new pid, or `0`/`u64::MAX`/`1` (driver) on error |
| `10` | `SYS_THREAD_SPAWN` | `arg0 = entry`, `arg1 = arg` (in rdi), `arg2 = stack_top` | new tid, or `0` on failure |
| `11` | `SYS_THREAD_EXIT` | none | does not return (last thread exits the process) |
| `12` | `SYS_THREAD_JOIN` | `arg0 = tid` | blocks until that thread exits; `0` if already exited |
| `13` | `SYS_THREAD_NEXT` | `arg0 = pid`, `arg1 = previous tid` (0 to start) | next tid `>` arg1 of that pid, or `u64::MAX` if none |
| `14` | `SYS_THREAD_INFO` | `arg0 = tid`, `arg1 = *mut ThreadInfo` | `0` on success; `u64::MAX` if no such tid |
| `15` | `SYS_KILL` | `arg0 = pid` | `0` |
| `16` | `SYS_WAIT` | `arg0 = pid` | blocks until target exits; returns `1` |
| `17` | `SYS_PROCESS_INFO` | `arg0 = selector`, `arg1 = buffer` | selector-dependent (see below) |
| `18` | `SYS_PROCESS_NEXT` | `arg0 = previous pid` | next pid, or `u64::MAX` if none |
| `19` | `SYS_SLEEP` | `arg0 = duration`, `arg1 = unit: `SLEEP_UNIT_MS`(0) / `SLEEP_UNIT_US`(1) / `SLEEP_UNIT_TICK`(2)` | `0` after blocking |
| `20` | `SYS_MEM_INFO` | none | `(total_kib << 32) \| used_kib`, or `0` |
| `21` | `SYS_HEAP_ALLOC` | `arg0 = size` | user VA (page-aligned, zeroed), or `u64::MAX` on error |
| `22` | `SYS_HEAP_FREE` | `arg0 = VA from SYS_HEAP_ALLOC` | `0` on success; `u64::MAX` on error |
| `23` | `SYS_SIGNAL_CATCH` | `arg0 = 1 to catch, 0 to reset` | `0` |
| `24` | `SYS_SIGNAL_CHECK` | none | `1` if Ctrl+C since last check, else `0` |
| `25` | `SYS_IPC_OPEN` | `arg0 = name ptr`, `arg1 = name len` | channel id (1-based), or `u64::MAX` on error |
| `26` | `SYS_IPC_SEND` | `arg0 = channel id`, `arg1 = buf ptr`, `arg2 = buf len` | `0` on success; blocks if full; `u64::MAX` on error |
| `27` | `SYS_IPC_RECV` | `arg0 = channel id`, `arg1 = buf ptr`, `arg2 = buf len` | bytes written; blocks; `u64::MAX` on error |
| `28` | `SYS_IPC_TRY_RECV` | `arg0 = channel id`, `arg1 = buf ptr`, `arg2 = buf len` | bytes written, `0` if empty (non-blocking), `u64::MAX` on error |
| `29` | `SYS_IPC_CLOSE` | `arg0 = channel id` | `0` |
| `30` | `SYS_OPEN` | `arg0 = path ptr`, `arg1 = path len` | fd (1-based), or `u64::MAX` on error |
| `31` | `SYS_CLOSE` | `arg0 = fd` | `0` |
| `32` | `SYS_READ` | `arg0 = fd`, `arg1 = buf ptr`, `arg2 = buf len` | bytes read, or `u64::MAX` on error |
| `33` | `SYS_TRY_READ` | `arg0 = fd`, `arg1 = buf ptr`, `arg2 = buf len` | bytes read, `0` = would block, `u64::MAX` = EOF/error (non-blocking) |
| `34` | `SYS_WRITE` | `arg0 = fd`, `arg1 = buf ptr`, `arg2 = buf len` | bytes written, or `u64::MAX` on error |
| `35` | `SYS_IOCTL` | `arg0 = fd`, `arg1 = request`, `arg2 = arg` | device-specific, or `u64::MAX` |
| `36` | `SYS_PIPE` | `arg0 = *mut [u64; 2]` (filled with `[read_fd, write_fd]`) | `0` on success; `u64::MAX` on error |
| `37` | `SYS_PCI_INFO` | `arg0 = index`, `arg1 = *mut PciDeviceInfo` | count, or `u64::MAX` on error |
| `38` | `SYS_IOPORT_REQUEST` | `arg0 = port`, `arg1 = count` | `0` (driver only) |
| `39` | `SYS_IRQ_WAIT` | `arg0 = irq_num` | blocks until the IRQ fires; `u64::MAX` if interrupted (driver only) |
| `40` | `SYS_DMA_ALLOC` | `arg0 = size`, `arg1 = *mut u64 phys_out` | user VA; `u64::MAX` on error (driver only) |
| `41` | `SYS_DMA_FREE` | `arg0 = VA from SYS_DMA_ALLOC` | `0` on success; `u64::MAX` on error (driver only) |
| `42` | `SYS_PCI_BAR_MAP` | `arg0 = BDF ((bus << 16) \| (dev << 8) \| func)`, `arg1 = BAR index (0-5)` | user VA on success; `u64::MAX` on error (driver only) |
| `43` | `SYS_PCI_BAR_UNMAP` | `arg0 = VA from SYS_PCI_BAR_MAP` | `0` on success; `u64::MAX` on error (driver only) |
| `44` | `SYS_KEYBOARD_POLL` | none | key byte if available, or `0` (non-blocking) |
| `45` | `SYS_CPU_INFO` | `arg0 = selector`, `arg1 = per-cpu index when noted` | selector-dependent (see below) |
| `46` | `SYS_SHUTDOWN` | none | does not return |
| `47` | `SYS_REBOOT` | none | does not return |
| `48` | `SYS_READDIR` | `arg0 = path ptr` (or 0 for `/`), `arg1 = path len`, `arg2 = caller buffer` | entry count, or `u64::MAX` on error |
| `49` | `SYS_MODULE_LOAD` | `arg0 = path ptr`, `arg1 = path len` | module id, or `u64::MAX` on error |
| `50` | `SYS_MODULE_UNLOAD` | `arg0 = module id` | `0` on success; `u64::MAX` on error |
| `51` | `SYS_MODULE_LIST` | `arg0 = buf ptr`, `arg1 = buf len` | entry count (48-byte entries) |
| `52` | `SYS_MODULE_INFO` | `arg0 = module id`, `arg1 = buf ptr` | `0` on success; `u64::MAX` on error |
| `53` | `SYS_CREATE` | `arg0 = path ptr`, `arg1 = path len` | fd (RW) on success; `u64::MAX` on error |
| `54` | `SYS_UNLINK` | `arg0 = path ptr`, `arg1 = path len` | `0` on success; `u64::MAX` on error |
| `55` | `SYS_MKDIR` | `arg0 = path ptr`, `arg1 = path len` | `0` on success; `u64::MAX` on error |
| `56` | `SYS_RMDIR` | `arg0 = path ptr`, `arg1 = path len` | `0` on success; `u64::MAX` on error |
| `57` | `SYS_SIGINT_FG` | `arg0 = pid` | `1` if a foreground descendant was signaled, `0` if `arg0` is itself the wait-chain leaf |

## `SYS_PROCESS_INFO` selectors

| `arg0` | Return |
| --- | --- |
| `0` | current PID |
| `1` | process count |
| `2` | first PID, or `0` if none |
| any other value (PID) | **`arg1` = userspace `ProcessInfo*` buffer** — kernel writes the full `ProcessInfo` struct to the buffer and returns `0`. Returns `u64::MAX` if PID not found or `arg1` is null. |

`ProcessInfo` layout (`#[repr(C)]`, 104 bytes):

```
pid: u64          — process ID
state: u64        — 1=Ready, 2=Running, 3=Sleeping, 4=Exited
image_name: [u8; 32] — NUL-terminated ASCII name
start_tsc: u64    — TSC at process start
entry: u64        — virtual entry point
stack_top: u64    — virtual stack top
step: u64         — scheduler step counter
cpu_ticks: u64    — accumulated TSC ticks
memory_bytes: u64 — allocated memory in bytes
parent: u64       — parent process ID (PPID); 0 = kernel
```

Userspace can retrieve the full `ProcessInfo` (including `memory_bytes`, `cpu_ticks`, and the parent PID) for any process by calling `SYS_PROCESS_INFO(pid, buf)` with a 104-byte buffer.

## Threads

A process may run multiple threads in its single address space (shared code, heap and
fds). On SMP they can run on different cores in parallel. Shared data must be synchronised
by userspace (atomics or a spinlock); the kernel does no implicit locking.

| Syscall | Args | Return |
| --- | --- | --- |
| `SYS_THREAD_SPAWN` | `arg0 = entry` (`extern "C" fn(u64)`), `arg1 = arg` (passed in rdi), `arg2 = stack_top` (caller-allocated, e.g. via `SYS_HEAP_ALLOC`) | new tid, or `0` on failure |
| `SYS_THREAD_EXIT` | none | does not return; ends the calling thread (the last thread ending exits the process) |
| `SYS_THREAD_JOIN` | `arg0 = tid` | blocks until that thread exits; returns immediately if it already has |
| `SYS_THREAD_NEXT` | `arg0 = pid`, `arg1 = previous tid` (0 to start) | lowest tid `>` arg1 belonging to that pid, or `u64::MAX` when there are no more |
| `SYS_THREAD_INFO` | `arg0 = tid`, `arg1 = *mut ThreadInfo` | `0` on success; `u64::MAX` if no such tid |

The thread entry function must not return — finish it with `SYS_THREAD_EXIT`. The blocking
syscalls (`SYS_SLEEP`, `SYS_WAIT`, `SYS_THREAD_JOIN`, `SYS_IRQ_WAIT`) block the calling
thread, not the whole process.

A process's CPU time (`ProcessInfo.cpu_ticks`) is the sum of its threads': each timer tick
is credited to the thread that was actually running, so a multi-threaded program can read
up to `cpu_count * 100%` of total CPU.

`SYS_THREAD_NEXT` / `SYS_THREAD_INFO` enumerate the threads of any process (used by `ktop`).
`ThreadInfo` layout (`#[repr(C)]`, 40 bytes):

```
tid: u64          — thread ID
pid: u64          — owning process ID
state: u64        — 1=Ready, 2=Running, 3=Sleeping
cpu_ticks: u64    — accumulated TSC ticks for this thread
assigned_cpu: u64 — the CPU this thread is pinned to
```

## `SYS_CPU_INFO` selectors

| `arg0` | `arg1` | Return |
| --- | --- | --- |
| `0` | — | total timer ticks |
| `1` | — | total user CPU ticks across all tracked processes |
| `2` | — | kernel CPU ticks |
| `3` | — | idle CPU ticks |
| `4` | — | CPU count |
| `5` | — | BSP local APIC ID |
| `6` | — | current CPU index |
| `7` | CPU index | local APIC ID of that CPU, or `0xFF` if invalid |
| `8` | CPU index | idle CPU ticks for that CPU |
| `9` | CPU index | kernel CPU ticks for that CPU |
| `10` | CPU index | user CPU ticks for that CPU |
| any other value | — | CPU ticks for PID=`arg0`, or `u64::MAX` if not found |

## Framebuffer access

A user program can get exclusive access to the physical framebuffer via `SYS_FB_ACQUIRE`.

### `SYS_FB_ACQUIRE`

`arg0` points to a caller-allocated `FbInfo` buffer (24 bytes, `#[repr(C)]`):

```
base:   u64  — user-space virtual address where the FB is mapped
width:  u32  — pixels per row
height: u32  — rows
stride: u32  — pixels between the start of adjacent rows (may be ≥ width)
format: u32  — 0 = RGB, 1 = BGR (byte order of the red channel)
```

Pixel address: `base + (y * stride + x) * 4`.  Each pixel is 4 bytes; the 4th byte is unused (always 0).

On success the kernel:
1. Saves the current framebuffer pixels to a kernel-side back buffer.
2. Maps the physical framebuffer pages into the calling process's address space at `base`.
3. Writes `FbInfo` and returns `0`.

On failure (another non-shell process already owns the framebuffer) returns `u64::MAX` and does not modify the buffer.

### `SYS_FB_RELEASE`

Releases ownership and restores the saved back buffer (the shell's screen).  
Called automatically when the process exits, so explicit release is optional.

### Shell exception

The shell never holds framebuffer ownership.  Any program can therefore always acquire it
(assuming no other non-shell process already has it).  
While a program holds the framebuffer, `SYS_CONSOLE_WRITE` from other processes is suppressed
on the framebuffer (still forwarded to the serial port) so the shell's text output does not
corrupt the program's display.

### Exclusive access

Only one process can hold the framebuffer at a time.  A second `SYS_FB_ACQUIRE` from a
different process returns `u64::MAX`; the caller must retry later or exit.

---

## CPU accounting

CPU usage is tracked as TSC ticks, accumulated per timer tick to the thread that was
running. A process's `cpu_ticks` is the sum of its threads'. Use `SYS_CPU_INFO` selector
`1` for total user ticks, selector `pid` for a process's own ticks, and selectors `8`/`9`/`10`
with a CPU index for per-core idle/kernel/user ticks.

## `int 0x80` gate type

The IDT entry for `int 0x80` is a **trap gate** (type `0xEF`, DPL=3). Unlike an interrupt gate, a trap gate does **not** clear IF on entry, so timer and keyboard interrupts can be delivered while a syscall handler is running. Kernel code must be aware that it can be preempted mid-syscall.

## `int 0x80` kernel stack frame layout

`syscall_int80_asm` pushes 14 general-purpose registers plus an 8-byte alignment pad before calling `syscall_handler`. The full layout on the kernel stack (from `blocking_rsp` upward) is:

```
blocking_rsp+0   r15
blocking_rsp+8   r14
blocking_rsp+16  r13
blocking_rsp+24  r12
blocking_rsp+32  r11
blocking_rsp+40  r10
blocking_rsp+48  r9
blocking_rsp+56  r8
blocking_rsp+64  rdi
blocking_rsp+72  rsi
blocking_rsp+80  rdx
blocking_rsp+88  rcx
blocking_rsp+96  rbx
blocking_rsp+104 rbp
blocking_rsp+112 user_rip   (CPU-pushed iretq frame)
blocking_rsp+120 user_cs
blocking_rsp+128 user_rflags
blocking_rsp+136 user_rsp
blocking_rsp+144 user_ss
```

`blocking_rsp` points to the r15 slot (skipping the 8-byte alignment pad below it). The blocking resume path in `enter_next_process` uses this pointer directly: 14 pops restore r15..rbp, then `iretq` restores the CPU frame.

## Blocking syscalls

A blocking `SYS_READ` on console input (and any other syscall that returns `BLOCK_TO_SCHEDULER`) suspends the calling thread:

1. `syscall_int80_asm` saves the full frame pointer (`rsp+8`) to `BLOCKING_RSP_TMP`.
2. The kernel switches to the per-process kernel stack, saves `blocking_rsp` in the process table, and calls `enter_next_process`.
3. When a key arrives, `wakeup_key_waiters` calls `restore_ctx_from_blocking_frame` to copy registers from the blocking frame into `user_context`, then marks the process Ready.
4. `enter_next_process` resumes via the `blocking_rsp` path (14 pops + iretq) with `rax` set to the wakeup return value.

## Driver processes

Driver processes are spawned by the kernel at boot via `exec::spawn_driver()`. They differ from normal user processes in the following ways:

- `privilege = Driver` in the process table
- Can call the driver-only syscalls: `SYS_IOPORT_REQUEST`, `SYS_IRQ_WAIT`, `SYS_DMA_ALLOC`, `SYS_DMA_FREE`
- **Cannot be killed** — `SYS_KILL` and `send_sigint` both refuse to terminate a driver process
- Currently started at fixed boot time; dynamic stop/start is not yet implemented

### Built-in drivers

| Binary | IPC channel | Description |
| --- | --- | --- |
| `drv_ac97.kxe` | `ac97` | AC97 audio playback |

### Audio playback (`drv_ac97.kxe`)

Send the VFS path of a WAV file as raw bytes to the `ac97` IPC channel:

```
ch = SYS_IPC_OPEN("ac97", 4)
SYS_IPC_SEND(ch, "/KazuOS/sound.wav", 17)
SYS_IPC_CLOSE(ch)
```

Supported formats: PCM 8-bit or 16-bit, mono or stereo.  
The driver plays the file synchronously (one file at a time); the next `SYS_IPC_RECV` blocks until playback finishes.

---

## IPC (Inter-Process Communication)

KazuOS provides named message-passing channels. Any process can create or attach to a channel by name.

### Channel lifecycle

```
channel_id = SYS_IPC_OPEN("my-service", 10)   // create or attach
SYS_IPC_SEND(channel_id, buf, len)             // send a message (blocks if queue full)
len = SYS_IPC_RECV(channel_id, buf, max_len)   // receive a message (blocks until available)
SYS_IPC_CLOSE(channel_id)                      // decrement ref count; destroyed when it reaches 0
```

### Constraints

- Max message size: 4096 bytes
- Max queued messages per channel: 8
- Max open channels: 32
- `SYS_IPC_SEND` blocks when the queue is full; unblocked when a receiver calls `SYS_IPC_RECV`
- `SYS_IPC_RECV` blocks when the queue is empty; unblocked when a sender calls `SYS_IPC_SEND`

### Intended use

A driver or service process opens a named channel at startup and loops on `SYS_IPC_RECV`.  
Client processes open the same channel by name and call `SYS_IPC_SEND` to make requests.  
Responses can be sent back on a separate per-client channel opened by the client.

---

## SIGINT (Ctrl+C)

Ctrl+C is handled by the keyboard interrupt handler independently of `int 0x80`.

### Default behavior (`sigint_catch = false`)

The foreground process (the framebuffer owner, or whatever process the shell is waiting on) is killed immediately. No action required from the program.

### Handling Ctrl+C in a program (`sigint_catch = true`)

1. Call `SYS_SIGNAL_CATCH(1)` at startup. The process will no longer be killed on Ctrl+C; instead an internal pending flag is set.
2. Poll `SYS_SIGNAL_CHECK` in the main loop. Returns `1` if Ctrl+C has been received since the last call (flag is cleared automatically).
3. Perform any cleanup (e.g. restore the screen) then call `SYS_EXIT`.

```
// Example: program that handles Ctrl+C
SYS_SIGNAL_CATCH(1)     // opt in

loop:
    // ... work ...
    if SYS_SIGNAL_CHECK() != 0:
        cleanup()
        SYS_EXIT
```

To silently ignore Ctrl+C, call `SYS_SIGNAL_CATCH(1)` and never check `SYS_SIGNAL_CHECK`.  
To restore default kill behavior, call `SYS_SIGNAL_CATCH(0)`.

For processes that do not own the framebuffer, Ctrl+C is delivered by the shell's `wait_foreground` loop via `SYS_KILL`.

---

## Notes

This ABI is experimental.
User pointers are not fully validated yet.
`SYS_EXEC` spawns a new KXE process; it does not replace the calling process.
`SYS_CONSOLE_WRITE` writes to the framebuffer via `console::screen_print`.
