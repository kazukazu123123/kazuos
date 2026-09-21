use alloc::vec::Vec;
use alloc::collections::VecDeque;
use crate::util::SyncUnsafeCell;

const MAX_CHANNELS: usize = 32;
const MAX_MSG_SIZE: usize = 8192;
const MAX_QUEUE:    usize = 8;
const MAX_DIRECTED_PER_ROUTE: usize = 8;
const MAX_DIRECTED_PER_TARGET: usize = 64;
const MAX_DIRECTED_PER_CHANNEL: usize = 256;
const MAX_DIRECTED_BYTES_PER_CHANNEL: usize = 1024 * 1024;
const NAME_LEN:     usize = 32;

struct Message {
    data: Vec<u8>,
}

struct DirectedMessage {
    sender: u64,
    target: u64,
    data: Vec<u8>,
}

struct Channel {
    name: [u8; NAME_LEN],
    name_len: usize,
    queue: VecDeque<Message>,
    directed_queue: VecDeque<DirectedMessage>,
    // PIDs blocked in RECV waiting for a message
    recv_waiters: Vec<u64>,
    // PIDs blocked in SEND waiting for queue space
    send_waiters: Vec<u64>,
    holders: Vec<(u64, usize)>,
    ref_count: usize,
}

/// Slots are stable: a channel id is `index + 1` for the life of the table, so a
/// closed channel leaves a `None` hole rather than shifting its successors down.
/// Compacting the vector would silently repoint every id above the removed one at a
/// different channel — a process holding id 2 would start talking to what used to be
/// channel 3.
static CHANNELS: SyncUnsafeCell<Vec<Option<Channel>>> = SyncUnsafeCell::new(Vec::new());

// All channel state is shared between CPUs: a hardware publisher (e.g. ps2mouse.kkm)
// sends from one CPU while a consumer receives on another. Every access goes through
// the thread lock so the queue and waiter lists are never mutated concurrently, and
// so the "queue empty? then register as a waiter" decision is atomic with the sender's
// "enqueue then wake a waiter" — otherwise a wakeup slips between the two and the
// receiver sleeps forever. The thread lock (not a private one) is reused because the
// wake path already takes it; sharing one lock keeps the ordering consistent and the
// reentrant guard makes the nested wake calls safe.
fn channels() -> &'static mut Vec<Option<Channel>> {
    unsafe { &mut *CHANNELS.0.get() }
}

fn with_lock<F: FnOnce() -> R, R>(f: F) -> R {
    crate::task::thread::with_threads_lock(f)
}

/// Resolve a 1-based channel id to its slot, or `None` if the id is out of range or
/// refers to a closed channel.
fn slot(ch: &mut Vec<Option<Channel>>, channel_id: u64) -> Option<&mut Channel> {
    let idx = (channel_id as usize).checked_sub(1)?;
    ch.get_mut(idx)?.as_mut()
}

