#![no_std]
#![no_main]
include!("../../crates/user_rt/module_runtime.rs");

pub fn kkm_info() -> KkmInfo {
    KkmInfo { name: "ps2mouse", depends: &[] }
}

// ── IPC channel ───────────────────────────────────────────────────────────────

static mut IPC_CH: u64 = 0;

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

// ── packet state ─────────────────────────────────────────────────────────────

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
            let mut dx = pb[1] as i16;
            let mut dy = pb[2] as i16;
            if pb[0] & 0x10 != 0 { dx -= 256; }
            if pb[0] & 0x20 != 0 { dy -= 256; }
            return Some((buttons, dx, dy));
        }
        None
    }
}

fn send_event(buttons: u8, dx: i16, dy: i16) {
    let ch = unsafe { IPC_CH };
    if ch == 0 || ch == u64::MAX { return; }
    let mut msg = [0u8; 5];
    msg[0] = buttons;
    msg[1..3].copy_from_slice(&dx.to_le_bytes());
    msg[3..5].copy_from_slice(&dy.to_le_bytes());
    sys_ipc_send(ch, &msg);
}

// ── lifecycle ─────────────────────────────────────────────────────────────────

pub fn kkm_init() -> bool {
    if !sys_ioport_request(0x60, 1) { return false; }
    if !sys_ioport_request(0x64, 1) { return false; }

    // Open IPC channel before initialising hardware so no events are missed.
    let ch = sys_ipc_open(b"module_mouse");
    if ch == u64::MAX { return false; }
    unsafe { IPC_CH = ch; }

    // Enable PS/2 auxiliary port and start streaming.
    outb(0x64, 0xA8);
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

pub fn kkm_run() {
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

pub fn kkm_exit() {
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

    let ch = unsafe { IPC_CH };
    if ch != 0 && ch != u64::MAX {
        syscall(SYS_IPC_CLOSE, ch, 0, 0);
    }
}
