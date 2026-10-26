//! Which Windows input API the game reads the mouse through.
//!
//! The mod's mouse casting is implemented by rewriting a *pad* report, so it depends on the game
//! polling XInput at all. With no controller plugged in ELDEN RING stops calling
//! `XInputGetState` entirely (0 polls, 0 "not connected" replies), so that path cannot fire.
//! Supporting keyboard and mouse only means driving whatever layer the game actually reads.
//!
//! This module exists to answer that with evidence instead of a guess: it hooks
//! `GetAsyncKeyState` and counts the calls the game makes, per key, without changing a single
//! return value. If the counts move, the game reads key state through user32 and a cast can be
//! presented there. If they stay at zero, it uses raw input and no user-mode forgery will work.

use std::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
    fn GetProcAddress(module: *mut u8, name: *const u8) -> *mut u8;
}

/// Calls seen from the game (ours excluded), total and for the keys that matter.
static TOTAL: AtomicU32 = AtomicU32::new(0);
static LMB: AtomicU32 = AtomicU32::new(0);
static RMB: AtomicU32 = AtomicU32::new(0);
static SHIFT: AtomicU32 = AtomicU32::new(0);
static CTRL: AtomicU32 = AtomicU32::new(0);
static ALT: AtomicU32 = AtomicU32::new(0);
static TRAMPOLINE: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Raw-input counters: the other place a game can read the mouse from.
static RAW_DATA_CALLS: AtomicU32 = AtomicU32::new(0);
static RAW_BUFFER_CALLS: AtomicU32 = AtomicU32::new(0);
static RAW_MOUSE_ENTRIES: AtomicU32 = AtomicU32::new(0);
static RAW_BUTTON_EVENTS: AtomicU32 = AtomicU32::new(0);
static RAW_TRAMP_DATA: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static RAW_TRAMP_BUFFER: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// `GetRawInputData` / `GetRawInputBuffer` signatures, and the little of `RAWINPUT` needed to
/// count mouse buttons.
type RawInputDataFn = unsafe extern "system" fn(*mut u8, u32, *mut u8, *mut u32, u32) -> isize;
type RawInputBufferFn = unsafe extern "system" fn(*mut u8, *mut u32, u32) -> u32;

const RID_INPUT: u32 = 0x1000_0003;
/// `RAWINPUTHEADER` on x64: dwType, dwSize, hDevice, wParam.
const HEADER_SIZE: usize = 24;
/// `RAWMOUSE.ulButtons` sits after usFlags (u16) + padding + itself.
const MOUSE_BUTTONS_OFFSET: usize = 4;
const RI_MOUSE_LEFT_BUTTON_DOWN: u32 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_UP: u32 = 0x0002;
const RI_MOUSE_RIGHT_BUTTON_DOWN: u32 = 0x0004;
const RI_MOUSE_RIGHT_BUTTON_UP: u32 = 0x0008;

/// Count the button edges in one `RAWINPUT` record.
unsafe fn count_buttons(record: *const u8) {
    let kind = *(record as *const u32);
    if kind != 0 {
        return; // 0 = mouse, 1 = keyboard, 2 = HID
    }
    RAW_MOUSE_ENTRIES.fetch_add(1, Ordering::Relaxed);
    let buttons = *(record.add(HEADER_SIZE + MOUSE_BUTTONS_OFFSET) as *const u32);
    let mask = RI_MOUSE_LEFT_BUTTON_DOWN
        | RI_MOUSE_LEFT_BUTTON_UP
        | RI_MOUSE_RIGHT_BUTTON_DOWN
        | RI_MOUSE_RIGHT_BUTTON_UP;
    if buttons & mask != 0 {
        RAW_BUTTON_EVENTS.fetch_add(1, Ordering::Relaxed);
    }
}

