use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;

struct BumpAllocator {
    heap_start: UnsafeCell<*mut u8>,
    heap_end: UnsafeCell<*mut u8>,
    next: UnsafeCell<*mut u8>,
}

unsafe impl Sync for BumpAllocator {}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator {
    heap_start: UnsafeCell::new(null_mut()),
    heap_end: UnsafeCell::new(null_mut()),
    next: UnsafeCell::new(null_mut()),
};

pub fn init(heap_start: *mut u8, heap_size: usize) {
    unsafe {
        *ALLOCATOR.heap_start.get() = heap_start;
        *ALLOCATOR.heap_end.get() = (heap_start as usize + heap_size) as *mut u8;
        *ALLOCATOR.next.get() = heap_start;
    }
}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let next = unsafe { *self.next.get() };
        let aligned = ((next as usize) + layout.align() - 1) & !(layout.align() - 1);
        let end = aligned + layout.size();
        if end > unsafe { *self.heap_end.get() } as usize {
            null_mut()
        } else {
            unsafe {
                *self.next.get() = end as *mut u8;
            }
            aligned as *mut u8
        }
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // bump allocator: no free
    }
}
