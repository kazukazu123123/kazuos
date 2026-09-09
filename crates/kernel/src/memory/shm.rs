use alloc::vec::Vec;

use crate::util::{IrqGuard, SpinLock, SyncUnsafeCell};
use crate::{process, vmm};

const PAGE_SIZE: u64 = 4096;
const USER_SHM_BASE: u64 = 0x0000_00B0_0000_0000;
const USER_SHM_LIMIT: u64 = 0x0000_00C0_0000_0000;
const MAX_SIZE: u64 = 128 * 1024 * 1024;

static LOCK: SpinLock = SpinLock::new();
static OBJECTS: SyncUnsafeCell<Option<Vec<Shm>>> = SyncUnsafeCell::new(None);
static NEXT_ID: SyncUnsafeCell<u64> = SyncUnsafeCell::new(1);

struct Shm {
    id: u64,
    owner: u64,
    frames: Vec<u64>,
    holders: Vec<u64>,
    mappings: Vec<Mapping>,
}

struct Mapping {
    pid: u64,
    virt: u64,
}

fn with_objects<T>(f: impl FnOnce(&mut Vec<Shm>) -> T) -> T {
    let _irq = IrqGuard::new();
    LOCK.lock();
    let result = unsafe {
        let objects = &mut *OBJECTS.0.get();
        if objects.is_none() {
            *objects = Some(Vec::new());
        }
        f(objects.as_mut().unwrap())
    };
    LOCK.unlock();
    result
}

fn current_context() -> Option<(u64, u64)> {
    let pid = crate::scheduler::current_user_pid()?;
    let cr3 = vmm::active_cr3();
    if cr3 == vmm::kernel_cr3() {
        return None;
    }
    Some((pid, cr3))
}

pub fn create(size: u64) -> u64 {
    let pid = match crate::scheduler::current_user_pid() {
        Some(pid) => pid,
        None => return u64::MAX,
    };
    if size == 0 || size > MAX_SIZE {
        return u64::MAX;
    }
    let Some(id) = with_objects(|_| unsafe {
        let next_id = &mut *NEXT_ID.0.get();
        let id = *next_id;
        *next_id = next_id.checked_add(1)?;
        Some(id)
    }) else {
        return u64::MAX;
    };
    let pages = size.div_ceil(PAGE_SIZE);
    let mut frames = Vec::new();
    for _ in 0..pages {
        let Some(frame) = crate::pmm::alloc_frame() else {
            for frame in frames {
                crate::pmm::free_frame(frame);
            }
            return u64::MAX;
        };
        unsafe {
            core::ptr::write_bytes(frame as *mut u8, 0, PAGE_SIZE as usize);
        }
        frames.push(frame);
    }

    with_objects(|objects| {
        objects.push(Shm {
            id,
            owner: pid,
            frames,
            holders: alloc::vec![pid],
            mappings: Vec::new(),
        });
    });
    id
}

pub fn grant(id: u64, target_pid: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| {
        let Some(caller) = crate::scheduler::current_user_pid() else {
            return u64::MAX;
        };
        if target_pid == 0 || !process::is_live(target_pid) {
            return u64::MAX;
        }
        with_objects(|objects| {
            let Some(shm) = objects
                .iter_mut()
                .find(|shm| shm.id == id && shm.owner == caller && shm.holders.contains(&caller))
            else {
                return u64::MAX;
            };
            if !shm.holders.contains(&target_pid) {
                shm.holders.push(target_pid);
            }
            0
        })
    })
}

pub fn map(id: u64) -> u64 {
    let Some((pid, cr3)) = current_context() else {
        return u64::MAX;
    };
    let result = with_objects(|objects| {
        let Some(index) = objects
            .iter()
            .position(|shm| shm.id == id && shm.holders.contains(&pid))
        else {
            return None;
        };
        if let Some(mapping) = objects[index]
            .mappings
            .iter()
            .find(|mapping| mapping.pid == pid)
        {
            return Some((mapping.virt, 0));
        }
        let size = objects[index].frames.len() as u64 * PAGE_SIZE;
        let virt = find_free_range(objects, pid, cr3, size)?;
        let frames = &objects[index].frames;
        for (page, &frame) in frames.iter().enumerate() {
            let addr = virt + page as u64 * PAGE_SIZE;
            if unsafe {
                vmm::map_page(
                    cr3,
                    addr,
                    frame,
                    vmm::MapFlags::USER_READ_WRITE.no_execute(),
                )
            }
            .is_err()
            {
                unsafe {
                    vmm::unmap_range(cr3, virt, page as u64 * PAGE_SIZE);
                    vmm::flush_tlb(cr3);
                }
                return None;
            }
        }
        objects[index].mappings.push(Mapping { pid, virt });
        Some((virt, size))
    });
    match result {
        Some((virt, added)) => {
            if added != 0 {
                process::add_memory_bytes(pid, added);
            }
            virt
        }
        None => u64::MAX,
    }
}

