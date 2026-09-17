use alloc::vec::Vec;
use crate::fs::devfs::DeviceOps;
use crate::ipc::{self, SendResult};
use crate::util::SyncUnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

static CHANNEL: SyncUnsafeCell<u64> = SyncUnsafeCell::new(u64::MAX);
static ONLINE: AtomicBool = AtomicBool::new(false);

static AUDIO_OPS: DeviceOps = DeviceOps {
    open: audio_open,
    close: audio_close,
    read: audio_read,
    write: audio_write,
    ioctl: audio_ioctl,
};

pub fn init() {
    let channel = ipc::open(b"module_audio");
    unsafe { *CHANNEL.0.get() = channel; }
    crate::fs::devfs::register("/dev/audio", &AUDIO_OPS);
}

pub fn driver_started() {
    ONLINE.store(true, Ordering::Release);
}

pub fn driver_stopped() {
    ONLINE.store(false, Ordering::Release);
    ipc::abort_senders(channel());
}

fn channel() -> u64 {
    unsafe { *CHANNEL.0.get() }
}

fn send(command: u8, data: &[u8]) -> SendResult {
    if !ONLINE.load(Ordering::Acquire) { return SendResult::Error; }
    let mut message = Vec::with_capacity(data.len() + 1);
    message.push(command);
    message.extend_from_slice(data);
    let sender = crate::scheduler::current_user_pid().unwrap_or(0);
    ipc::try_send_reliable(channel(), sender, &message)
}

fn audio_open() -> u64 { 1 }
fn audio_close(_handle: u64) {}
fn audio_read(_handle: u64, _buf: &mut [u8]) -> usize { 0 }

fn audio_write(_handle: u64, buf: &[u8]) -> usize {
    if buf.is_empty() || buf.len() > 8191 { return 0; }
    match send(2, buf) {
        SendResult::Ok => buf.len(),
        SendResult::Block | SendResult::Error => 0,
    }
}

fn audio_ioctl(_handle: u64, cmd: u64, arg: u64) -> i64 {
    match cmd {
        0 => match send(0, &[]) {
            SendResult::Ok => 0,
            _ => -1,
        },
        1 => {
            let frequency = (arg as u32).to_le_bytes();
            match send(3, &frequency) {
                SendResult::Ok => 0,
                _ => -1,
            }
        }
        2 => {
            let channels = [if arg == 1 { 1 } else { 2 }];
            match send(1, &channels) {
                SendResult::Ok => 0,
                _ => -1,
            }
        }
        3 => {
            if !ONLINE.load(Ordering::Acquire) { return -1; }
            let pid = crate::scheduler::current_user_pid().unwrap_or(0);
            if ipc::wait_send_space(channel(), pid) {
                crate::syscall::BLOCK_TO_SCHEDULER as i64
            } else {
                0
            }
        }
        _ => -1,
    }
}