/// Open or create a named channel. Returns channel id (1-based), or u64::MAX on error.
pub fn open(name: &[u8], pid: u64) -> u64 {
    if name.is_empty() || name.len() > NAME_LEN {
        return u64::MAX;
    }
    with_lock(|| {
        let ch = channels();

        // Return existing channel id if name matches.
        for (i, slot) in ch.iter_mut().enumerate() {
            if let Some(c) = slot {
                if c.name_len == name.len() && c.name[..c.name_len] == *name {
                    c.ref_count += 1;
                    if let Some(holder) = c.holders.iter_mut().find(|holder| holder.0 == pid) {
                        holder.1 += 1;
                    } else {
                        c.holders.push((pid, 1));
                    }
                    return (i + 1) as u64;
                }
            }
        }

        let mut n = [0u8; NAME_LEN];
        n[..name.len()].copy_from_slice(name);
        let channel = Channel {
            name: n,
            name_len: name.len(),
            queue: VecDeque::new(),
            directed_queue: VecDeque::new(),
            recv_waiters: Vec::new(),
            send_waiters: Vec::new(),
            holders: alloc::vec![(pid, 1)],
            ref_count: 1,
        };

        // Reuse a freed slot before growing, so a create/close cycle does not walk the
        // table off the MAX_CHANNELS ceiling.
        if let Some(i) = ch.iter().position(|s| s.is_none()) {
            ch[i] = Some(channel);
            return (i + 1) as u64;
        }
        if ch.len() >= MAX_CHANNELS {
            return u64::MAX;
        }
        ch.push(Some(channel));
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
pub fn try_send_reliable(channel_id: u64, sender: u64, data: &[u8]) -> SendResult {
    if data.len() > MAX_MSG_SIZE {
        return SendResult::Error;
    }
    with_lock(|| {
        let ch = channels();
        let Some(c) = slot(ch, channel_id) else {
            return SendResult::Error;
        };
        if c.queue.len() >= MAX_QUEUE {
            return SendResult::Block;
        }
        let _ = sender;
        c.queue.push_back(Message { data: data.to_vec() });
        if !c.recv_waiters.is_empty() {
            let waiter_pid = c.recv_waiters.remove(0);
            crate::process::wakeup_ipc_waiter(waiter_pid, 0);
        }
        SendResult::Ok
    })
}

pub fn wait_send_space(channel_id: u64, pid: u64) -> bool {
    with_lock(|| {
        let ch = channels();
        let Some(c) = slot(ch, channel_id) else { return false; };
        if c.queue.len() < MAX_QUEUE { return false; }
        if pid != 0 && !c.send_waiters.contains(&pid) {
            c.send_waiters.push(pid);
            crate::process::set_wait_target(pid, crate::process::WaitTarget::Ipc(channel_id));
            crate::process::set_sleeping(pid);
        }
        true
    })
}

pub fn try_send(channel_id: u64, sender: u64, data: &[u8]) -> SendResult {
    if data.len() > MAX_MSG_SIZE {
        return SendResult::Error;
    }
    with_lock(|| {
        let ch = channels();
        let Some(c) = slot(ch, channel_id) else {
            return SendResult::Error;
        };
        if !c.holders.iter().any(|holder| holder.0 == sender) {
            return SendResult::Error;
        }
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

pub enum DirectedSendResult {
    Ok(usize),
    WouldBlock,
    Error,
}

pub fn try_send_to(channel_id: u64, sender: u64, target: u64, data: &[u8]) -> DirectedSendResult {
    if sender == 0 || target == 0 || data.is_empty() || data.len() > MAX_MSG_SIZE
        || crate::process::info(target).is_none() {
        return DirectedSendResult::Error;
    }
    with_lock(|| {
        let Some(c) = slot(channels(), channel_id) else {
            return DirectedSendResult::Error;
        };
        if !c.holders.iter().any(|holder| holder.0 == sender)
            || !c.holders.iter().any(|holder| holder.0 == target) {
            return DirectedSendResult::Error;
        }
        let queued_bytes = c.directed_queue.iter().map(|message| message.data.len()).sum::<usize>();
        if c.directed_queue.len() >= MAX_DIRECTED_PER_CHANNEL
            || queued_bytes > MAX_DIRECTED_BYTES_PER_CHANNEL.saturating_sub(data.len())
            || c.directed_queue.iter().filter(|message| message.target == target).count()
                >= MAX_DIRECTED_PER_TARGET
            || c.directed_queue.iter().filter(|message| message.sender == sender && message.target == target).count()
                >= MAX_DIRECTED_PER_ROUTE {
            return DirectedSendResult::WouldBlock;
        }
        c.directed_queue.push_back(DirectedMessage { sender, target, data: data.to_vec() });
        DirectedSendResult::Ok(data.len())
    })
}

pub enum DirectedRecvResult {
    Ok { sender: u64, len: usize },
    WouldBlock,
    Error,
}

pub fn try_recv_from(channel_id: u64, target: u64, buf: &mut [u8]) -> DirectedRecvResult {
    if target == 0 {
        return DirectedRecvResult::Error;
    }
    with_lock(|| {
        let Some(c) = slot(channels(), channel_id) else {
            return DirectedRecvResult::Error;
        };
        if !c.holders.iter().any(|holder| holder.0 == target) {
            return DirectedRecvResult::Error;
        }
        let Some(index) = c.directed_queue.iter().position(|message| message.target == target) else {
            return DirectedRecvResult::WouldBlock;
        };
        if c.directed_queue[index].data.len() > buf.len() {
            return DirectedRecvResult::Error;
        }
        let message = c.directed_queue.remove(index).unwrap();
        buf[..message.data.len()].copy_from_slice(&message.data);
        DirectedRecvResult::Ok { sender: message.sender, len: message.data.len() }
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
        let ch = channels();
        let Some(c) = slot(ch, channel_id) else {
            return RecvResult::Error;
        };
        if !c.holders.iter().any(|holder| holder.0 == pid) {
            return RecvResult::Error;
        }
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

/// Non-blocking dequeue: like `try_recv` but never registers a waiter or sleeps —
/// returns `Block` immediately when the queue is empty so a single-threaded poller
/// (e.g. the GUI compositor) can multiplex several sources in one loop.
pub fn try_recv_nonblock(channel_id: u64, buf: &mut [u8], pid: u64) -> RecvResult {
    with_lock(|| {
        let ch = channels();
        let Some(c) = slot(ch, channel_id) else {
            return RecvResult::Error;
        };
        if !c.holders.iter().any(|holder| holder.0 == pid) {
            return RecvResult::Error;
        }
        match c.queue.pop_front() {
            None => RecvResult::Block,
            Some(msg) => {
                let len = msg.data.len().min(buf.len());
                buf[..len].copy_from_slice(&msg.data[..len]);
                if !c.send_waiters.is_empty() {
                    let waiter_pid = c.send_waiters.remove(0);
                    crate::process::wakeup_ipc_waiter(waiter_pid, 0);
                }
                RecvResult::Ok(len)
            }
        }
    })
}

pub fn abort_senders(channel_id: u64) {
    with_lock(|| {
        let ch = channels();
        let Some(c) = slot(ch, channel_id) else { return; };
        c.queue.clear();
        for waiter in c.send_waiters.drain(..) {
            crate::process::wakeup_ipc_waiter(waiter, 0);
        }
    })
}

pub fn add_send_waiter(channel_id: u64, pid: u64) {
    with_lock(|| {
        let ch = channels();
        if let Some(c) = slot(ch, channel_id) {
            c.send_waiters.push(pid);
        }
    })
}

pub fn close(channel_id: u64, pid: u64) {
    with_lock(|| {
        let ch = channels();
        let Some(c) = slot(ch, channel_id) else { return; };
        let Some(index) = c.holders.iter().position(|holder| holder.0 == pid) else { return; };
        c.holders[index].1 -= 1;
        c.ref_count -= 1;
        if c.holders[index].1 == 0 {
            c.holders.remove(index);
            c.directed_queue.retain(|message| message.target != pid);
        }
        if c.ref_count != 0 { return; }
        ch[channel_id as usize - 1] = None;
    })
}

pub fn cleanup_pid(pid: u64) {
    with_lock(|| {
        let ch = channels();
        for slot in ch.iter_mut() {
            let Some(channel) = slot else { continue; };
            if let Some(index) = channel.holders.iter().position(|holder| holder.0 == pid) {
                channel.ref_count = channel.ref_count.saturating_sub(channel.holders[index].1);
                channel.holders.remove(index);
            }
            channel.directed_queue.retain(|message| message.target != pid);
            channel.recv_waiters.retain(|waiter| *waiter != pid);
            channel.send_waiters.retain(|waiter| *waiter != pid);
            if channel.ref_count == 0 {
                *slot = None;
            }
        }
    })
}
