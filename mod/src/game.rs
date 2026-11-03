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
    let snapshot = {
        let mut slot = BORROWED.lock().unwrap_or_else(|e| e.into_inner());
        slot.take()
    };
    BORROW_ACTIVE.store(false, Ordering::Relaxed);
    STEERED.store(-1, Ordering::Relaxed);
    let Some(snapshot) = snapshot else {
        return false;
    };
    // From here the borrow is gone and the pointer is about to be walked: everything below this
    // line is game memory, whatever it returns.
    let Some(magic) = magic_data_verified() else {
        return true;
    };
    let Some(entry) = magic.entries.get_mut(snapshot.idx) else {
        return true;
    };
    // Only undo our own write. If the entry no longer holds the spell we steered, someone else
    // (the game rebuilding the wheel, another selection mod) owns the value now.
    if entry.param_id != snapshot.steered {
        return true;
    }
    entry.param_id = snapshot.param_id;
    entry.charges = snapshot.charges;
    true
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

/// How many writes to the magic entry read back correctly, and how many did not.
///
/// The report used to say "the write path is checked on the first cast" and then never say what
/// the check found: a claim with no observation behind it. The counters make the claim auditable
/// from the report alone - a session with casts and zero refusals *is* the write verification.
static STEER_OK: AtomicU32 = AtomicU32::new(0);
static STEER_REFUSED: AtomicU32 = AtomicU32::new(0);

/// Writes that read back as written, and writes that did not. Zero handles means no cast yet.
pub(crate) fn steer_counts() -> (u32, u32) {
    (
        STEER_OK.load(Ordering::Relaxed),
        STEER_REFUSED.load(Ordering::Relaxed),
    )
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
    // Read back what was just written. This is the whole of the write verification now: the write
    // is one the cast needs anyway, so checking it costs two loads and adds no memory shape of its
    // own — no snapshot, no restore, no synthetic sentinel. A wrong offset or a read-only page
    // shows up on the first cast instead of on a probe nobody asked for.
    let took = magic.entries.get(idx).map(|e| e.param_id) == Some(spell)
        && magic.selected_slot == idx as i32;
    if !took {
        STEER_REFUSED.fetch_add(1, Ordering::Relaxed);
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
    let Some(raw) = raw_player() else { return None };
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

/// The spells the character has memorised, in wheel order, with our own steer put back.
///
/// `select_spell` writes a spell into the selected entry, so the raw list can hold the spell we are
/// about to cast instead of the one the player attuned; the borrowed value is substituted back
/// here, or the gate would approve the very spell it is supposed to stop. Duplicates are dropped:
/// the count is what the report prints, and the game does not let the same spell be attuned twice.
pub(crate) fn attuned_spells() -> Option<Vec<i32>> {
    let magic = magic_data_verified()?;
    let borrowed = BORROWED
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|b| (b.idx, b.param_id)));
    let mut spells: Vec<i32> = Vec::with_capacity(magic.entries.len());
    for (idx, entry) in magic.entries.iter().enumerate() {
        let mut id = entry.param_id;
        if let Some((borrowed_idx, original)) = borrowed {
            if borrowed_idx == idx {
                id = original;
            }
        }
        if id > 0 && !spells.contains(&id) {
            spells.push(id);
        }
    }
    Some(spells)
}

/// Re-read the memorised list and publish it: at panel open, after a character load, and on the
/// first press after the net expires.
pub(crate) fn refresh_attuned() {
    let spells = attuned_spells();
    let read_at = crate::millis();
    if let Ok(mut cache) = ATTUNED.lock() {
        cache.ok = spells.is_some();
        cache.spells = spells.unwrap_or_default();
        cache.read_at = read_at;
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
    if crate::config::load().balance == nr_casting_core::Balance::Free {
        return true;
    }
    attuned_now()
        .map(|spells| spells.contains(&spell))
        .unwrap_or(false)
}

/// The memorised set, for the panel and the report. Cheap: the published list when it is fresh.
pub(crate) fn attuned_snapshot() -> Vec<i32> {
    attuned_now().unwrap_or_default()
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
