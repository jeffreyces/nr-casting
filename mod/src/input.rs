//! The `XInputGetState` hook: chains other mods' hooks instead of replacing them, routes each
//! press to a spell slot, and steers the game's selection for that press.
//!
//! The rules themselves (which input means which slot, and which wins when several are held)
//! live in `nr_casting_core::route` so they are tested; this module carries them out.

use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use nr_casting_core::{Held, Route};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetForegroundWindow, GetWindowThreadProcessId, CURSORINFO, CURSOR_SHOWING,
};

use crate::game::{
    release_borrowed, sample_player_selection, select_spell, selection_is, weapon_ids,
    PLAYER_SPELL,
};
use crate::logging::{debug, log};
use crate::store::{store_get, store_set};
use crate::{config, probe, UI_OPEN};

const XINPUT_GAMEPAD_RIGHT_THUMB: u16 = 0x0080;
const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
const XINPUT_GAMEPAD_Y: u16 = 0x8000;
const TRIGGER_THRESHOLD: u8 = 100;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputGamepad {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    thumb_lx: i16,
    thumb_ly: i16,
    thumb_rx: i16,
    thumb_ry: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputState {
    packet_number: u32,
    gamepad: XInputGamepad,
}

extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut u8;
    fn GetProcAddress(module: *mut u8, name: *const u8) -> *mut u8;
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
}

/// Trampoline to the original `XInputGetState` (retour 0.3's `static_detour!` needs
/// nightly, so `RawDetour` + a hand-kept trampoline).
static TRAMPOLINE: std::sync::atomic::AtomicPtr<()> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

static HOOK_READY: AtomicBool = AtomicBool::new(false);
/// Pad polls seen by our detour. Zero after the game is up means our hook is not in the path.
static POLLS: AtomicU32 = AtomicU32::new(0);
/// Highest poll count seen: proves the hook was live at least once, which distinguishes "another
/// mod overwrote us" from "the game has not started polling yet".
static PEAK_POLLS: AtomicU32 = AtomicU32::new(0);

/// Address our detour currently sits on (the game's pad entry point).
static HOOK_TARGET: AtomicUsize = AtomicUsize::new(0);
/// Live `RawDetour`, kept so it can be disabled before re-targeting.
static ACTIVE_DETOUR: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());
static LAST_INPUT: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_CAPTURE_KEY: AtomicU32 = AtomicU32::new(u32::MAX);
/// When on, idle polls mirror the weapon's slot 1 into the selection so the game's
/// own spell HUD shows what is bound. Locks the D-pad wheel while on (see README).
pub(crate) static HUD_MIRROR: AtomicBool = AtomicBool::new(false);

pub(crate) fn install_hook() -> Result<*mut u8, String> {
    unsafe {
        // Hook the pointer the *game* calls, not just the export in the xinput DLL.
        //
        // Another mod that hooks input (a class port, an overlay) can route the game's polls
        // around our patch, which reads as "the mod loads, the panel works, but triggers stop
        // casting". So we detour the game's own pad entry point and keep watching it: if
        // somebody moves it later, `watch_hook` re-targets.
        let target = current_pad_target()?;
        hook_target(target)?;
        Ok(target)
    }
}

/// Where the game's pad calls currently go.
unsafe fn current_pad_target() -> Result<*mut u8, String> {
    let export = {
        let module = LoadLibraryA(c"xinput1_4.dll".as_ptr() as *const u8);
        let module = if module.is_null() {
            LoadLibraryA(c"xinput9_1_0.dll".as_ptr() as *const u8)
        } else {
            module
        };
        if module.is_null() {
            return Err("xinput dll not loadable".into());
        }
        let addr = GetProcAddress(module, c"XInputGetState".as_ptr() as *const u8);
        if addr.is_null() {
            return Err("XInputGetState not found".into());
        }
        addr
    };
    match game_import_slot() {
        Some(slot) => {
            let value = *slot;
            if value.is_null() {
                Ok(export)
            } else {
                Ok(value)
            }
        }
        None => Ok(export),
    }
}

