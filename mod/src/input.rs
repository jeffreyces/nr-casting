//! The `XInputGetState` hook: chains other mods' hooks instead of replacing them, routes each
//! press to a spell slot, and steers the game's selection for that press.
//!
//! The rules themselves (which input means which slot, and which wins when several are held)
//! live in `nr_casting_core::route` so they are tested; this module carries them out.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use nr_casting_core::{Held, Route};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetForegroundWindow, GetWindowThreadProcessId, CURSORINFO, CURSOR_SHOWING,
};

use crate::game::{
    release_borrowed, sample_player_selection, sample_player_selection_now, select_spell,
    selection_is, weapon_ids, PLAYER_SPELL,
};
use crate::logging::{debug, log};
use crate::store::{store_get, store_set};
use crate::{config, probe, UI_OPEN};

const XINPUT_GAMEPAD_RIGHT_THUMB: u16 = 0x0080;
const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
const XINPUT_GAMEPAD_Y: u16 = 0x8000;
const TRIGGER_THRESHOLD: u8 = 100;
/// With Y held the trigger does not have to be a deliberate pull: the game's own two-hand chord
/// fires well below our cast threshold, so a press still ramping through 40-90 would reach the
/// game as Y+L2 (two-hand) before we ever saw it as a chord.
const CHORD_TRIGGER_FLOOR: u8 = 20;
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

static HOOK_READY: AtomicBool = AtomicBool::new(false);
/// Pad polls seen by our detour. Used to tell "the hook is dead" from "the game is not polling".
pub(crate) static POLLS: AtomicU32 = AtomicU32::new(0);
/// Polls that were not skipped as repeats — the work the gate saved. Reported when debug_log is on.
pub(crate) static PASSES: AtomicU32 = AtomicU32::new(0);
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
static LAST_CAPTURE_KEY: AtomicU32 = AtomicU32::new(u32::MAX);
/// Last pad state we wrote to the debug log, packed buttons|rt|lt, so a held button logs once
/// instead of 125 times a second.
static LAST_PAD_LOG: AtomicU32 = AtomicU32::new(u32::MAX);
/// Last HUD-mirror pass, in milliseconds (see `idle`).
static LAST_MIRROR_MS: AtomicU32 = AtomicU32::new(0);
/// When on, idle polls mirror the weapon's slot 1 into the selection so the game's
/// own spell HUD shows what is bound. Locks the D-pad wheel while on (see README).
pub(crate) static HUD_MIRROR: AtomicBool = AtomicBool::new(false);

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
                Ok(target)
            }
        }
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
    let mut last_polls = POLLS.load(Ordering::Relaxed);
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
        if !crate::probe::validated() {
            // Nothing to protect before the world exists: the title screen and loading screens
            // legitimately poll nothing, and re-patching there is pure risk.
            continue;
        }

        let polls = POLLS.load(Ordering::Relaxed);
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
            log(
                "input: no pad polls received — another mod is intercepting XInput, so the \
                 trigger/ash rewriting cannot work until it is removed or disabled.",
            );
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
    let tramp = TRAMPOLINE.load(Ordering::Acquire);
    if tramp.is_null() {
        return 1167; // ERROR_DEVICE_NOT_CONNECTED
    }
    let original: unsafe extern "system" fn(u32, *mut XInputState) -> u32 =
        std::mem::transmute(tramp);
    let ret = original(user, state);
    if ret == 0 && !state.is_null() && HOOK_READY.load(Ordering::Relaxed) {
        let n = POLLS.fetch_add(1, Ordering::Relaxed) + 1;
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
    // Buttons first: no click means no routing, and this runs on every idle poll. Asking for the
    // foreground window and the cursor costs two more user32 calls (server round-trips under
    // Wine) that only matter once a click is actually down.
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
    let primary = down(VK_LBUTTON);
    let secondary = down(VK_RBUTTON);
    if !primary && !secondary {
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
    nr_casting_core::mouse_route(
        primary,
        secondary,
        down(VK_SHIFT),
        down(VK_CONTROL),
    )
}

const VK_LBUTTON: i32 = 0x01;
const VK_RBUTTON: i32 = 0x02;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;

/// Milliseconds since the epoch, truncated to 32 bits. Interval tests use wrapping subtraction,
/// so the truncation is harmless and the whole thing is one `clock_gettime` (vDSO) call.
fn millis() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0)
}

