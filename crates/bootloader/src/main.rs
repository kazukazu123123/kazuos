#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::time::Duration;
use kazuos_shared::{BootInfo, FramebufferInfo, MemoryMapEntry};
use uefi::boot;
use uefi::entry;
use uefi::fs::FileSystem;
use uefi::mem::memory_map::MemoryMap;
use uefi::mem::memory_map::MemoryType;
use uefi::prelude::*;
use uefi::proto::console::gop::GraphicsOutput;
use uefi::proto::console::text::{Key, ScanCode};
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::system::{with_config_table, with_stdout};
use uefi::table::cfg::ConfigTableEntry;
use xmas_elf::program::{ProgramHeader, Type};
use xmas_elf::{ElfFile, header};

#[entry]
fn main() -> Status {
    uefi::helpers::init().unwrap();
    clear_screen();

    uefi::println!("KazuOS Bootloader");

    let cmdline = boot_menu();
    clear_screen();
    uefi::println!("Command line: {}", cmdline);

    let kernel_bytes = match load_file(cstr16!("\\KazuOS\\kernel.elf")) {
        Ok(data) => data,
        Err(e) => {
            uefi::println!("Failed to load kernel.elf: {:?}", e);
            boot::stall(Duration::from_secs(5));
            return Status::DEVICE_ERROR;
        }
    };

    let font_data = match load_file(cstr16!("\\KazuOS\\font.ttf")) {
        Ok(data) => data,
        Err(_) => {
            uefi::println!("[KazuOS] font.ttf not found, using built-in bitmap font");
            Vec::new()
        }
    };

    let initrd_data = match load_file(cstr16!("\\KazuOS\\initrd.kfs")) {
        Ok(data) => Some(data),
        Err(_) => {
            uefi::println!("[KazuOS] initrd.kfs not found");
            None
        }
    };

    uefi::println!("Kernel: {} bytes", kernel_bytes.len());
    uefi::println!("Font: {} bytes", font_data.len());
    uefi::println!("Initrd: {} bytes", initrd_data.as_ref().map(|d| d.len()).unwrap_or(0));

    let elf = match ElfFile::new(&kernel_bytes) {
        Ok(elf) => elf,
        Err(_) => {
            uefi::println!("Invalid ELF file");
            boot::stall(Duration::from_secs(5));
            return Status::DEVICE_ERROR;
        }
    };

    if elf.header.pt2.machine().as_machine() != header::Machine::X86_64 {
        uefi::println!("Kernel is not x86_64 ELF");
        boot::stall(Duration::from_secs(5));
        return Status::DEVICE_ERROR;
    }

    let mut kernel_start = u64::MAX;
    let mut kernel_end = 0u64;
    for ph in elf.program_iter() {
        if let ProgramHeader::Ph64(ph) = ph
            && ph.get_type().unwrap_or(Type::Null) == Type::Load
        {
            let dest = ph.physical_addr;
            let file_size = ph.file_size;
            let mem_size = ph.mem_size;
            let offset = ph.offset as usize;

            uefi::println!(
                "Loading segment: addr={:x}, file_size={}, mem_size={}",
                dest,
                file_size,
                mem_size
            );

            kernel_start = kernel_start.min(dest);
            kernel_end = kernel_end.max(dest + mem_size);

            unsafe {
                core::ptr::write_bytes(dest as *mut u8, 0, mem_size as usize);
                core::ptr::copy_nonoverlapping(
                    kernel_bytes.as_ptr().add(offset),
                    dest as *mut u8,
                    file_size as usize,
                );
            }
        }
    }

    let entry_point = elf.header.pt2.entry_point();
    uefi::println!("Entry point: {:x}", entry_point);

    let handle = boot::get_handle_for_protocol::<GraphicsOutput>().unwrap();
    let mut gop = boot::open_protocol_exclusive::<GraphicsOutput>(handle).unwrap();
    let mode_info = gop.current_mode_info();
    let (width, height) = mode_info.resolution();
    let stride = mode_info.stride();
    let pixel_format = match mode_info.pixel_format() {
        uefi::proto::console::gop::PixelFormat::Rgb => 0u32,
        _ => 1u32,
    };

    let mut fb = gop.frame_buffer();
    let fb_base = fb.as_mut_ptr();
    let fb_size = fb.size();

    let heap_size = 256 * 1024 * 1024;
    let heap_start = unsafe {
        alloc::alloc::alloc(alloc::alloc::Layout::from_size_align(heap_size, 4096).unwrap())
    };

    let rsdp = with_config_table(|entries| {
        for entry in entries {
            if entry.guid == ConfigTableEntry::ACPI2_GUID
                || entry.guid == ConfigTableEntry::ACPI_GUID
            {
                return entry.address as u64;
            }
        }
        0
    });
    uefi::println!("RSDP at {:#x}", rsdp);

    let mut boot_info = alloc::boxed::Box::new(BootInfo {
        framebuffer: FramebufferInfo {
            base: fb_base,
            size: fb_size,
            width,
            height,
            stride,
            pixel_format,
        },
        font_data: font_data.as_ptr(),
        font_size: font_data.len(),
        heap_start,
        heap_size,
        memory_map: core::ptr::null(),
        memory_map_entries: 0,
        kernel_start,
        kernel_end,
        rsdp,
        command_line: cmdline.as_ptr(),
        command_line_len: cmdline.len(),
        initrd_data: initrd_data
            .as_ref()
            .map(|data| data.as_ptr())
            .unwrap_or(core::ptr::null()),
        initrd_size: initrd_data.as_ref().map(|data| data.len()).unwrap_or(0),
    });

    uefi::println!("Exiting Boot Services...");
    boot::stall(Duration::from_millis(500));

    let memory_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };

    const MAX_ENTRIES: usize = 256;
    let mut entries_buffer: [MemoryMapEntry; MAX_ENTRIES] = [MemoryMapEntry {
        phys_start: 0,
        page_count: 0,
        ty: 0,
    }; MAX_ENTRIES];
    let mut count = 0;
    for desc in memory_map.entries() {
        if count < MAX_ENTRIES {
            entries_buffer[count] = MemoryMapEntry {
                phys_start: desc.phys_start,
                page_count: desc.page_count,
                ty: desc.ty.0,
            };
            count += 1;
        }
    }

    boot_info.memory_map = entries_buffer.as_ptr();
    boot_info.memory_map_entries = count;

    let kernel_entry: extern "sysv64" fn(*const BootInfo) =
        unsafe { core::mem::transmute(entry_point) };
    unsafe { core::arch::asm!("cli"); }
    kernel_entry(&*boot_info);

    loop {
        core::arch::x86_64::_mm_pause();
    }
}

