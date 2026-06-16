//! In-RAM, writable root filesystem.
//!
//! At boot the read-only KFS initramfs image is unpacked into an in-memory flat
//! inode table (like a Linux initramfs becoming a writable rootfs). After that,
//! every path operation — create / write / delete / mkdir / rmdir — mutates RAM
//! only; changes are lost on reboot. Files hand out node ids (stable indices),
//! never borrows, so a write that grows/reallocates a file can't dangle a fd.

use alloc::string::String;
use alloc::vec::Vec;
use crate::util::{SpinLock, SyncUnsafeCell};

const MAGIC: &[u8; 4] = b"KFS\0";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 28;
const ENTRY_SIZE: usize = 32;
const FLAG_DIR: u32 = 0x1;
const FLAG_FILE: u32 = 0x2;

/// Max path length stored inline in a `DirEntry` (listing has no allocator).
pub const PATH_CAP: usize = 96;

struct Node {
    path: String,
    is_dir: bool,
    data: Vec<u8>,
}

/// A table slot. `generation` is bumped every time the slot is reused for a new node,
/// so an fd holding `(index, generation)` cannot alias a different file created after
/// the original was unlinked (a use-after-free across unlink + create).
struct Slot {
    generation: u32,
    node: Option<Node>,
}

static NODES: SyncUnsafeCell<Vec<Slot>> = SyncUnsafeCell::new(Vec::new());
static FS_LOCK: SpinLock = SpinLock::new();

/// Run `f` with exclusive access to the node table (interrupts disabled). File
/// ops only run from syscall context, so a leaf spinlock is sufficient.
fn with_fs<R>(f: impl FnOnce(&mut Vec<Slot>) -> R) -> R {
    let flags = crate::util::irq_save();
    FS_LOCK.lock();
    let r = f(unsafe { &mut *NODES.0.get() });
    FS_LOCK.unlock();
    crate::util::restore_flags(flags);
    r
}

#[derive(Clone, Copy)]
pub enum VfsNode {
    File { node: u32, generation: u32, len: usize },
    Dir,
    Device(&'static crate::devfs::DeviceOps),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
}

#[derive(Clone, Copy)]
pub struct Metadata {
    pub len: usize,
    pub kind: FileType,
}

#[derive(Clone, Copy)]
pub struct DirEntry {
    pub path: [u8; PATH_CAP],
    pub path_len: usize,
    pub kind: FileType,
    pub len: usize,
}

impl DirEntry {
    pub const fn empty() -> Self {
        Self { path: [0; PATH_CAP], path_len: 0, kind: FileType::File, len: 0 }
    }
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_len]).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FsError {
    NotInitialized,
    InvalidImage,
    InvalidPath,
    NotFound,
    NotAFile,
    NotADirectory,
    BufferTooSmall,
    AlreadyExists,
    NotEmpty,
}

// ── boot-time unpack ────────────────────────────────────────────────────────

pub fn init(image: &[u8]) -> Result<(), FsError> {
    if image.len() < HEADER_SIZE || &image[..4] != MAGIC {
        return Err(FsError::InvalidImage);
    }
    if read_u32(image, 4)? != VERSION {
        return Err(FsError::InvalidImage);
    }
    let file_count = read_u32(image, 8)? as usize;
    let entries_size = file_count.checked_mul(ENTRY_SIZE).ok_or(FsError::InvalidImage)?;
    let paths_start = HEADER_SIZE.checked_add(entries_size).ok_or(FsError::InvalidImage)?;
    if paths_start > image.len() {
        return Err(FsError::InvalidImage);
    }

    with_fs(|nodes| {
        nodes.clear();
        // Root always exists.
        nodes.push(Slot { generation: 0, node: Some(Node { path: String::from("/"), is_dir: true, data: Vec::new() }) });

        for index in 0..file_count {
            let base = HEADER_SIZE + index * ENTRY_SIZE;
            let path_len = read_u16(image, base)? as usize;
            let path_off = read_u16(image, base + 2)? as usize;
            let data_off = read_u64(image, base + 4)? as usize;
            let size = read_u64(image, base + 12)? as usize;
            let flags = read_u32(image, base + 20)?;

            let pstart = paths_start.checked_add(path_off).ok_or(FsError::InvalidImage)?;
            let pend = pstart.checked_add(path_len).ok_or(FsError::InvalidImage)?;
            let path_bytes = image.get(pstart..pend).ok_or(FsError::InvalidImage)?;
            let path = core::str::from_utf8(path_bytes).map_err(|_| FsError::InvalidImage)?;
            validate_path(path)?;

            ensure_parents(nodes, path);
            if find(nodes, path).is_some() {
                continue; // already created as a synthesized parent
            }
            if flags & FLAG_FILE != 0 {
                let end = data_off.checked_add(size).ok_or(FsError::InvalidImage)?;
                let data = image.get(data_off..end).ok_or(FsError::InvalidImage)?.to_vec();
                nodes.push(Slot { generation: 0, node: Some(Node { path: String::from(path), is_dir: false, data }) });
            } else if flags & FLAG_DIR != 0 {
                nodes.push(Slot { generation: 0, node: Some(Node { path: String::from(path), is_dir: true, data: Vec::new() }) });
            } else {
                return Err(FsError::InvalidImage);
            }
        }
        Ok(())
    })
}

