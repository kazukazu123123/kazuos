#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;

pub mod allocator;
pub mod audio;
pub mod beep_songs;
pub mod boot;
pub mod console;
pub mod debug;
pub mod devfs;
pub mod drivers;
pub mod exec;
pub mod fd;
pub mod gdt;
pub mod handlers;
pub mod ipc;
pub mod idt;
pub mod init;
pub mod log;
pub mod memory;
pub mod platform;
pub mod pipe;
pub mod pmm;
pub mod process;
pub mod scheduler;
pub mod syscall;
pub mod task;
pub mod terminal;
pub mod tty;
pub mod user;
pub mod user_programs;
pub mod util;
pub mod vfs;
pub mod vmm;

pub use kazuos_shared::{BootInfo, FramebufferInfo, MemoryMapEntry};

#[repr(C, align(4096))]
struct Stack([u8; 1024 * 1024]);
static mut STACK: Stack = Stack([0; 1024 * 1024]);

core::arch::global_asm!(
    ".global _start",
    "_start:",
    "    cli",
    "    lea rsp, [rip + {0} + 0x100000]",
    "    call kernel_main",
    "    cli",
    "    hlt",
    sym STACK,
);

#[unsafe(no_mangle)]
pub extern "C" fn kernel_main(boot_info: &'static BootInfo) {
    unsafe {
        gdt::init((core::ptr::addr_of!(STACK) as u64) + 1024 * 1024);
    }
    boot::init(boot_info);
    syscall::init();
    user::init();
    let _state = init::run(boot_info);
    let initramfs = boot_info.initrd_slice();
    if initramfs.is_empty() {
        panic!("no initrd provided by bootloader");
    }
    if let Err(error) = vfs::init(initramfs) {
        panic!("initramfs invalid: {:?}", error);
    }
    register_audio_device();
    // Disable interrupts while spawning processes so the timer cannot preempt
    // the kernel before all processes are fully configured (privilege levels etc.).
    unsafe { core::arch::asm!("cli"); }
    let pid = exec::spawn("/bin/shell.kxe");
    if pid == 0 {
        panic!("shell spawn failed");
    }

    crate::scheduler::enter_next_process();
}

fn register_audio_device() {
    use drivers::pci::{Device, ScanKind};

    let mut ac97: Option<Device> = None;
    let mut es1371: Option<Device> = None;

    drivers::pci::scan(ScanKind::Pci, |d| {
        if d.class_code == 0x04 && d.subclass == 0x01 {
            match d.vendor_id {
                0x1274 => {
                    if es1371.is_none() {
                        es1371 = Some(d);
                    }
                }
                _ => {
                    if ac97.is_none() {
                        ac97 = Some(d);
                    }
                }
            }
        }
    });

    if let Some(dev) = ac97 {
        crate::serial_println!(
            "AC97 audio device found: {:04x}:{:04x}",
            dev.vendor_id,
            dev.device_id
        );
        devfs::register("/dev/audio", &drivers::ac97::AUDIO_OPS);
    } else if let Some(dev) = es1371 {
        crate::serial_println!(
            "ES1371 audio device found: {:04x}:{:04x}",
            dev.vendor_id,
            dev.device_id
        );
        devfs::register("/dev/audio", &drivers::es1371::AUDIO_OPS);
    } else {
        crate::serial_println!("No audio device found");
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    crate::log_fatal!("{}", info);
    loop {
        util::pause();
    }
}
