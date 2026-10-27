//! The keyboard and mouse path: raw input, and the Shift the game must not see.
//!
//! ELDEN RING reads the mouse through raw input (`GetRawInputData`), not through
//! `GetAsyncKeyState` and not through XInput. With a controller connected the mod synthesises a
//! bumper press in the pad report and the game casts from that; with no controller there is no pad
//! report to rewrite (the game stops polling XInput entirely), so the click itself has to carry
//! the cast.
//!
//! That gives this module two jobs, both on the same records: point the selected spell at the slot
//! the click means before the game processes it, and hide Shift from the game, which would
//! otherwise read Shift + click as its own strong attack. The record layout lives in
//! `nr_casting_core` so it can be tested on the host.

use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, GetAsyncKeyState, KEYEVENTF_KEYUP};

use nr_casting_core::{
    is_shift_vkey, raw_hide_shift, raw_kind, raw_keyboard, raw_mouse_buttons, raw_set_mouse_buttons,
    raw_size, RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, RI_MOUSE_RIGHT_BUTTON_DOWN,
    RI_MOUSE_RIGHT_BUTTON_UP, RAW_TYPE_KEYBOARD, RAW_TYPE_MOUSE,
};

use crate::logging::debug;
use crate::stats;

extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
    fn GetProcAddress(module: *mut u8, name: *const u8) -> *mut u8;
}

type RawInputDataFn = unsafe extern "system" fn(*mut u8, u32, *mut u8, *mut u32, u32) -> isize;
type RawInputBufferFn = unsafe extern "system" fn(*mut u8, *mut u32, u32) -> u32;

/// `GetRawInputData(hRawInput, RID_INPUT, ...)`.
const RID_INPUT: u32 = 0x1000_0003;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;
const VK_MENU: i32 = 0x12;

static KEY_TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static RAW_DATA_TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static RAW_BUFFER_TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// When a pad poll last succeeded; the two input paths must never steer the same click.
static LAST_PAD_OK: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Set while our own code asks for key state, so the mod's probes are not counted as the
    /// game's — and are never answered with a hidden Shift.
    static OURS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Run `f` with our own key-state reads excluded from the game's counters.
pub(crate) fn ours<R>(f: impl FnOnce() -> R) -> R {
    OURS.with(|ours| {
        let previous = ours.replace(true);
        let out = f();
        ours.set(previous);
        out
    })
}

/// Called by the pad detour whenever a poll succeeds.
pub(crate) fn note_pad_ok() {
    LAST_PAD_OK.store(crate::millis(), Ordering::Relaxed);
}

fn key_down(vk: i32) -> bool {
    ours(|| unsafe { GetAsyncKeyState(vk) }) < 0
}

/// The class port's own keyboard slots: 4 and 5, which its Y+R1 / Y+L1 chords map to.
const RECLUSE_KEY_4: u8 = 0x34;
const RECLUSE_KEY_5: u8 = 0x35;

/// The bind gesture's modifier on keyboard and mouse: the Windows/Super key, the counterpart of
/// the pad's L3. Under Proton it is the same physical key (Super), so one spelling covers both.
const VK_LWIN: i32 = 0x5B;
const VK_RWIN: i32 = 0x5C;

/// Bind the game's selected spell to the slot this click means.
///
/// Held Super + click is the keyboard/mouse counterpart of L3 + R1/R2/L1/L2: it assigns to the
/// same slot the bare click would cast from, and the click bits are cleared from the record so
/// the press never also attacks. Deliberately checked before `pad_alive()`: a player with a
/// controller plugged in but a mouse in their hand still needs a way to bind.
/// Returns whether the record was consumed.
unsafe fn bind_gesture(record: &mut [u8]) -> bool {
    // Buttons first, and before the modifier lookup: this runs for *every* mouse record,
    // movement included, and each `key_down` is a `GetAsyncKeyState` call — a wineserver
    // round-trip under Wine. Asking about Super before knowing there is a click to bind turned
    // an idle mouse into two round-trips per record.
    let Some(buttons) = raw_mouse_buttons(record) else {
        return false;
    };
    let left = buttons & RI_MOUSE_LEFT_BUTTON_DOWN != 0;
    let right = buttons & RI_MOUSE_RIGHT_BUTTON_DOWN != 0;
    if (!left && !right) || (left && right) {
        return false;
    }
    if !key_down(VK_LWIN) && !key_down(VK_RWIN) {
        return false;
    }
    // `true` for the fourth slot: binding is our own data, so an Alt chord is bindable even on a
    // character where the cast chord belongs to a class port.
    let Some((right_hand, slot, which)) = nr_casting_core::mouse_route(
        left,
        right,
        key_down(VK_SHIFT),
        key_down(VK_CONTROL),
        key_down(VK_MENU),
        true,
    ) else {
        return false;
    };
    if !crate::input::bind_now(right_hand, slot, which) {
        return false;
    }
    let remaining = buttons & !(RI_MOUSE_LEFT_BUTTON_DOWN | RI_MOUSE_RIGHT_BUTTON_DOWN);
    raw_set_mouse_buttons(record, remaining);
    stats::bound(which);
    true
}

