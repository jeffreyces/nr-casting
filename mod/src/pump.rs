//! The frame heartbeat.
//!
//! Every frame measurement in this mod hangs off a call the game makes once per frame. With a
//! controller attached that call is the pad poll, which is also what makes casting work. Without one
//! there is no pad poll at all (ER only reads XInput when a pad was present at launch), and the
//! keyboard is not a substitute: the game calls `GetAsyncKeyState` when it wants one key's state, not
//! every frame - 24 times in a seven-minute session, which made a "worst gap" the time between two
//! keystrokes.
//!
//! `PeekMessageW` is called by the game's own loop once per frame, in the world and in every menu,
//! and it is nowhere near the graphics path (this mod does not hook Present, deliberately). It is
//! only ever used to timestamp: the call is passed through untouched.

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

static TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static INSTALLED: AtomicBool = AtomicBool::new(false);
/// A thread whose pump calls are not the game's (the panel's own message loop).
static IGNORE_THREAD: AtomicU32 = AtomicU32::new(0);

extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
    fn GetCurrentThreadId() -> u32;
}

/// Hook the pump. Failing to install is not an error worth stopping for: the pad poll still exists,
/// and a run without a controller then says why it has no data.
pub fn install() -> Result<(), String> {
    unsafe {
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr() as *const u8);
        if user32.is_null() {
            return Err("user32 not loaded".into());
        }
        crate::pointer::hook(c"PeekMessageW", user32, peek_detour as *const (), &TRAMPOLINE)?;
        INSTALLED.store(true, Ordering::Relaxed);
        Ok(())
    }
}

pub fn installed() -> bool {
    INSTALLED.load(Ordering::Relaxed)
}

/// Stop counting the calling thread's pump calls. Called from the panel's own thread.
pub fn ignore_current_thread() {
    unsafe { IGNORE_THREAD.store(GetCurrentThreadId(), Ordering::Relaxed) };
}

unsafe extern "system" fn peek_detour(
    msg: *mut u8,
    hwnd: *mut u8,
    min: u32,
    max: u32,
    remove: u32,
) -> i32 {
    let started = crate::perf::ticks();
    let tramp = TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: unsafe extern "system" fn(*mut u8, *mut u8, u32, u32, u32) -> i32 =
        std::mem::transmute(tramp);
    let out = original(msg, hwnd, min, max, remove);
    // The panel pumps its own messages on its own thread; those ticks say nothing about the game's
    // frame cadence and would flatten the series while the panel is open.
    if GetCurrentThreadId() != IGNORE_THREAD.load(Ordering::Relaxed) {
        crate::perf::report_pump(started);
    }
    out
}