fn adjust(state: &mut XInputState) {
    let now = millis();
    let due = state.packet_number != LAST_PACKET.load(Ordering::Relaxed)
        || now.wrapping_sub(LAST_RUN_MS.load(Ordering::Relaxed)) >= POLL_INTERVAL_MS;
    LAST_PACKET.store(state.packet_number, Ordering::Relaxed);

    let pad = &mut state.gamepad;
    let held = held_from(pad);
    // Which input means what is decided by `nr_casting_core::route`, which is unit-tested; this
    // function only carries out the answer. It is pure and cheap, so it runs on every poll.
    let route = nr_casting_core::route(&held, config::load().modifier);

    if !due {
        // Every caller passes its own `XInputState`, so a repeated report still has to be
        // rewritten in place: skipping it handed the game Y+R2 (the chord) and the staff got
        // two-handed instead of a spell cast. Only the game-memory work below is worth gating.
        if !probe::validated() {
            return;
        }
        match route {
            Route::Cast {
                right_hand, slot, ..
            }
            | Route::Chord {
                right_hand, slot, ..
            } => {
                if bound_spell(right_hand, slot) > 0 {
                    rewrite_cast(pad, right_hand);
                }
            }
            Route::AshOfWar => pad.buttons &= !XINPUT_GAMEPAD_RIGHT_THUMB,
            // Binding must not also attack: swallow the same inputs the capturing pass did.
            Route::Capture { .. } => swallow_spell_inputs(pad),
            _ => {}
        }
        return;
    }

    LAST_RUN_MS.store(now, Ordering::Relaxed);
    PASSES.fetch_add(1, Ordering::Relaxed);

    // Keeps the panel's "selected" line fresh. Throttled to 100 ms inside, because this runs on
    // every XInput poll and the read is a several-syscall pointer chase.
    sample_player_selection();

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
            // Bind the spell selected *now*: the throttled sample can be up to 100 ms old and
            // the player may have flicked the wheel since.
            sample_player_selection_now();
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
    // While Y is held, treat any real trigger movement as the chord. The game's two-hand fires on
    // a much lighter pull than our cast threshold, so waiting for 100 let the first frames of a
    // press through as Y+L2 and the staff got two-handed.
    let threshold = if pad.buttons & XINPUT_GAMEPAD_Y != 0 {
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
    // Mirroring is a HUD convenience, not an input path: 30 Hz is plenty, and each pass is a
    // weapon read plus a selection compare (three `VirtualQuery` calls each) on the poll thread.
    let now = millis();
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
fn bound_spell(right: bool, slot: usize) -> i32 {
    let Some((left_id, right_id)) = weapon_ids() else {
        return -1;
    };
    let weapon = if right { right_id } else { left_id };
    store_get(weapon)
        .map(|s| s.get(slot).copied().unwrap_or(-1))
        .unwrap_or(-1)
}

/// Turn this report's trigger into the shoulder press the game casts on.
///
/// Pure pad edit, no game memory: it has to happen for *every* caller's report, including the
/// repeats that skip the rest of `adjust`, or the game reads Y+R2 and two-hands the staff.
fn rewrite_cast(pad: &mut XInputGamepad, right: bool) {
    pad.right_trigger = 0;
    pad.left_trigger = 0;
    pad.buttons &= !XINPUT_GAMEPAD_Y;
    if right {
        pad.buttons |= XINPUT_GAMEPAD_RIGHT_SHOULDER;
    } else {
        pad.buttons |= XINPUT_GAMEPAD_LEFT_SHOULDER;
    }
}

fn cast_slot(right: bool, slot: usize, which: &str, pad: &mut XInputGamepad) -> bool {
    let bound = bound_spell(right, slot);
    if bound <= 0 {
        debug!("input: {which} has no bound spell");
        return false;
    }
    rewrite_cast(pad, right);
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

