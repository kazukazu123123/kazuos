#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

fn fail(message: &str) -> ! {
    println!("shmtest: FAIL: {}", message);
    sys_exit(1)
}

fn pipe() -> [u64; 2] {
    let mut fds = [0u64; 2];
    let result: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_PIPE => result,
            in("rdi") fds.as_mut_ptr(),
            in("rsi") 0,
            in("rdx") 0,
        );
    }
    if result != 0 {
        fail("pipe");
    }
    fds
}

fn recv(fd: u64, buffer: &mut [u8]) -> u64 {
    loop {
        let size = sys_read(fd, buffer);
        if size != 0 {
            return size;
        }
    }
}

fn format_u64(mut value: u64, buffer: &mut [u8; 20]) -> &[u8] {
    let mut index = buffer.len();
    loop {
        index -= 1;
        buffer[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            return &buffer[index..];
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    if sys_shm_create(0) != u64::MAX || sys_shm_create(128 * 1024 * 1024 + 1) != u64::MAX {
        fail("invalid size accepted");
    }
    let id = sys_shm_create(8192);
    if id == u64::MAX {
        fail("create");
    }
    if sys_shm_grant(id, u64::MAX) != u64::MAX {
        fail("invalid target accepted");
    }

    let address = sys_shm_map(id);
    if address < 0x0000_00B0_0000_0000 || address >= 0x0000_00C0_0000_0000 {
        fail("map range");
    }
    if sys_shm_map(id) != address {
        fail("duplicate map");
    }
    unsafe {
        if (address as *const u64).read_volatile() != 0
            || ((address + 4096) as *const u64).read_volatile() != 0
        {
            fail("not zeroed");
        }
        (address as *mut u64).write_volatile(0x1122_3344_5566_7788);
        ((address + 4092) as *mut u64).write_unaligned(0x8877_6655_4433_2211);
        ((address + 16) as *mut u64).write_volatile(0);
    }
    if sys_shm_unmap(id) != 0 {
        fail("unmap");
    }
    let address = sys_shm_map(id);
    if address == u64::MAX
        || unsafe { ((address + 4092) as *const u64).read_unaligned() } != 0x8877_6655_4433_2211
    {
        fail("remap contents");
    }

    let output = pipe();
    let mut id_buffer = [0u8; 20];
    let id_arg = format_u64(id, &mut id_buffer);
    let stdio = 0xffff | (output[1] << 16);
    let child = sys_exec_with(b"/bin/shmtest_child.kxe", &[id_arg], stdio);
    if child == 0 || child == u64::MAX {
        fail("spawn child");
    }
    let _ = sys_close(output[1]);
    if sys_shm_grant(id, child) != 0 {
        fail("grant");
    }

    let mut message = [0u8; 8];
    if recv(output[0], &mut message) != 5 || &message[..5] != b"ready" {
        fail("child map");
    }
    if unsafe { (address as *const u64).read_volatile() } != 0xaabb_ccdd_eeff_0011 {
        fail("child write");
    }
    unsafe {
        ((address + 16) as *mut u64).write_volatile(1);
    }
    if sys_shm_close(id) != 0 {
        fail("owner close");
    }
    if recv(output[0], &mut message) != 4 || &message[..4] != b"done" {
        fail("child retained access");
    }
    let _ = sys_close(output[0]);
    let _ = sys_wait(child);
    if sys_shm_map(id) != u64::MAX || sys_shm_close(id) != u64::MAX {
        fail("closed holder retained access");
    }

    let denied_id = sys_shm_create(4096);
    if denied_id == u64::MAX {
        fail("create denied object");
    }
    let denied_output = pipe();
    let mut denied_buffer = [0u8; 20];
    let denied_arg = format_u64(denied_id, &mut denied_buffer);
    let denied_stdio = 0xffff | (denied_output[1] << 16);
    let denied_child = sys_exec_with(b"/bin/shmtest_denied.kxe", &[denied_arg], denied_stdio);
    if denied_child == 0 || denied_child == u64::MAX {
        fail("spawn denied child");
    }
    let _ = sys_close(denied_output[1]);
    if recv(denied_output[0], &mut message) != 6 || &message[..6] != b"denied" {
        fail("unauthorized map accepted");
    }
    let _ = sys_close(denied_output[0]);
    let _ = sys_wait(denied_child);

    println!("shmtest: PASS");
    sys_exit(0)
}
