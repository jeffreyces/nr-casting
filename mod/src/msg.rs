//! Item names, read out of the game's own text tables.
//!
//! Names are not in the params and not in `regulation.bin`: the game resolves them from FMG
//! text (`msg/<lang>/*.msgbnd.dcx`), and a mod that adds weapons or spells ships its own loose
//! copy of those files. That is why every id a mod adds comes out unnamed from a Paramdex table,
//! and why the panel used to show `mod weapon <id>` for a port's own weapons.
//!
//! The loaded text is reachable in-process through the `MsgRepository` singleton. The vendored
//! crate already declares it (`#[shared::singleton("MsgRepository")]`), and singleton lookup goes
//! through the executable's DLRF reflection data by *name*, so the pointer resolves on any build
//! without an RVA entry — only the layout was missing, and it is:
//!
//! ```text
//! MsgRepositoryImp
//!   +0x08  u8***  slots-by-layer: [layer][slot] -> FMG buffer
//!   +0x14  i32    slot count
//!
//! FMG buffer (the .fmg body exactly as it sits in a msgbnd, minus the "FMG " magic)
//!   +0x02  u8     version (2)
//!   +0x0C  u32    group count
//!   +0x10  u32    string count
//!   +0x18  u64    string-offset table: absolute pointer if > 0x1000000, else an
//!                 offset from the FMG base
//!   +0x28  groups, 16 bytes each: { i32 string_index, i32 first_id, i32 last_id, i32 pad }
//!   string_offsets: u64[string_count], each an absolute pointer or a base-relative
//!                   offset to a NUL-terminated UTF-16LE string
//! ```
//!
//! Name ids are the param row ids (`7140000` = Scimitar in both `EquipParamWeapon` and
//! `WeaponName`; spells are goods ids, `4431` = Adula's Moonblade), so a lookup is
//! `(group, row id)` with group 10 = `GoodsName` and 11 = `WeaponName`.
//!
//! Two things this has to get right, both learned the hard way:
//!
//! * **Every msgbnd layer is its own slot** — base `g`, `_dlc01` at `g + 310`, `_dlc02` at
//!   `g + 410` — and the game merges them at lookup time, highest layer first. A mod can put
//!   its added strings in a DLC layer, so reading the base slot alone silently misses them.
//! * **A layout is only trusted after it answers a known question.** `probe()` looks up two
//!   base-game rows and compares the text before any of this is used; until then every caller
//!   keeps its compiled table.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use eldenring::cs::MsgRepositoryImp;
use fromsoftware_shared::FromStatic;

use crate::logging::debug;

/// FMG slot for a group in the base layer, `_dlc01` and `_dlc02`: the runtime keeps each layer
/// in its own slot and merges them at lookup time. Highest layer first, like the game does.
const LAYER_OFFSETS: [usize; 3] = [410, 310, 0];

/// `(group, id, expected text)`: base-game rows every install has, in the player's own language.
const KNOWN: [(u32, u32, &str); 2] = [
    // GoodsName: the spell "Adula's Moonblade" (spell ids are goods ids).
    (10, 4431, "Adula's Moonblade"),
    // WeaponName: "Scimitar".
    (11, 7_140_000, "Scimitar"),
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

static LAYOUT: Mutex<Option<()>> = Mutex::new(None);

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

    /// NUL-terminated UTF-16, bounded, rejecting anything that decodes to binary noise (an
    /// offset that lands in the middle of a string table must not produce a name).
    fn utf16(&mut self, addr: usize) -> Option<String> {
        if addr < 0x10000 {
            return None;
        }
        let mut units = Vec::with_capacity(48);
        for i in 0..256 {
            let at = addr.checked_add(i * 2)?;
            if !self.readable(at, 2) {
                return None;
            }
            // SAFETY: checked readable for two bytes.
            let unit = unsafe { (at as *const u16).read_unaligned() };
            if unit == 0 {
                return if units.is_empty() {
                    None
                } else {
                    Some(String::from_utf16_lossy(&units))
                };
            }
            if (unit < 0x20 && unit != u16::from(b'\n')) || unit == 0xFFFF {
                return None;
            }
            units.push(unit);
        }
        None
    }
}

/// The repository and its slot table, resolved fresh on every use: a reload replaces both, and
/// caching the addresses across one would be reading a freed object.
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

/// An FMG buffer's header, validated.
struct Fmg {
    base: usize,
    groups: usize,
    group_count: u32,
    offsets: usize,
    string_count: u32,
}

impl Fmg {
    fn at(mem: &mut Mem, base: usize) -> Option<Fmg> {
        if base < 0x10000 {
            return None;
        }
        // A stale or foreign slot pointer would otherwise send us through a garbage group table.
        if mem.u8(base + FMG_VERSION)? != 2 {
            return None;
        }
        let group_count = mem.u32(base + FMG_GROUP_COUNT)?;
        let string_count = mem.u32(base + FMG_STRING_COUNT)?;
        if group_count == 0 || group_count > 0x10_0000 || string_count > 0x10_0000 {
            return None;
        }
        let raw = mem.u64(base + FMG_OFFSETS)?;
        let offsets = if raw > ABSOLUTE_POINTER {
            raw as usize
        } else {
            base.checked_add(raw as usize)?
        };
        if !mem.readable(offsets, 8) {
            return None;
        }
        Some(Fmg {
            base,
            groups: base + FMG_GROUPS,
            group_count,
            offsets,
            string_count,
        })
    }

