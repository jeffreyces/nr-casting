//! The mod's log file: one file beside the DLL, rotated at 4 MiB, plus the debug gate.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::config;

static LOG_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
/// Open once and reused: opening/closing per line cost a syscall pair per cast.
static LOG_FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
static LOG_WRITES: AtomicU32 = AtomicU32::new(0);
pub(crate) static DEBUG_LOG: AtomicBool = AtomicBool::new(false);
/// Rotate at 4 MiB so a long session cannot grow the file without bound.
const LOG_LIMIT: u64 = 4 * 1024 * 1024;
fn log_path() -> &'static std::path::Path {
    LOG_PATH.get_or_init(|| config::path("nr_casting_probe.log"))
}

/// Detailed per-cast chatter; enabled with `debug_log = on` in the ini.
pub(crate) fn debug_enabled() -> bool {
    DEBUG_LOG.load(Ordering::Relaxed)
}

/// Log only when debugging is on: the check comes before `format!`, so a disabled debug log
/// costs one relaxed load instead of a `String` per input poll.
macro_rules! debug {
    ($($arg:tt)*) => {
        if $crate::debug_enabled() {
            $crate::log(&format!($($arg)*));
        }
    };
}

pub(crate) fn log(msg: &str) {
    use std::io::Write;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let slot = LOG_FILE.get_or_init(|| Mutex::new(None));

    // Size check costs a syscall, so only pay for it occasionally.
    if LOG_WRITES.fetch_add(1, Ordering::Relaxed).is_multiple_of(256)
        && std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0) > LOG_LIMIT
    {
        let _ = std::fs::rename(log_path(), config::path("nr_casting_probe.old.log"));
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
            .ok();
    }
    if let Some(handle) = guard.as_mut() {
        let _ = handle.write_all(format!("[{ms}] {msg}\n").as_bytes());
    }
}

/// Log a line without ever blocking on the log mutex.
///
/// For panic and fault paths: if the code that panicked was the logger itself, a blocking lock
/// here would deadlock the process instead of letting it die with a note in the file.
pub(crate) fn try_log(msg: &str) {
    use std::io::Write;
    let slot = LOG_FILE.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.try_lock() else {
        return;
    };
    if guard.is_none() {
        *guard = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
            .ok();
    }
    if let Some(handle) = guard.as_mut() {
        let _ = handle.write_all(format!("{msg}\n").as_bytes());
    }
}

/// One line about a fault, built in a stack buffer.
///
/// Called from an exception handler, so it must not allocate (the heap may be what broke) and
/// must not block (the fault may have happened while the logger's mutex was held — `try_lock`
/// drops the line instead of deadlocking the process). The formatting itself lives in
/// `nr_casting_core::fault_line` so it can be tested without a game.
pub(crate) fn log_fault(
    ms: u64,
    code: u32,
    address: usize,
    thread: u32,
    module_base: usize,
    offset: usize,
    module: &str,
    access: Option<(usize, usize)>,
) {
    use std::io::Write;
    let mut buf = [0u8; 512];
    let len = nr_casting_core::fault_line(
        &mut buf,
        ms,
        code,
        address,
        thread,
        module_base,
        offset,
        module,
        access,
    );

    let slot = LOG_FILE.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = slot.try_lock() else {
        return;
    };
    if guard.is_none() {
        *guard = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
            .ok();
    }
    if let Some(handle) = guard.as_mut() {
        let _ = handle.write_all(&buf[..len]);
    }
}

pub(crate) use debug;