thread_local! {
    /// Set while our own code asks for key state, so the mod's own probes are not counted as the
    /// game's. Without it, `mouse_combo` alone would make the counters look alive.
    static OURS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

const VK_LBUTTON_V: i32 = 0x01;
const VK_RBUTTON_V: i32 = 0x02;
const VK_SHIFT_V: i32 = 0x10;
const VK_CONTROL_V: i32 = 0x11;
const VK_MENU_V: i32 = 0x12;

unsafe extern "system" fn detour(vk: i32) -> i16 {
    let tramp = TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    if !OURS.with(|ours| ours.get()) {
        TOTAL.fetch_add(1, Ordering::Relaxed);
        match vk {
            VK_LBUTTON_V => LMB.fetch_add(1, Ordering::Relaxed),
            VK_RBUTTON_V => RMB.fetch_add(1, Ordering::Relaxed),
            VK_SHIFT_V => SHIFT.fetch_add(1, Ordering::Relaxed),
            VK_CONTROL_V => CTRL.fetch_add(1, Ordering::Relaxed),
            VK_MENU_V => ALT.fetch_add(1, Ordering::Relaxed),
            _ => 0,
        };
    }
    let original: unsafe extern "system" fn(i32) -> i16 = std::mem::transmute(tramp);
    original(vk)
}

/// Run `f` with our own key-state reads excluded from the counters.
pub(crate) fn ours<R>(f: impl FnOnce() -> R) -> R {
    OURS.with(|ours| {
        let previous = ours.replace(true);
        let out = f();
        ours.set(previous);
        out
    })
}

/// `GetRawInputData` detour: count, then hand the call through untouched.
unsafe extern "system" fn raw_data_detour(
    hraw: *mut u8,
    command: u32,
    data: *mut u8,
    size: *mut u32,
    header_size: u32,
) -> isize {
    let tramp = RAW_TRAMP_DATA.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: RawInputDataFn = std::mem::transmute(tramp);
    let out = original(hraw, command, data, size, header_size);
    if command == RID_INPUT && !data.is_null() && out as usize > HEADER_SIZE {
        RAW_DATA_CALLS.fetch_add(1, Ordering::Relaxed);
        count_buttons(data);
    }
    out
}

/// `GetRawInputBuffer` detour: count every record in the buffer, then pass it back.
unsafe extern "system" fn raw_buffer_detour(
    data: *mut u8,
    size: *mut u32,
    header_size: u32,
) -> u32 {
    let tramp = RAW_TRAMP_BUFFER.load(Ordering::Acquire);
    if tramp.is_null() {
        return 0;
    }
    let original: RawInputBufferFn = std::mem::transmute(tramp);
    let count = original(data, size, header_size);
    if !data.is_null() && count > 0 {
        RAW_BUFFER_CALLS.fetch_add(1, Ordering::Relaxed);
        let mut record = data;
        for _ in 0..count {
            let record_size = *(record.add(4) as *const u32) as usize;
            if record_size < HEADER_SIZE {
                break;
            }
            count_buttons(record);
            // Records are padded to 8 bytes each.
            record = record.add((record_size + 7) & !7);
        }
    }
    count
}

/// Hook `GetAsyncKeyState` read-only. Never fails the mod: if it cannot be installed, the counters
/// simply stay at zero and the log says so.
pub(crate) fn install() {
    unsafe {
        let module = GetModuleHandleA(b"user32.dll\0".as_ptr());
        if module.is_null() {
            crate::log("kbm: user32 not found");
            return;
        }
        let target = GetProcAddress(module, b"GetAsyncKeyState\0".as_ptr());
        if target.is_null() {
            crate::log("kbm: GetAsyncKeyState not found");
            return;
        }
        match retour::RawDetour::new(target as *const (), detour as *const ()) {
            Ok(raw) => {
                if let Err(e) = raw.enable() {
                    crate::log(&format!("kbm: enable failed: {e:?}"));
                } else {
                    TRAMPOLINE.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
                    std::mem::forget(raw);
                    crate::log("kbm: watching GetAsyncKeyState (read-only)");
                }
            }
            Err(e) => crate::log(&format!("kbm: RawDetour failed: {e:?}")),
        }
        hook_read_only(
            GetProcAddress(module, b"GetRawInputData\0".as_ptr()),
            raw_data_detour as *const (),
            &RAW_TRAMP_DATA,
            "GetRawInputData",
        );
        hook_read_only(
            GetProcAddress(module, b"GetRawInputBuffer\0".as_ptr()),
            raw_buffer_detour as *const (),
            &RAW_TRAMP_BUFFER,
            "GetRawInputBuffer",
        );
    }
}

/// Install one pass-through detour, logging either way.
fn hook_read_only(target: *mut u8, detour: *const (), tramp: &AtomicPtr<()>, name: &str) {
    if target.is_null() {
        crate::log(&format!("kbm: {name} not found"));
        return;
    }
    match unsafe { retour::RawDetour::new(target as *const (), detour) } {
        Ok(raw) => {
            if let Err(e) = unsafe { raw.enable() } {
                crate::log(&format!("kbm: {name} enable failed: {e:?}"));
                return;
            }
            tramp.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
            std::mem::forget(raw);
            crate::log(&format!("kbm: watching {name} (read-only)"));
        }
        Err(e) => crate::log(&format!("kbm: {name} RawDetour failed: {e:?}")),
    }
}

/// One line for the heartbeat: what the game asked for since the last call.
pub(crate) fn report() -> String {
    format!(
        "{} key polls (LMB {}, RMB {}, shift {}, ctrl {}, alt {}); raw input: {} data, {} buffer, \
         {} mouse records, {} button events",
        TOTAL.swap(0, Ordering::Relaxed),
        LMB.swap(0, Ordering::Relaxed),
        RMB.swap(0, Ordering::Relaxed),
        SHIFT.swap(0, Ordering::Relaxed),
        CTRL.swap(0, Ordering::Relaxed),
        ALT.swap(0, Ordering::Relaxed),
        RAW_DATA_CALLS.swap(0, Ordering::Relaxed),
        RAW_BUFFER_CALLS.swap(0, Ordering::Relaxed),
        RAW_MOUSE_ENTRIES.swap(0, Ordering::Relaxed),
        RAW_BUTTON_EVENTS.swap(0, Ordering::Relaxed),
    )
}
