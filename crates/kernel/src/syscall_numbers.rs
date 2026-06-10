// Console / Display
pub const SYS_CONSOLE_WRITE: u64 = 1;
pub const SYS_CONSOLE_CLEAR: u64 = 2;
pub const SYS_CURSOR_SAVE:   u64 = 3;
pub const SYS_CURSOR_DRAW:   u64 = 4;
pub const SYS_FB_ACQUIRE:    u64 = 5;
pub const SYS_FB_RELEASE:    u64 = 6;
pub const SYS_CONSOLE_SIZE:  u64 = 7;
pub const SYS_FB_QUERY:      u64 = 8;

// Process / Lifecycle
pub const SYS_EXIT:         u64 = 9;
pub const SYS_EXEC:         u64 = 10;
pub const SYS_KILL:         u64 = 11;
pub const SYS_WAIT:         u64 = 12;
pub const SYS_PROCESS_INFO: u64 = 13;
pub const SYS_PROCESS_NEXT: u64 = 14;
pub const SYS_SLEEP:         u64 = 15;
pub const SLEEP_UNIT_MS:     u64 = 0;
pub const SLEEP_UNIT_US:     u64 = 1;

// Memory
pub const SYS_MEM_INFO:   u64 = 16;
pub const SYS_HEAP_ALLOC: u64 = 17;
pub const SYS_HEAP_FREE:  u64 = 18;

// Signals
pub const SYS_SIGNAL_CATCH: u64 = 19;
pub const SYS_SIGNAL_CHECK: u64 = 20;

// IPC
pub const SYS_IPC_OPEN:  u64 = 21;
pub const SYS_IPC_SEND:  u64 = 22;
pub const SYS_IPC_RECV:  u64 = 23;
pub const SYS_IPC_CLOSE: u64 = 24;

// File I/O
pub const SYS_OPEN:  u64 = 25;
pub const SYS_CLOSE: u64 = 26;
pub const SYS_READ:  u64 = 27;
pub const SYS_WRITE: u64 = 28;
pub const SYS_IOCTL: u64 = 29;
pub const SYS_PIPE:  u64 = 30;

// Hardware / Driver
pub const SYS_PCI_INFO:       u64 = 31;
pub const SYS_IOPORT_REQUEST: u64 = 32;
pub const SYS_IRQ_WAIT:       u64 = 33;
pub const SYS_DMA_ALLOC:      u64 = 34;
pub const SYS_DMA_FREE:       u64 = 35;

// Keyboard / Mouse
pub const SYS_KEYBOARD_READ: u64 = 36;
pub const SYS_KEYBOARD_POLL: u64 = 37;
pub const SYS_MOUSE_READ:    u64 = 38;
pub const SYS_MOUSE_POLL:    u64 = 39;

// System / Misc
pub const SYS_CPU_INFO:  u64 = 40;
pub const SYS_SHUTDOWN:  u64 = 41;
pub const SYS_REBOOT:    u64 = 42;
pub const SYS_LS:        u64 = 43;
