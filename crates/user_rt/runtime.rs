extern crate alloc;

use core::fmt::Write;

include!("../kernel/src/syscall_numbers.rs");

pub struct KazuWriter;

impl Write for KazuWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        sys_write_raw(s.as_bytes());
        Ok(())
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!(KazuWriter, $($arg)*);
    }};
}

#[macro_export]
macro_rules! println {
    () => { sys_write_raw(b"\r\n") };
    ($($arg:tt)*) => {{
        use core::fmt::Write;
        let _ = write!(KazuWriter, $($arg)*);
        sys_write_raw(b"\r\n");
    }};
}

#[unsafe(no_mangle)]
pub extern "C" fn _start(argc: u64, argv: u64) -> ! {
    unsafe extern "C" {
        fn user_main(argc: u64, argv: u64) -> !;
    }
    unsafe { user_main(argc, argv) }
}

#[panic_handler]
fn panic_handler(info: &core::panic::PanicInfo) -> ! {
    print!("[PANIC] ");
    let _ = write!(KazuWriter, "{}", info.message());
    print!("\r\n");
    sys_exit(1)
}

// ---------------------------------------------------------------------------
// User-space heap: a first-fit free-list allocator backed by page "arenas" from
// SYS_HEAP_ALLOC. Sub-page allocations are carved out of arenas and reused on
// free (adjacent free blocks within an arena are coalesced), instead of the old
// behaviour of one syscall + a whole page per allocation. Single process thread,
// but a tiny spinlock keeps it correct if that ever changes.
// ---------------------------------------------------------------------------

#[repr(C)]
struct FreeNode {
    size: usize,
    next: *mut FreeNode,
}

#[repr(C)]
struct AllocHeader {
    block_addr: usize,
    block_size: usize,
}

/// Max number of arenas tracked for release back to the kernel.
const MAX_ARENAS: usize = 1024;

struct Heap {
    head: core::cell::UnsafeCell<*mut FreeNode>,
    /// (addr, size) of each arena obtained from SYS_HEAP_ALLOC; addr==0 = empty.
    arenas: core::cell::UnsafeCell<[(usize, usize); MAX_ARENAS]>,
    lock: core::sync::atomic::AtomicBool,
}
unsafe impl Sync for Heap {}

/// How much to request from the kernel when the free list runs dry.
const ARENA_SIZE: usize = 128 * 1024;

#[inline]
fn align_up(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

impl Heap {
    const fn new() -> Self {
        Heap {
            head: core::cell::UnsafeCell::new(core::ptr::null_mut()),
            arenas: core::cell::UnsafeCell::new([(0usize, 0usize); MAX_ARENAS]),
            lock: core::sync::atomic::AtomicBool::new(false),
        }
    }

    fn acquire(&self) {
        while self.lock.swap(true, core::sync::atomic::Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }
    fn release(&self) {
        self.lock.store(false, core::sync::atomic::Ordering::Release);
    }

    /// Insert [addr, addr+size) into the address-sorted free list, coalescing
    /// with adjacent free blocks.
    unsafe fn insert(&self, addr: usize, size: usize) {
        let head = self.head.get();
        let mut prev: *mut FreeNode = core::ptr::null_mut();
        let mut cur = *head;
        while !cur.is_null() && (cur as usize) < addr {
            prev = cur;
            cur = (*cur).next;
        }
        let node = addr as *mut FreeNode;
        (*node).size = size;
        (*node).next = cur;
        if prev.is_null() {
            *head = node;
        } else {
            (*prev).next = node;
        }
        // Merge with the following block if contiguous.
        if !cur.is_null() && addr + size == cur as usize {
            (*node).size += (*cur).size;
            (*node).next = (*cur).next;
        }
        // Merge with the preceding block if contiguous.
        if !prev.is_null() && (prev as usize) + (*prev).size == addr {
            (*prev).size += (*node).size;
            (*prev).next = (*node).next;
        }
    }

    /// Request a fresh arena from the kernel and add it to the free list.
    unsafe fn grow(&self, min: usize) -> bool {
        let want = align_up(if min > ARENA_SIZE { min } else { ARENA_SIZE }, 4096);
        let p = sys_heap_alloc(want as u64);
        if p == 0 || p == u64::MAX {
            return false;
        }
        // Record the arena so a fully-freed one can be returned to the kernel.
        let arenas = &mut *self.arenas.get();
        for slot in arenas.iter_mut() {
            if slot.0 == 0 {
                *slot = (p as usize, want);
                break;
            }
        }
        self.insert(p as usize, want);
        true
    }

    /// Return any arena that is now entirely free back to the kernel, so the
    /// process's memory footprint actually shrinks after frees.
    unsafe fn release_empty_arenas(&self) {
        let head = self.head.get();
        let arenas = &mut *self.arenas.get();
        for slot in arenas.iter_mut() {
            if slot.0 == 0 {
                continue;
            }
            let (a_addr, a_size) = *slot;
            // Find a free node that exactly covers this arena.
            let mut prev: *mut FreeNode = core::ptr::null_mut();
            let mut cur = *head;
            while !cur.is_null() {
                if cur as usize == a_addr && (*cur).size == a_size {
                    if prev.is_null() { *head = (*cur).next; } else { (*prev).next = (*cur).next; }
                    sys_heap_free(a_addr as u64);
                    *slot = (0, 0);
                    break;
                }
                prev = cur;
                cur = (*cur).next;
            }
        }
    }

    /// First-fit search; splits the remainder back into the free list and writes
    /// an AllocHeader just before the returned (aligned) pointer.
    unsafe fn take(&self, need: usize, header: usize, align: usize) -> *mut u8 {
        let head = self.head.get();
        let mut prev: *mut FreeNode = core::ptr::null_mut();
        let mut cur = *head;
        while !cur.is_null() {
            if (*cur).size >= need {
                let addr = cur as usize;
                let total = (*cur).size;
                let next = (*cur).next;
                if prev.is_null() { *head = next; } else { (*prev).next = next; }

                let remainder = total - need;
                let kept = if remainder >= core::mem::size_of::<FreeNode>() {
                    self.insert(addr + need, remainder);
                    need
                } else {
                    total
                };

                let user = align_up(addr + header, align);
                let hdr = (user - header) as *mut AllocHeader;
                (*hdr).block_addr = addr;
                (*hdr).block_size = kept;
                return user as *mut u8;
            }
            prev = cur;
            cur = (*cur).next;
        }
        core::ptr::null_mut()
    }
}

#[global_allocator]
static ALLOC: Heap = Heap::new();

unsafe impl core::alloc::GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let header = core::mem::size_of::<AllocHeader>();
        let align = layout
            .align()
            .max(core::mem::align_of::<AllocHeader>())
            .max(core::mem::align_of::<FreeNode>());
        // Worst case: a block can start at any address, so reserve room to align
        // up plus the header.
        let need = align_up(header + layout.size() + align, core::mem::align_of::<FreeNode>());

        self.acquire();
        let mut ptr = self.take(need, header, align);
        if ptr.is_null() && self.grow(need) {
            ptr = self.take(need, header, align);
        }
        self.release();
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: core::alloc::Layout) {
        let header = core::mem::size_of::<AllocHeader>();
        let hdr = (ptr as usize - header) as *const AllocHeader;
        let addr = (*hdr).block_addr;
        let size = (*hdr).block_size;
        self.acquire();
        self.insert(addr, size);
        self.release_empty_arenas();
        self.release();
    }
}