/// Create any missing ancestor directories of `path` (e.g. "/bin" for
/// "/bin/sh.kxe"). Caller holds the FS lock.
fn ensure_parents(nodes: &mut Vec<Slot>, path: &str) {
    let parent = parent_of(path);
    if parent == "/" {
        return;
    }
    ensure_parents(nodes, parent);
    if find(nodes, parent).is_none() {
        nodes.push(Slot { generation: 0, node: Some(Node { path: String::from(parent), is_dir: true, data: Vec::new() }) });
    }
}

// ── lookups (locked, return owned/copied data) ──────────────────────────────

pub fn lookup(path: &str) -> Result<VfsNode, FsError> {
    validate_path(path)?;
    if path.starts_with("/dev/") {
        if let Some(ops) = crate::devfs::lookup(path) {
            return Ok(VfsNode::Device(ops));
        }
    }
    with_fs(|nodes| {
        let id = find(nodes, path).ok_or(FsError::NotFound)?;
        let s = &nodes[id];
        let n = s.node.as_ref().unwrap();
        if n.is_dir {
            Ok(VfsNode::Dir)
        } else {
            Ok(VfsNode::File { node: id as u32, generation: s.generation, len: n.data.len() })
        }
    })
}

pub fn metadata(path: &str) -> Result<Metadata, FsError> {
    validate_path(path)?;
    with_fs(|nodes| {
        let id = find(nodes, path).ok_or(FsError::NotFound)?;
        let n = nodes[id].node.as_ref().unwrap();
        Ok(Metadata {
            len: n.data.len(),
            kind: if n.is_dir { FileType::Directory } else { FileType::File },
        })
    })
}

/// Copy a whole file out of the ramfs (used by the program loader).
pub fn read_file(path: &str) -> Result<Vec<u8>, FsError> {
    validate_path(path)?;
    with_fs(|nodes| {
        let id = find(nodes, path).ok_or(FsError::NotFound)?;
        let n = nodes[id].node.as_ref().unwrap();
        if n.is_dir {
            return Err(FsError::NotAFile);
        }
        Ok(n.data.clone())
    })
}

pub fn read_dir(path: &str, out: &mut [DirEntry]) -> Result<usize, FsError> {
    validate_path(path)?;
    with_fs(|nodes| {
        let dir_id = find(nodes, path).ok_or(FsError::NotFound)?;
        if !nodes[dir_id].node.as_ref().unwrap().is_dir {
            return Err(FsError::NotADirectory);
        }
        let mut count = 0usize;
        for slot in nodes.iter() {
            let Some(n) = slot.node.as_ref() else { continue };
            if !is_child(path, &n.path) {
                continue;
            }
            if count >= out.len() {
                return Err(FsError::BufferTooSmall);
            }
            let bytes = n.path.as_bytes();
            let take = bytes.len().min(PATH_CAP);
            let mut e = DirEntry::empty();
            e.path[..take].copy_from_slice(&bytes[..take]);
            e.path_len = take;
            e.kind = if n.is_dir { FileType::Directory } else { FileType::File };
            e.len = n.data.len();
            out[count] = e;
            count += 1;
        }
        Ok(count)
    })
}

// ── fd-level read/write by (node id, generation) ────────────────────────────

/// Borrow the live node at `(node, generation)`, or None if the slot was reused/freed.
fn live<'a>(nodes: &'a [Slot], node: u32, generation: u32) -> Option<&'a Node> {
    let s = nodes.get(node as usize)?;
    if s.generation != generation { return None; }
    s.node.as_ref()
}

fn live_mut<'a>(nodes: &'a mut [Slot], node: u32, generation: u32) -> Option<&'a mut Node> {
    let s = nodes.get_mut(node as usize)?;
    if s.generation != generation { return None; }
    s.node.as_mut()
}

