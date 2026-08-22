use crate::{console, process, syscall};
use alloc;

pub use kazuos_abi::*;

pub fn init() {
    syscall::register(crate::syscall::dispatch::syscall_dispatch, 0);
}


pub(crate) fn sys_wait(pid: u64) -> u64 {
    match process::info(pid) {
        Some(info) if matches!(info.state, crate::process::ProcessState::Exited) => 1,
        None => 1, // process already gone = done
        Some(_) => {
            // Block the calling thread until the target process exits.
            crate::process::block_current(crate::process::WaitTarget::Pid(pid));
            syscall::BLOCK_TO_SCHEDULER
        }
    }
}

pub(crate) fn sys_sleep(duration: u64, unit: u64) -> u64 {
    if duration == 0 {
        return 0;
    }
    // SLEEP_UNIT_TICK: block until the next timer interrupt fires.
    if unit == SLEEP_UNIT_TICK {
        crate::process::block_current(crate::process::WaitTarget::Tick);
        return syscall::BLOCK_TO_SCHEDULER;
    }
    let tsc_per_ms = unsafe { crate::syscall::context::TSC_PER_MS };
    let tsc = match unit {
        SLEEP_UNIT_US => {
            let r = tsc_per_ms.checked_mul(duration).map(|v| v / 1000);
            match r {
                Some(0) | None => return 0,
                Some(v) => v,
            }
        }
        _ => {
            // SLEEP_UNIT_MS
            match tsc_per_ms.checked_mul(duration) {
                None => return 0,
                Some(v) => v,
            }
        }
    };
    let deadline = crate::util::rdtsc() + tsc;
    crate::process::block_current(crate::process::WaitTarget::Timer(deadline));
    syscall::BLOCK_TO_SCHEDULER
}

// Directory entry handed to user space. kind: 0 = file, 1 = directory, 2 = device.
const READDIR_NAME_LEN: usize = 32;
const READDIR_CAP: usize = 64;

#[repr(C)]
struct UserDirEntry {
    kind: u8,
    name: [u8; READDIR_NAME_LEN],
}

unsafe fn write_dirent(out: *mut UserDirEntry, idx: usize, kind: u8, name: &str) {
    if idx >= READDIR_CAP { return; }
    let mut e = UserDirEntry { kind, name: [0u8; READDIR_NAME_LEN] };
    let b = name.as_bytes();
    let m = b.len().min(READDIR_NAME_LEN - 1);
    e.name[..m].copy_from_slice(&b[..m]);
    unsafe { core::ptr::write(out.add(idx), e); }
}

/// Enumerate a directory into the caller's buffer (`out_ptr`: `[UserDirEntry; 64]`).
/// Returns the entry count, or `u64::MAX` on error. The kernel only provides the
/// mechanism — formatting and output are the caller's job (so `ls` output follows
/// the program's stdout, e.g. into a pipe).
pub(crate) fn sys_readdir(ptr: u64, len: u64, out_ptr: u64) -> u64 {
    if out_ptr == 0 { return u64::MAX; }
    // The ABI has no output-length argument: the caller must supply room for the full
    // READDIR_CAP array, so that is what we require to be writable.
    const OUT_BYTES: u64 = (READDIR_CAP * core::mem::size_of::<UserDirEntry>()) as u64;
    if !crate::memory::uaccess::validate_range(out_ptr, OUT_BYTES, true) {
        return u64::MAX;
    }
    let owned;
    let path = if ptr == 0 || len == 0 {
        "/"
    } else {
        match crate::memory::uaccess::read_str(ptr, len) {
            Some(s) => { owned = s; owned.as_str() }
            None => return u64::MAX,
        }
    };
    let out = out_ptr as *mut UserDirEntry;
    let mut n = 0usize;

    if path == "/dev" || path == "/dev/" {
        crate::fs::devfs::for_each(|name| {
            let display = name.strip_prefix("/dev/").unwrap_or(name);
            unsafe { write_dirent(out, n, 2, display); } // 2 = device
            n += 1;
        });
        return n.min(READDIR_CAP) as u64;
    }

    let mut entries = [crate::fs::vfs::DirEntry::empty(); READDIR_CAP];
    match crate::fs::vfs::read_dir(path, &mut entries) {
        Ok(count) => {
            for entry in &entries[..count] {
                let kind = match entry.kind {
                    crate::fs::vfs::FileType::Directory => 1u8,
                    crate::fs::vfs::FileType::File => 0u8,
                };
                unsafe { write_dirent(out, n, kind, entry.name()); }
                n += 1;
            }
        }
        Err(_) => return u64::MAX,
    }
    if path == "/" {
        unsafe { write_dirent(out, n, 1, "dev"); }
        n += 1;
    }
    n.min(READDIR_CAP) as u64
}

