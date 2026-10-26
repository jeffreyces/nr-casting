//! Startup resolution and validation.
//!
//! Two jobs:
//!
//! 1. Record what the RVA layer decided — which table, which executable version, and
//!    whether the version was recognised at all — into `nr_casting_offsets.txt` beside
//!    `eldenring.exe`. That file is the map: it is rewritten when the exe changes, and it
//!    is the first thing to look at when a build misbehaves on another machine.
//! 2. Validate the structures the mod reads, field by field, once a player exists. A
//!    fallback table is only a guess, so nothing is written into game memory until these
//!    checks pass; a failure is recorded with the exact check that failed.
//!
//! Failures are never fatal: the mod simply stays out of the way and the log says why.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

const UNVALIDATED: u8 = 0;
const VALIDATED: u8 = 1;
const FAILED: u8 = 2;

static STATE: AtomicU8 = AtomicU8::new(UNVALIDATED);

extern "system" {
    fn GetModuleHandleA(name: *const u8) -> *mut u8;
}

/// True once the structural probe has passed. Steering is refused until then.
pub fn validated() -> bool {
    STATE.load(Ordering::Relaxed) == VALIDATED
}

/// Item param ids are 27-bit (the item-id bitfield reserves the top 4 bits for category),
/// so anything in that range is structurally possible. ER weapon ids are 8 digits with the
/// upgrade level in the low two, which a tighter bound would have rejected.
fn plausible_id(id: i32) -> bool {
    (-1..=0x0FFF_FFFF).contains(&id)
}

