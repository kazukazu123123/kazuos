use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

// Kernel heap allocator.
//
// - A shared global free-list (first-fit + coalescing) over the heap region is
//   the source of truth and serves any size, including large contiguous
//   allocations (e.g. DMA buffers).
// - On top of it, each CPU has its own lock-free cache of small fixed-size
//   blocks. The common case (small alloc/free) hits only the per-CPU cache with
//   interrupts disabled and never touches the global lock, so CPUs don't contend
//   with each other. Refills/flushes and large allocations use the global lock.

#[repr(C)]
struct FreeNode {
    size: usize,
    next: *mut FreeNode,
}

#[repr(C)]
struct AllocHeader {
    block_addr: usize,
    block_size: usize,
}

const NCLASS: usize = 6;
/// Cacheable block sizes (the size carved from the global pool, header+payload).
const CLASS_SIZES: [usize; NCLASS] = [64, 128, 256, 512, 1024, 2048];
/// Max blocks held per size class per CPU before flushing back to the global pool.
const CACHE_CAP: usize = 64;
const MAX_CPUS: usize = crate::smp::MAX_CPUS;

#[inline]
fn align_up(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

/// Smallest class index whose size >= need, if any.
#[inline]
fn class_for(need: usize) -> Option<usize> {
    CLASS_SIZES.iter().position(|&c| c >= need)
}

/// Class index whose size == size exactly, if any.
#[inline]
fn exact_class(size: usize) -> Option<usize> {
    CLASS_SIZES.iter().position(|&c| c == size)
}

// ---- Shared global free-list ----

struct Global {
    head: UnsafeCell<*mut FreeNode>,
    locked: AtomicBool,
}

impl Global {
    const fn new() -> Self {
        Global {
            head: UnsafeCell::new(null_mut()),
            locked: AtomicBool::new(false),
        }
    }
    fn lock(&self) {
        while self.locked.swap(true, Ordering::Acquire) {
            core::hint::spin_loop();
        }
    }
    fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    /// Insert [addr, addr+size) into the address-sorted list, coalescing. Lock held.
    unsafe fn insert_nolock(&self, addr: usize, size: usize) {
        if size < core::mem::size_of::<FreeNode>() {
            return;
        }
        unsafe {
            let head = self.head.get();
            let mut prev: *mut FreeNode = null_mut();
            let mut cur = *head;
            while !cur.is_null() && (cur as usize) < addr {
                prev = cur;
                cur = (*cur).next;
            }
            let node = addr as *mut FreeNode;
            (*node).size = size;
            (*node).next = cur;
            if prev.is_null() { *head = node; } else { (*prev).next = node; }
            if !cur.is_null() && addr + size == cur as usize {
                (*node).size += (*cur).size;
                (*node).next = (*cur).next;
            }
            if !prev.is_null() && (prev as usize) + (*prev).size == addr {
                (*prev).size += (*node).size;
                (*prev).next = (*node).next;
            }
        }
    }

    /// Carve a block of at least `need` bytes; returns (addr, kept_size).
    unsafe fn take_raw(&self, need: usize) -> Option<(usize, usize)> {
        self.lock();
        let res = unsafe {
            let head = self.head.get();
            let mut prev: *mut FreeNode = null_mut();
            let mut cur = *head;
            let mut res = None;
            while !cur.is_null() {
                if (*cur).size >= need {
                    let addr = cur as usize;
                    let total = (*cur).size;
                    let next = (*cur).next;
                    if prev.is_null() { *head = next; } else { (*prev).next = next; }
                    let rem = total - need;
                    let kept = if rem >= core::mem::size_of::<FreeNode>() {
                        self.insert_nolock(addr + need, rem);
                        need
                    } else {
                        total
                    };
                    res = Some((addr, kept));
                    break;
                }
                prev = cur;
                cur = (*cur).next;
            }
            res
        };
        self.unlock();
        res
    }

    unsafe fn give_raw(&self, addr: usize, size: usize) {
        self.lock();
        unsafe { self.insert_nolock(addr, size); }
        self.unlock();
    }
}

// ---- Per-CPU small-block cache (intrusive stack; owner CPU only) ----

struct Cache {
    heads: [UnsafeCell<*mut u8>; NCLASS],
    counts: [UnsafeCell<usize>; NCLASS],
}

impl Cache {
    const fn new() -> Self {
        Cache {
            heads: [const { UnsafeCell::new(null_mut()) }; NCLASS],
            counts: [const { UnsafeCell::new(0) }; NCLASS],
        }
    }
}

struct KernelAllocator {
    global: Global,
    caches: [Cache; MAX_CPUS],
}

unsafe impl Sync for KernelAllocator {}

#[global_allocator]
static ALLOCATOR: KernelAllocator = KernelAllocator {
    global: Global::new(),
    caches: [const { Cache::new() }; MAX_CPUS],
};

/// Initialise the heap with one big free block.
pub fn init(heap_start: *mut u8, heap_size: usize) {
    unsafe {
        ALLOCATOR.global.lock();
        ALLOCATOR.global.insert_nolock(heap_start as usize, heap_size);
        ALLOCATOR.global.unlock();
    }
}

impl KernelAllocator {
    #[inline]
    fn cpu(&self) -> usize {
        let i = crate::smp::current_cpu_index();
        if i < MAX_CPUS { i } else { 0 }
    }
}

unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let header = core::mem::size_of::<AllocHeader>();
        let align = layout
            .align()
            .max(core::mem::align_of::<AllocHeader>())
            .max(core::mem::align_of::<FreeNode>());
        let need = align_up(header + layout.size() + align, core::mem::align_of::<FreeNode>());

        let flags = crate::util::irq_save();
        let result = unsafe {
            // Small allocation: try this CPU's cache first.
            if let Some(ci) = class_for(need) {
                let cs = CLASS_SIZES[ci];
                let cache = &self.caches[self.cpu()];
                let head = cache.heads[ci].get();
                if !(*head).is_null() {
                    // Pop a cached block (exactly cs bytes).
                    let addr = *head as usize;
                    *head = *(addr as *const *mut u8);
                    *cache.counts[ci].get() -= 1;
                    finish_alloc(addr, cs, header, align)
                } else {
                    // Refill straight from the global pool.
                    match self.global.take_raw(cs) {
                        Some((addr, kept)) => finish_alloc(addr, kept, header, align),
                        None => null_mut(),
                    }
                }
            } else {
                // Large allocation: global pool directly.
                match self.global.take_raw(need) {
                    Some((addr, kept)) => finish_alloc(addr, kept, header, align),
                    None => null_mut(),
                }
            }
        };
        crate::util::restore_flags(flags);
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        let header = core::mem::size_of::<AllocHeader>();
        let (addr, size) = unsafe {
            let hdr = (ptr as usize - header) as *const AllocHeader;
            ((*hdr).block_addr, (*hdr).block_size)
        };
        let flags = crate::util::irq_save();
        unsafe {
            if let Some(ci) = exact_class(size) {
                let cache = &self.caches[self.cpu()];
                let count = cache.counts[ci].get();
                if *count < CACHE_CAP {
                    // Push onto this CPU's cache (store next pointer in the block).
                    let head = cache.heads[ci].get();
                    *(addr as *mut *mut u8) = *head;
                    *head = addr as *mut u8;
                    *count += 1;
                } else {
                    self.global.give_raw(addr, size);
                }
            } else {
                self.global.give_raw(addr, size);
            }
        }
        crate::util::restore_flags(flags);
    }
}

/// Write the allocation header just before the aligned user pointer.
#[inline]
unsafe fn finish_alloc(addr: usize, block_size: usize, header: usize, align: usize) -> *mut u8 {
    unsafe {
        let user = align_up(addr + header, align);
        let hdr = (user - header) as *mut AllocHeader;
        (*hdr).block_addr = addr;
        (*hdr).block_size = block_size;
        user as *mut u8
    }
}
