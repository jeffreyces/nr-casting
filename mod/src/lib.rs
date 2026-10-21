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
mod params;
mod pointer;
mod probe;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use eldenring::cs::{CSMenuManImp, ChrAsmSlot, WorldChrMan};
use eldenring::util::system::{wait_for_system_init, SystemInitError};
use fromsoftware_shared::{FromStatic, Program};

// XInput button bits (xinput.h).
const XINPUT_GAMEPAD_RIGHT_THUMB: u16 = 0x0080;
const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
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
static STORE: OnceLock<Mutex<HashMap<i32, [i32; 2]>>> = OnceLock::new();

/// A panic while holding the store would otherwise poison it and, with `panic = "abort"`,
/// take the process with it. The data is plain values, so recovering the guard is safe.
fn store_lock() -> std::sync::MutexGuard<'static, HashMap<i32, [i32; 2]>> {
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
    overlay::start(HINSTANCE_USIZE.load(Ordering::Relaxed));
    match install_hook() {
        Ok(addr) => log(&format!("XInputGetState hooked at {addr:p}")),
        Err(e) => {
            log(&format!("HOOK FAILED: {e}"));
            return;
        }
    }
    log(&format!("slots file: {}", slots_path().display()));
    log(&format!("loaded slots: {}", describe_store()));

    // Low-rate readout: mapping state, the player's current spell, slots file changes.
    let mut last_mtime = file_mtime();
    let mut catalogue_ready = false;
    loop {
        probe::maybe_validate();
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
            if parts.len() == 2 {
                map.insert(id, [parts[0], parts[1]]);
            }
        }
    }
    *store_lock() = map;
}

fn save_store(map: &HashMap<i32, [i32; 2]>) {
    let mut out = String::from(
        "# weapon_param_id = bumper_spell, trigger_spell\n\
         # Hold D-pad Up + press R1/R2/L1/L2 in game to overwrite a slot.\n",
    );
    let mut ids: Vec<&i32> = map.keys().collect();
    ids.sort();
    for id in ids {
        let s = map[id];
        out.push_str(&format!("{id} = {}, {}\n", s[0], s[1]));
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

pub(crate) fn store_get(weapon: i32) -> Option<[i32; 2]> {
    store_lock().get(&weapon).copied()
}

pub(crate) fn store_set(weapon: i32, slot: usize, spell: i32) {
    // Snapshot under one lock, then write: dropping the guard first let two assignments
    // interleave and save out of order.
    let snapshot = {
        let mut map = store_lock();
        map.entry(weapon).or_insert([-1, -1])[slot] = spell;
        map.clone()
    };
    save_store(&snapshot);
}

fn install_hook() -> Result<*mut u8, String> {
    unsafe {
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
        let raw = retour::RawDetour::new(addr as *const (), detour as *const ())
            .map_err(|e| format!("RawDetour::new: {e:?}"))?;
        raw.enable().map_err(|e| format!("enable: {e:?}"))?;
        TRAMPOLINE.store(raw.trampoline() as *const () as *mut (), Ordering::Release);
        HOOK_READY.store(true, Ordering::Relaxed);
        std::mem::forget(raw); // process-lifetime state
        Ok(addr)
    }
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
        adjust(&mut *state);
    }
    ret
}

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

    let (spell, which) = match (right_pull, right_bump, left_pull, left_bump) {
        (true, _, _, _) => (store_get(right_id).map(|s| s[1]), "R2"),
        (_, true, _, _) => (store_get(right_id).map(|s| s[0]), "R1"),
        (_, _, true, _) => (store_get(left_id).map(|s| s[1]), "L2"),
        (_, _, _, true) => (store_get(left_id).map(|s| s[0]), "L1"),
        _ => (None, ""),
    };

    let Some(spell) = spell.filter(|s| *s > 0) else {
        return;
    };

    // R2/L2 are not cast inputs: hand the game a shoulder press instead.
    if which == "R2" {
        pad.right_trigger = 0;
        pad.buttons |= XINPUT_GAMEPAD_RIGHT_SHOULDER;
    } else if which == "L2" {
        pad.left_trigger = 0;
        pad.buttons |= XINPUT_GAMEPAD_LEFT_SHOULDER;
    }

    if select_spell(spell) && LAST_INPUT.swap(key, Ordering::Relaxed) != key {
        debug_log(&format!("{which} -> spell {spell}"));
    }
}

