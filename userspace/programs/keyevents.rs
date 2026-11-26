#![no_std]
#![no_main]
include!("../runtime/runtime.rs");

#[repr(C)]
struct FbInfo {
    base: u64,
    width: u32,
    height: u32,
    stride: u32,
    format: u32,
}

fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!("int 0x80", inlateout("rax") n => r, in("rdi") a0, in("rsi") a1, in("rdx") a2);
    }
    r
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let mut info = FbInfo { base: 0, width: 0, height: 0, stride: 0, format: 0 };
    if syscall(SYS_FB_ACQUIRE, &mut info as *mut FbInfo as u64, 0, 0) == u64::MAX {
        println!("keyevents: framebuffer busy");
        sys_exit(1);
    }
    let keyboard_fd = sys_open(b"/dev/input/keyboard");
    if keyboard_fd == u64::MAX {
        println!("keyevents: /dev/input/keyboard not found");
        syscall(SYS_FB_RELEASE, 0, 0, 0);
        sys_exit(1);
    }
    println!("keyevents: waiting for key press and release");
    let mut pressed = false;
    for _ in 0..500 {
        let mut record = [0u8; 2];
        let event = if sys_try_read(keyboard_fd, &mut record) == 2 {
            u16::from_le_bytes(record) as u64
        } else { 0 };
        if event != 0 {
            println!("keyevents: event {:#x}", event);
            if event & 0x100 == 0 { pressed = true; }
            if pressed && event & 0x100 != 0 {
                syscall(SYS_FB_RELEASE, 0, 0, 0);
                sys_exit(0);
            }
        }
        sys_sleep(10);
    }
    syscall(SYS_FB_RELEASE, 0, 0, 0);
    sys_exit(1)
}
