use crate::devfs;
use crate::util::rdtsc;
use crate::util::SyncUnsafeCell;

static WEAK_STATE: SyncUnsafeCell<u64> = SyncUnsafeCell::new(0);

fn has_rdrand() -> bool {
    let ecx: u32;
    unsafe {
        core::arch::asm!(
            "mov {tmp:r}, rbx",
            "cpuid",
            "mov rbx, {tmp:r}",
            inout("eax") 1u32 => _,
            out("ecx") ecx,
            out("edx") _,
            tmp = out(reg) _,
            options(nostack),
        );
    }
    ecx & (1 << 30) != 0
}

fn rdrand64() -> Option<u64> {
    let mut val: u64;
    let mut ok: u8;
    for _ in 0..10 {
        unsafe {
            core::arch::asm!(
                "rdrand {v}",
                "setc {c}",
                v = out(reg) val,
                c = out(reg_byte) ok,
                options(nostack, nomem),
            );
        }
        if ok == 1 {
            return Some(val);
        }
    }
    None
}

fn weak_next() -> u64 {
    let slot = WEAK_STATE.0.get();
    unsafe {
        let mut x = *slot;
        if x == 0 {
            x = rdtsc() | 1;
        }
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *slot = x ^ rdtsc();
        x
    }
}

fn next_u64() -> u64 {
    if has_rdrand() {
        if let Some(v) = rdrand64() {
            return v ^ weak_next();
        }
    }
    weak_next()
}

pub fn fill(buf: &mut [u8]) {
    let mut i = 0;
    while i < buf.len() {
        let bytes = next_u64().to_le_bytes();
        let n = (buf.len() - i).min(8);
        buf[i..i + n].copy_from_slice(&bytes[..n]);
        i += n;
    }
}

fn kazuos_getrandom(buf: &mut [u8]) -> Result<(), getrandom::Error> {
    fill(buf);
    Ok(())
}

getrandom::register_custom_getrandom!(kazuos_getrandom);

fn dev_open() -> u64 {
    0
}

fn dev_close(_handle: u64) {}

fn dev_read(_handle: u64, buf: &mut [u8]) -> usize {
    fill(buf);
    buf.len()
}

fn dev_write(_handle: u64, buf: &[u8]) -> usize {
    buf.len()
}

fn dev_ioctl(_handle: u64, _cmd: u64, _arg: u64) -> i64 {
    0
}

static RANDOM_OPS: devfs::DeviceOps = devfs::DeviceOps {
    open: dev_open,
    close: dev_close,
    read: dev_read,
    write: dev_write,
    ioctl: dev_ioctl,
};

pub fn init() {
    devfs::register("/dev/random", &RANDOM_OPS);
    devfs::register("/dev/urandom", &RANDOM_OPS);
}
