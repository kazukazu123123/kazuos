#![no_std]
#![no_main]
include!("../runtime/driver_runtime.rs");

pub fn kdm_info() -> KdmInfo {
    KdmInfo { name: "ps2mouse", depends: &[] }
}

// ── IPC channel ───────────────────────────────────────────────────────────────


// ── PS/2 I/O ─────────────────────────────────────────────────────────────────

fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe { core::arch::asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack)) }
    v
}

fn outb(port: u16, v: u8) {
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") v, options(nomem, nostack)) }
}

fn ps2_wait_write() {
    let mut i = 0u32;
    while i < 10000 {
        if inb(0x64) & 0x02 == 0 { return; }
        i += 1;
    }
}

fn mouse_cmd(cmd: u8) {
    ps2_wait_write();
    outb(0x64, 0xD4);
    ps2_wait_write();
    outb(0x60, cmd);
}

/// Wait for the mouse to ACK a command (0xFA), skipping any stale bytes.
/// Returns false when nothing ACKs: with no controller present the data
/// port reads back 0xFF, which is never a valid ACK.
fn mouse_wait_ack() -> bool {
    for _ in 0..4 {
        let mut i = 0u32;
        let data = loop {
            if inb(0x64) & 0x01 != 0 { break inb(0x60); }
            i += 1;
            if i >= 100_000 { return false; }
        };
        if data == 0xFA { return true; }
    }
    false
}

// ── packet state ─────────────────────────────────────────────────────────────

static mut INPUT_FD: u64 = u64::MAX;
static mut PKT: [u8; 3] = [0; 3];
static mut PKT_IDX: u8  = 0;

/// Returns Some((buttons, dx, dy)) when a complete 3-byte packet is ready.
fn feed_byte(data: u8) -> Option<(u8, i16, i16)> {
    unsafe {
        let idx = PKT_IDX as usize;
        PKT[idx] = data;
        if idx == 0 {
            if data & 0x08 == 0 { return None; }
            PKT_IDX = 1;
        } else if idx == 1 {
            PKT_IDX = 2;
        } else {
            PKT_IDX = 0;
            let pb = PKT;
            let buttons = pb[0] & 0x07;
            let x_negative = pb[0] & 0x10 != 0;
            let y_negative = pb[0] & 0x20 != 0;
            let dx = if pb[0] & 0x40 != 0 {
                if x_negative { -255 } else { 255 }
            } else if x_negative {
                pb[1] as i16 - 256
            } else {
                pb[1] as i16
            };
            let dy = if pb[0] & 0x80 != 0 {
                if y_negative { -255 } else { 255 }
            } else if y_negative {
                pb[2] as i16 - 256
            } else {
                pb[2] as i16
            };
            return Some((buttons, dx, dy));
        }
        None
    }
}

fn send_event(buttons: u8, dx: i16, dy: i16) {
    // Write one record to /dev/input/mouse; the kernel queue merges all drivers.
    let fd = unsafe { INPUT_FD };
    if fd == u64::MAX { return; }
    let mut record = [0u8; 12];
    record[0] = buttons;
    record[4..8].copy_from_slice(&(dx as i32).to_le_bytes());
    record[8..12].copy_from_slice(&(dy as i32).to_le_bytes());
    syscall(SYS_WRITE, fd, record.as_ptr() as u64, record.len() as u64);
}

// ── lifecycle ─────────────────────────────────────────────────────────────────

pub fn kdm_init() -> bool {
    if !sys_ioport_request(0x60, 1) { return false; }
    if !sys_ioport_request(0x64, 1) { return false; }

    // Enable the auxiliary port, then probe with a reset: a present mouse
    // ACKs (0xFA). On machines without a PS/2 controller (USB-only input)
    // the ports read back 0xFF instead, and the poll loop would mistake
    // that for an endless stream of mouse packets, so stop here.
    outb(0x64, 0xA8);
    mouse_cmd(0xFF);
    if !mouse_wait_ack() { return false; }

    unsafe {
        PKT_IDX = 0;
    }

    // Open the unified input device; events are written there as records.
    let fd = syscall(SYS_OPEN, b"/dev/input/mouse".as_ptr() as u64, 16, 0);
    if fd == u64::MAX { return false; }
    unsafe { INPUT_FD = fd; }

    // Set defaults and start streaming (auxiliary port already enabled).
    mouse_cmd(0xF6);
    mouse_cmd(0xF4);

    // Drain leftover bytes.
    let mut i = 0u32;
    while i < 16 {
        if inb(0x64) & 0x01 != 0 { inb(0x60); } else { break; }
        i += 1;
    }

    true
}

pub fn kdm_run() {
    loop {
        // Wait for the next timer tick (works in both IRQ and polling environments).
        sys_sleep_tick();

        if sys_signal_check() { return; }

        // Drain all available PS/2 bytes accumulated since last tick.
        let mut i = 0u32;
        while i < 16 {
            let status = inb(0x64);
            if status & 0x01 == 0 { break; }
            if status & 0x20 == 0 { break; } // not mouse data
            let data = inb(0x60);
            if let Some((buttons, dx, dy)) = feed_byte(data) {
                send_event(buttons, dx, dy);
            }
            i += 1;
        }
    }
}

pub fn kdm_exit() {
    mouse_cmd(0xF5); // disable streaming

    // Drain any remaining mouse bytes from the PS/2 controller.
    let mut i = 0u32;
    while i < 16 {
        let status = inb(0x64);
        if status & 0x01 == 0 { break; }
        if status & 0x20 != 0 {
            inb(0x60); // discard mouse data
        } else {
            break; // keyboard data — stop draining
        }
        i += 1;
    }

    // Disable PS/2 auxiliary port so the controller stops routing mouse data.
    ps2_wait_write();
    outb(0x64, 0xA7);

    let fd = unsafe { INPUT_FD };
    if fd != u64::MAX {
        syscall(SYS_CLOSE, fd, 0, 0);
    }
}
