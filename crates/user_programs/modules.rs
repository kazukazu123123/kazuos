#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

const ENTRY_SIZE: usize = 48;
const MAX_MODULES: usize = 16;

#[repr(C, packed)]
struct ModuleEntry {
    id:       u32,
    pid:      u32,
    status:   u32,
    name:     [u8; 32],
    name_len: u32,
}

fn parse_entry(buf: &[u8]) -> ModuleEntry {
    unsafe { core::ptr::read_unaligned(buf.as_ptr() as *const ModuleEntry) }
}

fn status_str(s: u32) -> &'static str {
    match s {
        0 => "running",
        1 => "unloading",
        _ => "failed",
    }
}

fn cmd_list() {
    let mut buf = [0u8; ENTRY_SIZE * MAX_MODULES];
    let count = sys_module_list(&mut buf) as usize;
    if count == 0 {
        println!("No modules loaded.");
        return;
    }
    println!("{:<4} {:<20} {:<6} {}", "ID", "NAME", "PID", "STATUS");
    for i in 0..count {
        let entry = parse_entry(&buf[i * ENTRY_SIZE..]);
        // Copy fields out of packed struct to avoid unaligned reference errors.
        let id     = entry.id;
        let pid    = entry.pid;
        let status = entry.status;
        let name_len = (entry.name_len as usize).min(32);
        let name = core::str::from_utf8(&entry.name[..name_len]).unwrap_or("?");
        println!("{:<4} {:<20} {:<6} {}", id, name, pid, status_str(status));
    }
}

fn cmd_load(path: &[u8]) {
    let mut full_path = alloc::vec::Vec::new();
    full_path.extend_from_slice(b"/modules/");
    full_path.extend_from_slice(path);
    full_path.extend_from_slice(b".kkm");
    let r = sys_module_load(&full_path);
    if r == u64::MAX - 1 {
        println!("Error: permission denied.");
    } else if r == u64::MAX {
        println!("Error: failed to load module.");
    } else {
        println!("Module loaded: id={}", r);
    }
}

fn cmd_unload(id_str: &[u8]) {
    let mut id: u64 = 0;
    for &b in id_str {
        if b < b'0' || b > b'9' { break; }
        id = id * 10 + (b - b'0') as u64;
    }
    let r = sys_module_unload(id);
    if r == 0 {
        println!("Module {} unloading.", id);
    } else if r == u64::MAX - 1 {
        println!("Error: permission denied.");
    } else {
        println!("Error: module {} not found.", id);
    }
}

fn sys_module_list(buf: &mut [u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_MODULE_LIST => r,
            in("rdi") buf.as_mut_ptr(),
            in("rsi") buf.len(),
            in("rdx") 0u64,
        );
    }
    r
}

fn sys_module_load(path: &[u8]) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_MODULE_LOAD => r,
            in("rdi") path.as_ptr(),
            in("rsi") path.len(),
            in("rdx") 0u64,
        );
    }
    r
}

fn sys_module_unload(id: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") SYS_MODULE_UNLOAD => r,
            in("rdi") id,
            in("rsi") 0u64,
            in("rdx") 0u64,
        );
    }
    r
}

fn cmd_help() {
    println!("Usage: modules <command> [args]");
    println!();
    println!("Commands:");
    println!("  list             List loaded kernel modules");
    println!("  load <path>      Load a kernel module (.kkm)");
    println!("  unload <id>      Unload a module by id");
    println!("  help             Show this help");
}

#[unsafe(no_mangle)]
pub extern "C" fn user_main(argc: u64, argv: u64) -> ! {
    let args = parse_args(argc, argv);

    if args.is_empty() || args[0] == b"help" as &[u8] {
        cmd_help();
    } else if args[0] == b"list" as &[u8] {
        cmd_list();
    } else if args[0] == b"load" as &[u8] {
        if args.len() < 2 {
            println!("Usage: modules load <path>");
        } else {
            cmd_load(args[1]);
        }
    } else if args[0] == b"unload" as &[u8] {
        if args.len() < 2 {
            println!("Usage: modules unload <id>");
        } else {
            cmd_unload(args[1]);
        }
    } else {
        println!("modules: unknown command");
        cmd_help();
    }

    sys_exit(0);
}

fn parse_args(argc: u64, argv: u64) -> alloc::vec::Vec<&'static [u8]> {
    let mut out = alloc::vec::Vec::new();
    if argc == 0 { return out; }
    let ptrs = unsafe { core::slice::from_raw_parts(argv as *const u64, argc as usize) };
    for &ptr in ptrs {
        if ptr == 0 { break; }
        let mut len = 0usize;
        unsafe {
            while *(ptr as *const u8).add(len) != 0 { len += 1; }
            out.push(core::slice::from_raw_parts(ptr as *const u8, len));
        }
    }
    out
}
