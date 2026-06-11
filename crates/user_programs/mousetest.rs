#![no_std]
#![no_main]
include!("../../crates/user_rt/runtime.rs");

const CURSOR_COLOR: u32 = 0x00_FF_FF_00; // yellow
const BG_COLOR:     u32 = 0x00_10_10_10; // near-black
const TEXT_COLOR:   u32 = 0x00_FF_FF_FF; // white
const BTN_COLOR:    u32 = 0x00_00_CC_FF; // cyan

#[repr(C)]
struct FbInfo {
    base:   u64,
    width:  u32,
    height: u32,
    stride: u32,
    format: u32, // 0=RGB, 1=BGR
}

// ── pixel helpers ─────────────────────────────────────────────────────────────

fn pack(info: &FbInfo, r: u8, g: u8, b: u8) -> u32 {
    if info.format == 1 {
        (b as u32) << 16 | (g as u32) << 8 | (r as u32)
    } else {
        (r as u32) << 16 | (g as u32) << 8 | (b as u32)
    }
}

fn unpack_rgb(info: &FbInfo, px: u32) -> (u8, u8, u8) {
    if info.format == 1 {
        ((px & 0xFF) as u8, ((px >> 8) & 0xFF) as u8, ((px >> 16) & 0xFF) as u8)
    } else {
        (((px >> 16) & 0xFF) as u8, ((px >> 8) & 0xFF) as u8, (px & 0xFF) as u8)
    }
}

fn fb_ptr(info: &FbInfo) -> *mut u32 {
    info.base as *mut u32
}

fn put_pixel(info: &FbInfo, x: u32, y: u32, color: u32) {
    if x >= info.width || y >= info.height { return; }
    unsafe {
        *fb_ptr(info).add((y * info.stride + x) as usize) = color;
    }
}

fn get_pixel(info: &FbInfo, x: u32, y: u32) -> u32 {
    if x >= info.width || y >= info.height { return 0; }
    unsafe { *fb_ptr(info).add((y * info.stride + x) as usize) }
}

fn fill_rect(info: &FbInfo, x: u32, y: u32, w: u32, h: u32, color: u32) {
    for dy in 0..h {
        for dx in 0..w {
            put_pixel(info, x + dx, y + dy, color);
        }
    }
}

// ── tiny 5×7 font (digits, letters, punctuation we need) ─────────────────────

fn draw_char(info: &FbInfo, x: u32, y: u32, c: u8, color: u32) {
    let glyph: [u8; 7] = match c {
        b'0' => [0x7E,0x81,0x81,0x81,0x81,0x81,0x7E],
        b'1' => [0x10,0x30,0x10,0x10,0x10,0x10,0x38],
        b'2' => [0x7E,0x01,0x01,0x7E,0x80,0x80,0xFF],
        b'3' => [0x7E,0x01,0x01,0x3E,0x01,0x01,0x7E],
        b'4' => [0x81,0x81,0x81,0x7F,0x01,0x01,0x01],
        b'5' => [0xFF,0x80,0x80,0xFE,0x01,0x01,0xFE],
        b'6' => [0x7E,0x80,0x80,0xFE,0x81,0x81,0x7E],
        b'7' => [0xFF,0x01,0x02,0x04,0x08,0x10,0x10],
        b'8' => [0x7E,0x81,0x81,0x7E,0x81,0x81,0x7E],
        b'9' => [0x7E,0x81,0x81,0x7F,0x01,0x01,0x7E],
        b'-' => [0x00,0x00,0x00,0x7E,0x00,0x00,0x00],
        b'+' => [0x00,0x18,0x18,0x7E,0x18,0x18,0x00],
        b'L' => [0x80,0x80,0x80,0x80,0x80,0x80,0xFF],
        b'R' => [0xFE,0x81,0x81,0xFE,0x90,0x88,0x87],
        b'M' => [0x81,0xC3,0xA5,0x99,0x81,0x81,0x81],
        b'x' => [0x00,0x00,0x81,0x42,0x24,0x42,0x81],
        b'y' => [0x00,0x81,0x81,0x7E,0x01,0x01,0x7E],
        b'd' => [0x01,0x01,0x7F,0x81,0x81,0x81,0x7F],
        b'X' => [0x81,0x42,0x24,0x18,0x24,0x42,0x81],
        b'Y' => [0x81,0x42,0x24,0x18,0x18,0x18,0x18],
        b'(' => [0x18,0x30,0x60,0x60,0x60,0x30,0x18],
        b')' => [0x30,0x18,0x0C,0x0C,0x0C,0x18,0x30],
        b' ' => [0x00;7],
        b':' => [0x00,0x18,0x18,0x00,0x18,0x18,0x00],
        b',' => [0x00,0x00,0x00,0x00,0x18,0x18,0x30],
        _   => [0x99,0x5A,0x3C,0x18,0x3C,0x5A,0x99],
    };
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..8u32 {
            if bits & (0x80 >> col) != 0 {
                put_pixel(info, x + col, y + row as u32, color);
            }
        }
    }
}

