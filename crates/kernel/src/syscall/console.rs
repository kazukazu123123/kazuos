#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    match number {
    // Console / Display
            SYS_LOG_WRITE => {
                if arg0 != 0 && arg1 > 0 {
                    if !crate::memory::uaccess::validate_range(arg0, arg1, false) {
                        return u64::MAX;
                    }
                    // The log channel: records land in the kernel log buffer
                    // and on serial. The screen belongs to the interactive
                    // session (SYS_WRITE), so log output cannot interleave
                    // with the shell — except urgent records (arg2), which
                    // are also shown on the console unless another process
                    // owns the framebuffer.
                    let to_screen = arg2 >= LOG_LEVEL_URGENT && console_writable();
                    log_user_bytes(arg0, arg1, to_screen);
                }
                0
            }
            SYS_LOG_READ => {
                // arg0 = offset from the oldest retained byte. With
                // arg1 == 0, returns the retained byte count instead.
                if arg1 == 0 {
                    return crate::logbuf::len() as u64;
                }
                if arg2 == 0 || !crate::memory::uaccess::validate_range(arg1, arg2, true) {
                    return u64::MAX;
                }
                let mut done = 0usize;
                let mut tmp = [0u8; 512];
                while (done as u64) < arg2 {
                    let want = (arg2 as usize - done).min(tmp.len());
                    let n = crate::logbuf::read(arg0 as usize + done, &mut tmp[..want]);
                    if n == 0 {
                        break;
                    }
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            tmp.as_ptr(),
                            (arg1 as usize + done) as *mut u8,
                            n,
                        );
                    }
                    done += n;
                }
                done as u64
            }
            // Console / cursor ops touch the framebuffer, so suppress them when another
            // process owns it (a background console shell must not draw over a GUI).
            SYS_CURSOR_SAVE => { if console_writable() { console::save_cursor_pos(); } 0 }
            SYS_CURSOR_RESTORE => { if console_writable() { console::restore_cursor_pos(); } 0 }
            SYS_CURSOR_DRAW => {
                if console_writable() { console::draw_saved_cursor(arg0 != 0); }
                0
            }
            SYS_FB_ACQUIRE => {
                if let Some(pid) = crate::scheduler::current_user_pid() {
                    if let Some(ctx) = process::user_context(pid) {
                        crate::drivers::fb_owner::acquire(pid, ctx.cr3, arg0 as *mut crate::drivers::fb_owner::FbInfo)
                    } else { u64::MAX }
                } else { u64::MAX }
            }
            SYS_FB_RELEASE => { if let Some(pid) = crate::scheduler::current_user_pid() { crate::drivers::fb_owner::release(pid); } 0 }
            SYS_CONSOLE_SIZE => {
                // Terminal-size get/set: arg0 == 0 gets, arg0 != 0 sets.
                if arg0 == 0 {
                    // GET: the caller's terminal size, or the console if none was set.
                    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
                    let (mut cols, mut rows) = process::winsize(caller);
                    if cols == 0 || rows == 0 {
                        let (c, r) = crate::terminal::console::console_size();
                        cols = c as u16;
                        rows = r as u16;
                    }
                    ((rows as u64) << 32) | (cols as u64)
                } else {
                    // SET: cols = arg0 & 0xFFFF, rows = arg0 >> 16; target pid = arg1 (0 = self).
                    let cols = (arg0 & 0xFFFF) as u16;
                    let rows = ((arg0 >> 16) & 0xFFFF) as u16;
                    let target = if arg1 == 0 { crate::scheduler::current_user_pid().unwrap_or(0) } else { arg1 };
                    process::set_winsize(target, cols, rows);
                    0
                }
            }

                _ => u64::MAX,
    }
}
