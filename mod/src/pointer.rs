//! Pointer ownership while the panel is open.
//!
//! ER takes the mouse for granted: it clips the pointer, re-centres it every frame, calls
//! `SetCapture`, and reads mouse-look from raw input. Left alone, an ImGui panel renders
//! but cannot be used — the pointer never travels and the camera turns with it.
//!
//! Four small interventions, each aimed at one of those behaviours:
//!
//! | game call                     | while open                                    |
//! |-------------------------------|-----------------------------------------------|
//! | `ClipCursor` / `SetCursorPos` | swallowed (no clipping, no re-centring)       |
//! | `SetCapture`                  | swallowed; `ReleaseCapture` each frame        |
//! | `GetRawInputData[Buffer]`     | empty for the game's threads (kills mouse-look)|
//! | `ShowCursor`                  | pushed visible on open, unwound exactly on close|
//!
//! Raw-input starvation spares the panel's render thread: ImGui feeds its cursor from raw
//! input too, and starving every thread froze the panel's own pointer.
//!
//! The capture handling is aimed at Wine: a captured pointer is re-centred by the X11
//! driver every frame, which pins it to the screen centre. On native Windows it is
//! harmless but unnecessary.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, Ordering};

static OPEN: AtomicBool = AtomicBool::new(false);
static CLIP_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static POS_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static CAPTURE_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static RAW_DATA_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static RAW_BUFFER_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
/// `ShowCursor` is a counter; remember how far we pushed it to unwind the same amount.
static CURSOR_SHOWS: AtomicI32 = AtomicI32::new(0);
/// Thread the panel renders (and reads raw input for ImGui) on.
static RENDER_THREAD: AtomicU32 = AtomicU32::new(0);

extern "system" {
    fn ClipCursor(rect: *const u8) -> i32;
    fn ShowCursor(show: i32) -> i32;
    fn ReleaseCapture() -> i32;
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
    fn GetProcAddress(module: *mut u8, name: *const u8) -> *mut u8;
    fn GetCurrentThreadId() -> u32;
}

fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Starve every raw-input reader except the panel's render thread.
fn starving() -> bool {
    is_open() && unsafe { GetCurrentThreadId() } != RENDER_THREAD.load(Ordering::Relaxed)
}

unsafe extern "system" fn clip_detour(rect: *const u8) -> i32 {
    if is_open() {
        return 1;
    }
    let tramp = CLIP_TRAMP.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: unsafe extern "system" fn(*const u8) -> i32 = std::mem::transmute(tramp);
    original(rect)
}

unsafe extern "system" fn pos_detour(x: i32, y: i32) -> i32 {
    if is_open() {
        return 1;
    }
    let tramp = POS_TRAMP.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: unsafe extern "system" fn(i32, i32) -> i32 = std::mem::transmute(tramp);
    original(x, y)
}

unsafe extern "system" fn capture_detour(hwnd: *mut u8) -> *mut u8 {
    if is_open() {
        return std::ptr::null_mut();
    }
    let tramp = CAPTURE_TRAMP.load(Ordering::Acquire);
    if tramp.is_null() {
        return std::ptr::null_mut();
    }
    let original: unsafe extern "system" fn(*mut u8) -> *mut u8 = std::mem::transmute(tramp);
    original(hwnd)
}

/// `RID_INPUT` is the only command whose payload starts with a `RAWINPUT` header, and mouse is
/// type 0. Starvation exists to stop mouse-look, so keyboard is never dropped: a stuck panel
/// flag must not be able to take the keyboard away from the game.
const RID_INPUT: u32 = 0x1000_0003;
const RIM_TYPE_MOUSE: u32 = 0;

unsafe fn mouse_payload(data: *const u8) -> bool {
    unsafe { *(data as *const u32) == RIM_TYPE_MOUSE }
}

