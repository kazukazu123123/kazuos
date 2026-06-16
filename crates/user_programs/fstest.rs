#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

// Exercises the writable RAM rootfs end to end: mkdir, create/write,
// open/read-back, create-truncate, unlink, and rmdir (empty vs non-empty).

static mut PASS: u32 = 0;
static mut FAIL: u32 = 0;

fn check(cond: bool, label: &str) {
    unsafe {
        if cond {
            PASS += 1;
            println!("[OK]   {}", label);
        } else {
            FAIL += 1;
            println!("[FAIL] {}", label);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    println!("fstest: RAM rootfs test");

    // ── mkdir ────────────────────────────────────────────────────────────
    check(sys_mkdir(b"/tmp") == 0, "mkdir /tmp");
    check(sys_mkdir(b"/tmp") == u64::MAX, "mkdir /tmp again fails (exists)");
    check(sys_mkdir(b"/nope/sub") == u64::MAX, "mkdir with missing parent fails");

    // ── create + write ───────────────────────────────────────────────────
    let msg: &[u8] = b"Hello, ramfs!";
    let fd = sys_create(b"/tmp/hello.txt");
    check(fd != u64::MAX, "create /tmp/hello.txt");
    check(sys_write_fd(fd, msg) == msg.len() as u64, "write 13 bytes");
    sys_close(fd);

    // ── open + read back ─────────────────────────────────────────────────
    let rfd = sys_open(b"/tmp/hello.txt");
    check(rfd != u64::MAX, "open for read");
    let mut buf = [0u8; 32];
    let n = sys_read(rfd, &mut buf) as usize;
    check(n == msg.len(), "read back 13 bytes");
    check(&buf[..n] == msg, "content matches");
    sys_close(rfd);

    // ── create truncates an existing file ────────────────────────────────
    let fd2 = sys_create(b"/tmp/hello.txt");
    check(sys_write_fd(fd2, b"hi") == 2, "rewrite 2 bytes");
    sys_close(fd2);
    let rfd2 = sys_open(b"/tmp/hello.txt");
    let mut b2 = [0u8; 32];
    let n2 = sys_read(rfd2, &mut b2) as usize;
    check(n2 == 2 && &b2[..2] == b"hi", "create truncated old content");
    sys_close(rfd2);

    // ── unlink ───────────────────────────────────────────────────────────
    check(sys_unlink(b"/tmp/hello.txt") == 0, "unlink file");
    check(sys_open(b"/tmp/hello.txt") == u64::MAX, "open after unlink fails");

    // ── rmdir: non-empty fails, empty succeeds ───────────────────────────
    let f3 = sys_create(b"/tmp/x");
    check(f3 != u64::MAX, "create /tmp/x");
    sys_close(f3);
    check(sys_rmdir(b"/tmp") == u64::MAX, "rmdir non-empty fails");
    check(sys_unlink(b"/tmp/x") == 0, "unlink /tmp/x");
    check(sys_rmdir(b"/tmp") == 0, "rmdir empty ok");
    check(sys_rmdir(b"/") == u64::MAX, "rmdir / fails");

    // ── generation safety: a stale fd must not alias a reused slot ────────
    let gf = sys_create(b"/g1");
    sys_write_fd(gf, b"AAAA");
    sys_close(gf);
    let stale = sys_open(b"/g1"); // fresh read fd at offset 0
    check(sys_unlink(b"/g1") == 0, "unlink /g1 while a fd is open");
    let gf2 = sys_create(b"/g2"); // may reuse /g1's freed slot
    sys_write_fd(gf2, b"BBBB");
    sys_close(gf2);
    let mut gb = [0u8; 8];
    let gn = sys_read(stale, &mut gb) as usize;
    check(gn == 0, "stale fd after unlink reads nothing (no slot aliasing)");
    sys_close(stale);
    let _ = sys_unlink(b"/g2");

    let (pass, fail) = unsafe { (PASS, FAIL) };
    println!("fstest: {} passed, {} failed", pass, fail);
    sys_exit(if fail == 0 { 0 } else { 1 });
}
