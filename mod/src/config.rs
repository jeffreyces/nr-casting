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

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// Panel font scale: `None` = follow the screen (`window::monitor_scale`), `Some(n)` = that
    /// multiplier (15 px base font). Persisted when changed in the panel.
    pub ui_scale: Option<f32>,
    /// How the scale is spelled in the ini, for log messages.
    pub ui_scale_name: String,
    /// The panel window's size, as saved when the player last resized it. `None` = auto: size the
    /// window from the screen.
    pub panel_size: Option<(u32, u32)>,
    /// The panel window's top-left corner, as saved when the player last moved it. `None` = the
    /// default spot (80, 80), clamped onto the screen. Saved with the size, one ini write.
    pub panel_pos: Option<(i32, i32)>,
    /// Install the input detours at all. `off` is a stutter experiment, not a setting: with no
    /// detours the mod cannot cast, but it can still say what the game's frame timing looks like
    /// with the DLL loaded and doing nothing - the one comparison a loaded-but-inert mod cannot
    /// otherwise make.
    pub hooks: bool,
    /// Install the mouse/keyboard raw-input hooks (mouse casting, Shift hiding).
    pub raw_hook: bool,
    /// Install the cursor detours (panel cursor behaviour). These come out again whenever the
    /// panel is closed; this decides whether they exist at all.
    pub pointer_hook: bool,
    /// Install the pass-through `Present` timer, the frame clock that is once per frame.
    ///
    /// Off by default: creating a throwaway device and swap chain to read the vtable address
    /// crashes inside `dxgi.dll` under Proton/vkd3d (the address is fine on Windows, where the
    /// clock is wanted). Until that is understood, the pad poll and the pump remain the clocks.
    pub present_clock: bool,
    /// Log every cast, not just captures and errors.
    pub debug_log: bool,
    /// Log a ten-second line of pad-poll timing: frame gaps as the game sees them, and the cost of
    /// our own detour inside them. This is what a stutter report needs and the game does not log.
    pub perf_log: bool,
    /// Panel planning mode: list spells the character cannot cast yet. Persisted when toggled.
    pub show_all: bool,
    /// Whether assignments are limited to the character's memorised spells.
    ///
    /// `Free` is today's behaviour and the default: every spell the catalogue offers can be
    /// assigned. `Attuned` is the balance gate — the mod then uses the player's memory slots
    /// instead of replacing them, because capacity no longer scales with catalysts owned. Live:
    /// the panel writes it and the watcher publishes the result.
    pub balance: nr_casting_core::Balance,
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
    /// Install the XInput hooks at all. `off` = no pad hook, no watch thread: the game's XInput
    /// calls never enter our code, so pad casting is gone and mouse casting is untouched.
    ///
    /// This exists to make one comparison possible that nothing else can make. A stutter that only
    /// appears with a controller connected cannot be bisected by the guided run, because every
    /// phase keeps the hook in the path on purpose (see `bisect`). Unplugging the pad removes the
    /// game's calls *and* our hook in the same act, so it cannot tell "our polling work costs
    /// frames" from "being in the call path costs frames". Flipping this to `off` removes only the
    /// hook, which is the missing half of that measurement.
    pub pad_hook: bool,
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
            ui_scale: None,
            ui_scale_name: "auto".to_string(),
            panel_size: None,
            panel_pos: None,
            hooks: true,
            raw_hook: true,
            pointer_hook: true,
            present_clock: false,
            debug_log: false,
            // Matches the shipped ini: quiet (one line per five minutes, or when a frame is lost),
            // and the only way a stutter is recorded at the time it happens.
            perf_log: true,
            show_all: false,
            balance: nr_casting_core::Balance::Free,
            hud_mirror: false,
            y_bumper: YBumper::Auto,
            y_bumper_name: "auto".to_string(),
            recluse_ids: vec![8131],
            bind_modifier: 0x0040, // L3
            bind_modifier_kb: 0x5B, // Super (VK_LWIN)
            bind_modifier_kb_name: "Super".to_string(),
            chord_stage: 2,
            bind_modifier_name: "L3".to_string(),
            pad_hook: true,
        }
    }
}

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
    save_ui_key("show_all", if show { "on" } else { "off" });
}

