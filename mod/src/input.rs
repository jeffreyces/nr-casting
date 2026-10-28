//! The `XInputGetState` hook: chains other mods' hooks instead of replacing them, routes each
//! press to a spell slot, and steers the game's selection for that press.
//!
//! The rules themselves (which input means which slot, and which wins when several are held)
//! live in `nr_casting_core::route` so they are tested; this module carries them out.

use std::sync::atomic::{
    AtomicBool, AtomicU8, AtomicU16, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use std::time::Duration;

use nr_casting_core::{Held, Route};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorInfo, CURSORINFO, CURSOR_SHOWING};

use crate::game::{release_borrowed, sample_player_selection, select_spell, selection_is, weapon_ids};
use crate::logging::{debug, log};
use crate::store::store_get;
use crate::{probe, UI_OPEN};

const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
const XINPUT_GAMEPAD_Y: u16 = 0x8000;
/// Any D-pad direction. The game cycles the spell wheel with these, and the HUD mirror has to get
/// out of their way (see `idle`).
const XINPUT_GAMEPAD_DPAD: u16 = 0x000F;
const TRIGGER_THRESHOLD: u8 = 100;
/// With Y held, *any* trigger movement counts as a pull — one, not twenty.
///
/// ER's swap chord fires on any nonzero trigger with Y down, and it does not act on that input
/// immediately: the swap is buffered for roughly 300 ms, which is why the staff went two-handed
/// *after* the spell had already cast and my correction then fought it twice. At 20 we handed the
/// game Y with a feathered trigger (rt 1-19) on every pass where the player was easing off, and
/// that is precisely the swap input. The trigger is binary here for the same reason it is binary
/// to the game.
const CHORD_TRIGGER_FLOOR: u8 = 1;
/// `XInputGetState` is ordinal 2 in `xinput1_4.dll`, and ELDEN RING imports it that way.
const XINPUT_GET_STATE_ORDINAL: u64 = 2;

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

