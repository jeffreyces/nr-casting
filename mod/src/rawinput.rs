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
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

use nr_casting_core::{
    raw_hide_shift, raw_kind, raw_keyboard, raw_mouse_buttons, raw_size, is_shift_vkey,
    RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_DOWN, RAW_TYPE_KEYBOARD, RAW_TYPE_MOUSE,
};

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

/// Whether a pad poll succeeded in the last two seconds.
fn pad_alive() -> bool {
    crate::millis().wrapping_sub(LAST_PAD_OK.load(Ordering::Relaxed)) < 2000
}

fn key_down(vk: i32) -> bool {
    ours(|| unsafe { GetAsyncKeyState(vk) }) < 0
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
    if !left && !right || pad_alive() {
        return;
    }
    if let Some((right_hand, slot, which)) =
        nr_casting_core::mouse_route(left, right, key_down(VK_SHIFT), key_down(VK_CONTROL), key_down(VK_MENU))
    {
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
        // No controller: Shift is the mod's slot modifier and the game would read it as the
        // strong-attack modifier instead.
        if is_shift_vkey(vk as u16) && !pad_alive() {
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
            // Safe: `raw_mouse_buttons` already rejected anything truncated.
            unsafe { steer_click(record) };
        }
        Some(RAW_TYPE_KEYBOARD) => {
            if raw_keyboard(record).is_some() && !pad_alive() && raw_hide_shift(record) {
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