// DMA VA base for user-space driver mappings (distinct from code/stack region).
const DMA_VA_BASE: u64 = 0x0000_00A0_0000_0000;
static DMA_VA_BUMP: crate::util::SyncUnsafeCell<u64> =
    crate::util::SyncUnsafeCell::new(DMA_VA_BASE);

// PCI MMIO VA base for user-space driver BAR mappings.
const PCI_MMIO_VA_BASE: u64 = 0x0000_00E0_0000_0000;
static PCI_MMIO_VA_BUMP: crate::util::SyncUnsafeCell<u64> =
    crate::util::SyncUnsafeCell::new(PCI_MMIO_VA_BASE);

struct DmaAlloc {
    pid: u64,
    virt: u64,
    phys: u64,
    size: u64,
}

static DMA_ALLOCS: crate::util::SyncUnsafeCell<alloc::vec::Vec<DmaAlloc>> =
    crate::util::SyncUnsafeCell::new(alloc::vec::Vec::new());

struct PciMmioAlloc {
    pid: u64,
    virt: u64,
    size: u64,
}

static PCI_MMIO_ALLOCS: crate::util::SyncUnsafeCell<alloc::vec::Vec<PciMmioAlloc>> =
    crate::util::SyncUnsafeCell::new(alloc::vec::Vec::new());

pub(crate) fn sys_dma_alloc(size: u64, phys_out_ptr: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    if process::privilege_level(caller) > process::PrivilegeLevel::Driver {
        return u64::MAX;
    }
    if size == 0 || size > 128 * 1024 * 1024 {
        return u64::MAX;
    }
    let aligned = (size + 4095) & !4095;
    let layout = match alloc::alloc::Layout::from_size_align(aligned as usize, 4096) {
        Ok(l) => l,
        Err(_) => return u64::MAX,
    };
    let phys = unsafe { alloc::alloc::alloc_zeroed(layout) } as u64;
    if phys == 0 {
        return u64::MAX;
    }
    let cr3 = match process::user_cr3(caller) {
        Some(c) => c,
        None => return u64::MAX,
    };
    let virt = unsafe {
        let bump = &mut *DMA_VA_BUMP.0.get();
        let va = *bump;
        *bump += aligned;
        va
    };
    unsafe {
        if crate::vmm::map_range(cr3, virt, phys, aligned, crate::vmm::MapFlags::USER_READ_WRITE)
            .is_err()
        {
            return u64::MAX;
        }
        if phys_out_ptr != 0 && !crate::memory::uaccess::write_value(phys_out_ptr, phys) {
            return u64::MAX;
        }
        let allocs = &mut *DMA_ALLOCS.0.get();
        allocs.push(DmaAlloc { pid: caller, virt, phys, size: aligned });
    }
    virt
}

