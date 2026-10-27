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

#[derive(Clone, Debug)]
pub struct Config {
    /// Virtual-key code for the panel toggle.
    pub hotkey: i32,
    /// How the hotkey is spelled in the ini, for log messages.
    pub hotkey_name: String,
    /// Open the assignment panel window. Off = casting only.
    pub overlay: bool,
    /// Log every cast, not just captures and errors.
    pub debug_log: bool,
    /// Panel planning mode: list spells the character cannot cast yet. Persisted when toggled.
    pub show_all: bool,
    /// Force the game's own spell-wheel selection to slot 1 so its HUD shows what R1 will cast.
    ///
    /// Off by default: it rewrites `selected_slot` ~30 times a second, which fights the player's
    /// own wheel selection (the HUD visibly cycles, and a cast that reads the selection after a
    /// rewrite uses the wrong spell). Only useful if you never touch the wheel yourself.
    pub hud_mirror: bool,
    /// Who owns the Y+bumper chords (Y+R1 / Y+L1, the fourth slot).
    pub y_bumper: YBumper,
    pub y_bumper_name: String,
    /// Character ids that mean "this character is a Recluse". Matched against the player's
    /// character event id and character id. 8131 is the Recluse port's character model.
    pub recluse_ids: Vec<i32>,
    /// XInput button bit held with a spell input to bind the wheel's selected spell to that
    /// slot. `0` disables the gesture.
    pub bind_modifier: u16,
    /// How the bind modifier is spelled in the ini, for log messages.
    pub bind_modifier_name: String,
    /// Virtual-key code held with a click to bind, on mouse and keyboard.
    ///
    /// Separate from `bind_modifier` because the two devices have nothing in common here, and
    /// because Super is a poor citizen on Linux: KDE and GNOME both claim it, and a compositor grab
    /// eats `Super + click` before the game sees it. `menu` (the context-menu key) and `F9`-`F12`
    /// are offered as grab-free alternatives. `none` disables the gesture.
    pub bind_modifier_kb: i32,
    /// How the keyboard bind modifier is spelled, for log messages.
    pub bind_modifier_kb_name: String,
    /// How many reports a chord spends letting go of Y before the shoulder press is sent.
    ///
    /// This is the latch insurance: ER keeps its own "Y is held" state for a frame or two, and a
    /// press inside that window becomes a weapon swap. Two is reliable; one is about 8-13 ms faster
    /// and is worth trying once `chord_trigger_floor` handles the swap input (it does now).
    pub chord_stage: u8,
}

/// Who gets Y+R1 / Y+L1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum YBumper {
    /// Ours unless the Recluse mod is loaded and this character is a Recluse.
    Auto,
    /// Always ours, even on a Recluse character.
    On,
    /// Never ours; leave the chords to the game and to Recluse.
    Off,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            hotkey: 0x77, // F8
            hotkey_name: "F8".to_string(),
            overlay: true,
            debug_log: false,
            show_all: false,
            hud_mirror: false,
            y_bumper: YBumper::Auto,
            y_bumper_name: "auto".to_string(),
            recluse_ids: vec![8131],
            bind_modifier: 0x0040, // L3
            bind_modifier_kb: 0x5B, // Super (VK_LWIN)
            bind_modifier_kb_name: "Super".to_string(),
            chord_stage: 2,
            bind_modifier_name: "L3".to_string(),
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


/// XInput button bit that starts a bind when held with a spell input.
///
/// Deliberately a small set: this is held *while* pressing R1/R2/L1/L2, so it has to be a button
/// a thumb can hold down comfortably — L3 (click the left stick) is the default the v0.1 builds
/// used, and `none` turns the gesture off for anyone who would rather not risk the press.
fn modifier_bits(name: &str) -> Option<(u16, &'static str)> {
    Some(match name.trim().to_ascii_uppercase().as_str() {
        "NONE" | "OFF" | "DISABLED" => (0, "none"),
        "L3" | "LS" | "LEFT_THUMB" => (0x0040, "L3"),
        "R3" | "RS" | "RIGHT_THUMB" => (0x0080, "R3"),
        "LB" | "L1_BUTTON" => (0x0100, "LB"),
        "RB" | "R1_BUTTON" => (0x0200, "RB"),
        "DPAD_UP" | "UP" => (0x0001, "D-pad Up"),
        "DPAD_DOWN" | "DOWN" => (0x0002, "D-pad Down"),
        _ => return None,
    })
}