/// Remember the panel window's size and corner in one ini write.
///
/// Both change during the same drag — moving and resizing are the same gesture as far as the player
/// is concerned — so they are written together, once, after the drag settles.
pub fn save_panel_geometry(size: Option<(u32, u32)>, pos: Option<(i32, i32)>) {
    let size = match size {
        Some((width, height)) => format!("{width}x{height}"),
        None => "auto".to_string(),
    };
    let pos = match pos {
        Some((x, y)) => format!("{x},{y}"),
        None => "auto".to_string(),
    };
    save_ui_keys("ui", &[("panel_size", &size), ("panel_pos", &pos)]);
}

/// Remember a UI scale and the window size that goes with it in one ini write.
///
/// Picking a scale resizes the window, so the two are saved together; two separate writers would
/// read and rewrite the whole file twice for one user action.
pub fn save_panel_layout(scale: Option<f32>, size: Option<(u32, u32)>) {
    let scale = match scale {
        Some(scale) => format!("{:.0}%", scale * 100.0),
        None => "auto".to_string(),
    };
    let size = match size {
        Some((width, height)) => format!("{width}x{height}"),
        None => "auto".to_string(),
    };
    save_ui_keys("ui", &[("ui_scale", &scale), ("panel_size", &size)]);
}

/// Rewrite one key in the ini, keeping every other line (comments included) as it is.
fn save_ui_key(key: &str, value: &str) {
    save_ui_keys("ui", &[(key, value)]);
}

/// Rewrite several keys in one pass, keeping every other line (comments included) as they are.
///
/// A key the file does not have yet is added at the end of the `[ui]` section rather than at the end
/// of the file: an install shipped before the key existed gains it in the section a reader would
/// look in, not under `[logging]`.
/// The hash of the last file content we wrote ourselves.
///
/// The panel saves geometry, scale and `show_all` into this ini, and the watcher watches that file:
/// every panel resize would otherwise reload the config mid-frame. Comparing *content* rather than
/// time is what makes this safe in both directions - a player's edit a second after a resize is
/// still their edit and still reloads, and our own write is recognised however late the event lands.
static SELF_HASH: AtomicU64 = AtomicU64::new(0);

/// FNV-1a: enough to tell "the bytes we wrote" from "the bytes the player wrote".
fn content_hash(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Remember what we just wrote, so the watcher can recognise it.
fn note_self_write(bytes: &[u8]) {
    SELF_HASH.store(content_hash(bytes), Ordering::Relaxed);
}

/// Whether the file on disk is still exactly what we last wrote.
fn is_self_write() -> bool {
    let ours = SELF_HASH.load(Ordering::Relaxed);
    if ours == 0 {
        return false;
    }
    match std::fs::read(ini_file()) {
        Ok(bytes) => content_hash(&bytes) == ours,
        // Cannot read it: do not claim it is ours.
        Err(_) => false,
    }
}

fn save_ui_keys(section: &str, pairs: &[(&str, &str)]) {
    let file = ini_file();
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let header = format!("[{section}]");
    let mut out = String::with_capacity(text.len() + 32 * pairs.len());
    let mut written = vec![false; pairs.len()];
    // Where a missing key goes: just before the section after ours, or at the end of the file.
    let mut insert_at: Option<usize> = None;
    let mut in_section = false;
    for line in text.lines() {
        let bare = line
            .split([';', '#'])
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        if bare.starts_with('[') {
            if in_section && insert_at.is_none() {
                insert_at = Some(out.len());
            }
            in_section = bare == header;
        }
        let mut replaced = false;
        for (index, (key, value)) in pairs.iter().enumerate() {
            if !written[index] && bare.starts_with(key) && bare.contains('=') {
                out.push_str(&format!("{key} = {value}\n"));
                written[index] = true;
                replaced = true;
                break;
            }
        }
        if !replaced {
            out.push_str(line);
            out.push('\n');
        }
    }
    if in_section && insert_at.is_none() {
        insert_at = Some(out.len());
    }
    let mut missing = String::new();
    for (index, (key, value)) in pairs.iter().enumerate() {
        if !written[index] {
            missing.push_str(&format!("{key} = {value}\n"));
        }
    }
    if !missing.is_empty() {
        match insert_at {
            Some(at) => out.insert_str(at, &missing),
            None => out.push_str(&missing),
        }
    }
    match std::fs::write(&file, &out) {
        Ok(()) => note_self_write(out.as_bytes()),
        Err(e) => {
            let keys: Vec<&str> = pairs.iter().map(|(key, _)| *key).collect();
            crate::log(&format!("config: could not save {}: {e}", keys.join(", ")));
        }
    }
}

static CONFIG: std::sync::atomic::AtomicPtr<Config> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// The ini is read once, and again whenever the file changes (see `watch`).
///
/// A pointer rather than a `OnceLock` because the file is event-driven now: a watcher notices the
/// write, re-parses, and publishes. Readers keep whatever pointer they loaded, so a reload can never
/// pull the ground out from under a call in flight; the superseded config is leaked on purpose
/// (a few hundred bytes, only on a hand edit, never in a loop).
pub fn load() -> &'static Config {
    let published = CONFIG.load(Ordering::Acquire);
    if !published.is_null() {
        // SAFETY: only ever set to a leaked `Config` that lives for the process.
        return unsafe { &*published };
    }
    let fresh: &'static Config = Box::leak(Box::new(read()));
    match CONFIG.compare_exchange(
        std::ptr::null_mut(),
        fresh as *const Config as *mut Config,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => fresh,
        // Lost a race with another first caller: theirs is published, use it.
        Err(existing) => unsafe { &*existing },
    }
}