/// Hand an Alt chord to the class port when the fourth slot is not ours.
///
/// Letting the click through is not enough: the game ignores Alt, so it would perform a plain
/// attack instead, and the port only listens for its own keys. The click is therefore swallowed
/// (so the game does not also attack) and the port's key is pressed in its place — 4 for the
/// right hand, 5 for the left. Returns whether the record was consumed.
unsafe fn hand_off_to_recluse(record: &mut [u8]) -> bool {
    if crate::input::y_bumper() {
        return false;
    }
    // Buttons before the modifier: same reasoning as `bind_gesture` — this runs on every mouse
    // record, and `key_down` is a `GetAsyncKeyState` call.
    let Some(buttons) = raw_mouse_buttons(record) else {
        return false;
    };
    if !key_down(VK_MENU) {
        return false;
    }
    let mut remaining = buttons;
    let mut press: Option<(u8, bool)> = None;
    for (down, up, key) in [
        (RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, RECLUSE_KEY_4),
        (RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, RECLUSE_KEY_5),
    ] {
        if buttons & down != 0 {
            remaining &= !down;
            press = Some((key, false));
        }
        if buttons & up != 0 {
            remaining &= !up;
            press = Some((key, true));
        }
    }
    let Some((key, released)) = press else {
        return false;
    };
    if !raw_set_mouse_buttons(record, remaining) {
        return false;
    }
    keybd_event(key, 0, if released { KEYEVENTF_KEYUP } else { Default::default() }, 0);
    stats::handoff();
    true
}

/// Steer the selected spell for a click the game is about to process.
///
/// The game casts from its own click once the magic entry points at the right spell, so this is
/// the whole job: work out which hand and slot the click means, steer, and let the click through.
unsafe fn steer_click(record: &[u8]) {
    let Some(buttons) = raw_mouse_buttons(record) else {
        return;
    };
    let left = buttons & RI_MOUSE_LEFT_BUTTON_DOWN != 0;
    let right = buttons & RI_MOUSE_RIGHT_BUTTON_DOWN != 0;
    // Buttons first: most records are movement only, and the pad check costs a clock read.
    if !left && !right {
        return;
    }
    // Mouse and keyboard keep their own layer, with or without a controller attached: the mod
    // steers the selected spell and the game's own click performs the cast, exactly as it does
    // with no pad. Nothing is synthesised and the click is never altered, so there is nothing to
    // fight over — one click is one action. (v0.2.2 routed mouse input through the pad report
    // when a controller was live, which is what made a click an attack *and* a cast, and left
    // click timing at the mercy of the pad poll.)
    if crate::input::game_shows_cursor() {
        // The game is showing its cursor: a menu, not the world. Clicks there are the UI's.
        return;
    }
    let shift = key_down(VK_SHIFT);
    let ctrl = key_down(VK_CONTROL);
    let alt = key_down(VK_MENU);
    let route = nr_casting_core::mouse_route(left, right, shift, ctrl, alt, crate::input::y_bumper());
    debug!(
        "mouse: l={left} r={right} shift={shift} ctrl={ctrl} alt={alt} -> {route:?}"
    );
    if let Some((right_hand, slot, which)) = route {
        if crate::input::steer_mouse(right_hand, slot) {
            stats::steered(which, slot);
        }
    }
}

unsafe extern "system" fn key_detour(vk: i32) -> i16 {
    let tramp = KEY_TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    if !OURS.with(|ours| ours.get()) {
        // Shift is the mouse slots' modifier, and the game reads Shift + click as its strong
        // attack — which does not cast. Hidden unconditionally now that the mouse layer owns
        // mouse input whether or not a controller is attached (ER keeps its mouse bindings live
        // with a pad plugged in, which is how Shift + click was arriving as an attack).
        if is_shift_vkey(vk as u16) {
            return 0;
        }
        stats::key_poll(vk);
    }
    let original: unsafe extern "system" fn(i32) -> i16 = std::mem::transmute(tramp);
    original(vk)
}

