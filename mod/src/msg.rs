//! Item names, read out of the game's own text tables.
//!
//! Names are not in the params and not in `regulation.bin`: the game resolves them from FMG
//! text (`msg/<lang>/*.msgbnd.dcx`), and a mod that adds weapons or spells ships its own loose
//! copy of those files. That is why every id a mod adds comes out unnamed from a Paramdex table,
//! and why the panel used to show `mod weapon <id>` for a port's own weapons.
//!
//! The loaded text is reachable in-process through the `MsgRepository` singleton. The vendored
//! crate already declares it (`#[shared::singleton("MsgRepository")]`), and singleton lookup goes
//! through the executable's DLRF reflection data by name, so the pointer resolves on any build
//! without an RVA entry. Only the layout was missing:
//!
//! ```text
//! MsgRepositoryImp
//!   +0x08  u8***  slots-by-layer: [layer][slot] -> FMG buffer     (layer 0 = base game)
//!   +0x14  i32    slot count
//! ```
//!
//! The FMG buffer itself is parsed by [`nr_casting_core::fmg`], which is where the header, the
//! group table and the string-offset rules live — and where they are tested without a game.
//! Those offsets are also encoded in `tools/msgdump.py` for reading the same files offline; keep
//! the two in step when either changes.
//!
//! Two things this has to get right:
//!
//! * **Every msgbnd layer is its own slot** — base `g`, `_dlc01` at `g + 310`, `_dlc02` at
//!   `g + 410` — and the game merges them at lookup time, highest layer first. A mod can put its
//!   added strings in a DLC layer, so reading the base slot alone silently misses them.
//! * **A layout is only trusted after it proves itself**, and the proof is structural: a coherent
//!   group table plus two known ids returning two distinct, non-empty strings. It is deliberately
//!   *not* a comparison against English text: on a German install those same rows read
//!   "Krummsäbel" and "Adulas Mondklinge", and an English expectation would switch the feature off
//!   for everyone who does not play in English.
//!
//! When nothing can be proven, every caller keeps its compiled table and the panel falls back to
//! the raw id (`mod weapon 1234`), exactly as before any of this existed.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use eldenring::cs::MsgRepositoryImp;
use fromsoftware_shared::FromStatic;
use nr_casting_core::fmg::{self, Group};

use crate::logging::debug;

/// FMG slot for a group in the base layer, `_dlc01` and `_dlc02`: the runtime keeps each layer in
/// its own slot and merges them at lookup time. Highest layer first, like the game does.
const LAYER_OFFSETS: [usize; 3] = [410, 310, 0];

/// Two ids from two different groups that every install has. They are used only to prove the
/// table is readable and self-consistent, never to match a particular language: the two texts are
/// compared *with each other*, not against expected strings.
const PROBE_IDS: [(u32, u32); 2] = [
    // GoodsName: the spell "Adula's Moonblade" (spell ids are goods ids).
    (10, 4431),
    // WeaponName: "Scimitar".
    (11, 7_140_000),
];

/// Header offsets inside an FMG buffer.
const FMG_VERSION: usize = 0x02;
const FMG_GROUP_COUNT: usize = 0x0C;
const FMG_STRING_COUNT: usize = 0x10;
const FMG_OFFSETS: usize = 0x18;
const FMG_GROUPS: usize = 0x28;
const GROUP_SIZE: usize = 0x10;
/// A value below this is a base-relative offset, above it an absolute pointer (the runtime
/// relocates the offset table on load; the game's own reader does the same test).
const ABSOLUTE_POINTER: u64 = 0x100_0000;
/// Sanity ceilings for the two header counts. The largest real table in the groups this module
/// reads is 3,295 ranges over 4,285 strings, so these are ~20x headroom; they exist so a corrupt
/// header cannot make the parser allocate or walk megabytes before it is rejected.
const MAX_GROUPS: u32 = 0x1_0000;
const MAX_STRINGS: u32 = 0x10_0000;

/// Whether the layout has been proven this session.
static PROVEN: AtomicBool = AtomicBool::new(false);

/// Page-granular readability cache: `probe::readable` is a `VirtualQuery` per call, and a lookup
/// touches the header, the group table, the offset table and the string. Remembering which pages
/// are mapped keeps a panel rebuild from costing thousands of syscalls.
struct Mem {
    ok: HashSet<usize>,
    bad: HashSet<usize>,
}

impl Mem {
    fn new() -> Self {
        Mem {
            ok: HashSet::new(),
            bad: HashSet::new(),
        }
    }