fn sys_write_raw(buf: &[u8]) {
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") SYS_CONSOLE_WRITE,
            in("rdi") buf.as_ptr(),
            in("rsi") buf.len(),
            in("rdx") 0,
            lateout("rax") _,
        );
    }
}

pub fn sys_exit(code: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") SYS_EXIT,
            in("rdi") code,
            in("rsi") 0,
            in("rdx") 0,
            lateout("rax") _,
        );
    }
    loop {}
}

pub fn sys_open(path: &[u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_OPEN => r,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_close(fd: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_CLOSE => r,
            in("rdi") fd,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_read(fd: u64, buf: &mut [u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_READ => r,
            in("rdi") fd,
            in("rsi") buf.as_mut_ptr(),
            in("rdx") buf.len(),
        );
    }
    r
}

pub fn sys_write_fd(fd: u64, buf: &[u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_WRITE => r,
            in("rdi") fd,
            in("rsi") buf.as_ptr(),
            in("rdx") buf.len(),
        );
    }
    r
}

/// Create (or truncate) a file and return a writable fd, or u64::MAX on error.
pub fn sys_create(path: &[u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_CREATE => r,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") 0,
        );
    }
    r
}

/// Delete a file. Returns 0 on success, u64::MAX on error.
pub fn sys_unlink(path: &[u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_UNLINK => r,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") 0,
        );
    }
    r
}

/// Create a directory. Returns 0 on success, u64::MAX on error.
pub fn sys_mkdir(path: &[u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_MKDIR => r,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") 0,
        );
    }
    r
}

/// Remove an empty directory. Returns 0 on success, u64::MAX on error.
pub fn sys_rmdir(path: &[u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_RMDIR => r,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_exec(path: &[u8], stdio_pack: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_EXEC => r,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") stdio_pack,
        );
    }
    r
}

pub fn sys_heap_alloc(size: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_HEAP_ALLOC => r,
            in("rdi") size,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_heap_free(ptr: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_HEAP_FREE => r,
            in("rdi") ptr,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_sleep(ms: u64) {
    unsafe {
        core::arch::asm!(
            "int 0x80",
            in("rax") SYS_SLEEP,
            in("rdi") ms,
            in("rsi") SLEEP_UNIT_MS,
            in("rdx") 0,
            lateout("rax") _,
        );
    }
}

pub fn sys_proc_info(pid: u64, out: *mut u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_PROCESS_INFO => r,
            in("rdi") pid,
            in("rsi") out,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_proc_next(prev: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_PROCESS_NEXT => r,
            in("rdi") prev,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_cpu_info(sel: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_CPU_INFO => r,
            in("rdi") sel,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_mem_info() -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_MEM_INFO => r,
            in("rdi") 0,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_kill(pid: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_KILL => r,
            in("rdi") pid,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_wait(pid: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_WAIT => r,
            in("rdi") pid,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_signal_catch(sig: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_SIGNAL_CATCH => r,
            in("rdi") sig,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}

pub fn sys_signal_check() -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_SIGNAL_CHECK => r,
            in("rdi") 0,
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    r
}
