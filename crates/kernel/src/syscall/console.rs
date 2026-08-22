#[allow(unused_imports)]
use crate::{console, ipc, process, syscall};
#[allow(unused_imports)]
use crate::syscall::runtime::*;

pub(crate) fn handle(number: u64, arg0: u64, arg1: u64, _arg2: u64) -> u64 {
    match number {
    // Console / Display
            SYS_CONSOLE_WRITE => {
                if arg0 != 0 && arg1 > 0 {
                    if !crate::memory::uaccess::validate_range(arg0, arg1, false) {
                        return u64::MAX;
                    }
                    let caller = crate::scheduler::current_user_pid().unwrap_or(0);
                    let fb_owner = crate::drivers::fb_owner::owner();
                    let do_fb = fb_owner.is_none() || fb_owner == Some(caller);
                    print_user_bytes(arg0, arg1, do_fb);
                }
                0
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