fn boot_menu() -> &'static str {
    const ENTRIES: [(&str, &str); 2] = [
        ("KazuOS", ""),
        ("KazuOS (Verbose)", "verbose"),
    ];
    let mut selected = 0usize;
    loop {
        draw_menu(selected);
        let key = read_key();
        match key {
            Some(Key::Special(ScanCode::UP)) => selected = selected.saturating_sub(1),
            Some(Key::Special(ScanCode::DOWN)) => {
                if selected + 1 < ENTRIES.len() {
                    selected += 1;
                }
            }
            Some(Key::Printable(c)) if c == uefi::Char16::try_from('\r').unwrap() => {
                return ENTRIES[selected].1;
            }
            Some(Key::Special(ScanCode::NULL)) => return ENTRIES[selected].1,
            _ => {}
        }
    }
}

fn draw_menu(selected: usize) {
    clear_screen();
    uefi::println!("KazuOS Bootloader");
    uefi::println!("");
    let entries = ["KazuOS", "KazuOS (Verbose)"];
    for (i, entry) in entries.iter().enumerate() {
        if i == selected {
            uefi::println!("> {}", entry);
        } else {
            uefi::println!("  {}", entry);
        }
    }
    uefi::println!("");
    uefi::println!("Use Up/Down and Enter");
}

fn clear_screen() {
    with_stdout(|stdout| {
        let _ = stdout.clear();
        let _ = stdout.set_cursor_position(0, 0);
    });
}

fn read_key() -> Option<Key> {
    loop {
        let key = uefi::system::with_stdin(|stdin| stdin.read_key())
            .ok()
            .flatten();
        if key.is_some() {
            return key;
        }
        boot::stall(Duration::from_millis(10));
    }
}

fn load_file(path: &uefi::CStr16) -> Result<Vec<u8>, uefi::Error> {
    if let Ok(fs) = boot::get_image_file_system(boot::image_handle()) {
        let mut fs = FileSystem::new(fs);
        if let Ok(data) = fs.read(path) {
            return Ok(data);
        }
    }

    // Fall back to scanning every SimpleFileSystem volume. Use a non-exclusive
    // GET_PROTOCOL open: filesystem volumes (e.g. an ISO9660 CD) are typically
    // already open BY_DRIVER, so open_protocol_exclusive would fail with
    // ACCESS_DENIED and we would skip the very volume that holds the file.
    let handles = boot::find_handles::<SimpleFileSystem>()?;
    for handle in handles {
        let params = boot::OpenProtocolParams {
            handle,
            agent: boot::image_handle(),
            controller: None,
        };
        let opened = unsafe {
            boot::open_protocol::<SimpleFileSystem>(params, boot::OpenProtocolAttributes::GetProtocol)
        };
        if let Ok(fs) = opened {
            let mut fs = FileSystem::new(fs);
            if let Ok(data) = fs.read(path) {
                return Ok(data);
            }
        }
    }

    Err(uefi::Error::new(Status::NOT_FOUND, ()))
}

