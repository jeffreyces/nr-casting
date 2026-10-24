//! Two spells per catalyst: L1/L2 for the left weapon, R1/R2 for
//! the right — with in-game capture so you bind the spells yourself.
//!
//! Elden Ring already casts on **R1 and L1** with a catalyst in that hand (it casts
//! the *selected* spell). **R2** (heavy attack) and **L2** (ash of war) do not cast,
//! so those two reports are rewritten: clear the trigger, set the shoulder bit, and
//! the game performs its normal cast with vanilla FP checks and animations.
//!
//! On every press the chosen spell is steered into a spare magic entry and
//! `selected_slot` is pointed at it, which is what makes four different spells work
//! off one selection.
//!
//! **Ash of war:** hold R3 while pressing L2 and the trigger is passed through unchanged,
//! so the weapon skill fires exactly as it does without the mod.
//!
//! **Capture mode:** hold D-pad Up and press the input you want to bind (R1/R2/L1/L2).
//! Whatever spell is currently selected in the game is written to that weapon's slot
//! and saved to `nr_casting_slots.txt` beside eldenring.exe:
//!
//! ```text
//! # weapon_param_id = bumper_spell, trigger_spell
//! 33240009 = 4710, 4200
//! 33090010 = 4001, 4830
//! ```
//!
//! The file is the store the param-backed version will read too; edit it by hand or
//! let capture write it.

mod config;
mod overlay;
mod window;
mod params;
mod pointer;
mod probe;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use eldenring::cs::{ChrAsmSlot, WorldChrMan};
use eldenring::util::system::{wait_for_system_init, SystemInitError};
use fromsoftware_shared::{FromStatic, Program};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorInfo, GetForegroundWindow, GetWindowThreadProcessId, CURSORINFO, CURSOR_SHOWING,
};

// XInput button bits (xinput.h).
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

static LOG_PATH: OnceLock<std::path::PathBuf> = OnceLock::new();
/// Open once and reused: opening/closing per line cost a syscall pair per cast.
static LOG_FILE: OnceLock<Mutex<Option<std::fs::File>>> = OnceLock::new();
static LOG_WRITES: AtomicU32 = AtomicU32::new(0);
static DEBUG_LOG: AtomicBool = AtomicBool::new(false);
/// Rotate at 4 MiB so a long session cannot grow the file without bound.
const LOG_LIMIT: u64 = 4 * 1024 * 1024;
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
/// Magic-entry index pinning the steered spell. Cached: once written the entry is no
/// longer "free", so re-finding it each press would consume all spare slots.
/// The entry we borrowed for the current cast burst: its index and original contents, so it
/// can be put back the moment no spell input is held.
struct Borrow {
    idx: usize,
    param_id: i32,
    charges: i32,
}

static BORROWED: Mutex<Option<Borrow>> = Mutex::new(None);
/// The spell the *player* has selected (sampled between our writes) — the value
/// capture binds. -1 until seen.
pub(crate) static PLAYER_SPELL: AtomicI32 = AtomicI32::new(-1);
/// When on, idle polls mirror the weapon's slot 1 into the selection so the game's
/// own spell HUD shows what is bound. Locks the D-pad wheel while on (see README).
pub(crate) static HUD_MIRROR: AtomicBool = AtomicBool::new(false);
/// True while the assignment panel is open. Mirroring is suppressed then, because it forces
/// the wheel selection every frame and capture needs the player to be able to cycle it.
pub(crate) static UI_OPEN: AtomicBool = AtomicBool::new(false);
/// weapon param id -> [bumper spell, trigger spell]
static STORE: OnceLock<Mutex<HashMap<i32, [i32; 3]>>> = OnceLock::new();

/// A panic while holding the store would otherwise poison it and, with `panic = "abort"`,
/// take the process with it. The data is plain values, so recovering the guard is safe.
fn store_lock() -> std::sync::MutexGuard<'static, HashMap<i32, [i32; 3]>> {
    STORE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Our own module handle, captured in `DllMain`.
pub(crate) fn module_handle() -> usize {
    HINSTANCE_USIZE.load(Ordering::Relaxed)
}

/// All mod files live beside the DLL (see `config`), so a package is self-contained.
fn game_dir_file(name: &str) -> std::path::PathBuf {
    config::path(name)
}