pub fn read_at(node: u32, generation: u32, offset: usize, buf: &mut [u8]) -> usize {
    with_fs(|nodes| {
        let Some(n) = live(nodes, node, generation) else { return 0 };
        if n.is_dir || offset >= n.data.len() {
            return 0;
        }
        let avail = n.data.len() - offset;
        let take = buf.len().min(avail);
        buf[..take].copy_from_slice(&n.data[offset..offset + take]);
        take
    })
}

pub fn truncate(node: u32, generation: u32) {
    with_fs(|nodes| {
        if let Some(n) = live_mut(nodes, node, generation) {
            if !n.is_dir {
                n.data.clear();
            }
        }
    });
}

pub fn write_at(node: u32, generation: u32, offset: usize, src: &[u8]) -> usize {
    with_fs(|nodes| {
        let Some(n) = live_mut(nodes, node, generation) else { return 0 };
        if n.is_dir {
            return 0;
        }
        let end = offset + src.len();
        if end > n.data.len() {
            n.data.resize(end, 0);
        }
        n.data[offset..end].copy_from_slice(src);
        src.len()
    })
}

// ── mutations (create / delete / mkdir / rmdir) ─────────────────────────────

/// Create a file; returns its `(node, generation)` handle.
pub fn create(path: &str) -> Result<(u32, u32), FsError> {
    make(path, false)
}

pub fn mkdir(path: &str) -> Result<(), FsError> {
    make(path, true).map(|_| ())
}

fn make(path: &str, is_dir: bool) -> Result<(u32, u32), FsError> {
    validate_path(path)?;
    if path == "/" {
        return Err(FsError::AlreadyExists);
    }
    with_fs(|nodes| {
        if find(nodes, path).is_some() {
            return Err(FsError::AlreadyExists);
        }
        let parent = parent_of(path);
        match find(nodes, parent) {
            Some(pid) if nodes[pid].node.as_ref().unwrap().is_dir => {}
            Some(_) => return Err(FsError::NotADirectory),
            None => return Err(FsError::NotFound),
        }
        let node = Node { path: String::from(path), is_dir, data: Vec::new() };
        // Reuse a tombstoned slot if any (bumping its generation so stale fds
        // can't alias the new file), else append a fresh slot.
        if let Some(i) = nodes.iter().position(|s| s.node.is_none()) {
            nodes[i].generation = nodes[i].generation.wrapping_add(1);
            nodes[i].node = Some(node);
            Ok((i as u32, nodes[i].generation))
        } else {
            nodes.push(Slot { generation: 0, node: Some(node) });
            Ok(((nodes.len() - 1) as u32, 0))
        }
    })
}

pub fn unlink(path: &str) -> Result<(), FsError> {
    validate_path(path)?;
    with_fs(|nodes| {
        let id = find(nodes, path).ok_or(FsError::NotFound)?;
        if nodes[id].node.as_ref().unwrap().is_dir {
            return Err(FsError::NotAFile);
        }
        nodes[id].node = None;
        Ok(())
    })
}

pub fn rmdir(path: &str) -> Result<(), FsError> {
    validate_path(path)?;
    if path == "/" {
        return Err(FsError::NotEmpty);
    }
    with_fs(|nodes| {
        let id = find(nodes, path).ok_or(FsError::NotFound)?;
        if !nodes[id].node.as_ref().unwrap().is_dir {
            return Err(FsError::NotADirectory);
        }
        let has_child = nodes.iter().any(|s| matches!(s.node.as_ref(), Some(n) if is_child(path, &n.path)));
        if has_child {
            return Err(FsError::NotEmpty);
        }
        nodes[id].node = None;
        Ok(())
    })
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn find(nodes: &[Slot], path: &str) -> Option<usize> {
    nodes.iter().position(|s| matches!(s.node.as_ref(), Some(n) if n.path == path))
}

fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

fn validate_path(path: &str) -> Result<(), FsError> {
    if !path.starts_with('/')
        || (path.len() > 1 && path.ends_with('/'))
        || path.contains("//")
        || path.contains("/./")
        || path.contains("/../")
    {
        return Err(FsError::InvalidPath);
    }
    Ok(())
}

fn is_child(parent: &str, child: &str) -> bool {
    if child == parent {
        return false;
    }
    let rest = if parent == "/" {
        child.strip_prefix('/')
    } else {
        child.strip_prefix(parent).and_then(|v| v.strip_prefix('/'))
    };
    matches!(rest, Some(v) if !v.is_empty() && !v.contains('/'))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, FsError> {
    let b = data.get(offset..offset + 2).ok_or(FsError::InvalidImage)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, FsError> {
    let b = data.get(offset..offset + 4).ok_or(FsError::InvalidImage)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let b = data.get(offset..offset + 8).ok_or(FsError::InvalidImage)?;
    Ok(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}
