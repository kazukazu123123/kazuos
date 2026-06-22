use alloc::vec::Vec;

use crate::drivers::{ioapic, keyboard, lapic, pic};
use crate::handlers::interrupts;
use crate::{allocator, console, drivers, idt, platform, pmm, syscall, vmm};
use kazuos_shared::BootInfo;

static mut VERBOSE: bool = false;
static mut HEARTBEAT_LOG: bool = false;

pub fn is_verbose() -> bool {
    unsafe { VERBOSE }
}

/// Whether the timer should emit the periodic verbose `HEARTBEAT` liveness line.
/// Gated by its own `heartbeat` boot arg (not plain `verbose`) so normal verbose
/// boots stay quiet; the headless test harness opts in to distinguish idle from
/// a real freeze via `--liveness-pattern HEARTBEAT`.
pub fn heartbeat_log() -> bool {
    unsafe { HEARTBEAT_LOG }
}

pub struct InitState {
    pub tsc_per_ms: u64,
}

struct InterruptConfig {
    ioapic: Option<drivers::acpi::IoApicInfo>,
    irq_override: Option<drivers::acpi::IrqOverride>,
    bsp_apic_id: u8,
}

pub fn run(boot_info: &'static BootInfo) -> InitState {
    unsafe {
        VERBOSE = boot_info.command_line().contains("verbose");
        HEARTBEAT_LOG = boot_info.command_line().contains("heartbeat");
    }
    let platform = platform::Platform::detect();
    crate::drivers::serial::init();
    drivers::beep::off();
    allocator::init(boot_info.heap_start, boot_info.heap_size);
    crate::process::init();
    init_console(boot_info);
    print_boot_banner(boot_info);
    init_memory(boot_info);
    drivers::power::init(boot_info.rsdp);
    drivers::pci::init(boot_info.rsdp);
    unsafe {
        vmm::init();
    }
    let hda_irq = drivers::hda::init();
    drivers::e1000::init();
    crate::rng::init();
    init_idt();
    crate::logln!("Platform: {:?}", platform.hypervisor);
    let interrupt_config = init_acpi(boot_info.rsdp);
    init_interrupts(interrupt_config, platform, hda_irq);
    // Build the PCI device cache now, while we are still single-threaded (no APs, no user
    // processes), so the scan can never race concurrent PCI config access — which had made
    // `lspci` return a truncated/empty device list.
    crate::user::build_pci_cache();
    unsafe {
        crate::smp::detect_cpus(boot_info.rsdp);
    }
    InitState {
        tsc_per_ms: calibrate_tsc(boot_info),
    }
}

fn init_console(boot_info: &BootInfo) {
    let pixel_format = match boot_info.framebuffer.pixel_format {
        0 => drivers::framebuffer::PixelFormat::Rgb,
        _ => drivers::framebuffer::PixelFormat::Bgr,
    };
    let fb_info = drivers::framebuffer::FramebufferInfo {
        base: boot_info.framebuffer.base,
        size: boot_info.framebuffer.size,
        width: boot_info.framebuffer.width,
        height: boot_info.framebuffer.height,
        stride: boot_info.framebuffer.stride,
        pixel_format,
    };
    let font_data = Vec::from(boot_info.font_slice());
    let mut console = console::Console::new(fb_info, font_data);
    console.clear();
    console::init(console);
}

fn print_boot_banner(boot_info: &BootInfo) {
    crate::serial_println!("KazuOS kernel started");
    crate::println!("==============================");
    crate::println!("  Welcome to KazuOS Kernel!");
    crate::println!("==============================");
    crate::logln!("");
    crate::logln!("Real kernel mode (ELF)");
    crate::logln!("ExitBootServices: DONE");
    crate::logln!(
        "Framebuffer: {}x{}",
        boot_info.framebuffer.width,
        boot_info.framebuffer.height
    );
    crate::logln!("Heap: {} bytes", boot_info.heap_size);
    crate::logln!("Boot args: {}", boot_info.command_line());
    crate::logln!("");
}

