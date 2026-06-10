use crate::util::SyncUnsafeCell;

/// User-space virtual address where the framebuffer is mapped on acquire.
pub const USER_FB_VA: u64 = 0x0000_0082_0000_0000;

static OWNER: SyncUnsafeCell<Option<u64>> = SyncUnsafeCell::new(None);
static BACK_PTR: SyncUnsafeCell<*mut u8> = SyncUnsafeCell::new(core::ptr::null_mut());
static BACK_LEN: SyncUnsafeCell<usize> = SyncUnsafeCell::new(0);

/// Layout written into the user buffer by SYS_FB_ACQUIRE.
#[repr(C)]
pub struct FbInfo {
    pub base: u64, // user-space VA
    pub width: u32,
    pub height: u32,
    pub stride: u32, // pixels per row
    pub format: u32, // 0=RGB, 1=BGR
}

pub fn owner() -> Option<u64> {
    unsafe { *OWNER.0.get() }
}

/// Acquire the framebuffer for `pid`.
/// Maps the physical FB into the process's page table at USER_FB_VA,
/// saves the current pixels to a back buffer, and writes FbInfo to `out`.
/// Returns 0 on success, u64::MAX if already owned by a different process.
pub fn acquire(pid: u64, cr3: u64, out: *mut FbInfo) -> u64 {
    unsafe {
        match *OWNER.0.get() {
            Some(o) if o == pid => return write_info(out),
            Some(_) => return u64::MAX,
            None => {}
        }

        let p = match crate::console::fb_params() {
            Some(p) => p,
            None => return u64::MAX,
        };

        // Save current framebuffer pixels as back buffer.
        let fb_bytes = p.stride as usize * p.height as usize * 4;
        if fb_bytes > 0 {
            let src = core::slice::from_raw_parts(p.base as *const u8, fb_bytes);
            if let Ok(layout) = alloc::alloc::Layout::from_size_align(fb_bytes, 1) {
                let ptr = alloc::alloc::alloc(layout);
                if !ptr.is_null() {
                    core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, fb_bytes);
                    *BACK_PTR.0.get() = ptr;
                    *BACK_LEN.0.get() = fb_bytes;
                }
            }
        }

        // Map physical framebuffer pages into the user process at USER_FB_VA.
        let r = crate::vmm::map_range(
            cr3,
            USER_FB_VA,
            p.base,
            fb_bytes as u64,
            crate::vmm::MapFlags::USER_READ_WRITE,
        );
        if r.is_err() {
            release_backup();
            return u64::MAX;
        }

        // Clear framebuffer to black.
        core::ptr::write_bytes(p.base as *mut u8, 0, fb_bytes);

        *OWNER.0.get() = Some(pid);
        write_info(out)
    }
}

/// Release the framebuffer owned by `pid`, restoring the saved back buffer.
/// No-op if `pid` is not the current owner.
pub fn release(pid: u64) {
    unsafe {
        if *OWNER.0.get() != Some(pid) {
            return;
        }
        *OWNER.0.get() = None;

        if let Some(p) = crate::console::fb_params() {
            let fb_bytes = p.stride as usize * p.height as usize * 4;
            let back_ptr = *BACK_PTR.0.get();
            let back_len = *BACK_LEN.0.get();
            if !back_ptr.is_null() && back_len > 0 {
                let copy_len = fb_bytes.min(back_len);
                core::ptr::copy_nonoverlapping(back_ptr, p.base as *mut u8, copy_len);
            }
        }
        release_backup();
    }
}

unsafe fn release_backup() {
    unsafe {
        let ptr = *BACK_PTR.0.get();
        let len = *BACK_LEN.0.get();
        if !ptr.is_null() && len > 0 {
            if let Ok(layout) = alloc::alloc::Layout::from_size_align(len, 1) {
                alloc::alloc::dealloc(ptr, layout);
            }
        }
        *BACK_PTR.0.get() = core::ptr::null_mut();
        *BACK_LEN.0.get() = 0;
    }
}

unsafe fn write_info(out: *mut FbInfo) -> u64 {
    if out.is_null() {
        return u64::MAX;
    }
    unsafe {
        if let Some(p) = crate::console::fb_params() {
            out.write(FbInfo {
                base: USER_FB_VA,
                width: p.width,
                height: p.height,
                stride: p.stride,
                format: p.format,
            });
            0
        } else {
            u64::MAX
        }
    }
}
