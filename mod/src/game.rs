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

/// One borrow per steered entry, not one for the whole mod.
///
/// Two casts can overlap - both hands pressed near enough together, a mouse cast next to a pad
/// cast - and with a single record the second one *replaced* the first one's snapshot, so only one
/// of the two entries was ever put back and the other kept our spell for good. That is exactly how
/// a player's memorised slot got displaced, and the log showed it as two steers 17 ms apart with a
/// single restore. A list costs a few bytes and makes the loss impossible.
static BORROWED: Mutex<Vec<Borrow>> = Mutex::new(Vec::new());
/// More simultaneous steers than a player can meaningfully pull. Past this a cast is refused rather
/// than risking a place: refusing one cast is recoverable, losing a memorised spell is not.
const MAX_BORROWS: usize = 4;
/// Non-blocking mirror of `!BORROWED.is_empty()` for the pad poll, which runs on the game's input
/// thread and must never queue behind a lock for a question it asks every poll.
///
/// Invariant: this is written **only** while the borrows are held, in the same function that changes
/// the list, and its value is derived from the list rather than from what the caller believes. The
/// version that stored `true` on the way in and `false` on the way out could be cleared underneath a
/// borrow taken concurrently - and then nothing ever released that entry.
static BORROW_ACTIVE: AtomicBool = AtomicBool::new(false);

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
/// How long the character block is trusted without being re-examined.
///
/// This is a *net*, not a trigger: the pointer changing is the event that re-arms it, and that
/// covers loading, dying and warping, which is when the block actually moves. What the net is for is
/// the case the event cannot see - the block freed and something else allocated at the same address.
/// It was 1000 ms, which paid a `VirtualQuery` over the whole character struct once a second
/// (175 us mean, 633 us worst here; milliseconds on a machine whose pages are not resident) to catch
/// a case that every write already re-checks for itself (`magic_data_verified`, `image_range`).
/// Five seconds keeps the safety property and costs a fifth as much.
const RECHECK_MS: u32 = 5000;
/// Steer the game's selection at `spell` through a spare magic entry.
/// Point the game's *selected* entry at `spell`, remembering what was there so it can be
/// restored as soon as the input is released.
///
/// Borrowing the selected entry rather than a spare one matters: a full wheel has no free
/// slot, and permanently displacing somebody's spell risks the game saving that displacement.
/// The tampering window here is one burst of input, and `release_borrowed` closes it on the
/// first poll with no spell input held.
/// The borrows, with the poison rule in one place.
///
/// A panic elsewhere must not cost a player their memorised spell, so a poisoned lock is used
/// rather than skipped: the earlier code had two call sites that quietly returned on poison, which
/// meant one of them silently skipped a restore.
fn borrows() -> std::sync::MutexGuard<'static, Vec<Borrow>> {
    BORROWED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which entry the game's selection points at, clamped to something that exists.
///
/// No slots means nothing to steer (seen for a frame during a teardown); `entries[0]` would panic,
/// and this crate is panic=abort, so a panic here kills the game.
fn selected_index(magic: &eldenring::cs::EquipMagicData) -> Option<usize> {
    if magic.entries.is_empty() {
        return None;
    }
    let selected = magic.selected_slot;
    Some(if (0..magic.entries.len() as i32).contains(&selected) {
        selected as usize
    } else {
        0
    })
}

/// Snapshot the selected entry, steer it at `spell` and confirm the write, **as one operation**.
///
/// The snapshot and the write used to be two steps with the lock released between them, so a poll
/// that released the borrow in that window restored the entry before our write landed - leaving our
/// spell in the player's list with no record of it. Holding the lock across the whole thing is what
/// makes that impossible; the write is a single store, so the lock is held for microseconds.
fn borrow_and_write(magic: &mut eldenring::cs::EquipMagicData, spell: i32) -> Option<usize> {
    let idx = selected_index(magic)?;
    let mut list = borrows();
    match list.iter_mut().find(|b| b.idx == idx) {
        // Already steering this entry: its original is what is recorded, and re-snapshotting now
        // would save our own value as the "original" - the other way a place gets lost.
        Some(existing) => existing.steered = spell,
        None => {
            if list.len() >= MAX_BORROWS {
                STEER_BUSY.fetch_add(1, Ordering::Relaxed);
                crate::logging::debug!(
                    "steer: {} casts still in flight, refusing entry {idx} instead of risking their places",
                    list.len()
                );
                return None;
            }
            let entry = &magic.entries[idx];
            crate::logging::debug!(
                "steer: borrowing entry {} holding spell {} (charges {})",
                idx,
                entry.param_id,
                entry.charges
            );
            list.push(Borrow {
                idx,
                param_id: entry.param_id,
                charges: entry.charges,
                steered: spell,
            });
        }
    }
    let entry = magic.entries.get_mut(idx)?;
    entry.param_id = spell;
    if entry.charges > 0 {
        entry.charges = -1;
    }
    magic.selected_slot = idx as i32;
    BORROW_ACTIVE.store(!list.is_empty(), Ordering::Relaxed);
    Some(idx)
}