fn draw_str(info: &FbInfo, mut x: u32, y: u32, s: &[u8], color: u32) {
    for &c in s {
        draw_char(info, x, y, c, color);
        x += 9;
    }
}

fn draw_i16(info: &FbInfo, x: u32, y: u32, v: i16, color: u32) {
    let mut buf = [b' '; 6]; // sign + 5 digits
    let mut tmp = if v < 0 { buf[0] = b'-'; -(v as i32) } else { buf[0] = b'+'; v as i32 };
    let mut i = 5usize;
    loop {
        buf[i] = b'0' + (tmp % 10) as u8;
        tmp /= 10;
        if tmp == 0 { break; }
        i -= 1;
    }
    draw_str(info, x, y, &buf, color);
}

fn draw_u32(info: &FbInfo, x: u32, y: u32, v: u32, color: u32) {
    let mut buf = [b'0'; 5];
    let mut tmp = v;
    let mut i = 4usize;
    loop {
        buf[i] = b'0' + (tmp % 10) as u8;
        tmp /= 10;
        if tmp == 0 { break; }
        if i == 0 { break; }
        i -= 1;
    }
    draw_str(info, x, y, &buf[i..], color);
}

// ── crosshair cursor ──────────────────────────────────────────────────────────

const CROSS_HALF: i32 = 12;
const CROSS_GAP:  i32 = 3;

struct SavedLines {
    hbuf: [u32; 32], // 2*CROSS_HALF pixels
    vbuf: [u32; 32],
    x: u32,
    y: u32,
    valid: bool,
}

impl SavedLines {
    const fn new() -> Self {
        Self { hbuf: [0; 32], vbuf: [0; 32], x: 0, y: 0, valid: false }
    }
}

static mut SAVED: SavedLines = SavedLines::new();

fn save_cross(info: &FbInfo, cx: u32, cy: u32) {
    unsafe {
        let s = &mut SAVED;
        let half = CROSS_HALF as u32;
        // The cross spans 2*half+1 pixels per arm — save every one, including the
        // far end, or the last pixel is never restored and leaves a stray dot.
        let span = 2 * half as usize + 1;
        for i in 0..span {
            let px = cx.saturating_sub(half) + i as u32;
            s.hbuf[i] = get_pixel(info, px, cy);
        }
        for i in 0..span {
            let py = cy.saturating_sub(half) + i as u32;
            s.vbuf[i] = get_pixel(info, cx, py);
        }
        s.x = cx;
        s.y = cy;
        s.valid = true;
    }
}

fn restore_cross(info: &FbInfo) {
    unsafe {
        let s = &SAVED;
        if !s.valid { return; }
        let half = CROSS_HALF as u32;
        let span = 2 * half as usize + 1;
        for i in 0..span {
            let px = s.x.saturating_sub(half) + i as u32;
            put_pixel(info, px, s.y, s.hbuf[i]);
        }
        for i in 0..span {
            let py = s.y.saturating_sub(half) + i as u32;
            put_pixel(info, s.x, py, s.vbuf[i]);
        }
    }
}

fn draw_cross(info: &FbInfo, cx: u32, cy: u32, color: u32) {
    let half = CROSS_HALF as u32;
    let gap  = CROSS_GAP  as u32;
    // horizontal arms
    let x0 = cx.saturating_sub(half);
    for dx in 0..(2 * half + 1) {
        let px = x0 + dx;
        if px >= cx.saturating_sub(gap) && px <= cx + gap { continue; }
        put_pixel(info, px, cy, color);
    }
    // vertical arms
    let y0 = cy.saturating_sub(half);
    for dy in 0..(2 * half + 1) {
        let py = y0 + dy;
        if py >= cy.saturating_sub(gap) && py <= cy + gap { continue; }
        put_pixel(info, cx, py, color);
    }
}

// ── HUD ───────────────────────────────────────────────────────────────────────

