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
use fromsoftware_shared::FromStatic;

#[derive(Clone, Copy, Debug)]
pub struct LiveSpell {
    pub id: i32,
    pub int: u32,
    pub fai: u32,
    pub arc: u32,
    pub fp: u32,
    pub slots: u32,
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
        // SAFETY: the pointer is owned by the repository and valid for this borrow.
        let file = unsafe { cap.data.as_ref() };
        for id in file.row_ids() {
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
                slots: row.slot_length() as u32,
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
        // SAFETY: the pointer is owned by the repository and valid for this borrow.
        let file = unsafe { cap.data.as_ref() };
        for id in file.row_ids() {
            // Weapon rows are 8-digit base ids; the upgrade level is not part of the row id.
            if id < 0x0FFF_FFFF {
                out.push(id as i32);
            }
        }
        break;
    }
    out
}