/// The original of the borrowed entry at `idx`, if we are steering it.
///
/// A lookup over the live borrows (at most `MAX_BORROWS`) rather than a map built per call: this is
/// read on the frame clock, and the earlier version allocated a `HashMap` every frame for a value
/// that only changes when a cast starts or ends. Readers show the player's wheel, not the spell we
/// are about to cast, or the gate would approve the very spell it is supposed to stop.
fn original_at(list: &[Borrow], idx: usize) -> Option<i32> {
    list.iter().find(|b| b.idx == idx).map(|b| b.param_id)
}

/// Put back whatever `borrow_and_write` overwrote.
///
/// **Only call this from the game's own threads** — the pad poll is the only caller. A timer or
/// the panel thread must never write here: the character can be freed by a load, a warp or a
/// death the moment the world goes away, and a write into that freed block corrupts the heap
/// and crashes the game somewhere unrelated later. Teardown uses `forget_borrowed` instead.
/// Returns whether it touched game memory. `false` means it took one atomic load and stopped - the
/// common case - which is what lets a report distinguish "a borrow was in flight" from "a read was
/// performed". An earlier counter conflated the two and reported 462 idle reads where the real
/// number is the handful of restores.
pub(crate) fn release_borrowed() -> bool {
    // Cheap gate for the common case: no cast in flight, no need to take the lock at all. This is
    // called from every idle poll.
    if !BORROW_ACTIVE.load(Ordering::Relaxed) {
        return false;
    }
    // A mouse cast asked for the entry to stay a little longer (see `hold_borrowed`). Same
    // "zero means nothing armed" rule as the pad's Y suppression: an unset deadline here used to
    // look pending for half of every clock wrap, which kept the borrow from ever being released.
    if nr_casting_core::armed_until(now_ms(), BORROW_HOLD_UNTIL.load(Ordering::Relaxed)) {
        return false;
    }
    // Held for the whole operation, and the in-flight flag is derived from the list rather than
    // stored beside it. Taking the snapshots out, restoring them and only then clearing the flag
    // left a window where a borrow taken by another poll was merged after the fact - or, worse, the
    // flag was cleared while that borrow was live, so nothing ever released it.
    let mut list = borrows();
    if list.is_empty() {
        BORROW_ACTIVE.store(false, Ordering::Relaxed);
        return false;
    }
    let snapshots = std::mem::take(&mut *list);
    // From here the pointer is about to be walked: everything below this line is game memory,
    // whatever it returns.
    //
    // The snapshot is only dropped once the restore has actually happened. Dropping it first - which
    // is what this did - meant that a world that was unreadable for that one instant (a load, a
    // warp, a death) lost the record of what to put back, and our steered spell stayed in the
    // player's memorised list with nothing left to undo it. The teardown path calls
    // `forget_borrowed`, so this cannot retry forever against a world that is really gone.
    // One verified read for the whole pass: `restore_one` used to walk the pointer chain once per
    // borrow, up to four times for a hand's worth of overlapping casts.
    let mut unrestored: Vec<Borrow> = Vec::new();
    match magic_data_verified() {
        Some(magic) => {
            for snapshot in snapshots {
                if !restore_one(magic, &snapshot) {
                    unrestored.push(snapshot);
                }
            }
        }
        None => unrestored = snapshots,
    }
    list.extend(unrestored);
    let live = !list.is_empty();
    drop(list);
    BORROW_ACTIVE.store(live, Ordering::Relaxed);

    true
}

/// Undo one steer: put the entry back, and put the wheel back where the player left it.
///
/// `false` means the world was not readable and the record must be kept for another attempt.
fn restore_one(magic: &mut eldenring::cs::EquipMagicData, snapshot: &Borrow) -> bool {
    match magic.entries.get_mut(snapshot.idx) {
        Some(entry) if entry.param_id == snapshot.steered => {
            entry.param_id = snapshot.param_id;
            entry.charges = snapshot.charges;
            // Read the entry back: a restore that did not land is the whole bug, and writing it
            // without checking is how it could go unnoticed.
            let landed = magic
                .entries
                .get(snapshot.idx)
                .is_some_and(|e| e.param_id == snapshot.param_id);
            if landed {
                crate::logging::debug!(
                    "steer: restored entry {} to spell {} (after steering {})",
                    snapshot.idx,
                    snapshot.param_id,
                    snapshot.steered
                );
            } else {
                RESTORE_FAILED.fetch_add(1, Ordering::Relaxed);
                // Logged once: a restore can be retried on every idle poll, and a failure that
                // repeats (another mod rewriting the entry each frame) would otherwise write a line
                // 125 times a second and churn the log's own 4 MiB rotation. The counter keeps
                // counting, and the report prints it.
                static WARNED_RESTORE: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !WARNED_RESTORE.swap(true, Ordering::Relaxed) {
                    crate::log(&format!(
                        "steer: restore of entry {} to spell {} did not land (found {}) - that slot holds our spell; further failures are counted only",
                        snapshot.idx,
                        snapshot.param_id,
                        magic.entries.get(snapshot.idx).map(|e| e.param_id).unwrap_or(-2)
                    ));
                }
            }
            // Casting makes the game re-point the wheel at the spell it just cast: the selection
            // lands on the entry that really holds that spell, not on the one we borrowed, and the
            // wheel visibly jumps - and then the skip steps off that assigned spell as it should.
            // Put the selection back where the player left it, but only when the entry really went
            // back: on a failed restore that index holds *our* spell, and pointing the wheel there
            // would show the player a spell they never chose.
            let selected = magic.selected_slot;
            if landed && selected >= 0 && selected as usize != snapshot.idx {
                let moved_onto_our_cast = magic
                    .entries
                    .get(selected as usize)
                    .is_some_and(|e| e.param_id == snapshot.steered);
                if moved_onto_our_cast {
                    magic.selected_slot = snapshot.idx as i32;
                    crate::logging::debug!(
                        "steer: wheel sat on the cast spell {} at entry {selected}; selection put back to entry {}",
                        snapshot.steered,
                        snapshot.idx
                    );
                }
            }
            true
        }
        Some(entry) => {
            // Someone else owns the value now, so our spell is not there and nothing is left in the
            // player's list. Counted apart from a failed restore: one means a displaced spell, the
            // other means there is nothing to displace.
            RESTORE_SUPERSEDED.fetch_add(1, Ordering::Relaxed);
            crate::logging::debug!(
                "steer: restore not needed at entry {} - our {} is gone, found {} (original {})",
                snapshot.idx,
                snapshot.steered,
                entry.param_id,
                snapshot.param_id
            );
            true
        }
        None => false,
    }
}
/// Drop the borrow without touching game memory. For a world that is going away: the object is
/// being freed, so there is nothing left to restore and no thread we may write from.
pub(crate) fn forget_borrowed() {
    let mut list = borrows();
    list.clear();
    BORROW_ACTIVE.store(false, Ordering::Relaxed);
}

