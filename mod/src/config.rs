//! Where the mod's own files live, and the ini parser.
//!
//! The DLL normally sits in a shared mods folder together with other mods, so everything the
//! mod *writes* goes into an `nr_casting/` subfolder beside it instead of littering that
//! folder. Nothing is ever written into the game directory.
//!
//! Files written (all inside `nr_casting/`): `nr_casting.ini` (only when absent),
//! `nr_casting_slots.txt`, `nr_casting_catalogue.txt`, `nr_casting_build.txt`,
//! `nr_casting_offsets.txt`, `nr_casting_probe.log`. Files read: the same names, falling back
//! to the DLL's own folder so a hand-placed ini or table override keeps working.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

extern "system" {
    fn GetModuleFileNameW(module: *mut u8, buffer: *mut u16, size: u32) -> u32;
}

/// Files migrated from the game directory once. The name tables are deliberately absent:
/// they are compiled in, and copying a stale copy next to the DLL would pin it forever.
const OWNED_FILES: [&str; 4] = [
    "nr_casting_slots.txt",
    "nr_casting_offsets.txt",
    "nr_casting_probe.log",
    "nr_casting_probe.old.log",
];

/// When the panel's Present hook is installed.
///
/// A hook cannot be installed lazily — hudhook has no teardown — so it runs every frame while
/// loaded. That is wasted work in normal play and, with another overlay in the process, it
/// disturbs that overlay's drawing (a class port's HUD disappears). `Auto` therefore skips
/// installing when another overlay DLL is already loaded, and `On` forces it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayMode {
    Auto,
    On,
    Off,
}

#[derive(Clone, Debug)]
pub struct Config {
    /// Virtual-key code for the panel toggle.
    pub hotkey: i32,
    /// How the hotkey is spelled in the ini, for log messages.
    pub hotkey_name: String,
    /// XInput button bit that starts a bind when held with a spell input.
    pub modifier: u16,
    pub modifier_name: String,
    /// When to install the ImGui overlay.
    pub overlay: OverlayMode,
    /// Mirror slot 1 onto the game's HUD while the panel is closed.
    pub hud_mirror: bool,
    /// Log every cast, not just captures and errors.
    pub debug_log: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: 0x77, // F8
            hotkey_name: "F8".to_string(),
            modifier: 0x0040, // L3
            modifier_name: "L3".to_string(),
            overlay: OverlayMode::Auto,
            hud_mirror: false,
            debug_log: false,
        }
    }
}

static CONFIG: OnceLock<Config> = OnceLock::new();
static DIR: OnceLock<PathBuf> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Directory holding the mod's files: the DLL's own folder.
pub fn dir() -> &'static Path {
    DIR.get_or_init(|| {
        // Wide characters on purpose: the mod can live under a Steam library with a non-ASCII
        // path (C:\\Spiele\\..., C:\\Juegos\\...), and the ANSI variant mangles those.
        let mut buffer = [0u16; 1024];
        let module = crate::module_handle() as *mut u8;
        // SAFETY: buffer is valid and sized; the call writes at most `size` u16 units.
        let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), buffer.len() as u32) };
        if len > 0 {
            use std::os::windows::ffi::OsStringExt;
            let os = std::ffi::OsString::from_wide(&buffer[..len as usize]);
            let path = PathBuf::from(os);
            if let Some(parent) = path.parent() {
                return parent.to_path_buf();
            }
        }
        // Fall back to the executable's directory.
        let mut fallback = std::env::current_exe().unwrap_or_default();
        fallback.pop();
        fallback
    })
}

/// Folder the mod writes into: `nr_casting/` inside the DLL's folder, created on first use.
///
/// If that location cannot be written — the DLL was unpacked into `Program Files`, or into a
/// mods directory a manager keeps read-only — fall back to `%APPDATA%\nr_casting` rather than
/// failing silently. Where the files ended up is logged at startup, and the log is the first
/// thing a bug report needs.
pub fn data_dir() -> &'static Path {
    DATA_DIR.get_or_init(|| {
        let beside = dir().join("nr_casting");
        if usable(&beside) {
            return beside;
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let fallback = PathBuf::from(appdata).join("nr_casting");
            if usable(&fallback) {
                return fallback;
            }
        }
        dir().to_path_buf()
    })
}

/// Can we create this directory and write into it? Nothing here logs: this runs inside a
/// `OnceLock` initialiser that the logger itself depends on.
fn usable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Where a file is written.
pub fn path(name: &str) -> PathBuf {
    data_dir().join(name)
}

/// Where a file is read from: the data folder first, then the DLL's own folder, so an ini or
/// table override placed beside the DLL is still honoured.
pub fn read_path(name: &str) -> PathBuf {
    let in_data = path(name);
    if in_data.exists() {
        in_data
    } else {
        dir().join(name)
    }
}

fn game_dir() -> PathBuf {
    let mut p = std::env::current_exe().unwrap_or_default();
    p.pop();
    p
}

