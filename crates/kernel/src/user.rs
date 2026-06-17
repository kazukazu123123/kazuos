use crate::process::ProcessInfo;
use crate::smp::{MAX_CPUS, current_cpu_index};
use crate::util::SyncUnsafeCell;
use crate::{console, exec, fd, ipc, process, syscall};
use alloc;

pub static mut TSC_PER_MS: u64 = 3_000_000;

static EXITING_PID_TMPS: SyncUnsafeCell<[u64; MAX_CPUS]> = SyncUnsafeCell::new([0; MAX_CPUS]);

pub fn exiting_pid_tmp() -> u64 {
    unsafe { (*EXITING_PID_TMPS.0.get())[current_cpu_index()] }
}

pub fn set_exiting_pid_tmp(value: u64) {
    unsafe {
        (*EXITING_PID_TMPS.0.get())[current_cpu_index()] = value;
    }
}

static KERNEL_RETURN_STACKS: SyncUnsafeCell<[u64; MAX_CPUS]> = SyncUnsafeCell::new([0; MAX_CPUS]);

#[unsafe(no_mangle)]
pub extern "C" fn kernel_return_stack_ptr() -> *mut u64 {
    unsafe { (*KERNEL_RETURN_STACKS.0.get()).as_mut_ptr().add(current_cpu_index()) }
}

pub fn set_kernel_return_stack(value: u64) {
    unsafe {
        (*KERNEL_RETURN_STACKS.0.get())[current_cpu_index()] = value;
    }
}

static BLOCKING_RSP_TMPS: SyncUnsafeCell<[u64; MAX_CPUS]> = SyncUnsafeCell::new([0; MAX_CPUS]);

#[unsafe(no_mangle)]
pub extern "C" fn blocking_rsp_tmp_ptr() -> *mut u64 {
    unsafe { (*BLOCKING_RSP_TMPS.0.get()).as_mut_ptr().add(current_cpu_index()) }
}

pub fn blocking_rsp_tmp() -> u64 {
    unsafe { (*BLOCKING_RSP_TMPS.0.get())[current_cpu_index()] }
}

pub fn set_blocking_rsp_tmp(value: u64) {
    unsafe {
        (*BLOCKING_RSP_TMPS.0.get())[current_cpu_index()] = value;
    }
}

pub use kazuos_abi::*;

pub fn init() {
    syscall::register(syscall_dispatch, 0);
}

