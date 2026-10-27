//! Everything that reads the running game: the player, their magic slots, inventory and stats.
//!
//! Every pointer here was produced by `probe`, which validates the structures first; the reads
//! below trust that check and nothing else.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use eldenring::cs::{ChrAsmArmStyle, ChrAsmHand, WorldChrMan};
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
/// Lock-free mirror of `Borrow::steered`: the spell the borrowed entry currently holds.
///
/// Steering is a write to game memory behind two `VirtualQuery` checks and two locks. Doing that
/// once per press is the point; doing it on every poll of a held button (125 a second) was the
/// per-poll cost most likely to be felt as a hitch while casting.
static STEERED: AtomicI32 = AtomicI32::new(-1);

/// Keep the borrowed entry in place until this millisecond timestamp.
///
/// A pad cast is performed by the report we rewrite, in the same poll, so the entry only has to
/// survive one frame. A mouse cast is performed by the *game's own click*, which it processes on
/// its next frame — and the pad poll runs every 8 ms in between, calling `release_borrowed` and
/// putting the player's spell back before the click is ever acted on. That is why every mouse
/// click cast the wheel's current spell no matter what was steered. The hold covers the gap.
static BORROW_HOLD_UNTIL: AtomicU32 = AtomicU32::new(0);

/// Hold the borrowed entry for `ms` so the game's own click can act on it.
pub(crate) fn hold_borrowed(ms: u32) {
    BORROW_HOLD_UNTIL.store(now_ms().wrapping_add(ms), Ordering::Relaxed);
}