    /// `(string_index, first_id, last_id)` of the group holding `id`, by binary search over a
    /// group table that is sorted by first id.
    fn group_of(&self, mem: &mut Mem, id: u32) -> Option<(u32, u32, u32)> {
        let group = |mem: &mut Mem, i: u32| -> Option<(u32, u32, u32)> {
            let at = self.groups + i as usize * GROUP_SIZE;
            let string_index = mem.u32(at)?;
            let first = mem.u32(at + 4)?;
            let last = mem.u32(at + 8)?;
            Some((string_index, first, last))
        };
        let (mut lo, mut hi) = (0u32, self.group_count);
        let mut candidate: Option<(u32, u32, u32)> = None;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let (si, first, last) = group(mem, mid)?;
            if id < first {
                hi = mid;
            } else if id > last {
                lo = mid + 1;
            } else {
                // Group tables can cover an id in more than one range; the first hit is fine,
                // the game takes the same one.
                candidate = Some((si, first, last));
                break;
            }
        }
        // Fall back to a bounded linear scan only when the table is small: a table whose ranges
        // overlap in a way binary search misses should still not cost a full walk per name.
        if candidate.is_none() && self.group_count <= 4096 {
            for i in 0..self.group_count {
                let (si, first, last) = group(mem, i)?;
                if id >= first && id <= last {
                    candidate = Some((si, first, last));
                    break;
                }
            }
        }
        candidate
    }

    fn text(&self, mem: &mut Mem, id: u32) -> Option<String> {
        let (string_index, first, _) = self.group_of(mem, id)?;
        let index = string_index.checked_add(id.checked_sub(first)?)?;
        if index >= self.string_count {
            return None;
        }
        let raw = mem.u64(self.offsets + index as usize * 8)?;
        if raw == 0 {
            return None;
        }
        let at = if raw > ABSOLUTE_POINTER {
            raw as usize
        } else {
            self.base.checked_add(raw as usize)?
        };
        mem.utf16(at)
    }
}

/// The game's own "no text here" marker. An FMG entry that exists but holds this is a missing
/// name, not a name: every mod that ships a msg file leaves rows like this behind, and the game
/// renders the marker instead of a title.
fn placeholder(text: &str) -> bool {
    text.is_empty() || text == "[ERROR]"
}

/// Name for one id of one FMG group, walking the layers in the game's own priority order.
fn lookup(mem: &mut Mem, group: u32, id: u32) -> Option<String> {
    let slots = slots(mem)?;
    lookup_with(mem, &slots, group, id)
}

/// The same walk with the repository already resolved, for callers asking about many ids.
fn lookup_with(mem: &mut Mem, slots: &Slots, group: u32, id: u32) -> Option<String> {
    for offset in LAYER_OFFSETS {
        let slot = group as usize + offset;
        if slot >= slots.count as usize {
            continue;
        }
        let Some(fmg_ptr) = mem.u64(slots.slots + slot * 8) else {
            continue;
        };
        let Some(fmg) = Fmg::at(mem, fmg_ptr as usize) else {
            continue;
        };
        if let Some(text) = fmg.text(mem, id) {
            if !placeholder(&text) {
                return Some(text);
            }
        }
    }
    None
}

/// True once the layout has answered a known question on this session.
pub fn ready() -> bool {
    LAYOUT.lock().map(|l| l.is_some()).unwrap_or(false)
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

/// Prove the layout against rows the game itself can confirm. Returns `true` once proven.
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
    debug!("msg: repository slots={:#x} count={}", slot_table.slots, slot_table.count);

    for (group, id, want) in KNOWN {
        match lookup(&mut mem, group, id) {
            Some(text) if text == want => {}
            Some(text) => {
                crate::log(&format!(
                    "msg: layout mismatch — group {group} id {id} read {text:?}, expected {want:?}; keeping the table"
                ));
                return false;
            }
            None => {
                debug!("msg: group {group} id {id} not found yet");
                return false;
            }
        }
    }

    if let Ok(mut guard) = LAYOUT.lock() {
        *guard = Some(());
    }
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
/// an unproven layout or a reload costs a placeholder, never a wrong name.
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

/// One id, for a caller on a render path: cached answers cost two mutex locks and no game reads.
pub fn name(group: u32, id: i32) -> Option<String> {
    if !ready() {
        return None;
    }
    if let Some(cached) = CACHE.lock().ok().and_then(|c| c.as_ref()?.get(&(group, id)).cloned()) {
        return Some(cached);
    }
    if MISSES
        .lock()
        .ok()
        .is_some_and(|m| m.as_ref().is_some_and(|s| s.contains(&(group, id))))
    {
        return None;
    }
    let (found, answered) = scan(group, &[id]);
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

/// One sweep of the repository for `wanted` ids of one group.
///
/// Returns the names found and whether the walk actually reached the record vector: a layout that
/// is proven but whose repository is momentarily unreadable (a reload) must not be recorded as
/// "this id has no name".
fn scan(group: u32, wanted: &[i32]) -> (HashMap<i32, String>, bool) {
    let mut out = HashMap::new();
    let mut mem = Mem::new();
    let Some(slots) = slots(&mut mem) else {
        return (out, false);
    };
    for &id in wanted {
        if let Some(text) = lookup_with(&mut mem, &slots, group, id as u32) {
            out.insert(id, text);
        }
    }
    (out, true)
}
