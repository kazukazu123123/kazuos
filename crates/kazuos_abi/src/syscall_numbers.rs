// Console / Display
pub const SYS_CONSOLE_WRITE:  u64 = 1;
pub const SYS_CONSOLE_CLEAR:  u64 = 2;
pub const SYS_CURSOR_SAVE:    u64 = 3;
pub const SYS_CURSOR_DRAW:    u64 = 4;
pub const SYS_CURSOR_RESTORE: u64 = 5;
pub const SYS_FB_ACQUIRE:     u64 = 6;
pub const SYS_FB_RELEASE:     u64 = 7;
pub const SYS_CONSOLE_SIZE:   u64 = 8;
pub const SYS_FB_QUERY:       u64 = 9;

// Process / Lifecycle
pub const SYS_EXIT:         u64 = 10;
pub const SYS_EXEC:         u64 = 11;
pub const SYS_KILL:         u64 = 12;
pub const SYS_WAIT:         u64 = 13;
pub const SYS_PROCESS_INFO: u64 = 14;
pub const SYS_PROCESS_NEXT: u64 = 15;
pub const SYS_SLEEP:         u64 = 16;
pub const SLEEP_UNIT_MS:     u64 = 0;
pub const SLEEP_UNIT_US:     u64 = 1;
pub const SLEEP_UNIT_TICK:   u64 = 2;

// Memory
pub const SYS_MEM_INFO:   u64 = 17;
pub const SYS_HEAP_ALLOC: u64 = 18;
pub const SYS_HEAP_FREE:  u64 = 19;

// Signals
pub const SYS_SIGNAL_CATCH: u64 = 20;
pub const SYS_SIGNAL_CHECK: u64 = 21;

// IPC
pub const SYS_IPC_OPEN:     u64 = 22;
pub const SYS_IPC_SEND:     u64 = 23;
pub const SYS_IPC_RECV:     u64 = 24;
pub const SYS_IPC_TRY_RECV: u64 = 25; // non-blocking SYS_IPC_RECV (returns immediately if empty)
pub const SYS_IPC_CLOSE:    u64 = 26;

// File I/O
pub const SYS_OPEN:  u64 = 27;
pub const SYS_CLOSE: u64 = 28;
pub const SYS_READ:  u64 = 29;
pub const SYS_WRITE: u64 = 30;
pub const SYS_IOCTL: u64 = 31;
pub const SYS_PIPE:  u64 = 32;

// Hardware / Driver
pub const SYS_PCI_INFO:       u64 = 33;
pub const SYS_IOPORT_REQUEST: u64 = 34;
pub const SYS_IRQ_WAIT:       u64 = 35;
pub const SYS_DMA_ALLOC:      u64 = 36;
pub const SYS_DMA_FREE:       u64 = 37;

// PCI
pub const SYS_PCI_BAR_MAP:   u64 = 38;
pub const SYS_PCI_BAR_UNMAP: u64 = 39;

// Keyboard
pub const SYS_KEYBOARD_READ: u64 = 40;
pub const SYS_KEYBOARD_POLL: u64 = 41;

// System / Misc
pub const SYS_CPU_INFO:  u64 = 42;
pub const SYS_SHUTDOWN:  u64 = 43;
pub const SYS_REBOOT:    u64 = 44;
pub const SYS_LS:        u64 = 45;

// Kernel modules
pub const SYS_MODULE_LOAD:   u64 = 46;
pub const SYS_MODULE_UNLOAD: u64 = 47;
pub const SYS_MODULE_LIST:   u64 = 48;
pub const SYS_MODULE_INFO:   u64 = 49;

// Filesystem mutations (RAM rootfs)
pub const SYS_CREATE: u64 = 50; // create an empty file, returns fd (RW)
pub const SYS_UNLINK: u64 = 51; // delete a file
pub const SYS_MKDIR:  u64 = 52; // create a directory
pub const SYS_RMDIR:  u64 = 53; // remove an empty directory
