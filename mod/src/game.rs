//! Everything that reads the running game: the player, their magic slots, inventory and stats.
//!
//! Every pointer here was produced by `probe`, which validates the structures first; the reads
//! below trust that check and nothing else.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
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
    /// The spell we steered the entry at. Restoring is only correct while the entry still
    /// holds it: if the game or another mod has since pointed the entry somewhere else, our
    /// snapshot is stale and writing it back would clobber their value.
    steered: i32,
}

static BORROWED: Mutex<Option<Borrow>> = Mutex::new(None);
/// Non-blocking mirror of `BORROWED.is_some()` for the pad poll. The poll path runs on the game's
/// input thread and must never queue behind the panel thread's lock.
static BORROW_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Cached game pointers, in `usize` form so they can live in atomics.
///
/// The chain (WorldChrMan -> main_player -> PlayerGameData -> EquipMagicData) is cheap to walk;
/// what is not cheap is the `VirtualQuery` in front of every hop, and this runs on the thread the
/// game polls the pad from — dozens of times a second. `CHAR` is compared against the game's own
/// current player on every access, so a reload invalidates it immediately; `PGD`/`MAGIC` are
/// re-resolved once a second and always re-verified before a write.
static CHAR: AtomicUsize = AtomicUsize::new(0);
static PGD: AtomicUsize = AtomicUsize::new(0);
static MAGIC: AtomicUsize = AtomicUsize::new(0);
/// When the cached pointers were last checked for readability.
static CHAR_CHECKED: AtomicU32 = AtomicU32::new(0);
/// How often to re-verify the cached pointers even when nothing changed.
const RECHECK_MS: u32 = 1000;
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
            steered: 0,
        });
        BORROW_ACTIVE.store(true, Ordering::Relaxed);
    }
    Some(idx)
}

/// Remember what `select_spell` wrote into the borrowed entry.
fn remember_steered(spell: i32) {
    if let Ok(mut slot) = BORROWED.lock() {
        if let Some(borrow) = slot.as_mut() {
            borrow.steered = spell;
        }
    }
}

/// Put back whatever `borrow_selected` overwrote.
///
/// **Only call this from the game's own threads** — the pad poll is the only caller. A timer or
/// the panel thread must never write here: the character can be freed by a load, a warp or a
/// death the moment the world goes away, and a write into that freed block corrupts the heap
/// and crashes the game somewhere unrelated later. Teardown uses `forget_borrowed` instead.
pub(crate) fn release_borrowed() {
    // Cheap gate for the common case: no cast in flight, no need to take the lock at all. This is
    // called from every idle poll.
    if !BORROW_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let snapshot = {
        let mut slot = BORROWED.lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    BORROW_ACTIVE.store(false, Ordering::Relaxed);
    let Some(snapshot) = snapshot else {
        return;
    };
    // A restore is a write: verify the pointer now rather than trust the one-second cache.
    let Some(magic) = magic_data_verified() else {
        return;
    };
    let Some(entry) = magic.entries.get_mut(snapshot.idx) else {
        return;
    };
    // Only undo our own write. If the entry no longer holds the spell we steered, someone else
    // (the game rebuilding the wheel, another selection mod) owns the value now.
    if entry.param_id != snapshot.steered {
        return;
    }
    entry.param_id = snapshot.param_id;
    entry.charges = snapshot.charges;
}

/// Drop the borrow without touching game memory. For a world that is going away: the object is
/// being freed, so there is nothing left to restore and no thread we may write from.
pub(crate) fn forget_borrowed() {
    let mut slot = BORROWED.lock().unwrap_or_else(|e| e.into_inner());
    *slot = None;
    BORROW_ACTIVE.store(false, Ordering::Relaxed);
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
    let Some(magic) = magic_data_verified() else {
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
    remember_steered(spell);
    // The menu's `selected_magic_item` is deliberately left alone: the cast reads
    // `EquipMagicData`, that field is a display cache (it sat at the "no item" sentinel while
    // casting worked fine), and writing it is one more piece of shared state to fight another
    // mod over — class-port mods drive that exact field.
    true
}

/// Sample the spell the player has selected, i.e. anything that is not our scratch
/// entry. Throttled: the poll path calls this once per XInput poll, and the read is three
/// `VirtualQuery` calls down a pointer chain. 100 ms is far below what the panel's "selected"
/// line can show, and `sample_player_selection_now` covers the one place that needs the exact
/// value (binding).
pub(crate) fn sample_player_selection() {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    if now.saturating_sub(LAST.load(Ordering::Relaxed)) < 100 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    sample_now();
}

/// Sample without the throttle. For binding: the spell captured must be the one selected at the
/// press, not one from up to 100 ms ago.
pub(crate) fn sample_player_selection_now() {
    sample_now();
}

fn sample_now() {
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
    if BORROW_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let id = magic.entries[sel].param_id;
    if id > 0 {
        PLAYER_SPELL.store(id, Ordering::Relaxed);
    }
}

pub(crate) fn player() -> Option<&'static eldenring::cs::PlayerIns> {
    let Some(raw) = raw_player() else { return None };
    let cached = CHAR.load(Ordering::Relaxed);
    let due = now_ms().wrapping_sub(CHAR_CHECKED.load(Ordering::Relaxed)) >= RECHECK_MS;
    if cached == raw && !due {
        return Some(unsafe { &*(raw as *const eldenring::cs::PlayerIns) });
    }
    // Either the character changed or the periodic sanity check is due. This is the whole point
    // of the cache: a `VirtualQuery` per second instead of one per read, on the thread the game
    // polls the pad from.
    if !probe::readable(raw as *const u8, std::mem::size_of::<eldenring::cs::PlayerIns>()) {
        forget_pointers();
        return None;
    }
    // Whatever the reason we got here, the sub-objects are re-resolved on next use: a new
    // character has new ones, and on the periodic check they are exactly what we want to
    // re-verify.
    PGD.store(0, Ordering::Relaxed);
    MAGIC.store(0, Ordering::Relaxed);
    CHAR.store(raw, Ordering::Relaxed);
    CHAR_CHECKED.store(now_ms(), Ordering::Relaxed);
    Some(unsafe { &*(raw as *const eldenring::cs::PlayerIns) })
}

/// Milliseconds since the epoch, low 32 bits (wrapping compares are fine).
fn now_ms() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u32)
        .unwrap_or(0)
}

