use crate::util::SyncUnsafeCell;

pub use kazuos_shared::{BootInfo, FramebufferInfo, MemoryMapEntry};

static BOOT_INFO: SyncUnsafeCell<Option<&'static BootInfo>> = SyncUnsafeCell::new(None);

pub fn init(boot_info: &'static BootInfo) {
    unsafe {
        *BOOT_INFO.0.get() = Some(boot_info);
    }
}

pub fn boot_info() -> &'static BootInfo {
    unsafe { (*BOOT_INFO.0.get()).unwrap() }
}
