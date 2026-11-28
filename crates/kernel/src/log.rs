use core::fmt;

use crate::util::SyncUnsafeCell;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
    Fatal = 4,
}

impl Level {
    pub const fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO ",
            Level::Warn => "WARN ",
            Level::Error => "ERROR",
            Level::Fatal => "FATAL",
        }
    }
}

static MAX_LEVEL: SyncUnsafeCell<Level> = SyncUnsafeCell::new(Level::Debug);

pub fn set_max_level(level: Level) {
    unsafe { *MAX_LEVEL.0.get() = level; }
}

pub fn max_level() -> Level {
    unsafe { *MAX_LEVEL.0.get() }
}

fn basename(file: &str) -> &str {
    let s = file.rsplit('/').next().unwrap_or(file);
    let s = s.rsplit('\\').next().unwrap_or(s);
    s.strip_suffix(".rs").unwrap_or(s)
}

/// Core logging function. Writes to serial only (no framebuffer) so it is
/// safe to call from interrupt handlers, panic handlers, and fault paths.
pub fn log(level: Level, file: &str, line: u32, args: fmt::Arguments) {
    if level < unsafe { *MAX_LEVEL.0.get() } {
        return;
    }

    let module = basename(file);
    let ticks = crate::handlers::interrupts::timer_ticks();

    // Compose the whole record first so it reaches serial and the log
    // buffer as one piece instead of interleaving with other writers.
    use fmt::Write;
    let mut lb = crate::logbuf::LineBuf::new();
    let _ = write!(
        lb,
        "[T+{:>10}] [{}] [{:20}:{:>3}] {}\n",
        ticks,
        level.as_str(),
        module,
        line,
        args
    );
    crate::serial_print!("{}", lb.as_str());
    crate::logbuf::push(lb.as_bytes());
}

/// Log with explicit level. Used by the macros below.
#[macro_export]
macro_rules! _log_with_level {
    ($level:expr, $($arg:tt)*) => {
        $crate::log::log(
            $level,
            file!(),
            line!(),
            format_args!($($arg)*)
        )
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::_log_with_level!($crate::log::Level::Debug, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::_log_with_level!($crate::log::Level::Info, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {
        $crate::_log_with_level!($crate::log::Level::Warn, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::_log_with_level!($crate::log::Level::Error, $($arg)*)
    };
}

#[macro_export]
macro_rules! log_fatal {
    ($($arg:tt)*) => {
        $crate::_log_with_level!($crate::log::Level::Fatal, $($arg)*)
    };
}
