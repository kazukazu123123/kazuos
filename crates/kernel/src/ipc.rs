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

fn channels() -> &'static mut Vec<Channel> {
    unsafe { &mut *CHANNELS.0.get() }
}

/// Open or create a named channel. Returns channel id (1-based), or u64::MAX on error.
pub fn open(name: &[u8]) -> u64 {
    if name.is_empty() || name.len() > NAME_LEN {
        return u64::MAX;
    }
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
    let idx = channel_id as usize - 1;
    let ch = channels();
    if idx >= ch.len() {
        return SendResult::Error;
    }
    let c = &mut ch[idx];
    if c.queue.len() >= MAX_QUEUE {
        return SendResult::Block;
    }
    let _ = sender;
    c.queue.push_back(Message { data: data.to_vec() });

    // Wake the first RECV waiter if any.
    if !c.recv_waiters.is_empty() {
        let waiter_pid = c.recv_waiters.remove(0);
        crate::process::wakeup_ipc_waiter(waiter_pid, 0);
    }
    SendResult::Ok
}

pub enum RecvResult {
    /// Message written to buf; returns actual length.
    Ok(usize),
    /// No message yet; caller should block.
    Block,
    /// Bad channel id or buf too small.
    Error,
}

/// Try to dequeue a message into buf.
pub fn try_recv(channel_id: u64, buf: &mut [u8]) -> RecvResult {
    let idx = channel_id as usize - 1;
    let ch = channels();
    if idx >= ch.len() {
        return RecvResult::Error;
    }
    let c = &mut ch[idx];
    match c.queue.pop_front() {
        None => RecvResult::Block,
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
}

pub fn add_recv_waiter(channel_id: u64, pid: u64) {
    let idx = channel_id as usize - 1;
    let ch = channels();
    if idx < ch.len() {
        ch[idx].recv_waiters.push(pid);
    }
}

pub fn add_send_waiter(channel_id: u64, pid: u64) {
    let idx = channel_id as usize - 1;
    let ch = channels();
    if idx < ch.len() {
        ch[idx].send_waiters.push(pid);
    }
}

pub fn close(channel_id: u64) {
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
}
