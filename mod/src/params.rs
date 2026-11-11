//! The live spell catalogue, read out of the running game's own params.
//!
//! A generated table goes stale the moment the game patches. Everything numeric about a
//! spell — FP cost, slot count, INT/FAI/ARC requirements — is in `Magic.param`, which is
//! loaded into memory and reachable through `SoloParamRepository`, so it can be read fresh
//! on every launch instead:
//!
//! 1. enumerate the `Magic` param's row ids (the vendored crate gained `ParamFile::row_ids`
//!    for this; upstream only offers lookups by id),
//! 2. look each row up through the typed accessor to read the numbers.
//!
//! Names are deliberately *not* here: they are not in the params. The game's text lives in
//! FMG assets inside the encrypted archive, so names still come from the compiled-in table.
//! A spell added by a patch therefore appears with correct requirements and a placeholder
//! name until the table is regenerated — assignable and correctly filtered either way.

use eldenring::cs::{EquipParamWeapon, Magic, SoloParam, SoloParamRepository};
use eldenring::fd4::ParamFile;
use fromsoftware_shared::FromStatic;

#[derive(Clone, Copy, Debug)]
pub struct LiveSpell {
    pub id: i32,
    pub int: u32,
    pub fai: u32,
    pub arc: u32,
    pub fp: u32,
}

/// Read every `Magic` row. Returns an empty vector if the repository is not up yet or the
/// lookup fails, which makes the caller fall back to the table.
pub fn live_spells() -> Vec<LiveSpell> {
    // SAFETY: singleton; the probe has already established the game's structures are sane.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for cap in repo.params() {
        if cap.res_cap.name != Magic::NAME {
            continue;
        }
        let ptr = cap.data.as_ptr();
        // A reload can release the param file while the repository entry still points at it, so
        // validate the pointer before walking its row table.
        if !crate::probe::readable(ptr as *const u8, std::mem::size_of::<ParamFile>()) {
            continue;
        }
        // SAFETY: readable, owned by the repository, re-checked on every call.
        let file = cap.data.as_ref();
        // Bounded: these tables hold a few hundred rows (spells) to a few thousand (weapons).
        for id in file.row_ids().take(200_000) {
            // 999999999 is the game's "no spell" sentinel row, not a castable spell.
            if id >= 999_999_999 {
                continue;
            }
            let Some(row) = repo.get::<Magic>(id) else {
                continue;
            };
            out.push(LiveSpell {
                id: id as i32,
                // The generated param structs keep fields private and expose accessors.
                int: row.requirement_intellect() as u32,
                fai: row.requirement_faith() as u32,
                arc: row.requirement_luck() as u32,
                fp: row.mp().max(0) as u32,
            });
        }
        break;
    }
    out
}

/// Read every `EquipParamWeapon` row id. Names are not in the params (they are community
/// annotations), but the row set is: comparing it against the compiled table tells us whether
/// a game update added weapons the table cannot name yet.
pub fn live_weapon_ids() -> Vec<i32> {
    // SAFETY: singleton; the probe has already established the game's structures are sane.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for cap in repo.params() {
        if cap.res_cap.name != EquipParamWeapon::NAME {
            continue;
        }
        let ptr = cap.data.as_ptr();
        // A reload can release the param file while the repository entry still points at it, so
        // validate the pointer before walking its row table.
        if !crate::probe::readable(ptr as *const u8, std::mem::size_of::<ParamFile>()) {
            continue;
        }
        // SAFETY: readable, owned by the repository, re-checked on every call.
        let file = cap.data.as_ref();
        // Bounded: these tables hold a few hundred rows (spells) to a few thousand (weapons).
        for id in file.row_ids().take(200_000) {
            // Weapon rows are 8-digit base ids; the upgrade level is not part of the row id.
            if id < 0x0FFF_FFFF {
                out.push(id as i32);
            }
        }
        break;
    }
    out
}

/// Byte offsets of the two catalyst flag bytes inside an `EquipParamWeapon` row.
///
/// The vendored bindings generate these two fields without accessors (`bits_105` / `bits_106` are
/// private), so they are read by offset - validated before use, because an offset that moved with a
/// patch must not be believed. `flags_validated` proves the offset still lands on the two bits by
/// checking a known staff and a known seal.
const BITS_105_OFFSET: usize = 0x105;
const BITS_106_OFFSET: usize = 0x106;

/// The `(bits_105, bits_106)` pair of a weapon's param row.
pub fn catalyst_flags(weapon: i32) -> Option<(u8, u8)> {
    if weapon <= 0 {
        return None;
    }
    let base = nr_casting_core::base_weapon_id(weapon);
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return None;
    };
    let row = repo.get::<EquipParamWeapon>(base as u32)?;
    let size = std::mem::size_of_val(row);
    if size <= BITS_106_OFFSET {
        return None;
    }
    // SAFETY: the row came from the repository, is `size` bytes of a plain struct, and both offsets
    // were bounds-checked against that size.
    let bytes = unsafe { std::slice::from_raw_parts(row as *const _ as *const u8, size) };
    Some((bytes[BITS_105_OFFSET], bytes[BITS_106_OFFSET]))
}

/// Whether the flag offsets still mean what they meant when they were measured.
///
/// Checked once per process against two rows whose answer cannot change: a vanilla glintstone staff
/// casts sorceries, a vanilla finger seal casts incantations. If either reads otherwise, the offsets
/// have moved and the caller falls back to the id band rather than mislabelling every catalyst in
/// the game.
pub fn flags_validated() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        use nr_casting_core::CatalystSchools;
        let staff =
            catalyst_flags(33_000_000).map(|(a, b)| nr_casting_core::schools_from_flags(a, b));
        let seal =
            catalyst_flags(34_000_000).map(|(a, b)| nr_casting_core::schools_from_flags(a, b));
        let ok =
            staff == Some(CatalystSchools::SORCERY) && seal == Some(CatalystSchools::INCANTATION);
        crate::log(if ok {
            "catalyst: school flags read from the weapon rows (staff and seal both check out)"
        } else {
            "catalyst: school flags did not validate against a known staff/seal - using the id band"
        });
        ok
    })
}

/// The weapon's `EquipParamWeapon.weaponCategory`, for the log.
///
/// Logged rather than used as a test: the Paramdex enum for this field is coarse and the numbers
/// differ between builds, so classification is by weapon name (see `overlay::weapon_name_base`)
/// and this is the evidence needed to tighten that later without guessing.
pub fn weapon_category(id: i32) -> Option<u8> {
    if id <= 0 {
        return None;
    }
    let base = nr_casting_core::base_weapon_id(id);
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return None;
    };
    Some(repo.get::<EquipParamWeapon>(base as u32)?.weapon_category())
}
