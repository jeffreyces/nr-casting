//! One file that says everything a bug report needs, written on demand from the panel.
//!
//! The alternative is a conversation: "which GPU?", "which Proton?", "what else is loaded?",
//! "can you enable debug and do it again?". Every one of those answers is already in this process,
//! so the panel writes them into a single text file next to the DLL and the player attaches it.
//!
//! Deliberately *not* sent anywhere. There is no endpoint, no key, and no automatic upload: the
//! report contains the character's name and the player's Windows user name in paths, so it leaves
//! the machine only when they choose to attach it — and every path is redacted first.

use std::fmt::Write as _;
use std::path::PathBuf;

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::SystemInformation::{
    GetNativeSystemInfo, GlobalMemoryStatusEx, MEMORYSTATUSEX, OSVERSIONINFOW, SYSTEM_INFO,
};

#[link(name = "ntdll")]
extern "system" {
    /// The real OS version. `GetVersionEx` lies on anything newer than 8.1.
    fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> i32;
}

/// The issue page without a body, for a note that has to name it in one line.
pub const ISSUE_PAGE: &str = "https://github.com/jeffreyces/nr-casting/issues/new";

/// The GitHub issue page, with a body that already contains the numbers we would ask for.
///
/// Returned rather than opened so the caller decides when to shell out.
pub fn issue_url() -> String {
    let body = format!(
        "**Version:** {}\n**Game exe:** {}\n**Perf:** {}\n**Modules:** {}\n\n\
         Attach the file written by the panel's `diagnostics` button \
         (`nr_casting_diag_*.txt`) — it has the log, the system, the GPU and the frame series.\n\n\
         **What happened:**\n\n\n**Steps:**\n\n",
        env!("CARGO_PKG_VERSION"),
        crate::probe::exe_version(),
        crate::perf::summary(),
        crate::foreign_modules()
            .map(|m| m.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_else(|| "unknown".to_string()),
    );
    // The title too: an empty one is a field the reporter has to think about, and the version is
    // the one part we always want in it.
    let title = format!("nr_casting {} — ", env!("CARGO_PKG_VERSION"));
    format!(
        "{ISSUE_PAGE}?title={}&body={}",
        percent_encode(&title),
        percent_encode(&body)
    )
}

fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push_str("%20"),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Write the bundle beside the DLL and return its path and size in bytes.
pub fn write_bundle(reason: &str) -> Result<(PathBuf, usize), String> {
    // Local time, spelled the way a person reads it: the file name is what a player quotes in a
    // bug report, and an epoch stamp is not a date.
    let name = format!("nr_casting_diag_{}.txt", local_stamp());
    let path = crate::config::path(&name);
    let text = bundle(reason);
    std::fs::write(&path, text.as_bytes()).map_err(|e| format!("could not write {name}: {e}"))?;
    let size = text.len();
    prune_old(5);
    crate::log(&format!(
        "diag: wrote {} ({size} bytes, {reason})",
        path.display()
    ));
    Ok((path, size))
}

/// Keep the folder tidy: one report is useful, a pile of them is clutter. Newest five survive.
fn prune_old(keep: usize) {
    let folder = crate::config::data_dir();
    let Ok(entries) = std::fs::read_dir(&folder) else { return };
    let mut reports: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().starts_with("nr_casting_diag_"))
                .unwrap_or(false)
        })
        .collect();
    if reports.len() <= keep {
        return;
    }
    reports.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
    });
    for path in reports.iter().take(reports.len() - keep) {
        let _ = std::fs::remove_file(path);
    }
}

fn bundle(reason: &str) -> String {
    let redact = redactor();
    let mut out = String::with_capacity(96 * 1024);
    section(&mut out, "report", &report_header(reason));
    section(&mut out, "system", &system_info());
    section(&mut out, "gpu", &gpu_info());
    section(&mut out, "display", &display_info());
    section(&mut out, "environment", &environment());
    section(&mut out, "modules", &modules());
    section(&mut out, "state", &state());
    section(&mut out, "performance", &performance());
    section(&mut out, "config", &read_text("nr_casting.ini"));
    section(&mut out, "slots", &crate::store::describe_store());
    section(&mut out, "offsets", &head_of("nr_casting_offsets.txt", 20));
    section(&mut out, "catalogue", &catalogue());
    section(&mut out, "log tail", &log_tail());
    // Paths last: everything above may contain them.
    redact.apply(&mut out);
    out
}

fn section(out: &mut String, name: &str, body: &str) {
    let _ = writeln!(out, "==== {name} ====");
    if body.trim().is_empty() {
        let _ = writeln!(out, "(empty)");
    } else {
        let _ = writeln!(out, "{}", body.trim_end());
    }
    out.push('\n');
}

fn report_header(reason: &str) -> String {
    format!(
        "mod: nr_casting {}\nexe: {}\nreason: {reason}\nuptime: {} ms\nlog: {}",
        env!("CARGO_PKG_VERSION"),
        crate::probe::exe_version(),
        crate::logging::uptime_ms(),
        crate::logging::log_path().display(),
    )
}

