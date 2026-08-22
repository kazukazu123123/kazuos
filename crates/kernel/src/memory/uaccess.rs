use alloc::vec;
use alloc::vec::Vec;

/// Start of the user half. `USER_BASE` in `exec.rs` is PML4 entry 1; entry 0 is the
/// kernel identity map, which is present in every address space, so a user pointer
/// below this bound would resolve to kernel memory during a syscall.
const USER_MIN: u64 = 0x0000_0080_0000_0000;
/// End of the canonical lower half (PML4 entries 0..255).
const USER_MAX: u64 = 0x0000_8000_0000_0000;

/// Ceiling on a single user-supplied transfer length. The page walk below already
/// rejects unmapped ranges, so this only guards against a caller asking us to size a
/// kernel-side buffer from an absurd length before the walk runs.
pub const MAX_TRANSFER: u64 = 16 * 1024 * 1024;

/// True if every page of `[ptr, ptr + len)` is mapped in `cr3` and reachable from
/// ring 3 with the requested access. Zero-length ranges are accepted without a walk.
pub fn validate_range_in(cr3: u64, ptr: u64, len: u64, write: bool) -> bool {
    if len == 0 {
        return true;
    }
    if len > MAX_TRANSFER {
        return false;
    }
    let Some(end) = ptr.checked_add(len) else {
        return false;
    };
    if ptr < USER_MIN || end > USER_MAX {
        return false;
    }
    let first = ptr & !0xfff;
    let last = (end - 1) & !0xfff;
    let mut page = first;
    loop {
        if !unsafe { crate::vmm::user_accessible(cr3, page, write) } {
            return false;
        }
        if page == last {
            return true;
        }
        page += 4096;
    }
}

/// `validate_range_in` against the address space active on this CPU.
pub fn validate_range(ptr: u64, len: u64, write: bool) -> bool {
    validate_range_in(crate::vmm::active_cr3(), ptr, len, write)
}

/// Copy `len` bytes out of user space into a fresh kernel buffer.
pub fn read_bytes(ptr: u64, len: u64) -> Option<Vec<u8>> {
    if !validate_range(ptr, len, false) {
        return None;
    }
    let mut out = vec![0u8; len as usize];
    if len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(ptr as *const u8, out.as_mut_ptr(), len as usize);
        }
    }
    Some(out)
}

/// Copy a user string out and validate it as UTF-8.
pub fn read_str(ptr: u64, len: u64) -> Option<alloc::string::String> {
    let bytes = read_bytes(ptr, len)?;
    alloc::string::String::from_utf8(bytes).ok()
}

/// Copy `src` into user space at `ptr`.
pub fn write_bytes(ptr: u64, src: &[u8]) -> bool {
    if !validate_range(ptr, src.len() as u64, true) {
        return false;
    }
    if !src.is_empty() {
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), ptr as *mut u8, src.len());
        }
    }
    true
}

/// Copy `src` into user space at `ptr` in the address space `cr3`, which must be the
/// one currently loaded on this CPU. Used by the pipe wakeup path, which switches CR3
/// to a sleeping reader to deliver data.
pub fn write_bytes_in(cr3: u64, ptr: u64, src: &[u8]) -> bool {
    if !validate_range_in(cr3, ptr, src.len() as u64, true) {
        return false;
    }
    if !src.is_empty() {
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), ptr as *mut u8, src.len());
        }
    }
    true
}

/// Write a single `T` into user space. `T` must be a plain-data type that is safe to
/// expose to ring 3 — no padding holes carrying kernel bytes, no pointers.
pub fn write_value<T: Copy>(ptr: u64, value: T) -> bool {
    if !validate_range(ptr, core::mem::size_of::<T>() as u64, true) {
        return false;
    }
    unsafe {
        core::ptr::write_unaligned(ptr as *mut T, value);
    }
    true
}
