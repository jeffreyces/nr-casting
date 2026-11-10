//! Per-weapon spell assignments: the in-memory store and its text file beside the DLL.
//!
//! Four slots per catalyst (bumper, trigger, Y+trigger chord, Y+bumper chord), one file per
//! character, and one or more **profiles** in that file - named assignment sheets the player can
//! switch between. Parsing, the profile rules and the file name live in `nr_casting_core` so they
//! can be tested without a game; this module owns the file and the live state.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use nr_casting_core::{Category, Profiles, Sheet};

use crate::config;
use crate::logging::log;

/// Header written into a fresh slots file. The rows and their layout come from the core crate.
const NR_DEFAULTS: &str = "# weapon_param_id = bumper_spell, trigger_spell, y_trigger_spell, y_bumper_spell\n\
                           # Per character, one [profile <name>] section each. Assign spells in the\n\
                           # panel (F8); this file is written by the mod.\n";

/// Everything the store knows: the profiles as written, which one is live, and the live one's rows
/// indexed for the hot path.
///
/// Two shapes on purpose. `profiles` is the *file* model - ordered, duplicates allowed, exactly what
/// parsing produces - and `rows` is the *runtime* model the cast path wants: a map looked up per
/// press. `save_store` folds the map back into the active sheet before formatting, so the two can
/// never disagree on disk.
struct Store {
    profiles: Profiles,
    active: usize,
    rows: HashMap<i32, Category>,
}

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();

