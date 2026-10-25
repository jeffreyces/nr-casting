//! A last-resort observer: if the process faults, say where it faulted before it dies.
//!
//! A user's "it crashed at the loading screen" is only actionable with an address. This installs
//! a **vectored exception handler** that never handles anything — it returns
//! `EXCEPTION_CONTINUE_SEARCH` so the game's own handling, the debugger and the crash reporter all
//! behave exactly as they would without us. All it does is write one line naming the exception,
//! the faulting address, the module that owns it and the offset inside that module.
//!
//! Rules, because this runs on a stack that is already broken:
//!
//! - **No allocation, no formatting machinery.** The line is built in a fixed stack buffer.
//! - **Never block.** The log mutex is taken with `try_lock`; if the fault happened inside the
//!   logger, the line is dropped rather than deadlocking the process.
//! - **Cap the noise.** Eight lines per process, then silence.
//! - **Skip the exceptions that are normal.** Breakpoints, MSVC C++ throws and Ctrl-C are not
//!   crashes.

use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
struct ExceptionRecord {
    code: u32,
    flags: u32,
    record: *mut ExceptionRecord,
    address: *mut u8,
    number_parameters: u32,
    information: [usize; 15],
}

#[repr(C)]
struct ExceptionPointers {
    exception_record: *mut ExceptionRecord,
    context_record: *mut u8,
}

extern "system" {
    fn AddVectoredExceptionHandler(first: u32, handler: *const ()) -> *mut u8;
    fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut *mut u8) -> i32;
    fn GetModuleFileNameW(module: *mut u8, buffer: *mut u16, size: u32) -> u32;
    fn GetCurrentThreadId() -> u32;
}

const FROM_ADDRESS: u32 = 0x4;
const UNCHANGED_REFCOUNT: u32 = 0x2;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const ACCESS_VIOLATION: u32 = 0xC000_0005;
const MAX_LOGGED: u32 = 8;

static LOGGED: AtomicU32 = AtomicU32::new(0);

/// Install the observer. Cheap, idempotent enough for a single call at startup.
pub fn install() {
    // SAFETY: a process-lifetime handler; the pointer is a plain function address.
    unsafe {
        AddVectoredExceptionHandler(1, exception_handler as *const ());
    }
}

unsafe extern "system" fn exception_handler(info: *mut ExceptionPointers) -> i32 {
    if info.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let record = (*info).exception_record;
    if record.is_null() {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let code = (*record).code;
    // Not crashes: a debug break, a C++ exception being thrown, Ctrl-C, and the two
    // `RaiseException` codes Wine and MSVC use for thread names and OutputDebugString. Logging
    // those buries the one line that matters.
    const IGNORED: [u32; 6] = [
        0x8000_0003, // breakpoint
        0xE06D_7363, // MSVC C++ throw
        0x4001_0003, // Ctrl-C
        0x406D_1388, // MS_VC_EXCEPTION (thread name)
        0x4001_0006, // DBG_PRINTEXCEPTION_C (OutputDebugString)
        0x4001_000A, // DBG_PRINTEXCEPTION_WIDE_C
    ];
    if IGNORED.contains(&code) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    if LOGGED.fetch_add(1, Ordering::Relaxed) >= MAX_LOGGED {
        return EXCEPTION_CONTINUE_SEARCH;
    }

    let address = (*record).address as usize;
    let mut module = [0u16; 96];
    let mut base = 0usize;
    let mut module_len = 0usize;
    // SAFETY: out-parameter is a local; the address is whatever the CPU faulted on, which
    // GetModuleHandleExW answers without touching it.
    unsafe {
        let mut handle: *mut u8 = std::ptr::null_mut();
        if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address as *const u16, &mut handle) != 0
            && !handle.is_null()
        {
            base = handle as usize;
            module_len = GetModuleFileNameW(handle, module.as_mut_ptr(), module.len() as u32) as usize;
        }
    }

    // An access violation also says what it was doing and where.
    let access = if code == ACCESS_VIOLATION && (*record).number_parameters >= 2 {
        let operation = (*record).information[0];
        let target = (*record).information[1];
        Some((operation, target))
    } else {
        None
    };

    // The line is built from ASCII module names in a fixed buffer: no allocation, and no wide
    // conversion machinery that could itself fault.
    let mut ascii = [0u8; 96];
    let mut ascii_len = 0usize;
    for unit in &module[..module_len.min(module.len())] {
        if *unit < 128 && ascii_len < ascii.len() {
            ascii[ascii_len] = *unit as u8;
            ascii_len += 1;
        }
    }
    let name = std::str::from_utf8(&ascii[..ascii_len]).unwrap_or("");

    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    crate::logging::log_fault(
        ms,
        code,
        address,
        unsafe { GetCurrentThreadId() },
        base,
        address.wrapping_sub(base),
        name,
        access,
    );
    EXCEPTION_CONTINUE_SEARCH
}