fn init_memory(boot_info: &BootInfo) {
    let memory_map = boot_info.memory_map_slice();
    let is_usable = |ty: u32| ty == 7;
    let max_phys = memory_map
        .iter()
        .filter(|e| is_usable(e.ty))
        .map(|e| e.phys_start + e.page_count * 4096)
        .max()
        .unwrap_or(0);
    let total_frames = (max_phys / 4096) as usize;
    let bitmap_bytes = total_frames.div_ceil(8);
    let bitmap_pages = bitmap_bytes.div_ceil(4096);
    let bitmap_size = bitmap_pages * 4096;
    let bitmap = if bitmap_size > 0 {
        let layout = alloc::alloc::Layout::from_size_align(bitmap_size, 4096).unwrap();
        let ptr = unsafe { alloc::alloc::alloc(layout) };
        if !ptr.is_null() {
            unsafe { core::ptr::write_bytes(ptr, 0, bitmap_size) };
            ptr
        } else {
            core::ptr::null_mut()
        }
    } else {
        core::ptr::null_mut()
    };
    unsafe {
        pmm::init(bitmap, bitmap_size, memory_map);
    }
    pmm::mark_used(0, boot_info.kernel_end);
    pmm::mark_used(
        boot_info.kernel_start,
        boot_info.kernel_end - boot_info.kernel_start,
    );
    pmm::mark_used(boot_info.heap_start as u64, boot_info.heap_size as u64);
    pmm::mark_used(
        boot_info.framebuffer.base as u64,
        boot_info.framebuffer.size as u64,
    );
    pmm::mark_used(boot_info.font_data as u64, boot_info.font_size as u64);
    crate::logln!("PMM initialized: {} frames", bitmap_size * 8);
}

fn init_idt() {
    unsafe {
        idt::init(
            interrupts::keyboard_handler_addr(),
            interrupts::timer_handler_addr(),
            syscall::handler_addr(),
            interrupts::mouse_handler_addr(),
            interrupts::hda_handler_addr(),
        );
    }
    crate::logln!("IDT loaded successfully");
}

fn init_acpi(rsdp: u64) -> InterruptConfig {
    let mut config = InterruptConfig {
        ioapic: None,
        irq_override: None,
        bsp_apic_id: 0,
    };
    if rsdp == 0 {
        crate::logln!("No ACPI RSDP");
        return config;
    }
    unsafe {
        if let Some((sdt_phys, is_xsdt)) = drivers::acpi::parse_rsdp(rsdp) {
            crate::logln!("ACPI SDT at {:#x} (XSDT={})", sdt_phys, is_xsdt);
            if let Some(madt_phys) = drivers::acpi::find_madt(sdt_phys, is_xsdt) {
                let (lapic_addr, ioapic, irq_override, apic_id) =
                    drivers::acpi::parse_madt(madt_phys);
                config.ioapic = ioapic;
                config.irq_override = irq_override;
                config.bsp_apic_id = apic_id;
                crate::logln!(
                    "ACPI MADT: LAPIC addr={:#x}, BSP apic_id={}",
                    lapic_addr,
                    apic_id
                );
                if let Some(ref i) = config.ioapic {
                    crate::logln!(
                        "ACPI IOAPIC: id={} addr={:#x} base={}",
                        i.id,
                        i.addr,
                        i.global_irq_base
                    );
                }
                if let Some(ref o) = config.irq_override {
                    crate::logln!(
                        "ACPI IRQ override: bus={} -> global={} flags={:#x}",
                        o.bus_irq,
                        o.global_irq,
                        o.flags
                    );
                }
            } else {
                crate::logln!("ACPI MADT not found");
            }
        } else {
            crate::logln!("ACPI RSDP invalid");
        }
    }
    config
}