/// A panic while holding the store would otherwise poison it and, with `panic = "abort"`,
/// take the process with it. The data is plain values, so recovering the guard is safe.
fn store_lock() -> std::sync::MutexGuard<'static, Store> {
    STORE
        .get_or_init(|| {
            Mutex::new(Store {
                profiles: Profiles::default(),
                active: 0,
                rows: HashMap::new(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// One sheet's rows, as the cast path looks them up: a map, last row wins, like the file parser.
fn rows_of(profiles: &Profiles, index: usize) -> HashMap<i32, Category> {
    profiles
        .sheets
        .get(index)
        .map(|sheet| sheet.rows.iter().copied().collect())
        .unwrap_or_default()
}

/// The file for the character that is loaded, or the shared one before any character exists.
///
/// Assignments are per character: the same catalyst on two characters usually has different
/// requirements met, so a shared file handed one character spells it could not cast. The profiles
/// themselves are the same list for every character - the file just holds the rows that character
/// can use.
static ACTIVE_SLUG: Mutex<Option<String>> = Mutex::new(None);

/// Set once the pre-per-character file has been adopted by the first character that loaded.
static MIGRATED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn slots_path() -> std::path::PathBuf {
    match ACTIVE_SLUG.lock().map(|slug| slug.clone()) {
        Ok(Some(slug)) => config::path(&format!("nr_casting_slots_{slug}.txt")),
        _ => config::path("nr_casting_slots.txt"),
    }
}

/// Keep the store on the loaded character's file. Cheap enough for the 500 ms watcher: two
/// fields of the player data and a string compare.
pub(crate) fn sync_character() {
    // A load replaces the memorised list wholesale; the gate's snapshot is refreshed here rather
    // than waiting for its one-second net, so the first cast on a new character is decided from
    // that character's spells and not the last one's.
    let Some((name, id)) = crate::game::character_key() else {
        // Once per process: a world with no readable character is worth exactly one line.
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            log("slots: character name unreadable; keeping the current file");
        }
        return;
    };
    set_character(&name, id);
}

/// Whether a slots file holds at least one weapon line.
fn has_assignments(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|text| !nr_casting_core::parse_slots(&text).is_empty())
        .unwrap_or(false)
}

/// Take over the assignments from the one sibling file that matches this character's name.
///
/// The slug gained the save-level id, so a file written by the previous version has a different
/// suffix. If exactly one sibling shares the name part and holds assignments, it is this
/// character's: copy it over, leaving the original in place as a backup.
fn adopt_previous_character_file(base: &str, target: &std::path::Path) {
    // The slots live in the data folder, which is a subfolder beside the DLL — not in `dir()`.
    let Ok(entries) = std::fs::read_dir(config::data_dir()) else {
        log("slots: could not list the data folder looking for a previous file");
        return;
    };
    let prefix = format!("nr_casting_slots_{base}-");
    let mut found = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(&prefix) || !name.ends_with(".txt") {
            continue;
        }
        if entry.path() == target {
            continue;
        }
        if found.is_some() {
            log(&format!("slots: several files match {base}; not guessing"));
            return;
        }
        found = Some(entry.path());
    }
    let Some(previous) = found else {
        return;
    };
    if !has_assignments(&previous) {
        return;
    }
    match std::fs::copy(&previous, target) {
        Ok(_) => log(&format!(
            "slots: took over {} for this character",
            previous.file_name().unwrap_or_default().to_string_lossy()
        )),
        Err(e) => log(&format!(
            "slots: could not take over {}: {e}",
            previous.display()
        )),
    }
}

/// Switch the store to a character's file, creating it (and adopting the old shared file once).
pub(crate) fn set_character(name: &str, id: u32) {
    let (base, slug) = nr_casting_core::character_slug(name, id);
    {
        let Ok(current) = ACTIVE_SLUG.lock() else {
            return;
        };
        if current.as_deref() == Some(slug.as_str()) {
            return;
        }
    }
    let path = config::path(&format!("nr_casting_slots_{slug}.txt"));
    if !path.exists() {
        let legacy = config::path("nr_casting_slots.txt");
        // Only a legacy file that actually holds assignments is worth adopting: a header-only
        // file is what a previous version seeded on startup.
        let adopt = legacy.exists()
            && has_assignments(&legacy)
            && !MIGRATED.swap(true, std::sync::atomic::Ordering::Relaxed);
        if adopt {
            // One-time: the character that loads first after upgrading keeps the assignments
            // that used to be shared. Every later character starts empty on purpose.
            match std::fs::copy(&legacy, &path) {
                Ok(_) => {
                    // Keep exactly one record of the adopted file: never overwrite a previous
                    // migration, or the original assignments would be lost for good.
                    let archive = config::path("nr_casting_slots.migrated.txt");
                    if !archive.exists() {
                        let _ = std::fs::rename(&legacy, archive);
                    }
                    log(&format!("slots: adopted the shared file for {name}"));
                }
                Err(e) => log(&format!("slots: could not adopt the shared file: {e}")),
            }
        }
        if !path.exists() {
            let _ = std::fs::write(&path, NR_DEFAULTS);
        }
        if !has_assignments(&path) {
            adopt_previous_character_file(&base, &path);
        }
    }
    match ACTIVE_SLUG.lock() {
        Ok(mut current) => *current = Some(slug),
        Err(_) => return,
    }
    reload_store();
    // A load replaces the memorised list wholesale, so the gate's snapshot is refreshed here rather
    // than waiting for its one-second net: the first cast on a new character must be decided from
    // that character's spells, not the last one's.
    crate::game::refresh_attuned();
    prune_uncastable();
    log(&format!(
        // `|` rather than a slash before the profile name: the redactor at the log sink treats a
        // bare `/...` run as another path and swallowed the rest of this line, which hid the very
        // answer the line exists to give.
        "slots: character {name} (id {id}) -> {} | profile {}",
        slots_path().display(),
        active_profile_name()
    ));
}

pub(crate) fn file_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(slots_path())
        .ok()
        .and_then(|m| m.modified().ok())
}

pub(crate) fn seed_store() {
    // Deliberately creates nothing: a missing file means "no assignments yet", and writing an
    // empty one here would be adopted by the first character as if it were the old shared file.
    reload_store();
}

/// Read the file into the store and make the profile named in the ini live.
pub(crate) fn reload_store() {
    let path = slots_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut profiles = nr_casting_core::parse_profiles(&text);
    // A fresh install, and a file from before hotkeys existed, both give `default` no digit - which
    // then let the NEXT profile take Ctrl+1. The default is always reachable, so it claims the
    // first digit when no sheet claims one.
    let default_gained_hotkey = nr_casting_core::ensure_default_hotkey(&mut profiles);
    // Every sheet is given concrete switch values here, from what the player is running right now.
    //
    // A sheet with no `balance`/`skip_assigned` line used to mean "as-is", which made switching to it
    // inherit whatever the previous profile had published: turn both switches off on one profile,
    // switch to the default, and the default appeared to have been switched off too. Nothing had been
    // written - it had simply kept the other sheet's values, which is indistinguishable from a bug.
    // Stamping once on load makes a switch deterministic from then on.
    let mut stamped = false;
    for sheet in profiles.sheets.iter_mut() {
        if sheet.balance.is_none() {
            sheet.balance = Some(config::balance());
            stamped = true;
        }
        if sheet.skip_assigned.is_none() {
            sheet.skip_assigned = Some(config::skip_assigned());
            stamped = true;
        }
    }
    let has_marker = text
        .lines()
        .any(|line| line.trim_start().starts_with("# format 3"));
    let mut rewritten = !has_marker;

    // Fold upgrade-level rows onto their base ids unless the file is one that understands per-copy
    // rows. The marker is what makes that a fact rather than a guess: guessing wrong either loses a
    // customised layout or resurrects one. Applies to every sheet, not only the live one.
    if !nr_casting_core::per_copy_rows(&text) {
        for sheet in profiles.sheets.iter_mut() {
            let mut as_read = sheet.rows.clone();
            let folded = nr_casting_core::canonicalize(&as_read);
            as_read.sort_by_key(|(id, _)| *id);
            if folded != as_read {
                log(&format!(
                    "slots: folded {} line(s) onto base weapon ids in profile {}",
                    as_read.len(),
                    sheet.name
                ));
                sheet.rows = folded;
                rewritten = true;
            }
        }
    }
    // Stamp the profile marker the first time a version that understands it reads the file, so a
    // later downgrade is visible in the file rather than silent.
    if default_gained_hotkey {
        log(&format!(
            "profile: '{}' is on {}+1",
            nr_casting_core::DEFAULT_PROFILE,
            config::profile_modifier().as_str()
        ));
    }
    if stamped {
        log(&format!(
            "profile: switch positions written into every profile (balance {}, wheel skip {})",
            config::balance().as_str(),
            if config::skip_assigned() { "on" } else { "off" }
        ));
    }
    if rewritten || default_gained_hotkey || stamped {
        if let Err(e) = std::fs::write(&path, nr_casting_core::format_profiles(&profiles)) {
            log(&format!("slots: FAILED to rewrite {}: {e}", path.display()));
        }
    }

    let wanted = config::profile_name();
    let (active, fell_back) = profiles.active(&wanted).unwrap_or((0, false));
    if fell_back {
        let name = profiles
            .sheets
            .get(active)
            .map(|sheet| sheet.name.clone())
            .unwrap_or_default();
        // Logged, and **not** written back. This runs before the character's file is selected - the
        // store starts on the shared pre-character file, which holds only whatever that file has - so
        // a name that is missing here may be present in the character's own file a moment later.
        // Writing the fallback into the ini is how a player's profile choice was lost on every
        // restart: the ini said `test`, the pre-character file had only `default`, and the fallback
        // overwrote the name before the right file was ever read.
        log(&format!(
            "slots: profile '{wanted}' is not in this file yet - using '{name}' until it is"
        ));
    }

    let rows = rows_of(&profiles, active);
    let mut store = store_lock();
    store.rows = rows;
    store.active = active;
    store.profiles = profiles;
    drop(store);
    bump_generation();
}

/// Bumped on every change to the in-memory store.
///
/// The input path caches "which slots on this weapon pair carry a spell" and needs to know when
/// a binding (panel, the bind gesture, or a profile switch) invalidates that answer.
static GENERATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn generation() -> u32 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

fn bump_generation() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Write the store back: the live map is folded into the active sheet, then every sheet is written.
///
/// The map is the runtime shape and the sheet is the file shape; folding one into the other here is
/// what keeps an edit from being lost the moment a second profile is written.
fn save_store(store: &mut Store) {
    if let Some(sheet) = store.profiles.sheets.get_mut(store.active) {
        sheet.rows = store
            .rows
            .iter()
            .map(|(id, spells)| (*id, *spells))
            .collect();
        sheet.rows.sort_by_key(|(id, _)| *id);
    }
    if let Err(e) = std::fs::write(
        slots_path(),
        nr_casting_core::format_profiles(&store.profiles),
    ) {
        log(&format!(
            "slots: FAILED to write {}: {e}",
            slots_path().display()
        ));
    }
}

/// Save and revalidate while still holding the lock, so two writers cannot interleave.
fn commit(store: &mut Store) {
    save_store(store);
    bump_generation();
}

/// Drop assignments whose spell the loaded character's stats cannot cast.
///
/// A file can be shared by two characters the game cannot tell apart (same name, same slot, same
/// appearance). This is the guarantee that survives even then: nothing on file is ever presented
/// for a spell the character could not cast. Skipped when the params are not readable yet — an
/// empty requirement list must never wipe assignments. Every sheet is pruned: the file belongs to
/// this character, so a sheet it could not cast from is wrong for all of them.
fn prune_uncastable() {
    // Planning mode lists and assigns spells the character cannot cast yet; do not undo that.
    if crate::overlay::show_all() {
        return;
    }
    let (int, fai, arc) = match crate::game::player_effective_stats() {
        // All zero means the read landed before the character existed: pruning on that would
        // wipe every assignment.
        Some((0, 0, 0)) | None => return,
        Some(stats) => stats,
    };
    let spells = crate::params::live_spells();
    if spells.is_empty() {
        return;
    }
    let requirements: HashMap<i32, (u32, u32, u32)> = spells
        .iter()
        .map(|spell| (spell.id, (spell.int, spell.fai, spell.arc)))
        .collect();
    let mut store = store_lock();
    let mut dropped = 0usize;
    for sheet in store.profiles.sheets.iter_mut() {
        for (_, slots) in sheet.rows.iter_mut() {
            for slot in slots.iter_mut() {
                let Some((need_int, need_fai, need_arc)) = requirements.get(slot) else {
                    continue;
                };
                if *slot > 0 && (*need_int > int || *need_fai > fai || *need_arc > arc) {
                    *slot = -1;
                    dropped += 1;
                }
            }
        }
    }
    if dropped > 0 {
        let rows = rows_of(&store.profiles, store.active);
        store.rows = rows;
        commit(&mut store);
        log(&format!(
            "slots: dropped {dropped} assignment(s) this character cannot cast"
        ));
    }
}

/// Forget every assignment in the live profile.
///
/// Only the active sheet: the others belong to other loadouts, and clearing those by accident would
/// be the one action here a player could not undo from memory.
pub(crate) fn clear_character() {
    let mut store = store_lock();
    store.rows.clear();
    let name = store
        .profiles
        .sheets
        .get(store.active)
        .map(|sheet| sheet.name.clone())
        .unwrap_or_default();
    commit(&mut store);
    log(&format!(
        "slots: cleared profile {name} in {}",
        slots_path().display()
    ));
}

pub(crate) fn describe_store() -> String {
    let store = store_lock();
    let mut ids: Vec<&i32> = store.rows.keys().collect();
    ids.sort();
    ids.iter()
        .map(|id| format!("{id}={:?}", store.rows[id]))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The four spells for the weapon in hand: this copy's override if it has one, otherwise the
/// default for the weapon type.
///
/// Rows are filed on the full param id only when a player customises one copy; every other row is
/// the type's default on the base id. Reading full-then-base is what makes an upgrade at a smith
/// keep its layout, because the game gives each upgrade level its own param row and the full id
/// therefore changes while the weapon does not.
pub(crate) fn store_get(weapon: i32) -> Option<Category> {
    if weapon <= 0 {
        return None;
    }
    let store = store_lock();
    // The rule lives in the core crate, with tests: `resolve_row` over a slice, this shape over a
    // map. Two spellings of "full id first, then the type's default" is how the tested copy and
    // the used copy drift apart.
    let base = nr_casting_core::base_weapon_id(weapon);
    nr_casting_core::resolve_row_with(base, weapon, |id| store.rows.get(&id).copied())
}

/// Whether this exact copy has a layout of its own, rather than inheriting the type's default.
pub(crate) fn has_override(weapon: i32) -> bool {
    let base = nr_casting_core::base_weapon_id(weapon);
    weapon > 0 && weapon != base && store_lock().rows.contains_key(&weapon)
}

pub(crate) fn store_set(weapon: i32, slot: usize, spell: i32) {
    // An edit lands on the row the panel showed: an override stays an override, and a weapon
    // inheriting the type's default keeps editing that default. Anything else would silently
    // change which weapons an edit applies to.
    let base = nr_casting_core::base_weapon_id(weapon);
    let mut store = store_lock();
    let target = nr_casting_core::store_target(base, weapon, store.rows.contains_key(&weapon));
    let entry = store
        .rows
        .entry(target)
        .or_insert([-1; nr_casting_core::SLOTS]);
    if slot < entry.len() {
        entry[slot] = spell;
    }
    commit(&mut store);
}

/// Give this copy a layout of its own, seeded from the type's default.
///
/// The only way two copies of one weapon can hold different spells: identical copies still share,
/// because nothing in the param data separates them, but a +0 and a +10 can diverge deliberately
/// rather than by accident.
pub(crate) fn store_customise(weapon: i32) -> bool {
    let base = nr_casting_core::base_weapon_id(weapon);
    if weapon <= 0 || weapon == base {
        return false;
    }
    let mut store = store_lock();
    if store.rows.contains_key(&weapon) {
        return false;
    }
    let inherited = store
        .rows
        .get(&base)
        .copied()
        .unwrap_or([-1; nr_casting_core::SLOTS]);
    store.rows.insert(weapon, inherited);
    commit(&mut store);
    log(&format!("slots: {weapon} now has its own layout (inherited {base})"));
    true
}

/// Drop this copy's layout and inherit the type's default again.
pub(crate) fn store_reset(weapon: i32) -> bool {
    let base = nr_casting_core::base_weapon_id(weapon);
    if weapon <= 0 || weapon == base {
        return false;
    }
    let mut store = store_lock();
    if store.rows.remove(&weapon).is_none() {
        return false;
    }
    commit(&mut store);
    log(&format!("slots: {weapon} back to the default for {base}"));
    true
}

// ---------------------------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------------------------

/// The live profile's name, for the panel and the report.
pub(crate) fn active_profile_name() -> String {
    let store = store_lock();
    store
        .profiles
        .sheets
        .get(store.active)
        .map(|sheet| sheet.name.clone())
        .unwrap_or_else(|| nr_casting_core::DEFAULT_PROFILE.to_string())
}

/// Every profile as (name, hotkey digit, flag note), in file order, for the panel's dropdown.
///
/// The flag note is what the sheet will switch `balance` and the wheel skip to. It is in the label
/// because a profile carrying its own mode is otherwise invisible: a switch can turn balanced mode on
/// and make a free-mode assignment read as unbound, which looks like a broken cast until the reason is
/// on screen.
pub(crate) fn profile_list() -> Vec<(String, Option<u8>, String)> {
    store_lock()
        .profiles
        .sheets
        .iter()
        .map(|sheet| {
            let mut note = String::new();
            match sheet.balance {
                Some(mode) => note.push_str(&format!(", {}", mode.as_str())),
                None => note.push_str(", as-is"),
            }
            if sheet.skip_assigned == Some(true) {
                note.push_str(", skip");
            }
            (sheet.name.clone(), sheet.hotkey, note)
        })
        .collect()
}

/// Make the named profile live. Returns false when there is no such profile, or it already is.
///
/// Publishes the sheet's `balance` and `skip_assigned` into the live config, because a profile is a
/// whole gameplay setup rather than only rows; the panel's own toggles take the same path. It does
/// not decide *whether* to switch - the caller does, with `hotkey_allowed`.
pub(crate) fn switch_profile(name: &str) -> bool {
    let mut store = store_lock();
    let Some(index) = store.profiles.index_of(name) else {
        return false;
    };
    if index == store.active {
        return false;
    }
    store.active = index;
    let live = rows_of(&store.profiles, index);
    store.rows = live;
    let sheet = store.profiles.sheets[index].clone();
    commit(&mut store);
    drop(store);
    config::set_profile_name(&sheet.name);
    config::apply_profile_flags(sheet.balance, sheet.skip_assigned);
    // No `refresh_attuned` here: it reads game memory, and a profile switch can be triggered from
    // the panel thread. The memorised list did not change anyway - only which rows are live - and
    // the cast path re-reads it on its own net.
    // A profile carries its own mode and wheel switch, so a switch that changes them says so in the
    // same line - the player is about to see spells the mod calls unbound otherwise. One string, not
    // four nested ones.
    let hotkey = match sheet.hotkey {
        Some(digit) => nr_casting_core::hotkey_label(config::profile_modifier(), digit),
        None => "no hotkey".to_string(),
    };
    log(&format!(
        "profile: -> {} ({} weapons, {}, balance {}, wheel skip {})",
        sheet.name,
        sheet.rows.len(),
        hotkey,
        sheet.balance.map_or("as-is", |mode| mode.as_str()),
        match sheet.skip_assigned {
            Some(true) => "on",
            Some(false) => "off",
            None => "as-is",
        }
    ));
    true
}

/// Copy the live rows into a new profile and switch to it.
///
/// This is "save as": the current setup becomes a named sheet, which is what a player wants when
/// they have just tuned one loadout and want to try a second without losing the first.
pub(crate) fn save_profile_as(name: &str) -> Result<(), &'static str> {
    if !nr_casting_core::profile_name_ok(name) {
        return Err("a name is printable ASCII, up to 24 characters, without separators");
    }
    let mut store = store_lock();
    if store.profiles.index_of(name).is_some() {
        return Err("a profile with that name already exists");
    }
    if store.profiles.sheets.len() >= nr_casting_core::MAX_PROFILES {
        return Err("nine profiles is the limit");
    }
    let mut sheet = Sheet::with_switches(name, config::balance(), config::skip_assigned());
    sheet.rows = store.rows.iter().map(|(id, spells)| (*id, *spells)).collect();
    store.profiles.sheets.push(sheet);
    let index = store.profiles.sheets.len() - 1;
    if let Some(digit) = nr_casting_core::autobind_digit(&store.profiles) {
        let _ = nr_casting_core::bind_digit(&mut store.profiles, index, digit);
    }
    store.active = index;
    commit(&mut store);
    let hotkey = store.profiles.sheets[index].hotkey;
    drop(store);
    config::set_profile_name(name);
    log(&format!(
        "profile: saved '{name}' ({}, {} weapons)",
        match hotkey {
            Some(digit) => nr_casting_core::hotkey_label(config::profile_modifier(), digit),
            None => "no hotkey".to_string(),
        },
        active_row_count()
    ));
    Ok(())
}

/// Write the live `balance` and wheel-skip values into the live profile.
///
/// The two switches belong to the profile, so flipping one while that profile is active has to
/// record it there - otherwise the ini says one thing, the sheet another, and switching away and back
/// silently reverts the player's change. Called by the panel after it saves either switch, and by
/// nothing else: a switch publishes a profile's values, so publishing is not a place to learn them.
pub(crate) fn record_active_flags() {
    let balance = config::balance();
    let skip = config::skip_assigned();
    let mut store = store_lock();
    let active = store.active;
    let Some(sheet) = store.profiles.sheets.get_mut(active) else {
        return;
    };
    if sheet.balance == Some(balance) && sheet.skip_assigned == Some(skip) {
        return;
    }
    sheet.balance = Some(balance);
    sheet.skip_assigned = Some(skip);
    let name = sheet.name.clone();
    commit(&mut store);
    drop(store);
    log(&format!(
        "profile: '{name}' now records balance {} and wheel skip {}",
        balance.as_str(),
        if skip { "on" } else { "off" }
    ));
}

/// How many profiles this character's file holds, for the report.
pub(crate) fn profile_count() -> usize {
    store_lock().profiles.sheets.len()
}

/// How many weapon rows the live profile holds, for the panel and the log.
pub(crate) fn active_row_count() -> usize {
    store_lock().rows.len()
}

/// Create a profile with **no assignments** and switch to it.
///
/// The dropdown's "create new": a fresh sheet for a loadout the player is about to build, as opposed
/// to `save as`, which copies the live one and is the way to fork an existing setup. The empty sheet
/// inherits the live `balance`/`skip` values so the switches do not move under the player while they
/// start assigning.
pub(crate) fn create_profile(name: &str) -> Result<(), &'static str> {
    if !nr_casting_core::profile_name_ok(name) {
        return Err("a name is printable ASCII, up to 24 characters, without separators");
    }
    let mut store = store_lock();
    if store.profiles.index_of(name).is_some() {
        return Err("a profile with that name already exists");
    }
    if store.profiles.sheets.len() >= nr_casting_core::MAX_PROFILES {
        return Err("nine profiles is the limit");
    }
    // A fresh sheet, not an inherited one: nothing assigned, free balance, wheel skip off - what a
    // new install looks like. `save as` is the one that copies the live switches.
    store.profiles.sheets.push(Sheet::with_switches(
        name,
        nr_casting_core::Balance::Free,
        false,
    ));
    let index = store.profiles.sheets.len() - 1;
    if let Some(digit) = nr_casting_core::autobind_digit(&store.profiles) {
        let _ = nr_casting_core::bind_digit(&mut store.profiles, index, digit);
    }
    store.active = index;
    store.rows.clear();
    commit(&mut store);
    let hotkey = store.profiles.sheets[index].hotkey;
    drop(store);
    config::set_profile_name(name);
    // The freshly created sheet is now live, so publish its switches: nothing is assigned yet, and
    // balanced mode over an empty sheet would refuse every pick for no reason.
    config::apply_profile_flags(Some(nr_casting_core::Balance::Free), Some(false));
    log(&format!(
        "profile: created '{name}' empty ({})",
        match hotkey {
            Some(digit) => nr_casting_core::hotkey_label(config::profile_modifier(), digit),
            None => "no hotkey".to_string(),
        }
    ));
    Ok(())
}