pub(crate) fn sys_dma_free(virt: u64) -> u64 {
    if virt == 0 {
        return u64::MAX;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    unsafe {
        let allocs = &mut *DMA_ALLOCS.0.get();
        let pos = allocs.iter().position(|a| a.virt == virt && a.pid == caller);
        let Some(pos) = pos else { return u64::MAX; };
        let alloc = allocs.swap_remove(pos);
        let cr3 = match process::user_cr3(caller) {
            Some(c) => c,
            None => return u64::MAX,
        };
        crate::vmm::unmap_range(cr3, alloc.virt, alloc.size);
        let layout = match alloc::alloc::Layout::from_size_align(alloc.size as usize, 4096) {
            Ok(l) => l,
            Err(_) => return u64::MAX,
        };
        alloc::alloc::dealloc(alloc.phys as *mut u8, layout);
    }
    0
}

pub(crate) fn sys_pci_bar_map(bdf: u64, bar_index: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    if process::privilege_level(caller) > process::PrivilegeLevel::Driver {
        return u64::MAX;
    }
    let bus = ((bdf >> 16) & 0xFF) as u8;
    let device = ((bdf >> 8) & 0xFF) as u8;
    let function = (bdf & 0xFF) as u8;
    let bar_idx = bar_index as u8;
    if bar_idx > 5 {
        return u64::MAX;
    }
    let bar_val = crate::drivers::pci::read_bar(bus, device, function, bar_idx);
    if bar_val & 0x1 != 0 {
        // I/O BAR not supported by this syscall
        return u64::MAX;
    }
    let Some(phys) = crate::drivers::pci::bar_phys_addr(bus, device, function, bar_idx) else {
        return u64::MAX;
    };
    let size = crate::drivers::pci::bar_size(bus, device, function, bar_idx);
    if size == 0 {
        return u64::MAX;
    }
    let aligned_size = (size + 4095) & !4095;
    let cr3 = match process::user_cr3(caller) {
        Some(c) => c,
        None => return u64::MAX,
    };
    let virt = unsafe {
        let bump = &mut *PCI_MMIO_VA_BUMP.0.get();
        let va = *bump;
        *bump += aligned_size;
        va
    };
    unsafe {
        if crate::vmm::map_range(cr3, virt, phys, aligned_size, crate::vmm::MapFlags::USER_MMIO)
            .is_err()
        {
            return u64::MAX;
        }
        let allocs = &mut *PCI_MMIO_ALLOCS.0.get();
        allocs.push(PciMmioAlloc {
            pid: caller,
            virt,
            size: aligned_size,
        });
    }
    virt
}

pub(crate) fn sys_pci_bar_unmap(virt: u64) -> u64 {
    if virt == 0 {
        return u64::MAX;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    unsafe {
        let allocs = &mut *PCI_MMIO_ALLOCS.0.get();
        let pos = allocs.iter().position(|a| a.virt == virt && a.pid == caller);
        let Some(pos) = pos else { return u64::MAX; };
        let alloc = allocs.swap_remove(pos);
        let cr3 = match process::user_cr3(caller) {
            Some(c) => c,
            None => return u64::MAX,
        };
        crate::vmm::unmap_range(cr3, alloc.virt, alloc.size);
    }
    0
}

/// Heap alloc for user programs. Backed by individual PMM frames (each page is a
/// separate frame, mapped into the caller's address space) so it draws from all
/// of RAM rather than the kernel's heap pool, and does not require physically
/// contiguous memory. Frames are returned to the PMM on free / process exit.
const HEAP_VA_BASE: u64 = 0x0000_00C0_0000_0000;
static HEAP_VA_BUMP: crate::util::SyncUnsafeCell<u64> =
    crate::util::SyncUnsafeCell::new(HEAP_VA_BASE);

struct HeapAlloc {
    pid: u64,
    virt: u64,
    size: u64,
}

static HEAP_ALLOCS: crate::util::SyncUnsafeCell<alloc::vec::Vec<HeapAlloc>> =
    crate::util::SyncUnsafeCell::new(alloc::vec::Vec::new());

// Serialises the heap bookkeeping (HEAP_VA_BUMP + HEAP_ALLOCS) across CPUs.
static HEAP_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn heap_lock() -> u64 {
    let flags = crate::util::irq_save();
    while HEAP_LOCK.swap(true, core::sync::atomic::Ordering::Acquire) {
        core::hint::spin_loop();
    }
    flags
}

fn heap_unlock(flags: u64) {
    HEAP_LOCK.store(false, core::sync::atomic::Ordering::Release);
    crate::util::restore_flags(flags);
}

/// Unmap `pages` pages starting at `virt` and return their frames to the PMM.
unsafe fn free_user_pages(cr3: u64, virt: u64, pages: u64) {
    for i in 0..pages {
        let v = virt + i * 4096;
        if let Some(phys) = unsafe { crate::vmm::translate(cr3, v) } {
            crate::pmm::free_frame(phys);
        }
        unsafe { crate::vmm::unmap_page(cr3, v); }
    }
}

/// Reclaim memory by killing the largest killable process (OOM killer).
/// Never kills the calling process itself (it can't be safely freed mid-syscall);
/// if the caller is the biggest, returns false so the allocation just fails and
/// the runaway process self-limits. Returns true if a victim was killed. The
/// caller must restore its CR3 afterwards, since kill_pid switches the address
/// space of the current CPU.
fn oom_kill(caller: u64) -> bool {
    match process::oom_victim(0) {
        Some(victim) if victim != caller => {
            crate::serial_println!("OOM: killing pid {} to reclaim memory", victim);
            crate::process::kill_pid(victim);
            true
        }
        _ => false,
    }
}

pub(crate) fn sys_heap_alloc(size: u64) -> u64 {
    if size == 0 || size > 128 * 1024 * 1024 {
        return u64::MAX;
    }
    let aligned = (size + 4095) & !4095;
    let pages = aligned / 4096;
    let caller = match crate::scheduler::current_user_pid() {
        Some(pid) => pid,
        None => return u64::MAX,
    };
    let cr3 = match process::user_cr3(caller) {
        Some(c) => c,
        None => return u64::MAX,
    };
    // Reserve a virtual range (short critical section).
    let virt = {
        let g = heap_lock();
        let virt = unsafe {
            let bump = &mut *HEAP_VA_BUMP.0.get();
            let va = *bump;
            *bump += aligned;
            va
        };
        heap_unlock(g);
        virt
    };
    // Map the pages (PMM and the page-table allocator lock internally; no heap
    // lock held here to keep lock ordering simple).
    unsafe {
        for i in 0..pages {
            let v = virt + i * 4096;
            // Get a frame; if out of memory, kill a process to reclaim and retry.
            let frame = loop {
                match crate::pmm::alloc_frame() {
                    Some(f) => break f,
                    None => {
                        let killed = oom_kill(caller);
                        // kill_pid switches this CPU's CR3 to the kernel's; put
                        // the caller's address space back before we continue.
                        crate::vmm::switch_cr3(cr3);
                        if !killed {
                            free_user_pages(cr3, virt, i); // give up; roll back
                            return u64::MAX;
                        }
                    }
                }
            };
            core::ptr::write_bytes(frame as *mut u8, 0, 4096);
            if crate::vmm::map_page(cr3, v, frame, crate::vmm::MapFlags::USER_READ_WRITE).is_err() {
                crate::pmm::free_frame(frame);
                free_user_pages(cr3, virt, i);
                return u64::MAX;
            }
        }
    }
    {
        let g = heap_lock();
        unsafe {
            let allocs = &mut *HEAP_ALLOCS.0.get();
            allocs.push(HeapAlloc { pid: caller, virt, size: aligned });
        }
        heap_unlock(g);
    }
    process::add_memory_bytes(caller, aligned);
    virt
}

pub(crate) fn sys_heap_free(virt: u64) -> u64 {
    if virt == 0 {
        return u64::MAX;
    }
    let caller = match crate::scheduler::current_user_pid() {
        Some(pid) => pid,
        None => return u64::MAX,
    };
    // Remove the bookkeeping entry under the heap lock, then free the pages
    // without holding it.
    let g = heap_lock();
    let removed = unsafe {
        let allocs = &mut *HEAP_ALLOCS.0.get();
        allocs
            .iter()
            .position(|a| a.virt == virt && a.pid == caller)
            .map(|pos| allocs.swap_remove(pos))
    };
    heap_unlock(g);
    let Some(alloc) = removed else { return u64::MAX; };
    let cr3 = match process::user_cr3(caller) {
        Some(c) => c,
        None => return u64::MAX,
    };
    unsafe { free_user_pages(cr3, alloc.virt, alloc.size / 4096); }
    process::sub_memory_bytes(caller, alloc.size);
    0
}

pub fn free_heap_for_pid(pid: u64) {
    // Pull entries out one at a time under the lock, freeing pages outside it.
    loop {
        let g = heap_lock();
        let removed = unsafe {
            let allocs = &mut *HEAP_ALLOCS.0.get();
            allocs
                .iter()
                .position(|a| a.pid == pid)
                .map(|pos| allocs.swap_remove(pos))
        };
        heap_unlock(g);
        let Some(alloc) = removed else { break; };
        if let Some(cr3) = process::user_cr3(pid) {
            unsafe { free_user_pages(cr3, alloc.virt, alloc.size / 4096); }
        }
    }
}

pub fn free_dma_for_pid(pid: u64) {
    unsafe {
        let allocs = &mut *DMA_ALLOCS.0.get();
        let mut i = 0;
        while i < allocs.len() {
            if allocs[i].pid == pid {
                let alloc = allocs.swap_remove(i);
                if let Some(cr3) = process::user_cr3(pid) {
                    crate::vmm::unmap_range(cr3, alloc.virt, alloc.size);
                }
                if let Ok(layout) = alloc::alloc::Layout::from_size_align(alloc.size as usize, 4096)
                {
                    alloc::alloc::dealloc(alloc.phys as *mut u8, layout);
                }
            } else {
                i += 1;
            }
        }
    }
}

pub(crate) fn sys_exec(ptr: u64, len: u64, stdio_pack: u64) -> u64 {
    if ptr == 0 || len == 0 {
        return u64::MAX;
    }
    // Snapshot the caller's path+args into kernel memory immediately. Process
    // creation below (create_address_space, page-table edits, arg push) is long
    // and reads from `bytes` deep inside; if we kept the raw user pointer and an
    // IRQ switched CR3 mid-build, those reads would hit the caller's user VA in
    // the wrong address space and page-fault. Copying once up front (under the
    // caller's CR3) makes all later reads come from kernel memory, visible in
    // every address space.
    let Some(bytes_owned) = crate::memory::uaccess::read_bytes(ptr, len) else {
        return u64::MAX;
    };
    let bytes: &[u8] = &bytes_owned;
    // New format: "path\0arg1\0arg2\0\0" — null-separated path and args.
    // Old format: just path bytes (no null) — no args.
    let path_end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let path = core::str::from_utf8(&bytes[..path_end]).unwrap_or("");
    let args = if path_end < bytes.len() - 1 {
        // Parse args after the path's null terminator
        let mut arg_list = alloc::vec::Vec::new();
        let mut pos = path_end + 1;
        while pos < bytes.len() && bytes[pos] != 0 {
            let arg_start = pos;
            while pos < bytes.len() && bytes[pos] != 0 {
                pos += 1;
            }
            arg_list.push(&bytes[arg_start..pos]);
            if pos < bytes.len() {
                pos += 1; // skip null
            }
        }
        arg_list
    } else {
        alloc::vec::Vec::new()
    };

    let stdin_fd  = (stdio_pack & 0xFFFF) as u16;
    let stdout_fd = ((stdio_pack >> 16) & 0xFFFF) as u16;
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    let pid = if stdin_fd == 0xFFFF && stdout_fd == 0xFFFF {
        let pid = crate::task::exec::spawn_user_with_args(path, &args);
        if pid != 0 {
            crate::fs::fd::alloc_fd_at(pid, 0, crate::fs::fd::FdEntry::ConsoleIn);
            crate::fs::fd::alloc_fd_at(pid, 1, crate::fs::fd::FdEntry::ConsoleOut);
            crate::fs::fd::alloc_fd_at(pid, 2, crate::fs::fd::FdEntry::ConsoleOut);
        }
        pid
    } else {
        crate::task::exec::spawn_user_with_fds_and_args(path, &args, caller, stdin_fd, stdout_fd)
    };
    // Record the spawner as the child's parent so it's cleaned up if the parent exits,
    // and inherit its terminal size so children see the same terminal.
    if pid != 0 && pid != u64::MAX {
        process::set_parent(pid, caller);
        let (cols, rows) = process::winsize(caller);
        if cols != 0 && rows != 0 {
            process::set_winsize(pid, cols, rows);
        }
        // Give the child a controlling-terminal handle at fd 3: a dup of the caller's
        // fd 0 (its keyboard source). Lets an interactive child (e.g. a pager) read keys
        // even when its own fd 0 is a redirected data pipe.
        if stdio_pack & STDIO_CTTY != 0 {
            if let Some(tty) = crate::fs::fd::get_fd(caller, 0) {
                crate::fs::fd::alloc_fd_at(pid, 3, tty);
            }
        }
        // All of the child's fds (stdio, ctty, any redirections) are now installed. Make it
        // schedulable only now: spawn leaves it Sleeping so its first syscalls can't race
        // this fd setup on another CPU (an early open() would otherwise be clobbered here).
        process::set_ready(pid);
    }
    if pid == 0 || pid == u64::MAX {
        crate::log_warn!("sys_exec: spawn failed for '{}' (caller={}, stdio={:#x})", path, caller, stdio_pack);
    }
    pid
}

pub(crate) fn sys_pipe(out_ptr: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    let Some(pipe_id) = crate::fs::pipe::create() else {
        crate::log_warn!("sys_pipe: crate::fs::pipe::create failed (pid={})", caller);
        return u64::MAX;
    };
    let read_fd  = crate::fs::fd::alloc_fd(caller, crate::fs::fd::FdEntry::PipeRead(pipe_id));
    let write_fd = crate::fs::fd::alloc_fd(caller, crate::fs::fd::FdEntry::PipeWrite(pipe_id));
    match (read_fd, write_fd) {
        (Some(r), Some(w)) => {
            if out_ptr != 0 {
                let fds = [r as u64, w as u64];
                if !crate::memory::uaccess::write_value(out_ptr, fds) {
                    // Roll back: the caller never learns the fd numbers, so leaving them
                    // installed would leak both ends of the pipe for the process's life.
                    crate::fs::fd::free_fd(caller, r);
                    crate::fs::fd::free_fd(caller, w);
                    return u64::MAX;
                }
            }
            0
        }
        _ => {
            // fd table full: roll back whatever we did grab so we don't leak it.
            if let Some(r) = read_fd { crate::fs::fd::free_fd(caller, r); }
            if let Some(w) = write_fd { crate::fs::fd::free_fd(caller, w); }
            crate::log_warn!("sys_pipe: out of fds (pid={}, MAX_FD={})", caller, crate::fs::fd::MAX_FD);
            u64::MAX
        }
    }
}

/// Snapshot a user-space path string into kernel memory (under the caller's
/// active CR3) before touching the filesystem.
fn read_user_path(ptr: u64, len: u64) -> Option<alloc::string::String> {
    if ptr == 0 || len == 0 || len > 256 {
        return None;
    }
    crate::memory::uaccess::read_str(ptr, len)
}

pub(crate) fn sys_create(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    let (node, generation) = match crate::fs::vfs::create(&path) {
        Ok(h) => h,
        // create-or-truncate, so `echo ... > file` overwrites cleanly.
        Err(crate::fs::vfs::FsError::AlreadyExists) => match crate::fs::vfs::lookup(&path) {
            Ok(crate::fs::vfs::VfsNode::File { node, generation, .. }) => {
                crate::fs::vfs::truncate(node, generation);
                (node, generation)
            }
            _ => return u64::MAX,
        },
        Err(_) => return u64::MAX,
    };
    match crate::fs::fd::alloc_fd(caller, crate::fs::fd::FdEntry::File { node, generation, offset: 0 }) {
        Some(fd) => fd as u64,
        None => u64::MAX,
    }
}

pub(crate) fn sys_unlink(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    crate::fs::vfs::unlink(&path).map_or(u64::MAX, |_| 0)
}

pub(crate) fn sys_mkdir(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    crate::fs::vfs::mkdir(&path).map_or(u64::MAX, |_| 0)
}

pub(crate) fn sys_rmdir(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    crate::fs::vfs::rmdir(&path).map_or(u64::MAX, |_| 0)
}

pub(crate) fn sys_open(path_ptr: u64, path_len: u64) -> u64 {
    let Some(path) = read_user_path(path_ptr, path_len) else { return u64::MAX };
    let path = path.as_str();
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::fs::vfs::lookup(path) {
        Ok(crate::fs::vfs::VfsNode::Device(ops)) => {
            let handle = (ops.open)();
            if handle == u64::MAX {
                return u64::MAX;
            }
            match crate::fs::fd::alloc_fd(caller, crate::fs::fd::FdEntry::Device { ops, handle }) {
                Some(fd) => fd as u64,
                None => {
                    (ops.close)(handle);
                    u64::MAX
                }
            }
        }
        Ok(crate::fs::vfs::VfsNode::File { node, generation, len: _ }) => {
            match crate::fs::fd::alloc_fd(caller, crate::fs::fd::FdEntry::File { node, generation, offset: 0 }) {
                Some(fd) => fd as u64,
                None => u64::MAX,
            }
        }
        Ok(crate::fs::vfs::VfsNode::Dir) => u64::MAX,
        Err(_) => u64::MAX,
    }
}

pub(crate) fn sys_close(fd: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    if crate::fs::fd::free_fd(caller, fd as usize) {
        0
    } else {
        u64::MAX
    }
}

/// Keyboard input belongs to the framebuffer owner while one exists: a background
/// process (e.g. the console shell that launched a graphical app) must not steal
/// keys from the focused GUI by polling. True when someone else owns the framebuffer.
/// True when the caller may draw to the console framebuffer: either nobody owns the
/// framebuffer, or the caller is the owner. A background process (e.g. the console
/// shell that launched a GUI) must not paint over the graphical owner.
pub(crate) fn console_writable() -> bool {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::drivers::fb_owner::owner() {
        None => true,
        Some(owner) => owner == caller,
    }
}

/// Print `len` bytes of already-validated user memory to the console and/or serial.
/// Shared by SYS_CONSOLE_WRITE and sys_write's ConsoleOut branch.
///
/// The caller must have validated `[ptr, ptr + len)` as readable user memory.
pub(crate) fn print_user_bytes(ptr: u64, len: u64, do_fb: bool) {
    let src = ptr as *const u8;
    let len = len as usize;
    const CHUNK: usize = 256;
    let mut buf = [0u8; CHUNK];
    let mut offset = 0usize;
    let verbose = crate::init::is_verbose();
    while offset < len {
        let n = (len - offset).min(CHUNK);
        unsafe {
            core::ptr::copy_nonoverlapping(src.add(offset), buf.as_mut_ptr(), n);
        }
        // A chunk boundary can split a multi-byte sequence. Print the valid prefix and
        // resume from the split point instead of fabricating a &str from invalid bytes.
        let (chunk, consumed) = match core::str::from_utf8(&buf[..n]) {
            Ok(s) => (s, n),
            Err(e) => match e.valid_up_to() {
                // Genuinely invalid input rather than a split sequence: skip the chunk.
                0 => ("", n),
                valid => (unsafe { core::str::from_utf8_unchecked(&buf[..valid]) }, valid),
            },
        };
        if do_fb {
            console::screen_print(chunk);
        }
        if verbose {
            crate::serial_print!("{}", chunk);
        }
        offset += consumed;
    }
}

pub(crate) fn kbd_locked_out() -> bool {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    matches!(crate::drivers::fb_owner::owner(), Some(o) if o != caller)
}

/// Non-blocking read. Returns the byte count, `0` when no data is available right
/// now (would block), or `u64::MAX` on EOF (pipe writer gone) or a bad fd. Lets a
/// single-threaded program (e.g. the GUI terminal) poll a pipe without sleeping.
pub(crate) fn sys_try_read(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return 0;
    }
    // One check for every branch below, and it also bounds buf_len before it is used
    // to size a kernel-side Vec.
    if !crate::memory::uaccess::validate_range(buf_ptr, buf_len, true) {
        return u64::MAX;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::fs::fd::get_fd(caller, fd as usize) {
        Some(crate::fs::fd::FdEntry::ConsoleIn) => {
            if kbd_locked_out() { return 0; }
            if let Some(ch) = crate::drivers::keyboard::get_raw() {
                unsafe { core::ptr::write(buf_ptr as *mut u8, ch); }
                1
            } else {
                0
            }
        }
        Some(crate::fs::fd::FdEntry::PipeRead(pipe_id)) => {
            if crate::fs::pipe::is_empty(pipe_id) {
                return if crate::fs::pipe::writer_closed(pipe_id) { u64::MAX } else { 0 };
            }
            let mut kbuf = alloc::vec![0u8; buf_len as usize];
            let n = crate::fs::pipe::read(pipe_id, &mut kbuf);
            unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n); }
            n as u64
        }
        _ => u64::MAX,
    }
}