fn log_path() -> &'static std::path::Path {
    LOG_PATH.get_or_init(|| game_dir_file("nr_casting_probe.log"))
}

fn slots_path() -> std::path::PathBuf {
    game_dir_file("nr_casting_slots.txt")
}

/// Detailed per-cast chatter; enabled with `debug_log = on` in the slots file.
pub(crate) fn debug_log(msg: &str) {
    if DEBUG_LOG.load(Ordering::Relaxed) {
        log(msg);
    }
}

pub(crate) fn log(msg: &str) {
    use std::io::Write;
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let slot = LOG_FILE.get_or_init(|| Mutex::new(None));

    // Size check costs a syscall, so only pay for it occasionally.
    if LOG_WRITES.fetch_add(1, Ordering::Relaxed) % 256 == 0
        && std::fs::metadata(log_path()).map(|m| m.len()).unwrap_or(0) > LOG_LIMIT
    {
        let _ = std::fs::rename(log_path(), game_dir_file("nr_casting_probe.old.log"));
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    let mut guard = slot.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
            .ok();
    }
    if let Some(handle) = guard.as_mut() {
        let _ = handle.write_all(format!("[{ms}] {msg}\n").as_bytes());
    }
}

static HINSTANCE_USIZE: AtomicUsize = AtomicUsize::new(0);

#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged_pool: usize,
    quota_paged_pool: usize,
    quota_peak_non_paged_pool: usize,
    quota_non_paged_pool: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private: usize,
}

extern "system" {
    fn K32GetProcessMemoryInfo(process: *mut u8, counters: *mut ProcessMemoryCounters, size: u32) -> i32;
}

/// (working set MiB, pagefile usage MiB) — logged on the heartbeat so growth over a session
/// is visible rather than argued about. PrivateUsage is left at zero under Wine, so
/// PagefileUsage is used instead: it is the RAM-backed figure that matters for a leak.
fn memory_mib() -> Option<(usize, usize)> {
    let mut counters = ProcessMemoryCounters {
        cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set: 0,
        working_set: 0,
        quota_peak_paged_pool: 0,
        quota_paged_pool: 0,
        quota_peak_non_paged_pool: 0,
        quota_non_paged_pool: 0,
        pagefile: 0,
        peak_pagefile: 0,
        private: 0,
    };
    // SAFETY: -1 is the current-process pseudo handle; the struct is repr(C) and sized.
    let ok = unsafe { K32GetProcessMemoryInfo(-1isize as *mut u8, &mut counters, counters.cb) };
    if ok == 0 {
        None
    } else {
        Some((
            counters.working_set / 1_048_576,
            counters.pagefile / 1_048_576,
        ))
    }
}

#[no_mangle]
pub extern "system" fn DllMain(hinst: *mut u8, reason: u32, _reserved: *mut u8) -> i32 {
    if reason == 1 {
        HINSTANCE_USIZE.store(hinst as usize, Ordering::Relaxed);
        std::thread::spawn(run);
    }
    1
}

