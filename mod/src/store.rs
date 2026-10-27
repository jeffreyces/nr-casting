//! Per-weapon spell assignments: the in-memory store and its text file beside the DLL.
//!
//! Four slots per catalyst (bumper, trigger, Y+trigger chord, Y+bumper chord), one file per
//! character. Parsing, formatting and the file name live in `nr_casting_core` so they can be
//! tested without a game.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use nr_casting_core::Category;

use crate::config;
use crate::logging::log;

/// Header written into a fresh slots file; the format itself is `nr_casting_core`'s.
const NR_DEFAULTS: &str = "# weapon_param_id = bumper_spell, trigger_spell, y_trigger_spell, y_bumper_spell\n\
                           # Per character. Assign spells in the panel (F8); this file is written by the mod.\n";

/// weapon param id -> [bumper, trigger, Y+trigger, Y+bumper] spell ids
static STORE: OnceLock<Mutex<HashMap<i32, Category>>> = OnceLock::new();

/// A panic while holding the store would otherwise poison it and, with `panic = "abort"`,
/// take the process with it. The data is plain values, so recovering the guard is safe.
fn store_lock() -> std::sync::MutexGuard<'static, HashMap<i32, Category>> {
    STORE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The file for the character that is loaded, or the shared one before any character exists.
///
/// Assignments are per character: the same catalyst on two characters usually has different
/// requirements met, so a shared file handed one character spells it could not cast.
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
    let Some((name, id)) = crate::game::character_key() else {
        // Once per process: a world with no readable character is worth exactly one line.
        static WARNED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
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
        Err(e) => log(&format!("slots: could not take over {}: {e}", previous.display())),
    }
}

/// Switch the store to a character's file, creating it (and adopting the old shared file once).
pub(crate) fn set_character(name: &str, id: u32) {
    let (base, slug) = nr_casting_core::character_slug(name, id);
    {
        let Ok(current) = ACTIVE_SLUG.lock() else { return };
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
    prune_uncastable();
    log(&format!(
        "slots: character {name} (id {id}) -> {}",
        slots_path().display()
    ));
}

pub(crate) fn file_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(slots_path()).ok().and_then(|m| m.modified().ok())
}

pub(crate) fn seed_store() {
    // Deliberately creates nothing: a missing file means "no assignments yet", and writing an
    // empty one here would be adopted by the first character as if it were the old shared file.
    reload_store();
}

pub(crate) fn reload_store() {
    let mut map = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(slots_path()) {
        let parsed = nr_casting_core::parse_slots(&text);
        let folded = nr_casting_core::canonicalize(&parsed);
        // A file written before assignments moved onto base weapon ids holds `33090010` where
        // the store now wants `33090000`. Fold it and write the result back once, so the
        // upgrade does not look like a new weapon on every future load either.
        //
        // `canonicalize` sorts, and `format_slots` writes sorted, so a file that needs nothing
        // compares equal here once it has been through this. Comparing against the raw parse
        // would rewrite (and log) on every load of an unsorted file.
        let mut sorted = parsed.clone();
        sorted.sort_by_key(|(id, _)| *id);
        if folded != sorted {
            log(&format!(
                "slots: folded {} line(s) onto base weapon ids ({} after folding)",
                parsed.len(),
                folded.len()
            ));
            if let Err(e) = std::fs::write(slots_path(), nr_casting_core::format_slots(&folded)) {
                log(&format!(
                    "slots: FAILED to rewrite {}: {e}",
                    slots_path().display()
                ));
            }
        }
        for (id, spells) in folded {
            map.insert(id, spells);
        }
    }
    *store_lock() = map;
    bump_generation();
}

/// Bumped on every change to the in-memory store.
///
/// The input path caches "which slots on this weapon pair carry a spell" and needs to know when
/// a binding (panel, or the bind gesture) invalidates that answer.
static GENERATION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub(crate) fn generation() -> u32 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

fn bump_generation() {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

fn save_store(map: &HashMap<i32, Category>) {
    let entries: Vec<(i32, Category)> = map.iter().map(|(id, spells)| (*id, *spells)).collect();
    if let Err(e) = std::fs::write(slots_path(), nr_casting_core::format_slots(&entries)) {
        log(&format!("slots: FAILED to write {}: {e}", slots_path().display()));
    }
}

/// Drop assignments whose spell the loaded character's stats cannot cast.
///
/// A file can be shared by two characters the game cannot tell apart (same name, same slot, same
/// appearance). This is the guarantee that survives even then: nothing on file is ever presented
/// for a spell the character could not cast. Skipped when the params are not readable yet — an
/// empty requirement list must never wipe assignments.
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
    let dropped = {
        let mut map = store_lock();
        let mut dropped = 0usize;
        for (_, slots) in map.iter_mut() {
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
        dropped
    };
    if dropped > 0 {
        let snapshot: HashMap<i32, Category> = store_lock().clone();
        save_store(&snapshot);
        bump_generation();
        log(&format!(
            "slots: dropped {dropped} assignment(s) this character cannot cast"
        ));
    }
}

/// Forget every assignment for the loaded character.
///
/// Needed because two characters can be indistinguishable to the game: same name, same save
/// slot, same appearance. Nothing in the player data separates those, so the file is shared and
/// this is the way out.
pub(crate) fn clear_character() {
    {
        let mut map = store_lock();
        map.clear();
    }
    save_store(&HashMap::new());
    bump_generation();
    log(&format!("slots: cleared {}", slots_path().display()));
}

pub(crate) fn describe_store() -> String {
    let map = store_lock();
    let mut ids: Vec<&i32> = map.keys().collect();
    ids.sort();
    ids.iter()
        .map(|id| format!("{id}={:?}", map[id]))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn store_get(weapon: i32) -> Option<Category> {
    store_lock()
        .get(&nr_casting_core::base_weapon_id(weapon))
        .copied()
}

pub(crate) fn store_set(weapon: i32, slot: usize, spell: i32) {
    // Assignments follow the weapon, not one upgrade level of it: the game gives every level
    // its own param row, so filing them under the equipped id lost them at the smith.
    let weapon = nr_casting_core::base_weapon_id(weapon);
    // Snapshot under one lock, then write: dropping the guard first let two assignments
    // interleave and save out of order.
    let snapshot = {
        let mut map = store_lock();
        let entry = map.entry(weapon).or_insert([-1; nr_casting_core::SLOTS]);
        if slot < entry.len() {
            entry[slot] = spell;
        }
        map.clone()
    };
    save_store(&snapshot);
    bump_generation();
}