pub(crate) fn sys_read(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return 0;
    }
    if !crate::memory::uaccess::validate_range(buf_ptr, buf_len, true) {
        return u64::MAX;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::fs::fd::get_fd(caller, fd as usize) {
        Some(crate::fs::fd::FdEntry::ConsoleIn) => {
            let ch = if kbd_locked_out() { None } else { crate::drivers::keyboard::get_raw() };
            if let Some(ch) = ch {
                unsafe { core::ptr::write(buf_ptr as *mut u8, ch); }
                1
            } else {
                if let Some(pid) = crate::scheduler::current_user_pid() {
                    crate::process::set_wait_target(pid, crate::process::WaitTarget::Keyboard);
                    crate::process::set_sleeping(pid);
                }
                syscall::BLOCK_TO_SCHEDULER
            }
        }
        Some(crate::fs::fd::FdEntry::PipeRead(pipe_id)) => {
            // Decide read-vs-EOF-vs-block atomically. pipe and thread state share the same
            // reentrant lock, so holding it across the whole decision serialises us against
            // a concurrent writer's write+close+notify. Otherwise (separate lock acquisitions
            // for is_empty / writer_closed / set_sleeping) a writer that writes-then-exits in
            // the gap makes us return EOF while its bytes sit unread in the buffer — the data
            // loss that truncated `cmd | less` output under load.
            crate::task::thread::with_threads_lock(|| {
                if !crate::fs::pipe::is_empty(pipe_id) {
                    let mut kbuf = alloc::vec![0u8; buf_len as usize];
                    let n = crate::fs::pipe::read(pipe_id, &mut kbuf);
                    unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n); }
                    n as u64
                } else if crate::fs::pipe::writer_closed(pipe_id) {
                    0 // EOF: empty and no writers can ever add more
                } else {
                    if let Some(pid) = crate::scheduler::current_user_pid() {
                        crate::process::set_wait_target(pid, crate::process::WaitTarget::PipeRead {
                            pipe_id,
                            buf_ptr,
                            buf_len,
                        });
                        crate::process::set_sleeping(pid);
                    }
                    syscall::BLOCK_TO_SCHEDULER
                }
            })
        }
        Some(crate::fs::fd::FdEntry::File { node, generation, offset }) => {
            let want = buf_len as usize;
            let mut kbuf = alloc::vec![0u8; want];
            let n = crate::fs::vfs::read_at(node, generation, offset, &mut kbuf);
            if n == 0 {
                return 0;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n);
            }
            let _ = crate::fs::fd::set_fd(caller, fd as usize, crate::fs::fd::FdEntry::File { node, generation, offset: offset + n });
            n as u64
        }
        Some(crate::fs::fd::FdEntry::Device { ops, handle }) => {
            let mut total = 0usize;
            const CHUNK: usize = 8192;
            let mut kbuf = [0u8; CHUNK];
            let dst = buf_ptr as *mut u8;
            let mut remain = buf_len as usize;
            while remain > 0 {
                let n = remain.min(CHUNK);
                let read = (ops.read)(handle, &mut kbuf[..n]);
                if read == 0 {
                    break;
                }
                unsafe {
                    core::ptr::copy_nonoverlapping(kbuf.as_ptr(), dst.add(total), read);
                }
                total += read;
                remain -= read;
                if read < n {
                    break;
                }
            }
            total as u64
        }
        _ => u64::MAX,
    }
}

