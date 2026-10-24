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

pub(crate) use debug;
