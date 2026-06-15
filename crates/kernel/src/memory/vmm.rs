use crate::util::SyncUnsafeCell;
use alloc::alloc::{Layout, alloc_zeroed};

const PAGE_SIZE: u64 = 4096;
const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PWT: u64 = 1 << 3;
const PCD: u64 = 1 << 4;
const NO_EXECUTE: u64 = 1 << 63;
const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

static CURRENT_PML4: SyncUnsafeCell<u64> = SyncUnsafeCell::new(0);
static KERNEL_PML4: SyncUnsafeCell<u64> = SyncUnsafeCell::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    OutOfMemory,
    InvalidAddress,
}

#[derive(Clone, Copy)]
pub struct MapFlags(u64);

impl MapFlags {
    pub const READ: Self = Self(0);
    pub const READ_WRITE: Self = Self(WRITABLE);
    pub const USER_READ: Self = Self(USER);
    pub const USER_READ_WRITE: Self = Self(USER | WRITABLE);
    pub const USER_MMIO: Self = Self(USER | WRITABLE | PWT | PCD);

    pub const fn no_execute(self) -> Self {
        Self(self.0 | NO_EXECUTE)
    }
}

unsafe fn read_cr3() -> u64 {
    unsafe {
        let val: u64;
        core::arch::asm!("mov {}, cr3", out(reg) val, options(nomem, nostack));
        val
    }
}

pub(crate) unsafe fn init() {
    unsafe {
        // Enable NX (no-execute) support via EFER.NXE so that bit 63 of PTEs
        // is interpreted as the NX flag rather than causing reserved-bit faults.
        const EFER_MSR: u32 = 0xC000_0080;
        const NXE_BIT: u64 = 1 << 11;
        let low: u32;
        let high: u32;
        core::arch::asm!(
            "rdmsr",
            in("ecx") EFER_MSR,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack),
        );
        let mut efer = ((high as u64) << 32) | (low as u64);
        efer |= NXE_BIT;
        let new_low = efer as u32;
        let new_high = (efer >> 32) as u32;
        core::arch::asm!(
            "wrmsr",
            in("ecx") EFER_MSR,
            in("eax") new_low,
            in("edx") new_high,
            options(nomem, nostack),
        );

        let cr3 = read_cr3() & ADDR_MASK;
        *CURRENT_PML4.0.get() = cr3;
        *KERNEL_PML4.0.get() = cr3;
    }
}

pub(crate) unsafe fn switch_cr3(cr3: u64) {
    unsafe {
        let cr3 = cr3 & ADDR_MASK;
        *CURRENT_PML4.0.get() = cr3;
        core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nostack, preserves_flags));
    }
}

pub fn kernel_cr3() -> u64 {
    unsafe { *KERNEL_PML4.0.get() }
}

pub fn current_cr3() -> u64 {
    unsafe { *CURRENT_PML4.0.get() }
}

pub unsafe fn create_address_space() -> Result<u64, MapError> {
    unsafe {
        let pml4 = alloc_table()?;
        let kernel = kernel_cr3() as *const u64;
        let dst = pml4 as *mut u64;
        // Copy kernel lower-half entry 0 (identity-mapped code, stack, heap, framebuffer)
        // without USER flag so it remains supervisor-only.
        let entry0 = kernel.add(0).read_volatile();
        dst.add(0).write_volatile(entry0);
        // Leave entries 1..255 empty for user space fresh allocations.
        for i in 1..256usize {
            dst.add(i).write_volatile(0);
        }
        // Copy kernel upper-half entries 256..511 as-is.
        for i in 256..512usize {
            let entry = kernel.add(i).read_volatile();
            dst.add(i).write_volatile(entry);
        }
        Ok(pml4)
    }
}

pub unsafe fn map_page(cr3: u64, virt: u64, phys: u64, flags: MapFlags) -> Result<(), MapError> {
    if virt & (PAGE_SIZE - 1) != 0 || phys & (PAGE_SIZE - 1) != 0 {
        return Err(MapError::InvalidAddress);
    }
    unsafe {
        let pml4 = (cr3 & ADDR_MASK) as *mut u64;
        let pml4_i = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
        let pd_i = ((virt >> 21) & 0x1ff) as usize;
        let pt_i = ((virt >> 12) & 0x1ff) as usize;

        let pdpt = next_table(pml4, pml4_i, flags)?;
        let pd = next_table(pdpt, pdpt_i, flags)?;
        let pt = next_table(pd, pd_i, flags)?;
        pt.add(pt_i)
            .write_volatile((phys & ADDR_MASK) | PRESENT | flags.0);
        Ok(())
    }
}

