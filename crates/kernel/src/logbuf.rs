//! Kernel log ring buffer: the log side of the console/log split.
//!
//! Kernel log records, kernel `println!` output, and driver output
//! (SYS_LOG_WRITE) are appended here and mirrored to serial. The
//! framebuffer console is reserved for the interactive session, so log
//! output can no longer interleave with the shell prompt. Userspace reads
//! the buffer back with SYS_LOG_READ (the shell's `dmesg` command).

use core::fmt;

use crate::util::SpinLock;

const CAP: usize = 64 * 1024;

static LOCK: SpinLock = SpinLock::new();
static mut BUF: [u8; CAP] = [0; CAP];
/// Total bytes ever pushed. The retained window is the last
/// min(TOTAL, CAP) bytes; logical offset 0 is the oldest retained byte.
static mut TOTAL: u64 = 0;

/// Append bytes to the ring, overwriting the oldest bytes when full.
pub fn push(bytes: &[u8]) {
    LOCK.lock();
    unsafe {
        let mut src = bytes;
        if src.len() > CAP {
            // Only the tail of an oversized write can be retained.
            TOTAL += (src.len() - CAP) as u64;
            src = &src[src.len() - CAP..];
        }
        while !src.is_empty() {
            let pos = (TOTAL as usize) % CAP;
            let n = (CAP - pos).min(src.len());
            core::ptr::copy_nonoverlapping(
                src.as_ptr(),
                core::ptr::addr_of_mut!(BUF).cast::<u8>().add(pos),
                n,
            );
            TOTAL += n as u64;
            src = &src[n..];
        }
    }
    LOCK.unlock();
}

/// Number of bytes currently retained.
pub fn len() -> usize {
    LOCK.lock();
    let total = unsafe { TOTAL };
    LOCK.unlock();
    total.min(CAP as u64) as usize
}

/// Copy bytes starting at logical `offset` (0 = oldest retained byte)
/// into `out`. Returns the number of bytes copied.
pub fn read(offset: usize, out: &mut [u8]) -> usize {
    LOCK.lock();
    let mut copied = 0usize;
    unsafe {
        let avail = TOTAL.min(CAP as u64) as usize;
        if offset < avail {
            let n = (avail - offset).min(out.len());
            let mut start = TOTAL - avail as u64 + offset as u64;
            while copied < n {
                let pos = (start as usize) % CAP;
                let chunk = (CAP - pos).min(n - copied);
                core::ptr::copy_nonoverlapping(
                    core::ptr::addr_of!(BUF).cast::<u8>().add(pos),
                    out.as_mut_ptr().add(copied),
                    chunk,
                );
                copied += chunk;
                start += chunk as u64;
            }
        }
    }
    LOCK.unlock();
    copied
}

/// Fixed-capacity formatting sink used to compose one log record on the
/// stack before pushing it, so records are pushed whole, never in
/// fragments. Overlong records are truncated.
pub struct LineBuf {
    buf: [u8; 512],
    len: usize,
}

impl LineBuf {
    pub fn new() -> Self {
        Self { buf: [0; 512], len: 0 }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let n = (self.buf.len() - self.len).min(bytes.len());
        self.buf[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
    }
}

impl fmt::Write for LineBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push_bytes(s.as_bytes());
        Ok(())
    }
}

/// Format one record into a LineBuf and push it whole.
pub fn push_fmt(args: fmt::Arguments) {
    use core::fmt::Write;
    let mut lb = LineBuf::new();
    let _ = lb.write_fmt(args);
    push(lb.as_bytes());
}