fn draw_hud(info: &FbInfo, mx: u32, my: u32, dx: i16, dy: i16, buttons: u8) {
    // background strip
    fill_rect(info, 0, 0, info.width, 20, pack(info, 0x20, 0x20, 0x20));

    // "mousetest" label
    draw_str(info, 4, 6, b"mousetest", TEXT_COLOR);

    // X: <val>
    draw_str(info, 100, 6, b"X:", TEXT_COLOR);
    draw_u32(info, 118, 6, mx, TEXT_COLOR);

    // Y: <val>
    draw_str(info, 172, 6, b"Y:", TEXT_COLOR);
    draw_u32(info, 190, 6, my, TEXT_COLOR);

    // dX: <val>
    draw_str(info, 244, 6, b"dX:", TEXT_COLOR);
    draw_i16(info, 271, 6, dx, TEXT_COLOR);

    // dY: <val>
    draw_str(info, 334, 6, b"dY:", TEXT_COLOR);
    draw_i16(info, 361, 6, dy, TEXT_COLOR);

    // button indicators
    let lb = if buttons & 1 != 0 { BTN_COLOR } else { pack(info, 0x40, 0x40, 0x40) };
    let rb = if buttons & 2 != 0 { BTN_COLOR } else { pack(info, 0x40, 0x40, 0x40) };
    let mb = if buttons & 4 != 0 { BTN_COLOR } else { pack(info, 0x40, 0x40, 0x40) };
    draw_char(info, 424, 6, b'L', lb);
    draw_char(info, 434, 6, b'M', mb);
    draw_char(info, 444, 6, b'R', rb);

    // quit hint
    draw_str(info, info.width.saturating_sub(80), 6, b"(q)quit", TEXT_COLOR);
}

// ── syscall helpers ───────────────────────────────────────────────────────────

fn sys_write(buf: &[u8]) {
    syscall(SYS_CONSOLE_WRITE, buf.as_ptr() as u64, buf.len() as u64, 0);
}

fn syscall(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let r: u64;
    unsafe {
        core::arch::asm!(
            "int 0x80",
            inlateout("rax") n => r,
            in("rdi") a0, in("rsi") a1, in("rdx") a2,
        );
    }
    r
}

// ── main ──────────────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn user_main(_argc: u64, _argv: u64) -> ! {
    let mut info = FbInfo { base: 0, width: 0, height: 0, stride: 0, format: 0 };

    // Acquire framebuffer
    let r = syscall(SYS_FB_ACQUIRE, &mut info as *mut FbInfo as u64, 0, 0);
    if r == u64::MAX {
        sys_write(b"mousetest: failed to acquire framebuffer\r\n");
        sys_exit(1);
    }

    syscall(SYS_SIGNAL_CATCH, 1, 0, 0);

    // Open IPC channel (ps2mouse.kkm must be loaded)
    let ipc = syscall(SYS_IPC_OPEN, b"module_mouse".as_ptr() as u64, 12, 0);
    if ipc == u64::MAX {
        sys_write(b"mousetest: IPC module_mouse not found (is ps2mouse.kkm loaded?)\r\n");
        syscall(SYS_FB_RELEASE, 0, 0, 0);
        sys_exit(1);
    }

    // Clear to background
    fill_rect(&info, 0, 0, info.width, info.height, BG_COLOR);

    let cx = info.width  / 2;
    let cy = info.height / 2;
    let mut mx: u32 = cx;
    let mut my: u32 = cy;
    let mut buttons: u8 = 0;
    let mut last_dx: i16 = 0;
    let mut last_dy: i16 = 0;

    draw_hud(&info, mx, my, 0, 0, 0);
    save_cross(&info, mx, my);
    draw_cross(&info, mx, my, pack(&info, 0xFF, 0xFF, 0x00));

    let mut msg = [0u8; 5];

    loop {
        // Non-blocking keyboard check
        let k = syscall(SYS_KEYBOARD_POLL, 0, 0, 0);
        if k != 0 && k != u64::MAX {
            let ch = (k & 0xFF) as u8;
            if ch == b'q' || ch == b'Q' { break; }
        }

        // Signal check
        if syscall(SYS_SIGNAL_CHECK, 0, 0, 0) != 0 { break; }

        // Receive mouse event (non-blocking via IPC_RECV with len check)
        let n = syscall(SYS_IPC_RECV, ipc, msg.as_mut_ptr() as u64, 5);
        if n == u64::MAX || n < 5 {
            // No message yet; yield to avoid busy-loop
            sys_sleep(1);
            continue;
        }

        buttons = msg[0];
        let dx = i16::from_le_bytes([msg[1], msg[2]]);
        let dy = i16::from_le_bytes([msg[3], msg[4]]);
        last_dx = dx;
        last_dy = dy;

        // Update position, clamped to screen
        let new_x = (mx as i32 + dx as i32).clamp(0, info.width  as i32 - 1) as u32;
        let new_y = (my as i32 - dy as i32).clamp(20, info.height as i32 - 1) as u32; // Y inverted, avoid HUD

        // Move cursor
        restore_cross(&info);
        mx = new_x;
        my = new_y;

        draw_hud(&info, mx, my, last_dx, last_dy, buttons);
        save_cross(&info, mx, my);
        draw_cross(&info, mx, my, pack(&info, 0xFF, 0xFF, 0x00));
    }

    // Cleanup: restore cursor area, close the mouse channel, release FB.
    restore_cross(&info);
    syscall(SYS_IPC_CLOSE, ipc, 0, 0);
    syscall(SYS_FB_RELEASE, 0, 0, 0);
    sys_exit(0);
}