pub unsafe fn map_range(
    cr3: u64,
    virt: u64,
    phys: u64,
    size: u64,
    flags: MapFlags,
) -> Result<(), MapError> {
    if size == 0 {
        return Ok(());
    }
    let pages = size.div_ceil(PAGE_SIZE);
    for page in 0..pages {
        unsafe {
            map_page(cr3, virt + page * PAGE_SIZE, phys + page * PAGE_SIZE, flags)?;
        }
    }
    Ok(())
}

pub unsafe fn unmap_page(cr3: u64, virt: u64) {
    unsafe {
        let pml4 = (cr3 & ADDR_MASK) as *mut u64;
        let pml4_i = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
        let pd_i = ((virt >> 21) & 0x1ff) as usize;
        let pt_i = ((virt >> 12) & 0x1ff) as usize;

        let pdpt = table_if_present(pml4, pml4_i);
        if pdpt.is_null() { return; }
        let pd = table_if_present(pdpt, pdpt_i);
        if pd.is_null() { return; }
        let pt = table_if_present(pd, pd_i);
        if pt.is_null() { return; }
        pt.add(pt_i).write_volatile(0);
    }
}

pub unsafe fn unmap_range(cr3: u64, virt: u64, size: u64) {
    if size == 0 { return; }
    let pages = size.div_ceil(PAGE_SIZE);
    for page in 0..pages {
        unsafe {
            unmap_page(cr3, virt + page * PAGE_SIZE);
        }
    }
}

/// Return the physical address mapped at `virt` in `cr3`, if any (4 KiB page).
pub unsafe fn translate(cr3: u64, virt: u64) -> Option<u64> {
    unsafe {
        let pml4 = (cr3 & ADDR_MASK) as *mut u64;
        let pml4_i = ((virt >> 39) & 0x1ff) as usize;
        let pdpt_i = ((virt >> 30) & 0x1ff) as usize;
        let pd_i = ((virt >> 21) & 0x1ff) as usize;
        let pt_i = ((virt >> 12) & 0x1ff) as usize;
        let pdpt = table_if_present(pml4, pml4_i);
        if pdpt.is_null() { return None; }
        let pd = table_if_present(pdpt, pdpt_i);
        if pd.is_null() { return None; }
        let pt = table_if_present(pd, pd_i);
        if pt.is_null() { return None; }
        let entry = pt.add(pt_i).read_volatile();
        if entry & PRESENT != 0 {
            Some(entry & ADDR_MASK)
        } else {
            None
        }
    }
}

unsafe fn table_if_present(parent: *mut u64, index: usize) -> *mut u64 {
    unsafe {
        let entry = parent.add(index).read_volatile();
        if entry & PRESENT != 0 {
            (entry & ADDR_MASK) as *mut u64
        } else {
            core::ptr::null_mut()
        }
    }
}

unsafe fn next_table(
    parent: *mut u64,
    index: usize,
    flags: MapFlags,
) -> Result<*mut u64, MapError> {
    unsafe {
        let entry = parent.add(index).read_volatile();
        if entry & PRESENT != 0 {
            return Ok((entry & ADDR_MASK) as *mut u64);
        }
        let table = alloc_table()?;
        let value = (table & ADDR_MASK) | PRESENT | WRITABLE | (flags.0 & USER);
        parent.add(index).write_volatile(value);
        Ok(table as *mut u64)
    }
}

unsafe fn alloc_table() -> Result<u64, MapError> {
    unsafe {
        let layout = Layout::from_size_align(PAGE_SIZE as usize, PAGE_SIZE as usize)
            .map_err(|_| MapError::OutOfMemory)?;
        let ptr = alloc_zeroed(layout);
        if ptr.is_null() {
            return Err(MapError::OutOfMemory);
        }
        Ok(ptr as u64)
    }
}

/// Unmap the user portion (PML4 entries 1..255) of an address space.
/// Kernel-shared entries 0 and 256..511 are left untouched, and the physical
/// pages mapped by PTEs are not freed here; callers must release heap/DMA/PCI-MMIO
/// pages separately.
///
/// Note: page tables are allocated by map_page from the kernel heap (bump allocator),
/// so the table frames themselves cannot be reclaimed. We only clear the user PML4
/// entries; calling pmm::free_frame here would corrupt the PMM bitmap because those
/// frames were never allocated from the PMM.
pub unsafe fn free_user_address_space(cr3: u64) {
    unsafe {
        let pml4_phys = cr3 & ADDR_MASK;
        if pml4_phys == 0 || pml4_phys == (kernel_cr3() & ADDR_MASK) {
            return;
        }
        let pml4 = pml4_phys as *mut u64;
        // Entries 1..255 are user-space mappings allocated by map_page.
        for pml4_i in 1..256usize {
            pml4.add(pml4_i).write_volatile(0);
        }
        // The PML4 itself is heap-allocated; do not free to PMM.
    }
}