extern "C" fn syscall_dispatch(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    // A remote kill that arrived while this thread was running was deferred (the
    // killer could not safely free our address space underneath us). Now that we
    // are back in the kernel on our own CPU, honor it: exit cleanly instead of
    // servicing the syscall against soon-to-be-freed memory.
    if let Some(pid) = crate::scheduler::current_user_pid() {
        if process::is_kill_pending(pid) {
            crate::user::set_exiting_pid_tmp(pid);
            process::exit_current();
            return syscall::EXIT_TO_KERNEL;
        }
    }
    match number {
        // Console / Display
        SYS_CONSOLE_WRITE => {
            if arg0 != 0 && arg1 > 0 {
                let src = arg0 as *const u8;
                let len = arg1 as usize;
                const CHUNK: usize = 256;
                let mut buf = [0u8; CHUNK];
                let mut offset = 0usize;
                let caller = crate::scheduler::current_user_pid().unwrap_or(0);
                let fb_owner = crate::drivers::fb_owner::owner();
                let do_fb = fb_owner.is_none() || fb_owner == Some(caller);
                while offset < len {
                    let remain = len - offset;
                    let n = remain.min(CHUNK);
                    unsafe {
                        core::ptr::copy_nonoverlapping(src.add(offset), buf.as_mut_ptr(), n);
                    }
                    let chunk = unsafe { core::str::from_utf8_unchecked(&buf[..n]) };
                    if do_fb { console::screen_print(chunk); }
                    if crate::init::is_verbose() { crate::serial_print!("{}", chunk); }
                    offset += n;
                }
            }
            0
        }
        // Console / cursor ops touch the framebuffer, so suppress them when another
        // process owns it (a background console shell must not draw over a GUI).
        SYS_CURSOR_SAVE => { if console_writable() { console::save_cursor_pos(); } 0 }
        SYS_CURSOR_RESTORE => { if console_writable() { console::restore_cursor_pos(); } 0 }
        SYS_CURSOR_DRAW => {
            if console_writable() { console::draw_saved_cursor(arg0 != 0); }
            0
        }
        SYS_FB_ACQUIRE => {
            if let Some(pid) = crate::scheduler::current_user_pid() {
                if let Some(ctx) = process::user_context(pid) {
                    crate::drivers::fb_owner::acquire(pid, ctx.cr3, arg0 as *mut crate::drivers::fb_owner::FbInfo)
                } else { u64::MAX }
            } else { u64::MAX }
        }
        SYS_FB_RELEASE => { if let Some(pid) = crate::scheduler::current_user_pid() { crate::drivers::fb_owner::release(pid); } 0 }
        SYS_CONSOLE_SIZE => {
            // Terminal-size get/set: arg0 == 0 gets, arg0 != 0 sets.
            if arg0 == 0 {
                // GET: the caller's terminal size, or the console if none was set.
                let caller = crate::scheduler::current_user_pid().unwrap_or(0);
                let (mut cols, mut rows) = process::winsize(caller);
                if cols == 0 || rows == 0 {
                    let (c, r) = crate::terminal::console::console_size();
                    cols = c as u16;
                    rows = r as u16;
                }
                ((rows as u64) << 32) | (cols as u64)
            } else {
                // SET: cols = arg0 & 0xFFFF, rows = arg0 >> 16; target pid = arg1 (0 = self).
                let cols = (arg0 & 0xFFFF) as u16;
                let rows = ((arg0 >> 16) & 0xFFFF) as u16;
                let target = if arg1 == 0 { crate::scheduler::current_user_pid().unwrap_or(0) } else { arg1 };
                process::set_winsize(target, cols, rows);
                0
            }
        }

        // Process / Lifecycle
        SYS_EXIT => {
            if let Some(pid) = crate::scheduler::current_user_pid() {
                crate::user::set_exiting_pid_tmp(pid);
            }
            process::exit_current();
            syscall::EXIT_TO_KERNEL
        }
        SYS_EXEC => sys_exec(arg0, arg1, arg2),
        SYS_THREAD_SPAWN => process::spawn_user_thread(arg0, arg1, arg2),
        SYS_THREAD_EXIT => {
            // Last thread of the process? Then exiting it exits the whole process.
            let pid = crate::scheduler::current_user_pid().unwrap_or(0);
            if pid != 0 && process::live_thread_count(pid) <= 1 {
                crate::user::set_exiting_pid_tmp(pid);
                process::exit_current();
            } else {
                process::exit_current_thread();
            }
            syscall::EXIT_TO_KERNEL
        }
        SYS_THREAD_JOIN => {
            if process::join_current(arg0) {
                syscall::BLOCK_TO_SCHEDULER
            } else {
                0 // already exited
            }
        }
        SYS_THREAD_NEXT => crate::task::thread::next_thread_in_pid(arg0, arg1).unwrap_or(u64::MAX),
        SYS_THREAD_INFO => {
            if arg1 != 0 {
                match crate::task::thread::thread_info(arg0) {
                    Some(info) => {
                        unsafe {
                            core::ptr::write_unaligned(
                                arg1 as *mut crate::task::thread::ThreadInfo,
                                info,
                            );
                        }
                        0
                    }
                    None => u64::MAX,
                }
            } else {
                u64::MAX
            }
        }
        SYS_KILL => { process::kill_pid(arg0); 0 }
        SYS_SIGINT_FG => {
            let leaf = process::foreground_leaf(arg0);
            if leaf != 0 && leaf != arg0 { process::send_sigint(leaf); 1 } else { 0 }
        }
        SYS_WAIT => sys_wait(arg0),
        SYS_PROCESS_INFO => {
            if arg1 != 0 {
                match process::info(arg0) {
                    Some(info) => { unsafe { core::ptr::write_unaligned(arg1 as *mut ProcessInfo, info); } 0 }
                    None => u64::MAX,
                }
            } else {
                match arg0 {
                    0 => process::current_pid(),
                    1 => process::count(),
                    2 => process::first_pid().unwrap_or(0),
                    _ => u64::MAX,
                }
            }
        }
        SYS_PROCESS_NEXT => process::next_pid_after(arg0).unwrap_or(u64::MAX),
        SYS_SLEEP => sys_sleep(arg0, arg1),

        // Memory
        SYS_MEM_INFO => {
            if let Some(stats) = crate::pmm::stats() {
                ((stats.total_kib() as u64) << 32) | stats.used_kib() as u64
            } else { 0 }
        }
        SYS_HEAP_ALLOC => sys_heap_alloc(arg0),
        SYS_HEAP_FREE => sys_heap_free(arg0),

        // Signals
        SYS_SIGNAL_CATCH => {
            if let Some(pid) = crate::scheduler::current_user_pid() { process::sigint_set_catch(pid, arg0 != 0); } 0
        }
        SYS_SIGNAL_CHECK => {
            if let Some(pid) = crate::scheduler::current_user_pid() {
                if process::sigint_check_and_clear(pid) { 1 } else { 0 }
            } else { 0 }
        }

        // IPC
        SYS_IPC_OPEN => {
            if arg0 == 0 || arg1 == 0 { u64::MAX }
            else { let name = unsafe { core::slice::from_raw_parts(arg0 as *const u8, arg1 as usize) }; ipc::open(name) }
        }
        SYS_IPC_SEND => {
            let channel_id = arg0;
            let buf_ptr    = arg1;
            let buf_len    = arg2 as usize;
            if buf_ptr == 0 || buf_len == 0 { return u64::MAX; }
            let data = unsafe { core::slice::from_raw_parts(buf_ptr as *const u8, buf_len) };
            let sender = crate::scheduler::current_user_pid().unwrap_or(0);
            match ipc::try_send(channel_id, sender, data) {
                ipc::SendResult::Ok => 0,
                ipc::SendResult::Error => u64::MAX,
                ipc::SendResult::Block => {
                    if let Some(pid) = crate::scheduler::current_user_pid() {
                        ipc::add_send_waiter(channel_id, pid);
                        process::set_wait_target(pid, process::WaitTarget::Ipc(channel_id));
                        process::set_sleeping(pid);
                    }
                    syscall::BLOCK_TO_SCHEDULER
                }
            }
        }
        SYS_IPC_RECV => {
            let channel_id = arg0;
            let buf_ptr    = arg1;
            let buf_len    = arg2 as usize;
            if buf_ptr == 0 || buf_len == 0 { return u64::MAX; }
            let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };
            let pid = crate::scheduler::current_user_pid().unwrap_or(0);
            // try_recv registers the waiter and marks it sleeping atomically on Block.
            match ipc::try_recv(channel_id, buf, pid) {
                ipc::RecvResult::Ok(len) => len as u64,
                ipc::RecvResult::Error   => u64::MAX,
                ipc::RecvResult::Block   => syscall::BLOCK_TO_SCHEDULER,
            }
        }
        SYS_IPC_TRY_RECV => {
            let channel_id = arg0;
            let buf_ptr    = arg1;
            let buf_len    = arg2 as usize;
            if buf_ptr == 0 || buf_len == 0 { return u64::MAX; }
            let buf = unsafe { core::slice::from_raw_parts_mut(buf_ptr as *mut u8, buf_len) };
            match ipc::try_recv_nonblock(channel_id, buf) {
                ipc::RecvResult::Ok(len) => len as u64,
                ipc::RecvResult::Error   => u64::MAX,
                // No message available right now; caller polls again later.
                ipc::RecvResult::Block   => 0,
            }
        }
        SYS_IPC_CLOSE => { ipc::close(arg0); 0 }

        // File I/O
        SYS_OPEN => sys_open(arg0, arg1),
        SYS_CLOSE => sys_close(arg0),
        SYS_READ => sys_read(arg0, arg1, arg2),
        SYS_TRY_READ => sys_try_read(arg0, arg1, arg2),
        SYS_WRITE => sys_write(arg0, arg1, arg2),
        SYS_IOCTL => sys_ioctl(arg0, arg1, arg2),
        SYS_PIPE => sys_pipe(arg0),
        SYS_CREATE => sys_create(arg0, arg1),
        SYS_UNLINK => sys_unlink(arg0, arg1),
        SYS_MKDIR => sys_mkdir(arg0, arg1),
        SYS_RMDIR => sys_rmdir(arg0, arg1),

        // Hardware / Driver
        SYS_PCI_INFO => sys_pci_info(arg0, arg1),
        SYS_IOPORT_REQUEST => {
            let caller = crate::scheduler::current_user_pid().unwrap_or(0);
            if process::privilege_level(caller) > process::PrivilegeLevel::Driver { return u64::MAX; }
            let port  = arg0 as u16;
            let count = arg1 as u16;
            for i in 0..count { crate::gdt::iopb_allow_port(port + i); }
            0
        }
        SYS_IRQ_WAIT => {
            let caller = crate::scheduler::current_user_pid().unwrap_or(0);
            if process::privilege_level(caller) > process::PrivilegeLevel::Driver { return u64::MAX; }
            let irq = arg0 as u8;
            process::block_current(process::WaitTarget::Irq(irq));
            syscall::BLOCK_TO_SCHEDULER
        }
        SYS_DMA_ALLOC => sys_dma_alloc(arg0, arg1),
        SYS_DMA_FREE => sys_dma_free(arg0),
        SYS_PCI_BAR_MAP => sys_pci_bar_map(arg0, arg1),
        SYS_PCI_BAR_UNMAP => sys_pci_bar_unmap(arg0),

        // Keyboard
        SYS_KEYBOARD_POLL => {
            if kbd_locked_out() { 0 } else { crate::drivers::keyboard::get_raw().map(|c| c as u64).unwrap_or(0) }
        }

        // System / Misc
        SYS_CPU_INFO => match arg0 {
            0 => crate::handlers::interrupts::timer_ticks(),
            1 => crate::handlers::interrupts::user_cpu_ticks(),
            2 => crate::handlers::interrupts::kernel_cpu_ticks(),
            3 => crate::handlers::interrupts::idle_cpu_ticks(),
            4 => crate::smp::cpu_count() as u64,
            5 => crate::smp::bsp_apic_id() as u64,
            6 => crate::smp::current_cpu_index() as u64,
            7 => crate::smp::apic_id_for_cpu_index(arg1 as usize).unwrap_or(0xff) as u64,
            8 => crate::handlers::interrupts::idle_cpu_ticks_for_cpu(arg1 as usize),
            9 => crate::handlers::interrupts::kernel_cpu_ticks_for_cpu(arg1 as usize),
            10 => crate::handlers::interrupts::user_cpu_ticks_for_cpu(arg1 as usize),
            pid => process::cpu_ticks(pid).unwrap_or(u64::MAX),
        },
        SYS_SHUTDOWN => crate::drivers::power::shutdown(),
        SYS_REBOOT => crate::drivers::power::reboot(),
        SYS_READDIR => sys_readdir(arg0, arg1, arg2),

        // Kernel modules
        SYS_MODULE_LOAD => {
            let caller = crate::scheduler::current_user_pid().unwrap_or(0);
            if process::privilege_level(caller) > process::PrivilegeLevel::User { return u64::MAX - 1; }
            if arg0 == 0 || arg1 == 0 { return u64::MAX; }
            let path_bytes = unsafe { core::slice::from_raw_parts(arg0 as *const u8, arg1 as usize) };
            match core::str::from_utf8(path_bytes) {
                Ok(path) => crate::kmod::load(path),
                Err(_) => u64::MAX,
            }
        }
        SYS_MODULE_UNLOAD => {
            let caller = crate::scheduler::current_user_pid().unwrap_or(0);
            if process::privilege_level(caller) > process::PrivilegeLevel::User { return u64::MAX - 1; }
            if crate::kmod::unload(arg0 as u32) { 0 } else { u64::MAX }
        }
        SYS_MODULE_LIST => crate::kmod::list(arg0, arg1),
        SYS_MODULE_INFO => crate::kmod::info(arg0 as u32, arg1),

        _ => u64::MAX,
    }
}

