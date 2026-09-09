#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

fn fail(message: &str) -> ! {
    println!("shmtest_child: FAIL: {}", message);
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
    let address = loop {
        let address = sys_shm_map(id);
        if address != u64::MAX {
            break address;
        }
    };
    let valid = unsafe {
        (address as *const u64).read_volatile() == 0x1122_3344_5566_7788
            && ((address + 4092) as *const u64).read_unaligned() == 0x8877_6655_4433_2211
    };
    if !valid {
        fail("contents");
    }
    unsafe {
        (address as *mut u64).write_volatile(0xaabb_ccdd_eeff_0011);
    }
    if sys_write_fd(1, b"ready") != 5 {
        fail("send ready");
    }
    while unsafe { ((address + 16) as *const u64).read_volatile() } == 0 {
        core::hint::spin_loop();
    }
    if unsafe { (address as *const u64).read_volatile() } != 0xaabb_ccdd_eeff_0011 {
        fail("owner close revoked mapping");
    }
    if sys_write_fd(1, b"done") != 4 {
        fail("send done");
    }
    sys_exit(0)
}