fn system_info() -> String {
    let mut out = String::new();
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    if unsafe { RtlGetVersion(&mut version) } == 0 {
        let _ = writeln!(
            out,
            "os: {}.{} build {}",
            version.dwMajorVersion, version.dwMinorVersion, version.dwBuildNumber
        );
    } else {
        let _ = writeln!(out, "os: unknown");
    }
    let _ = writeln!(out, "cpu: {}", std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default());
    let _ = writeln!(
        out,
        "arch: {}",
        std::env::var("PROCESSOR_ARCHITECTURE").unwrap_or_default()
    );
    let mut info = SYSTEM_INFO::default();
    unsafe { GetNativeSystemInfo(&mut info) };
    let _ = writeln!(out, "cores: {}", info.dwNumberOfProcessors);
    let mut memory = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut memory) }.is_ok() {
        let _ = writeln!(
            out,
            "ram: {} MiB total, {} MiB available",
            memory.ullTotalPhys / (1024 * 1024),
            memory.ullAvailPhys / (1024 * 1024)
        );
    }
    if let Some((working, private)) = crate::memory_mib() {
        let _ = writeln!(out, "process: {working} MiB working set, {private} MiB private");
    }
    out
}

fn gpu_info() -> String {
    let mut out = String::new();
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(factory) => factory,
        Err(e) => return format!("dxgi factory failed: {e}"),
    };
    for index in 0..4 {
        let adapter = match unsafe { factory.EnumAdapters(index) } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        let Ok(desc) = (unsafe { adapter.GetDesc() }) else { continue };
        let name = String::from_utf16_lossy(
            &desc.Description[..desc.Description.iter().position(|c| *c == 0).unwrap_or(128)],
        );
        let _ = writeln!(
            out,
            "adapter {index}: {name} (vendor {:#06x} device {:#06x}), {} MiB dedicated, {} MiB shared",
            desc.VendorId,
            desc.DeviceId,
            desc.DedicatedVideoMemory / (1024 * 1024),
            desc.SharedSystemMemory / (1024 * 1024)
        );
    }
    out
}

fn display_info() -> String {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
    format!("screen: {width}x{height}\n")
}

fn environment() -> String {
    let mut out = String::new();
    let mut keys: Vec<&str> = vec![
        "WINE",
        "WINEARCH",
        "WINEPREFIX",
        "PROTON_VERSION",
        "STEAM_COMPAT_CLIENT_INSTALL_PATH",
        "STEAM_COMPAT_DATA_PATH",
        "STEAM_COMPAT_TOOL_PATHS",
        "SteamAppId",
        "SteamGameId",
        "PROTON_ENABLE_WAYLAND",
        "DXVK_HUD",
        "MANGOHUD",
        "VK_ICD_FILENAMES",
        "GAMEID",
        "PROTONPATH",
    ];
    keys.sort_unstable();
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            let _ = writeln!(out, "{key}={value}");
        }
    }
    out
}

