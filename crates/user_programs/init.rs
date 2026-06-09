#![no_std]
#![no_main]

include!("../../crates/kernel/src/syscall_numbers.rs");

const SHELL_PATH: &[u8] = b"/bin/shell.kxe";

#[no_mangle]
pub extern "C" fn _start() -> ! {
    loop {
        let pid = syscall(SYS_EXEC, SHELL_PATH.as_ptr() as u64, SHELL_PATH.len() as u64, 0);
        if pid == 0 || pid == u64::MAX {
            syscall(SYS_NAP_MS, 1000, 0, 0);
            continue;
        }
        loop {
            let r = syscall(SYS_WAIT, pid, 0, 0);
            if r != 0 {
                break;
            }
            syscall(SYS_NAP_MS, 100, 0, 0);
        }
    }
}

fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
        );
    }
    r
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}
