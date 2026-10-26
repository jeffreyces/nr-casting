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
                    return;
                }
                TRAMPOLINE.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
                std::mem::forget(raw);
                crate::log("kbm: watching GetAsyncKeyState (read-only)");
            }
            Err(e) => crate::log(&format!("kbm: RawDetour failed: {e:?}")),
        }
    }
}

/// One line for the heartbeat: what the game asked for since the last call.
pub(crate) fn report() -> String {
    format!(
        "{} key polls (LMB {}, RMB {}, shift {}, ctrl {}, alt {})",
        TOTAL.swap(0, Ordering::Relaxed),
        LMB.swap(0, Ordering::Relaxed),
        RMB.swap(0, Ordering::Relaxed),
        SHIFT.swap(0, Ordering::Relaxed),
        CTRL.swap(0, Ordering::Relaxed),
        ALT.swap(0, Ordering::Relaxed)
    )
}
