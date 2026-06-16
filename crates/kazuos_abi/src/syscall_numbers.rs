// Console / Display
pub const SYS_CONSOLE_WRITE:  u64 = 1;
pub const SYS_CURSOR_SAVE:    u64 = 2;
pub const SYS_CURSOR_DRAW:    u64 = 3;
pub const SYS_CURSOR_RESTORE: u64 = 4;
pub const SYS_FB_ACQUIRE:     u64 = 5;
pub const SYS_FB_RELEASE:     u64 = 6;
pub const SYS_CONSOLE_SIZE:   u64 = 7;

// Process / Lifecycle
pub const SYS_EXIT:         u64 = 8;
pub const SYS_EXEC:         u64 = 9;
pub const SYS_KILL:         u64 = 10;
pub const SYS_WAIT:         u64 = 11;
pub const SYS_PROCESS_INFO: u64 = 12;
pub const SYS_PROCESS_NEXT: u64 = 13;
pub const SYS_SLEEP:         u64 = 14;
pub const SLEEP_UNIT_MS:     u64 = 0;
pub const SLEEP_UNIT_US:     u64 = 1;
pub const SLEEP_UNIT_TICK:   u64 = 2;

// Memory
pub const SYS_MEM_INFO:   u64 = 15;
pub const SYS_HEAP_ALLOC: u64 = 16;
pub const SYS_HEAP_FREE:  u64 = 17;

// Signals
pub const SYS_SIGNAL_CATCH: u64 = 18;
pub const SYS_SIGNAL_CHECK: u64 = 19;

// IPC
pub const SYS_IPC_OPEN:     u64 = 20;
pub const SYS_IPC_SEND:     u64 = 21;
pub const SYS_IPC_RECV:     u64 = 22;
pub const SYS_IPC_TRY_RECV: u64 = 23; // non-blocking SYS_IPC_RECV (returns immediately if empty)
pub const SYS_IPC_CLOSE:    u64 = 24;

// File I/O
pub const SYS_OPEN:     u64 = 25;
pub const SYS_CLOSE:    u64 = 26;
pub const SYS_READ:     u64 = 27;
pub const SYS_TRY_READ: u64 = 28; // non-blocking SYS_READ (0 = would block, u64::MAX = EOF)
pub const SYS_WRITE:    u64 = 29;
pub const SYS_IOCTL:    u64 = 30;
pub const SYS_PIPE:     u64 = 31;

// Hardware / Driver
pub const SYS_PCI_INFO:       u64 = 32;
pub const SYS_IOPORT_REQUEST: u64 = 33;
pub const SYS_IRQ_WAIT:       u64 = 34;
pub const SYS_DMA_ALLOC:      u64 = 35;
pub const SYS_DMA_FREE:       u64 = 36;

// PCI
pub const SYS_PCI_BAR_MAP:   u64 = 37;
pub const SYS_PCI_BAR_UNMAP: u64 = 38;

// Keyboard
pub const SYS_KEYBOARD_POLL: u64 = 39;

// System / Misc
pub const SYS_CPU_INFO:  u64 = 40;
pub const SYS_SHUTDOWN:  u64 = 41;
pub const SYS_REBOOT:    u64 = 42;
pub const SYS_READDIR:   u64 = 43; // enumerate a directory into a caller buffer (no kernel-side printing)

// Kernel modules
pub const SYS_MODULE_LOAD:   u64 = 44;
pub const SYS_MODULE_UNLOAD: u64 = 45;
pub const SYS_MODULE_LIST:   u64 = 46;
pub const SYS_MODULE_INFO:   u64 = 47;

// Filesystem mutations (RAM rootfs)
pub const SYS_CREATE: u64 = 48; // create an empty file, returns fd (RW)
pub const SYS_UNLINK: u64 = 49; // delete a file
pub const SYS_MKDIR:  u64 = 50; // create a directory
pub const SYS_RMDIR:  u64 = 51; // remove an empty directory

// Send SIGINT to the foreground (wait-chain leaf) of arg0's process. Returns 1 if a
// descendant was signaled, 0 if arg0 is itself the leaf (idle at its own prompt).
// Lets a terminal interrupt the command its shell is currently waiting on.
pub const SYS_SIGINT_FG: u64 = 52;
