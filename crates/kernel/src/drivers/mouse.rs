use crate::util::SyncUnsafeCell;

static INITIALIZED: SyncUnsafeCell<bool> = SyncUnsafeCell::new(false);
static BUTTONS: SyncUnsafeCell<u8> = SyncUnsafeCell::new(0);
static DX: SyncUnsafeCell<i32> = SyncUnsafeCell::new(0);
static DY: SyncUnsafeCell<i32> = SyncUnsafeCell::new(0);
static X: SyncUnsafeCell<u32> = SyncUnsafeCell::new(0);
static Y: SyncUnsafeCell<u32> = SyncUnsafeCell::new(0);
static NEW_DATA: SyncUnsafeCell<bool> = SyncUnsafeCell::new(false);

static PKT: SyncUnsafeCell<[u8; 3]> = SyncUnsafeCell::new([0; 3]);
static PKT_IDX: SyncUnsafeCell<usize> = SyncUnsafeCell::new(0);

pub(crate) unsafe fn init() {
    unsafe {
        crate::util::outb(0x64, 0xA8);
        mouse_command(0xF6);
        mouse_command(0xF4);
        for _ in 0..16 {
            if crate::util::inb(0x64) & 0x01 != 0 {
                crate::util::inb(0x60);
            } else {
                break;
            }
        }
        *INITIALIZED.0.get() = true;
        *PKT_IDX.0.get() = 0;
    }
}

unsafe fn mouse_command(cmd: u8) {
    while unsafe { crate::util::inb(0x64) } & 0x02 != 0 {}
    unsafe { crate::util::outb(0x64, 0xD4); }
    while unsafe { crate::util::inb(0x64) } & 0x02 != 0 {}
    unsafe { crate::util::outb(0x60, cmd); }
}

pub(crate) unsafe fn poll() {
    unsafe {
        if !*INITIALIZED.0.get() {
            return;
        }
        let status = crate::util::inb(0x64);
        if status & 0x01 == 0 {
            return;
        }
        if status & 0x20 == 0 {
            return;
        }
        let data = crate::util::inb(0x60);
        let idx = *PKT_IDX.0.get();
        (*PKT.0.get())[idx] = data;
        if idx == 0 {
            if data & 0x08 == 0 {
                return;
            }
            *PKT_IDX.0.get() = 1;
        } else if idx == 1 {
            *PKT_IDX.0.get() = 2;
        } else {
            *PKT_IDX.0.get() = 0;
            let pb = *PKT.0.get();
            let buttons = pb[0] & 0x07;
            let mut dx_v = pb[1] as i32;
            let mut dy_v = pb[2] as i32;
            if pb[0] & 0x10 != 0 {
                dx_v -= 256;
            }
            if pb[0] & 0x20 != 0 {
                dy_v -= 256;
            }
            *BUTTONS.0.get() = buttons;
            let old_dx = *DX.0.get();
            *DX.0.get() = old_dx.wrapping_add(dx_v);
            let old_dy = *DY.0.get();
            *DY.0.get() = old_dy.wrapping_add(dy_v);
            if let Some((w, h)) = screen_size() {
                let old_x = *X.0.get();
                let old_y = *Y.0.get();
                let new_x = (old_x as i32 + dx_v).clamp(0, w as i32 - 1);
                let new_y = (old_y as i32 + dy_v).clamp(0, h as i32 - 1);
                *X.0.get() = new_x as u32;
                *Y.0.get() = new_y as u32;
            }
            *NEW_DATA.0.get() = true;
            crate::process::wakeup_mouse_waiters();
        }
    }
}

fn screen_size() -> Option<(u32, u32)> {
    if let Some(p) = crate::console::fb_params() {
        Some((p.width, p.height))
    } else {
        None
    }
}

pub fn read_state() -> u64 {
    unsafe {
        if !*NEW_DATA.0.get() {
            return 0;
        }
        *NEW_DATA.0.get() = false;
        let buttons = *BUTTONS.0.get();
        let dx = *DX.0.get();
        let dy = *DY.0.get();
        let x = *X.0.get();
        let y = *Y.0.get();
        *DX.0.get() = 0;
        *DY.0.get() = 0;
        1 | ((buttons as u64) << 1)
            | (((dx as i16) as u16 as u64) << 8)
            | (((dy as i16) as u16 as u64) << 24)
            | ((x as u64) << 40)
            | ((y as u64) << 52)
    }
}