/// Move our detour onto `target`, disabling whatever it was on before.
fn hook_target(target: *mut u8) -> Result<(), String> {
    if unsafe { patch_is_ours(target) } {
        return Ok(()); // already ours; patching again would recurse into ourselves
    }
    unsafe {
        // Deliberately do NOT disable a previous detour: when another mod patched this entry
        // first, the bytes here are its jump, and whatever we displace becomes our trampoline.
        // Capturing them keeps that mod's hook in the chain — disabling would delete its patch
        // and silently break its input handling (a class port's skill chords, for example).
        let raw = retour::RawDetour::new(target as *const (), detour as *const ())
            .map_err(|e| format!("RawDetour::new: {e:?}"))?;
        raw.enable().map_err(|e| format!("enable: {e:?}"))?;
        TRAMPOLINE.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
        HOOK_TARGET.store(target as usize, Ordering::Release);
        HOOK_READY.store(true, Ordering::Relaxed);
        // Leaked deliberately: the trampoline must outlive the process.
        ACTIVE_DETOUR.store(Box::into_raw(Box::new(raw)) as *mut (), Ordering::Release);
        Ok(())
    }
}

/// Is our jump still the one installed at `target`?
///
/// This is the liveness test that matters: another hook installer (MinHook-based input mods)
/// rewrites the same prologue bytes, which silently removes our patch and leaves the game
/// calling their detour instead. Poll counters cannot tell "overwritten" from "the game has not
/// started polling yet", but the bytes can.
unsafe fn patch_is_ours(target: *mut u8) -> bool {
    if target.is_null() || !probe::readable(target, 5) {
        return false;
    }
    // RawDetour writes a rel32 jump on x86-64.
    if *target != 0xE9 {
        return false;
    }
    let rel = *(target.add(1) as *const i32) as isize;
    let destination = (target as isize).wrapping_add(5).wrapping_add(rel);
    destination as usize == detour as *const () as usize
}

/// Re-target the hook if another mod has since routed the game's polls elsewhere.
pub(crate) fn watch_hook() {
    let mut attempts = 0;
    let mut warned = false;
    let mut ticks = 0u32;

    loop {
        std::thread::sleep(Duration::from_secs(2));
        ticks += 1;
        // A loaded hook that never fires means somebody else intercepts the pad: the mod then
        // loads, the panel opens, and the triggers silently stop casting. Give the game time to
        // start polling (loading screens legitimately poll nothing) before saying so, once.
        if !warned && ticks >= 12 && PEAK_POLLS.load(Ordering::Relaxed) == 0 {
            log(
                "input: no pad polls received — another mod is intercepting XInput, so the \
                 trigger/ash rewriting cannot work. Remove or disable that mod's input DLL.",
            );
            warned = true;
        }
        // Another mod's hook installer can overwrite our patch bytes on the export (MinHook
        // rewrites the same prologue). Nothing is logged by the OS when that happens: the hook
        // simply stops firing. If polls stop arriving after the game is up, put our patch back.
        if ticks >= 4 && attempts < 4 {
            if let Ok(target) = unsafe { current_pad_target() } {
                // Capturing whatever is there now keeps a third-party hook in the chain.
                if !unsafe { patch_is_ours(target) } && hook_target(target).is_ok() {
                    attempts += 1;
                    log(&format!(
                        "XInputGetState: our patch was replaced — re-installed over it at {target:p} (previous hook chained)"
                    ));
                }
            }
        }
        if !HOOK_READY.load(Ordering::Relaxed) || attempts >= 4 {
            return;
        }
        let target = unsafe { current_pad_target() };
        let Ok(target) = target else { continue };
        if target as usize == HOOK_TARGET.load(Ordering::Acquire) {
            continue;
        }
        match hook_target(target) {
            Ok(()) => {
                attempts += 1;
                log(&format!(
                    "XInputGetState: another mod moved the pad entry point — re-hooked at {target:p}"
                ));
            }
            Err(e) => {
                attempts += 1;
                log(&format!("XInputGetState: re-hook failed at {target:p}: {e}"));
            }
        }
    }
}

