use alloc::alloc::{Layout, alloc_zeroed};

use crate::{process, vfs};

const KXE_MAGIC: &[u8; 4] = b"KXE\0";
const USER_BASE: u64 = 0x0000_0080_0000_0000;
const USER_STACK_TOP: u64 = 0x0000_0080_8000_0000;
const USER_STACK_SIZE: u64 = 0x4000;

pub const KXE_FLAG_DRIVER: u32 = 1;

#[repr(C, packed)]
struct KxeHeader {
    magic: [u8; 4],
    entry: u64,
    code_offset: u64,
    code_size: u64,
    flags: u32,
    reserved: u32,
}

fn spawn_init() -> u64 {
    spawn_process("/bin/init.kxe", crate::process::PrivilegeLevel::System)
}

fn init_exit_handler() {
    let init_pid = spawn_init();
    if init_pid != 0 {
        crate::log_info!("init respawning: pid={}", init_pid);
    }
}

pub fn load_init(tsc_per_ms: u64) -> ! {
    let _ = vfs::read_file("/bin/init.kxe").unwrap_or_else(|_| panic!("init not found"));
    unsafe {
        crate::user::TSC_PER_MS = tsc_per_ms;
    }
    crate::log_info!("init.kxe loaded");
    crate::scheduler::on_user_exit(init_exit_handler);
    let init_pid = spawn_init();
    if init_pid == 0 {
        panic!("init spawn failed");
    }
    crate::log_info!("init: pid={}", init_pid);
    crate::scheduler::enter_next_process();
}

pub fn spawn(path: &str) -> u64 {
    spawn_process(path, crate::process::PrivilegeLevel::System)
}

/// Spawn from user space with explicit stdin/stdout fds (inherited from caller's fd table).
/// stdin_fd / stdout_fd = 0xFFFF means use console default.
pub fn spawn_user_with_fds(path: &str, caller_pid: u64, stdin_fd: u16, stdout_fd: u16) -> u64 {
    let image = match crate::vfs::read_file(path) {
        Ok(data) => data,
        Err(_) => return 0,
    };
    let Some(kxe) = parse_kxe(&image) else { return 0; };
    if kxe.flags & KXE_FLAG_DRIVER != 0 { return 0; }
    let pid = spawn_kxe(path, kxe, crate::process::PrivilegeLevel::User);
    if pid == 0 { return 0; }

    // fd 2 always = ConsoleOut
    crate::fd::alloc_fd_at(pid, 2, crate::fd::FdEntry::ConsoleOut);

    // stdin (fd 0)
    let stdin_entry = if stdin_fd == 0xFFFF {
        crate::fd::FdEntry::ConsoleIn
    } else {
        crate::fd::get_fd(caller_pid, stdin_fd as usize).unwrap_or(crate::fd::FdEntry::ConsoleIn)
    };
    crate::fd::alloc_fd_at(pid, 0, stdin_entry);

    // stdout (fd 1)
    let stdout_entry = if stdout_fd == 0xFFFF {
        crate::fd::FdEntry::ConsoleOut
    } else {
        crate::fd::get_fd(caller_pid, stdout_fd as usize).unwrap_or(crate::fd::FdEntry::ConsoleOut)
    };
    crate::fd::alloc_fd_at(pid, 1, stdout_entry);

    pid
}

/// Spawn from user space. Rejects driver binaries.
pub fn spawn_user(path: &str) -> u64 {
    let image = match vfs::read_file(path) {
        Ok(data) => data,
        Err(_) => return 0,
    };
    let Some(kxe) = parse_kxe(&image) else {
        return 0;
    };
    if kxe.flags & KXE_FLAG_DRIVER != 0 {
        return 0;
    }
    spawn_kxe(path, kxe, crate::process::PrivilegeLevel::User)
}

pub fn spawn_driver(path: &str) -> u64 {
    spawn_process(path, crate::process::PrivilegeLevel::Driver)
}

pub fn is_driver_kxe(path: &str) -> bool {
    let image = match vfs::read_file(path) {
        Ok(data) => data,
        Err(_) => return false,
    };
    let Some(kxe) = parse_kxe(&image) else {
        return false;
    };
    kxe.flags & KXE_FLAG_DRIVER != 0
}

fn spawn_process(path: &str, privilege: crate::process::PrivilegeLevel) -> u64 {
    let image = match vfs::read_file(path) {
        Ok(data) => data,
        Err(_) => return 0,
    };
    let Some(kxe) = parse_kxe(&image) else {
        return 0;
    };
    spawn_kxe(path, kxe, privilege)
}

struct KxeImage<'a> {
    entry: u64,
    code: &'a [u8],
    flags: u32,
}

fn parse_kxe(image: &[u8]) -> Option<KxeImage<'_>> {
    if image.len() < core::mem::size_of::<KxeHeader>() {
        return None;
    }
    let header = unsafe { core::ptr::read_unaligned(image.as_ptr() as *const KxeHeader) };
    if &header.magic != KXE_MAGIC {
        return None;
    }
    let code_offset = header.code_offset as usize;
    let code_size = header.code_size as usize;
    let end = code_offset.checked_add(code_size)?;
    if code_offset < core::mem::size_of::<KxeHeader>() || end > image.len() {
        return None;
    }
    Some(KxeImage {
        entry: USER_BASE + header.entry,
        code: &image[code_offset..end],
        flags: header.flags,
    })
}

fn spawn_kxe(
    path: &str,
    image: KxeImage<'_>,
    privilege: crate::process::PrivilegeLevel,
) -> u64 {
    let Some(cr3) = create_process_space(image.entry, image.code) else {
        return 0;
    };
    let pid = process::spawn_user_process(path, image.entry, USER_STACK_TOP, cr3, privilege);
    if pid != 0 {
        process::set_memory_bytes(pid, image.code.len() as u64 + USER_STACK_SIZE);
    }
    pid
}

fn create_process_space(entry: u64, code: &[u8]) -> Option<u64> {
    unsafe {
        let cr3 = crate::vmm::create_address_space().ok()?;
        let code_size = code.len() as u64;
        let code_pages = code_size.div_ceil(4096).max(1);
        let code_size_bytes = code_pages * 4096;
        let code_phys = alloc_page_range(code_size_bytes as usize)?;
        core::ptr::write_bytes(code_phys as *mut u8, 0, code_size_bytes as usize);
        core::ptr::copy_nonoverlapping(code.as_ptr(), code_phys as *mut u8, code.len());
        let code_base = entry & !0xfff;
        crate::vmm::map_range(
            cr3,
            code_base,
            code_phys,
            code_size_bytes,
            crate::vmm::MapFlags::USER_READ_WRITE,
        )
        .ok()?;
        let stack_base = USER_STACK_TOP - USER_STACK_SIZE;
        let stack_phys = alloc_page_range(USER_STACK_SIZE as usize)?;
        crate::vmm::map_range(
            cr3,
            stack_base,
            stack_phys,
            USER_STACK_SIZE,
            crate::vmm::MapFlags::USER_READ_WRITE.no_execute(),
        )
        .ok()?;
        Some(cr3)
    }
}

fn alloc_page_range(size: usize) -> Option<u64> {
    let layout = Layout::from_size_align(size, 4096).ok()?;
    let ptr = unsafe { alloc_zeroed(layout) };
    if ptr.is_null() {
        None
    } else {
        Some(ptr as u64)
    }
}
