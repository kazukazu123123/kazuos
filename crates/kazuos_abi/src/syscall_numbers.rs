// Console / Display
pub const SYS_CONSOLE_WRITE:  u64 = 1;
pub const SYS_CURSOR_SAVE:    u64 = 2;
pub const SYS_CURSOR_DRAW:    u64 = 3;
pub const SYS_CURSOR_RESTORE: u64 = 4;
pub const SYS_FB_ACQUIRE:     u64 = 5;
pub const SYS_FB_RELEASE:     u64 = 6;
pub const SYS_CONSOLE_SIZE:   u64 = 7;

// Process / Lifecycle
pub const SYS_EXIT:          u64 = 8;
pub const SYS_EXEC:          u64 = 9;
pub const SYS_THREAD_SPAWN:  u64 = 10; // new thread in the caller's address space
pub const SYS_THREAD_EXIT:   u64 = 11; // exit only the calling thread
pub const SYS_THREAD_JOIN:   u64 = 12; // block until a thread (by tid) exits
pub const SYS_THREAD_NEXT:   u64 = 13; // enumerate a process's threads: next tid > arg1 of pid arg0
pub const SYS_THREAD_INFO:   u64 = 14; // fill a ThreadInfo for tid arg0 into buffer arg1
pub const SYS_KILL:          u64 = 15;
pub const SYS_WAIT:          u64 = 16;
pub const SYS_PROCESS_INFO:  u64 = 17;
pub const SYS_PROCESS_NEXT:  u64 = 18;
pub const SYS_SLEEP:         u64 = 19;
pub const SLEEP_UNIT_MS:     u64 = 0;
pub const SLEEP_UNIT_US:     u64 = 1;
pub const SLEEP_UNIT_TICK:   u64 = 2;

// Memory
pub const SYS_MEM_INFO:   u64 = 20;
pub const SYS_HEAP_ALLOC: u64 = 21;
pub const SYS_HEAP_FREE:  u64 = 22;

// Signals
pub const SYS_SIGNAL_CATCH: u64 = 23;
pub const SYS_SIGNAL_CHECK: u64 = 24;

// IPC
pub const SYS_IPC_OPEN:     u64 = 25;
pub const SYS_IPC_SEND:     u64 = 26;
pub const SYS_IPC_RECV:     u64 = 27;
pub const SYS_IPC_TRY_RECV: u64 = 28; // non-blocking SYS_IPC_RECV (returns immediately if empty)
pub const SYS_IPC_CLOSE:    u64 = 29;

// File I/O
pub const SYS_OPEN:     u64 = 30;
pub const SYS_CLOSE:    u64 = 31;
pub const SYS_READ:     u64 = 32;
pub const SYS_TRY_READ: u64 = 33; // non-blocking SYS_READ (0 = would block, u64::MAX = EOF)
pub const SYS_WRITE:    u64 = 34;
pub const SYS_IOCTL:    u64 = 35;
pub const SYS_PIPE:     u64 = 36;

// Hardware / Driver
pub const SYS_PCI_INFO:       u64 = 37;
pub const SYS_IOPORT_REQUEST: u64 = 38;
pub const SYS_IRQ_WAIT:       u64 = 39;
pub const SYS_DMA_ALLOC:      u64 = 40;
pub const SYS_DMA_FREE:       u64 = 41;

// PCI
pub const SYS_PCI_BAR_MAP:   u64 = 42;
pub const SYS_PCI_BAR_UNMAP: u64 = 43;

// Keyboard
pub const SYS_KEYBOARD_POLL: u64 = 44;

// System / Misc
pub const SYS_CPU_INFO:  u64 = 45;
pub const SYS_SHUTDOWN:  u64 = 46;
pub const SYS_REBOOT:    u64 = 47;
pub const SYS_READDIR:   u64 = 48; // enumerate a directory into a caller buffer (no kernel-side printing)

// Kernel modules
pub const SYS_MODULE_LOAD:   u64 = 49;
pub const SYS_MODULE_UNLOAD: u64 = 50;
pub const SYS_MODULE_LIST:   u64 = 51;
pub const SYS_MODULE_INFO:   u64 = 52;

// Filesystem mutations (RAM rootfs)
pub const SYS_CREATE: u64 = 53; // create an empty file, returns fd (RW)
pub const SYS_UNLINK: u64 = 54; // delete a file
pub const SYS_MKDIR:  u64 = 55; // create a directory
pub const SYS_RMDIR:  u64 = 56; // remove an empty directory

// Send SIGINT to the foreground (wait-chain leaf) of arg0's process. Returns 1 if a
// descendant was signaled, 0 if arg0 is itself the leaf (idle at its own prompt).
// Lets a terminal interrupt the command its shell is currently waiting on.
pub const SYS_SIGINT_FG: u64 = 57;

// SYS_EXEC stdio pack: bits[0..16] = stdin fd, bits[16..32] = stdout fd (0xFFFF on either
// = console default). When this bit is also set, the child additionally gets fd 3 as a
// controlling-terminal handle — a dup of the *caller's* fd 0 (the shell's keyboard source:
// ConsoleIn on the console, the compositor's key pipe under the GUI). An interactive
// program (e.g. a pager) reads fd 3 for keys even when its fd 0 is a redirected data pipe.
pub const STDIO_CTTY: u64 = 1 << 48;

// Note: SYS_CONSOLE_SIZE is also the terminal-size get/set call. arg0 == 0 gets the
// caller's terminal size; arg0 != 0 sets it (cols = arg0 & 0xFFFF, rows = arg0 >> 16,
// target pid = arg1, 0 = self), letting a terminal set the size its shell sees.

// Threads (all within the caller's address space):
// - SYS_THREAD_SPAWN(entry, arg, stack_top): start a thread at fn `entry`
//   (extern "C" fn(u64)) with `arg` in rdi and rsp = stack_top (caller-allocated, e.g.
//   via SYS_HEAP_ALLOC). Returns the new tid, or 0 on failure.
// - SYS_THREAD_EXIT(): end the calling thread (the last thread ending exits the process).
// - SYS_THREAD_JOIN(tid): block until thread `tid` has exited (returns immediately if
//   it already has).
// - SYS_THREAD_NEXT(pid, prev_tid): the lowest tid > prev_tid belonging to process `pid`
//   (pass 0 to start), or u64::MAX when there are no more. Lets a task manager list the
//   threads of any process.
// - SYS_THREAD_INFO(tid, buf): write a ThreadInfo { tid, pid, state, cpu_ticks,
//   assigned_cpu } (5 x u64) for `tid` into `buf`. Returns 0, or u64::MAX if no such tid.