    fn readable(&mut self, addr: usize, len: usize) -> bool {
        if addr == 0 || len == 0 {
            return false;
        }
        // Windows regions are page-aligned and page-multiples, so one probe settles a whole page.
        let first = addr & !0xFFF;
        let last = (addr + len - 1) & !0xFFF;
        let mut page = first;
        loop {
            if self.bad.contains(&page) {
                return false;
            }
            if !self.ok.contains(&page) {
                if crate::probe::readable(page as *const u8, 0x1000) {
                    self.ok.insert(page);
                } else {
                    self.bad.insert(page);
                    return false;
                }
            }
            if page == last {
                return true;
            }
            page += 0x1000;
        }
    }

    fn u8(&mut self, addr: usize) -> Option<u8> {
        if !self.readable(addr, 1) {
            return None;
        }
        // SAFETY: checked readable for exactly one byte.
        Some(unsafe { *(addr as *const u8) })
    }

    fn u32(&mut self, addr: usize) -> Option<u32> {
        if !self.readable(addr, 4) {
            return None;
        }
        // SAFETY: checked readable for four bytes; unaligned because these are packed game
        // structures with no alignment guarantee we can rely on.
        Some(unsafe { (addr as *const u32).read_unaligned() })
    }

    fn u64(&mut self, addr: usize) -> Option<u64> {
        if !self.readable(addr, 8) {
            return None;
        }
        // SAFETY: as above.
        Some(unsafe { (addr as *const u64).read_unaligned() })
    }

    /// A string from game memory: collect the UTF-16 units (terminator included, because the
    /// decoder in `nr_casting_core::fmg` refuses a string that runs off the end of its slice),
    /// then let that tested decoder decide whether what came back is text or binary noise.
    fn text(&mut self, addr: usize) -> Option<String> {
        if addr < 0x10000 {
            return None;
        }
        let mut raw = Vec::with_capacity(64);
        let mut terminated = false;
        for i in 0..256 {
            let at = addr.checked_add(i * 2)?;
            if !self.readable(at, 2) {
                return None;
            }
            // SAFETY: checked readable for two bytes.
            let unit = unsafe { (at as *const u16).read_unaligned() };
            raw.extend_from_slice(&unit.to_le_bytes());
            if unit == 0 {
                terminated = true;
                break;
            }
        }
        if !terminated {
            return None;
        }
        fmg::decode_utf16(&raw)
    }
}

/// The repository's slot table, resolved fresh on every use: a reload replaces it, and caching the
/// address across one would be reading a freed object.
struct Slots {
    slots: usize,
    count: u32,
}

fn slots(mem: &mut Mem) -> Option<Slots> {
    // SAFETY: the singleton is resolved by name out of the executable's reflection data; the
    // pointer is only read through the checked helpers.
    let repo = unsafe { MsgRepositoryImp::instance() }.ok()? as *const MsgRepositoryImp as usize;
    if repo == 0 {
        return None;
    }
    let layer_array = mem.u64(repo + 0x08)? as usize;
    let count = mem.u32(repo + 0x14)?;
    if layer_array < 0x10000 || count == 0 || count > 0x2000 {
        return None;
    }
    let slots = mem.u64(layer_array)? as usize;
    if slots < 0x10000 {
        return None;
    }
    Some(Slots { slots, count })
}

/// One layer's FMG buffer for a group: its header, and its group table read once so a sweep over
/// many ids does not re-read it per id.
struct Fmg {
    base: usize,
    offsets: usize,
    string_count: u32,
    groups: Vec<Group>,
}

impl Fmg {
    /// Parse one layer's buffer.
    ///
    /// A `Result` with a reason, not a bare `Option`: "the table did not parse" is the one thing
    /// that makes every name disappear, and a reason in the debug log turns that from a mystery
    /// into a line. The public shape stays `Option` — callers only care whether it worked.
    fn parse(mem: &mut Mem, base: usize) -> Result<Fmg, &'static str> {
        if base < 0x10000 {
            return Err("pointer below the image");
        }
        if mem.u8(base + FMG_VERSION).ok_or("version unreadable")? != 2 {
            return Err("not an FMG v2 buffer");
        }
        let group_count = mem.u32(base + FMG_GROUP_COUNT).ok_or("group count unreadable")?;
        let string_count = mem.u32(base + FMG_STRING_COUNT).ok_or("string count unreadable")?;
        if group_count == 0 || group_count > MAX_GROUPS {
            return Err("implausible group count");
        }
        if string_count == 0 || string_count > MAX_STRINGS {
            return Err("implausible string count");
        }
        // Every range covers at least one id, so more ranges than strings is not a table.
        if group_count > string_count {
            return Err("more groups than strings");
        }
        let raw = mem.u64(base + FMG_OFFSETS).ok_or("offset table unreadable")?;
        let offsets = fmg::resolve(raw, base, ABSOLUTE_POINTER).ok_or("offset table pointer")?;
        if !mem.readable(offsets, 8) {
            return Err("offset table not mapped");
        }

