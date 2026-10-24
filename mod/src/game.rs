//! Everything that reads the running game: the player, their magic slots, inventory and stats.
//!
//! Every pointer here was produced by `probe`, which validates the structures first; the reads
//! below trust that check and nothing else.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Mutex;

use eldenring::cs::{ChrAsmSlot, WorldChrMan};
use fromsoftware_shared::FromStatic;

use crate::probe;

/// The spell the *player* has selected (sampled between our writes) — the value capture binds.
/// -1 until seen.
pub(crate) static PLAYER_SPELL: AtomicI32 = AtomicI32::new(-1);

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
pub(crate) fn selection_is(spell: i32) -> bool {
    let Some(magic) = magic_data() else {
        return false;
    };
    let selected = magic.selected_slot;
    (0..magic.entries.len() as i32).contains(&selected)
        && magic.entries[selected as usize].param_id == spell
}

pub(crate) fn select_spell(spell: i32) -> bool {
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

pub(crate) fn player() -> Option<&'static eldenring::cs::PlayerIns> {
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