unsafe extern "system" fn raw_data_detour(
    hraw: *mut u8,
    command: u32,
    data: *mut u8,
    size: *mut u32,
    header_size: u32,
) -> isize {
    let tramp = RAW_DATA_TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: RawInputDataFn = std::mem::transmute(tramp);
    let out = original(hraw, command, data, size, header_size);
    if command == RID_INPUT && !data.is_null() && out > 0 {
        stats::raw_call();
        // The buffer is the caller's, sized by the same call: trust `out`, and let the helpers
        // reject anything whose own `dwSize` does not fit.
        let record = std::slice::from_raw_parts_mut(data, out as usize);
        visit_record(record);
    }
    out
}

unsafe extern "system" fn raw_buffer_detour(
    data: *mut u8,
    size: *mut u32,
    header_size: u32,
) -> u32 {
    let tramp = RAW_BUFFER_TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: RawInputBufferFn = std::mem::transmute(tramp);
    let count = original(data, size, header_size);
    if !data.is_null() && count > 0 {
        stats::raw_buffer_call();
        let total = if size.is_null() { 0 } else { *size as usize };
        if total > 0 {
            let buffer = std::slice::from_raw_parts_mut(data, total);
            let mut offset = 0usize;
            for _ in 0..count {
                let Some(record) = buffer.get_mut(offset..) else {
                    break;
                };
                let Some(record_size) = raw_size(record) else {
                    break;
                };
                if record_size == 0 || record_size > record.len() {
                    break;
                }
                visit_record(&mut record[..record_size]);
                // Records are padded to 8 bytes each.
                offset += (record_size + 7) & !7;
                if offset >= buffer.len() {
                    break;
                }
            }
        }
    }
    count
}

/// Count and act on one raw record. Shared by both entry points so they cannot drift apart.
fn visit_record(record: &mut [u8]) {
    match raw_kind(record) {
        Some(RAW_TYPE_MOUSE) => {
            stats::mouse_record();
            if let Some(buttons) = raw_mouse_buttons(record) {
                stats::mouse_buttons(buttons);
            }
            // A chord that belongs to a class port is handed over instead of steered, and a
            // Super + click binds rather than casts.
            if !unsafe { bind_gesture(record) } && !unsafe { hand_off_to_recluse(record) } {
                unsafe { steer_click(record) }
            }
        }
        Some(RAW_TYPE_KEYBOARD) => {
            // Same reasoning as the `GetAsyncKeyState` side: the raw stream is where the game
            // tracks held modifiers, so Shift has to be released in both places.
            if raw_keyboard(record).is_some() && raw_hide_shift(record) {
                stats::shift_hidden();
            }
        }
        _ => {}
    }
}

/// Install the three pass-through hooks. A failure is logged and never fatal: the mod works the
/// same on the pad, and the counters simply stay at zero.
pub(crate) fn install() {
    unsafe {
        let module = GetModuleHandleA(b"user32.dll\0".as_ptr());
        if module.is_null() {
            crate::log("input: user32 not found; keyboard/mouse path disabled");
            return;
        }
        hook(
            GetProcAddress(module, b"GetAsyncKeyState\0".as_ptr()),
            key_detour as *const (),
            &KEY_TRAMPOLINE,
            "GetAsyncKeyState",
        );
        hook(
            GetProcAddress(module, b"GetRawInputData\0".as_ptr()),
            raw_data_detour as *const (),
            &RAW_DATA_TRAMPOLINE,
            "GetRawInputData",
        );
        hook(
            GetProcAddress(module, b"GetRawInputBuffer\0".as_ptr()),
            raw_buffer_detour as *const (),
            &RAW_BUFFER_TRAMPOLINE,
            "GetRawInputBuffer",
        );
    }
}

fn hook(target: *mut u8, detour: *const (), tramp: &AtomicPtr<()>, name: &str) {
    if target.is_null() {
        crate::log(&format!("input: {name} not found"));
        return;
    }
    match unsafe { retour::RawDetour::new(target as *const (), detour) } {
        Ok(raw) => {
            if let Err(e) = unsafe { raw.enable() } {
                crate::log(&format!("input: {name} enable failed: {e:?}"));
                return;
            }
            tramp.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
            // Leaked deliberately: the trampoline must outlive the process.
            std::mem::forget(raw);
            crate::log(&format!("input: {name} hooked (keyboard/mouse path)"));
        }
        Err(e) => crate::log(&format!("input: {name} RawDetour failed: {e:?}")),
    }
}