fn run() {
    config::migrate_from_game_dir();

    // Derived files belong to one executable build. If the game changed, drop them before the
    // log handle is opened: the catalogue is re-read from the live params on the next panel
    // open, and the log starts clean instead of carrying lines from a different build.
    let (hash, size) = probe::exe_fingerprint();
    let stamp = format!("{hash:#x} {size}");
    let marker = config::path("nr_casting_build.txt");
    let previous = std::fs::read_to_string(&marker).unwrap_or_default();
    let build_changed = previous.trim() != stamp;
    if build_changed {
        let _ = std::fs::remove_file(config::path("nr_casting_catalogue.txt"));
        let _ = std::fs::remove_file(game_dir_file("nr_casting_probe.log"));
        let _ = std::fs::remove_file(game_dir_file("nr_casting_probe.old.log"));
    }
    let _ = std::fs::write(&marker, format!("{stamp}\n"));


    let cfg = config::load();
    log(&format!(
        "=== nr_casting loaded ({} opens the panel; hold {} + R1/R2/L1/L2 to bind)",
        cfg.hotkey_name, cfg.modifier_name
    ));
    log(&format!(
        "config: hud_mirror={} debug_log={} files in {}",
        cfg.hud_mirror,
        cfg.debug_log,
        config::dir().display()
    ));
    HUD_MIRROR.store(cfg.hud_mirror, Ordering::Relaxed);
    DEBUG_LOG.store(cfg.debug_log, Ordering::Relaxed);
    if build_changed {
        log("build: executable changed — cleared the cached catalogue and previous log");
    }

    let program = Program::current();
    match wait_for_system_init(&program, Duration::from_secs(60)) {
        Ok(()) => log("system init OK (Ww2720 table resolved)"),
        Err(SystemInitError::InvalidRva) => {
            log("RVA TABLE INVALID for this exe — aborting");
            return;
        }
        Err(e) => {
            log(&format!("system init failed: {e}"));
            return;
        }
    }

    seed_store();
    probe::resolve_table();
    window::start();
    std::thread::spawn(watch_hook);
    match install_hook() {
        Ok(addr) => log(&format!("XInputGetState hooked at {addr:p}")),
        Err(e) => {
            log(&format!("HOOK FAILED: {e}"));
            return;
        }
    }
    log_foreign_modules();
    log(&format!("files in: {}", config::data_dir().display()));
    log(&format!("slots file: {}", slots_path().display()));
    log(&format!("loaded slots: {}", describe_store()));

    // Low-rate readout: mapping state, the player's current spell, slots file changes.
    let mut last_mtime = file_mtime();
    let mut catalogue_ready = false;
    let mut world_was_live = false;
    loop {
        probe::maybe_validate();
        // A reload destroys the character and builds a new one. Logging the edge is what tells
        // us whether a freeze happened while we were holding game memory.
        let world_live = player().is_some();
        if world_live != world_was_live {
            world_was_live = world_live;
            if world_live {
                log("world: character present");
            } else {
                log("world: character gone (reload or quit to menu)");
                release_borrowed();
            }
        }
        // Build the spell catalogue as soon as the regulation is loaded, so the generated file
        // exists whether or not the player ever opens the panel.
        if !catalogue_ready {
            catalogue_ready = overlay::ensure_catalogue().is_some();
        }
        if let Some((l, r)) = weapon_ids() {
            let memory = memory_mib()
                .map(|(ws, pf)| format!(" | mem ws={ws}MiB pf={pf}MiB"))
                .unwrap_or_default();
            log(&format!(
                "weapons L1={l} R1={r} | player spell {} | slots [{}]{}",
                PLAYER_SPELL.load(Ordering::Relaxed),
                describe_store(),
                memory
            ));
        }
        let m = file_mtime();
        if m != last_mtime && m.is_some() {
            last_mtime = m;
            reload_store();
            log(&format!("slots reloaded: {}", describe_store()));
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

fn file_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(slots_path()).ok().and_then(|m| m.modified().ok())
}

fn seed_store() {
    if !slots_path().exists() {
        // Empty on purpose: a release must not ship somebody else's weapons.
        let defaults = "# weapon_param_id = bumper_spell, trigger_spell\n\
                        # Hold the bind modifier + press R1/R2/L1/L2 in game to bind a spell.\n";
        let _ = std::fs::write(slots_path(), defaults);
        log("created default slots file");
    }
    reload_store();
}

fn reload_store() {
    let mut map = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(slots_path()) {
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            if line.starts_with("hud_mirror") || line.starts_with("debug_log") {
                log("slots: hud_mirror/debug_log moved to nr_casting.ini — ignored here");
                continue;
            }
            let Some((id, spells)) = line.split_once('=') else { continue };
            let Ok(id) = id.trim().parse::<i32>() else { continue };
            let parts: Vec<i32> = spells
                .split(',')
                .filter_map(|s| s.trim().parse::<i32>().ok())
                .collect();
            if parts.len() >= 2 {
                // Older files carry two entries; the third (Y + trigger) defaults to unbound.
                let third = parts.get(2).copied().unwrap_or(-1);
                map.insert(id, [parts[0], parts[1], third]);
            }
        }
    }
    *store_lock() = map;
}

fn save_store(map: &HashMap<i32, [i32; 3]>) {
    let mut out = String::from(
        "# weapon_param_id = bumper_spell, trigger_spell, y_chord_spell\n\
         # y_chord_spell is cast by holding Y and pulling the trigger: Y+R2 (right weapon)\n\
         # and Y+L2 (left weapon). Two-entry lines from older versions are still accepted.\n",
    );
    let mut ids: Vec<&i32> = map.keys().collect();
    ids.sort();
    for id in ids {
        let s = map[id];
        out.push_str(&format!("{id} = {}, {}, {}\n", s[0], s[1], s[2]));
    }
    if let Err(e) = std::fs::write(slots_path(), out) {
        log(&format!("slots: FAILED to write {}: {e}", slots_path().display()));
    }
}

fn describe_store() -> String {
    let map = store_lock();
    let mut ids: Vec<&i32> = map.keys().collect();
    ids.sort();
    ids.iter()
        .map(|id| format!("{id}={:?}", map[id]))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn store_get(weapon: i32) -> Option<[i32; 3]> {
    store_lock().get(&weapon).copied()
}

pub(crate) fn store_set(weapon: i32, slot: usize, spell: i32) {
    // Snapshot under one lock, then write: dropping the guard first let two assignments
    // interleave and save out of order.
    let snapshot = {
        let mut map = store_lock();
        let entry = map.entry(weapon).or_insert([-1, -1, -1]);
        if slot < entry.len() {
            entry[slot] = spell;
        }
        map.clone()
    };
    save_store(&snapshot);
}

fn install_hook() -> Result<*mut u8, String> {
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
fn watch_hook() {
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
        if n == 1 || n % 600 == 0 {
            debug_log(&format!("detour: poll #{n} (user {user}, buttons={:#06x})", unsafe { (*state).gamepad.buttons }));
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
    // hides and captures it. Clicking there belongs to the game's UI — synthesising a pad
    // press for it turned "Exit" into an attack/confirm, so the cursor decides.
    let mut cursor = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetCursorInfo(&mut cursor) }.is_ok() && cursor.flags.0 & CURSOR_SHOWING.0 != 0 {
        return None;
    }
    let down = |vk: i32| unsafe { GetAsyncKeyState(vk) } < 0;
    let (left_click, right_click) = (down(VK_LBUTTON), down(VK_RBUTTON));
    let (shift, ctrl) = (down(VK_SHIFT), down(VK_CONTROL));
    match (left_click, right_click) {
        (true, false) => Some(if ctrl {
            (true, 2, "ctrl + LMB")
        } else if shift {
            (true, 1, "shift + LMB")
        } else {
            (true, 0, "LMB")
        }),
        (false, true) => Some(if ctrl {
            (false, 2, "ctrl + RMB")
        } else if shift {
            (false, 1, "shift + RMB")
        } else {
            (false, 0, "RMB")
        }),
        _ => None,
    }
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
    let buttons = pad.buttons;
    let right_pull = pad.right_trigger >= TRIGGER_THRESHOLD;
    let left_pull = pad.left_trigger >= TRIGGER_THRESHOLD;
    let right_bump = buttons & XINPUT_GAMEPAD_RIGHT_SHOULDER != 0;
    let left_bump = buttons & XINPUT_GAMEPAD_LEFT_SHOULDER != 0;

    if right_pull || left_pull || right_bump || left_bump {
        debug_log(&format!(
            "pad: buttons={:#06x} rt={} lt={}",
            buttons, pad.right_trigger, pad.left_trigger
        ));
    }

    // Ash of war: R3 + L2 hands the trigger back to the game untouched. L2 is ours on a
    // catalyst (it casts slot 2), which left the skill with no input at all; holding R3 for
    // that press restores vanilla L2, so the ash of war on the weapon still fires.
    if buttons & XINPUT_GAMEPAD_RIGHT_THUMB != 0 && left_pull {
        // R3 is the combo's modifier here, so hide it from the game (it would otherwise toggle
        // lock-on mid-skill) while leaving L2 exactly as the pad reported it.
        pad.buttons &= !XINPUT_GAMEPAD_RIGHT_THUMB;
        release_borrowed();
        return;
    }

    if !(right_pull || left_pull || right_bump || left_bump) {
        if let Some((right, slot, which)) = mouse_combo() {
            probe::maybe_validate();
            if probe::validated() {
                cast_slot(right, slot, which, pad);
            }
            return;
        }
        // No spell input held: hand the borrowed entry back before anything else reads it.
        release_borrowed();
        // Idle: optionally show the right weapon's slot-1 spell on the game's own HUD.
        // Mirroring forces the wheel selection every frame, which would stop the player
        // cycling spells — so it is suppressed while the panel is open, and only writes
        // when the selection actually differs.
        if HUD_MIRROR.load(Ordering::Relaxed) && !UI_OPEN.load(Ordering::Relaxed) {
            if let Some((left_id, right_id)) = weapon_ids() {
                if let Some(pair) = store_get(right_id).or_else(|| store_get(left_id)) {
                    if pair[0] > 0 && !selection_is(pair[0]) {
                        select_spell(pair[0]);
                    }
                }
            }
        }
        return;
    }

    probe::maybe_validate();
    if !probe::validated() {
        return;
    }
    let Some((left_id, right_id)) = weapon_ids() else {
        debug_log("input: no weapon ids (player data unreadable)");
        return;
    };

    // Capture: the configured modifier held + one of the four inputs = bind the selected spell.
    let modifier = config::load().modifier;
    if buttons & modifier != 0 {
        let target = if right_pull {
            Some((right_id, 1, "R2"))
        } else if right_bump {
            Some((right_id, 0, "R1"))
        } else if left_pull {
            Some((left_id, 1, "L2"))
        } else if left_bump {
            Some((left_id, 0, "L1"))
        } else {
            None
        };
        if let Some((weapon, slot, which)) = target {
            let key = (weapon as u32) << 8 | slot as u32;
            if LAST_CAPTURE_KEY.swap(key, Ordering::Relaxed) != key {
                let spell = PLAYER_SPELL.load(Ordering::Relaxed);
                if spell > 0 {
                    store_set(weapon, slot, spell);
                    log(&format!("capture: {which} on {weapon} = spell {spell}"));
                } else {
                    log(&format!("capture: {which} on {weapon} — no spell selected yet"));
                }
            }
            // Swallow the press so binding never also casts.
            pad.buttons &= !(XINPUT_GAMEPAD_LEFT_SHOULDER
                | XINPUT_GAMEPAD_RIGHT_SHOULDER
                | modifier);
            pad.left_trigger = 0;
            pad.right_trigger = 0;
            return;
        }
    }

    let key = (buttons as u32) << 16 | (pad.right_trigger as u32) << 8 | pad.left_trigger as u32;

    // Y + trigger: a fourth and fifth slot per catalyst. Two-handing is meaningless on a staff,
    // which is what Y+RB/Y+LB would otherwise do, so the chord is spent on a spell instead.
    //
    // Under Y the trigger is never rewritten into a shoulder press on its own: Recluse reads
    // Y+RB (0x8200) and Y+LB (0x8100) as its own skills. `cast_slot` clears Y for that press,
    // so the game still gets a plain cast while the port (chained beneath us) keeps seeing the
    // raw Y + trigger, which is no chord of theirs. With nothing bound the press passes through.
    if buttons & XINPUT_GAMEPAD_Y != 0 && (right_pull || left_pull) {
        let which = if right_pull { "Y+R2" } else { "Y+L2" };
        if cast_slot(right_pull, 2, which, pad) {
            return;
        }
        debug_log(&format!("input: {which} has no bound spell — trigger passed through"));
        release_borrowed();
        return;
    }

    let (right, slot, which) = match (right_pull, right_bump, left_pull, left_bump) {
        (true, _, _, _) => (true, 1, "R2"),
        (_, true, _, _) => (true, 0, "R1"),
        (_, _, true, _) => (false, 1, "L2"),
        (_, _, _, true) => (false, 0, "L1"),
        _ => return,
    };
    cast_slot(right, slot, which, pad);
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
        debug_log(&format!("input: {which} has no bound spell"));
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
        debug_log(&format!("input: {which} bound to {bound} but magic data unreadable"));
        return false;
    }
    let key = (slot as u32) << 1 | u32::from(right);
    if LAST_INPUT.swap(key, Ordering::Relaxed) != key {
        debug_log(&format!("{which} -> spell {bound}"));
    }
    true
}

/// Steer the game's selection at `spell` through a spare magic entry.
/// Point the game's *selected* entry at `spell`, remembering what was there so it can be
/// restored as soon as the input is released.
///
/// Borrowing the selected entry rather than a spare one matters: a full wheel has no free
/// slot, and permanently displacing somebody's spell risks the game saving that displacement.
/// The tampering window here is one burst of input, and `release_borrowed` closes it on the
/// first poll with no spell input held.
fn borrow_selected(magic: &eldenring::cs::EquipMagicData) -> Option<usize> {
    // No slots means nothing to steer (seen for a frame during a teardown); `entries[0]` would
    // panic, and this crate is panic=abort, so a panic here kills the game.
    if magic.entries.is_empty() {
        return None;
    }
    let selected = magic.selected_slot;
    let idx = if (0..magic.entries.len() as i32).contains(&selected) {
        selected as usize
    } else {
        0
    };
    let mut borrowed = BORROWED.lock().unwrap_or_else(|e| e.into_inner());
    if borrowed.is_none() {
        let entry = &magic.entries[idx];
        *borrowed = Some(Borrow {
            idx,
            param_id: entry.param_id,
            charges: entry.charges,
        });
    }
    Some(idx)
}

/// Put back whatever `borrow_selected` overwrote. Cheap enough to call on every idle poll.
pub(crate) fn release_borrowed() {
    let snapshot = {
        let mut slot = BORROWED.lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    let Some(snapshot) = snapshot else {
        return;
    };
    if let Some(magic) = magic_data() {
        if snapshot.idx < magic.entries.len() {
            magic.entries[snapshot.idx].param_id = snapshot.param_id;
            magic.entries[snapshot.idx].charges = snapshot.charges;
        }
    }
}

/// Does the selection already point at `spell`?
fn selection_is(spell: i32) -> bool {
    let Some(magic) = magic_data() else {
        return false;
    };
    let selected = magic.selected_slot;
    (0..magic.entries.len() as i32).contains(&selected)
        && magic.entries[selected as usize].param_id == spell
}

fn select_spell(spell: i32) -> bool {
    let Some(magic) = magic_data() else {
        return false;
    };
    let Some(idx) = borrow_selected(magic) else {
        return false;
    };
    let Some(entry) = magic.entries.get_mut(idx) else {
        return false;
    };
    entry.param_id = spell;
    if entry.charges > 0 {
        entry.charges = -1;
    }
    magic.selected_slot = idx as i32;
    // The menu's `selected_magic_item` is deliberately left alone: the cast reads
    // `EquipMagicData`, that field is a display cache (it sat at the "no item" sentinel while
    // casting worked fine), and writing it is one more piece of shared state to fight another
    // mod over — class-port mods drive that exact field.
    true
}

/// Sample the spell the player has selected, i.e. anything that is not our scratch
/// entry. Called from the poll path so capture always sees a fresh value.
pub(crate) fn sample_player_selection() {
    let Some(magic) = magic_data() else { return };
    let sel = magic.selected_slot;
    if sel < 0 {
        return;
    }
    let sel = sel as usize;
    if sel >= magic.entries.len() {
        return;
    }
    // While a borrow is active the entry holds our steered spell, not the player's choice.
    if BORROWED.lock().map(|b| b.is_some()).unwrap_or(false) {
        return;
    }
    let id = magic.entries[sel].param_id;
    if id > 0 {
        PLAYER_SPELL.store(id, Ordering::Relaxed);
    }
}

fn player() -> Option<&'static eldenring::cs::PlayerIns> {
    let wcm = unsafe { WorldChrMan::instance() }.ok()?;
    let owned = wcm.main_player.as_ref()?;
    let ptr = owned.as_ptr();
    // The pointer having a value does not mean the character exists any more: a reload frees
    // it. Anything unreadable is treated exactly like "no player".
    if !probe::readable(ptr as *const u8, std::mem::size_of::<eldenring::cs::PlayerIns>()) {
        return None;
    }
    Some(unsafe { &*ptr })
}

/// The player's game data, guarded the same way. Every accessor below goes through this.
fn game_data() -> Option<&'static mut eldenring::cs::PlayerGameData> {
    let player = player()?;
    let ptr = player.player_game_data.as_ptr();
    if !probe::readable(ptr as *const u8, std::mem::size_of::<eldenring::cs::PlayerGameData>()) {
        return None;
    }
    // SAFETY: readable, owned by PlayerIns, and re-derived on every call.
    Some(unsafe { &mut *ptr })
}

pub(crate) fn weapon_ids() -> Option<(i32, i32)> {
    let pgd = game_data()?;
    let ids = &pgd.equipment.chr_asm.equipment_param_ids;
    Some((
        ids[ChrAsmSlot::WeaponLeft1 as usize],
        ids[ChrAsmSlot::WeaponRight1 as usize],
    ))
}

/// INT, FAI, ARC of the loaded character.
pub(crate) fn player_stats() -> Option<(u32, u32, u32)> {
    let pgd = game_data()?;
    Some((pgd.intelligence, pgd.faith, pgd.arcane))
}

/// Param ids in the goods half of the inventory. Spells are goods entries in ER, so
/// this is how the panel learns what the character has picked up.
pub(crate) fn inventory_goods_ids() -> Vec<i32> {
    let Some(pgd) = game_data() else {
        return Vec::new();
    };
    pgd.equipment
        .equip_inventory_data
        .items_data
        .items()
        // The list is rebuilt on load; cap the walk so a stale length cannot spin through
        // garbage. Real inventories are a few hundred entries.
        .take(4096)
        .filter(|entry| entry.item_id.category() == eldenring::cs::ItemCategory::Goods)
        .map(|entry| entry.item_id.param_id() as i32)
        .collect()
}

/// Slot count and current selection, for the structural probe.
pub(crate) fn magic_slots() -> Option<(usize, i32)> {
    magic_data().map(|magic| (magic.entries.len(), magic.selected_slot))
}

fn magic_data() -> Option<&'static mut eldenring::cs::EquipMagicData> {
    let pgd = game_data()?;
    let ptr = pgd.equipment.equip_magic_data.as_ptr();
    // OwnedPtr derefs without a check; during a teardown this is exactly the pointer that can be
    // released while it still looks valid.
    if !probe::readable(ptr as *const u8, std::mem::size_of::<eldenring::cs::EquipMagicData>()) {
        return None;
    }
    // SAFETY: readable, owned by the player's game data, re-derived on every call.
    Some(unsafe { &mut *ptr })
}

