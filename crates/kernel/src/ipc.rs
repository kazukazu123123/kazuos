use alloc::vec::Vec;
use alloc::collections::VecDeque;
use crate::util::SyncUnsafeCell;

const MAX_CHANNELS: usize = 32;
const MAX_MSG_SIZE: usize = 4096;
const MAX_QUEUE:    usize = 8;
const NAME_LEN:     usize = 32;

struct Message {
    data: Vec<u8>,
}

struct Channel {
    name: [u8; NAME_LEN],
    name_len: usize,
    queue: VecDeque<Message>,
    // PIDs blocked in RECV waiting for a message
    recv_waiters: Vec<u64>,
    // PIDs blocked in SEND waiting for queue space
    send_waiters: Vec<u64>,
    ref_count: usize,
}

static CHANNELS: SyncUnsafeCell<Vec<Channel>> = SyncUnsafeCell::new(Vec::new());

// All channel state is shared between CPUs: a hardware publisher (e.g. ps2mouse.kkm)
// sends from one CPU while a consumer receives on another. Every access goes through
// the thread lock so the queue and waiter lists are never mutated concurrently, and
// so the "queue empty? then register as a waiter" decision is atomic with the sender's
// "enqueue then wake a waiter" — otherwise a wakeup slips between the two and the
// receiver sleeps forever. The thread lock (not a private one) is reused because the
// wake path already takes it; sharing one lock keeps the ordering consistent and the
// reentrant guard makes the nested wake calls safe.
fn channels() -> &'static mut Vec<Channel> {
    unsafe { &mut *CHANNELS.0.get() }
}

fn with_lock<F: FnOnce() -> R, R>(f: F) -> R {
    crate::task::thread::with_threads_lock(f)
}

/// Open or create a named channel. Returns channel id (1-based), or u64::MAX on error.
pub fn open(name: &[u8]) -> u64 {
    if name.is_empty() || name.len() > NAME_LEN {
        return u64::MAX;
    }
    with_lock(|| {
        let ch = channels();

        // Return existing channel id if name matches.
        for (i, c) in ch.iter_mut().enumerate() {
            if c.name_len == name.len() && c.name[..c.name_len] == *name {
                c.ref_count += 1;
                return (i + 1) as u64;
            }
        }

        if ch.len() >= MAX_CHANNELS {
            return u64::MAX;
        }

        let mut n = [0u8; NAME_LEN];
        n[..name.len()].copy_from_slice(name);
        ch.push(Channel {
            name: n,
            name_len: name.len(),
            queue: VecDeque::new(),
            recv_waiters: Vec::new(),
            send_waiters: Vec::new(),
            ref_count: 1,
        });
        ch.len() as u64
    })
}

pub enum SendResult {
    /// Message enqueued; return 0 to caller.
    Ok,
    /// Queue full; caller should block and retry.
    Block,
    /// Bad channel id.
    Error,
}

/// Try to enqueue a message. If a RECV waiter exists, wake it immediately.
pub fn try_send(channel_id: u64, sender: u64, data: &[u8]) -> SendResult {
    if data.len() > MAX_MSG_SIZE {
        return SendResult::Error;
    }
    with_lock(|| {
        let idx = channel_id as usize - 1;
        let ch = channels();
        if idx >= ch.len() {
            return SendResult::Error;
        }
        let c = &mut ch[idx];
        // Drop the oldest message instead of blocking the sender when the queue is full.
        // A hardware event publisher (e.g. ps2mouse.kkm) must never block: if it did, it
        // would stop draining the shared PS/2 controller, which then backs up with mouse
        // data and wedges the keyboard too. Stale relative-movement events are safe to drop.
        while c.queue.len() >= MAX_QUEUE {
            c.queue.pop_front();
        }
        let _ = sender;
        c.queue.push_back(Message { data: data.to_vec() });

        // Wake the first RECV waiter if any.
        if !c.recv_waiters.is_empty() {
            let waiter_pid = c.recv_waiters.remove(0);
            crate::process::wakeup_ipc_waiter(waiter_pid, 0);
        }
        SendResult::Ok
    })
}

pub enum RecvResult {
    /// Message written to buf; returns actual length.
    Ok(usize),
    /// No message yet; the caller's pid was registered as a waiter — block it.
    Block,
    /// Bad channel id or buf too small.
    Error,
}

/// Try to dequeue a message into `buf`. When the queue is empty, `pid` is registered
/// as a receive waiter AND put to sleep — all atomically under the lock, with the empty
/// check. This closes both races: a concurrent `try_send` cannot enqueue-and-miss the
/// wake (it is serialized by the same lock), and the wake cannot land between "register
/// waiter" and "mark sleeping" (which would otherwise remove the waiter then have it
/// sleep forever). The caller just returns `BLOCK_TO_SCHEDULER` on `Block`.
pub fn try_recv(channel_id: u64, buf: &mut [u8], pid: u64) -> RecvResult {
    with_lock(|| {
        let idx = channel_id as usize - 1;
        let ch = channels();
        if idx >= ch.len() {
            return RecvResult::Error;
        }
        let c = &mut ch[idx];
        match c.queue.pop_front() {
            None => {
                if pid != 0 {
                    c.recv_waiters.push(pid);
                    crate::process::set_wait_target(pid, crate::process::WaitTarget::Ipc(channel_id));
                    crate::process::set_sleeping(pid);
                }
                RecvResult::Block
            }
            Some(msg) => {
                let len = msg.data.len().min(buf.len());
                buf[..len].copy_from_slice(&msg.data[..len]);

                // Wake the first SEND waiter if any.
                if !c.send_waiters.is_empty() {
                    let waiter_pid = c.send_waiters.remove(0);
                    crate::process::wakeup_ipc_waiter(waiter_pid, 0);
                }
                RecvResult::Ok(len)
            }
        }
    })
}

pub fn add_send_waiter(channel_id: u64, pid: u64) {
    with_lock(|| {
        let idx = channel_id as usize - 1;
        let ch = channels();
        if idx < ch.len() {
            ch[idx].send_waiters.push(pid);
        }
    })
}

pub fn close(channel_id: u64) {
    with_lock(|| {
        let idx = channel_id as usize - 1;
        let ch = channels();
        if idx >= ch.len() {
            return;
        }
        let c = &mut ch[idx];
        if c.ref_count > 1 {
            c.ref_count -= 1;
        } else if c.queue.is_empty() {
            ch.remove(idx);
        } else {
            // Messages still pending — keep channel alive so the receiver can open it later.
            c.ref_count = 0;
        }
    })
}