fn modules() -> String {
    match crate::foreign_modules() {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|(name, base)| format!("{name}@{base:#x}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(_) => "no foreign DLLs loaded".to_string(),
        None => "enumeration failed".to_string(),
    }
}

/// The live picture a bug report needs, and that the debug log would otherwise have to be turned on
/// to provide: what is equipped, what the mod thinks of it, and where the input path stands.
///
/// The counters underneath always run — `debug_log` only decides whether they are *printed* — so
/// this section is complete with the ini untouched.
fn state() -> String {
    let mut out = String::new();
    let cfg = crate::config::load();
    let _ = writeln!(
        out,
        "config: overlay={} debug_log={} perf_log={} ui_scale={} bind_modifier={} chord_stage={}",
        cfg.overlay, cfg.debug_log, cfg.perf_log, cfg.ui_scale_name, cfg.bind_modifier_name, cfg.chord_stage
    );
    match crate::weapon_ids() {
        Some((left, right)) => {
            let _ = writeln!(out, "weapons in hand: left={left} right={right}");
        }
        None => {
            let _ = writeln!(out, "weapons: unavailable (character not loaded)");
        }
    }
    match crate::player_effective_stats() {
        Some((int, fai, arc)) => {
            let _ = writeln!(out, "stats: INT {int} FAI {fai} ARC {arc}");
        }
        None => {
            let _ = writeln!(out, "stats: unavailable");
        }
    }
    let _ = writeln!(
        out,
        "spell selected in the game: {}",
        crate::PLAYER_SPELL.load(std::sync::atomic::Ordering::Relaxed)
    );
    let _ = writeln!(
        out,
        "input: {} polls, {} full passes, {} with no controller, {} unplugged",
        crate::stats::pad_polls(),
        crate::stats::passes(),
        crate::stats::pad_unplugged_count(),
        0
    );
    let _ = writeln!(out, "raw: {}", crate::stats::raw_report());
    let _ = writeln!(
        out,
        "chords: Y+R1/Y+L1 {} | panel {} | player cursor {}",
        if crate::input::y_bumper() { "ours" } else { "Recluse's" },
        if crate::UI_OPEN.load(std::sync::atomic::Ordering::Relaxed) { "open" } else { "closed" },
        if crate::input::game_shows_cursor() { "shown" } else { "hidden" }
    );
    out
}

fn performance() -> String {
    let mut out = format!("{}\n", crate::perf::summary());
    let series = crate::perf::series();
    if !series.is_empty() {
        out.push_str("worst gap per second (ms, oldest first):\n");
        out.push_str(&series);
        out.push('\n');
    }
    out
}

fn read_text(name: &str) -> String {
    std::fs::read_to_string(crate::config::read_path(name)).unwrap_or_else(|e| format!("({e})"))
}

fn head_of(name: &str, lines: usize) -> String {
    read_text(name)
        .lines()
        .take(lines)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The generated spell catalogue, in full.
///
/// It was six lines - the three-line header and the first two spells of 316 - so the section said
/// "catalogue" and answered nothing, while the log above it said "316 spells read". The file is
/// machine-generated from the game's own params and holds no user data; 316 lines is 20 KB.
fn catalogue() -> String {
    read_text("nr_casting_catalogue.txt")
        .lines()
        .take(4000)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The last part of the log: the tail is where the failure is.
fn log_tail() -> String {
    let path = crate::logging::log_path();
    // Bytes, then a lossy decode. `read_to_string` returns an error if *any* byte in the file is
    // not UTF-8, and one such sequence made the entire tail disappear from a report that then said
    // `(empty)` while its own header named the log - which is exactly the file the reporter needed
    // us to see.
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => return format!("(could not read {}: {e})", path.display()),
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(2000);
    lines[start..].join("\n")
}

/// Replaces the player's own identifiers with placeholders before the file leaves the machine.
///
/// Under Proton the interesting paths are not `C:\Users\...` at all: the host home appears both as
/// `/var/home/<user>` and as `Z:\var\home\<user>` inside the prefix, while `USERPROFILE` is the
/// Wine account. All of them have to be covered, case-insensitively — Windows paths are, and a
/// missed capital letter is a leaked user name.
struct Redactor {
    needles: Vec<(String, &'static str)>,
}

impl Redactor {
    fn apply(&self, text: &mut String) {
        let needles: Vec<(String, String)> = self
            .needles
            .iter()
            .map(|(needle, replacement)| (needle.clone(), replacement.to_string()))
            .collect();
        *text = nr_casting_core::redact(text, &needles);
    }
}

fn redactor() -> Redactor {
    let mut needles: Vec<(String, &'static str)> = Vec::new();
    let mut add = |value: Option<String>, replacement: &'static str| {
        if let Some(value) = value {
            let value = value.trim_end_matches(['/', '\\']).to_string();
            if value.len() > 3 {
                // Both spellings of a path: the host form and the Wine drive-letter form.
                needles.push((value.clone(), replacement));
                needles.push((value.replace('\\', "/"), replacement));
                needles.push((format!("Z:{}", value.replace('/', "\\")), replacement));
            }
        }
    };
    add(std::env::var("USERPROFILE").ok(), "%USERPROFILE%");
    add(std::env::var("HOME").ok(), "%HOME%");
    if let Ok(user) = std::env::var("USERNAME").or_else(|_| std::env::var("USER")) {
        if user.len() > 2 {
            // Alone, a user name is too short to replace safely everywhere; in a path it is not.
            needles.push((format!("\\{user}\\"), "\\%USER%\\"));
            needles.push((format!("/{user}/"), "/%USER%/"));
            needles.push((format!("\\{user}$"), "\\%USER%$"));
        }
    }
    // The character's name is the player's own, and it is written in prose as well as in the slots
    // file name: "slots: character Warlock (id 1)" and "series restarted for Warlock". The generic
    // path and slug rules do not catch either, so the loaded name is a needle of its own.
    if let Some((name, _)) = crate::game::character_key() {
        let name = name.trim();
        // Two characters or fewer is not a name worth matching (and would redact far too much).
        if name.len() > 2 {
            for spelling in [name.to_string(), name.to_lowercase(), name.to_uppercase()] {
                needles.push((spelling, "<character>"));
            }
            let slug = nr_casting_core::character_slug(name, 0).0;
            if slug.len() > 2 {
                needles.push((slug, "<character>"));
            }
        }
    }
    Redactor { needles }
}

/// `2026-10-31 15-05-12`, in the machine's own time zone.
fn local_stamp() -> String {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let now = unsafe { GetLocalTime() };
    format!(
        "{:04}-{:02}-{:02}_{:02}{:02}{:02}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
    )
}
