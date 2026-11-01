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
static SETCURSOR_TRAMP: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
/// The panel's own window. Captures it takes must pass through: the sizing loop that
/// `DefWindowProcW` runs on a frame drag depends on having the capture, and swallowing it left the
/// panel unable to resize (and with no frame drawn) after dragging it small.
static PANEL_HWND: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
/// The game's window and its own window procedure, kept so ours can chain to it.
static GAME_HWND: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static GAME_WNDPROC: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
/// `GCLP_HCURSOR`: the index `SetClassLongPtrW` takes for the window class's cursor.
const GCLP_HCURSOR: i32 = -12;
/// The class cursor the game's window had before we replaced it, so closing the panel can put the
/// game's own (absent) cursor back.
static GAME_CLASS_CURSOR: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
/// Whether the "clip to the panel" line has been written.
static CLIP_LOGGED: AtomicBool = AtomicBool::new(false);
/// Whether the "cursor supplied for the game's window" line has been written.
static GAME_CURSOR_LOGGED: AtomicBool = AtomicBool::new(false);
/// Pushes made from the *game's* thread. `ShowCursor`'s counter lives in the calling thread's input
/// queue, so only that thread can bring the pointer back over the game's window.
static CURSOR_SHOWS_GAME: AtomicI32 = AtomicI32::new(0);
/// Set on close; the game-thread tick unwinds the counter from the thread that raised it.
static CURSOR_RESTORE: AtomicBool = AtomicBool::new(false);
/// Whether the "the pointer is visible now" line has been written for this open.
static CURSOR_REPORTED: AtomicBool = AtomicBool::new(false);
extern "system" {
    fn ClipCursor(rect: *const Rect) -> i32;
    fn GetWindowRect(hwnd: *mut u8, rect: *mut Rect) -> i32;
    fn ShowCursor(show: i32) -> i32;
    fn ReleaseCapture() -> i32;
    fn GetCapture() -> *mut u8;
    fn SetCursor(cursor: *mut u8) -> *mut u8;
    fn GetCursor() -> *mut u8;
    fn LoadCursorW(instance: *mut u8, name: *const u16) -> *mut u8;
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
    fn GetProcAddress(module: *mut u8, name: *const u8) -> *mut u8;
    fn SetWindowLongPtrW(hwnd: *mut u8, index: i32, value: isize) -> isize;
    fn SetClassLongPtrW(hwnd: *mut u8, index: i32, value: isize) -> isize;
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// `GWL_WNDPROC`: the index `SetWindowLongPtrW` takes for a window procedure.
const GWLP_WNDPROC: i32 = -4;
const WM_SETCURSOR: u32 = 0x0020;
/// `WM_SETCURSOR`'s lparam low word: the pointer is over the client area rather than a frame.
const HTCLIENT: u32 = 1;

/// Tell the pointer layer which window is the panel's.
pub fn set_panel_window(hwnd: usize) {
    PANEL_HWND.store(hwnd as *mut (), Ordering::Release);
}

/// Take over `WM_SETCURSOR` for the game's window so the panel can supply a pointer for it.
///
/// ELDEN RING's window class has no cursor at all, so there is nothing for the window manager to
/// draw over the game while the panel is open — `GetCursorInfo` reports the cursor as *showing*
/// and still nothing appears, which is what the log said. A window's cursor comes from its own
/// `WM_SETCURSOR` handling, and the only way to add one to a window in this process that we do not
/// own is to subclass it and chain to its original procedure.
pub fn adopt_game_window(hwnd: usize) {
    if hwnd == 0 || !GAME_HWND.load(Ordering::Acquire).is_null() {
        return;
    }
    unsafe {
        let previous = SetWindowLongPtrW(
            hwnd as *mut u8,
            GWLP_WNDPROC,
            game_wndproc as *const () as isize,
        );
        if previous == 0 {
            crate::log("pointer: could not subclass the game's window for the cursor");
            return;
        }
        GAME_WNDPROC.store(previous as *mut (), Ordering::Release);
        GAME_HWND.store(hwnd as *mut (), Ordering::Release);
        // A cursor of its own is the lever that survives Wine's own cursor handling: with a class
        // cursor set, the window manager's own enter-window path supplies it, no matter whether the
        // game ever calls `DefWindowProc`, and no matter what the game's `SetCursor` asked for.
        set_class_cursor(true);
        crate::log("pointer: game window adopted — its client gets the arrow while the panel is open");
    }
}

/// Our procedure for the game's window: supply a cursor for the client area, then chain.
unsafe extern "system" fn game_wndproc(hwnd: *mut u8, msg: u32, wparam: usize, lparam: isize) -> isize {
    if msg == WM_SETCURSOR && is_open() && (lparam & 0xFFFF) as u32 == HTCLIENT {
        let arrow = arrow();
        if !arrow.is_null() {
            let _ = SetCursor(arrow);
            if !GAME_CURSOR_LOGGED.swap(true, Ordering::Relaxed) {
                crate::log("pointer: arrow supplied for the game's window while the panel is open");
            }
            // Handled: a chained DefWindowProc would clear it again, because the class has no
            // cursor of its own.
            return 1;
        }
    }
    let original = GAME_WNDPROC.load(Ordering::Acquire);
    if original.is_null() {
        return 0;
    }
    let chained: unsafe extern "system" fn(*mut u8, u32, usize, isize) -> isize =
        std::mem::transmute(original);
    chained(hwnd, msg, wparam, lparam)
}

/// Give the game's window class an arrow, or put back the (absent) cursor it had.
///
/// `WM_SETCURSOR` only reaches us if the game's own window procedure chains, which it does not have
/// to; the class cursor is consulted by the window manager itself.
fn set_class_cursor(on: bool) {
    let hwnd = GAME_HWND.load(Ordering::Acquire) as *mut u8;
    if hwnd.is_null() {
        return;
    }
    unsafe {
        if on {
            let arrow = arrow();
            if arrow.is_null() {
                return;
            }
            let previous = SetClassLongPtrW(hwnd, GCLP_HCURSOR, arrow as isize);
            if previous != 0 && GAME_CLASS_CURSOR.load(Ordering::Acquire).is_null() {
                GAME_CLASS_CURSOR.store(previous as *mut (), Ordering::Release);
                crate::log("pointer: game window class cursor set to the arrow");
            }
        } else {
            let previous = GAME_CLASS_CURSOR.load(Ordering::Acquire);
            if !previous.is_null() {
                SetClassLongPtrW(hwnd, GCLP_HCURSOR, previous as isize);
            }
        }
    }
}

fn panel_window() -> *mut u8 {
    PANEL_HWND.load(Ordering::Acquire) as *mut u8
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
    if crate::bisect::disabled(crate::bisect::bit::POINTER) {
        return original(cursor);
    }
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
    if !crate::bisect::disabled(crate::bisect::bit::POINTER) && is_open() {
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
    if !crate::bisect::disabled(crate::bisect::bit::POINTER) && is_open() {
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
    // The panel's own capture is the window manager's, not the game's: the sizing loop behind a
    // frame drag needs it, and a swallowed one is what left the panel with no border and no way to
    // resize after it had been dragged small.
    if !crate::bisect::disabled(crate::bisect::bit::POINTER) && is_open() && hwnd != panel_window() {
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

pub(crate) fn hook(
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

/// The cursor detours, kept alive so they can be switched off again.
///
/// `hook` hands its `RawDetour` to the process (it is needed for its lifetime), which means the
/// patch cannot be removed - and these four functions exist only to keep the cursor usable while the
/// panel is open. A session that opens the panel once paid for them until it exited. Disabling a
/// detour restores the original bytes, so the game calls the real function with nothing of ours in
/// the path at all; that is the only way to make "not needed" actually mean "not there".
static CURSOR: std::sync::Mutex<Vec<retour::RawDetour>> = std::sync::Mutex::new(Vec::new());
static CURSOR_ENGAGED: AtomicBool = AtomicBool::new(false);

/// Engaged while the panel is open, disengaged while it is closed. Idempotent.
pub fn engage_cursor(on: bool) {
    if CURSOR_ENGAGED.swap(on, Ordering::Relaxed) == on {
        return;
    }
    let guard = CURSOR.lock().unwrap_or_else(|e| e.into_inner());
    for raw in guard.iter() {
        // SAFETY: the detour records we keep are all installed and owned by this module; a toggle
        // writes the function's prologue back and forth. The panel thread does this while the game
        // runs, which is safe because the original bytes are restored before the call returns.
        let outcome = unsafe { if on { raw.enable() } else { raw.disable() } };
        if let Err(e) = outcome {
            crate::log(&format!(
                "pointer: could not {} the cursor hooks: {e:?}",
                if on { "engage" } else { "disengage" }
            ));
        }
    }
    crate::log(&format!(
        "pointer: cursor hooks {} ({} of them)",
        if on { "engaged" } else { "disengaged" },
        guard.len()
    ));
}

/// Whether the cursor hooks are currently in the path.
pub fn cursor_engaged() -> bool {
    CURSOR_ENGAGED.load(Ordering::Relaxed)
}

/// A detour whose patch can be undone, recorded so it can be toggled later.
fn hook_switchable(
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
        CURSOR.lock().unwrap_or_else(|e| e.into_inner()).push(raw);
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
        // The four cursor hooks exist for the panel and are switched off when it closes; the two
        // raw-input hooks below are the mouse casting path and stay in place.
        hook_switchable(c"ClipCursor", user32, clip_detour as *const (), &CLIP_TRAMP)
            .and_then(|_| {
                hook_switchable(c"SetCursorPos", user32, pos_detour as *const (), &POS_TRAMP)
            })
            .and_then(|_| {
                hook_switchable(c"SetCursor", user32, set_cursor_detour as *const (), &SETCURSOR_TRAMP)
            })
            .and_then(|_| {
                hook_switchable(c"SetCapture", user32, capture_detour as *const (), &CAPTURE_TRAMP)
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

/// Push `ShowCursor(TRUE)` from the calling thread. Returns how many pushes were spent.
unsafe fn push_visible(cap: i32) -> i32 {
    let mut pushes = 0;
    while pushes < cap && !crate::input::game_shows_cursor() {
        ShowCursor(1);
        pushes += 1;
    }
    pushes
}

/// Pushes allowed for one panel session. Far past anything the game hides.
const MAX_CURSOR_PUSHES: i32 = 400;

/// Open or close the panel's claim on the pointer.
pub fn set_open(open: bool) {
    if open == is_open() {
        return;
    }
    OPEN.store(open, Ordering::Relaxed);
    unsafe {
        if open {
            set_class_cursor(true);
            confine_to_panel();
            CURSOR_REPORTED.store(false, Ordering::Relaxed);
            let visible = crate::input::game_shows_cursor();
            let shape = GetCursor();
            // The display counter belongs to the game's thread, so the panel's own thread cannot
            // raise it: `game_thread_tick` does that from the pad poll, and logs when it lands.
            crate::log(&format!(
                "pointer: panel open (the game's clip is off; the frame loop clips to the panel, \
                 system reports visible={visible} shape_set={} — the game's thread raises the counter)",
                !shape.is_null()
            ));
        } else {
            // Unwind on the thread that raised the counter, not this one.
            CURSOR_RESTORE.store(true, Ordering::Relaxed);
            ClipCursor(std::ptr::null());
            set_class_cursor(false);
            crate::log("pointer: panel closed, game has the pointer again");
        }
    }
}

/// Keep the pointer inside the panel window while the panel is open.
///
/// Clamped to the screen by Windows itself, so a panel hanging off the edge still gets a usable
/// rectangle rather than a rejected clip.
fn confine_to_panel() {
    unsafe {
        let panel = panel_window();
        if panel.is_null() {
            return;
        }
        let mut rect = Rect::default();
        if GetWindowRect(panel, &mut rect) == 0 || rect.right <= rect.left || rect.bottom <= rect.top {
            return;
        }
        let clipped = ClipCursor(&rect);
        if !CLIP_LOGGED.swap(true, Ordering::Relaxed) {
            crate::log(if clipped != 0 {
                "pointer: pointer clipped to the panel"
            } else {
                "pointer: ClipCursor refused — the pointer can leave the panel, and the game hides it there"
            });
        }
    }
}

/// Per-frame upkeep while the panel is open.
pub fn frame() {
    if !is_open() {
        return;
    }
    unsafe {
        // The game's window has no cursor of its own and hides the OS pointer for mouse-look, and
        // nothing we do from this side makes it reappear over the game's client area under Wine.
        // Confining the pointer to the panel — which does draw a cursor — keeps it visible where
        // the player actually needs it.
        confine_to_panel();
        // Wine re-centres a captured pointer every frame, so the game must not keep its capture.
        // The panel's own capture is the window manager's sizing/moving loop — never touch it.
        let capture = GetCapture();
        if capture.is_null() || capture != panel_window() {
            ReleaseCapture();
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

/// The once-per-poll hook that runs on the game's own thread (the pad poll).
///
/// `ShowCursor`'s display counter is stored in the calling thread's input queue: pushes from the
/// panel's thread raised its own counter and nothing else, which is why the pointer stayed
/// invisible over the game while 400 pushes were logged. The pad poll runs on the game's thread, so
/// this is where the counter can actually be raised — and where it is unwound when the panel closes,
/// because a counter can only be given back by the thread that took it.
pub fn game_thread_tick() {
    if CURSOR_RESTORE.swap(false, Ordering::Relaxed) {
        let pushed = CURSOR_SHOWS_GAME.swap(0, Ordering::Relaxed);
        unsafe {
            for _ in 0..pushed {
                ShowCursor(0);
            }
        }
        return;
    }
    if !is_open() {
        return;
    }
    if CURSOR_SHOWS_GAME.load(Ordering::Relaxed) >= MAX_CURSOR_PUSHES {
        return;
    }
    if crate::input::game_shows_cursor() {
        if !CURSOR_REPORTED.swap(true, Ordering::Relaxed) {
            crate::log(&format!(
                "pointer: cursor visible after {} pushes from the game's thread",
                CURSOR_SHOWS_GAME.load(Ordering::Relaxed)
            ));
        }
        return;
    }
    // A couple per poll: the pad poll runs at up to 125 Hz, so a deeply hidden cursor is caught up
    // within a few frames without spending a long loop on the game's input thread.
    let pushed = unsafe { push_visible(2) };
    if pushed > 0 {
        CURSOR_SHOWS_GAME.fetch_add(pushed, Ordering::Relaxed);
    }
}