/// The twelve memorised entries as the game holds them, oldest slot first.
///
/// Logged once when the panel opens: the wheel the player sees and the entries we steer must be the
/// same list, and this is the cheapest way to prove it against what the memory menu shows.
pub(crate) fn log_entries() {
    let Some(magic) = magic_data_verified() else {
        return;
    };
    let ids: Vec<i32> = magic.entries.iter().map(|e| e.param_id).collect();
    crate::logging::debug!(
        "spell: entries = {ids:?} (selected {})",
        magic.selected_slot
    );
    // The six carried slots, labelled, on the same line as the wheel: the mapping from
    // `equipment_param_ids` index to hand and number is the one thing here that came from the
    // bindings' documentation rather than from a measurement of our own, so it is printed for the
    // report to confirm against the equipment screen.
    let carried = equipped_weapons();
    crate::logging::debug!(
        "spell: carried L1={} R1={} L2={} R2={} L3={} R3={}",
        carried[0],
        carried[1],
        carried[2],
        carried[3],
        carried[4],
        carried[5]
    );
    // A spell cannot legitimately occupy two memory slots, so a duplicate is the fingerprint of a
    // steer write that was never put back - ours from an older build, or another mod's. First-seen
    // map rather than a scan per slot: this runs once at panel open, and the index arithmetic that
    // version used was one more thing to misread.
    let mut first_seen: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    for (slot, id) in ids.iter().enumerate() {
        if *id <= 0 {
            continue;
        }
        if let Some(first) = first_seen.insert(*id, slot) {
            crate::log(&format!(
                "spell: {id} is in slots {first} and {slot} - a steer write was never put back; re-memorise it at a grace"
            ));
        }
    }
}

/// The last wheel we saw, so any change made while we are not casting can be named.
static LAST_WHEEL: std::sync::Mutex<Vec<i32>> = std::sync::Mutex::new(Vec::new());

/// Name every slot that changed while no steer was in flight.
///
/// This is the observer that was missing for the whole "the mod replaced my spell" thread: our own
/// writes are restored and verified, so the displacement either comes from the game or from another
/// mod, and this says which slot, from what, to what, and when.
pub(crate) fn watch_wheel() {
    if BORROW_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let Some(magic) = magic_data_verified() else {
        return;
    };
    let now: Vec<i32> = magic.entries.iter().map(|e| e.param_id).collect();
    let mut last = LAST_WHEEL.lock().unwrap_or_else(|e| e.into_inner());
    if last.len() == now.len() {
        for (slot, (was, is)) in last.iter().zip(now.iter()).enumerate() {
            if was != is {
                crate::log(&format!(
                    "spell: slot {slot} changed {was} -> {is} with no steer in flight"
                ));
            }
        }
    }
    *last = now;
}