pub(crate) fn sys_write(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return 0;
    }
    if !crate::memory::uaccess::validate_range(buf_ptr, buf_len, false) {
        return u64::MAX;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::fs::fd::get_fd(caller, fd as usize) {
        Some(crate::fs::fd::FdEntry::ConsoleOut) => {
            let fb_owner = crate::drivers::fb_owner::owner();
            let do_fb = fb_owner.is_none() || fb_owner == Some(caller);
            print_user_bytes(buf_ptr, buf_len, do_fb);
            buf_len
        }
        Some(crate::fs::fd::FdEntry::PipeWrite(pipe_id)) => {
            let mut kbuf = alloc::vec![0u8; buf_len as usize];
            unsafe { core::ptr::copy_nonoverlapping(buf_ptr as *const u8, kbuf.as_mut_ptr(), buf_len as usize); }
            let n = crate::fs::pipe::write(pipe_id, &kbuf);
            crate::process::notify_pipe_readers(pipe_id);
            n as u64
        }
        Some(crate::fs::fd::FdEntry::File { node, generation, offset }) => {
            let len = buf_len as usize;
            let mut kbuf = alloc::vec![0u8; len];
            unsafe { core::ptr::copy_nonoverlapping(buf_ptr as *const u8, kbuf.as_mut_ptr(), len); }
            let n = crate::fs::vfs::write_at(node, generation, offset, &kbuf);
            let _ = crate::fs::fd::set_fd(caller, fd as usize, crate::fs::fd::FdEntry::File { node, generation, offset: offset + n });
            n as u64
        }
        Some(crate::fs::fd::FdEntry::Device { ops, handle }) => {
            let mut total = 0usize;
            const CHUNK: usize = 8192;
            let mut kbuf = [0u8; CHUNK];
            let src = buf_ptr as *const u8;
            let mut remain = buf_len as usize;
            while remain > 0 {
                let n = remain.min(CHUNK);
                unsafe {
                    core::ptr::copy_nonoverlapping(src.add(total), kbuf.as_mut_ptr(), n);
                }
                let written = (ops.write)(handle, &kbuf[..n]);
                if written == 0 {
                    break;
                }
                total += written;
                remain -= written;
                if written < n {
                    break;
                }
            }
            total as u64
        }
        _ => u64::MAX,
    }
}

