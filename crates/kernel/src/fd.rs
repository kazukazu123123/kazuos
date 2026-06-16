use alloc::vec::Vec;
use crate::util::SyncUnsafeCell;
use crate::devfs::DeviceOps;

pub const MAX_FD: usize = 16;

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

#[derive(Clone, Copy)]
pub struct FdTable {
    pub entries: [FdEntry; MAX_FD],
}

impl FdTable {
    pub const fn new() -> Self {
        Self {
            entries: [FdEntry::Empty; MAX_FD],
        }
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
    ensure_table(pid);
    pipe_clone(&entry);
    unsafe {
        let tables = &mut *TABLES.0.get();
        if let Some(Some(table)) = tables.get_mut(pid as usize) {
            if fd < MAX_FD {
                table.entries[fd] = entry;
                return true;
            }
        }
    }
    false
}

pub fn alloc_fd(pid: u64, entry: FdEntry) -> Option<usize> {
    ensure_table(pid);
    pipe_clone(&entry);
    unsafe {
        let tables = &mut *TABLES.0.get();
        let table = tables[pid as usize].as_mut()?;
        for i in 0..MAX_FD {
            if matches!(table.entries[i], FdEntry::Empty) {
                table.entries[i] = entry;
                return Some(i);
            }
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
            if fd < MAX_FD {
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
            if fd < MAX_FD {
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
            for i in 0..MAX_FD {
                close_entry(table.entries[i]);
                table.entries[i] = FdEntry::Empty;
            }
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
