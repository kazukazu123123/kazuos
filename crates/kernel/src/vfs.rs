use crate::util::SyncUnsafeCell;

const MAGIC: &[u8; 4] = b"KFS\0";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 28;
const ENTRY_SIZE: usize = 32;
const FLAG_DIR: u32 = 0x1;
const FLAG_FILE: u32 = 0x2;

static FS: SyncUnsafeCell<Option<Initramfs>> = SyncUnsafeCell::new(None);

#[derive(Clone, Copy)]
pub enum VfsNode {
    File { path: &'static str, data: &'static [u8] },
    Dir,
    Device(&'static crate::devfs::DeviceOps),
}

#[derive(Clone, Copy)]
struct Initramfs {
    image: &'static [u8],
    file_count: usize,
    paths_start: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
}

#[derive(Clone, Copy)]
pub struct Metadata {
    pub path: &'static str,
    pub len: usize,
    pub kind: FileType,
}

#[derive(Clone, Copy)]
pub struct DirEntry {
    pub path: &'static str,
    pub kind: FileType,
    pub len: usize,
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
}

#[derive(Clone, Copy)]
struct Entry {
    path: &'static str,
    data_offset: usize,
    file_size: usize,
    flags: u32,
}

pub fn init(image: &'static [u8]) -> Result<(), FsError> {
    if image.len() < HEADER_SIZE || &image[..4] != MAGIC {
        return Err(FsError::InvalidImage);
    }
    let version = read_u32(image, 4)?;
    let file_count = read_u32(image, 8)? as usize;
    if version != VERSION {
        return Err(FsError::InvalidImage);
    }
    let entries_size = file_count
        .checked_mul(ENTRY_SIZE)
        .ok_or(FsError::InvalidImage)?;
    let paths_start = HEADER_SIZE
        .checked_add(entries_size)
        .ok_or(FsError::InvalidImage)?;
    if paths_start > image.len() {
        return Err(FsError::InvalidImage);
    }
    let fs = Initramfs {
        image,
        file_count,
        paths_start,
    };
    for index in 0..file_count {
        parse_entry(fs, index)?;
    }
    unsafe {
        *FS.0.get() = Some(fs);
    }
    Ok(())
}

pub fn metadata(path: &str) -> Result<Metadata, FsError> {
    let entry = find(path)?;
    Ok(Metadata {
        path: entry.path,
        len: entry.file_size,
        kind: entry.kind(),
    })
}

pub fn read_file(path: &str) -> Result<&'static [u8], FsError> {
    let entry = find(path)?;
    if entry.kind() != FileType::File {
        return Err(FsError::NotAFile);
    }
    let fs = fs()?;
    let end = entry
        .data_offset
        .checked_add(entry.file_size)
        .ok_or(FsError::InvalidImage)?;
    fs.image
        .get(entry.data_offset..end)
        .ok_or(FsError::InvalidImage)
}

pub fn read_dir(path: &str, out: &mut [DirEntry]) -> Result<usize, FsError> {
    if metadata(path)?.kind != FileType::Directory {
        return Err(FsError::NotADirectory);
    }
    let fs = fs()?;
    let prefix = if path == "/" { "/" } else { path };
    let mut count = 0usize;
    for index in 0..fs.file_count {
        let entry = parse_entry(fs, index)?;
        if is_child(prefix, entry.path) {
            if count >= out.len() {
                return Err(FsError::BufferTooSmall);
            }
            out[count] = DirEntry {
                path: entry.path,
                kind: entry.kind(),
                len: entry.file_size,
            };
            count += 1;
        }
    }
    Ok(count)
}

fn fs() -> Result<Initramfs, FsError> {
    unsafe { (*FS.0.get()).ok_or(FsError::NotInitialized) }
}

fn find(path: &str) -> Result<Entry, FsError> {
    validate_path(path)?;
    let fs = fs()?;
    for index in 0..fs.file_count {
        let entry = parse_entry(fs, index)?;
        if entry.path == path {
            return Ok(entry);
        }
    }
    Err(FsError::NotFound)
}

fn parse_entry(fs: Initramfs, index: usize) -> Result<Entry, FsError> {
    let base = HEADER_SIZE + index * ENTRY_SIZE;
    let path_length = read_u16(fs.image, base)? as usize;
    let path_offset = read_u16(fs.image, base + 2)? as usize;
    let data_offset = read_u64(fs.image, base + 4)? as usize;
    let file_size = read_u64(fs.image, base + 12)? as usize;
    let flags = read_u32(fs.image, base + 20)?;

    let path_start = fs
        .paths_start
        .checked_add(path_offset)
        .ok_or(FsError::InvalidImage)?;
    let path_end = path_start
        .checked_add(path_length)
        .ok_or(FsError::InvalidImage)?;
    let path_bytes = fs
        .image
        .get(path_start..path_end)
        .ok_or(FsError::InvalidImage)?;
    let path = core::str::from_utf8(path_bytes).map_err(|_| FsError::InvalidImage)?;
    validate_path(path)?;
    if flags & FLAG_FILE != 0 {
        let end = data_offset
            .checked_add(file_size)
            .ok_or(FsError::InvalidImage)?;
        if end > fs.image.len() {
            return Err(FsError::InvalidImage);
        }
    } else if flags & FLAG_DIR == 0 {
        return Err(FsError::InvalidImage);
    }
    Ok(Entry {
        path,
        data_offset,
        file_size,
        flags,
    })
}

impl Entry {
    fn kind(self) -> FileType {
        if self.flags & FLAG_DIR != 0 {
            FileType::Directory
        } else {
            FileType::File
        }
    }
}

fn validate_path(path: &str) -> Result<(), FsError> {
    if !path.starts_with('/')
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
        child
            .strip_prefix(parent)
            .and_then(|value| value.strip_prefix('/'))
    };
    matches!(rest, Some(value) if !value.is_empty() && !value.contains('/'))
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, FsError> {
    let bytes = data.get(offset..offset + 2).ok_or(FsError::InvalidImage)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Result<u32, FsError> {
    let bytes = data.get(offset..offset + 4).ok_or(FsError::InvalidImage)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, FsError> {
    let bytes = data.get(offset..offset + 8).ok_or(FsError::InvalidImage)?;
    Ok(u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]))
}

pub fn lookup(path: &str) -> Result<VfsNode, FsError> {
    validate_path(path)?;
    if path.starts_with("/dev/") {
        if let Some(ops) = crate::devfs::lookup(path) {
            return Ok(VfsNode::Device(ops));
        }
    }
    let entry = find(path)?;
    if entry.kind() == FileType::Directory {
        Ok(VfsNode::Dir)
    } else {
        let fs = fs()?;
        let end = entry
            .data_offset
            .checked_add(entry.file_size)
            .ok_or(FsError::InvalidImage)?;
        let data = fs
            .image
            .get(entry.data_offset..end)
            .ok_or(FsError::InvalidImage)?;
        Ok(VfsNode::File { path: entry.path, data })
    }
}
