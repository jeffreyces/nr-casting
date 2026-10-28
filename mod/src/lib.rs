//! Up to four spells per catalyst: the bumper and trigger of each hand, a third on the Y+trigger
//! chord, and a fourth on the Y+bumper chord when Recluse is not using it.
//!
//! Elden Ring already casts on **R1 and L1** with a catalyst in that hand (it casts the
//! *selected* spell). **R2** (heavy attack) and **L2** (ash of war) do not cast, so those two
//! reports are rewritten: clear the trigger, set the shoulder bit, and the game performs its
//! normal cast with vanilla FP checks and animations. Holding **Y** turns a trigger into the
//! third slot; the chord clears Y for that press so the game sees a plain cast.
//!
//! A Y chord is only ours when the slot behind it carries a spell. **Y + R1/R2 and Y + L1/L2 are
//! also the game's two-hand command**, so an empty chord is handed back untouched: two-handing a
//! weapon whose hand has nothing bound works exactly as it does without the mod, mid-fight
//! included. On that hand the chord that *does* carry a spell belongs to the spell, which is the
//! trade this design makes out loud.
//!
//! On every press the chosen spell is steered into the borrowed magic entry and `selected_slot`
//! is pointed at it, which is what makes several spells work off one selection. Steering happens
//! once per press, not once per poll: the entry stays where it was put until the input is
//! released.
//!
//! **Keyboard and mouse:** with no controller the game never polls XInput, so a click cannot be
//! answered by rewriting a pad report. The mouse is read through raw input instead: the click
//! steers the selection and the game's own click performs the cast. Shift is hidden from the game
//! while it is used as a slot modifier, because the game binds Shift + click to the strong attack.
//!
//! **Assignment:** in the panel (F8) **or with the bind gesture** — hold the configured modifier
//! (L3 by default) and press the input you want, or hold Super (the Windows key) and click. The
//! gesture binds whatever the game's own wheel has selected, which is the only way to assign a
//! spell the character can cast *because of* a Great Rune or a talisman. Assignments are per
//! character, in `nr_casting_slots_<name>-<hash>.txt`, filed on the weapon's base id:
//!
//! ```text
//! # weapon_param_id = bumper_spell, trigger_spell, y_trigger_spell, y_bumper_spell
//! 33240000 = 4710, 4200, -1, -1
//! ```
//!
//! The base id is why upgrading a staff at a smith no longer drops its spells: the game treats
//! every upgrade level as its own row, so the store files them under `id - id % 100`.
//!
//! Layout: `logging` (file), `store` (assignments), `game` (reads), `input` (the pad hook and its
//! rules), `rawinput` (the keyboard/mouse hooks), `stats` (input counters), `config`, `params`,
//! `pointer`, `probe`, `overlay` (panel model), `window` (panel host).

mod config;
mod crash;
mod game;
mod input;
mod rawinput;
mod msg;
mod stats;
mod logging;
mod overlay;
mod params;
mod pointer;
mod probe;
mod store;
mod window;

// The panel and the probe read these through the crate root; re-exporting keeps one obvious
// name for each instead of a module path in every call site.
pub(crate) use game::{
    character_name, inventory_goods_ids, magic_slots, player, player_effective_stats, player_stats,
    forget_borrowed, forget_pointers, weapon_ids, PLAYER_SPELL,
};
pub(crate) use logging::{debug_enabled, log};
pub(crate) use store::{store_get, store_set};

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use eldenring::util::system::{wait_for_system_init, SystemInitError};
use fromsoftware_shared::Program;

use crate::input::{install_hook, watch_hook};
use crate::logging::DEBUG_LOG;
use crate::store::{describe_store, file_mtime, reload_store, seed_store, slots_path};

/// True while the assignment panel is open. Mirroring is suppressed then, because it forces
/// the wheel selection every frame and capture needs the player to be able to cycle it.
pub(crate) static UI_OPEN: AtomicBool = AtomicBool::new(false);
/// Our own module handle, captured in `DllMain`.
pub(crate) fn module_handle() -> usize {
    HINSTANCE_USIZE.load(Ordering::Relaxed)
}