/// Rename a profile. `default` keeps its name: every unknown active name resolves to it.
pub(crate) fn rename_profile(old: &str, new: &str) -> Result<(), &'static str> {
    let mut store = store_lock();
    let Some(index) = store.profiles.index_of(old) else {
        return Err("no such profile");
    };
    nr_casting_core::rename_profile(&mut store.profiles, index, new)?;
    let active_renamed = index == store.active;
    commit(&mut store);
    drop(store);
    if active_renamed {
        config::set_profile_name(new);
    }
    log(&format!("profile: renamed '{old}' to '{new}'"));
    Ok(())
}

/// Delete a profile, landing on `default` when the deleted one was live.
pub(crate) fn delete_profile(name: &str) -> Result<String, &'static str> {
    let mut store = store_lock();
    let Some(index) = store.profiles.index_of(name) else {
        return Err("no such profile");
    };
    let active_name = store
        .profiles
        .sheets
        .get(store.active)
        .map(|sheet| sheet.name.clone())
        .unwrap_or_default();
    let after = nr_casting_core::delete_profile(&mut store.profiles, index, &active_name)?;
    if let Some(new_index) = store.profiles.index_of(&after) {
        store.active = new_index;
    }
    let live = rows_of(&store.profiles, store.active);
    store.rows = live;
    commit(&mut store);
    let sheet = store.profiles.sheets.get(store.active).cloned();
    drop(store);
    config::set_profile_name(&after);
    if let Some(sheet) = sheet {
        config::apply_profile_flags(sheet.balance, sheet.skip_assigned);
    }
    log(&format!("profile: deleted '{name}', live is now '{after}'"));
    Ok(after)
}

/// Bind a digit to a profile, refusing one another profile already holds.
pub(crate) fn set_profile_hotkey(name: &str, digit: u8) -> Result<(), &'static str> {
    let mut store = store_lock();
    let Some(index) = store.profiles.index_of(name) else {
        return Err("no such profile");
    };
    nr_casting_core::bind_digit(&mut store.profiles, index, digit)?;
    commit(&mut store);
    drop(store);
    log(&format!(
        "profile: '{name}' is on {}",
        nr_casting_core::hotkey_label(config::profile_modifier(), digit)
    ));
    Ok(())
}

/// The profile a digit switches to, if one is bound to it.
pub(crate) fn profile_for_digit(digit: u8) -> Option<String> {
    let store = store_lock();
    store
        .profiles
        .for_digit(digit)
        .and_then(|index| store.profiles.sheets.get(index))
        .map(|sheet| sheet.name.clone())
}