/// Retry the probe at most every two seconds until it passes. Validation is three derefs,
/// so retrying is cheap; a genuine mismatch simply never passes and steering stays off.
pub fn maybe_validate() {
    use std::sync::atomic::{AtomicU64, Ordering as O};
    static LAST: AtomicU64 = AtomicU64::new(0);
    if validated() {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // 250 ms, not 2 s: this is gated on a button press or a mouse cast, and the player pressing
    // R2 ten frames after a load should cast. The title screen legitimately fails the probe, so
    // the failure path is kept free of file writes (see `validate_player`).
    if now.saturating_sub(LAST.load(O::Relaxed)) < 250 {
        return;
    }
    LAST.store(now, O::Relaxed);
    if crate::player().is_some() {
        validate_player();
    }
}

/// True while the probe has not run yet.
fn path(name: &str) -> PathBuf {
    crate::config::path(name)
}

/// Cached: the fingerprint is a 4 MiB read, and the map is written more than once.
pub(crate) fn exe_fingerprint() -> (u64, u64) {
    static CACHED: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    *CACHED.get_or_init(read_fingerprint)
}

fn read_fingerprint() -> (u64, u64) {
    // FNV-1a over the first 4 MiB plus the file size: enough to notice a patch, cheap
    // enough to run at every startup.
    use std::io::Read;
    let mut file = match std::fs::File::open(std::env::current_exe().unwrap_or_default()) {
        Ok(f) => f,
        Err(_) => return (0, 0),
    };
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let n = file.read(&mut buf).unwrap_or(0);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in &buf[..n] {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let size = std::fs::metadata(std::env::current_exe().unwrap_or_default())
        .map(|m| m.len())
        .unwrap_or(0);
    (hash, size)
}

/// Write (or refresh) the map. `extra` carries the probe results, if known yet.
fn write_map(extra: &[String]) {
    let info = eldenring::rva::resolved_info();
    let rvas = eldenring::rva::get();
    let (hash, size) = exe_fingerprint();
    // SAFETY: GetModuleHandleA(NULL) returns the main module handle.
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;

    let table = info.as_ref().map(|i| i.table).unwrap_or("unknown");
    let version = info
        .as_ref()
        .map(|i| i.exe_version.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let fallback = info.as_ref().map(|i| i.used_fallback).unwrap_or(false);

    let mut out = String::new();
    out.push_str("# nr-casting resolved map\n");
    out.push_str("# Regenerated automatically when the executable changes.\n");
    out.push_str(&format!("exe_version = {version}\n"));
    out.push_str(&format!("rva_table = {table}\n"));
    out.push_str(&format!("version_recognised = {}\n", !fallback));
    out.push_str(&format!("module_base = {base:#x}\n"));
    out.push_str(&format!("exe_hash = {hash:#018x}\n"));
    out.push_str(&format!("exe_size = {size}\n"));
    out.push_str(&format!(
        "global_hinstance = {:#x} (va {:#x})\n",
        rvas.global_hinstance,
        base + rvas.global_hinstance as usize
    ));
    out.push_str(&format!(
        "game_man = {:#x} (va {:#x})\n",
        rvas.game_man,
        base + rvas.game_man as usize
    ));
    out.push_str(&format!(
        "field_area_ptr = {:#x} (va {:#x})\n",
        rvas.field_area_ptr,
        base + rvas.field_area_ptr as usize
    ));
    for line in extra {
        out.push_str(line);
        out.push('\n');
    }

    let target = path("nr_casting_offsets.txt");
    // Skip identical rewrites so the file's timestamp means "something changed".
    if std::fs::read_to_string(&target).map(|existing| existing == out).unwrap_or(false) {
        return;
    }
    if let Err(e) = std::fs::write(&target, out) {
        crate::log(&format!("rva: FAILED to write {}: {e}", target.display()));
    }
}

/// Phase one: note what the RVA layer resolved. Called before anything reads game state.
pub fn resolve_table() {
    let info = eldenring::rva::resolved_info();
    match &info {
        Some(info) if info.used_fallback => crate::log(&format!(
            "rva: exe {} is NOT in the table — falling back to {}; validating before use",
            info.exe_version, info.table
        )),
        Some(info) => crate::log(&format!(
            "rva: exe {} -> table {}",
            info.exe_version, info.table
        )),
        None => crate::log("rva: table resolved but no version info available"),
    }
    write_map(&["probe = deferred (no player yet)".to_string()]);
    crate::log("rva: map written to nr_casting_offsets.txt");
}

/// Phase two: check the structures we read, once the player exists.
pub fn validate_player() {
    let mut results: Vec<String> = Vec::new();
    let mut failure: Option<String> = None;

    match crate::weapon_ids() {
        Some((left, right)) if plausible_id(left) && plausible_id(right) => {
            results.push(format!("probe.weapon_ids = ok ({left}, {right})"));
        }
        Some((left, right)) => {
            failure = Some(format!("weapon ids out of range: {left}, {right}"));
            results.push(format!("probe.weapon_ids = FAILED {left}, {right}"));
        }
        None => {
            failure = Some("player instance not reachable".to_string());
            results.push("probe.weapon_ids = FAILED (no player)".to_string());
        }
    }

    match crate::player_stats() {
        Some((int, fai, arc)) if int <= 99 && fai <= 99 && arc <= 99 => {
            results.push(format!("probe.stats = ok ({int}, {fai}, {arc})"));
        }
        Some((int, fai, arc)) => {
            failure.get_or_insert_with(|| format!("stats implausible: {int}, {fai}, {arc}"));
            results.push(format!("probe.stats = FAILED ({int}, {fai}, {arc})"));
        }
        None => {
            failure.get_or_insert_with(|| "player stats unreadable".to_string());
            results.push("probe.stats = FAILED (unreadable)".to_string());
        }
    }

    match crate::magic_slots() {
        Some((count, selected)) if count >= 14 && (-1..=count as i32).contains(&selected) => {
            results.push(format!("probe.magic = ok ({count} slots, selected {selected})"));
        }
        Some((count, selected)) => {
            failure.get_or_insert_with(|| format!("magic slots implausible: {count}, {selected}"));
            results.push(format!("probe.magic = FAILED ({count} slots, selected {selected})"));
        }
        None => {
            failure.get_or_insert_with(|| "magic slots unreadable".to_string());
            results.push("probe.magic = FAILED (unreadable)".to_string());
        }
    }

    match failure {
        None => {
            STATE.store(VALIDATED, Ordering::Relaxed);
            results.push("probe = PASSED — steering enabled".to_string());
            crate::log("probe: structure validation passed");
            write_map(&results);
        }
        Some(reason) => {
            STATE.store(FAILED, Ordering::Relaxed);
            results.push(format!("probe = FAILED — {reason}"));
            results.push("probe = steering disabled; the game is untouched".to_string());
            // Retries are frequent (250 ms) while the title screen and loading screens fail this
            // check, so only speak and rewrite the map when the reason is new. Otherwise every
            // retry would be a log line and two file writes.
            static LAST_REASON: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
            let mut last = LAST_REASON.lock().unwrap_or_else(|e| e.into_inner());
            let changed = last.as_deref() != Some(reason.as_str());
            if changed {
                *last = Some(reason.clone());
                drop(last);
                crate::log(&format!("probe FAILED: {reason} — steering disabled"));
                write_map(&results);
            }
        }
    }
}

#[repr(C)]
struct MemoryBasicInformation {
    base_address: *mut u8,
    allocation_base: *mut u8,
    allocation_protect: u32,
    region_size: usize,
    state: u32,
    protect: u32,
    memory_type: u32,
}

extern "system" {
    fn VirtualQuery(address: *const u8, buffer: *mut MemoryBasicInformation, length: usize) -> usize;
}

/// Is `ptr..ptr+len` committed and readable right now?
///
/// A reload tears the world down and builds a new one, so a pointer sampled a moment ago can be
/// freed underneath us. Dereferencing it is how a mod turns "load a save" into a hard freeze, so
/// every game-structure read goes through this first and simply reports "no player" if the
/// memory is gone.
pub fn readable(ptr: *const u8, len: usize) -> bool {
    const MEM_COMMIT: u32 = 0x1000;
    const PAGE_NOACCESS: u32 = 0x01;
    const PAGE_GUARD: u32 = 0x100;

    if ptr.is_null() || len == 0 {
        return false;
    }
    let mut info = MemoryBasicInformation {
        base_address: std::ptr::null_mut(),
        allocation_base: std::ptr::null_mut(),
        allocation_protect: 0,
        region_size: 0,
        state: 0,
        protect: 0,
        memory_type: 0,
    };
    // SAFETY: info is a valid, correctly sized out-parameter.
    let got = unsafe {
        VirtualQuery(
            ptr,
            &mut info,
            std::mem::size_of::<MemoryBasicInformation>(),
        )
    };
    if got == 0 || info.state != MEM_COMMIT {
        return false;
    }
    if info.protect & PAGE_GUARD != 0 || info.protect & 0xFF == PAGE_NOACCESS {
        return false;
    }
    // A single region must cover the whole read.
    let end = ptr as usize + len;
    end <= info.base_address as usize + info.region_size
}
