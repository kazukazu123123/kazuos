use crate::util::SyncUnsafeCell;

pub struct DeviceOps {
    pub open: fn() -> u64,
    pub close: fn(handle: u64),
    pub read: fn(handle: u64, buf: &mut [u8]) -> usize,
    pub write: fn(handle: u64, buf: &[u8]) -> usize,
    pub ioctl: fn(handle: u64, cmd: u64, arg: u64) -> i64,
}

struct Entry {
    name: [u8; 32],
    name_len: usize,
    ops: &'static DeviceOps,
}

const MAX_DEV: usize = 16;
static REGISTRY: SyncUnsafeCell<[Option<Entry>; MAX_DEV]> =
    SyncUnsafeCell::new([const { None }; MAX_DEV]);

pub fn register(name: &str, ops: &'static DeviceOps) {
    if name.len() > 32 {
        return;
    }
    unsafe {
        let reg = &mut *REGISTRY.0.get();
        for slot in reg.iter_mut() {
            if slot.is_none() {
                let mut n = [0u8; 32];
                n[..name.len()].copy_from_slice(name.as_bytes());
                *slot = Some(Entry {
                    name: n,
                    name_len: name.len(),
                    ops,
                });
                return;
            }
        }
    }
}

pub fn for_each(mut f: impl FnMut(&str)) {
    unsafe {
        let reg = &*REGISTRY.0.get();
        for slot in reg.iter() {
            if let Some(entry) = slot {
                if let Ok(name) = core::str::from_utf8(&entry.name[..entry.name_len]) {
                    f(name);
                }
            }
        }
    }
}

pub fn lookup(name: &str) -> Option<&'static DeviceOps> {
    unsafe {
        let reg = &*REGISTRY.0.get();
        for slot in reg.iter() {
            if let Some(entry) = slot {
                if entry.name_len == name.len()
                    && entry.name[..entry.name_len] == *name.as_bytes()
                {
                    return Some(entry.ops);
                }
            }
        }
    }
    None
}
