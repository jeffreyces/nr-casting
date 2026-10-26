//! Per-weapon spell assignments: the in-memory store and its text file beside the DLL.
//!
//! Three slots per catalyst (bumper, trigger, Y chord). Parsing and formatting live in
//! `nr_casting_core` so they can be tested without a game.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use nr_casting_core::Category;

use crate::config;
use crate::logging::log;

/// Header written into a fresh slots file; the format itself is `nr_casting_core`'s.
const NR_DEFAULTS: &str = "# weapon_param_id = bumper_spell, trigger_spell, y_trigger_spell, y_bumper_spell\n\
                           # Per character. Assign spells in the panel (F8); this file is written by the mod.\n";

/// weapon param id -> [bumper spell, trigger spell]
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

/// A filename-safe, collision-resistant slug for a character name.
///
/// The hash suffix matters: two names that differ only in punctuation or case would otherwise
/// share a file, which is the bug this whole scheme exists to avoid.
fn slug(name: &str) -> String {
    let mut base = String::new();
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !base.is_empty() {
            base.push('-');
            last_dash = true;
        }
    }
    while base.ends_with('-') {
        base.pop();
    }
    base.truncate(24);
    if base.is_empty() {
        base.push_str("character");
    }
    // FNV-1a over the raw name, four hex digits.
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{base}-{hash:04x}")
}

/// Switch the store to a character's file, creating it (and adopting the old shared file once).
pub(crate) fn set_character(name: &str) {
    let slug = slug(name);
    {
        let Ok(current) = ACTIVE_SLUG.lock() else { return };
        if current.as_deref() == Some(slug.as_str()) {
            return;
        }
    }
    let path = config::path(&format!("nr_casting_slots_{slug}.txt"));
    if !path.exists() {
        let legacy = config::path("nr_casting_slots.txt");
        let adopt = legacy.exists()
            && !MIGRATED.swap(true, std::sync::atomic::Ordering::Relaxed);
        if adopt {
            // One-time: the character that loads first after upgrading keeps the assignments
            // that used to be shared. Every later character starts empty on purpose.
            match std::fs::copy(&legacy, &path) {
                Ok(_) => {
                    let _ = std::fs::rename(&legacy, config::path("nr_casting_slots.migrated.txt"));
                    log(&format!("slots: adopted the shared file for {name}"));
                }
                Err(e) => log(&format!("slots: could not adopt the shared file: {e}")),
            }
        }
        if !path.exists() {
            let _ = std::fs::write(&path, NR_DEFAULTS);
        }
    }
    match ACTIVE_SLUG.lock() {
        Ok(mut current) => *current = Some(slug),
        Err(_) => return,
    }
    reload_store();
    log(&format!("slots: character {name} -> {}", slots_path().display()));
}

pub(crate) fn file_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(slots_path()).ok().and_then(|m| m.modified().ok())
}

pub(crate) fn seed_store() {
    if !slots_path().exists() {
        // Empty on purpose: a release must not ship somebody else's weapons.
        let _ = std::fs::write(slots_path(), NR_DEFAULTS);
        log("created default slots file");
    }
    reload_store();
}

pub(crate) fn reload_store() {
    let mut map = HashMap::new();
    if let Ok(text) = std::fs::read_to_string(slots_path()) {
        for (id, spells) in nr_casting_core::parse_slots(&text) {
            map.insert(id, spells);
        }
    }
    *store_lock() = map;
}

fn save_store(map: &HashMap<i32, Category>) {
    let entries: Vec<(i32, Category)> = map.iter().map(|(id, spells)| (*id, *spells)).collect();
    if let Err(e) = std::fs::write(slots_path(), nr_casting_core::format_slots(&entries)) {
        log(&format!("slots: FAILED to write {}: {e}", slots_path().display()));
    }
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
    store_lock().get(&weapon).copied()
}

pub(crate) fn store_set(weapon: i32, slot: usize, spell: i32) {
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
}