/// Re-read the ini and publish the result. Returns the keys that only take effect next launch.
pub fn reload() -> Vec<&'static str> {
    let fresh: &'static Config = Box::leak(Box::new(read()));
    let previous = CONFIG.swap(
        fresh as *const Config as *mut Config,
        Ordering::AcqRel,
    );
    if previous.is_null() {
        return Vec::new();
    }
    // SAFETY: every published pointer is a leaked `Config`.
    let old = unsafe { &*previous };
    let mut late = Vec::new();
    if old.overlay != fresh.overlay {
        late.push("overlay");
    }
    if old.hooks != fresh.hooks || old.raw_hook != fresh.raw_hook
        || old.pointer_hook != fresh.pointer_hook || old.pad_hook != fresh.pad_hook
    {
        late.push("the hook switches");
    }
    if old.present_clock != fresh.present_clock {
        late.push("present_clock");
    }
    if old.hotkey != fresh.hotkey {
        late.push("hotkey");
    }
    late
}

/// Watch the ini's folder and reload when the file changes.
///
/// An event, not a timer: `FindFirstChangeNotificationW` puts the thread to sleep until somebody
/// writes, and the common case for the rest of the session costs one blocked thread. The debounce is
/// for editors that write in two steps (truncate, then content) - reloading on the first write would
/// read an empty file.
pub fn watch() {
    const FILE_NOTIFY_CHANGE_LAST_WRITE: u32 = 0x0000_0010;
    const WAIT_OBJECT_0: u32 = 0;
    extern "system" {
        fn FindFirstChangeNotificationW(path: *const u16, subtree: i32, filter: u32) -> isize;
        fn FindNextChangeNotification(handle: isize) -> i32;
        fn WaitForSingleObject(handle: isize, millis: u32) -> u32;
    }
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        // The folder, not the file: a file handle follows the inode, and editors replace the file.
        let folder: Vec<u16> = dir()
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe { FindFirstChangeNotificationW(folder.as_ptr(), 0, FILE_NOTIFY_CHANGE_LAST_WRITE) };
        if handle == isize::MAX || handle == 0 {
            crate::log("config: cannot watch the folder - the ini is read at launch only");
            return;
        }
        // The first load has to happen before a reload can be compared against it.
        let _ = load();
        loop {
            let hit = unsafe { WaitForSingleObject(handle, 60_000) };
            if hit != WAIT_OBJECT_0 {
                continue;
            }
            // Re-arm. If this fails the handle stays signaled, so every later wait would return
            // immediately - a background thread spinning at a full core for the rest of the session,
            // which nobody would attribute to the ini. Stop instead, and say so once.
            if unsafe { FindNextChangeNotification(handle) } == 0 {
                crate::log(
                    "config: the folder watcher stopped (the notification could not be re-armed); \
                     the ini is read at launch only from here on",
                );
                return;
            }
            // Editors write in two steps; give the second one time to land.
            std::thread::sleep(std::time::Duration::from_millis(500));
            if !ini_file().exists() {
                continue;
            }
            if is_self_write() {
                // The panel wrote it (geometry, scale, show_all). Nothing for the player to see.
                continue;
            }
            let late = reload();
            crate::apply_runtime_config();
            if late.is_empty() {
                crate::log("config: reloaded from the ini");
            } else {
                crate::log(&format!(
                    "config: reloaded from the ini ({} take effect next launch)",
                    late.join(", ")
                ));
            }
        }
    });
}

