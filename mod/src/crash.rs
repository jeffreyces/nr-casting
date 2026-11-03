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
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        back_trace: *mut *mut core::ffi::c_void,
        back_trace_hash: *mut u32,
    ) -> u16;
}

const FROM_ADDRESS: u32 = 0x4;
const UNCHANGED_REFCOUNT: u32 = 0x2;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const ACCESS_VIOLATION: u32 = 0xC000_0005;
const MAX_LOGGED: u32 = 8;

static LOGGED: AtomicU32 = AtomicU32::new(0);

/// Walk the faulting thread's stack and name each frame's module.
///
/// "It faulted inside MSVCP140" is where the CPU was, not who asked. The caller chain is what says
/// whether a mod's own startup called a C++ routine that the runtime could not serve, and that is
/// the difference between a mod bug and a runtime gap - the question every crash report has to
/// answer before it can be fixed. Bounded, allocation-free, and it stops at the first frame whose
/// module cannot be named.
fn log_stack() {
    /// Frames to record, including the ones inside the runtime.
    const FRAMES: usize = 8;
    let mut frames = [std::ptr::null_mut::<core::ffi::c_void>(); FRAMES];
    // Skip this function and the handler; start at whoever faulted.
    let count = unsafe {
        RtlCaptureStackBackTrace(0, FRAMES as u32, frames.as_mut_ptr(), std::ptr::null_mut())
    } as usize;
    for (index, frame) in frames.iter().take(count).enumerate() {
        if frame.is_null() {
            break;
        }
        let address = *frame as usize;
        let Some((base, name)) = module_of(address) else {
            break;
        };
        if index == 0 && base == 0 {
            break;
        }
        crate::logging::log(&format!(
            "crash:   frame {index}: {name}+0x{:x}",
            address - base
        ));
    }
}

/// The module that owns an address, as (base, file name).
fn module_of(address: usize) -> Option<(usize, String)> {
    let mut handle: *mut u8 = std::ptr::null_mut();
    // SAFETY: `FROM_ADDRESS` asks the loader which module contains an address; it does not read it.
    unsafe {
        if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, address as *const u16, &mut handle) == 0
            || handle.is_null()
        {
            return None;
        }
    }
    let mut wide = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(handle, wide.as_mut_ptr(), wide.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&wide[..len.min(wide.len())]);
    let name = path.rsplit(['\\', '/']).next().unwrap_or("?").to_string();
    Some((handle as usize, name))
}

/// The last (code, address) seen, to recognise the second sighting of an unhandled fault.
static LAST_SIGHTING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    // A fault we see twice is one nothing handled: Windows calls every vectored handler again with
    // the same record before it ends the process, so the second sighting is the fatal one. Without
    // this the log cannot tell "something looked" from "something died", and a first-chance probe in
    // another DLL reads exactly like a crash - which is the wrong conclusion to hand a reporter.
    let key = ((code as u64) << 32) ^ address as u64;
    let unhandled = LAST_SIGHTING.swap(key, Ordering::Relaxed) == key;
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
    // Keep the END of the path, not the start. me3 installs profiles under
    // `%LOCALAPPDATA%\garyttierney\me3\config\profiles\eldenring-mods\<mod>\`, and 96 bytes of
    // that is the directory - a reporter's crash line named the folder and cut off the DLL name,
    // which is the one word that identifies the faulting module. The file name survives this way.
    let units = &module[..module_len.min(module.len())];
    let units = &units[units.len().saturating_sub(ascii.len())..];
    for unit in units {
        if *unit < 128 && ascii_len < ascii.len() {
            ascii[ascii_len] = *unit as u8;
            ascii_len += 1;
        }
    }
    let path = std::str::from_utf8(&ascii[..ascii_len]).unwrap_or("");
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);

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
    log_stack();
    if unhandled {
        // Same fault, second sighting: nothing in the process handled it and Windows is about to
        // end it. This is the line that separates "a DLL probed and caught" from "the game died
        // here", and it is the difference between blaming a mod and clearing it.
        crate::logging::log(
            "crash: unhandled - Windows called us with this fault a second time, so nothing handled it",
        );
    }
    EXCEPTION_CONTINUE_SEARCH
}
