#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

const SHELL_PATH: &[u8] = b"/bin/shell.kxe\0";

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    loop {
        // 0xFFFF_FFFF = default stdio: the shell gets fd0=ConsoleIn, fd1=ConsoleOut.
        // (Passing 0 means "use init's fd 0 for both", leaving the shell writing fd 1
        // into the wrong place once it routes output through stdout.)
        let pid = syscall(SYS_EXEC, SHELL_PATH.as_ptr() as u64, SHELL_PATH.len() as u64, 0xFFFF_FFFF);
        if pid == 0 || pid == u64::MAX {
            syscall(SYS_SLEEP, 1000, SLEEP_UNIT_MS, 0);
            continue;
        }
        loop {
            let r = syscall(SYS_WAIT, pid, 0, 0);
            if r != 0 {
                break;
            }
            syscall(SYS_SLEEP, 100, SLEEP_UNIT_MS, 0);
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