// Cached PCI device list, populated on first SYS_PCI_INFO call.
static PCI_CACHE: crate::util::SyncUnsafeCell<alloc::vec::Vec<crate::drivers::pci::Device>> =
    crate::util::SyncUnsafeCell::new(alloc::vec::Vec::new());
static PCI_CACHE_READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
// Serializes the one-time cache build. Without it, two CPUs racing on the cold cache both
// see READY=false and push into the same Vec concurrently, corrupting it (garbage length /
// reallocation race) — which is why `lspci` showed a varying or empty device list.
static PCI_CACHE_LOCK: crate::util::SpinLock = crate::util::SpinLock::new();

#[repr(C)]
#[derive(Clone, Copy)]
pub struct PciDeviceInfo {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub _pad: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

/// Scan PCI once into the cache. Called eagerly at boot (single-threaded, before APs and
/// user processes run) so the scan never races concurrent PCI access — and lazily as a
/// fallback. The lock makes the build atomic so two cold callers can't double-scan and
/// corrupt the cache Vec.
pub fn build_pci_cache() {
    use crate::drivers::pci;
    use core::sync::atomic::Ordering;
    if PCI_CACHE_READY.load(Ordering::Acquire) {
        return;
    }
    PCI_CACHE_LOCK.lock();
    if !PCI_CACHE_READY.load(Ordering::Acquire) {
        let cache = unsafe { &mut *PCI_CACHE.0.get() };
        cache.clear();
        let kind = if pci::pcie_available() { pci::ScanKind::Pcie } else { pci::ScanKind::Pci };
        pci::scan(kind, |dev| cache.push(dev));
        PCI_CACHE_READY.store(true, Ordering::Release);
    }
    PCI_CACHE_LOCK.unlock();
}

pub(crate) fn sys_pci_info(index: u64, out_ptr: u64) -> u64 {
    build_pci_cache();

    let cache = unsafe { &*PCI_CACHE.0.get() };
    let idx = index as usize;
    if idx >= cache.len() {
        return u64::MAX;
    }
    if out_ptr != 0 {
        let dev = &cache[idx];
        let info = PciDeviceInfo {
            bus: dev.bus,
            device: dev.device,
            function: dev.function,
            _pad: 0,
            vendor_id: dev.vendor_id,
            device_id: dev.device_id,
            class_code: dev.class_code,
            subclass: dev.subclass,
            prog_if: dev.prog_if,
            header_type: dev.header_type,
        };
        if !crate::memory::uaccess::write_value(out_ptr, info) {
            return u64::MAX;
        }
    }
    cache.len() as u64
}

pub(crate) fn sys_ioctl(fd: u64, cmd: u64, arg: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::fs::fd::get_fd(caller, fd as usize) {
        Some(crate::fs::fd::FdEntry::Device { ops, handle }) => {
            let r = (ops.ioctl)(handle, cmd, arg);
            if r == syscall::BLOCK_TO_SCHEDULER as i64 {
                return syscall::BLOCK_TO_SCHEDULER;
            }
            r as u64
        }
        _ => u64::MAX,
    }
}


