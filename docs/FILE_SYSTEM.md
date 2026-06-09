# KazuOS File System Design

## Goal

KazuOS will use a small VFS layer with an initramfs-backed read-only filesystem as the first real filesystem step.

The immediate goal is not a full Unix filesystem. The goal is to provide a stable path-based file interface for:

- shell commands such as `ls` and `cat`
- loading executable files from `/bin`
- later `exec` and independent user processes

## Initial Scope

The first implementation should stay intentionally small.

Supported initially:

- read-only initramfs
- absolute paths only
- `/bin` files first
- flat or shallow directory listing
- file lookup by path
- reading whole files or byte ranges
- kernel-side API only

Deferred:

- writes
- file descriptors
- per-process fd tables
- `.` and `..` path normalization
- mount points
- permissions
- timestamps
- disk-backed filesystems
- page cache

## Architecture

The filesystem should be split into clear layers:

```text
shell / exec / syscalls
        |
      vfs.rs
        |
 initramfs / ramfs backend
        |
 BootInfo initrd_data/initrd_size
```

### `vfs.rs`

Responsibilities:

- path lookup
- common file metadata type
- `read_file(path)` style API
- `read_dir(path)` style API
- backend-independent interface

It should not parse hardware storage formats directly.

### initramfs backend

Responsibilities:

- validate the initramfs image
- parse file metadata
- expose files and directories to VFS
- return borrowed slices into the boot-loaded image when possible

The initial backend can be read-only and backed directly by `BootInfo::initrd_slice()`.

Current implementation note: the kernel currently builds a small in-memory initramfs at boot with `initramfs_builder.rs` instead of loading `initramfs.img` from the bootloader. This is a temporary bridge so VFS, `/bin/init.kxe`, `/bin/shell.kxe`, and shell `ls` can be developed before adding a host-side image builder.

## Initramfs Image Format

The old C KazuOS design used a compact custom image format. That design is acceptable as the starting point, with a reduced read-only interpretation.

### Header

```c
typedef struct {
    uint8_t magic[4];
    uint32_t version;
    uint32_t file_count;
    uint32_t reserved[4];
} InitramfsHeader;
```

Values:

- `magic`: `"KFS\0"`
- `version`: `1`
- `file_count`: number of metadata entries

### Metadata Entry

```c
typedef struct {
    uint16_t path_length;
    uint16_t path_offset;
    uint64_t data_offset;
    uint64_t file_size;
    uint32_t flags;
    uint32_t reserved;
} FileMetadata;
```

Interpretation:

- `path_length`: path length in bytes, excluding NUL
- `path_offset`: offset from the path string table start
- `data_offset`: offset from image start
- `file_size`: file size in bytes
- `flags`: file type flags

Initial flags:

- `0x1`: directory
- `0x2`: regular file
- other bits reserved

### Layout

```text
InitramfsHeader
FileMetadata[file_count]
path string table
file data area
```

All offsets must be bounds-checked before use.

## Paths

Initial path rules:

- paths must be UTF-8
- paths must start with `/`
- repeated `/` should be treated as invalid in the first implementation
- `.` and `..` are not supported initially
- trailing `/` only matches directories

Required initial paths:

```text
/bin
/bin/<program>
```

Future paths:

```text
/etc
/tmp
/proc
/dev
```

## Kernel API

Suggested first API:

```rust
pub struct Metadata {
    pub path: &'static str,
    pub len: usize,
    pub kind: FileType,
}

pub enum FileType {
    File,
    Directory,
}

pub fn init(initrd: &'static [u8]) -> Result<(), FsError>;
pub fn metadata(path: &str) -> Result<Metadata, FsError>;
pub fn read_file(path: &str) -> Result<&'static [u8], FsError>;
pub fn read_dir(path: &str, out: &mut [DirEntry]) -> Result<usize, FsError>;
```

Keep allocation optional. The parser can initially scan metadata entries directly instead of building a heap-backed inode tree.

## Error Model

Use a small kernel-side error enum:

```rust
pub enum FsError {
    NotInitialized,
    InvalidImage,
    InvalidPath,
    NotFound,
    NotAFile,
    NotADirectory,
    BufferTooSmall,
}
```

Syscalls can later translate these to negative integer return codes.

## Boot Flow

Current `BootInfo` already has:

```rust
initrd_data: *const u8,
initrd_size: usize,
```

The bootloader can load `initramfs.img` into this field. Until then, the existing loaded runtime file may continue to use the same fields for WAV testing, but the long-term meaning should become initramfs.

Recommended boot path:

1. Bootloader loads `initramfs.img`.
2. Bootloader sets `BootInfo::initrd_data` and `BootInfo::initrd_size`.
3. Kernel calls `vfs::init(boot_info.initrd_slice())` during initialization.
4. Shell and exec use VFS paths instead of direct bootloader blobs.

## Shell Integration

Initial shell commands:

- `ls`
- `ls /bin`

`cat /path` is planned but not implemented yet.

Do not implement filesystem parsing inside `shell.rs`. Shell should call VFS/syscall APIs.

## Exec Integration

The first `exec` implementation should load static ELF64 files from `/bin` through VFS.

Initial constraints:

- ELF64 only
- static executable only
- no dynamic linker
- no arguments or environment initially
- one new process per exec
- no fork

## Safety Rules

- Never trust offsets from the image.
- Check every offset and size for overflow.
- Reject paths that are not valid UTF-8.
- Do not allocate in interrupt, fault, allocator, or panic paths.
- Do not expose writable references into the initramfs image.
- Keep userspace pointer validation separate from VFS.

## Roadmap

1. Document the initramfs format.
2. Add temporary kernel-built initramfs.
3. Add read-only initramfs parser.
4. Add VFS lookup/read APIs.
5. Add shell `ls` through `SYS_LS`.
6. Add a host-side image builder.
7. Load `initramfs.img` in the bootloader.
8. Add shell `cat`.
9. Add `/bin` ELF loading.
10. Complete `exec` syscall.
11. Add per-process file descriptor tables.
12. Add writable ramfs or tmpfs.