/// The spell the borrow currently holds, or `-1` when nothing is borrowed.
pub(crate) fn steered_spell() -> i32 {
    if BORROW_ACTIVE.load(Ordering::Relaxed) {
        STEERED.load(Ordering::Relaxed)
    } else {
        -1
    }
}

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
    STEERED.store(spell, Ordering::Relaxed);
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
    // A mouse cast asked for the entry to stay a little longer (see `hold_borrowed`). Wrapping
    // comparison: still held while `now` has not reached the deadline.
    if (now_ms().wrapping_sub(BORROW_HOLD_UNTIL.load(Ordering::Relaxed)) as i32) < 0 {
        return;
    }
    let snapshot = {
        let mut slot = BORROWED.lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    BORROW_ACTIVE.store(false, Ordering::Relaxed);
    STEERED.store(-1, Ordering::Relaxed);
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
    STEERED.store(-1, Ordering::Relaxed);
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

/// The spell the *game* has selected, or `None` while our own borrow is in place.
///
/// This is what the bind gesture assigns: the player's own wheel selection, not our steered
/// entry. Reading it from the game is exactly why the gesture works for a spell the character can
/// only cast with a Great Rune or a talisman on — the panel's requirement filter cannot see the
/// buffed stat, but the game has already let the player equip the spell.
pub(crate) fn selected_spell() -> Option<i32> {
    if BORROW_ACTIVE.load(Ordering::Relaxed) {
        // The entry holds our steered spell; the player's choice is not readable through it.
        return None;
    }
    let magic = magic_data()?;
    let selected = magic.selected_slot;
    if !(0..magic.entries.len() as i32).contains(&selected) {
        return None;
    }
    let id = magic.entries[selected as usize].param_id;
    (id > 0).then_some(id)
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
/// line can show.
/// value (binding).
pub(crate) fn sample_player_selection(now: u32) {
    // Wrapping compare on the caller's millisecond clock: `saturating_sub` would stall this for
    // the 49 days between a wrap and the stored value being overtaken again.
    static LAST: AtomicU32 = AtomicU32::new(0);
    if now.wrapping_sub(LAST.load(Ordering::Relaxed)) < 100 {
        return;
    }
    LAST.store(now, Ordering::Relaxed);
    sample_now();
}

/// Sample without the throttle. For binding: the spell captured must be the one selected at the

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
    WEAPON_PAIR.store(u64::MAX, Ordering::Relaxed);
    CHAR.store(raw, Ordering::Relaxed);
    CHAR_CHECKED.store(now_ms(), Ordering::Relaxed);
    Some(unsafe { &*(raw as *const eldenring::cs::PlayerIns) })
}

/// Milliseconds since the epoch, low 32 bits (wrapping compares are fine).
pub(crate) fn now_ms() -> u32 {
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
    WEAPON_PAIR.store(u64::MAX, Ordering::Relaxed);
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

/// The equipped weapon pair, cached for one poll pass.
///
/// `weapon_ids` is asked several times per poll (is this slot bound, which chord is this, the HUD
/// mirror) and each answer used to walk `WorldChrMan -> main_player -> PlayerGameData` *and* take a
/// clock reading, because `player()` checks its revalidation timer on every call. Answering from a
/// cached pair collapses that to one walk per pass. 8 ms is the pass interval itself, so a weapon
/// swap is still seen on the pass it happens in: this is de-duplication, not staleness.
const WEAPON_CACHE_MS: u32 = 8;
static WEAPON_PAIR: AtomicU64 = AtomicU64::new(u64::MAX);
static WEAPON_AT: AtomicU32 = AtomicU32::new(0);

pub(crate) fn weapon_ids() -> Option<(i32, i32)> {
    let now = now_ms();
    let cached = WEAPON_PAIR.load(Ordering::Relaxed);
    if cached != u64::MAX
        && now.wrapping_sub(WEAPON_AT.load(Ordering::Relaxed)) < WEAPON_CACHE_MS
    {
        return Some((unpack_weapon_id(cached >> 32), unpack_weapon_id(cached)));
    }
    let pgd = game_data()?;
    let chr_asm = &pgd.equipment.chr_asm;
    let ids = &chr_asm.equipment_param_ids;
    // The *active* slot, not slot 1: each hand has three weapon slots and the player cycles them,
    // so reading `WeaponRight1` reports whatever is in the first slot no matter what is in hand.
    // That is how the panel kept naming a staff after the right hand was switched to a sword, and
    // why the chord logic and the store were answering for the wrong weapon.
    let left = ids[chr_asm.equipment.active_weapon_slot(ChrAsmHand::Left) as usize];
    let right = ids[chr_asm.equipment.active_weapon_slot(ChrAsmHand::Right) as usize];
    WEAPON_PAIR.store(
        pack_weapon_id(left) << 32 | pack_weapon_id(right),
        Ordering::Relaxed,
    );
    WEAPON_AT.store(now, Ordering::Relaxed);
    Some((left, right))
}

/// The game's own stance value, for the diagnostic trace.
pub(crate) fn arm_style() -> Option<ChrAsmArmStyle> {
    Some(game_data()?.equipment.chr_asm.equipment.arm_style)
}

/// Weapon ids are `i32`; the cache is a `u64` pair. Round-trip through `u32` so a negative or
/// sentinel id comes back exactly as it went in.
fn pack_weapon_id(id: i32) -> u64 {
    id as u32 as u64
}

fn unpack_weapon_id(bits: u64) -> i32 {
    bits as u32 as i32
}

/// The loaded character's name, as the game stores it (UTF-16, 16 characters).
///
/// This is the key assignments are filed under: the same weapon on two characters must not share
/// a set of spells, because the second character may not meet their requirements.
pub(crate) fn character_name() -> Option<String> {
    character_key().map(|(name, _)| name)
}

/// Name and save-level id of the loaded character.
///
/// The id is included because two characters can share a name; the store hashes both.
pub(crate) fn character_key() -> Option<(String, u32)> {
    let pgd = game_data()?;
    let units: Vec<u16> = pgd
        .character_name
        .iter()
        .take_while(|unit| **unit != 0)
        .copied()
        .collect();
    if units.is_empty() {
        return None;
    }
    let name = String::from_utf16_lossy(&units).trim().to_string();
    (!name.is_empty()).then_some((name, pgd.character_id))
}

/// Ids that identify the loaded character, for deciding whether a class port owns a chord.
///
/// `(character_event_id, character_id)`: the event id is the character template the save was
/// made with, the chr id is the model the game spawned. A class port like Recluse ships its own
/// model (`c8131`), so either number matching is enough to call the character that class.
pub(crate) fn character_ids() -> Option<(i32, u32)> {
    let player = player()?;
    let chr_id = player.chr_ins.character_id;
    let event_id = game_data().map(|pgd| pgd.character_event_id).unwrap_or(0);
    Some((event_id, chr_id))
}

/// INT, FAI, ARC of the loaded character.
pub(crate) fn player_stats() -> Option<(u32, u32, u32)> {
    let pgd = game_data()?;
    Some((pgd.intelligence, pgd.faith, pgd.arcane))
}

/// INT/FAI/ARC as the game applies them: the levelled value plus whatever is boosting it.
///
/// `PlayerGameData` carries both: `intelligence`/`faith`/`arcane` are the levelled values on the
/// character sheet's left column, and `effective_*` are what the game itself computed after great
/// runes, talismans, physick and buffs. Reading `effective_*` is both exact and free — the earlier
/// version of this summed `change_magic_point`/`change_faith_point`/`change_luck_point` over the
/// active `SpEffect` rows and came out 8 short of the sheet (76 vs 84 with a rune and talismans
/// on), because those param fields are not where the game accumulates talisman bonuses.
///
/// Falls back to the levelled values if the game has not filled the effective set in yet (they are
/// valid from load, but zero is a cheap and certain tell that something is off).
pub(crate) fn player_effective_stats() -> Option<(u32, u32, u32)> {
    let pgd = game_data()?;
    let effective = (
        pgd.effective_intelligence,
        pgd.effective_faith,
        pgd.effective_arcane,
    );
    if effective.0 == 0 || effective.1 == 0 || effective.2 == 0 {
        return Some((pgd.intelligence, pgd.faith, pgd.arcane));
    }
    Some(effective)
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