/// Address the executable's import table routes `XInputGetState` to, if it can be found.
///
/// Walks the PE import directory of the running game rather than scanning memory: the IAT entry
/// is what the game's own code loads before every call, so its current value is the truth about
/// who receives the poll. Every dereference is range-checked first — a parse mistake here must
/// return `None`, never take the game down with it.
unsafe fn game_import_slot() -> Option<*const *mut u8> {
    #[repr(C)]
    struct ImportDescriptor {
        original_first_thunk: u32,
        timestamp: u32,
        forwarder_chain: u32,
        name: u32,
        first_thunk: u32,
    }

    let base = GetModuleHandleA(std::ptr::null()) as *const u8;
    if base.is_null() || !probe::readable(base, 0x40) {
        return None;
    }
    if *(base as *const u16) != 0x5A4D {
        return None;
    }
    let pe_offset = *((base as *const u32).add(0x3C)) as usize;
    if !probe::readable(base.add(pe_offset), 0x18) {
        return None;
    }
    let nt = base.add(pe_offset);
    if *(nt as *const u32) != 0x0000_4550 {
        return None;
    }

    // COFF header is 20 bytes; the optional header follows.
    let opt = nt.add(24);
    if !probe::readable(opt, 0x80) {
        return None;
    }
    let magic = *(opt as *const u16);
    let (dir_offset, size_of_image_offset) = match magic {
        0x20b => (112usize, 56usize), // PE32+
        0x10b => (96, 56),            // PE32
        _ => return None,
    };
    let image_size = *(opt.add(size_of_image_offset) as *const u32) as usize;
    if image_size == 0 {
        return None;
    }
    let in_module = |rva: usize| rva != 0 && rva < image_size;

    // Data directory 1 is the import table; entry 0 is exports.
    let import_rva = *(opt.add(dir_offset + 8) as *const u32) as usize;
    if !in_module(import_rva) {
        return None;
    }

    for index in 0..256usize {
        let descriptor = base.add(import_rva + index * std::mem::size_of::<ImportDescriptor>());
        if !probe::readable(descriptor, std::mem::size_of::<ImportDescriptor>()) {
            return None;
        }
        let descriptor = &*(descriptor as *const ImportDescriptor);
        if descriptor.name == 0 {
            return None; // end of the descriptor array
        }
        if !in_module(descriptor.name as usize) {
            return None;
        }
        let module_name = read_c_string(base.add(descriptor.name as usize));

        if module_name.to_ascii_lowercase().contains("xinput") {
            let thunk_rva = if descriptor.original_first_thunk != 0 {
                descriptor.original_first_thunk
            } else {
                descriptor.first_thunk
            };
            if !in_module(thunk_rva as usize) || !in_module(descriptor.first_thunk as usize) {
                return None;
            }
            for slot_index in 0..4096usize {
                let thunk = base.add(thunk_rva as usize + slot_index * 8) as *const u64;
                let slot = base.add(descriptor.first_thunk as usize + slot_index * 8) as *const *mut u8;
                if !probe::readable(thunk as *const u8, 8) || !probe::readable(slot as *const u8, 8) {
                    return None;
                }
                let entry = *thunk;
                if entry == 0 {
                    break;
                }
                // High bit set means "imported by ordinal"; XInputGetState never is.
                if entry & 0x8000_0000_0000_0000 != 0 {
                    continue;
                }
                let name_rva = entry as usize + 2;
                if !in_module(name_rva) || !probe::readable(base.add(name_rva), 16) {
                    continue;
                }
                if read_c_string(base.add(name_rva)) == "XInputGetState" {
                    return Some(slot);
                }
            }
        }
    }
    None
}

unsafe fn read_c_string(ptr: *const u8) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let byte = *ptr.add(i);
        if byte == 0 || out.len() > 128 {
            break;
        }
        out.push(byte);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// Runs on whatever thread the game polls the pad from.