fn sys_wait(pid: u64) -> u64 {
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

fn sys_sleep(duration: u64, unit: u64) -> u64 {
    if duration == 0 {
        return 0;
    }
    // SLEEP_UNIT_TICK: block until the next timer interrupt fires.
    if unit == SLEEP_UNIT_TICK {
        crate::process::block_current(crate::process::WaitTarget::Tick);
        return syscall::BLOCK_TO_SCHEDULER;
    }
    let tsc_per_ms = unsafe { TSC_PER_MS };
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
fn sys_readdir(ptr: u64, len: u64, out_ptr: u64) -> u64 {
    if out_ptr == 0 { return u64::MAX; }
    let path = if ptr == 0 || len == 0 {
        "/"
    } else {
        let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) };
        core::str::from_utf8(bytes).unwrap_or("/")
    };
    let out = out_ptr as *mut UserDirEntry;
    let mut n = 0usize;

    if path == "/dev" || path == "/dev/" {
        crate::devfs::for_each(|name| {
            let display = name.strip_prefix("/dev/").unwrap_or(name);
            unsafe { write_dirent(out, n, 2, display); } // 2 = device
            n += 1;
        });
        return n.min(READDIR_CAP) as u64;
    }

    let mut entries = [crate::vfs::DirEntry::empty(); READDIR_CAP];
    match crate::vfs::read_dir(path, &mut entries) {
        Ok(count) => {
            for entry in &entries[..count] {
                let kind = match entry.kind {
                    crate::vfs::FileType::Directory => 1u8,
                    crate::vfs::FileType::File => 0u8,
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

fn sys_dma_alloc(size: u64, phys_out_ptr: u64) -> u64 {
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
        if phys_out_ptr != 0 {
            core::ptr::write_unaligned(phys_out_ptr as *mut u64, phys);
        }
        let allocs = &mut *DMA_ALLOCS.0.get();
        allocs.push(DmaAlloc { pid: caller, virt, phys, size: aligned });
    }
    virt
}

fn sys_dma_free(virt: u64) -> u64 {
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

fn sys_pci_bar_map(bdf: u64, bar_index: u64) -> u64 {
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

fn sys_pci_bar_unmap(virt: u64) -> u64 {
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

fn sys_heap_alloc(size: u64) -> u64 {
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

fn sys_heap_free(virt: u64) -> u64 {
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

fn sys_exec(ptr: u64, len: u64, stdio_pack: u64) -> u64 {
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
    let bytes_owned =
        unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) }.to_vec();
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
        let pid = exec::spawn_user_with_args(path, &args);
        if pid != 0 {
            crate::fd::alloc_fd_at(pid, 0, crate::fd::FdEntry::ConsoleIn);
            crate::fd::alloc_fd_at(pid, 1, crate::fd::FdEntry::ConsoleOut);
            crate::fd::alloc_fd_at(pid, 2, crate::fd::FdEntry::ConsoleOut);
        }
        pid
    } else {
        exec::spawn_user_with_fds_and_args(path, &args, caller, stdin_fd, stdout_fd)
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
            if let Some(tty) = crate::fd::get_fd(caller, 0) {
                crate::fd::alloc_fd_at(pid, 3, tty);
            }
        }
    }
    if pid == 0 || pid == u64::MAX {
        crate::log_warn!("sys_exec: spawn failed for '{}' (caller={}, stdio={:#x})", path, caller, stdio_pack);
    }
    pid
}

fn sys_pipe(out_ptr: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    let Some(pipe_id) = crate::pipe::create() else {
        crate::log_warn!("sys_pipe: pipe::create failed (pid={})", caller);
        return u64::MAX;
    };
    let read_fd  = fd::alloc_fd(caller, fd::FdEntry::PipeRead(pipe_id));
    let write_fd = fd::alloc_fd(caller, fd::FdEntry::PipeWrite(pipe_id));
    match (read_fd, write_fd) {
        (Some(r), Some(w)) => {
            if out_ptr != 0 {
                unsafe {
                    core::ptr::write_unaligned(out_ptr as *mut u64, r as u64);
                    core::ptr::write_unaligned((out_ptr + 8) as *mut u64, w as u64);
                }
            }
            0
        }
        _ => {
            // fd table full: roll back whatever we did grab so we don't leak it.
            if let Some(r) = read_fd { fd::free_fd(caller, r); }
            if let Some(w) = write_fd { fd::free_fd(caller, w); }
            crate::log_warn!("sys_pipe: out of fds (pid={}, MAX_FD={})", caller, fd::MAX_FD);
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
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len as usize) }.to_vec();
    core::str::from_utf8(&bytes).ok().map(alloc::string::String::from)
}

fn sys_create(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    let (node, generation) = match crate::vfs::create(&path) {
        Ok(h) => h,
        // create-or-truncate, so `echo ... > file` overwrites cleanly.
        Err(crate::vfs::FsError::AlreadyExists) => match crate::vfs::lookup(&path) {
            Ok(crate::vfs::VfsNode::File { node, generation, .. }) => {
                crate::vfs::truncate(node, generation);
                (node, generation)
            }
            _ => return u64::MAX,
        },
        Err(_) => return u64::MAX,
    };
    match fd::alloc_fd(caller, fd::FdEntry::File { node, generation, offset: 0 }) {
        Some(fd) => fd as u64,
        None => u64::MAX,
    }
}

fn sys_unlink(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    crate::vfs::unlink(&path).map_or(u64::MAX, |_| 0)
}

fn sys_mkdir(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    crate::vfs::mkdir(&path).map_or(u64::MAX, |_| 0)
}

fn sys_rmdir(ptr: u64, len: u64) -> u64 {
    let Some(path) = read_user_path(ptr, len) else { return u64::MAX };
    crate::vfs::rmdir(&path).map_or(u64::MAX, |_| 0)
}

fn sys_open(path_ptr: u64, path_len: u64) -> u64 {
    if path_ptr == 0 || path_len == 0 {
        return u64::MAX;
    }
    let path_bytes =
        unsafe { core::slice::from_raw_parts(path_ptr as *const u8, path_len as usize) };
    let path = match core::str::from_utf8(path_bytes) {
        Ok(s) => s,
        Err(_) => return u64::MAX,
    };
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::vfs::lookup(path) {
        Ok(crate::vfs::VfsNode::Device(ops)) => {
            let handle = (ops.open)();
            if handle == u64::MAX {
                return u64::MAX;
            }
            match fd::alloc_fd(caller, fd::FdEntry::Device { ops, handle }) {
                Some(fd) => fd as u64,
                None => {
                    (ops.close)(handle);
                    u64::MAX
                }
            }
        }
        Ok(crate::vfs::VfsNode::File { node, generation, len: _ }) => {
            match fd::alloc_fd(caller, fd::FdEntry::File { node, generation, offset: 0 }) {
                Some(fd) => fd as u64,
                None => u64::MAX,
            }
        }
        Ok(crate::vfs::VfsNode::Dir) => u64::MAX,
        Err(_) => u64::MAX,
    }
}

fn sys_close(fd: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    if fd::free_fd(caller, fd as usize) {
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
fn console_writable() -> bool {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match crate::drivers::fb_owner::owner() {
        None => true,
        Some(owner) => owner == caller,
    }
}

fn kbd_locked_out() -> bool {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    matches!(crate::drivers::fb_owner::owner(), Some(o) if o != caller)
}

/// Non-blocking read. Returns the byte count, `0` when no data is available right
/// now (would block), or `u64::MAX` on EOF (pipe writer gone) or a bad fd. Lets a
/// single-threaded program (e.g. the GUI terminal) poll a pipe without sleeping.
fn sys_try_read(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return 0;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match fd::get_fd(caller, fd as usize) {
        Some(fd::FdEntry::ConsoleIn) => {
            if kbd_locked_out() { return 0; }
            if let Some(ch) = crate::drivers::keyboard::get_raw() {
                unsafe { core::ptr::write(buf_ptr as *mut u8, ch); }
                1
            } else {
                0
            }
        }
        Some(fd::FdEntry::PipeRead(pipe_id)) => {
            if crate::pipe::is_empty(pipe_id) {
                return if crate::pipe::writer_closed(pipe_id) { u64::MAX } else { 0 };
            }
            let mut kbuf = alloc::vec![0u8; buf_len as usize];
            let n = crate::pipe::read(pipe_id, &mut kbuf);
            unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n); }
            n as u64
        }
        _ => u64::MAX,
    }
}

fn sys_read(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return 0;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match fd::get_fd(caller, fd as usize) {
        Some(fd::FdEntry::ConsoleIn) => {
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
        Some(fd::FdEntry::PipeRead(pipe_id)) => {
            // Decide read-vs-EOF-vs-block atomically. pipe and thread state share the same
            // reentrant lock, so holding it across the whole decision serialises us against
            // a concurrent writer's write+close+notify. Otherwise (separate lock acquisitions
            // for is_empty / writer_closed / set_sleeping) a writer that writes-then-exits in
            // the gap makes us return EOF while its bytes sit unread in the buffer — the data
            // loss that truncated `cmd | less` output under load.
            crate::task::thread::with_threads_lock(|| {
                if !crate::pipe::is_empty(pipe_id) {
                    let mut kbuf = alloc::vec![0u8; buf_len as usize];
                    let n = crate::pipe::read(pipe_id, &mut kbuf);
                    unsafe { core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n); }
                    n as u64
                } else if crate::pipe::writer_closed(pipe_id) {
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
        Some(fd::FdEntry::File { node, generation, offset }) => {
            let want = buf_len as usize;
            let mut kbuf = alloc::vec![0u8; want];
            let n = crate::vfs::read_at(node, generation, offset, &mut kbuf);
            if n == 0 {
                return 0;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf_ptr as *mut u8, n);
            }
            let _ = fd::set_fd(caller, fd as usize, fd::FdEntry::File { node, generation, offset: offset + n });
            n as u64
        }
        Some(fd::FdEntry::Device { ops, handle }) => {
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

fn sys_write(fd: u64, buf_ptr: u64, buf_len: u64) -> u64 {
    if buf_ptr == 0 || buf_len == 0 {
        return 0;
    }
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match fd::get_fd(caller, fd as usize) {
        Some(fd::FdEntry::ConsoleOut) => {
            // same as SYS_CONSOLE_WRITE
            let src = buf_ptr as *const u8;
            let len = buf_len as usize;
            const CHUNK: usize = 256;
            let mut buf = [0u8; CHUNK];
            let mut offset = 0usize;
            let fb_owner = crate::drivers::fb_owner::owner();
            let do_fb = fb_owner.is_none() || fb_owner == Some(caller);
            while offset < len {
                let n = (len - offset).min(CHUNK);
                unsafe { core::ptr::copy_nonoverlapping(src.add(offset), buf.as_mut_ptr(), n); }
                let chunk = unsafe { core::str::from_utf8_unchecked(&buf[..n]) };
                if do_fb { console::screen_print(chunk); }
                if crate::init::is_verbose() { crate::serial_print!("{}", chunk); }
                offset += n;
            }
            buf_len
        }
        Some(fd::FdEntry::PipeWrite(pipe_id)) => {
            let mut kbuf = alloc::vec![0u8; buf_len as usize];
            unsafe { core::ptr::copy_nonoverlapping(buf_ptr as *const u8, kbuf.as_mut_ptr(), buf_len as usize); }
            let n = crate::pipe::write(pipe_id, &kbuf);
            crate::process::notify_pipe_readers(pipe_id);
            n as u64
        }
        Some(fd::FdEntry::File { node, generation, offset }) => {
            let len = buf_len as usize;
            let mut kbuf = alloc::vec![0u8; len];
            unsafe { core::ptr::copy_nonoverlapping(buf_ptr as *const u8, kbuf.as_mut_ptr(), len); }
            let n = crate::vfs::write_at(node, generation, offset, &kbuf);
            let _ = fd::set_fd(caller, fd as usize, fd::FdEntry::File { node, generation, offset: offset + n });
            n as u64
        }
        Some(fd::FdEntry::Device { ops, handle }) => {
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

fn sys_pci_info(index: u64, out_ptr: u64) -> u64 {
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
        unsafe { core::ptr::write_unaligned(out_ptr as *mut PciDeviceInfo, info); }
    }
    cache.len() as u64
}

fn sys_ioctl(fd: u64, cmd: u64, arg: u64) -> u64 {
    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
    match fd::get_fd(caller, fd as usize) {
        Some(fd::FdEntry::Device { ops, handle }) => {
            let r = (ops.ioctl)(handle, cmd, arg);
            if r == syscall::BLOCK_TO_SCHEDULER as i64 {
                return syscall::BLOCK_TO_SCHEDULER;
            }
            r as u64
        }
        _ => u64::MAX,
    }
}