fn read() -> Config {
    {
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
; Ignored while balance = attuned: planning stays planning, but selecting an unattuned spell is
; refused rather than assigned.
show_all = off
; Balanced mode. free = assign any spell the panel offers. attuned = only the spells you have
; memorised in the game's own spell menu can be assigned, so the mod uses your memory slots
; instead of replacing them. Saved when you toggle it in the panel.
balance = free
; Panel size of text and widgets: auto follows the panel's own size, so the UI grows with the
; monitor it opens on and with the window when you drag its corner. Set 125 / 150% / 1.5 / 200%
; to pin it. Saved when you change it in the panel.
ui_scale = auto
; Panel window size, saved when you resize the panel. auto sizes it from the screen; WxH pins it.
panel_size = auto
panel_pos = auto
; Force the game's own spell wheel to slot 1 so its HUD shows what R1 will cast. Off: the wheel
; stays yours (on, it rewrites the selection ~30x a second and fights your D-pad scrolling).
hud_mirror = off

[input]
; Hold this and press R1/R2/L1/L2 to bind the spell currently selected in the game to that slot.
; L3, R3, LB, RB, DPAD_UP, DPAD_DOWN or none. The same gesture on mouse and keyboard is
; Super (Windows key) + click: Super + LMB / Shift / Ctrl / Alt.
bind_modifier = L3
; Install the XInput hook at all. on = pad casting works as documented. off = the game's controller
; calls never enter this DLL: pad casting is gone, mouse casting is untouched. This is a diagnostic
; for stutter reports that only appear with a controller connected and disappear when it is
; unplugged, because unplugging removes both the game's calls and our hook at once.
pad_hook = on

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
; Perf log: one line every 10 s with the pad-poll interval (the game's frame time, as our hook
; sees it) and the longest time our own code spent inside it. Use it to report stutter.
perf_log = off
";
            match std::fs::write(&file, defaults) {
                Ok(()) => note_self_write(defaults.as_bytes()),
                Err(e) => {
                    crate::log(&format!("config: could not write {}: {e}", file.display()));
                }
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
                    "hooks" => config.hooks = truthy(value),
                    "raw_hook" => config.raw_hook = truthy(value),
                    "pointer_hook" => config.pointer_hook = truthy(value),
                    "present_clock" => config.present_clock = truthy(value),
                    "debug_log" => config.debug_log = truthy(value),
                    "perf_log" => config.perf_log = truthy(value),
                    "show_all" => config.show_all = truthy(value),
                    "balance" => match nr_casting_core::parse_balance(value) {
                        Some(mode) => config.balance = mode,
                        None => crate::log(&format!(
                            "config: balance '{value}' is not recognised (free, attuned); \
                             staying free"
                        )),
                    },
                    "ui_scale" => match nr_casting_core::ui_scale(value) {
                        Some((scale, spelling)) => {
                            config.ui_scale = scale;
                            config.ui_scale_name = spelling;
                        }
                        None => crate::log(&format!(
                            "config: ui_scale '{value}' is not recognised (auto, 150, 150%, 1.5 — \
                             100% to 300%); keeping {}",
                            config.ui_scale_name
                        )),
                    },
                    "panel_size" => match nr_casting_core::panel_size(value) {
                        Some((0, 0)) => config.panel_size = None,
                        Some(size) => config.panel_size = Some(size),
                        None => crate::log(&format!(
                            "config: panel_size '{value}' is not recognised (auto or WxH, at least \
                             900x400); sizing the panel from the screen"
                        )),
                    },
                    "panel_pos" => match nr_casting_core::panel_pos(value) {
                        Some((0, 0)) => config.panel_pos = None,
                        Some(pos) => config.panel_pos = Some(pos),
                        None => crate::log(&format!(
                            "config: panel_pos '{value}' is not recognised (auto or X,Y); \
                             opening the panel at its default spot"
                        )),
                    },
                    "hud_mirror" => config.hud_mirror = truthy(value),
                    "pad_hook" => config.pad_hook = truthy(value),
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
    }
}