/// What two different questions say about the chord modifier, logged when either changes.
///
/// The chord is decided from the raw XInput bit for the physical Y button, while the game decides
/// "Event Action is held" through its own binding table (`CSPad::poll_digital_input`, which resolves
/// `UserInputKey::EventAction` through `input_type_group` -> `key_assign.get_virtual_input_index`).
/// A player who moves Event Action off Y gets a chord that never fires, so both answers - plus the
/// pouch variant, which is what "hold Event Action" means in the world - have to be seen side by
/// side before anything is changed.
///
/// Diagnostic only: it reads, logs a transition, and nothing in the input path depends on it. Called
/// from the frame clock, never from the panel thread, and never while a hook is holding a lock.
pub(crate) fn chord_probe() {
    use eldenring::cs::UserInputKey;

    static LAST: Mutex<Option<(bool, bool, bool)>> = Mutex::new(None);
    static WARNED: AtomicBool = AtomicBool::new(false);

    let physical = crate::input::modifier_held();
    // SAFETY: `FD4PadManager` is a game-owned singleton with a documented accessor that returns Err
    // until it exists. The reference is not held past this call, and nothing is written through it.
    let states = match unsafe { eldenring::fd4::FD4PadManager::instance() } {
        Ok(manager) => manager.get_in_game_pad().map(|pad| {
            (
                pad.poll_digital_input(UserInputKey::EventAction),
                pad.poll_digital_input(UserInputKey::EventActionPouch),
            )
        }),
        Err(_) => None,
    };
    let Some((action, pouch)) = states else {
        if !WARNED.swap(true, Ordering::Relaxed) {
            crate::log("chord: the game's pad manager is not readable - the chord probe is off");
        }
        return;
    };
    let now = (physical, action, pouch);
    if *LAST.lock().unwrap_or_else(|e| e.into_inner()) == Some(now) {
        return;
    }
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(now);
    crate::log(&format!(
        "chord: physical_y={physical} event_action={action} pouch={pouch}"
    ));
}