fn find_free_range(objects: &[Shm], pid: u64, cr3: u64, size: u64) -> Option<u64> {
    let mut candidate = USER_SHM_BASE;
    loop {
        let end = candidate.checked_add(size)?;
        if end > USER_SHM_LIMIT {
            return None;
        }
        let mut next = None;
        for shm in objects {
            let mapped_size = shm.frames.len() as u64 * PAGE_SIZE;
            for mapping in &shm.mappings {
                if mapping.pid != pid {
                    continue;
                }
                let mapped_end = mapping.virt.checked_add(mapped_size)?;
                if candidate < mapped_end && mapping.virt < end {
                    next = Some(next.map_or(mapped_end, |value: u64| value.max(mapped_end)));
                }
            }
        }
        if let Some(value) = next {
            candidate = value;
            continue;
        }
        for offset in (0..size).step_by(PAGE_SIZE as usize) {
            if unsafe { vmm::translate(cr3, candidate + offset) }.is_some() {
                candidate = candidate.checked_add(PAGE_SIZE)?;
                next = Some(candidate);
                break;
            }
        }
        if next.is_none() {
            return Some(candidate);
        }
    }
}

pub fn unmap(id: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| {
        let Some((pid, cr3)) = current_context() else {
            return u64::MAX;
        };
        if process::live_thread_count(pid) != 1 {
            return u64::MAX;
        }
        let removed = with_objects(|objects| unmap_holder(objects, id, pid, cr3));
        match removed {
            Some(size) => {
                process::sub_memory_bytes(pid, size);
                0
            }
            None => u64::MAX,
        }
    })
}

fn unmap_holder(objects: &mut [Shm], id: u64, pid: u64, cr3: u64) -> Option<u64> {
    let shm = objects.iter_mut().find(|shm| shm.id == id)?;
    let mapping_index = shm.mappings.iter().position(|mapping| mapping.pid == pid)?;
    let mapping = shm.mappings.swap_remove(mapping_index);
    let size = shm.frames.len() as u64 * PAGE_SIZE;
    unsafe {
        vmm::unmap_range(cr3, mapping.virt, size);
        vmm::flush_tlb(cr3);
    }
    Some(size)
}

pub fn close(id: u64) -> u64 {
    crate::task::thread::with_threads_lock(|| {
        let Some((pid, cr3)) = current_context() else {
            return u64::MAX;
        };
        if process::live_thread_count(pid) != 1 {
            return u64::MAX;
        }
        let result = with_objects(|objects| {
            let index = objects
                .iter()
                .position(|shm| shm.id == id && shm.holders.contains(&pid))?;
            let unmapped = unmap_holder(objects, id, pid, cr3).unwrap_or(0);
            let shm = &mut objects[index];
            let holder_index = shm.holders.iter().position(|&holder| holder == pid)?;
            shm.holders.swap_remove(holder_index);
            let frames = if shm.holders.is_empty() {
                Some(objects.swap_remove(index).frames)
            } else {
                None
            };
            Some((unmapped, frames))
        });
        let Some((unmapped, frames)) = result else {
            return u64::MAX;
        };
        if unmapped != 0 {
            process::sub_memory_bytes(pid, unmapped);
        }
        if let Some(frames) = frames {
            for frame in frames {
                crate::pmm::free_frame(frame);
            }
        }
        0
    })
}

pub fn cleanup_pid(pid: u64, cr3: u64) {
    let (unmapped, frames) = with_objects(|objects| {
        let mut unmapped = 0;
        let mut frames = Vec::new();
        let mut index = 0;
        while index < objects.len() {
            let id = objects[index].id;
            let mapping_size = unmap_holder(objects, id, pid, cr3).unwrap_or(0);
            unmapped += mapping_size;
            objects[index].holders.retain(|&holder| holder != pid);
            if objects[index].holders.is_empty() {
                frames.extend(objects.swap_remove(index).frames);
            } else {
                index += 1;
            }
        }
        (unmapped, frames)
    });
    if unmapped != 0 {
        process::sub_memory_bytes(pid, unmapped);
    }
    for frame in frames {
        crate::pmm::free_frame(frame);
    }
}
