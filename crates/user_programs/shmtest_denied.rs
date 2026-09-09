#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

fn fail(message: &str) -> ! {
    println!("shmtest_denied: FAIL: {}", message);
    sys_exit(1)
}

fn first_arg(argc: u64, argv: u64) -> Option<&'static [u8]> {
    if argc == 0 || argv == 0 {
        return None;
    }
    let pointer = unsafe { *(argv as *const u64) };
    if pointer == 0 {
        return None;
    }
    let mut length = 0usize;
    while unsafe { *((pointer as *const u8).add(length)) } != 0 {
        length += 1;
    }
    Some(unsafe { core::slice::from_raw_parts(pointer as *const u8, length) })
}

fn parse_u64(bytes: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for &byte in bytes {
        if !byte.is_ascii_digit() {
            return None;
        }
        value = value.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
    }
    Some(value)
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    let Some(id) = first_arg(argc, argv).and_then(parse_u64) else {
        fail("argument");
    };
    if sys_shm_map(id) != u64::MAX || sys_shm_close(id) != u64::MAX {
        fail("unauthorized access");
    }
    if sys_write_fd(1, b"denied") != 6 {
        fail("send result");
    }
    sys_exit(0)
}