/// Whether a cast is holding a magic entry right now.
///
/// A profile switch must not land between the steer and the restore, so the keyboard path asks this
/// before it changes anything.
pub(crate) fn cast_in_flight() -> bool {
    BORROW_ACTIVE.load(Ordering::Relaxed)
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

/// What the wheel check has done, for the report.
///
/// `moved` is also the flicker measurement: the game applies its own cycle first, so the assigned
/// spell is the selection for the frame it arrived on and we move off it in the same tick. If that
/// is ever visible, this count is the evidence, not an opinion.
static WHEEL_MOVED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static WHEEL_ALL_ASSIGNED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static WHEEL_PAUSED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// The index we wrote last, so the change we see next tick can be told from our own write.
static WHEEL_WROTE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
/// Moves in the current second, and the second they belong to.
static WHEEL_RATE_SECOND: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static WHEEL_RATE_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Until this uptime in milliseconds, the check does nothing. Set when the game fights back.
static WHEEL_PAUSE_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Moves this session, times the wheel ran out of free spells, and times it stood down.
pub(crate) fn wheel_counts() -> (u32, u32, u32) {
    (
        WHEEL_MOVED.load(std::sync::atomic::Ordering::Relaxed),
        WHEEL_ALL_ASSIGNED.load(std::sync::atomic::Ordering::Relaxed),
        WHEEL_PAUSED.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// The assigned set with what it was built from: the store's generation and the equipped weapons.
struct Assigned {
    generation: u32,
    carried: [i32; 6],
    spells: std::collections::HashSet<i32>,
}

/// The spells this character has assigned, held behind the cache's own lock.
///
/// Returning a guard rather than a clone matters: this is read on the frame clock, and the earlier
/// version copied the whole set every frame for a value that changes only when a layout is saved.
fn assigned_cache() -> std::sync::MutexGuard<'static, Assigned> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<Assigned>> = OnceLock::new();
    let generation = crate::store::generation();
    // Scope is every weapon the character is **carrying**, all six slots - not the two in hand.
    // Switching hands mid-fight must not change what the wheel offers, and a spell assigned to the
    // staff in your second slot is still a spell one of your buttons owns. The two-in-hand answer
    // stays where it belongs: the cast path, which is what actually casts.
    let carried = equipped_weapons();
    let cache = CACHE.get_or_init(|| {
        Mutex::new(Assigned {
            generation: u32::MAX,
            carried: [0; 6],
            spells: std::collections::HashSet::new(),
        })
    });
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    if guard.generation != generation || guard.carried != carried {
        let mut rows = std::collections::HashMap::new();
        for weapon in carried {
            if let Some(row) = crate::store::store_get(weapon) {
                rows.insert(weapon, row);
            }
        }
        guard.spells = nr_casting_core::assigned_spells(&rows);
        guard.generation = generation;
        guard.carried = carried;
    }
    guard
}

/// Every weapon in the character's six slots, three per hand.
///
/// `equipment_param_ids` holds all of them (`ChrAsmSlot` 0..=5, `WeaponLeft1` to `WeaponRight3`);
/// `active_weapon_slot` says which is in hand, and that is the cast path's question, not this one.
fn equipped_weapons() -> [i32; 6] {
    let Some(pgd) = game_data() else {
        return [0; 6];
    };
    let ids = &pgd.equipment.chr_asm.equipment_param_ids;
    [ids[0], ids[1], ids[2], ids[3], ids[4], ids[5]]
}

/// The memorised list and where the game's selection points, read in one pass.
///
/// One function because both come from the same structure: two readers meant two `VirtualQuery`
/// walks per frame and two chances for the pointer to move between them.
/// How many memory slots the game's `EquipMagicData` has (`entries: [EquipMagicItem; 14]`).
///
/// The size is fixed by the structure, so the wheel is a fixed array and the per-frame read
/// allocates nothing. A `Vec` here was one heap allocation per frame on the frame thread, for a
/// length that can never vary.
const MAGIC_SLOTS: usize = 14;

struct Wheel {
    spells: [i32; MAGIC_SLOTS],
    current: usize,
}

fn read_wheel() -> Option<Wheel> {
    let magic = magic_data_verified()?;
    let len = magic.entries.len();
    let current = magic.selected_slot;
    if current < 0 || current as usize >= len {
        return None;
    }
    let originals = borrows();
    let mut spells = [0i32; MAGIC_SLOTS];
    for (index, slot) in spells.iter_mut().enumerate() {
        *slot = magic
            .entries
            .get(index)
            .map(|entry| original_at(&originals, index).unwrap_or(entry.param_id))
            .unwrap_or(-1);
    }
    Some(Wheel {
        spells,
        current: current as usize,
    })
}

/// Put the game's selection on `index`, without borrowing an entry.
///
/// Returns whether the write read back. The entry is untouched, so the spell that was there stays
/// attuned and remains castable from its own button.
pub(crate) fn set_selected_index(index: usize) -> bool {
    let Some(magic) = magic_data_verified() else {
        return false;
    };
    if index >= magic.entries.len() {
        return false;
    }
    magic.selected_slot = index as i32;
    magic.selected_slot == index as i32
}

/// What the two hands can cast between them.
///
/// The union, not the intersection: the wheel is one list for both hands, so a spell the left-hand
/// seal can cast is castable.
fn hands_capability() -> nr_casting_core::CatalystSchools {
    let (left, right) = crate::weapon_ids().unwrap_or((0, 0));
    let a = crate::overlay::weapon_schools(left);
    let b = crate::overlay::weapon_schools(right);
    nr_casting_core::CatalystSchools {
        sorcery: a.sorcery || b.sorcery,
        incantation: a.incantation || b.incantation,
    }
}

/// Skip the spells the panel owns when the game cycles its wheel. Runs on the game's frame clock.
pub(crate) fn wheel_tick() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED_ALL: AtomicBool = AtomicBool::new(false);
    static WARNED_FIGHT: AtomicBool = AtomicBool::new(false);

    // Off is off: no read of game memory at all.
    if !crate::config::skip_assigned() {
        WHEEL_WROTE.store(-1, Ordering::Relaxed);
        return;
    }
    // The panel owns the input while it is up.
    if crate::UI_OPEN.load(Ordering::Relaxed) {
        return;
    }
    // A cast in flight is not the player cycling the wheel. Our steer writes an assigned spell into
    // the selected entry, so the skip would read "the selection is on something assigned" and step
    // off it mid-cast - and the next poll of the same held press then steers the *new* selection,
    // which is how one press left the wheel on a different spell. Stand down until the borrow is
    // released; the entry is ours for exactly that window.
    if BORROW_ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let now = crate::logging::uptime_ms() as u64;
    if now < WHEEL_PAUSE_UNTIL.load(Ordering::Relaxed) {
        return;
    }
    let Some(wheel) = read_wheel() else {
        return;
    };
    // Our own write from last tick: nothing to do, and re-reading the list would only confirm it.
    if WHEEL_WROTE.swap(-1, Ordering::Relaxed) == wheel.current as i32 {
        return;
    }
    let assigned = assigned_cache();
    // What the wheel should not offer: what the buttons in hand already own, and what these
    // catalysts cannot cast at all. The second half is why unequipping a seal hides the incantations
    // again - the game would list them and then refuse the cast.
    let hidden = nr_casting_core::wheel_hidden(
        &wheel.spells[..],
        &assigned.spells,
        hands_capability(),
        crate::overlay::spell_school_static,
    );
    let Some(target) = nr_casting_core::skip_target(wheel.current, &wheel.spells[..], &hidden)
    else {
        // Nothing to do. "Everything is assigned" is worth one line, so a player who presses up and
        // sees the wheel behave normally knows why rather than filing a bug.
        let on_hidden = hidden.contains(&wheel.spells[wheel.current]);
        let nowhere =
            nr_casting_core::next_unassigned(wheel.current, &wheel.spells[..], &hidden).is_none();
        if on_hidden && nowhere && !WARNED_ALL.swap(true, Ordering::Relaxed) {
            WHEEL_ALL_ASSIGNED.fetch_add(1, Ordering::Relaxed);
            crate::log(
                "wheel: no spell is left to offer here (everything attuned is assigned or cannot be \
                 cast with these catalysts) - the game's own cycling is left alone",
            );
        }
        return;
    };
    // A hand cannot cycle twenty times a second; that rate means the game is rewriting the selection
    // itself, and writing back every frame is a fight rather than a feature.
    let second = (now / 1000) as u32;
    if WHEEL_RATE_SECOND.swap(second, Ordering::Relaxed) != second {
        WHEEL_RATE_COUNT.store(0, Ordering::Relaxed);
    }
    if WHEEL_RATE_COUNT.fetch_add(1, Ordering::Relaxed) + 1 > 20 {
        WHEEL_PAUSE_UNTIL.store(now + 1000, Ordering::Relaxed);
        WHEEL_PAUSED.fetch_add(1, Ordering::Relaxed);
        if !WARNED_FIGHT.swap(true, Ordering::Relaxed) {
            crate::log(
                "wheel: the game keeps taking the selection back - skipping pauses a second at a \
                 time rather than fighting it",
            );
        }
        return;
    }
    if set_selected_index(target) {
        WHEEL_WROTE.store(target as i32, Ordering::Relaxed);
        WHEEL_MOVED.fetch_add(1, Ordering::Relaxed);
        // Named, because the count alone cannot say whether the wheel moved off a spell the player
        // chose or off one a cast had just put there - and the second one is a bug, not the feature.
        crate::logging::debug!(
            "wheel: moved selection {} -> {target} (slot {} held {} which the panel owns)",
            wheel.current,
            wheel.current,
            wheel.spells.get(wheel.current).copied().unwrap_or(-1)
        );
    }
}

/// How many writes to the magic entry read back correctly, and how many did not.
///
/// The report used to say "the write path is checked on the first cast" and then never say what
/// the check found: a claim with no observation behind it. The counters make the claim auditable
/// from the report alone - a session with casts and zero refusals *is* the write verification.
static STEER_OK: AtomicU32 = AtomicU32::new(0);
/// The write did not read back: a wrong offset, a read-only page, or the game rewriting it first.
static STEER_WRITE_FAILED: AtomicU32 = AtomicU32::new(0);
/// A cast was refused because the borrow list was full. Not a failure of the write.
static STEER_BUSY: AtomicU32 = AtomicU32::new(0);
/// A restore did not land: the dangerous one, and the only one that leaves our spell in the list.
static RESTORE_FAILED: AtomicU32 = AtomicU32::new(0);
/// A restore found the entry already holding something else, so our value was gone and there was
/// nothing to put back. Benign - counted apart from `RESTORE_FAILED` because they mean opposites and
/// one counter for both made the report claim a displaced spell where none was left.
static RESTORE_SUPERSEDED: AtomicU32 = AtomicU32::new(0);

/// What the steer path has done, one number per meaning. Zero handles means no cast yet.
pub(crate) fn steer_counts() -> (u32, u32, u32, u32, u32) {
    (
        STEER_OK.load(Ordering::Relaxed),
        STEER_WRITE_FAILED.load(Ordering::Relaxed),
        STEER_BUSY.load(Ordering::Relaxed),
        RESTORE_FAILED.load(Ordering::Relaxed),
        RESTORE_SUPERSEDED.load(Ordering::Relaxed),
    )
}

pub(crate) fn select_spell(spell: i32) -> bool {
    let Some(magic) = magic_data_verified() else {
        return false;
    };
    // Snapshot and write together: a release on another poll cannot restore the entry between the
    // two, which is how our spell used to be left in the list with nothing recording it.
    let Some(idx) = borrow_and_write(magic, spell) else {
        return false;
    };
    // Read back what was just written. This is the whole of the write verification now: the write
    // is one the cast needs anyway, so checking it costs two loads and adds no memory shape of its
    // own — no snapshot, no restore, no synthetic sentinel. A wrong offset or a read-only page
    // shows up on the first cast instead of on a probe nobody asked for.
    let took = magic.entries.get(idx).map(|e| e.param_id) == Some(spell)
        && magic.selected_slot == idx as i32;
    if !took {
        STEER_WRITE_FAILED.fetch_add(1, Ordering::Relaxed);
        static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            crate::log(
                "steering: the write did not take (offset or page is not what we think) — \
                 casts will use the wheel's spell; reported once",
            );
        }
        return false;
    }
    STEER_OK.fetch_add(1, Ordering::Relaxed);
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
/// Ask for the selected spell to be read on the next poll.
///
/// Requested rather than performed by the caller, because this read belongs to the game's thread:
/// it can cost tens of milliseconds where pages are not resident, and reading it from the panel
/// thread races a teardown that the rest of this module refuses to race for writes. The poll thread
/// serves the request, so the value is at most one poll old - and the report prints how old.
pub(crate) fn request_selection_sample() {
    WANT_SELECTION.store(true, Ordering::Relaxed);
}

/// When the selected spell was last read, on the caller's millisecond clock. Zero means never.
pub(crate) static PLAYER_SPELL_AT: AtomicU32 = AtomicU32::new(0);

/// Serve a pending request. Called from the poll path: one atomic load when nobody asked.
pub(crate) fn serve_selection_request(now: u32) {
    if !WANT_SELECTION.swap(false, Ordering::Relaxed) {
        return;
    }
    if magic_data_verified().is_some() {
        sample_now();
        PLAYER_SPELL_AT.store(now, Ordering::Relaxed);
    }
}

static WANT_SELECTION: AtomicBool = AtomicBool::new(false);

/// Read the selected spell once. For binding, where the spell captured must be the one selected at
/// the moment the gesture completes, and for a report about to be written.
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
    let raw = raw_player()?;
    let cached = CHAR.load(Ordering::Relaxed);
    let due = now_ms().wrapping_sub(CHAR_CHECKED.load(Ordering::Relaxed)) >= RECHECK_MS;
    if cached == raw && !due {
        return Some(unsafe { &*(raw as *const eldenring::cs::PlayerIns) });
    }
    // Either the character changed or the periodic sanity check is due. This is the whole point
    // of the cache: a `VirtualQuery` per second instead of one per read, on the thread the game
    // polls the pad from.
    // Timed, because it is the one periodic read that was never measured: once a second, on the
    // thread whose stalls a player feels, asking the OS whether the character's block is still there.
    let readable = {
        let _t = crate::padcost::Timer::start(crate::padcost::RECHECK);
        probe::readable(
            raw as *const u8,
            std::mem::size_of::<eldenring::cs::PlayerIns>(),
        )
    };
    if !readable {
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

/// Milliseconds since boot, low 32 bits (wrapping compares are fine).
///
/// `GetTickCount64` reads a shared page; `SystemTime::now` is a syscall, and this runs several
/// times per pad poll on the game's own input thread — up to 250 syscalls a second for a clock
/// nobody needs to be wall-precise.
pub(crate) fn now_ms() -> u32 {
    // One clock for the whole crate: see `crate::millis`.
    crate::millis()
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
    if !probe::readable(
        ptr as *const u8,
        std::mem::size_of::<eldenring::cs::PlayerGameData>(),
    ) {
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
/// cached pair collapses that to one walk per pass.
///
/// 8 ms was chosen because it is the pad poll interval - on a fast machine. A poll interval is not
/// a constant: a reporter's polls arrive every ~28 ms, so an 8 ms cache expired before the next
/// call and *every* poll re-walked the chain, with tail latencies up to 76 ms measured inside our
/// own pad path. 250 ms still sees a weapon swap within a quarter second - a swap is a menu or a
/// deliberate input, not a per-frame event - and turns ~36 walks a second into four.
/// How long the weapon pair is trusted without being re-read.
///
/// Only a net, now that every input is an edge: a press (button or trigger) forces a fresh read,
/// `set_one_handed` drops the cache, and a new character drops it too. Equipment can only change
/// when the player equips something, and equipping means input - including in the inventory menu,
/// where every swap is D-pad and A presses. So the read happens on the event, and the timer exists
/// for one purpose only: a path nobody has thought of must not be able to serve a stale pair
/// forever. Ten seconds is long enough to be invisible and short enough to self-heal.
const WEAPON_CACHE_MS: u32 = 10_000;
static WEAPON_PAIR: AtomicU64 = AtomicU64::new(u64::MAX);
static WEAPON_AT: AtomicU32 = AtomicU32::new(0);

/// Drop the weapon-pair cache, so the next read walks the chain again.
///
/// The cache is 250 ms now, which is what stops a slow machine re-walking game memory on every
/// poll - but a weapon swap is exactly the event the cache must not hide: swapping to a staff and
/// pressing R1 within a quarter second would decide the chord from the pair we already had. A
/// button press is the moment that matters, and there are a handful of them a second at most.
pub(crate) fn invalidate_weapon_cache() {
    WEAPON_AT.store(0, Ordering::Relaxed);
}

/// When input was last seen, so the pair is re-read while the game may still be applying it.
static WEAPON_EDGE_MS: AtomicU32 = AtomicU32::new(0);

/// How long after input the weapon pair is re-read on every poll.
///
/// A read taken at the moment of the press is a read of the *old* weapon: the game applies an
/// equipment change in its own frame, after our hook has already returned that frame's report. So
/// invalidating on the edge is not enough - the read that follows it caches the state the player was
/// trying to leave, stamped as fresh, and the chord a frame later trusts it. For a fifth of a second
/// after any input the cache is simply not believed: one pointer walk per poll, which is microseconds
/// and only while the player is pressing things.
const WEAPON_SETTLE_MS: u32 = 200;

/// Note that the player pressed something. Called from the poll path on any edge.
pub(crate) fn note_input_edge(now: u32) {
    WEAPON_EDGE_MS.store(now, Ordering::Relaxed);
}

pub(crate) fn weapon_ids() -> Option<(i32, i32)> {
    let now = now_ms();
    let cached = WEAPON_PAIR.load(Ordering::Relaxed);
    let settling = now.wrapping_sub(WEAPON_EDGE_MS.load(Ordering::Relaxed)) < WEAPON_SETTLE_MS;
    if !settling
        && cached != u64::MAX
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

/// The `NpcParam` row behind the player's model.
///
/// Logged, not decided on: `ChrIns.character_id` reads 0 on this build, so the ini's class ids
/// never matched, and this is the next candidate for recognising a class port's character. It is
/// recorded on every character load so the rule can be written from measurements rather than from
/// a guess about which field means what.
pub(crate) fn player_npc_param_id() -> Option<u32> {
    Some(player()?.chr_ins.npc_param_id as u32)
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
    if !probe::readable(
        ptr as *const u8,
        std::mem::size_of::<eldenring::cs::EquipMagicData>(),
    ) {
        return None;
    }
    MAGIC.store(ptr as usize, Ordering::Relaxed);
    // SAFETY: readable, owned by the player's game data.
    Some(unsafe { &mut *ptr })
}

/// How long the memorised list is trusted before it is read again.
///
/// The list changes at a grace and when a talisman changes the slot count (Moon of Nokstella adds
/// two), so the event that changes it is a menu this mod does not own and cannot be notified
/// about. A one-second net on top of an explicit refresh at panel open is the same shape as the
/// weapon cache: one verified struct walk, paid at most once a second, and only while balanced
/// mode is actually asking.
const ATTUNED_TTL_MS: u32 = 1000;

/// A "not attuned" answer older than this is re-read before it is believed.
///
/// The cost of believing it wrongly is an input that does nothing for up to a second after a spell
/// is attuned; the cost of re-reading is one verified read. A quarter second keeps both small, and
/// bounds a stale row's checks to four a second instead of one per poll.
const VERIFY_NEGATIVE_MS: u32 = 250;

/// The last read of the character's memorised spells.
struct AttunedCache {
    spells: Vec<i32>,
    read_at: u32,
    /// Whether the last read succeeded. A failed read must not look like "nothing is attuned",
    /// which would grey every row on a warp.
    ok: bool,
}

static ATTUNED: Mutex<AttunedCache> = Mutex::new(AttunedCache {
    spells: Vec::new(),
    read_at: 0,
    ok: false,
});

/// Bumped whenever the published set changes, so a cache built from it (the pad path's bound mask)
/// knows to rebuild. Without this a grace visit would leave the mask saying "bound" for a spell that
/// is no longer attuned.
static ATTUNED_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The generation of the memorised set.
pub(crate) fn attuned_generation() -> u32 {
    ATTUNED_GEN.load(Ordering::Relaxed)
}

/// The spells the character has memorised, in wheel order, with our own steer put back.
///
/// `select_spell` writes a spell into the selected entry, so the raw list can hold the spell we are
/// about to cast instead of the one the player attuned; the borrowed value is substituted back
/// here, or the gate would approve the very spell it is supposed to stop. Duplicates are dropped:
/// the count is what the report prints, and the game does not let the same spell be attuned twice.
pub(crate) fn attuned_spells() -> Option<Vec<i32>> {
    let magic = magic_data_verified()?;
    let originals = borrows();
    let mut spells: Vec<i32> = Vec::with_capacity(magic.entries.len());
    for (idx, entry) in magic.entries.iter().enumerate() {
        let id = original_at(&originals, idx).unwrap_or(entry.param_id);
        if id > 0 && !spells.contains(&id) {
            spells.push(id);
        }
    }
    Some(spells)
}

/// Re-read the memorised list and publish it: at panel open, after a character load, and on the
/// first press after the net expires.
pub(crate) fn refresh_attuned() {
    let read = attuned_spells();
    let read_at = crate::millis();
    let next = read.clone().unwrap_or_default();
    if let Ok(mut cache) = ATTUNED.lock() {
        let changed = cache.ok != read.is_some() || cache.spells.as_slice() != next.as_slice();
        cache.ok = read.is_some();
        cache.spells = next;
        cache.read_at = read_at;
        if changed {
            ATTUNED_GEN.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The published list, re-read when it is older than `ATTUNED_TTL_MS`.
fn attuned_now() -> Option<Vec<i32>> {
    let fresh = {
        let now = crate::millis();
        match ATTUNED.lock() {
            Ok(cache) if cache.ok && now.wrapping_sub(cache.read_at) < ATTUNED_TTL_MS => {
                Some(cache.spells.clone())
            }
            _ => None,
        }
    };
    match fresh {
        Some(spells) => Some(spells),
        None => {
            refresh_attuned();
            ATTUNED
                .lock()
                .ok()
                .and_then(|cache| cache.ok.then(|| cache.spells.clone()))
        }
    }
}

/// Whether balanced mode lets this spell be cast from a catalyst right now.
///
/// Free mode never asks: the whole path is one load of the published config pointer. In balanced
/// mode an unreadable memorised list answers `false` — verifying is what earns a cast, and a
/// teardown is not evidence.
pub(crate) fn spell_attuned(spell: i32) -> bool {
    if crate::config::balance() == nr_casting_core::Balance::Free {
        return true;
    }
    // A "yes" from a slightly old snapshot is harmless: the game refuses a spell the character no
    // longer has, so the worst case is a press that does nothing of ours. A "no" costs the player
    // the input - attune a spell, press, nothing happens for the rest of the second - so the answer
    // that has to be right is the negative one, and that is the one worth re-reading for.
    match attuned_now() {
        Some(spells) if spells.contains(&spell) => true,
        _ => {
            // Re-read only when the answer is old enough to be wrong about a recent attunement, so a
            // permanently stale row costs one read per quarter second rather than one per poll.
            if attuned_age_ms() < VERIFY_NEGATIVE_MS {
                return false;
            }
            refresh_attuned();
            attuned_snapshot().contains(&spell)
        }
    }
}

/// How long ago the published set was read, in milliseconds (saturating).
fn attuned_age_ms() -> u32 {
    match ATTUNED.lock() {
        Ok(cache) if cache.ok => crate::millis().wrapping_sub(cache.read_at),
        _ => u32::MAX,
    }
}

/// The memorised set, for the panel and the report. Cheap: the published list when it is fresh.
/// The published set as it stands, without the freshness net: callers that need the newest answer
/// call `refresh_attuned()` first.
pub(crate) fn attuned_snapshot() -> Vec<i32> {
    ATTUNED
        .lock()
        .ok()
        .filter(|cache| cache.ok)
        .map(|cache| cache.spells.clone())
        .unwrap_or_default()
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