        let mut groups = Vec::with_capacity((group_count as usize).min(4096));
        for i in 0..group_count {
            let at = base + FMG_GROUPS + i as usize * GROUP_SIZE;
            let group = mem
                .u32(at)
                .zip(mem.u32(at + 4))
                .zip(mem.u32(at + 8))
                .map(|((si, first), last)| (si, first, last))
                .ok_or("group table unreadable")?;
            groups.push(group);
        }
        // The table has to describe itself before anything is indexed with it.
        if !fmg::groups_are_coherent(&groups, string_count) {
            debug!(
                "msg: group table at {base:#x} incoherent (groups={group_count} strings={string_count} first={:?})",
                groups.first()
            );
            return Err("incoherent group table");
        }
        Ok(Fmg {
            base,
            offsets,
            string_count,
            groups,
        })
    }

    fn at(mem: &mut Mem, base: usize) -> Option<Fmg> {
        match Self::parse(mem, base) {
            Ok(fmg) => Some(fmg),
            Err(reason) => {
                debug!("msg: no FMG at {base:#x}: {reason}");
                None
            }
        }
    }

    fn text(&self, mem: &mut Mem, id: u32) -> Option<String> {
        let group = *self.groups.get(fmg::group_index(&self.groups, id)?)?;
        let index = fmg::string_index(group, id, self.string_count)?;
        let raw = mem.u64(self.offsets + index as usize * 8)?;
        let at = fmg::resolve(raw, self.base, ABSOLUTE_POINTER)?;
        mem.text(at).filter(|text| !fmg::is_error_marker(text))
    }
}

/// The FMG buffers for one group across the layers, in the game's own priority order.
fn layer_fmgs(mem: &mut Mem, slots: &Slots, group: u32) -> Vec<Fmg> {
    let mut out = Vec::with_capacity(LAYER_OFFSETS.len());
    for offset in LAYER_OFFSETS {
        let slot = group as usize + offset;
        if slot >= slots.count as usize {
            continue;
        }
        if let Some(ptr) = mem.u64(slots.slots + slot * 8) {
            if let Some(fmg) = Fmg::at(mem, ptr as usize) {
                out.push(fmg);
            }
        }
    }
    out
}

/// Name for one id, walking the layers highest-first like the engine does.
fn lookup_in(mem: &mut Mem, fmgs: &mut [Fmg], id: u32) -> Option<String> {
    fmgs.iter_mut().find_map(|fmg| fmg.text(mem, id))
}

/// True once the layout has been proven on this session.
pub fn ready() -> bool {
    PROVEN.load(Ordering::Relaxed)
}

/// Attempts made this session, and when the last one ran: the repository is empty until the
/// regulation is loaded, so a first attempt can legitimately find nothing.
static ATTEMPTS: AtomicU32 = AtomicU32::new(0);
static LAST_ATTEMPT_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Prove the layout from the live process. Returns `true` once proven.
///
/// The proof is structural, in this order: the repository resolves, both known groups parse with a
/// coherent group table, and two known ids from two different groups return two distinct,
/// non-empty strings. Text is never compared against English — see the module docs for why.
pub fn probe() -> bool {
    if ready() {
        return true;
    }
    if ATTEMPTS.load(Ordering::Relaxed) >= 6 {
        return false;
    }
    let now = now_ms();
    let last = LAST_ATTEMPT_MS.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) < 10_000 {
        return false;
    }
    LAST_ATTEMPT_MS.store(now, Ordering::Relaxed);
    ATTEMPTS.fetch_add(1, Ordering::Relaxed);

    let mut mem = Mem::new();
    let Some(slot_table) = slots(&mut mem) else {
        debug!("msg: MsgRepository not ready yet");
        return false;
    };

    let mut seen: Vec<String> = Vec::with_capacity(PROBE_IDS.len());
    for (group, id) in PROBE_IDS {
        let mut fmgs = layer_fmgs(&mut mem, &slot_table, group);
        if fmgs.is_empty() {
            debug!("msg: group {group} table not readable yet");
            return false;
        }
        let Some(text) = lookup_in(&mut mem, &mut fmgs, id) else {
            debug!("msg: group {group} id {id} not found yet");
            return false;
        };
        if seen.contains(&text) {
            // Two different ids resolving to the same text means the index arithmetic or the
            // slot pairing is wrong, whatever the strings themselves say.
            crate::log(&format!(
                "msg: layout rejected — {PROBE_IDS:?} both read {text:?}; keeping the compiled table"
            ));
            return false;
        }
        debug!("msg: group {group} id {id} reads {text:?}");
        seen.push(text);
    }

    PROVEN.store(true, Ordering::Relaxed);
    crate::log(&format!(
        "msg: text tables proven ({} slots); mod-added names will be read from the game",
        slot_table.count
    ));
    true
}

