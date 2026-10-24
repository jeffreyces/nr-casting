//! Per-weapon spell assignments: the in-memory store and its text file beside the DLL.
//!
//! Three slots per catalyst (bumper, trigger, Y chord). Parsing and formatting live in
//! `nr_casting_core` so they can be tested without a game.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use nr_casting_core::Category;

use crate::config;
use crate::logging::log;

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

pub(crate) fn slots_path() -> std::path::PathBuf {
    config::path("nr_casting_slots.txt")
}

pub(crate) fn file_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(slots_path()).ok().and_then(|m| m.modified().ok())
}

pub(crate) fn seed_store() {
    if !slots_path().exists() {
        // Empty on purpose: a release must not ship somebody else's weapons.
        let defaults = "# weapon_param_id = bumper_spell, trigger_spell\n\
                        # Hold the bind modifier + press R1/R2/L1/L2 in game to bind a spell.\n";
        let _ = std::fs::write(slots_path(), defaults);
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
        let entry = map.entry(weapon).or_insert([-1, -1, -1]);
        if slot < entry.len() {
            entry[slot] = spell;
        }
        map.clone()
    };
    save_store(&snapshot);
}