/// All mod files live beside the DLL (see `config`), so a package is self-contained.
fn game_dir_file(name: &str) -> std::path::PathBuf {
    config::path(name)
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
    // First, before any thread or hook exists: if anything faults from here on, the log says
    // what faulted and where, instead of the user having to describe a crash.
    crash::install();
    // Read-only instrumentation: counts the game's own GetAsyncKeyState calls so the log can say
    // whether a keyboard/mouse cast can be presented there when no controller is connected.
    rawinput::install();
    // Panics are logged too, and the previous hook still runs: another mod may have installed
    // one, and a panic must not become invisible just because we replaced its handler.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed").to_string();
        logging::try_log(&format!("panic on thread '{name}': {info}"));
        previous_hook(info);
    }));
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
        "=== nr_casting loaded ({} opens the panel; cast with R1/R2/L1/L2, Y for the extra slots)",
        cfg.hotkey_name
    ));
    log("crash: exception observer installed (faults are logged with module and offset)");
    log(&format!(
        "config: debug_log={} y_bumper={} bind_modifier={} bind_modifier_kb={} files in {}",
        cfg.debug_log,
        cfg.y_bumper_name,
        cfg.bind_modifier_name,
        cfg.bind_modifier_kb_name,
        config::dir().display()
    ));
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
    let mut names_warmed = false;
    let mut world_was_live = false;
    let mut last_polls = 0u32;
    let mut last_passes = 0u32;
    let mut last_unplugged = 0u32;
    loop {
        probe::maybe_validate();
        // A reload destroys the character and builds a new one. Logging the edge is what tells
        // us whether a freeze happened while we were holding game memory.
        let world_live = player().is_some();
        if world_live != world_was_live {
            world_was_live = world_live;
            if world_live {
                log("world: character present");
                // Assignments are per character: switch the store to this one's file before
                // anything reads a slot.
                crate::store::sync_character();
                // A new character may be a class that owns the Y+bumper chords.
                refresh_y_bumper();
            } else {
                log("world: character gone (reload or quit to menu)");
                // Nothing to put back: the character is being freed. Writing into it from this
                // thread would corrupt memory the game still owns, and the crash would land
                // somewhere unrelated (the load screen, the menu) minutes later.
                forget_borrowed();
                // The cached pointers belong to that character, too.
                forget_pointers();
            }
        }
        // Build the spell catalogue as soon as the regulation is loaded, so the generated file
        // exists whether or not the player ever opens the panel.
        // Prove the text-table layout before the catalogue is built, so the spell names written
        // into the cached catalogue can come from the game rather than from placeholders. The
        // probe self-throttles and only needs the repository, which loads with the regulation.
        if !msg::ready() {
            msg::probe();
        }
        if !catalogue_ready {
            catalogue_ready = overlay::ensure_catalogue().is_some();
        }
        // Once the tables are proven, read every name the panel can show. Doing it here rather
        // than on panel open keeps the first open a cache read, and it is what makes names follow
        // the player's language and any mod's own text instead of the compiled English table.
        if !names_warmed && catalogue_ready && msg::ready() {
            let started = Instant::now();
            let (weapons, spells) = overlay::warm_names();
            log(&format!(
                "names: {weapons} weapon and {spells} spell names read from the game's own text ({} ms)",
                started.elapsed().as_millis()
            ));
            names_warmed = true;
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
        // Only with debug_log on: how often the game actually polls, and how much of that the
        // repeat gate threw away. This is the number to quote when judging the mod's CPU cost on
        // someone else's machine.
        if debug_enabled() {
            let polls = stats::pad_polls();
            let passes = stats::passes();
            let unplugged = stats::pad_unplugged_count();
            let (dp, dq) = (polls.wrapping_sub(last_polls), passes.wrapping_sub(last_passes));
            let du = unplugged.wrapping_sub(last_unplugged);
            last_polls = polls;
            last_passes = passes;
            last_unplugged = unplugged;
            log(&format!(
                "input: {dp} polls, {dq} full passes, {} skipped as repeats, {du} with no controller",
                dp.saturating_sub(dq)
            ));
            log(&format!("input: raw {}", stats::raw_report()));
        }
        std::thread::sleep(Duration::from_secs(30));
    }
}

/// The DLLs in this process that are neither Windows' nor ours, with their load bases.
///
/// Every compatibility report so far reduces to "which other DLL is loaded": proxy DLLs for
/// frame generation, another overlay, a class-port mod. Naming them in the log turns that from
/// a guessing game into a line somebody can paste into a bug report, and the base is what lets a
/// `crash:` line ("in X.dll (base …, offset …)") be matched to a module.
fn foreign_modules() -> Option<Vec<(String, usize)>> {
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
    let mut foreign: Vec<(String, usize)> = Vec::new();
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
            foreign.push((
                name.to_string_lossy().to_string(),
                *module as usize,
            ));
        }
    }
    foreign.sort();
    foreign.dedup();
    Some(foreign)
}