unsafe extern "system" fn detour(user: u32, state: *mut XInputState) -> u32 {
    let tramp = TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 1167; // ERROR_DEVICE_NOT_CONNECTED
    }
    let original: unsafe extern "system" fn(u32, *mut XInputState) -> u32 =
        std::mem::transmute(tramp);
    let ret = original(user, state);
    if ret == 0 && !state.is_null() && HOOK_READY.load(Ordering::Relaxed) {
        let n = POLLS.fetch_add(1, Ordering::Relaxed) + 1;
        if n > PEAK_POLLS.load(Ordering::Relaxed) {
            PEAK_POLLS.store(n, Ordering::Relaxed);
        }
        if n == 1 || n.is_multiple_of(600) {
            debug!("detour: poll #{n} (user {user}, buttons={:#06x})", unsafe { (*state).gamepad.buttons });
        }
        adjust(&mut *state);
    }
    ret
}

/// The six mouse combinations, in the same order as the pad's slots.
///
/// The game casts from its own mouse bindings (left = right hand, right = left hand, with the
/// usual modifier for the strong attacks), so the mod does what it does on the pad: steer the
/// selection at the bound spell and hand the game a shoulder press. Mouse and pad share one
/// code path so a slot can never mean two different things.
fn mouse_combo() -> Option<(bool, usize, &'static str)> {
    // Only while the game itself has focus: clicks in the panel or another window are somebody
    // else's input.
    if UI_OPEN.load(Ordering::Relaxed) {
        return None;
    }
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid != std::process::id() {
            return None;
        }
    }
    // In menus, the title screen and any other UI the game shows its cursor; in gameplay it
    // hides and captures it. Clicking there belongs to the game's UI — synthesising a pad press
    // for it turned "Exit" into an attack/confirm, so the cursor decides.
    let mut cursor = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetCursorInfo(&mut cursor) }.is_ok() && cursor.flags.0 & CURSOR_SHOWING.0 != 0 {
        return None;
    }
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
    nr_casting_core::mouse_route(
        down(VK_LBUTTON),
        down(VK_RBUTTON),
        down(VK_SHIFT),
        down(VK_CONTROL),
    )
}

const VK_LBUTTON: i32 = 0x01;
const VK_RBUTTON: i32 = 0x02;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;

fn adjust(state: &mut XInputState) {
    // One sample per poll keeps the panel's "selected" line and capture accurate without
    // putting the side effect inside a getter.
    sample_player_selection();
    let pad = &mut state.gamepad;
    let held = held_from(pad);

    if held.any() {
        debug!(
            "pad: buttons={:#06x} rt={} lt={}",
            held.buttons, pad.right_trigger, pad.left_trigger
        );
    }

    // Which input means what is decided by `nr_casting_core::route`, which is unit-tested; this
    // function only carries out the answer.
    let route = nr_casting_core::route(&held, config::load().modifier);
    if route == Route::AshOfWar {
        pad.buttons &= !XINPUT_GAMEPAD_RIGHT_THUMB;
        release_borrowed();
        return;
    }
    if route == Route::Idle {
        idle(pad);
        return;
    }

    probe::maybe_validate();
    if !probe::validated() {
        return;
    }

    match route {
        Route::Capture {
            right_hand,
            slot,
            label,
        } => {
            let Some((left_id, right_id)) = weapon_ids() else {
                debug!("input: no weapon ids (player data unreadable)");
                return;
            };
            let weapon = if right_hand { right_id } else { left_id };
            capture(pad, weapon, slot, label);
        }
        Route::Chord {
            right_hand,
            slot,
            label,
        }
        | Route::Cast {
            right_hand,
            slot,
            label,
        } => {
            if !cast_slot(right_hand, slot, label, pad) && label.starts_with("Y+") {
                debug!("input: {label} has no bound spell — trigger passed through");
                release_borrowed();
            }
        }
        // Handled above. A future variant lands here and does nothing rather than panicking
        // inside the game's call stack.
        Route::AshOfWar | Route::Idle => {}
    }
}

