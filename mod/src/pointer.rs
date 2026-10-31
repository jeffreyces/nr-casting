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
//! | `SetCapture`                  | swallowed; `ReleaseCapture` every frame       |
//! | `GetRawInputData[Buffer]`     | empty for the game's threads (kills mouse-look)|
//! | `ShowCursor`                  | pushed visible on open, unwound exactly on close|
//!
//! Raw-input starvation spares the panel's render thread: ImGui feeds its cursor from raw
//! input too, and starving every thread froze the panel's own pointer.
//!
//! The capture handling is aimed at Wine: a captured pointer is re-centred by the X11
//! driver every frame, which pins it to the screen centre. On native Windows it is
//! harmless but unnecessary.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};

static OPEN: AtomicBool = AtomicBool::new(false);
static CLIP_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static POS_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static CAPTURE_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static RAW_DATA_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static RAW_BUFFER_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
/// `ShowCursor` is a counter; remember how far we pushed it to unwind the same amount.
static CURSOR_SHOWS: AtomicI32 = AtomicI32::new(0);
static SETCURSOR_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
extern "system" {
    fn ClipCursor(rect: *const u8) -> i32;
    fn ShowCursor(show: i32) -> i32;
    fn ReleaseCapture() -> i32;
    fn SetCursor(cursor: *mut u8) -> *mut u8;
    fn GetCursor() -> *mut u8;
    fn LoadCursorW(instance: *mut u8, name: *const u16) -> *mut u8;
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
    fn GetProcAddress(module: *mut u8, name: *const u8) -> *mut u8;
}

/// `IDC_ARROW`, as `MAKEINTRESOURCE` spells it: the integer 32512 cast to a string pointer.
const IDC_ARROW: usize = 32512;
/// The arrow, loaded once. Kept forever: a cursor handle from `LoadCursorW(NULL, ..)` is a shared
/// system resource and never needs a release.
static ARROW: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

fn arrow() -> *mut u8 {
    let cached = ARROW.load(Ordering::Acquire);
    if !cached.is_null() {
        return cached as *mut u8;
    }
    let loaded = unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16) };
    if !loaded.is_null() {
        ARROW.store(loaded as *mut (), Ordering::Release);
    }
    loaded
}

/// While the panel is open, hand the game the arrow instead of the null cursor it asks for.
///
/// ELDEN RING's window class has no cursor, so in gameplay every `WM_SETCURSOR` resolves to
/// `SetCursor(NULL)` and the pointer is simply not drawn. That is why the mouse was invisible over
/// the game and appeared the moment it crossed the panel, whose own class sets the arrow: the
/// display counter was never the problem (`ShowCursor` was pushed visible on open). Only a null
/// cursor is replaced — the window's own resize borders still get their sizing cursors.
unsafe extern "system" fn set_cursor_detour(cursor: *mut u8) -> *mut u8 {
    let tramp = SETCURSOR_TRAMP.load(Ordering::Acquire);
    if tramp.is_null() {
        return std::ptr::null_mut();
    }
    let original: unsafe extern "system" fn(*mut u8) -> *mut u8 = std::mem::transmute(tramp);
    if cursor.is_null() && is_open() {
        let arrow = arrow();
        if !arrow.is_null() {
            return original(arrow);
        }
    }
    original(cursor)
}

fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// Starve raw mouse input while the panel owns the pointer.
///
/// Every thread, deliberately: the panel reads its cursor from window messages, not raw input,
/// so nothing in this process needs the mouse raw while it is open.
fn starving() -> bool {
    is_open()
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

/// Is `size` (a byte count the caller passed in) at least `needed` bytes?
///
/// These detours run inside another module's call, so the buffer behind the pointer is only as
/// trustworthy as the caller. Reading the header of a zero-length or null buffer is an access
/// violation in someone else's process — a crash with our name on it.
unsafe fn buffer_at_least(size: *const u32, needed: u32) -> bool {
    !size.is_null() && unsafe { *size } >= needed
}

unsafe extern "system" fn raw_data_detour(
    device: *mut u8,
    command: u32,
    data: *mut u8,
    size: *mut u32,
    header_size: u32,
) -> u32 {
    if starving()
        && !data.is_null()
        && command == RID_INPUT
        && buffer_at_least(size, 4)
        && mouse_payload(data)
    {
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
    // header's `dwType` sits four bytes in — but only if there is an entry at all.
    if starving()
        && !data.is_null()
        && buffer_at_least(size, 8)
        && unsafe { *((data as *const u32).add(1)) } == RIM_TYPE_MOUSE
    {
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
                hook(c"SetCursor", user32, set_cursor_detour as *const (), &SETCURSOR_TRAMP)
            })
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

/// Push `ShowCursor(TRUE)` until the system reports a visible pointer, and say how many that took.
///
/// The display counter ER left behind was tens of points below zero (16 pushes were not enough),
/// and `GetCursorInfo` is the only honest read on the result — `ShowCursor`'s return value is the
/// new count, which on this host disagreed with the desktop state.
unsafe fn force_cursor_visible(cap: i32) -> i32 {
    let mut pushes = 0;
    while pushes < cap && !crate::input::game_shows_cursor() {
        ShowCursor(1);
        pushes += 1;
    }
    pushes
}

/// Pushes allowed for one panel session. Far past anything the game hides; a cap at all so a
/// system that refuses to report the cursor visible cannot inflate the counter forever.
const MAX_CURSOR_PUSHES: i32 = 400;

/// Open or close the panel's claim on the pointer.
pub fn set_open(open: bool) {
    if open == is_open() {
        return;
    }
    OPEN.store(open, Ordering::Relaxed);
    unsafe {
        if open {
            // ER has hidden the cursor; each ShowCursor(TRUE) cancels one hide.
            let shows = force_cursor_visible(MAX_CURSOR_PUSHES);
            CURSOR_SHOWS.store(shows, Ordering::Relaxed);
            ClipCursor(std::ptr::null());
            let visible = crate::input::game_shows_cursor();
            let shape = GetCursor();
            crate::log(&format!(
                "pointer: panel open (cursor shown after {shows} calls, clip released, \
                 system reports visible={visible} shape_set={})",
                !shape.is_null()
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
        // The game hides the pointer again as it re-enters play; undo that while the panel owns it,
        // remembering every push so closing puts the counter back where the game left it.
        if !crate::input::game_shows_cursor() {
            let pushed = CURSOR_SHOWS.load(Ordering::Relaxed);
            if pushed < MAX_CURSOR_PUSHES {
                let more = force_cursor_visible(MAX_CURSOR_PUSHES - pushed);
                CURSOR_SHOWS.fetch_add(more, Ordering::Relaxed);
            }
        }
        // Nothing has set a cursor shape — the game's window class has none — so the pointer would
        // be the invisible one. Only fill in an empty shape: over our own resize borders a sizing
        // cursor is set and must stay.
        if GetCursor().is_null() {
            let arrow = arrow();
            if !arrow.is_null() {
                let _ = SetCursor(arrow);
            }
        }
    }
}