/// Milliseconds since the epoch, low 32 bits. Interval tests use wrapping subtraction, so the
/// truncation is harmless and the whole call is one `clock_gettime` (vDSO).
pub(crate) fn millis() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0)
}

/// Whether another native mod that owns the Y+bumper chords is loaded.
///
/// Recluse ships `recluse_input.dll` / `recluse_ui.dll` and uses Y+R1 / Y+L1 itself. The module
/// list is fixed for the life of the process (me3 loads every native before the game starts), so
/// this is resolved once: enumerating 512 modules and their names is not something to do on a
/// poll.
pub(crate) fn recluse_loaded() -> bool {
    static LOADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOADED.get_or_init(|| {
        matches!(foreign_modules(), Some(modules) if modules.iter().any(|(name, _)| {
            name.to_ascii_lowercase().contains("recluse")
        }))
    })
}

/// The last (event id, chr id) pair we acted on, packed. Written from the watcher thread only.
static CHARACTER_IDS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(u64::MAX);

/// Work out who owns Y+R1 / Y+L1 for the character that is loaded right now.
///
/// Called from the hook watcher, which is the only place that knows a character load has
/// finished. Cheap enough for 2 Hz: the character read is the cached pointer plus a compare, and
/// the module list is a `OnceLock`.
pub(crate) fn refresh_y_bumper() {
    use std::sync::atomic::Ordering;
    let cfg = config::load();
    let loaded = recluse_loaded();
    let ids = game::character_ids();
    let by_id = match ids {
        Some((event_id, chr_id)) => cfg
            .recluse_ids
            .iter()
            .any(|id| *id == event_id || (*id as u32) == chr_id),
        None => false,
    };
    // The chr id the player reports is 0 in this build, so ids alone cannot recognise a class.
    // A port ships its own catalyst, and that weapon id is missing from the vanilla table: the
    // most reliable signal available without reading the port's own data.
    let by_catalyst = game::weapon_ids()
        .map(|(left, right)| !overlay::is_vanilla_weapon(left) || !overlay::is_vanilla_weapon(right))
        .unwrap_or(false);
    let is_recluse = by_id || by_catalyst;
    if let Some((event_id, chr_id)) = ids {
        let packed = (event_id as u32 as u64) << 32 | chr_id as u64;
        if CHARACTER_IDS.swap(packed, Ordering::Relaxed) != packed && cfg.debug_log {
            log(&format!(
                "world: character ids event={event_id} chr={chr_id} (recluse mod: {loaded})"
            ));
        }
    }
    // The ini decides, except that a Recluse character never gets the chords under `auto`:
    // fighting the port on its own keybinds is the one outcome nobody wants.
    let allowed = match cfg.y_bumper {
        config::YBumper::On => true,
        config::YBumper::Off => false,
        // Recluse present means the chords are its on its own character. Anywhere else they are
        // ours, so nothing is lost for players who run the port for one character only.
        config::YBumper::Auto => !(loaded && is_recluse),
    };
    let previous = Y_BUMPER_ALLOWED.swap(allowed, Ordering::Relaxed);
    if previous != allowed {
        log(&format!(
            "input: Y+R1 / Y+L1 {} (recluse mod: {loaded}, recluse character: {is_recluse} \
             [id: {by_id}, catalyst: {by_catalyst}], mode: {:?})",
            if allowed { "are ours" } else { "left to Recluse" },
            cfg.y_bumper
        ));
    }
    input::set_y_bumper(allowed);
}

/// The last value handed to `input::set_y_bumper`, so the decision is logged on change only.
static Y_BUMPER_ALLOWED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);

fn log_foreign_modules() {
    match foreign_modules() {
        Some(modules) if !modules.is_empty() => {
            let listing: Vec<String> = modules
                .iter()
                .map(|(name, base)| format!("{name}@{base:#x}"))
                .collect();
            log(&format!("modules: {}", listing.join(", ")));
        }
        Some(_) => log("modules: no foreign DLLs loaded"),
        None => log("modules: enumeration failed"),
    }
}