/// Log the DLLs in this process that are neither Windows' nor ours.
///
/// Every compatibility report so far reduces to "which other DLL is loaded": proxy DLLs for
/// frame generation, another overlay, a class-port mod. Naming them in the log turns that from
/// a guessing game into a line somebody can paste into a bug report.
fn foreign_modules() -> Option<Vec<String>> {
    extern "system" {
        fn GetCurrentProcess() -> *mut u8;
        fn K32EnumProcessModules(p: *mut u8, modules: *mut *mut u8, size: u32, needed: *mut u32)
            -> i32;
        fn GetModuleFileNameW(module: *mut u8, buffer: *mut u16, size: u32) -> u32;
    }

    let mut modules = [std::ptr::null_mut::<u8>(); 512];
    let mut needed = 0u32;
    // SAFETY: fixed buffer; `needed` gets the true byte size, which is range-checked below.
    let ok = unsafe {
        K32EnumProcessModules(
            GetCurrentProcess(),
            modules.as_mut_ptr(),
            (modules.len() * std::mem::size_of::<*mut u8>()) as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }
    let count = (needed as usize / std::mem::size_of::<*mut u8>()).min(modules.len());

    let own = config::dir().to_string_lossy().to_ascii_lowercase();
    let mut foreign: Vec<String> = Vec::new();
    for module in modules.iter().take(count) {
        let mut buffer = [0u16; 1024];
        // SAFETY: handle from the enumeration; buffer is sized for MAX_PATH comfortably.
        let len = unsafe { GetModuleFileNameW(*module, buffer.as_mut_ptr(), buffer.len() as u32) };
        if len == 0 {
            continue;
        }
        use std::os::windows::ffi::OsStringExt;
        let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len as usize]));
        let text = path.to_string_lossy().to_ascii_lowercase();
        if text.contains("\\windows\\") || text.starts_with(&own) {
            continue;
        }
        if let Some(name) = path.file_name() {
            foreign.push(name.to_string_lossy().to_string());
        }
    }
    foreign.sort();
    foreign.dedup();
    Some(foreign)
}

fn log_foreign_modules() {
    match foreign_modules() {
        Some(modules) if !modules.is_empty() => log(&format!("modules: {}", modules.join(", "))),
        Some(_) => log("modules: no foreign DLLs loaded"),
        None => log("modules: enumeration failed"),
    }
}
