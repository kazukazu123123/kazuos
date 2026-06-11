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

#[global_allocator]
static ALLOC: KazuAlloc = KazuAlloc;

struct KazuAlloc;

unsafe impl core::alloc::GlobalAlloc for KazuAlloc {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        let ptr = sys_heap_alloc(layout.size() as u64);
        if ptr == 0 { return core::ptr::null_mut(); }
        ptr as *mut u8
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: core::alloc::Layout) {
        sys_heap_free(ptr as u64);
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