/// Resolved names, and the ids that resolved to nothing, so a panel reopen does not re-read the
/// repository for an answer that cannot have changed.
static CACHE: Mutex<Option<HashMap<(u32, i32), String>>> = Mutex::new(None);
static MISSES: Mutex<Option<HashSet<(u32, i32)>>> = Mutex::new(None);

/// Names for the requested ids of one FMG group, straight out of the game's loaded text.
///
/// Returns only what it could resolve: callers keep their own table and use this to fill gaps, so
/// an unproven layout or a reload costs the fallback (`mod weapon <id>`), never a wrong name.
pub fn names(group: u32, wanted: &[i32]) -> HashMap<i32, String> {
    let mut out = HashMap::new();
    if wanted.is_empty() || !ready() {
        return out;
    }
    let mut misses: HashSet<i32> = HashSet::new();
    {
        let Ok(cache) = CACHE.lock() else {
            return out;
        };
        let miss = MISSES.lock().ok();
        for &id in wanted {
            if let Some(name) = cache.as_ref().and_then(|c| c.get(&(group, id))) {
                out.insert(id, name.clone());
            } else if miss
                .as_ref()
                .and_then(|m| m.as_ref())
                .is_some_and(|m| m.contains(&(group, id)))
            {
                misses.insert(id);
            }
        }
    }

    let todo: Vec<i32> = wanted
        .iter()
        .copied()
        .filter(|id| !out.contains_key(id) && !misses.contains(id))
        .collect();
    if todo.is_empty() {
        return out;
    }

    let (found, answered) = scan(group, &todo);
    record(group, &todo, &found, answered);
    out.extend(found);
    out
}

/// When the last scan found the repository unreadable. A reload frees the tables for a moment,
/// and this call sits on a render path: without the floor, every frame of a panel open during a
/// load would walk and re-parse the group tables.
static LAST_UNREADABLE_MS: AtomicU64 = AtomicU64::new(0);

/// How long to trust "not readable" before trying again.
const UNREADABLE_RETRY_MS: u64 = 250;

/// One id, for a caller on a render path: a cached answer costs two mutex locks and no game reads.
pub fn name(group: u32, id: i32) -> Option<String> {
    if !ready() {
        return None;
    }
    if let Some(cached) = CACHE
        .lock()
        .ok()
        .and_then(|c| c.as_ref()?.get(&(group, id)).cloned())
    {
        return Some(cached);
    }
    if MISSES
        .lock()
        .ok()
        .is_some_and(|m| m.as_ref().is_some_and(|s| s.contains(&(group, id))))
    {
        return None;
    }
    // A recent failed read means the repository is between lives; do not pay for it again on
    // the next frame.
    let now = now_ms();
    let unreadable_at = LAST_UNREADABLE_MS.load(Ordering::Relaxed);
    if unreadable_at != 0 && now.wrapping_sub(unreadable_at) < UNREADABLE_RETRY_MS {
        return None;
    }
    let (found, answered) = scan(group, &[id]);
    if answered {
        LAST_UNREADABLE_MS.store(0, Ordering::Relaxed);
    } else {
        LAST_UNREADABLE_MS.store(now, Ordering::Relaxed);
    }
    record(group, &[id], &found, answered);
    found.get(&id).cloned()
}

/// Store what a scan learned. A miss is only final when the scan actually got an answer from a
/// proven layout — otherwise it is "not readable yet" and must be retried.
fn record(group: u32, asked: &[i32], found: &HashMap<i32, String>, answered: bool) {
    if !answered {
        return;
    }
    if let Ok(mut cache) = CACHE.lock() {
        let map = cache.get_or_insert_with(HashMap::new);
        for (id, name) in found {
            map.insert((group, *id), name.clone());
        }
    }
    if let Ok(mut miss) = MISSES.lock() {
        let set = miss.get_or_insert_with(HashSet::new);
        for id in asked {
            if !found.contains_key(id) {
                set.insert((group, *id));
            }
        }
    }
}

/// One sweep for `wanted` ids of one group: the layer tables are read once, then each id costs an
/// offset read and a string.
///
/// Returns the names found and whether the walk reached the repository at all: a proven layout
/// whose repository is momentarily unreadable (a reload) must not be recorded as "no name".
fn scan(group: u32, wanted: &[i32]) -> (HashMap<i32, String>, bool) {
    let mut out = HashMap::new();
    let mut mem = Mem::new();
    let Some(slots) = slots(&mut mem) else {
        return (out, false);
    };
    let mut fmgs = layer_fmgs(&mut mem, &slots, group);
    if fmgs.is_empty() {
        return (out, false);
    }
    for &id in wanted {
        if let Some(text) = lookup_in(&mut mem, &mut fmgs, id as u32) {
            out.insert(id, text);
        }
    }
    (out, true)
}
