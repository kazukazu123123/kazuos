use crate::util::SyncUnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};
use kazuos_shared::MemoryMapEntry;

pub struct Pmm {
    bitmap: *mut u8,
    _bitmap_size: usize,
    total_frames: usize,
    usable_frames: usize,
    next_frame: AtomicUsize,
}

#[derive(Debug, Clone, Copy)]
pub struct PmmStats {
    pub total_frames: usize,
    pub used_frames: usize,
    pub free_frames: usize,
}

impl PmmStats {
    pub fn total_kib(&self) -> usize {
        self.total_frames * 4
    }

    pub fn used_kib(&self) -> usize {
        self.used_frames * 4
    }

    pub fn free_kib(&self) -> usize {
        self.free_frames * 4
    }
}

static PMM: SyncUnsafeCell<Option<Pmm>> = SyncUnsafeCell::new(None);

const FRAME_SIZE: u64 = 4096;
const TY_CONVENTIONAL: u32 = 7;

pub(crate) unsafe fn init(bitmap: *mut u8, bitmap_size: usize, memory_map: &[MemoryMapEntry]) {
    unsafe {
        core::ptr::write_bytes(bitmap, 0xff, bitmap_size);
    }

    let total_frames = bitmap_size * 8;
    let mut usable_frames = 0;

    unsafe {
        for entry in memory_map {
            if entry.ty == TY_CONVENTIONAL {
                usable_frames += mark_region(
                    bitmap,
                    total_frames,
                    entry.phys_start,
                    entry.page_count * FRAME_SIZE,
                    false,
                );
            }
        }

        let bitmap_phys = bitmap as u64;
        let bitmap_pages = (bitmap_size as u64).div_ceil(FRAME_SIZE);
        mark_region(
            bitmap,
            total_frames,
            bitmap_phys,
            bitmap_pages * FRAME_SIZE,
            true,
        );
    }

    unsafe {
        *PMM.0.get() = Some(Pmm {
            bitmap,
            _bitmap_size: bitmap_size,
            total_frames,
            usable_frames,
            next_frame: AtomicUsize::new(0),
        });
    }
}

unsafe fn mark_region(
    bitmap: *mut u8,
    total_frames: usize,
    phys_start: u64,
    size: u64,
    used: bool,
) -> usize {
    let start_frame = (phys_start / FRAME_SIZE) as usize;
    let end_frame = (phys_start + size).div_ceil(FRAME_SIZE) as usize;
    let end_frame = end_frame.min(total_frames);
    let mut changed = 0;
    for frame in start_frame..end_frame {
        unsafe {
            if get_bit(bitmap, frame) != used {
                changed += 1;
            }
            set_bit(bitmap, frame, used);
        }
    }
    changed
}

unsafe fn set_bit(bitmap: *mut u8, index: usize, value: bool) {
    unsafe {
        let byte = bitmap.add(index / 8);
        let bit = (index % 8) as u8;
        if value {
            byte.write_volatile(byte.read_volatile() | (1 << bit));
        } else {
            byte.write_volatile(byte.read_volatile() & !(1 << bit));
        }
    }
}

unsafe fn get_bit(bitmap: *mut u8, index: usize) -> bool {
    unsafe {
        let byte = bitmap.add(index / 8);
        let bit = (index % 8) as u8;
        (byte.read_volatile() >> bit) & 1 != 0
    }
}

pub fn alloc_frame_below(limit: u64) -> Option<u64> {
    unsafe {
        let pmm = (*PMM.0.get()).as_ref()?;
        let limit_frame = (limit / FRAME_SIZE) as usize;
        for frame in 0..limit_frame.min(pmm.total_frames) {
            if !get_bit(pmm.bitmap, frame) {
                set_bit(pmm.bitmap, frame, true);
                return Some(frame as u64 * FRAME_SIZE);
            }
        }
        None
    }
}

pub fn alloc_frame() -> Option<u64> {
    unsafe {
        let pmm = (*PMM.0.get()).as_ref()?;

        // Simple linear scan with next-frame hint
        let start = pmm.next_frame.load(Ordering::Relaxed);
        for i in 0..pmm.total_frames {
            let frame = (start + i) % pmm.total_frames;
            if !get_bit(pmm.bitmap, frame) {
                set_bit(pmm.bitmap, frame, true);
                pmm.next_frame
                    .store((frame + 1) % pmm.total_frames, Ordering::Relaxed);
                return Some(frame as u64 * FRAME_SIZE);
            }
        }
        None
    }
}

pub fn free_frame(addr: u64) {
    unsafe {
        if let Some(pmm) = (*PMM.0.get()).as_ref() {
            let frame = (addr / FRAME_SIZE) as usize;
            if frame < pmm.total_frames {
                set_bit(pmm.bitmap, frame, false);
            }
        }
    }
}

/// Allocate contiguous frames
pub fn alloc_frames(count: usize) -> Option<u64> {
    unsafe {
        let pmm = (*PMM.0.get()).as_ref()?;
        'outer: for start in 0..pmm.total_frames.saturating_sub(count) {
            for i in 0..count {
                if get_bit(pmm.bitmap, start + i) {
                    continue 'outer;
                }
            }
            // Found contiguous block
            for i in 0..count {
                set_bit(pmm.bitmap, start + i, true);
            }
            pmm.next_frame
                .store((start + count) % pmm.total_frames, Ordering::Relaxed);
            return Some(start as u64 * FRAME_SIZE);
        }
        None
    }
}

pub fn free_frames(addr: u64, count: usize) {
    for i in 0..count {
        free_frame(addr + i as u64 * FRAME_SIZE);
    }
}

/// Mark a physical region as used (for reserving bootloader/kernel memory)
pub fn mark_used(phys_start: u64, size: u64) {
    unsafe {
        let pmm = (*PMM.0.get()).as_ref().unwrap();
        mark_region(pmm.bitmap, pmm.total_frames, phys_start, size, true);
    }
}

pub fn stats() -> Option<PmmStats> {
    unsafe {
        let pmm = (*PMM.0.get()).as_ref()?;
        let mut free_frames = 0;
        for frame in 0..pmm.total_frames {
            if !get_bit(pmm.bitmap, frame) {
                free_frames += 1;
            }
        }
        // Use usable_frames (conventional RAM only) as total so MMIO/reserved
        // regions don't inflate the "used" number.
        let total = pmm.usable_frames;
        let used_frames = total.saturating_sub(free_frames);
        Some(PmmStats {
            total_frames: total,
            used_frames,
            free_frames,
        })
    }
}
