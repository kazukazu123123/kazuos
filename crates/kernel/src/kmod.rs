use crate::util::SyncUnsafeCell;

const MAX_MODULES: usize = 16;
const MAX_NAME: usize = 32;

/// Size of a serialized entry in the SYS_MODULE_LIST/INFO buffer (bytes).
pub const ENTRY_SIZE: usize = 48;

#[derive(Clone, Copy, PartialEq)]
pub enum ModuleStatus {
    Running   = 0,
    Unloading = 1,
    Failed    = 2,
}

#[derive(Clone, Copy)]
struct ModuleEntry {
    id:       u32,
    name:     [u8; MAX_NAME],
    name_len: u8,
    pid:      u64,
    status:   ModuleStatus,
}

struct ModuleTable {
    entries: [Option<ModuleEntry>; MAX_MODULES],
    next_id: u32,
}

static TABLE: SyncUnsafeCell<ModuleTable> = SyncUnsafeCell::new(ModuleTable {
    entries: [None; MAX_MODULES],
    next_id: 1,
});

fn table() -> &'static mut ModuleTable {
    unsafe { &mut *TABLE.0.get() }
}

pub fn load(path: &str) -> u64 {
    let pid = crate::exec::spawn_module(path);
    if pid == 0 {
        return u64::MAX;
    }
    let name = extract_name(path);
    let t = table();
    for slot in t.entries.iter_mut() {
        if slot.is_none() {
            let id = t.next_id;
            t.next_id += 1;
            let mut entry = ModuleEntry {
                id,
                name: [0; MAX_NAME],
                name_len: name.len().min(MAX_NAME) as u8,
                pid,
                status: ModuleStatus::Running,
            };
            let len = name.len().min(MAX_NAME);
            entry.name[..len].copy_from_slice(name[..len].as_bytes());
            *slot = Some(entry);
            crate::logln!("kmod: loaded '{}' pid={} id={}", name, pid, id);
            return id as u64;
        }
    }
    crate::process::kill_pid(pid);
    u64::MAX
}

pub fn unload(id: u32) -> bool {
    let t = table();
    for slot in t.entries.iter_mut() {
        if let Some(e) = slot {
            if e.id == id {
                e.status = ModuleStatus::Unloading;
                crate::process::send_module_exit(e.pid);
                crate::logln!("kmod: unloading id={} pid={}", id, e.pid);
                return true;
            }
        }
    }
    false
}

pub fn on_process_exit(pid: u64) {
    let t = table();
    for slot in t.entries.iter_mut() {
        if let Some(e) = slot {
            if e.pid == pid {
                crate::logln!(
                    "kmod: module '{}' exited",
                    core::str::from_utf8(&e.name[..e.name_len as usize]).unwrap_or("?")
                );
                *slot = None;
                return;
            }
        }
    }
}

/// Write list entries into a user-space buffer. Each entry is ENTRY_SIZE bytes.
/// Returns count of entries written.
pub fn list(buf_ptr: u64, buf_len: u64) -> u64 {
    let max = (buf_len as usize) / ENTRY_SIZE;
    let t = table();
    let mut count = 0usize;
    for slot in t.entries.iter() {
        if count >= max { break; }
        if let Some(e) = slot {
            write_entry(buf_ptr + (count * ENTRY_SIZE) as u64, e);
            count += 1;
        }
    }
    count as u64
}

/// Write a single entry into a user-space buffer. Returns 0 on success, u64::MAX if not found.
pub fn info(id: u32, buf_ptr: u64) -> u64 {
    let t = table();
    for slot in t.entries.iter() {
        if let Some(e) = slot {
            if e.id == id {
                write_entry(buf_ptr, e);
                return 0;
            }
        }
    }
    u64::MAX
}

pub fn load_from_list(path: &str) {
    let data = match crate::vfs::read_file(path) {
        Ok(d) => d,
        Err(_) => return,
    };
    let text = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => return,
    };
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with('#') {
            let result = load(line);
            if result == u64::MAX {
                crate::logln!("kmod: failed to load '{}'", line);
            }
        }
    }
}

fn write_entry(buf_ptr: u64, e: &ModuleEntry) {
    let ptr = buf_ptr as *mut u8;
    unsafe {
        // offset  0: id (u32)
        (ptr as *mut u32).write_unaligned(e.id);
        // offset  4: pid (u32)
        (ptr.add(4) as *mut u32).write_unaligned(e.pid as u32);
        // offset  8: status (u32)
        (ptr.add(8) as *mut u32).write_unaligned(e.status as u32);
        // offset 12: name (32 bytes)
        core::ptr::copy_nonoverlapping(e.name.as_ptr(), ptr.add(12), MAX_NAME);
        // offset 44: name_len (u32)
        (ptr.add(44) as *mut u32).write_unaligned(e.name_len as u32);
    }
}

fn extract_name(path: &str) -> &str {
    let base = path.rfind('/').map(|i| &path[i + 1..]).unwrap_or(path);
    base.rfind('.').map(|i| &base[..i]).unwrap_or(base)
}
