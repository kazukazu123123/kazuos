use alloc::vec::Vec;
use crate::util::SyncUnsafeCell;
use crate::devfs::DeviceOps;

// The fd table grows on demand (see alloc_fd / alloc_fd_at), so a process opens as
// many fds as it needs — a compositor like the GUI holds ~2 per child terminal. This
// is only a safety ceiling so a runaway/hostile program can't exhaust kernel memory.
pub const MAX_FD: usize = 1024;

#[derive(Clone, Copy)]
pub enum FdEntry {
    Empty,
    File { node: u32, generation: u32, offset: usize },
    Device { ops: &'static DeviceOps, handle: u64 },
    ConsoleOut,
    ConsoleIn,
    PipeRead(u64),
    PipeWrite(u64),
}

#[derive(Clone)]
pub struct FdTable {
    // Grows as fds are allocated; freed fds become Empty holes that are reused.
    pub entries: Vec<FdEntry>,
}

impl FdTable {
    pub const fn new() -> Self {
        Self { entries: Vec::new() }
    }
}

static TABLES: SyncUnsafeCell<Vec<Option<FdTable>>> = SyncUnsafeCell::new(Vec::new());

pub fn ensure_table(pid: u64) {
    unsafe {
        let tables = &mut *TABLES.0.get();
        let idx = pid as usize;
        if idx >= tables.len() {
            tables.resize(idx + 1, None);
        }
        if tables[idx].is_none() {
            tables[idx] = Some(FdTable::new());
        }
    }
}

pub fn alloc_fd_at(pid: u64, fd: usize, entry: FdEntry) -> bool {
    if fd >= MAX_FD {
        return false;
    }
    ensure_table(pid);
    unsafe {
        let tables = &mut *TABLES.0.get();
        if let Some(Some(table)) = tables.get_mut(pid as usize) {
            if fd >= table.entries.len() {
                table.entries.resize(fd + 1, FdEntry::Empty);
            }
            pipe_clone(&entry);
            table.entries[fd] = entry;
            return true;
        }
    }
    false
}

pub fn alloc_fd(pid: u64, entry: FdEntry) -> Option<usize> {
    ensure_table(pid);
    unsafe {
        let tables = &mut *TABLES.0.get();
        let table = tables[pid as usize].as_mut()?;
        // Reuse the lowest freed slot first.
        for i in 0..table.entries.len() {
            if matches!(table.entries[i], FdEntry::Empty) {
                pipe_clone(&entry);
                table.entries[i] = entry;
                return Some(i);
            }
        }
        // Otherwise grow the table, up to the safety ceiling.
        if table.entries.len() < MAX_FD {
            let i = table.entries.len();
            pipe_clone(&entry);
            table.entries.push(entry);
            return Some(i);
        }
    }
    None
}

pub fn get_fd(pid: u64, fd: usize) -> Option<FdEntry> {
    unsafe {
        let tables = &*TABLES.0.get();
        let table = tables.get(pid as usize)?.as_ref()?;
        table.entries.get(fd).copied()
    }
}

pub fn set_fd(pid: u64, fd: usize, entry: FdEntry) -> bool {
    unsafe {
        let tables = &mut *TABLES.0.get();
        if let Some(Some(table)) = tables.get_mut(pid as usize) {
            if fd < table.entries.len() {
                table.entries[fd] = entry;
                return true;
            }
        }
        false
    }
}

pub fn free_fd(pid: u64, fd: usize) -> bool {
    unsafe {
        let tables = &mut *TABLES.0.get();
        if let Some(Some(table)) = tables.get_mut(pid as usize) {
            if fd < table.entries.len() {
                close_entry(table.entries[fd]);
                table.entries[fd] = FdEntry::Empty;
                return true;
            }
        }
        false
    }
}

pub fn close_all(pid: u64) {
    unsafe {
        let tables = &mut *TABLES.0.get();
        if let Some(Some(table)) = tables.get_mut(pid as usize) {
            for entry in table.entries.iter() {
                close_entry(*entry);
            }
            table.entries.clear();
        }
    }
}

fn pipe_clone(entry: &FdEntry) {
    match *entry {
        FdEntry::PipeWrite(id) => crate::pipe::clone_write(id),
        FdEntry::PipeRead(id)  => crate::pipe::clone_read(id),
        _ => {}
    }
}

fn close_entry(entry: FdEntry) {
    match entry {
        FdEntry::Device { ops, handle } => (ops.close)(handle),
        FdEntry::PipeWrite(id) => {
            crate::pipe::close_write(id);
            crate::process::notify_pipe_readers(id);
        }
        FdEntry::PipeRead(id) => crate::pipe::close_read(id),
        _ => {}
    }
}