/// Copy pre-existing files from the game directory once, so moving to DLL-relative paths
/// does not silently lose somebody's assignments.
pub fn migrate_from_game_dir() {
    let game = game_dir();
    // Older layouts to absorb: files beside the game executable, and files that used to sit
    // directly beside the DLL before the data folder existed. Both are moved, not copied, so
    // the old location ends up clean.
    for name in OWNED_FILES
        .iter()
        .chain(["nr_casting_catalogue.txt"].iter())
    {
        let target = path(name);
        for source in [dir().join(name), game.join(name)] {
            if !source.exists() {
                continue;
            }
            if target.exists() || std::fs::copy(&source, &target).is_ok() {
                let _ = std::fs::remove_file(&source);
                crate::log(&format!("config: moved {name} into {}", data_dir().display()));
            }
        }
    }
}

fn hotkey_code(name: &str) -> Option<i32> {
    let name = name.trim().to_ascii_uppercase();
    if let Some(rest) = name.strip_prefix('F') {
        if let Ok(n) = rest.parse::<i32>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + (n - 1)); // VK_F1 = 0x70
            }
        }
    }
    Some(match name.as_str() {
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "HOME" => 0x24,
        "END" => 0x23,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "NUMLOCK" => 0x90,
        "SCROLLLOCK" => 0x91,
        "TAB" => 0x09,
        "BACKSPACE" => 0x08,
        _ => return None,
    })
}

fn modifier_bits(name: &str) -> Option<(u16, &'static str)> {
    Some(match name.trim().to_ascii_uppercase().as_str() {
        "L3" | "LS" | "LEFT_THUMB" => (0x0040, "L3"),
        "R3" | "RS" | "RIGHT_THUMB" => (0x0080, "R3"),
        "LB" | "L1_BUTTON" => (0x0100, "LB"),
        "RB" | "R1_BUTTON" => (0x0200, "RB"),
        "DPAD_UP" | "UP" => (0x0001, "D-pad Up"),
        "DPAD_DOWN" | "DOWN" => (0x0002, "D-pad Down"),
        _ => return None,
    })
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes" | "enabled"
    )
}

/// Parse the ini, writing a documented default file if it is missing. Unknown keys are
/// ignored; malformed values fall back to the default and are logged.
pub fn load() -> &'static Config {
    CONFIG.get_or_init(|| {
        // The ini ships with the package beside the DLL; a copy in the data folder is only
        // used if that one is missing, so editing the shipped file always takes effect.
        let beside_dll = dir().join("nr_casting.ini");
        let file = if beside_dll.exists() {
            beside_dll
        } else {
            let in_data = path("nr_casting.ini");
            if in_data.exists() {
                in_data
            } else {
                beside_dll
            }
        };
        if !file.exists() {
            let defaults = "\
; nr-casting — two spells per catalyst
[ui]
; Install the in-game panel. Turn this off when another overlay owns the frame presentation
; (frame-generation mods, class-port HUDs): casting still works, only the panel goes away.
overlay = on
; Key that opens the assignment panel (F1-F24, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, TAB)
hotkey = F8

[input]
; Hold this and press R1/R2/L1/L2 to bind the spell currently selected in game.
; L3, R3, LB, RB, DPAD_UP, DPAD_DOWN
bind_modifier = L3

[display]
; Mirror slot 1 onto the game's HUD while the panel is closed.
; The D-pad wheel is locked while this is on, so turn it off to bind spells.
hud_mirror = off

[logging]
; Log every cast, not just captures and errors.
debug_log = off
";
            if let Err(e) = std::fs::write(&file, defaults) {
                crate::log(&format!("config: could not write {}: {e}", file.display()));
            }
        }

        let mut config = Config::default();
        if let Ok(text) = std::fs::read_to_string(&file) {
            for line in text.lines() {
                let line = line.split([';', '#']).next().unwrap_or("").trim();
                if line.is_empty() || line.starts_with('[') {
                    continue;
                }
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let key = key.trim().to_ascii_lowercase();
                let value = value.trim();
                match key.as_str() {
                    "hotkey" => match hotkey_code(value) {
                        Some(code) => {
                            config.hotkey = code;
                            config.hotkey_name = value.to_ascii_uppercase();
                        }
                        None => crate::log(&format!(
                            "config: hotkey '{value}' is not recognised, keeping {}",
                            config.hotkey_name
                        )),
                    },
                    "bind_modifier" => match modifier_bits(value) {
                        Some((bits, name)) => {
                            config.modifier = bits;
                            config.modifier_name = name.to_string();
                        }
                        None => crate::log(&format!(
                            "config: bind_modifier '{value}' is not recognised, keeping {}",
                            config.modifier_name
                        )),
                    },
                    "overlay" => {
                        config.overlay = match value.trim().to_ascii_lowercase().as_str() {
                            "off" | "0" | "false" | "no" | "never" => OverlayMode::Off,
                            "on" | "1" | "true" | "yes" | "force" | "always" => OverlayMode::On,
                            _ => OverlayMode::Auto,
                        }
                    }
                    "hud_mirror" => config.hud_mirror = truthy(value),
                    "debug_log" => config.debug_log = truthy(value),
                    _ => {}
                }
            }
        }
        config
    })
}
