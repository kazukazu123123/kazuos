use alloc::vec::Vec;
use crate::util::SyncUnsafeCell;

struct PipeState {
    data: Vec<u8>,
    write_refs: u32,
    read_refs: u32,
}

static PIPES: SyncUnsafeCell<Vec<Option<PipeState>>> = SyncUnsafeCell::new(Vec::new());

pub fn create() -> Option<u64> {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        for (i, slot) in pipes.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(PipeState { data: Vec::new(), write_refs: 0, read_refs: 0 });
                return Some(i as u64);
            }
        }
        let id = pipes.len() as u64;
        pipes.push(Some(PipeState { data: Vec::new(), write_refs: 0, read_refs: 0 }));
        Some(id)
    }
}

pub fn clone_write(id: u64) {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        if let Some(Some(pipe)) = pipes.get_mut(id as usize) {
            pipe.write_refs += 1;
        }
    }
}

pub fn clone_read(id: u64) {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        if let Some(Some(pipe)) = pipes.get_mut(id as usize) {
            pipe.read_refs += 1;
        }
    }
}

pub fn write(id: u64, data: &[u8]) -> usize {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        if let Some(Some(pipe)) = pipes.get_mut(id as usize) {
            if pipe.read_refs == 0 { return 0; }
            pipe.data.extend_from_slice(data);
            return data.len();
        }
        0
    }
}

pub fn read(id: u64, buf: &mut [u8]) -> usize {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        if let Some(Some(pipe)) = pipes.get_mut(id as usize) {
            let n = buf.len().min(pipe.data.len());
            buf[..n].copy_from_slice(&pipe.data[..n]);
            pipe.data.drain(..n);
            return n;
        }
        0
    }
}

/// Read directly into a raw user-space pointer. Used when waking a blocked reader.
pub fn read_raw(id: u64, buf_ptr: u64, max_len: usize) -> usize {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        if let Some(Some(pipe)) = pipes.get_mut(id as usize) {
            let n = max_len.min(pipe.data.len());
            core::ptr::copy_nonoverlapping(pipe.data.as_ptr(), buf_ptr as *mut u8, n);
            pipe.data.drain(..n);
            return n;
        }
        0
    }
}

pub fn is_empty(id: u64) -> bool {
    unsafe {
        let pipes = &*PIPES.0.get();
        pipes.get(id as usize).and_then(|s| s.as_ref()).map(|p| p.data.is_empty()).unwrap_or(true)
    }
}

pub fn writer_closed(id: u64) -> bool {
    unsafe {
        let pipes = &*PIPES.0.get();
        pipes.get(id as usize).and_then(|s| s.as_ref()).map(|p| p.write_refs == 0).unwrap_or(true)
    }
}

pub fn close_write(id: u64) {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        if let Some(Some(pipe)) = pipes.get_mut(id as usize) {
            pipe.write_refs = pipe.write_refs.saturating_sub(1);
            if pipe.write_refs == 0 && pipe.read_refs == 0 {
                pipes[id as usize] = None;
            }
        }
    }
}

pub fn close_read(id: u64) {
    unsafe {
        let pipes = &mut *PIPES.0.get();
        if let Some(Some(pipe)) = pipes.get_mut(id as usize) {
            pipe.read_refs = pipe.read_refs.saturating_sub(1);
            if pipe.write_refs == 0 && pipe.read_refs == 0 {
                pipes[id as usize] = None;
            }
        }
    }
}