/// The character the game currently considers the player, without validating the object.
/// Cheap: two reads through the game's own singleton.
fn raw_player() -> Option<usize> {
    let wcm = unsafe { WorldChrMan::instance() }.ok()?;
    let owned = wcm.main_player.as_ref()?;
    let ptr = owned.as_ptr() as usize;
    (ptr != 0).then_some(ptr)
}

/// Drop every cached game pointer: unreadable memory, or a world that is going away.
pub(crate) fn forget_pointers() {
    CHAR.store(0, Ordering::Relaxed);
    PGD.store(0, Ordering::Relaxed);
    MAGIC.store(0, Ordering::Relaxed);
    CHAR_CHECKED.store(0, Ordering::Relaxed);
}

/// The player's game data, guarded the same way. Every accessor below goes through this.
fn game_data() -> Option<&'static mut eldenring::cs::PlayerGameData> {
    let player = player()?;
    let cached = PGD.load(Ordering::Relaxed);
    if cached != 0 {
        // Verified when it was cached; `player()` clears it once a second so a reload cannot
        // leave this pointing at a freed block for longer than that.
        return Some(unsafe { &mut *(cached as *mut eldenring::cs::PlayerGameData) });
    }
    let ptr = player.player_game_data.as_ptr();
    if !probe::readable(ptr as *const u8, std::mem::size_of::<eldenring::cs::PlayerGameData>()) {
        return None;
    }
    PGD.store(ptr as usize, Ordering::Relaxed);
    // SAFETY: readable, owned by PlayerIns.
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
    let cached = MAGIC.load(Ordering::Relaxed);
    if cached != 0 {
        return Some(unsafe { &mut *(cached as *mut eldenring::cs::EquipMagicData) });
    }
    let ptr = pgd.equipment.equip_magic_data.as_ptr();
    // OwnedPtr derefs without a check; during a teardown this is exactly the pointer that can be
    // released while it still looks valid.
    if !probe::readable(ptr as *const u8, std::mem::size_of::<eldenring::cs::EquipMagicData>()) {
        return None;
    }
    MAGIC.store(ptr as usize, Ordering::Relaxed);
    // SAFETY: readable, owned by the player's game data.
    Some(unsafe { &mut *ptr })
}

/// The magic data, re-verified immediately before it is written.
///
/// Reads go through the cache; writes do not take that chance. A cast is rare, so paying the
/// `VirtualQuery` there costs nothing, and it closes the window where the cached pointer could
/// have outlived the object between the one-second checks.
fn magic_data_verified() -> Option<&'static mut eldenring::cs::EquipMagicData> {
    let magic = magic_data()?;
    if !probe::readable(
        magic as *const eldenring::cs::EquipMagicData as *const u8,
        std::mem::size_of::<eldenring::cs::EquipMagicData>(),
    ) {
        MAGIC.store(0, Ordering::Relaxed);
        return None;
    }
    Some(magic)
}