fn init_interrupts(config: InterruptConfig, platform: platform::Platform, hda_irq: Option<u8>) {
    unsafe {
        pic::init();
        pic::mask_all();
        interrupts::set_use_ioapic(false);
        interrupts::set_keyboard_polling(platform.keyboard_polling);

        if platform.use_ioapic {
            if let Some(ref info) = config.ioapic {
                let ioapic = ioapic::IoApic::new(info.addr as u64);
                ioapic.mask_all();
                let mut irq = 1u8;
                let mut flags = 0u16;
                if let Some(ref o) = config.irq_override
                    && o.bus_irq == 1
                {
                    irq = o.global_irq as u8;
                    flags = o.flags;
                }
                ioapic.set_irq_ext(irq, 0x21, config.bsp_apic_id, flags);
                // IRQ12 (PS/2 mouse) at vector 0x2C
                ioapic.set_irq_ext(12, 0x2C, config.bsp_apic_id, 0);
                // HDA IRQ at vector 0x31 (if available)
                if let Some(hda) = hda_irq
                    && hda != 0 && hda != 255
                {
                    ioapic.set_irq_ext(hda, 0x31, config.bsp_apic_id, 0);
                    ioapic.unmask_irq(hda);
                    // The HDA IRQ is delivered via the IOAPIC -> LAPIC (vector
                    // 0x31) regardless of keyboard mode, so its handler MUST end
                    // with a LAPIC EOI. USE_IOAPIC selects that EOI path; it was
                    // only set true in the non-polling branch below, so in
                    // keyboard-polling mode the HDA handler used pic::eoi() and
                    // left LAPIC ISR[0x31] permanently in-service — blocking all
                    // same/lower-priority vectors (including the timer 0x30) on
                    // the BSP. That wedged CPU0 on the first audio interrupt;
                    // round-robin later parking a foreground task there looked
                    // like a hang. Mark IOAPIC EOI now that an IOAPIC-routed IRQ
                    // is live (keyboard/mouse stay masked in polling mode, so
                    // their EOI path is unaffected).
                    interrupts::set_use_ioapic(true);
                    crate::log_info!("IOAPIC HDA IRQ{} enabled", hda);
                }
                if !platform.keyboard_polling {
                    ioapic.unmask_irq(irq);
                    ioapic.unmask_irq(12);
                    interrupts::set_use_ioapic(true);
                    crate::logln!("IOAPIC keyboard IRQ{} enabled", irq);
                    crate::logln!("IOAPIC mouse IRQ12 enabled");
                } else {
                    ioapic.mask_irq(irq);
                    ioapic.mask_irq(12);
                    crate::logln!("IOAPIC present, keyboard polling mode");
                }
            } else if !platform.keyboard_polling {
                pic::unmask_irq(1);
                crate::logln!("PIC keyboard IRQ1 enabled");
            }
        } else if !platform.keyboard_polling {
            pic::unmask_irq(1);
            crate::logln!("PIC keyboard IRQ1 enabled");
        } else {
            crate::logln!("Keyboard polling mode");
        }

        // PIC fallback for HDA: unmask the legacy IRQ if IOAPIC is not used.
        if let Some(hda) = hda_irq
            && hda != 0
            && hda != 255
            && !platform.use_ioapic
        {
            pic::unmask_irq(hda);
            crate::log_info!("PIC HDA IRQ{} enabled", hda);
        }

        if platform.use_lapic_timer {
            lapic::init();
            lapic::enable();
            // Increase initial count to lower timer frequency and reduce
            // context-switch overhead when many processes are running.
            lapic::set_timer(0x30, 0x20000);
            crate::logln!("LAPIC timer enabled");
        } else {
            crate::logln!("LAPIC timer disabled");
        }

        keyboard::init();
        core::arch::asm!("sti");
        crate::logln!("Interrupts enabled");
    }
}

fn calibrate_tsc(_boot_info: &BootInfo) -> u64 {
    let start = crate::util::rdtsc();
    drivers::pit::sleep_oneshot_ms(50);
    let elapsed = crate::util::rdtsc().saturating_sub(start);
    let tsc_per_ms = (elapsed / 50).max(1);
    crate::logln!("TSC calibrated: {} per ms", tsc_per_ms);
    tsc_per_ms
}