unsafe extern "system" fn raw_data_detour(
    device: *mut u8,
    command: u32,
    data: *mut u8,
    size: *mut u32,
    header_size: u32,
) -> u32 {
    if starving() && !data.is_null() && command == RID_INPUT && mouse_payload(data) {
        return 0;
    }
    let tramp = RAW_DATA_TRAMP.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: unsafe extern "system" fn(*mut u8, u32, *mut u8, *mut u32, u32) -> u32 =
        std::mem::transmute(tramp);
    original(device, command, data, size, header_size)
}

unsafe extern "system" fn raw_buffer_detour(data: *mut u8, size: *mut u32, header: u32) -> u32 {
    // Each entry in the buffer is a `u32` byte count followed by the `RAWINPUT`, so the first
    // header's `dwType` sits four bytes in.
    if starving() && !data.is_null() && unsafe { *((data as *const u32).add(1)) } == RIM_TYPE_MOUSE {
        return 0;
    }
    let tramp = RAW_BUFFER_TRAMP.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: unsafe extern "system" fn(*mut u8, *mut u32, u32) -> u32 = std::mem::transmute(tramp);
    original(data, size, header)
}

fn hook(
    name: &std::ffi::CStr,
    module: *mut u8,
    detour: *const (),
    slot: &'static AtomicPtr<()>,
) -> Result<(), String> {
    unsafe {
        let address = GetProcAddress(module, name.as_ptr() as *const u8);
        if address.is_null() {
            return Err(format!("{} not found", name.to_string_lossy()));
        }
        let raw = retour::RawDetour::new(address as *const (), detour)
            .map_err(|e| format!("{} detour: {e:?}", name.to_string_lossy()))?;
        raw.enable()
            .map_err(|e| format!("{} enable: {e:?}", name.to_string_lossy()))?;
        slot.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
        std::mem::forget(raw); // process-lifetime state
        Ok(())
    }
}

/// Install the pointer detours.
pub fn install() -> Result<(), String> {
    unsafe {
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr() as *const u8);
        if user32.is_null() {
            return Err("user32 not loaded".into());
        }
        hook(c"ClipCursor", user32, clip_detour as *const (), &CLIP_TRAMP)
            .and_then(|_| hook(c"SetCursorPos", user32, pos_detour as *const (), &POS_TRAMP))
            .and_then(|_| {
                hook(c"SetCapture", user32, capture_detour as *const (), &CAPTURE_TRAMP)
            })
            .and_then(|_| {
                hook(c"GetRawInputData", user32, raw_data_detour as *const (), &RAW_DATA_TRAMP)
            })
            .and_then(|_| {
                hook(
                    c"GetRawInputBuffer",
                    user32,
                    raw_buffer_detour as *const (),
                    &RAW_BUFFER_TRAMP,
                )
            })
    }
}

/// Record the calling thread as the panel render thread. Called every frame.
pub fn note_render_thread() {
    RENDER_THREAD.store(unsafe { GetCurrentThreadId() }, Ordering::Relaxed);
}

/// Open or close the panel's claim on the pointer.
pub fn set_open(open: bool) {
    if open == is_open() {
        return;
    }
    OPEN.store(open, Ordering::Relaxed);
    unsafe {
        if open {
            // ER has hidden the cursor; each ShowCursor(TRUE) cancels one hide.
            let mut shows = 0;
            while ShowCursor(1) >= 0 && shows < 16 {
                shows += 1;
            }
            CURSOR_SHOWS.store(shows, Ordering::Relaxed);
            ClipCursor(std::ptr::null());
            crate::log(&format!(
                "pointer: panel open (cursor shown after {shows} calls, clip released)"
            ));
        } else {
            let shows = CURSOR_SHOWS.swap(0, Ordering::Relaxed);
            for _ in 0..shows {
                ShowCursor(0);
            }
            crate::log("pointer: panel closed, game has the pointer again");
        }
    }
}

/// Per-frame upkeep while the panel is open.
pub fn frame() {
    if !is_open() {
        return;
    }
    unsafe {
        ClipCursor(std::ptr::null());
        // Wine re-centres a captured pointer every frame; drop the capture each tick.
        ReleaseCapture();
    }
}