/// Virtual-key code held with a click to bind. Deliberately excludes Shift, Ctrl and Alt: those
/// already select slots 2-4 on the click, so using one as the modifier would collide with itself.
fn keyboard_modifier(name: &str) -> Option<(i32, &'static str)> {
    Some(match name.trim().to_ascii_uppercase().as_str() {
        "NONE" | "OFF" | "DISABLED" => (0, "none"),
        "SUPER" | "WIN" | "WINDOWS" | "META" => (0x5B, "Super"), // VK_LWIN; VK_RWIN accepted too
        "MENU" | "APPS" | "CONTEXT" => (0x5D, "Menu"),           // VK_APPS
        "F9" => (0x78, "F9"),
        "F10" => (0x79, "F10"),
        "F11" => (0x7A, "F11"),
        "F12" => (0x7B, "F12"),
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
/// The ini the loader will use: beside the DLL when it exists, otherwise the data folder.
fn ini_file() -> PathBuf {
    let beside_dll = dir().join("nr_casting.ini");
    if beside_dll.exists() {
        return beside_dll;
    }
    let in_data = path("nr_casting.ini");
    if in_data.exists() {
        in_data
    } else {
        beside_dll
    }
}

/// Remember the panel's planning-mode toggle across sessions.
///
/// The value is only read at startup, so this writes the ini rather than mutating the loaded
/// `Config`; everything else about the file (comments, other keys) is preserved.
pub fn save_show_all(show: bool) {
    let file = ini_file();
    let value = if show { "on" } else { "off" };
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut out = String::with_capacity(text.len() + 16);
    let mut written = false;
    for line in text.lines() {
        let bare = line.split([';', '#']).next().unwrap_or("").trim();
        if !written && bare.to_ascii_lowercase().starts_with("show_all") && bare.contains('=') {
            out.push_str(&format!("show_all = {value}\n"));
            written = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !written {
        out.push_str(&format!("show_all = {value}\n"));
    }
    if let Err(e) = std::fs::write(&file, out) {
        crate::log(&format!("config: could not save show_all: {e}"));
    }
}

pub fn load() -> &'static Config {
    CONFIG.get_or_init(|| {
        // The ini ships with the package beside the DLL; a copy in the data folder is only
        // used if that one is missing, so editing the shipped file always takes effect.
        let file = ini_file();
        if !file.exists() {
            let defaults = "\
; nr-casting — four spells per catalyst (bumper, trigger, Y + trigger, Y + bumper)
[ui]
; Open the assignment panel on the hotkey. Set this to off for casting only — no window at all.
overlay = on
; Key that opens the assignment panel (F1-F24, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, TAB)
hotkey = F8
; List spells the character cannot cast yet (planning mode). Saved when you toggle it in the panel.
show_all = off
; Force the game's own spell wheel to slot 1 so its HUD shows what R1 will cast. Off: the wheel
; stays yours (on, it rewrites the selection ~30x a second and fights your D-pad scrolling).
hud_mirror = off

[input]
; Hold this and press R1/R2/L1/L2 to bind the spell currently selected in the game to that slot.
; L3, R3, LB, RB, DPAD_UP, DPAD_DOWN or none. The same gesture on mouse and keyboard is
; Super (Windows key) + click: Super + LMB / Shift / Ctrl / Alt.
bind_modifier = L3

[recluse]
; Who gets Y+R1 / Y+L1, the fourth spell. Recluse uses exactly those two chords.
; auto = ours unless the Recluse mod is loaded and this character is a Recluse,
;        in which case the chords pass through to the class port (its 4 / 5 keys)
; on   = always ours (even on a Recluse character)
; off  = never ours
y_bumper = auto
; Character ids that count as a Recluse: the port's character model is c8131.
recluse_chr_ids = 8131

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
                    "bind_modifier_kb" => match keyboard_modifier(value) {
                        Some((vk, spelling)) => {
                            config.bind_modifier_kb = vk;
                            config.bind_modifier_kb_name = spelling.to_string();
                        }
                        None => crate::log(&format!(
                            "config: bind_modifier_kb '{value}' is not recognised (Super, Menu, \
                             F9-F12, none); keeping {}",
                            config.bind_modifier_kb_name
                        )),
                    },
                    "bind_modifier" => match modifier_bits(value) {
                        Some((bits, spelling)) => {
                            config.bind_modifier = bits;
                            config.bind_modifier_name = spelling.to_string();
                        }
                        None => crate::log(&format!(
                            "config: bind_modifier '{value}' is not recognised (L3, R3, LB, RB, \
                             none); keeping {}",
                            config.bind_modifier_name
                        )),
                    },
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
                    // "auto" is accepted for older installs: the panel no longer hooks the
                    // game's swap chain, so there is nothing to skip automatically.
                    "overlay" => {
                        // v0.1.0 shipped `auto | on | off`. There is no swap-chain hook to be
                        // careful about any more, so the old default means what `on` means now —
                        // say so rather than going dark for everyone who kept their ini.
                        if value.trim().eq_ignore_ascii_case("auto") {
                            crate::log(
                                "config: overlay = auto is now the same as on — the panel is its own window",
                            );
                            config.overlay = true;
                        } else {
                            config.overlay = truthy(value);
                        }
                    }
                    "debug_log" => config.debug_log = truthy(value),
                    "show_all" => config.show_all = truthy(value),
                    "hud_mirror" => config.hud_mirror = truthy(value),
                    "chord_stage" => {
                        config.chord_stage = value.trim().parse::<u8>().unwrap_or(2).min(4)
                    }
                    "y_bumper" => match value.to_ascii_lowercase().as_str() {
                        "on" | "true" | "yes" => {
                            config.y_bumper = YBumper::On;
                            config.y_bumper_name = "on".to_string();
                        }
                        "off" | "false" | "no" => {
                            config.y_bumper = YBumper::Off;
                            config.y_bumper_name = "off".to_string();
                        }
                        "auto" => {
                            config.y_bumper = YBumper::Auto;
                            config.y_bumper_name = "auto".to_string();
                        }
                        _ => crate::log(&format!(
                            "config: y_bumper = {value:?} is not auto/on/off; keeping {}",
                            config.y_bumper_name
                        )),
                    },
                    "recluse_chr_ids" => {
                        let ids: Vec<i32> = value
                            .split(',')
                            .filter_map(|part| part.trim().parse::<i32>().ok())
                            .collect();
                        if ids.is_empty() {
                            crate::log(&format!(
                                "config: recluse_chr_ids = {value:?} has no numbers; keeping {:?}",
                                config.recluse_ids
                            ));
                        } else {
                            config.recluse_ids = ids;
                        }
                    }
                    _ => {}
                }
            }
        }
        config
    })
}