/// Read the pad into the shape the routing rules take.
fn held_from(pad: &XInputGamepad) -> Held {
    Held {
        buttons: pad.buttons,
        right_pull: pad.right_trigger >= TRIGGER_THRESHOLD,
        left_pull: pad.left_trigger >= TRIGGER_THRESHOLD,
        right_bump: pad.buttons & XINPUT_GAMEPAD_RIGHT_SHOULDER != 0,
        left_bump: pad.buttons & XINPUT_GAMEPAD_LEFT_SHOULDER != 0,
    }
}

/// Nothing spell-shaped is held: hand back the borrowed entry, take a mouse cast if one is
/// pending, and optionally mirror slot 1 onto the game's own HUD.
///
/// Mirroring forces the wheel selection every frame, which would stop the player cycling spells
/// — so it is suppressed while the panel is open, and only writes when the selection differs.
fn idle(pad: &mut XInputGamepad) {
    if let Some((right, slot, which)) = mouse_combo() {
        probe::maybe_validate();
        if probe::validated() {
            cast_slot(right, slot, which, pad);
        }
        return;
    }
    release_borrowed();
    if !HUD_MIRROR.load(Ordering::Relaxed) || UI_OPEN.load(Ordering::Relaxed) {
        return;
    }
    let Some((left_id, right_id)) = weapon_ids() else {
        return;
    };
    if let Some(pair) = store_get(right_id).or_else(|| store_get(left_id)) {
        if pair[0] > 0 && !selection_is(pair[0]) {
            select_spell(pair[0]);
        }
    }
}

/// Bind whatever the wheel has selected to `slot`, and swallow the press so binding never also
/// casts.
fn capture(pad: &mut XInputGamepad, weapon: i32, slot: usize, which: &str) {
    let key = (weapon as u32) << 8 | slot as u32;
    if LAST_CAPTURE_KEY.swap(key, Ordering::Relaxed) == key {
        // Same press as the last poll: swallow it without binding twice.
        swallow_spell_inputs(pad);
        return;
    }
    let spell = PLAYER_SPELL.load(Ordering::Relaxed);
    if spell > 0 {
        store_set(weapon, slot, spell);
        log(&format!("capture: {which} on {weapon} = spell {spell}"));
    } else {
        log(&format!("capture: {which} on {weapon} — no spell selected yet"));
    }
    swallow_spell_inputs(pad);
}

fn swallow_spell_inputs(pad: &mut XInputGamepad) {
    pad.buttons &= !(XINPUT_GAMEPAD_LEFT_SHOULDER
        | XINPUT_GAMEPAD_RIGHT_SHOULDER
        | config::load().modifier);
    pad.left_trigger = 0;
    pad.right_trigger = 0;
}

/// Cast the spell bound to `slot` of the weapon in `right`'s hand.
///
/// Used by both the pad and the mouse. The trigger is always swallowed and a shoulder press
/// substituted (that is what the game treats as a cast), and Y is cleared so the press reads as
/// a plain cast rather than two-handing or a class port's skill chord.
fn cast_slot(right: bool, slot: usize, which: &str, pad: &mut XInputGamepad) -> bool {
    let Some((left_id, right_id)) = weapon_ids() else {
        return false;
    };
    let weapon = if right { right_id } else { left_id };
    let bound = store_get(weapon).map(|s| s.get(slot).copied().unwrap_or(-1)).unwrap_or(-1);
    if bound <= 0 {
        debug!("input: {which} has no bound spell");
        return false;
    }
    pad.right_trigger = 0;
    pad.left_trigger = 0;
    pad.buttons &= !XINPUT_GAMEPAD_Y;
    if right {
        pad.buttons |= XINPUT_GAMEPAD_RIGHT_SHOULDER;
    } else {
        pad.buttons |= XINPUT_GAMEPAD_LEFT_SHOULDER;
    }
    if !select_spell(bound) {
        debug!("input: {which} bound to {bound} but magic data unreadable");
        return false;
    }
    let key = (slot as u32) << 1 | u32::from(right);
    if LAST_INPUT.swap(key, Ordering::Relaxed) != key {
        debug!("{which} -> spell {bound}");
    }
    true
}