/// The pad entry point we hooked. Cached so the watcher can ask "is my jump still there?" without
/// walking the game's import table every tick.
static HOOK_TARGET: AtomicUsize = AtomicUsize::new(0);
/// The game's IAT entry, when we routed that instead of patching the export's prologue.
static HOOK_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Trampoline to the original `XInputGetState` (retour 0.3's `static_detour!` needs
/// nightly, so `RawDetour` + a hand-kept trampoline).
static TRAMPOLINE: std::sync::atomic::AtomicPtr<()> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

#[link(name = "kernel32")]
extern "system" {
    fn GetModuleHandleExW(flags: u32, address: *const u16, module: *mut *mut u8) -> i32;
    fn GetModuleFileNameW(module: *mut u8, buffer: *mut u16, size: u32) -> u32;
    fn RtlCaptureStackBackTrace(
        frames_to_skip: u32,
        frames_to_capture: u32,
        back_trace: *mut *mut core::ffi::c_void,
        back_trace_hash: *mut u32,
    ) -> u16;
}

/// Trampoline for `XInputGetStateEx` (ordinal 100), which is the entry point ELDEN RING actually
/// polls for gameplay input.
///
/// `XInputGetState` (ordinal 2) and the Ex variant are *different functions*, and FromSoft titles
/// use the Ex one because it also carries the guide button. Hooking only ordinal 2 was enough for
/// the mod to look alive — Steam's overlay and other mods call it, and ER calls it too for things
/// that are not the gameplay pad read — while the buttons the game acts on came from a copy we
/// never touched. That is the two-hand that survived every input fix: the game was reading the real
/// Y+R1, not our rewritten report.
static TRAMPOLINE_EX: std::sync::atomic::AtomicPtr<()> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

thread_local! {
    /// Whether the in-flight detour call came in through the Ex entry point.
    static VIA_EX: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Same signature as `XInputGetState`, so it reuses the detour body and only swaps the trampoline.
unsafe extern "system" fn detour_ex(user: u32, state: *mut XInputState) -> u32 {
    VIA_EX.with(|flag| flag.set(true));
    let result = detour(user, state);
    VIA_EX.with(|flag| flag.set(false));
    result
}

static HOOK_READY: AtomicBool = AtomicBool::new(false);
/// Module bases we have already named in the log. A pad poll is only worth rewriting if it comes
/// from the game; Steam's overlay polls the same function and is preloaded into this process.
static CALLERS_SEEN: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
/// How many polls may still pay for caller detection. A stack walk plus `GetModuleHandleExW` plus a
/// mutex is far too much to spend 120 times a second forever; the callers do not change after load,
/// so the first handful of polls is the whole job.
static CALLER_BUDGET: AtomicU32 = AtomicU32::new(96);

/// Log which module called into the detour, once per module.
///
/// This is the question that decides whether the input rewrite reaches ELDEN RING at all: the
/// overlay renderer (`gameoverlayrenderer64.dll`) polls XInput for its own UI, and seeing its polls
/// instead of the game's would look exactly like "the mod runs but the game disagrees".
unsafe fn note_caller() {
    // Four frames from the top of our own call, so inlining cannot hide the caller: the module that
    // shows up here is the one the rewrite has to satisfy.
    let mut frames = [std::ptr::null_mut::<core::ffi::c_void>(); 4];
    let captured = RtlCaptureStackBackTrace(1, 4, frames.as_mut_ptr(), std::ptr::null_mut()) as usize;
    if captured == 0 {
        return;
    }
    for frame in frames.iter().take(captured) {
        if frame.is_null() {
            continue;
        }
        let mut module: *mut u8 = std::ptr::null_mut();
        if GetModuleHandleExW(
            0x0000_0004 | 0x0000_0002, // FROM_ADDRESS | UNCHANGED_REFCOUNT
            *frame as *const u16,
            &mut module,
        ) == 0
            || module.is_null()
        {
            continue;
        }
        let base = module as usize;
        let fresh = match CALLERS_SEEN.lock() {
            Ok(mut seen) => {
                if seen.contains(&base) {
                    false
                } else {
                    seen.push(base);
                    true
                }
            }
            Err(_) => false,
        };
        if !fresh {
            continue;
        }
        let mut name = [0u16; 260];
        let len = GetModuleFileNameW(module, name.as_mut_ptr(), 260) as usize;
        let name = String::from_utf16_lossy(&name[..len.min(260)]);
        log(&format!("XInputGetState: caller stack includes {name}"));
    }
}
/// Pad polls seen by our detour. Used to tell "the hook is dead" from "the game is not polling".
/// Polls that were not skipped as repeats — the work the gate saved. Reported when debug_log is on.
/// The `packet_number` of the last report we acted on. XInput only bumps it when the pad state
/// actually changes, so a repeated number means a repeated report — there is nothing new to route.
static LAST_PACKET: AtomicU32 = AtomicU32::new(u32::MAX);
/// Milliseconds of the last full poll pass (the mouse check still has to run on a timer, because
/// clicks arrive independently of the pad).
static LAST_RUN_MS: AtomicU32 = AtomicU32::new(0);
/// Floor between full passes. 8 ms is faster than any pad or mouse report the game consumes, and
/// it caps the per-poll work at ~125 Hz however often the game polls.
const POLL_INTERVAL_MS: u32 = 8;
static LAST_INPUT: AtomicU32 = AtomicU32::new(u32::MAX);
/// How many "Y released" reports the current chord has had. Cleared when the chord ends.
///
/// One, not two. The stage exists because ER keeps its own "Y is held" latch for a frame or two
/// after we stop showing it Y, and a chord that arrives while the latch lives gets two-handed
/// instead of cast. Two stages made that rare but cost ~2 frames (~16-25 ms) on *every* chord,
/// which is the latency you feel. One stage halves it, and the two-hand correction — which now runs
/// inside the chord press, puts the catalyst back and re-sends the press — is the safety net for
/// the misfires that remain. The cast lands either way; only the path differs.
static CHORD_STAGED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
const CHORD_STAGE_REPORTS: u8 = 2;  // default; overridden by `chord_stage` in the ini

/// One Y chord that belongs to the mod: which hand, and the spell behind it (`<= 0` = empty).
#[derive(Clone, Copy)]
struct ChordPress {
    right_hand: bool,
    spell: i32,
    label: &'static str,
}
/// Until when Y is hidden from the game.
///
/// ER's weapon swap is a *buffered chord*: the shoulder press we synthesise as a cast is remembered
/// as a swap candidate, and the game completes the swap when it next sees Y — even a few hundred
/// milliseconds later. Since holding Y with nothing else pressed is a report we pass through
/// untouched, that "next Y" arrived 475 ms after the cast and two-handed the staff. So for a short
/// window after every cast press, Y is cleared out of the report: the swap has nothing to complete
/// with, and the player's own Y press is unaffected once the window passes.
static Y_SUPPRESS_UNTIL: AtomicU32 = AtomicU32::new(0);
const Y_SUPPRESS_MS: u32 = 700;

/// Whether the previous report already had Y down. A press that brings Y and the button in the
/// same report was never seen by the game as "Y held", so it needs no staged release.
static Y_BEFORE: AtomicBool = AtomicBool::new(false);
/// Last pad state we wrote to the debug log, packed buttons|rt|lt, so a held button logs once
/// instead of 125 times a second.
static LAST_PAD_LOG: AtomicU32 = AtomicU32::new(u32::MAX);
/// Last HUD-mirror pass, in milliseconds (see `idle`).
static LAST_MIRROR_MS: AtomicU32 = AtomicU32::new(0);
/// When on, idle polls mirror the weapon's slot 1 into the selection so the game's
/// own spell HUD shows what is bound. Locks the D-pad wheel while on (see README).

pub(crate) fn install_hook() -> Result<*mut u8, String> {
    unsafe {
        // Hook the pointer the *game* calls, not just the export in the xinput DLL.
        //
        // Another mod that hooks input (a class port, an overlay) can route the game's polls
        // around our patch, which reads as "the mod loads, the panel works, but triggers stop
        // casting". So first choice is the executable's own import slot — an aligned pointer,
        // so the change is atomic and touches no instruction bytes — and only then the export's
        // prologue.
        let target = current_pad_target()?;
        HOOK_TARGET.store(target as usize, Ordering::Relaxed);
        match hook_import_slot() {
            Ok(_) => {
                log(&format!(
                    "XInputGetState: routed the game's import slot to our detour at {target:p} (no code patched)"
                ));
                Ok(target)
            }
            Err(e) => {
                log(&format!(
                    "XInputGetState: import slot unusable ({e}) — patching the export instead"
                ));
                hook_target(target)?;
                hook_ex(target);
                Ok(target)
            }
        }
    }
}

/// Hook `XInputGetStateEx` as well, when the DLL exports it separately.
///
/// This is the entry point the game polls for gameplay input (see `TRAMPOLINE_EX`). Failure is not
/// fatal: the ordinal-2 hook is what the mod has always had, and some xinput builds alias the two.
unsafe fn hook_ex(normal: *mut u8) {
    let module = LoadLibraryA(c"xinput1_4.dll".as_ptr() as *const u8);
    let module = if module.is_null() {
        LoadLibraryA(c"xinput9_1_0.dll".as_ptr() as *const u8)
    } else {
        module
    };
    if module.is_null() {
        return;
    }
    // Ordinals are passed as a small integer cast to a pointer, per MAKEINTRESOURCE.
    let ex = GetProcAddress(module, 100usize as *const u8);
    if ex.is_null() {
        log("XInputGetStateEx (ordinal 100): not exported by this xinput — the ordinal-2 hook is all there is");
        return;
    }
    if ex == normal || patch_is_ours(ex) {
        log("XInputGetStateEx (ordinal 100): same entry point as XInputGetState — nothing extra to hook");
        return;
    }
    match retour::RawDetour::new(ex as *const (), detour_ex as *const ()) {
        Ok(raw) => match raw.enable() {
            Ok(()) => {
                TRAMPOLINE_EX.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
                log(&format!(
                    "XInputGetStateEx (ordinal 100): hooked at {ex:p} — this is the pad read the game acts on"
                ));
                std::mem::forget(raw);
            }
            Err(e) => log(&format!("XInputGetStateEx (ordinal 100): enable failed: {e:?}")),
        },
        Err(e) => log(&format!("XInputGetStateEx (ordinal 100): detour failed: {e:?}")),
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
        Ok(slot) => {
            let value = *slot;
            if value.is_null() {
                Ok(export)
            } else {
                Ok(value)
            }
        }
        Err(_) => Ok(export),
    }
}

/// Move our detour onto `target`, disabling whatever it was on before.
fn hook_target(target: *mut u8) -> Result<(), String> {
    if unsafe { patch_is_ours(target) } {
        return Ok(()); // already ours; patching again would recurse into ourselves
    }
    unsafe {
        // If we own the game's import slot, hand it back before patching: two live paths into
        // the same detour would run the routing twice for one poll, and the second pass would
        // see an already-rewritten report.
        if import_slot_is_ours() {
            if let Ok(slot) = game_import_slot() {
                let previous = TRAMPOLINE.load(Ordering::Acquire) as *mut u8;
                *(slot as *mut *mut u8) = previous;
            }
        }
        // Deliberately do NOT disable a previous detour: when another mod patched this entry
        // first, the bytes here are its jump, and whatever we displace becomes our trampoline.
        // Capturing them keeps that mod's hook in the chain — disabling would delete its patch
        // and silently break its input handling (a class port's skill chords, for example).
        let raw = retour::RawDetour::new(target as *const (), detour as *const ())
            .map_err(|e| format!("RawDetour::new: {e:?}"))?;
        raw.enable().map_err(|e| format!("enable: {e:?}"))?;
        TRAMPOLINE.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
        HOOK_READY.store(true, Ordering::Relaxed);
        // Leaked deliberately: the detour and its trampoline must outlive the process, and the
        // displaced prologue has to stay captured for whoever we chained over.
        std::mem::forget(raw);
        Ok(())
    }
}

extern "system" {
    fn VirtualProtect(address: *mut u8, size: usize, new_protect: u32, old: *mut u32) -> i32;
}

/// Point the game's *own* import slot for `XInputGetState` at our detour.
///
/// This is the safe way to take the pad back when another mod already claimed the exported
/// function: an IAT entry is one aligned pointer, so the change is a single store and no
/// instruction bytes are rewritten while the game may be executing them. The trampoline becomes
/// whatever the slot held — the other mod's detour — so their hook keeps running inside ours.
///
/// `Ok(true)` means the slot was moved, `Ok(false)` that it already pointed at us.
unsafe fn hook_import_slot() -> Result<bool, String> {
    const PAGE_READWRITE: u32 = 0x04;
    let size = std::mem::size_of::<*mut u8>();
    let slot = game_import_slot()?;
    // The import walk hands back a read-only view; writing the entry is the whole point here.
    let slot = slot as *mut *mut u8;
    // SAFETY: the slot address came from the import walk, which bounds-checked it.
    if !probe::readable(slot as *const u8, size) {
        return Err("import slot is unreadable".into());
    }
    let previous = *slot;
    if previous.is_null() {
        return Err("import slot is empty".into());
    }
    if previous == detour as *const () as *mut u8 {
        return Ok(false);
    }
    let mut old = 0u32;
    let unlocked = VirtualProtect(slot as *mut u8, size, PAGE_READWRITE, &mut old) != 0;
    // Trampoline first: a poll that lands between the two stores must never enter a detour whose
    // trampoline is still null.
    TRAMPOLINE.store(previous as *mut (), Ordering::Release);
    *slot = detour as *const () as *mut u8;
    if unlocked {
        let _ = VirtualProtect(slot as *mut u8, size, old, &mut old);
    }
    HOOK_SLOT.store(slot as usize, Ordering::Relaxed);
    HOOK_READY.store(true, Ordering::Relaxed);
    Ok(true)
}

/// Does the game's import slot already point at our detour?
unsafe fn import_slot_is_ours() -> bool {
    // Cached slot first: re-walking the import table is thousands of `VirtualQuery` calls on a
    // packed-free image, and the watcher asks this every half second.
    let cached = HOOK_SLOT.load(Ordering::Relaxed);
    if cached != 0 {
        let slot = cached as *const *mut u8;
        if probe::readable(slot as *const u8, std::mem::size_of::<*mut u8>()) {
            if *slot == detour as *const () as *mut u8 {
                return true;
            }
        }
    }
    match game_import_slot() {
        Ok(slot) if probe::readable(slot as *const u8, std::mem::size_of::<*mut u8>()) => {
            let ours = *slot == detour as *const () as *mut u8;
            if ours {
                HOOK_SLOT.store(slot as usize, Ordering::Relaxed);
            }
            ours
        }
        _ => false,
    }
}

/// Is our jump still the one installed at `target`?
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

/// Re-target the hook when another mod has taken the pad entry point away from us.
///
/// The old version re-patched whenever the prologue did not look like ours, on a 2-second
/// timer, for the first four ticks — including while the game was starting and while polls
/// were arriving normally. Rewriting a function's prologue while another thread is executing
/// it is a crash, so the patch is now only touched when there is evidence it is needed: a live
/// character, and no new polls for a dozen seconds.
pub(crate) fn watch_hook() {
    let mut attempts = 0;
    let mut warned = false;
    let mut tried_import_slot = false;
    let mut last_polls = crate::stats::pad_polls();
    let mut idle_ticks = 0u32;

    loop {
        // Half a second per tick: recovery is gated on a validated character (the telemetry loop
        // only asks every 30 s), and a displacement detected by bytes should not wait for a poll
        // gap to be measured in seconds.
        std::thread::sleep(Duration::from_millis(500));
        if !HOOK_READY.load(Ordering::Relaxed) || attempts >= 8 {
            return;
        }
        crate::probe::maybe_validate();
        // Both the chord decision and the slots file follow the loaded character. Reading the
        // name here rather than only on the load edge is what catches a switch the edge missed
        // and a name that was not readable the instant the world appeared.
        crate::store::sync_character();
        crate::refresh_y_bumper();
        if !crate::probe::validated() {
            // Nothing to protect before the world exists: the title screen and loading screens
            // legitimately poll nothing, and re-patching there is pure risk.
            continue;
        }

        let polls = crate::stats::pad_polls();
        if polls != last_polls {
            last_polls = polls;
            idle_ticks = 0;
            attempts = 0; // a working hook resets the churn budget
            continue;
        }
        idle_ticks = idle_ticks.saturating_add(1);
        if idle_ticks < 2 {
            continue;
        }

        // Ask the cached entry point, not the import table: `current_pad_target` walks the PE's
        // import directory (thousands of `VirtualQuery` calls on an unpacked image) and this runs
        // twice a second for the whole session.
        let cached = HOOK_TARGET.load(Ordering::Relaxed);
        let target = if cached != 0 {
            cached as *mut u8
        } else {
            match unsafe { current_pad_target() } {
                Ok(t) => {
                    HOOK_TARGET.store(t as usize, Ordering::Relaxed);
                    t
                }
                Err(_) => continue,
            }
        };

        // The bytes decide, not the poll counter: "our jump is gone" is unambiguous, while "no
        // polls" also happens in menus, cutscenes and with no controller connected.
        if unsafe { patch_is_ours(target) } || unsafe { import_slot_is_ours() } {
            if !warned {
                log(
                    "input: hooked, but the game is not polling XInputGetState — is a \
                     controller connected? Casting via the triggers cannot work without one.",
                );
                warned = true;
            }
            continue;
        }

        if !warned {
            if crate::stats::pad_unplugged_count() > 0 || crate::stats::pad_polls() == 0 {
                log(
                    "input: no pad polls — no controller is connected, so the pad rewriting is \
                     idle; mouse casts go through the raw-input path instead.",
                );
            } else {
                log(
                    "input: no pad polls received — another mod is intercepting XInput, so the \
                     trigger/ash rewriting cannot work until it is removed or disabled.",
                );
            }
            warned = true;
        }

        // Not ours at the cached address: another mod may also have moved the entry point, so
        // re-resolve before installing anything.
        let Ok(target) = (unsafe { current_pad_target() }) else {
            continue;
        };
        HOOK_TARGET.store(target as usize, Ordering::Relaxed);

        // Prefer pointing the game's own import slot at our detour: that is one aligned pointer
        // store, with no instruction bytes rewritten while the game may be running them. Only
        // fall back to patching the export's prologue if the slot is already ours (the game is
        // evidently not reading it on each call) or cannot be found.
        if !tried_import_slot {
            tried_import_slot = true;
            match unsafe { hook_import_slot() } {
                Ok(true) => {
                    attempts += 1;
                    idle_ticks = 0;
                    log(
                        "XInputGetState: another mod owns the export's prologue — routed the \
                         game's import slot to our detour instead (no code patched, their hook \
                         still runs inside ours)",
                    );
                    continue;
                }
                Ok(false) => {
                    log(
                        "XInputGetState: import slot already ours but no polls — the game is \
                         not reading it per call; patching the export instead",
                    );
                }
                Err(e) => log(&format!("XInputGetState: import slot unusable ({e})")),
            }
        }

        // Capturing whatever is there now keeps a third-party hook in the chain.
        match hook_target(target) {
            Ok(()) => {
                attempts += 1;
                idle_ticks = 0;
                log(&format!(
                    "XInputGetState: our patch was replaced — re-installed over it at {target:p} \
                     (previous hook chained)"
                ));
            }
            Err(e) => {
                attempts += 1;
                idle_ticks = 0;
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
/// return `Err`, never take the game down with it. The `Err` carries what the walk saw, because
/// "not found" is otherwise indistinguishable from "the parse gave up on descriptor 3".
unsafe fn game_import_slot() -> Result<*const *mut u8, String> {
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
        return Err("main module header unreadable".into());
    }
    if *(base as *const u16) != 0x5A4D {
        return Err("main module is not an MZ image".into());
    }
    let pe_offset = *((base as *const u32).add(0x3C)) as usize;
    if !probe::readable(base.add(pe_offset), 0x18) {
        // Reality check: ELDEN RING's header is intact on disk (e_lfanew 0x168) but not in
        // memory under Proton — Arxan or the loader has overwritten it, so the import table
        // cannot be walked at runtime there. On Windows the walk works and the pad is hooked
        // through the import slot; here it falls back to the export's prologue.
        return Err(format!(
            "PE header unusable (e_lfanew {pe_offset:#x}, expected 0x168) — packed image"
        ));
    }
    let nt = base.add(pe_offset);
    if *(nt as *const u32) != 0x0000_4550 {
        return Err("PE signature missing".into());
    }

    // COFF header is 20 bytes; the optional header follows.
    let opt = nt.add(24);
    if !probe::readable(opt, 0x80) {
        return Err("optional header unreadable".into());
    }
    let magic = *(opt as *const u16);
    let (dir_offset, size_of_image_offset) = match magic {
        0x20b => (112usize, 56usize), // PE32+
        0x10b => (96, 56),            // PE32
        _ => return Err(format!("unknown optional header magic {magic:#x}")),
    };
    let image_size = *(opt.add(size_of_image_offset) as *const u32) as usize;
    if image_size == 0 {
        return Err("image size is zero".into());
    }
    let in_module = |rva: usize| rva != 0 && rva < image_size;

    // Data directory 1 is the import table; entry 0 is exports.
    let import_rva = *(opt.add(dir_offset + 8) as *const u32) as usize;
    if !in_module(import_rva) {
        return Err(format!("import directory rva {import_rva:#x} is outside the image"));
    }

    let mut seen: Vec<String> = Vec::new();
    for index in 0..256usize {
        let descriptor = base.add(import_rva + index * std::mem::size_of::<ImportDescriptor>());
        if !probe::readable(descriptor, std::mem::size_of::<ImportDescriptor>()) {
            return Err(format!("descriptor {index} unreadable; saw {seen:?}"));
        }
        let descriptor = &*(descriptor as *const ImportDescriptor);
        if descriptor.name == 0 {
            break; // end of the descriptor array
        }
        if !in_module(descriptor.name as usize) {
            // One bad entry must not hide the rest: Arxan and packing tools leave odd values
            // behind, and the entry we want is frequently further down the list.
            seen.push(format!("{index}:bad-name-rva"));
            continue;
        }
        let module_name = read_c_string(base.add(descriptor.name as usize));
        seen.push(module_name.clone());

        if !module_name.to_ascii_lowercase().contains("xinput") {
            continue;
        }
        let thunk_rva = if descriptor.original_first_thunk != 0 {
            descriptor.original_first_thunk
        } else {
            descriptor.first_thunk
        };
        if !in_module(thunk_rva as usize) || !in_module(descriptor.first_thunk as usize) {
            return Err(format!(
                "{module_name}: thunk rva (oft {:#x}, ft {:#x}) outside the image",
                descriptor.original_first_thunk, descriptor.first_thunk
            ));
        }
        for slot_index in 0..4096usize {
            let thunk = base.add(thunk_rva as usize + slot_index * 8) as *const u64;
            let slot = base.add(descriptor.first_thunk as usize + slot_index * 8) as *const *mut u8;
            if !probe::readable(thunk as *const u8, 8) || !probe::readable(slot as *const u8, 8) {
                return Err(format!("{module_name}: thunk {slot_index} unreadable"));
            }
            let entry = *thunk;
            if entry == 0 {
                break;
            }
            // The game imports this one **by ordinal** (XINPUT1_4 #2), so a name compare alone
            // never matches — that is why this walk used to fall through to the exported
            // function every time. Accept both spellings.
            if entry & 0x8000_0000_0000_0000 != 0 {
                if entry & 0xFFFF == XINPUT_GET_STATE_ORDINAL {
                    return Ok(slot);
                }
                continue;
            }
            let name_rva = entry as usize & 0x7FFF_FFFF;
            let name_rva = name_rva + 2; // skip the 2-byte hint
            if !in_module(name_rva) || !probe::readable(base.add(name_rva), 16) {
                continue;
            }
            if read_c_string(base.add(name_rva)) == "XInputGetState" {
                return Ok(slot);
            }
        }
    }
    Err(format!("no xinput entry among {seen:?}"))
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
    let tramp = if VIA_EX.with(|flag| flag.get()) {
        TRAMPOLINE_EX.load(Ordering::Acquire)
    } else {
        TRAMPOLINE.load(Ordering::Acquire)
    };
    if tramp.is_null() {
        return 1167; // ERROR_DEVICE_NOT_CONNECTED
    }
    let original: unsafe extern "system" fn(u32, *mut XInputState) -> u32 =
        std::mem::transmute(tramp);
    let ret = original(user, state);
    if ret != 0 {
        crate::stats::pad_unplugged();
    }
    if !state.is_null() && HOOK_READY.load(Ordering::Relaxed) {
        if ret == 0 {
            crate::rawinput::note_pad_ok();
            let n = crate::stats::pad_ok();
            if n == 1 || n.is_multiple_of(600) {
                debug!(
                    "detour: poll #{n} (user {user}, buttons={:#06x})",
                    unsafe { (*state).gamepad.buttons }
                );
            }
            if CALLER_BUDGET.load(Ordering::Relaxed) > 0 {
                CALLER_BUDGET.fetch_sub(1, Ordering::Relaxed);
                note_caller();
            }
            adjust_inner(&mut *state);
        }
        // A failed read goes back to the game unchanged. Mouse casts do not come through here any
        // more: they are steered in the raw-input layer and performed by the game's own click, so
        // there is nothing to synthesise (and a synthetic pad report alongside a real click was
        // one way to get two actions out of one press).
    }
    ret
}

/// Is the game showing its cursor, i.e. are we in a menu rather than in the world?
///
/// The one signal that separates "this click is the game's UI" from "this click is a cast": in
/// gameplay ER hides and captures the pointer, in every menu it shows it.
pub(crate) fn game_shows_cursor() -> bool {
    let mut cursor = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetCursorInfo(&mut cursor) }.is_ok() && cursor.flags.0 & CURSOR_SHOWING.0 != 0
}

/// Whether the Y+bumper chords (Y+R1, Y+L1, the fourth slot) are ours this session.
///
/// Recluse uses exactly those two chords, so the answer is decided per character and cached: it
/// changes on a character load, not on a poll, and `route` reads it on every poll.
static Y_BUMPER: AtomicBool = AtomicBool::new(true);

pub(crate) fn y_bumper() -> bool {
    Y_BUMPER.load(Ordering::Relaxed)
}

/// Record whether the fourth slot's chords belong to us, logging only when it changes.
pub(crate) fn set_y_bumper(allowed: bool) {
    if Y_BUMPER.swap(allowed, Ordering::Relaxed) != allowed {
        crate::log(if allowed {
            "input: Y+R1 / Y+L1 are ours (4 spells per hand)"
        } else {
            "input: Y+R1 / Y+L1 left to Recluse (3 spells per hand)"
        });
    }
}

/// Bits of "this (hand, slot) carries a spell" for the equipped weapon pair.
///
/// Bit `slot` is the right hand, bit `SLOTS + slot` the left. `route` and the chord checks ask
/// this on every poll while a chord is held, and answering it from the store meant a lock plus
/// two game-memory reads at up to 125 Hz. It only changes when the weapons or the assignments
/// do, so it is cached against both.
static BOUND_KEY: AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
static BOUND_GEN: AtomicU32 = AtomicU32::new(0);
static BOUND_MASK: AtomicU16 = std::sync::atomic::AtomicU16::new(0);
/// Whether each hand's weapon can cast at all. Cached with the mask: it is a param read.
static CATALYST_LEFT: AtomicBool = AtomicBool::new(false);
static CATALYST_RIGHT: AtomicBool = AtomicBool::new(false);

fn bound_mask(left: i32, right: i32) -> u16 {
    let key = (left as u32 as u64) << 32 | right as u32 as u64;
    let generation = crate::store::generation();
    if BOUND_KEY.load(Ordering::Relaxed) == key
        && BOUND_GEN.load(Ordering::Relaxed) == generation
    {
        return BOUND_MASK.load(Ordering::Relaxed);
    }
    let mut mask = 0u16;
    for slot in 0..nr_casting_core::SLOTS {
        if bound_on(right, slot) > 0 {
            mask |= 1 << slot;
        }
        if bound_on(left, slot) > 0 {
            mask |= 1 << (nr_casting_core::SLOTS + slot);
        }
    }
    // `weapon_is_catalyst` is a param read, so it is answered here — once per weapon change —
    // rather than per poll. An unreadable row falls back to "has assignments", which keeps a
    // port's own catalyst working when the base game's categories do not cover it.
    CATALYST_LEFT.store(catalyst_or_bound(left, mask, false), Ordering::Relaxed);
    CATALYST_RIGHT.store(catalyst_or_bound(right, mask, true), Ordering::Relaxed);
    BOUND_MASK.store(mask, Ordering::Relaxed);
    BOUND_GEN.store(generation, Ordering::Relaxed);
    BOUND_KEY.store(key, Ordering::Relaxed);
    mask
}

/// Can this hand's weapon cast?
///
/// Vanilla catalysts live in two dedicated param bands — `33xxxxxx` glintstone staffs and
/// `34xxxxxx` sacred seals — and every row in both is a catalyst (42 of them), while the weapons
/// that merely *look* like catalysts sit outside: `Watchdog's Staff` and `Rotten Staff` are
/// colossal weapons, `Devourer's Scepter` a hammer, `Barbed Staff-Spear` a spear. So the band is
/// the test, not the name.
///
/// A weapon outside those bands that the name table does not know belongs to a content mod, and
/// falls back to "has any spell assigned" — how a port's own catalyst keeps working.
fn catalyst_or_bound(weapon: i32, mask: u16, right: bool) -> bool {
    let hand_bits = if right {
        mask & ((1 << nr_casting_core::SLOTS) - 1)
    } else {
        (mask >> nr_casting_core::SLOTS) & ((1 << nr_casting_core::SLOTS) - 1)
    };
    let base = nr_casting_core::base_weapon_id(weapon);
    let band = base / 1_000_000;
    let answer = if band == 33 || band == 34 {
        true
    } else if crate::overlay::weapon_name_base(base).is_none() {
        hand_bits != 0
    } else {
        false
    };
    debug!(
        "weapons: {} {} (cat {:?}) — catalyst {answer}, slots {hand_bits:#x}",
        if right { "R" } else { "L" },
        crate::overlay::weapon_name_base(weapon).unwrap_or_else(|| format!("weapon {weapon}")),
        crate::params::weapon_category(weapon),
    );
    answer
}

/// Are the Y chords on this hand the mod's? Only when that hand holds a catalyst.
///
/// The player's rule, stated plainly: "we should only two-hand an item that is not a staff". So on
/// a catalyst every chord belongs to the mod — a cast when the slot carries a spell, a swallow when
/// it does not (never a two-hand). On anything else the game keeps the whole press, which is what
/// makes a sword two-handable again even if a stray assignment sits on its slots.
fn chords_owned(right: bool) -> bool {
    // Refresh first: the flags live in the same cache as the slot mask, and a pass can reach this
    // before anything has asked about the current weapon pair (after a swap, say). `bound_mask`
    // is a no-op when the pair and the store are unchanged.
    if let Some((left, right_id)) = weapon_ids() {
        let _ = bound_mask(left, right_id);
    }
    if right {
        CATALYST_RIGHT.load(Ordering::Relaxed)
    } else {
        CATALYST_LEFT.load(Ordering::Relaxed)
    }
}

/// Last report we traced, packed.
static LAST_WIRE: AtomicU64 = AtomicU64::new(u64::MAX);
/// Last stance value logged, so a transition is one line rather than one per poll.
static LAST_STANCE: AtomicU8 = AtomicU8::new(u8::MAX);

/// Whether the equipment says a catalyst is being held in both hands.
///
/// While a weapon is held two-handed, `active_weapon_slot` reports it for *both* hands, so the two
/// hands read the same param id. That is a fact about the equipment rather than about `arm_style`,
/// which the game updates on its own schedule.
fn catalyst_two_handed() -> bool {
    if !probe::validated() {
        return false;
    }
    let Some((left, right)) = weapon_ids() else {
        return false;
    };
    left > 0 && left == right && chords_owned(true)
}

/// The last (hand, slot) a held bind gesture acted on, so one press binds once.
static LAST_BIND: AtomicU32 = AtomicU32::new(u32::MAX);

fn adjust_inner(state: &mut XInputState) {
    let now = crate::millis();
    let due = state.packet_number != LAST_PACKET.load(Ordering::Relaxed)
        || now.wrapping_sub(LAST_RUN_MS.load(Ordering::Relaxed)) >= POLL_INTERVAL_MS;
    LAST_PACKET.store(state.packet_number, Ordering::Relaxed);

    let pad = &mut state.gamepad;
    let bind_modifier = crate::config::load().bind_modifier;

    // ---- bind gesture ------------------------------------------------------------------
    // Before anything is rewritten, and before the world check: the modifier plus one of the
    // four spell inputs assigns the spell the *game* has selected to that slot. Reading the
    // game's own selection is what makes this work for spells the character can only cast with
    // a Great Rune or a talisman on — the panel's requirement filter cannot see those.
    if bind_modifier != 0 && pad.buttons & bind_modifier != 0 {
        let held = held_from(pad, true);
        if let Some((right_hand, slot, which)) = nr_casting_core::bind_route(&held) {
            if bind_slot(right_hand, slot, which) {
                // Swallow the press so a bind never also casts, the modifier so its own function
                // (L3 is "reset camera") does not fire either, and Y so a bind made on a chord
                // does not arm the game's two-hand latch for the press after it.
                pad.buttons &= !(XINPUT_GAMEPAD_LEFT_SHOULDER
                    | XINPUT_GAMEPAD_RIGHT_SHOULDER
                    | XINPUT_GAMEPAD_Y
                    | bind_modifier);
                pad.left_trigger = 0;
                pad.right_trigger = 0;
                return;
            }
        }
    }

    // A cast press arms ER's buffered swap chord; keeping Y away from the game for a moment stops
    // it from ever completing. Pad-only, so it runs before any game-memory work.
    let quiet_y = pad.right_trigger == 0
        && pad.left_trigger == 0
        && pad.buttons & (XINPUT_GAMEPAD_LEFT_SHOULDER | XINPUT_GAMEPAD_RIGHT_SHOULDER) == 0;
    // `armed_until` treats a zero deadline as "not armed". Reading it as a deadline instead made
    // every trigger-less report drop Y for the first half of every 49.7-day clock wrap, which is
    // the Site of Grace that would not take Y until a cast had happened once.
    if quiet_y && nr_casting_core::armed_until(now, Y_SUPPRESS_UNTIL.load(Ordering::Relaxed)) {
        // Only the "Y on its own" report: that is the one that completes the buffered swap. A chord
        // press is left alone so chords keep working (and keep mapping to their own slots) even in
        // the window right after a cast.
        pad.buttons &= !XINPUT_GAMEPAD_Y;
    }

    let held = held_from(pad, pad.buttons & XINPUT_GAMEPAD_Y != 0);
    let y_before = Y_BEFORE.swap(held.buttons & XINPUT_GAMEPAD_Y != 0, Ordering::Relaxed);
    // Which input means what is decided by `nr_casting_core::route`, which is unit-tested; this
    // function only carries out the answer. It is pure and cheap, so it runs on every poll.
    let route = nr_casting_core::route(&held, y_bumper());

    // What this Y chord is ours to handle: nothing at all unless the hand it addresses holds a
    // catalyst (see `chords_owned`). On a catalyst it is always ours, because two-handing a staff
    // is the thing this mod exists to replace — a bound slot casts, an empty one is swallowed.
    let chord = match route {
        Route::Chord {
            right_hand,
            slot,
            label,
        } => chords_owned(right_hand).then_some(ChordPress {
            right_hand,
            spell: bound_spell(right_hand, slot),
            label,
        }),
        _ => None,
    };

    if matches!(chord, Some(press) if press.spell > 0) {
        // The game latches "Y held" and fires the swap on the shoulder press that follows, so the
        // first reports of a chord the game already saw Y held for are released ones: by the time
        // the synthetic shoulder press arrives, Y has been let go often enough that the latch is
        // gone. Two reports, not one — the latch survives a single released frame, which is the
        // "holding Y+combo still two-hands the staff" that survived v0.2.2's single stage.
        let done = CHORD_STAGED.load(Ordering::Relaxed);
        if y_before && done < crate::config::load().chord_stage.min(CHORD_STAGE_REPORTS + 2) {
            CHORD_STAGED.store(done.saturating_add(1), Ordering::Relaxed);
            let which = chord.map(|press| press.label).unwrap_or("");
            debug!("chord: released Y for one report before pressing the shoulder ({which})");
            pad.right_trigger = 0;
            pad.left_trigger = 0;
            pad.buttons &= !(XINPUT_GAMEPAD_Y
                | XINPUT_GAMEPAD_LEFT_SHOULDER
                | XINPUT_GAMEPAD_RIGHT_SHOULDER);
            return;
        }
    } else {
        CHORD_STAGED.store(0, Ordering::Relaxed);
    }

    if !due {
        // Every caller passes its own `XInputState`, so a repeated report still has to be
        // rewritten in place: skipping it handed the game Y+R2 (the chord) and the staff got
        // two-handed instead of a spell cast. This is pad-only work, so it happens before any
        // game-memory check: a world that is not readable yet must not let a chord through.
        if let Some(press) = chord {
            if press.spell > 0 {
                rewrite_cast(pad, press.right_hand, now);
            } else {
                swallow_chord(pad);
            }
        } else if let Route::Cast {
            right_hand, slot, ..
        } = route
        {
            if bound_spell(right_hand, slot) > 0 {
                rewrite_cast(pad, right_hand, now);
            }
        }
        return;
    }

    LAST_RUN_MS.store(now, Ordering::Relaxed);
    crate::stats::pass();

    // Diagnostic: the stance is what the game animates and equips from, and the two-hand symptoms
    // are about it changing when nothing of ours asked for it.
    if let Some(style) = crate::game::arm_style() {
        if LAST_STANCE.swap(style as u8, Ordering::Relaxed) != style as u8 {
            debug!(
                "stance: arm_style -> {style:?} | player buttons={:#06x} rt={} | two_handed={}",
                held.buttons,
                pad.right_trigger,
                catalyst_two_handed()
            );
        }
    }

    // Keeps the panel's "selected" line fresh. Throttled to 100 ms inside, because this runs on
    // every XInput poll and the read is a several-syscall pointer chase.
    sample_player_selection(now);

    // Debug: log the pad on change, not on every pass — a held button at 125 passes a second
    // would fill the 4 MiB log in half a minute.
    let packed =
        (held.buttons as u32) << 16 | (pad.right_trigger as u32) << 8 | pad.left_trigger as u32;
    if packed != LAST_PAD_LOG.swap(packed, Ordering::Relaxed) {
        debug!(
            "pad: buttons={:#06x} rt={} lt={}",
            held.buttons, pad.right_trigger, pad.left_trigger
        );
    }

    // ER's two-hand latch still fires on a catalyst now and then: the game latches "Y held" from
    // frames before our synthesised shoulder press, and staged releases do not reliably clear it.
    // Two-handing a catalyst does nothing but cost the chord that was meant to cast, so it is
    // undone here — for a catalyst only (a two-handed sword is the player's business).
    //
    // Checked on every full pass, unconditionally. Gating it on "Y is up" (or on a chord being in
    // the report) was wrong: the latch fires while Y is held with no button pressed, so the staff
    // stayed in both hands until the player let go — and that is exactly the press they wanted to
    // cast with.
    //
    // What the check costs: `weapon_ids()` is cached for 8 ms (one pointer walk plus two reads
    // when it misses), `chords_owned` reads the cached (hand,slot) mask, and `probe::validated()`
    // is an atomic load. So it is a handful of atomics per pass — ~60-120 times a second, a few
    // microseconds — and the memory write only happens while the game is actually two-handing a
    // catalyst, which stops as soon as the correction lands (`set_one_handed` drops the weapon
    // cache so the next pass sees the corrected state instead of writing again).
    if route == Route::Idle {
        LAST_BIND.store(u32::MAX, Ordering::Relaxed);
        idle(pad, now);
        return;
    }

    // Make the report safe before asking whether the world is readable: a chord that reaches the
    // game while pointers are being re-validated is a two-hand, and the pad edit needs no game
    // memory at all.
    if let Some(press) = chord {
        if press.spell > 0 {
            rewrite_cast(pad, press.right_hand, now);
        } else {
            swallow_chord(pad);
        }
    }

    probe::maybe_validate();
    if !probe::validated() {
        return;
    }

    match route {
        // Not our chord after all (a catalyst-less hand): the game owns this press — two-hand,
        // heavy attack — and nothing of ours is in flight.
        Route::Chord { .. } if chord.is_none() => release_borrowed(),
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
            if matches!(chord, Some(press) if press.spell <= 0) {
                // An owned chord with no spell behind it: swallowed above, nothing to cast.
                release_borrowed();
            } else if !cast_slot(right_hand, slot, label, pad, now) {
                // Nothing bound: a plain input was never rewritten (so the game does its own
                // thing) and an owned chord was already swallowed. Either way no cast is in
                // flight, so any borrowed entry goes back now.
                release_borrowed();
            }
        }
        Route::Idle => {}
    }
}

/// Bind the game's currently selected spell to `slot` of the weapon in one hand.
///
/// Edge-gated: holding the modifier and the button writes once, not once per poll.
fn bind_slot(right: bool, slot: usize, which: &str) -> bool {
    let key = (slot as u32) << 1 | u32::from(right);
    if LAST_BIND.load(Ordering::Relaxed) == key {
        return true; // same press: the report is still swallowed, nothing to write again
    }
    if !bind_now(right, slot, which) {
        return false;
    }
    LAST_BIND.store(key, Ordering::Relaxed);
    true
}

/// Write the game's selected spell into `slot`. Returns whether the gesture was recognised.
///
/// Also the mouse path's entry point (Super + click), which is edge-gated by the raw record
/// itself — only the button-down record reaches here.
pub(crate) fn bind_now(right: bool, slot: usize, which: &str) -> bool {
    if !probe::validated() {
        return true; // recognised and swallowed; there is just nothing to bind yet
    }
    let Some((left_id, right_id)) = weapon_ids() else {
        return false;
    };
    let weapon = if right { right_id } else { left_id };
    if weapon <= 0 {
        return false;
    }
    let Some(spell) = crate::game::selected_spell() else {
        crate::log(&format!(
            "bind: {which} on {weapon} — no spell is selected in the game's own wheel"
        ));
        return true;
    };
    crate::store_set(weapon, slot, spell);
    crate::log(&format!("bind: {which} on {weapon} = spell {spell}"));
    true
}

/// Read the pad into the shape the routing rules take.
///
/// `chord` is "the player is holding something that makes the trigger mean a chord" — Y for the
/// cast chords, the bind modifier for a bind. Either way the game's own chord fires on a much
/// lighter pull than our cast threshold, so any real trigger movement counts.
fn held_from(pad: &XInputGamepad, chord: bool) -> Held {
    let threshold = if chord {
        CHORD_TRIGGER_FLOOR
    } else {
        TRIGGER_THRESHOLD
    };
    Held {
        buttons: pad.buttons,
        right_pull: pad.right_trigger >= threshold,
        left_pull: pad.left_trigger >= threshold,
        right_bump: pad.buttons & XINPUT_GAMEPAD_RIGHT_SHOULDER != 0,
        left_bump: pad.buttons & XINPUT_GAMEPAD_LEFT_SHOULDER != 0,
    }
}

/// Nothing spell-shaped is held: hand back the borrowed entry, take a mouse cast if one is
/// pending, and optionally mirror slot 1 onto the game's own HUD.
///
/// Mirroring forces the wheel selection every frame, which would stop the player cycling spells
/// — so it is suppressed while the panel is open, and only writes when the selection differs.
fn idle(pad: &mut XInputGamepad, now: u32) {
    // Nothing spell-shaped is held, so any borrowed magic entry goes back now. Mouse and keyboard
    // are not polled here at all: they keep their own layer in `rawinput`, which steers the
    // selection and lets the game's own click cast (see that module).
    release_borrowed();
    if UI_OPEN.load(Ordering::Relaxed) {
        return;
    }
    // The mirror is opt-in (`hud_mirror = on` in the ini): forcing the selection every 33 ms
    // fights the player's own wheel and hands a cast the wrong spell. See `config::Config`.
    if !crate::config::load().hud_mirror {
        return;
    }
    // Get out of the way while the player is cycling the wheel. The mirror forces the game's
    // selection back to slot 1, so holding a D-pad direction means the game advances the wheel
    // and the mirror yanks it back — dozens of times a second, which reads as the selection
    // flickering through every spell instead of scrolling. The mirror exists to show what R1 will
    // cast when nobody is choosing; when somebody *is* choosing, they win.
    if pad.buttons & XINPUT_GAMEPAD_DPAD != 0 {
        return;
    }
    // Mirroring is a HUD convenience, not an input path: 30 Hz is plenty, and each pass is a
    // weapon read plus a selection compare (three `VirtualQuery` calls each) on the poll thread.
    if now.wrapping_sub(LAST_MIRROR_MS.load(Ordering::Relaxed)) < 33 {
        return;
    }
    LAST_MIRROR_MS.store(now, Ordering::Relaxed);
    let Some((left_id, right_id)) = weapon_ids() else {
        return;
    };
    if let Some(pair) = store_get(right_id).or_else(|| store_get(left_id)) {
        if pair[0] > 0 && !selection_is(pair[0]) {
            select_spell(pair[0]);
        }
    }
}

/// Cast the spell bound to `slot` of the weapon in `right`'s hand.
///
/// Used by both the pad and the mouse. The trigger is always swallowed and a shoulder press
/// substituted (that is what the game treats as a cast), and Y is cleared so the press reads as
/// a plain cast rather than two-handing or a class port's skill chord.
///
/// The bound-slot answer comes from the cached mask, so an unbound input costs one atomic read:
/// this is called on every pass while a button is held.
fn bound_spell(right: bool, slot: usize) -> i32 {
    let Some((left_id, right_id)) = weapon_ids() else {
        return -1;
    };
    let mask = bound_mask(left_id, right_id);
    let bit = if right {
        slot
    } else {
        nr_casting_core::SLOTS + slot
    };
    if mask & (1 << bit) == 0 {
        return -1;
    }
    bound_on(if right { right_id } else { left_id }, slot)
}

/// The spell bound to `slot` of one weapon id, `-1` when unbound.
fn bound_on(weapon: i32, slot: usize) -> i32 {
    store_get(weapon)
        .map(|s| s.get(slot).copied().unwrap_or(-1))
        .unwrap_or(-1)
}

/// Swallow a Y chord that is ours but has nothing to cast.
///
/// Clearing only the shoulder and the triggers is not enough: the game would still see Y held and
/// a shoulder press in the same report, which is its two-hand command — and on a catalyst that is
/// exactly what the player does not want. A keystroke that does nothing must look like nothing.
fn swallow_chord(pad: &mut XInputGamepad) {
    pad.buttons &= !(XINPUT_GAMEPAD_LEFT_SHOULDER
        | XINPUT_GAMEPAD_RIGHT_SHOULDER
        | XINPUT_GAMEPAD_Y);
    pad.left_trigger = 0;
    pad.right_trigger = 0;
}

/// Turn this report's trigger into the shoulder press the game casts on.
///
/// Pure pad edit, no game memory: it has to happen for *every* caller's report, including the
/// repeats that skip the rest of `adjust`, or the game reads Y+R2 and two-hands the staff.
fn rewrite_cast(pad: &mut XInputGamepad, right: bool, now: u32) {
    pad.right_trigger = 0;
    pad.left_trigger = 0;
    pad.buttons &= !XINPUT_GAMEPAD_Y;
    if right {
        pad.buttons |= XINPUT_GAMEPAD_RIGHT_SHOULDER;
    } else {
        pad.buttons |= XINPUT_GAMEPAD_LEFT_SHOULDER;
    }
    // This press is a swap candidate to the game; hide Y until the candidate expires.
    Y_SUPPRESS_UNTIL.store(now.wrapping_add(Y_SUPPRESS_MS), Ordering::Relaxed);
    trace_wire("cast", pad);
}

/// Log the report we hand over, but only when it changes: a held chord re-sends the same state every
/// poll, and the log is 4 MiB.
fn trace_wire(what: &str, pad: &XInputGamepad) {
    let packed = (pad.buttons as u64) << 16 | (pad.right_trigger as u64) << 8 | pad.left_trigger as u64;
    if LAST_WIRE.swap(packed, Ordering::Relaxed) == packed {
        return;
    }
    debug!(
        "wire: {what} buttons={:#06x} rt={} lt={} | stance {:?} | two_handed={}",
        pad.buttons,
        pad.right_trigger,
        pad.left_trigger,
        crate::game::arm_style(),
        catalyst_two_handed()
    );
}

/// Steer the game's selected spell for a mouse cast, without any pad involved.
///
/// The no-controller path: raw input tells us a mouse button went down, we point the magic entry
/// at the bound spell, and the game's own click then performs the cast. Returns whether a spell
/// was actually steered.
pub(crate) fn steer_mouse(right: bool, slot: usize) -> bool {
    if !probe::validated() {
        return false;
    }
    let bound = bound_spell(right, slot);
    if bound <= 0 {
        return false;
    }
    let steered = select_spell(bound);
    if steered {
        // The game performs this cast from its own click on its next frame; the pad poll would
        // otherwise restore the entry first (every 8 ms), and the click would cast whatever the
        // wheel held instead of the slot that was steered.
        crate::game::hold_borrowed(400);
    }
    steered
}

fn cast_slot(right: bool, slot: usize, which: &str, pad: &mut XInputGamepad, now: u32) -> bool {
    let bound = bound_spell(right, slot);
    if bound <= 0 {
        debug!("input: {which} has no bound spell");
        return false;
    }
    rewrite_cast(pad, right, now);
    // Steering is a write to game memory behind two readability checks and two locks. It is only
    // needed once per press: after that the borrowed entry already holds the spell and the
    // selection already points at it. Redoing it on every poll of a held button was real work at
    // up to 125 Hz that changed nothing — the per-poll cost most likely to be felt as a hitch
    // while casting. Skip it while the borrow and the selection both still agree.
    if crate::game::steered_spell() == bound && selection_is(bound) {
        return true;
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

