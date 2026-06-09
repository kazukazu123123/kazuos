#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hypervisor {
    None,
    Qemu,
    VirtualBox,
    Vmware,
    HyperV,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct Platform {
    pub hypervisor: Hypervisor,
    pub keyboard_polling: bool,
    pub use_ioapic: bool,
    pub use_lapic_timer: bool,
}

impl Platform {
    pub fn detect() -> Self {
        let hypervisor = detect_hypervisor();
        match hypervisor {
            Hypervisor::Vmware => Self {
                hypervisor,
                keyboard_polling: true,
                use_ioapic: false,
                use_lapic_timer: true,
            },
            Hypervisor::Qemu | Hypervisor::VirtualBox => Self {
                hypervisor,
                keyboard_polling: true,
                use_ioapic: true,
                use_lapic_timer: true,
            },
            _ => Self {
                hypervisor,
                keyboard_polling: true,
                use_ioapic: false,
                use_lapic_timer: true, // x86_64 always has LAPIC
            },
        }
    }
}

fn detect_hypervisor() -> Hypervisor {
    let leaf1 = cpuid(1, 0);
    if leaf1.ecx & (1 << 31) == 0 {
        return Hypervisor::None;
    }

    let leaf = cpuid(0x4000_0000, 0);
    let mut bytes = [0u8; 12];
    bytes[0..4].copy_from_slice(&leaf.ebx.to_le_bytes());
    bytes[4..8].copy_from_slice(&leaf.ecx.to_le_bytes());
    bytes[8..12].copy_from_slice(&leaf.edx.to_le_bytes());

    if bytes == *b"KVMKVMKVM\0\0\0" || bytes == *b"TCGTCGTCGTCG" {
        Hypervisor::Qemu
    } else if bytes == *b"VBoxVBoxVBox" {
        Hypervisor::VirtualBox
    } else if bytes == *b"VMwareVMware" {
        Hypervisor::Vmware
    } else if bytes == *b"Microsoft Hv" {
        Hypervisor::HyperV
    } else {
        Hypervisor::Unknown
    }
}

#[derive(Debug, Clone, Copy)]
struct CpuidResult {
    ebx: u32,
    ecx: u32,
    edx: u32,
}

fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    let result = core::arch::x86_64::__cpuid_count(leaf, subleaf);
    CpuidResult {
        ebx: result.ebx,
        ecx: result.ecx,
        edx: result.edx,
    }
}
