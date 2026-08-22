#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, _arg2: u64) -> u64 {
    match number {
    // Kernel modules
            // NOTE: module load/unload is deliberately open to PrivilegeLevel::User, which
            // means any process can gain Driver capabilities (ioport/DMA/IRQ/PCI BAR) by
            // loading a .kkm. That is a known gap, not an oversight: there is no privileged
            // session concept yet, so gating this at Driver would leave `/bin/modules` — the
            // only runtime module management there is — unable to do anything. Closing it
            // properly needs a System-privileged shell path first. Until then the gate below
            // is honest about being a no-op rather than looking like a real check.
            //
            // The previous form was `> PrivilegeLevel::User`, which can never be true (User
            // is the maximum) *and* returned `u64::MAX - 1` — the same value as
            // syscall::EXIT_TO_KERNEL, which the int-0x80 stub interprets as "abandon the
            // user frame and return to the kernel". Denials must use u64::MAX like every
            // other privileged syscall here.
            SYS_MODULE_LOAD => {
                if arg0 == 0 || arg1 == 0 { return u64::MAX; }
                match crate::memory::uaccess::read_str(arg0, arg1) {
                    Some(path) => crate::kmod::load(&path),
                    None => u64::MAX,
                }
            }
            SYS_MODULE_UNLOAD => {
                if crate::kmod::unload(arg0 as u32) { 0 } else { u64::MAX }
            }
            SYS_MODULE_LIST => crate::kmod::list(arg0, arg1),
            SYS_MODULE_INFO => crate::kmod::info(arg0 as u32, arg1),

                _ => u64::MAX,
    }
}