/// Steer the game's selection at `spell` through a spare magic entry.
/// Point the game's *selected* entry at `spell`, remembering what was there so it can be
/// restored as soon as the input is released.
///
/// Borrowing the selected entry rather than a spare one matters: a full wheel has no free
/// slot, and permanently displacing somebody's spell risks the game saving that displacement.
/// The tampering window here is one burst of input, and `release_borrowed` closes it on the
/// first poll with no spell input held.
fn borrow_selected(magic: &eldenring::cs::EquipMagicData) -> usize {
    let selected = magic.selected_slot;
    let idx = if (0..magic.entries.len() as i32).contains(&selected) {
        selected as usize
    } else {
        0
    };
    let mut borrowed = BORROWED.lock().unwrap_or_else(|e| e.into_inner());
    if borrowed.is_none() {
        *borrowed = Some(Borrow {
            idx,
            param_id: magic.entries[idx].param_id,
            charges: magic.entries[idx].charges,
        });
    }
    idx
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
    let idx = borrow_selected(magic);
    magic.entries[idx].param_id = spell;
    if magic.entries[idx].charges > 0 {
        magic.entries[idx].charges = -1;
    }
    magic.selected_slot = idx as i32;

    // The cast path also consults the menu selection; keep it in step. Bitfield
    // accessors are private, so the repr(transparent) u32 is edited directly:
    // bits 0..27 = param id, bits 28..31 = category.
    if let Ok(menu) = unsafe { CSMenuManImp::instance_mut() } {
        let item = menu.player_menu_ctrl.selected_magic_item;
        let raw: u32 = unsafe { std::mem::transmute(item) };
        let updated = (raw & 0xF000_0000) | ((spell as u32) & 0x0FFF_FFFF);
        menu.player_menu_ctrl.selected_magic_item = unsafe { std::mem::transmute(updated) };
    }
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
    // OwnedPtr cannot be null, and `as_ref` already answers "is the player loaded".
    Some(wcm.main_player.as_ref()?)
}

pub(crate) fn weapon_ids() -> Option<(i32, i32)> {
    let ptr = player()?.player_game_data;
    // SAFETY: OwnedPtr is non-null by construction and owned by PlayerIns.
    let pgd = unsafe { ptr.as_ref() };
    let ids = &pgd.equipment.chr_asm.equipment_param_ids;
    Some((
        ids[ChrAsmSlot::WeaponLeft1 as usize],
        ids[ChrAsmSlot::WeaponRight1 as usize],
    ))
}

/// INT, FAI, ARC of the loaded character.
pub(crate) fn player_stats() -> Option<(u32, u32, u32)> {
    let ptr = player()?.player_game_data;
    // SAFETY: OwnedPtr is non-null by construction and owned by PlayerIns.
    let pgd = unsafe { ptr.as_ref() };
    Some((pgd.intelligence, pgd.faith, pgd.arcane))
}

/// Param ids in the goods half of the inventory. Spells are goods entries in ER, so
/// this is how the panel learns what the character has picked up.
pub(crate) fn inventory_goods_ids() -> Vec<i32> {
    let Some(player) = player() else {
        return Vec::new();
    };
    let ptr = player.player_game_data;
    // SAFETY: OwnedPtr is non-null by construction and owned by PlayerIns.
    let pgd = unsafe { ptr.as_ref() };
    pgd.equipment
        .equip_inventory_data
        .items_data
        .items()
        .filter(|entry| entry.item_id.category() == eldenring::cs::ItemCategory::Goods)
        .map(|entry| entry.item_id.param_id() as i32)
        .collect()
}

/// Slot count and current selection, for the structural probe.
pub(crate) fn magic_slots() -> Option<(usize, i32)> {
    magic_data().map(|magic| (magic.entries.len(), magic.selected_slot))
}

fn magic_data() -> Option<&'static mut eldenring::cs::EquipMagicData> {
    let mut ptr = player()?.player_game_data;
    // SAFETY: OwnedPtr is non-null by construction and owned by PlayerIns.
    let pgd = unsafe { ptr.as_mut() };
    Some(pgd.equipment.equip_magic_data.as_mut())
}
