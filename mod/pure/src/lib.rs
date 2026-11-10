//! Pure logic for `nr_casting`, kept free of Win32 and of the game so it can be tested on the
//! host with `cargo test -p nr_casting_core`.
//!
//! Two things live here because they are the parts worth testing:
//!
//! * the slot file — parse and format, including files written by older versions;
//! * input routing — which input means which slot, and which inputs win when several are held.
//!
//! Everything else (the hook, the panel, the parameter reads) needs a running game and stays in
//! the DLL.

pub mod fmg;

/// The panel is laid out for this size; every other size is this one at a scale.
pub const DESIGN_WIDTH: f32 = 900.0;
/// Height the panel is designed at.
pub const DESIGN_HEIGHT: f32 = 620.0;
/// The smallest the text may get: below 100% the panel is unreadable anyway, and the window's own
/// minimum is the design size at this scale.
pub const MIN_UI_SCALE: f32 = 1.0;
/// The largest the text may get. 300% of a 15 px font is a 45 px font, which is already a 4K TV
/// read from a sofa.
pub const MAX_UI_SCALE: f32 = 3.0;
/// Scales are offered in 5% steps, and always the step *below* the fit.
const SCALE_STEPS: f32 = 20.0;

/// The scale that makes the design fit a window of this client size.
///
/// The **smaller** of the two ratios: averaging them guarantees the content is wider or taller
/// than the window on the axis that binds, which is what put scrollbars on both sides. Eight
/// pixels are held back from the height for the frame, so the last row does not tip into a
/// scrollbar either.
///
/// Floored to a scale step and never rounded up: half a percent over the fit is a scrollbar.
/// `natural` is the height the content actually needs at 100%, latched by the caller; a nonsense
/// value falls back to the design height rather than scaling by zero or by infinity.
pub fn fit_scale(width: f32, height: f32, natural: f32) -> f32 {
    let width = width.max(1.0);
    let height = height.max(1.0);
    let natural = if natural > 1.0 { natural } else { DESIGN_HEIGHT };
    let raw = (width / DESIGN_WIDTH).min((height - 8.0).max(1.0) / natural);
    let stepped = (raw * SCALE_STEPS).floor() / SCALE_STEPS;
    stepped.clamp(MIN_UI_SCALE, MAX_UI_SCALE)
}

/// Spells per catalyst: bumper, trigger, the Y chord on the trigger, and the Y chord on the
/// bumper. The fourth only exists when the Y+bumper chords are ours to take (no Recluse mod, or
/// not on a Recluse character); otherwise the game and Recluse keep them.
pub const SLOTS: usize = 4;

/// One weapon's four spell ids, `-1` for unbound.
pub type Category = [i32; SLOTS];

/// The weapon id with its upgrade level stripped: `33090010` (a +10 staff) -> `33090000`.
///
/// ELDEN RING's upgrade level is the low two digits of the equipped id — `overlay` already
/// relies on that to name a weapon. Assignments are filed on the *base* id so upgrading a
/// catalyst at a smith does not look like a different weapon and lose its spells: the game
/// treats each level as its own param row, which is why the store had to stop keying on the
/// raw id.
///
/// Weapon param ids are eight digits; anything outside that range (a negative, a zero, a short id
/// from a hand-edited file) is returned unchanged rather than folded into a neighbour.
pub fn base_weapon_id(id: i32) -> i32 {
    if id < 1_000_000 {
        id
    } else {
        id - id % 100
    }
}

/// Fold a parsed slot file onto base weapon ids.
///
/// Files written by older versions hold the upgraded id (`33090010 = ...`). Folding two of them
/// into one base id merges slot by slot, keeping the first spell each line assigns, so an
/// upgrade cannot silently split one catalyst's assignments into two rows.
pub fn canonicalize(entries: &[(i32, Category)]) -> Vec<(i32, Category)> {
    let mut out: Vec<(i32, Category)> = Vec::with_capacity(entries.len());
    for (id, spells) in entries {
        let id = base_weapon_id(*id);
        match out.iter_mut().find(|(existing, _)| *existing == id) {
            Some((_, target)) => {
                for (slot, spell) in spells.iter().enumerate() {
                    if target[slot] == -1 {
                        target[slot] = *spell;
                    }
                }
            }
            None => out.push((id, *spells)),
        }
    }
    out.sort_by_key(|(id, _)| *id);
    out
}

/// XInput button bits used by the routing rules.
pub const BTN_LEFT_SHOULDER: u16 = 0x0100;
pub const BTN_RIGHT_SHOULDER: u16 = 0x0200;
pub const BTN_Y: u16 = 0x8000;

/// What the pad reported this poll.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub buttons: u16,
    pub right_pull: bool,
    pub left_pull: bool,
    pub right_bump: bool,
    pub left_bump: bool,
}

impl Held {
    /// Any of the four spell inputs is down.
    pub fn any(&self) -> bool {
        self.right_pull || self.left_pull || self.right_bump || self.left_bump
    }
}

/// What one poll should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Nothing spell-shaped is held: release any borrowed entry, maybe mirror the HUD.
    Idle,
    /// Y plus a trigger: the third slot.
    Chord {
        right_hand: bool,
        slot: usize,
        label: &'static str,
    },
    /// A plain spell input.
    Cast {
        right_hand: bool,
        slot: usize,
        label: &'static str,
    },
}

/// Decide what a poll means.
///
/// Order is the contract and matches the DLL: the Y chords are checked before the plain inputs
/// because under Y the trigger means the third slot and the bumper the fourth.
///
/// This says *which slot* an input addresses, not whether that slot carries a spell. A chord
/// with no spell behind it is handed back to the game untouched (see the DLL), which is what
/// keeps vanilla two-handing working on a hand that has nothing bound.
///
/// `y_bumper` is false when the Y+bumper chords belong to someone else — Recluse uses Y+R1 and
/// Y+L1, so on a Recluse character (or with Recluse loaded) those reports must reach the game
/// untouched. They come back as `Idle`, which the DLL treats as "do not rewrite this report".
pub fn route(held: &Held, y_bumper: bool) -> Route {
    if !held.any() {
        return Route::Idle;
    }
    if held.buttons & BTN_Y != 0 {
        if held.right_pull {
            return Route::Chord {
                right_hand: true,
                slot: 2,
                label: "Y+R2",
            };
        }
        if held.left_pull {
            return Route::Chord {
                right_hand: false,
                slot: 2,
                label: "Y+L2",
            };
        }
        if y_bumper && held.right_bump {
            return Route::Chord {
                right_hand: true,
                slot: 3,
                label: "Y+R1",
            };
        }
        if y_bumper && held.left_bump {
            return Route::Chord {
                right_hand: false,
                slot: 3,
                label: "Y+L1",
            };
        }
        if held.right_bump || held.left_bump {
            // Someone else's chord (Recluse): leave the report exactly as the pad sent it.
            return Route::Idle;
        }
    }
    let (right_hand, slot, label) = if held.right_pull {
        (true, 1, "R2")
    } else if held.right_bump {
        (true, 0, "R1")
    } else if held.left_pull {
        (false, 1, "L2")
    } else {
        (false, 0, "L1")
    };
    Route::Cast {
        right_hand,
        slot,
        label,
    }
}

/// The bind gesture: with the bind modifier held, the same four inputs that cast also assign.
///
/// Returns `(right_hand, slot, label)` for the input that is down, or `None` when no spell
/// input is held. The Y chords keep their meaning here, so `L3 + Y + R2` binds the third slot
/// and `L3 + R2` the second — the player holds the same key they would cast with.
///
/// The caller owns the modifier: it knows whether the pad is reporting L3 (or whatever the ini
/// says) this report, and it must swallow the press so binding never also casts. Kept here
/// rather than in the DLL because the mapping has to match `route` exactly, and that is the
/// kind of thing worth a test.
pub fn bind_route(held: &Held) -> Option<(bool, usize, &'static str)> {
    match route(held, true) {
        Route::Idle => None,
        Route::Chord {
            right_hand,
            slot,
            label,
        }
        | Route::Cast {
            right_hand,
            slot,
            label,
        } => Some((right_hand, slot, label)),
    }
}

/// The mouse mapping: click plus modifiers to (right hand, slot, name).
///
/// Same eight slots as the pad: Shift selects the trigger slot, Ctrl the Y+trigger slot, Alt the
/// Y+bumper slot. Shift is the modifier the game itself uses for the strong attack, so the mod
/// hides Shift from the game while no controller is connected — otherwise a Shift click would
/// arrive as a heavy attack instead of a cast (see `kbm::hide_shift`).
pub fn mouse_route(
    left: bool,
    right: bool,
    shift: bool,
    ctrl: bool,
    alt: bool,
    y_bumper: bool,
) -> Option<(bool, usize, &'static str)> {
    let hand = match (left, right) {
        (true, false) => true,
        (false, true) => false,
        _ => return None,
    };
    // Alt carries the fourth slot, which is Recluse's on a Recluse character. Returning `None`
    // leaves the click alone: the class port's own binding (its 4 and 5 keys) sees it instead.
    if alt && !shift && !ctrl && !y_bumper {
        return None;
    }
    Some(match (shift, ctrl, alt) {
        (true, false, false) => (hand, 1, if hand { "shift + LMB" } else { "shift + RMB" }),
        (false, true, false) => (hand, 2, if hand { "ctrl + LMB" } else { "ctrl + RMB" }),
        (false, false, true) => (hand, 3, if hand { "alt + LMB" } else { "alt + RMB" }),
        (false, false, false) => (hand, 0, if hand { "LMB" } else { "RMB" }),
        // Several modifiers at once is not a slot the player asked for; treat it as the plain
        // click so a stray key never swallows a cast.
        _ => (hand, 0, if hand { "LMB" } else { "RMB" }),
    })
}

/// Parse a slot file. Unparseable lines are skipped, never fatal: the file is user-editable.
///
/// Layout: `weapon_param_id = bumper, trigger, y_chord`. Files from older versions carry two
/// ids; the third defaults to unbound. Duplicate ids are returned in file order so the caller
/// keeps its own last-wins behaviour.
pub fn parse_slots(text: &str) -> Vec<(i32, Category)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // Settings that used to live here; the ini owns them now.
        if line.starts_with("hud_mirror") || line.starts_with("debug_log") {
            continue;
        }
        let Some((id, spells)) = line.split_once('=') else {
            continue;
        };
        let Ok(id) = id.trim().parse::<i32>() else {
            continue;
        };
        let ids: Vec<i32> = spells
            .split(',')
            .filter_map(|part| part.trim().parse::<i32>().ok())
            .collect();
        if ids.len() >= 2 {
            out.push((
                id,
                [
                    ids[0],
                    ids[1],
                    ids.get(2).copied().unwrap_or(-1),
                    ids.get(3).copied().unwrap_or(-1),
                ],
            ));
        }
    }
    out
}

/// Marker line written into files that may hold per-copy rows.
///
/// A row on a non-base id means two different things depending on the file's age: before overrides
/// existed it was an upgrade level (`33090010`, folded onto `33090000` on load), and now it is a
/// layout that belongs to that one copy. The marker is what tells the two apart without guessing,
/// because guessing wrong either loses a customised layout or resurrects one.
pub const LAYOUT_MARKER: &str = "# layout 2: a row on the full param id overrides that copy only";

/// Whether a slot file was written by a version that understands per-copy rows.
pub fn per_copy_rows(text: &str) -> bool {
    text.lines()
        .any(|line| line.trim_start().starts_with("# layout 2"))
}

// ---- Character files -------------------------------------------------------------------
//
// Assignments are per character, and the file name is derived from the character's name and save
// slot. That derivation is the thing that decides which assignments a character sees, so it lives
// here where it can be tested rather than inside the DLL.

/// Filename-safe, collision-resistant parts for a character: `(base, slug)`.
///
/// The hash covers the name *and* the save slot, so two characters that normalise to the same
/// base — `Test 2`, `test-2`, `TEST_2` — still get separate files.
pub fn character_slug(name: &str, slot: u32) -> (String, String) {
    let mut base = String::new();
    let mut last_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            base.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && !base.is_empty() {
            base.push('-');
            last_dash = true;
        }
    }
    while base.ends_with('-') {
        base.pop();
    }
    base.truncate(24);
    if base.is_empty() {
        base.push_str("character");
    }
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes().iter().chain(&slot.to_le_bytes()) {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    (base.clone(), format!("{base}-{hash:04x}"))
}

// ---- Raw input records ---------------------------------------------------------------
//
// ELDEN RING reads the mouse through raw input (`GetRawInputData`), so the keyboard/mouse path
// has to understand the wire layout. These helpers keep that layout testable: they take bytes,
// never a pointer, so the offsets can be proven on the host instead of only inside the game.

/// `RAWINPUTHEADER` on x64: dwType, dwSize, hDevice, wParam.
pub const RAW_HEADER: usize = 24;
/// `RAWMOUSE.ulButtons` (dwType 0): after usFlags + padding.
pub const RAW_MOUSE_BUTTONS_AT: usize = RAW_HEADER + 4;
/// Shortest buffer a mouse record can be read from, and the same for a keyboard one.
pub const RAW_MOUSE_LEN: usize = RAW_MOUSE_BUTTONS_AT + 4;
pub const RAW_KEY_FLAGS_AT: usize = RAW_HEADER + 2;
pub const RAW_KEY_VKEY_AT: usize = RAW_HEADER + 6;
pub const RAW_KEY_LEN: usize = RAW_KEY_VKEY_AT + 2;
pub const RAW_TYPE_MOUSE: u32 = 0;
pub const RAW_TYPE_KEYBOARD: u32 = 1;
pub const RI_MOUSE_LEFT_BUTTON_DOWN: u32 = 0x0001;
pub const RI_MOUSE_LEFT_BUTTON_UP: u32 = 0x0002;
pub const RI_MOUSE_RIGHT_BUTTON_DOWN: u32 = 0x0004;
pub const RI_MOUSE_RIGHT_BUTTON_UP: u32 = 0x0008;
pub const RI_KEY_BREAK: u16 = 0x0001;

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// `dwType` of a raw record, or `None` when the buffer cannot hold a header.
pub fn raw_kind(record: &[u8]) -> Option<u32> {
    le_u32(record, 0)
}

/// `dwSize` of a raw record — what the buffer walk and every field read rely on.
pub fn raw_size(record: &[u8]) -> Option<usize> {
    le_u32(record, 4).map(|n| n as usize)
}

/// Button edges of a mouse record.
///
/// Returns `None` for anything else, and in particular for a record whose `dwSize` or buffer is
/// shorter than the struct it claims to be: this runs on the game's input thread, so a truncated
/// record must never be read past its end.
pub fn raw_mouse_buttons(record: &[u8]) -> Option<u32> {
    if raw_kind(record)? != RAW_TYPE_MOUSE {
        return None;
    }
    if raw_size(record)? < RAW_MOUSE_LEN || record.len() < RAW_MOUSE_LEN {
        return None;
    }
    le_u32(record, RAW_MOUSE_BUTTONS_AT)
}

/// Overwrite the button field of a mouse record, keeping every other flag.
///
/// Returns whether the record was a mouse record long enough to hold the field.
pub fn raw_set_mouse_buttons(record: &mut [u8], buttons: u32) -> bool {
    if raw_kind(record) != Some(RAW_TYPE_MOUSE) {
        return false;
    }
    if raw_size(record).unwrap_or(0) < RAW_MOUSE_LEN || record.len() < RAW_MOUSE_LEN {
        return false;
    }
    let Some(slot) = record.get_mut(RAW_MOUSE_BUTTONS_AT..RAW_MOUSE_BUTTONS_AT + 4) else {
        return false;
    };
    slot.copy_from_slice(&buttons.to_le_bytes());
    true
}

/// `(usFlags, usVKey)` of a keyboard record, with the same guards.
pub fn raw_keyboard(record: &[u8]) -> Option<(u16, u16)> {
    if raw_kind(record)? != RAW_TYPE_KEYBOARD {
        return None;
    }
    if raw_size(record)? < RAW_KEY_LEN || record.len() < RAW_KEY_LEN {
        return None;
    }
    let flags = le_u16(record, RAW_KEY_FLAGS_AT)?;
    let vkey = le_u16(record, RAW_KEY_VKEY_AT)?;
    Some((flags, vkey))
}

/// The three virtual keys that mean Shift (generic, left, right).
pub fn is_shift_vkey(vkey: u16) -> bool {
    matches!(vkey, 0x10 | 0xA0 | 0xA1)
}

/// Rewrite a Shift key-down record as a key-up, so the game never sees Shift held.
///
/// Returns whether the record was a Shift press and was changed; everything else, including a
/// truncated record, is left untouched.
pub fn raw_hide_shift(record: &mut [u8]) -> bool {
    let Some((flags, vkey)) = raw_keyboard(record) else {
        return false;
    };
    if !is_shift_vkey(vkey) || flags & RI_KEY_BREAK != 0 {
        return false;
    }
    let Some(slot) = record.get_mut(RAW_KEY_FLAGS_AT..RAW_KEY_FLAGS_AT + 2) else {
        return false;
    };
    slot.copy_from_slice(&(flags | RI_KEY_BREAK).to_le_bytes());
    true
}

/// Is a saved deadline still in the future?
///
/// `until == 0` means "nothing has been armed yet", and must never be read as "the deadline is
/// the epoch": that makes every timestamp look pending. A zeroed static once did exactly that to
/// the pad's Y key and to the mouse-borrow hold — `now - 0` on a 32-bit clock is a *positive*
/// number, but its `i32` reading is negative whenever the clock's high bit is set, which is half
/// of every 49.7-day wrap.
pub fn armed_until(now: u32, until: u32) -> bool {
    until != 0 && (now.wrapping_sub(until) as i32) < 0
}

/// The panel's UI scale, from the ini: `auto`, a percentage (`150`, `150%`) or a multiplier
/// (`1.5`).
///
/// Returns the multiplier — `None` for auto, i.e. follow the panel window's own size — and how to
/// spell it in the log. Both spellings are accepted because both are natural to write, and a bare
/// number above 4 cannot be a multiplier, so `100` and `1.0` mean the same thing. Out-of-range
/// answers are rejected rather than clamped: the caller keeps what it had and says so, and a value
/// that silently became something else is how a config file starts lying.
pub fn ui_scale(text: &str) -> Option<(Option<f32>, String)> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("auto") {
        return Some((None, "auto".to_string()));
    }
    let percent = text.ends_with('%');
    let digits: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let value = digits.parse::<f32>().ok()?;
    let scale = if percent || (value > 4.0 && !text.contains('.')) {
        value / 100.0
    } else {
        value
    };
    // The panel is laid out at 1x; below that nothing is legible, and above 3x the three columns
    // no longer fit a 4K screen.
    if !(1.0..=3.0).contains(&scale) {
        return None;
    }
    Some((Some(scale), format!("{:.0}%", scale * 100.0)))
}

/// The panel window's saved size, from the ini: `auto`, or `<width>x<height>` in pixels.
///
/// `auto` (and no value at all) means "size the window from the screen". Anything smaller than the
/// size the layout needs, or larger than any screen, is rejected rather than clamped: the caller
/// then falls back to auto and the file keeps what the player wrote.
pub fn panel_size(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("auto") {
        return Some((0, 0));
    }
    let (width, height) = text
        .split_once(['x', 'X', '×'])
        .or_else(|| text.split_once(','))?;
    let width: u32 = width.trim().parse().ok()?;
    let height: u32 = height.trim().parse().ok()?;
    // The window's own minimum is 900 wide by 400 tall (its content-fitted height), and the panel
    // saves whatever it currently is - which is why the floor cannot be the 620 design height: a
    // panel fitted to its content is 607 tall, so a 620 floor rejected the size the panel itself
    // had just written, and every launch warned and discarded the player's saved height.
    if !(900..=8192).contains(&width) || !(400..=8192).contains(&height) {
        return None;
    }
    Some((width, height))
}

/// The panel window's saved top-left corner: `auto` (or `0,0`) means "use the default spot".
///
/// Negative coordinates are allowed — a panel parked on a monitor to the left of the primary has
/// a negative x, and refusing that would put it back on the primary every launch.
pub fn panel_pos(text: &str) -> Option<(i32, i32)> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("auto") {
        return Some((0, 0));
    }
    let (x, y) = text.split_once(',').or_else(|| text.split_once(['x', 'X']))?;
    let x: i32 = x.trim().parse().ok()?;
    let y: i32 = y.trim().parse().ok()?;
    // Beyond this a saved position is a typo or a monitor that no longer exists; the clamp to the
    // screen at open time handles the rest.
    if x.abs() > 32768 || y.abs() > 32768 {
        return None;
    }
    Some((x, y))
}


/// Case-insensitive `String::replace`.
///
/// Windows paths are case-insensitive, and a redaction that misses `C:\Users\<name>` because it
/// only replaced `c:\users\<name>` is a leaked user name.
fn replace_ignore_case(text: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    let lower_text = text.to_ascii_lowercase();
    let lower_needle = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower_text[at..].find(&lower_needle) {
        let start = at + found;
        out.push_str(&text[at..start]);
        out.push_str(replacement);
        at = start + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// Drop the character name out of a slots file name, keeping the scheme recognisable.
fn redact_character_slug(text: &str) -> String {
    const PREFIX: &str = "nr_casting_slots_";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(found) = rest.find(PREFIX) {
        let (before, after) = rest.split_at(found + PREFIX.len());
        out.push_str(before);
        match after.find(['-', '.', '\\', '/', ' ']) {
            Some(end) => {
                out.push_str("<character>");
                rest = &after[end..];
            }
            None => rest = after,
        }
    }
    out.push_str(rest);
    out
}

/// Labels of the graphics wrappers whose strings appear in a binary's bytes.
///
/// Six bytes and up, and distinctive: a three-byte marker like "enb" matches a random 8 MiB
/// binary about a third of the time, which is how an earlier version of the report section
/// labelled two NVIDIA DLLs as ENB and ReShade. Absence is not proof of anything.
pub fn wrapper_markers(bytes: &[u8]) -> Vec<&'static str> {
    // Every needle is six bytes or more: "dxvk" alone is four, and it matched inside a block of
    // base64 in a 54 MB NVIDIA DLSS DLL - the same class of false positive as "enb" in an earlier
    // version of this section. Absence is not proof; a hit has to be worth reading.
    const MARKERS: [(&str, &[u8]); 10] = [
        ("DXVK", b"dxvk-cache"),
        ("DXVK", b"dxvk_config"),
        ("DXVK", b"dxvk_hud"),
        ("ReShade", b"reshade"),
        ("Special K", b"special k"),
        ("SpecialK", b"specialk"),
        ("Vulkan", b"vkcreateinstance"),
        ("DLSS-G", b"nvngx_dlssg"),
        ("D3D12 Agility", b"d3d12core"),
        ("Streamline", b"sl.interposer"),
    ];
    let lower: Vec<u8> = bytes.iter().map(|byte| byte.to_ascii_lowercase()).collect();
    MARKERS
        .iter()
        .filter(|(_, needle)| lower.windows(needle.len()).any(|window| window == *needle))
        .map(|(label, _)| *label)
        .collect()
}

/// The build timestamp from a PE header, in Unix seconds.
///
/// Every PE has one at `e_lfanew + 8`; a linker told not to write one leaves zero, and anything
/// before 1980 is a sentinel rather than a build.
pub fn pe_build_timestamp(bytes: &[u8]) -> Option<u64> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let offset = u32::from_le_bytes(bytes.get(0x3c..0x40)?.try_into().ok()?) as usize;
    let stamp = u32::from_le_bytes(bytes.get(offset + 8..offset + 12)?.try_into().ok()?);
    (stamp >= 315_532_800).then_some(stamp as u64)
}

/// One phase of a bundle run, ready to print.
pub struct PhaseSummary<'a> {
    pub index: usize,
    pub phases: usize,
    pub label: &'a str,
    /// What the phase switched off, or `nothing`.
    pub off: &'a str,
    pub stats: GapStats,
    pub hitches: u32,
    pub seconds_with_hitches: usize,
    pub seconds: usize,
    /// Which second held the worst gap, one-based.
    pub worst_at: usize,
    /// Ticks the recorder saw during the phase. A phase recorded at 30 ticks a second is not
    /// comparable with one recorded at 60, so the count travels with the numbers.
    pub ticks: u32,
}

/// The two lines a phase gets: the summary, then how often it stuttered and where the worst was.
pub fn phase_lines(phase: &PhaseSummary<'_>) -> String {
    format!(
        "phase {}/{} {} (off: {}): {} [{} ticks, {:.0}/s]\n    {} hitches over 32ms in {} of {} seconds; worst {}ms at second {}",
        phase.index,
        phase.phases,
        phase.label,
        phase.off,
        gap_line(&phase.stats),
        phase.ticks,
        if phase.seconds == 0 { 0.0 } else { phase.ticks as f32 / phase.seconds as f32 },
        phase.hitches,
        phase.seconds_with_hitches,
        phase.seconds,
        phase.stats.max as f32 / 1000.0,
        phase.worst_at
    )
}

/// `YYYY-MM-DD` for a Unix timestamp, by the civil-from-days algorithm (no `chrono`, no locale).
pub fn civil_date(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// What one second did to a run of fixed-length phases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseTick {
    /// Still in the same phase.
    Second,
    /// That second was the phase's last; this is the new one.
    NextPhase,
    /// That second was the last of the last phase.
    Finished,
}

/// A run of equal-length phases, advanced one second at a time.
///
/// Split out because the arithmetic is the part that is easy to get wrong by one: six phases of
/// sixty seconds is six minutes, and the last second has to finish the run rather than open a
/// seventh phase that nothing closes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhaseClock {
    phase: usize,
    left: u32,
    phases: usize,
    seconds: u32,
}

impl PhaseClock {
    pub const fn new(phases: usize, seconds: u32) -> Self {
        Self {
            phase: 0,
            left: if seconds == 0 { 1 } else { seconds },
            phases: if phases == 0 { 1 } else { phases },
            seconds: if seconds == 0 { 1 } else { seconds },
        }
    }

    pub fn phase(&self) -> usize {
        self.phase
    }

    pub fn left(&self) -> u32 {
        self.left
    }

    pub fn phases(&self) -> usize {
        self.phases
    }

    /// Total seconds the run needs.
    pub fn total(&self) -> u32 {
        self.phases as u32 * self.seconds
    }

    pub fn restart(&mut self) {
        self.phase = 0;
        self.left = self.seconds;
    }

    pub fn tick(&mut self) -> PhaseTick {
        if self.left > 1 {
            self.left -= 1;
            return PhaseTick::Second;
        }
        if self.phase + 1 >= self.phases {
            return PhaseTick::Finished;
        }
        self.phase += 1;
        self.left = self.seconds;
        PhaseTick::NextPhase
    }
}

/// What a per-second series of worst frame gaps says, in microseconds.
///
/// A zero is a second the game did not poll the pad at all - a menu, a loading screen, the panel
/// holding the keyboard. It is *no data*, not a 0 ms gap: counting it inside the percentiles
/// flatters them, and counting it as sampled flatters the coverage. So it is counted, then dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GapStats {
    /// Seconds with at least one poll.
    pub sampled: usize,
    /// Seconds with none.
    pub quiet: usize,
    pub p50: u32,
    pub p90: u32,
    pub max: u32,
    /// Seconds whose worst gap was over 32 ms, i.e. at least one dropped frame at 60 Hz.
    pub over_32ms: usize,
}

pub fn gap_stats(values: &[u32]) -> GapStats {
    let quiet = values.iter().filter(|v| **v == 0).count();
    let mut sorted: Vec<u32> = values.iter().copied().filter(|v| *v > 0).collect();
    if sorted.is_empty() {
        return GapStats { quiet, ..GapStats::default() };
    }
    sorted.sort_unstable();
    let at = |q: f64| {
        let last = (sorted.len() - 1) as f64;
        sorted[(last * q) as usize]
    };
    GapStats {
        sampled: sorted.len(),
        quiet,
        p50: at(0.5),
        p90: at(0.9),
        max: sorted[sorted.len() - 1],
        over_32ms: sorted.iter().filter(|v| **v > 32_000).count(),
    }
}

/// The same summary as one line of text, for a log or a report.
pub fn gap_line(stats: &GapStats) -> String {
    if stats.sampled == 0 {
        return format!("{} s sampled, none with a pad poll", stats.quiet);
    }
    format!(
        "{} s sampled ({} with no pad poll): worst-gap per second p50 {}ms p90 {}ms max {}ms; \
         {} seconds over 32ms",
        stats.sampled + stats.quiet,
        stats.quiet,
        stats.p50 as f32 / 1000.0,
        stats.p90 as f32 / 1000.0,
        stats.max as f32 / 1000.0,
        stats.over_32ms
    )
}

/// Redact the account segment of every user path, whatever the environment says.
///
/// `USERPROFILE` and `HOME` can be missing, or point somewhere that is not the path the log
/// carries — under Proton, `USERPROFILE` is the Wine account while the paths are
/// `/var/home/<user>` and `Z:\\var\\home\\<user>`. The *shape* of the path is the reliable signal, so
/// this works the same on Windows (`C:\\Users\\<name>\\`) and on Linux (`/home/<name>/`,
/// `/var/home/<name>/`).
fn redact_user_paths(text: &str) -> String {
    let mut out = text.to_string();
    for marker in ["\\users\\", "\\home\\"] {
        out = scrub_after(&out, marker, '\\');
    }
    for marker in ["/users/", "/home/"] {
        out = scrub_after(&out, marker, '/');
    }
    out
}

fn scrub_after(text: &str, marker: &str, separator: char) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = lower[at..].find(marker) {
        let segment_start = at + found + marker.len();
        let Some(length) = text[segment_start..].find(separator) else { break };
        out.push_str(&text[at..segment_start]);
        out.push_str("%USER%");
        at = segment_start + length;
    }
    out.push_str(&text[at..]);
    out
}

/// Apply a list of (needle, replacement) pairs, then the path shape, then the character name.
pub fn redact(text: &str, needles: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (needle, replacement) in needles {
        if needle.len() > 3 {
            out = replace_ignore_case(&out, needle, replacement);
        }
    }
    // Whole paths go *before* the name rules: a path is the machine's layout, and replacing only
    // the account segment leaves `%USERPROFILE%\\AppData\\Local\\<mod manager>\\...` - the shape
    // of somebody's disk on its way to a stranger.
    out = redact_paths(&out);
    redact_character_slug(&redact_user_paths(&out))
}

/// Replace every machine path with `<path>`.
///
/// A path is recognised by its root - a drive letter and a colon before a separator, or one of the
/// usual Unix roots - and runs to the end of its line. That is deliberately blunt: Windows paths
/// contain spaces (`C:\\SteamLibrary\\steamapps\\common\\ELDEN RING\\Game\\eldenring.exe`), so
/// splitting on whitespace loses half of it, and every log line that carries a path in this mod
/// ends with it.
///
/// The file name goes with the directory: the interesting part of
/// `nr_casting_slots_<character>-399e89de.txt` is the character, which the same line already names
/// with the slug redacted, and the trailing hash is derived from that name.
pub fn redact_paths(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| match path_start(line) {
            Some(at) => {
                let (before, path) = line.split_at(at);
                let newline = if line.ends_with('\n') { "\n" } else { "" };
                // A path inside a mod manager's folder keeps its tail from `me3` on: that names the
                // profile and the mod - which is what makes a report useful - and says nothing about
                // the machine. Everything else is replaced whole, because the interesting part of
                // `C:\\SteamLibrary\\steamapps\\common\\ELDEN RING\\Game\\eldenring.exe` is the
                // file name, which the line already says.
                let kept = me3_tail(path).unwrap_or("<path>");
                format!("{before}{kept}{newline}")
            }
            None => line.to_string(),
        })
        .collect()
}

/// The part of a path from a `me3` folder onward, if it runs through one.
///
/// Matched only as a whole segment, preceded by a separator or at the start: `ermme3\\x` and
/// `me3.exe` are not mod-manager folders, and keeping them would keep whatever sits above them.
fn me3_tail(path: &str) -> Option<&str> {
    let lower = path.to_ascii_lowercase();
    for marker in ["me3\\", "me3/"] {
        let mut from = 0;
        while let Some(found) = lower[from..].find(marker) {
            let at = from + found;
            let whole_segment = at == 0 || matches!(lower.as_bytes()[at - 1], b'\\' | b'/');
            if whole_segment {
                return Some(&path[at..]);
            }
            from = at + marker.len();
        }
    }
    None
}

/// Where a path begins on this line, if one does.
fn path_start(line: &str) -> Option<usize> {
    const ROOTS: [&str; 7] = [
        "/home/",
        "/var/home/",
        "/usr/",
        "/opt/",
        "/mnt/",
        "/media/",
        "/tmp/",
    ];
    let bytes = line.as_bytes();
    // A drive letter, then ':', then a separator.
    for i in 0..bytes.len().saturating_sub(2) {
        if bytes[i].is_ascii_alphabetic()
            && bytes[i + 1] == b':'
            && (bytes[i + 2] == b'\\' || bytes[i + 2] == b'/')
        {
            return Some(i);
        }
    }
    const PLACEHOLDERS: [&str; 2] = ["%USERPROFILE%\\", "%HOME%\\"];
    ROOTS
        .iter()
        .filter_map(|root| line.find(root))
        .chain(PLACEHOLDERS.iter().filter_map(|root| line.find(root)))
        .min()
        .filter(|at| !line[..*at].ends_with('.'))
}

/// The guided run's phases, as data: (label, subsystems switched off).
///
/// This lives here rather than beside the code that applies it because the plan has a shape worth
/// asserting, and the assertions are what stop it drifting: a baseline at each end (so a machine
/// that changes speed mid-run shows up as drift instead of as a phase result), every subsystem
/// measured alone, and one phase with everything off.
pub const PHASES: [(&str, u32); 9] = [
    ("everything on", 0),
    ("raw input hooks off", RAW),
    ("cursor hooks taken out", POINTER),
    ("pad casting off (the hook still times)", PAD),
    ("housekeeping thread off", THREADS),
    ("pad only (raw, cursor and thread off)", RAW | POINTER | THREADS),
    ("every hook group off", PAD | RAW | POINTER | THREADS),
    ("everything back on", 0),
    ("everything still on (drift check)", 0),
];

/// The subsystems a phase can switch off.
pub const PAD: u32 = 1;
pub const RAW: u32 = 2;
pub const POINTER: u32 = 4;
pub const THREADS: u32 = 8;

/// Whether a throttled job may run now.
///
/// The rule the dispatcher exists for, kept here so it can be asserted rather than described: an
/// event may fire sixty times a second, and the handler declares how often acting on it is worth.
/// `last == 0` means it has never run, which is always due - otherwise a one-second throttle would
/// also delay the first run by a second after load.
pub fn due(now: u32, last: u32, min_ms: u32) -> bool {
    last == 0 || min_ms == 0 || now.wrapping_sub(last) >= min_ms
}

/// One line of the pad-path section: where our pad work spends its time.
///
/// Five figures in one format string, so it is assembled here where a test can read it - the last
/// line of this shape printed the wrong count for weeks because nothing asserted its text.
pub fn pad_cost_line(name: &str, calls: u32, mean_us: f64, worst_us: u32, slow: u32, slow_us: u64) -> String {
    format!(
        "  {name}: {calls} calls, mean {mean_us:.1} us, worst {worst_us} us, {slow} over {slow_us} us"
    )
}


/// Whether assignments are limited to the character's memorised spells.
///
/// `Attuned` is the reviewer-requested gate: vanilla caps a caster's library at the number of
/// memory slots, while this mod stores four spells per catalyst, so capacity used to scale with
/// the number of catalysts owned. Balanced mode makes the mod a better way to *use* your attuned
/// spells, not a way around attunement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Balance {
    /// Every spell the catalogue offers: today's behaviour, and the default.
    Free,
    /// Only the spells the character has attuned at a grace.
    Attuned,
}

impl Balance {
    /// The word written to the ini and printed in the report.
    pub fn as_str(self) -> &'static str {
        match self {
            Balance::Free => "free",
            Balance::Attuned => "attuned",
        }
    }
}

/// The `balance` ini value. Only the two names are accepted; anything else is `None` so the caller
/// keeps its current mode and logs the line rather than silently switching a player's gate.
pub fn parse_balance(text: &str) -> Option<Balance> {
    match text.trim().to_ascii_lowercase().as_str() {
        "free" => Some(Balance::Free),
        "attuned" => Some(Balance::Attuned),
        _ => None,
    }
}

/// What a spell is, from the catalogue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum School {
    Sorcery,
    Incantation,
    /// Not classified: `Other` catalysts and odd rows. Never treated as a mismatch.
    Unknown,
}

/// What a hand's catalyst can cast.
///
/// A pair rather than a kind, because "both" is a real answer in this game: the Staff of the Great
/// Beyond casts sorceries and incantations, and a content mod's catalyst can be anything. A single
/// kind cannot say that without a third variant meaning "everything", which is the same thing as
/// both flags set and reads worse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CatalystSchools {
    pub sorcery: bool,
    pub incantation: bool,
}

impl CatalystSchools {
    /// Casts both schools.
    pub const BOTH: Self = CatalystSchools { sorcery: true, incantation: true };
    /// A glintstone staff.
    pub const SORCERY: Self = CatalystSchools { sorcery: true, incantation: false };
    /// A sacred seal.
    pub const INCANTATION: Self = CatalystSchools { sorcery: false, incantation: true };
    /// A weapon that is not a catalyst at all.
    pub const NONE: Self = CatalystSchools { sorcery: false, incantation: false };

    /// Whether this catalyst offers a spell of that school. Anything unclassified is offered:
    /// greying a spell the player can really cast is the worse error.
    pub fn wants(self, spell: School) -> bool {
        match spell {
            School::Sorcery => self.sorcery,
            School::Incantation => self.incantation,
            School::Unknown => true,
        }
    }

    /// The line shown under the equipped weapon.
    ///
    /// "not applicable" rather than an empty line for a weapon that cannot cast: nothing equipped,
    /// unarmed, a dagger. Blankness there read as "no filter applied", which is the opposite of
    /// what it means, and the rows were still offered.
    pub fn label(self) -> &'static str {
        match (self.sorcery, self.incantation) {
            (true, true) => "sorceries and incantations",
            (true, false) => "sorceries only",
            (false, true) => "incantations only",
            (false, false) => "not applicable",
        }
    }

    /// Whether this hand can cast anything at all.
    pub fn any(self) -> bool {
        self.sorcery || self.incantation
    }
}

/// How one assigned row should be drawn, and whether it will cast.
///
/// Three states, deliberately: a fourth would need a reason nobody has asked for, and
/// "attuned but the game will refuse it" is a catalogue concern (stat requirements), not a gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    /// Nothing assigned: the input belongs to the game.
    Empty,
    /// Assigned, attuned, right school: casts.
    Attuned,
    /// Assigned, but the spell is no longer in the memorised set. Greyed, kept, castable never.
    NotAttuned,
    /// Assigned, attuned, but this catalyst cannot cast that school. Greyed with its own reason,
    /// because the fix is a different spell, not a grace visit.
    WrongSchool,
}

impl RowState {
    /// Whether the row is drawn greyed and passes the press through to the game.
    pub fn dormant(self) -> bool {
        matches!(self, RowState::NotAttuned | RowState::WrongSchool)
    }

    /// The marker drawn beside the row, empty for the two states that need no explanation.
    pub fn label(self) -> &'static str {
        match self {
            RowState::Empty => "",
            RowState::Attuned => "attuned",
            RowState::NotAttuned => "not attuned",
            RowState::WrongSchool => "wrong school for this catalyst",
        }
    }
}

/// Classify one row of the panel.
///
/// Order matters and is the whole rule: an empty row is empty whatever the memory holds; a school
/// mismatch is reported as a mismatch even when the spell is attuned (the player could attune it
/// correctly and still not cast it from this catalyst); only then does attunement decide.
pub fn classify(
    assigned: i32,
    spell: School,
    catalyst: CatalystSchools,
    attuned: &[i32],
) -> RowState {
    if assigned <= 0 {
        return RowState::Empty;
    }
    if !catalyst.wants(spell) {
        return RowState::WrongSchool;
    }
    if attuned.contains(&assigned) {
        RowState::Attuned
    } else {
        RowState::NotAttuned
    }
}

/// What one catalyst's four rows add up to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Rows with a spell in them.
    pub assigned: u32,
    /// Rows that will cast.
    pub live: u32,
    /// Rows greyed for either reason.
    pub stale: u32,
}

/// Count the rows of one catalyst. `assigned == live + stale` always, which is what makes the
/// report line checkable against the panel by eye.
pub fn counts(rows: &[RowState]) -> Counts {
    let mut counts = Counts::default();
    for row in rows {
        match row {
            RowState::Empty => {}
            RowState::Attuned => {
                counts.assigned += 1;
                counts.live += 1;
            }
            RowState::NotAttuned | RowState::WrongSchool => {
                counts.assigned += 1;
                counts.stale += 1;
            }
        }
    }
    counts
}

/// The four spells to use for a catalyst, preferring this exact item over the type's default.
///
/// Rows are filed on the full param id when a player customises one copy, and on the base id as
/// the default for the weapon type. Upgrading a staff changes its full id, so without the fallback
/// an upgrade at a smith would look like a different weapon and lose its assignments; two copies at
/// different upgrade levels keep separate overrides. Identical copies still share - no key can tell
/// them apart, and the inventory handle changes on upgrade, so it is the wrong fix.
pub fn resolve_row(base: i32, full: i32, rows: &[(i32, Category)]) -> Option<Category> {
    // Full id first: a customised copy overrides the type's default. Then the base id, which is
    // what makes an upgrade keep its layout. A stored row of all -1 is an answer, not a miss.
    let find = |id: i32| rows.iter().find(|(row, _)| *row == id).map(|(_, spells)| *spells);
    find(full).or_else(|| find(base))
}

/// The report's balance line: four figures in one string, so it is assembled where a test reads it.
///
/// `attuned` is how many spells the character has memorised, with no denominator: the mod cannot
/// read how many memory slots a character has unlocked, and printing a guessed 12 would be a lie
/// for every character that has not collected them all. `assigned` and `stale` are this
/// catalyst's rows; `reachable` is how many distinct attuned spells the equipped pair can cast -
/// the number that answers "why is my row grey" without opening the panel.
pub fn balance_line(
    mode: Balance,
    attuned: u32,
    assigned: u32,
    reachable: u32,
    not_attuned: u32,
) -> String {
    match mode {
        Balance::Free => "balance=free".to_string(),
        Balance::Attuned => format!(
            "balance=attuned attuned={attuned} assigned={assigned} reachable={reachable} \
             not_attuned={not_attuned}"
        ),
    }
}

/// Why a hand's chords are not ours on a class character, and what it costs.
///
/// `auto` yields Y+R1 / Y+L1 to the class port, whose defaults are Y+RB / Y+LB - the same chords -
/// and which hides those buttons from the game while it uses them. So the mod keeps three spells on
/// that hand, not four. The note names the class, the chord and the trade, because the alternative is
/// a player editing `y_bumper = on` and rebinding in the game's own menu without ever being told why.
/// Two lines: the panel draws them as they are, so neither can be clipped by a narrow window.
pub fn passthrough_note(class: &str, right_hand: bool) -> [String; 2] {
    let chord = if right_hand { "Y+R1" } else { "Y+L1" };
    [
        format!("{class} class: {chord} is its skill - 3 slots here"),
        format!("y_bumper = on for 4 (move its skills off {chord})"),
    ]
}

/// Every spell the panel already owns for this character.
///
/// Built from the saved rows rather than from the equipped pair: the reporter's complaint is about
/// "my 7 main spells" spread across two staves, and a set that changed when a weapon changed would
/// shuffle the vanilla wheel every time they swapped. Per-copy overrides are included because the
/// cast path resolves them, and a spell it can cast from a row is a spell the wheel should not
/// offer twice. `-1` is an empty row, not a spell.
pub fn assigned_spells(
    rows: &std::collections::HashMap<i32, Category>,
) -> std::collections::HashSet<i32> {
    rows.values()
        .flat_map(|spells| spells.iter())
        .copied()
        .filter(|spell| *spell > 0)
        .collect()
}

/// The first index at or after `current` that is free, wrapping, and `None` if none is.
///
/// "At or after" is deliberate: a caller whose current entry is already free gets its own index
/// back and has nothing to do. The scan is bounded by the list length, so a corrupt or huge
/// assignment set cannot spin.
pub fn next_unassigned(
    current: usize,
    spells: &[i32],
    assigned: &std::collections::HashSet<i32>,
) -> Option<usize> {
    if spells.is_empty() || current >= spells.len() {
        return None;
    }
    // Bounded by the list length: `take(len)` cannot run away even if `assigned` claims
    // everything, and the wrap is one arithmetic step rather than a modulo on the index.
    (0..spells.len())
        .map(|step| (current + step) % spells.len())
        .take(spells.len())
        // An empty entry (`-1`) is never a destination: the game does not cycle onto one either,
        // and landing there would blank the wheel.
        .find(|index| spells[*index] > 0 && !assigned.contains(&spells[*index]))
}

/// Where the selection should move to, or `None` to leave it alone.
///
/// This is the whole decision the vanilla-skip feature makes, kept here so it can be tested
/// without a game: an empty or unreadable list, an out-of-range index and an already-free spell all
/// mean "no move"; an assigned spell with somewhere free to go means "move there".
pub fn skip_target(
    current: usize,
    spells: &[i32],
    assigned: &std::collections::HashSet<i32>,
) -> Option<usize> {
    let spell = *spells.get(current)?;
    if !assigned.contains(&spell) {
        return None;
    }
    // `next_unassigned` includes the current index, so a move is only a move when it differs.
    next_unassigned(current, spells, assigned).filter(|target| *target != current)
}

/// Which spells the vanilla wheel should not offer.
///
/// Two reasons to hide one, and they are different questions:
///
/// 1. **the buttons already own it** - it is assigned on a weapon in hand, so pressing up to reach a
///    spell you can cast with one button is the clutter this feature exists to remove;
/// 2. **it cannot be cast with what is in hand** - incantations while holding a staff. The game would
///    put them on the wheel and then refuse the cast; hiding them is what a player asks for when they
///    unequip a seal ("the incantations weren't present, that was better").
///
/// A spell whose school is unknown is never hidden by (2): a wrong guess here hides a spell the
/// player can really cast, which is the worse error. If neither hand is a catalyst at all, (2) does
/// nothing - with nothing equipped the wheel is the game's again.
pub fn wheel_hidden(
    spells: &[i32],
    assigned: &std::collections::HashSet<i32>,
    capability: CatalystSchools,
    school_of: impl Fn(i32) -> School,
) -> std::collections::HashSet<i32> {
    let mut hidden = std::collections::HashSet::new();
    for spell in spells.iter().copied().filter(|spell| *spell > 0) {
        let uncastable = capability != CatalystSchools::NONE && !capability.wants(school_of(spell));
        if assigned.contains(&spell) || uncastable {
            hidden.insert(spell);
        }
    }
    hidden
}

/// Copy `height` rows of `row_bytes` bytes from a mapped texture into a DIB's pixels.
///
/// The two pitches differ by construction: a mapped D3D11 staging texture reports its own
/// `RowPitch` (often padded to a driver alignment), while a DIB section's rows are `width * 4`
/// for 32-bit pixels. Copying `size` bytes straight across would either shear the image or run off
/// the end of the destination; copying row by row with both pitches is the whole job, and it is the
/// only part of the present path that can be tested without a GPU.
///
/// Returns the number of rows copied, or `None` if either buffer is too small for what was asked —
/// the caller treats that as a failed frame rather than a partial blit.
pub fn copy_rows(
    src: &[u8],
    src_pitch: usize,
    dst: &mut [u8],
    dst_pitch: usize,
    height: usize,
    row_bytes: usize,
) -> Option<usize> {
    if row_bytes > src_pitch || row_bytes > dst_pitch {
        return None;
    }
    let src_needed = height.checked_sub(1)?.checked_mul(src_pitch)?.checked_add(row_bytes)?;
    let dst_needed = height.checked_sub(1)?.checked_mul(dst_pitch)?.checked_add(row_bytes)?;
    if src.len() < src_needed || dst.len() < dst_needed {
        return None;
    }
    for row in 0..height {
        let from = row * src_pitch;
        let to = row * dst_pitch;
        dst[to..to + row_bytes].copy_from_slice(&src[from..from + row_bytes]);
    }
    Some(height)
}

/// The sentence shown when the gate leaves a hand with nothing to offer, empty when it is fine.
pub const ATTUNE_FIRST: &str = "attune spells in the game's spell menu, then come back";

/// Why this hand's pickers are empty, if they are.
pub fn balance_notice(catalyst: CatalystSchools, attuned: &[School]) -> Option<&'static str> {
    if attuned.is_empty() {
        return Some(ATTUNE_FIRST);
    }
    // Only a catalyst that casts one school can be short of it: a both-school staff with nothing
    // but incantations attuned is not short of anything, it is simply a different build.
    match (catalyst.sorcery, catalyst.incantation) {
        (true, false) if !attuned.contains(&School::Sorcery) => {
            Some("no attuned sorcery left for this staff")
        }
        (false, true) if !attuned.contains(&School::Incantation) => {
            Some("no attuned incantation left for this seal")
        }
        _ => None,
    }
}


/// The store's lookup, with the caller's own map instead of a slice.
///
/// `resolve_row` is the same rule; this shape exists so `store_get` can ask a `HashMap` directly
/// rather than carrying a second copy of "full id first, then the type's default" - the copy that
/// made the rule untestable while its pure twin stayed green.
pub fn resolve_row_with<F>(base: i32, full: i32, find: F) -> Option<Category>
where
    F: Fn(i32) -> Option<Category>,
{
    match find(full) {
        Some(spells) => Some(spells),
        None if full == base => None,
        None => find(base),
    }
}

/// The row an edit lands on: this copy's override if it has one, otherwise the type's default.
///
/// The panel shows which of the two is being edited, so the rule has to be the same one the
/// display used; anything else would silently change which weapons an edit applies to.
pub fn store_target(base: i32, full: i32, has_override: bool) -> i32 {
    if full > 0 && full != base && has_override {
        full
    } else {
        base
    }
}

/// What the report counts: rows with a spell in them, and how many distinct attuned spells they
/// can actually cast.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Rows with a spell in them.
    pub assigned: u32,
    /// Distinct spells among them that are attuned.
    pub reachable: u32,
    /// Rows whose spell is not attuned. Counted per row rather than derived from the two figures
    /// above: the same spell on two inputs is one reachable spell but two assigned rows.
    pub not_attuned: u32,
}

/// Count `assigned` (non-empty rows) and `reachable` (distinct assigned spells that are attuned).
///
/// Distinct on purpose: the figure answers "how many of my attuned spells can I cast from this
/// pair", so the same spell on two inputs is one spell.
pub fn coverage(rows: &[i32], attuned: &[i32]) -> Coverage {
    let mut counts = Coverage::default();
    let mut seen: Vec<i32> = Vec::new();
    for spell in rows.iter().copied().filter(|spell| *spell > 0) {
        counts.assigned += 1;
        if !attuned.contains(&spell) {
            counts.not_attuned += 1;
        } else if !seen.contains(&spell) {
            seen.push(spell);
            counts.reachable += 1;
        }
    }
    counts
}


/// `bits_105` bit that says the catalyst casts sorceries.
pub const CATALYST_SORCERY_BIT: u8 = 0x80;
/// `bits_106` bit that says the catalyst casts incantations.
pub const CATALYST_INCANTATION_BIT: u8 = 0x02;

/// Read a catalyst's schools from the two flag bits in its `EquipParamWeapon` row.
///
/// Measured, not assumed: every vanilla staff has `0x87`/`0x10`, every vanilla seal `0x07`/`0x12`,
/// and the two dual catalysts in the game (the Staff of the Great Beyond and a class port's staff)
/// have `0x87`/`0x12` - one bit each, which is exactly the "catalyst type" the paramdef does not
/// name. The bits are validated against a known staff and a known seal before they are trusted (see
/// `params::catalyst_flags`), so a patched game falls back to the id band instead of lying.
pub fn schools_from_flags(bits_105: u8, bits_106: u8) -> CatalystSchools {
    CatalystSchools {
        sorcery: bits_105 & CATALYST_SORCERY_BIT != 0,
        incantation: bits_106 & CATALYST_INCANTATION_BIT != 0,
    }
}

// ---------------------------------------------------------------------------------------------
// Assignment profiles
// ---------------------------------------------------------------------------------------------

/// Name the first profile has, and the one that can be neither renamed nor deleted.
pub const DEFAULT_PROFILE: &str = "default";
/// How many profiles a player can hold: one per switching digit.
pub const MAX_PROFILES: usize = 9;
/// Longest accepted profile name, in bytes.
pub const PROFILE_NAME_MAX: usize = 24;
/// A second press of the same chord inside this window is key repeat, not a second switch.
pub const HOTKEY_DEBOUNCE_MS: u32 = 250;
/// Marker written into a file that holds more than one sheet.
///
/// Without it, a file with `[profile ...]` sections reads to an older build of the mod as the union
/// of every sheet - which is exactly why the marker exists rather than being inferred from the text.
pub const PROFILES_MARKER: &str =
    "# format 3: [profile <name>] sections; weapon rows belong to the section above them";

/// One profile: a named assignment sheet plus the two gameplay flags that belong to it.
///
/// `None` on a flag means "inherit the ini", so a sheet written by an older version (which had no
/// flags) does not silently force a mode on the player.
#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub balance: Option<Balance>,
    pub skip_assigned: Option<bool>,
    /// Digit the profile is switched with, `Some(1..=9)` once it has one.
    pub hotkey: Option<u8>,
    pub rows: Vec<(i32, Category)>,
}

/// Every profile in a slot file, in file order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Profiles {
    pub sheets: Vec<Sheet>,
}

impl Sheet {
    /// A sheet with nothing decided: no rows, no switch values, no hotkey.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            balance: None,
            skip_assigned: None,
            hotkey: None,
            rows: Vec::new(),
        }
    }

    /// A sheet that is live from the moment it exists, so both switches are concrete.
    pub fn with_switches(name: &str, balance: Balance, skip_assigned: bool) -> Self {
        Self {
            balance: Some(balance),
            skip_assigned: Some(skip_assigned),
            ..Self::new(name)
        }
    }
}

impl Profiles {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.sheets.iter().position(|sheet| sheet.name == name)
    }

    pub fn get(&self, name: &str) -> Option<&Sheet> {
        self.sheets.iter().find(|sheet| sheet.name == name)
    }

    /// Which sheet is live for the name held in the ini, and whether that name was unknown.
    ///
    /// An unknown name (a hand-edited ini, a profile deleted while the game was running) falls back
    /// to the first sheet - the default - and says so, rather than leaving the mod with no
    /// assignments at all.
    pub fn active(&self, name: &str) -> Option<(usize, bool)> {
        if self.sheets.is_empty() {
            return None;
        }
        match self.index_of(name) {
            Some(index) => Some((index, false)),
            None => Some((0, true)),
        }
    }

    /// The profile bound to `digit`, if any.
    pub fn for_digit(&self, digit: u8) -> Option<usize> {
        self.sheets.iter().position(|sheet| sheet.hotkey == Some(digit))
    }
}

/// Whether a name can be used for a profile: printable ASCII, short enough, no path separators.
///
/// This is a file-facing value written next to the player's own paths, so the rule is deliberately
/// narrow: a name cannot invent a directory, a comment character or a line break.
pub fn profile_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= PROFILE_NAME_MAX
        && name
            .chars()
            .all(|c| c.is_ascii_graphic() || c == ' ')
        && !name.contains(['/', '\\', '#', '[', ']', '=', '\t'])
}

/// Parse a slot file that may hold `[profile <name>]` sections.
///
/// A file with no section header at all is one sheet named `default`, with every row and setting in
/// it - that is what every version before profiles wrote, so an upgrade reads today's assignments as
/// the default profile without touching the file.
///
/// Unparseable lines are skipped, never fatal: the file is user-editable. A section whose name is
/// not usable is skipped together with its rows rather than being merged into its neighbour.
pub fn parse_profiles(text: &str) -> Profiles {
    let mut profiles = Profiles::default();
    let mut current: Option<Sheet> = Some(Sheet::new(DEFAULT_PROFILE));

    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("[profile") {
            let name = rest.trim_end_matches(']').trim();
            if let Some(sheet) = current.take() {
                if !sheet.rows.is_empty() || sheet.balance.is_some() || sheet.skip_assigned.is_some() {
                    push_sheet(&mut profiles, sheet);
                }
            }
            current = if profile_name_ok(name) {
                Some(Sheet::new(name))
            } else {
                None
            };
            continue;
        }
        if line.starts_with('[') {
            // Another kind of section header: nothing of ours lives outside `[profile ...]`.
            current = None;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        let Some(sheet) = current.as_mut() else {
            continue;
        };
        match key {
            "balance" => sheet.balance = parse_balance(value),
            "skip_assigned" => sheet.skip_assigned = parse_on_off(value),
            "hotkey" => sheet.hotkey = value.parse::<u8>().ok().filter(|d| (1..=9).contains(d)),
            _ => {
                let Ok(id) = key.parse::<i32>() else {
                    continue;
                };
                let ids: Vec<i32> = value
                    .split(',')
                    .filter_map(|part| part.trim().parse::<i32>().ok())
                    .collect();
                if ids.len() >= 2 {
                    sheet.rows.push((
                        id,
                        [
                            ids[0],
                            ids[1],
                            ids.get(2).copied().unwrap_or(-1),
                            ids.get(3).copied().unwrap_or(-1),
                        ],
                    ));
                }
            }
        }
    }
    if let Some(sheet) = current {
        push_sheet(&mut profiles, sheet);
    }

    // A file with no headers at all still yields exactly one sheet - including an empty file, so a
    // caller never has to special-case "no profiles yet".
    if profiles.sheets.is_empty() {
        profiles.sheets.push(Sheet::new(DEFAULT_PROFILE));
    }
    profiles
}

/// Add a sheet, merging same-named sections in file order (last wins per row, like the old parser).
fn push_sheet(profiles: &mut Profiles, sheet: Sheet) {
    if let Some(existing) = profiles.sheets.iter_mut().find(|s| s.name == sheet.name) {
        for (id, spells) in sheet.rows {
            existing.rows.push((id, spells));
        }
        existing.balance = sheet.balance.or(existing.balance);
        existing.skip_assigned = sheet.skip_assigned.or(existing.skip_assigned);
        existing.hotkey = sheet.hotkey.or(existing.hotkey);
        return;
    }
    profiles.sheets.push(sheet);
}

/// Render a slot file: the markers first, then one section per profile in order, rows sorted.
/// One `default` sheet holding these rows, written through the writer that ships.
///
/// Test-only: the old single-sheet writer (`format_slots`) is gone, and the tests that checked the
/// file layout now check the file the mod actually writes.
#[cfg(test)]
fn written(rows: &[(i32, Category)]) -> String {
    let mut profiles = parse_profiles("");
    profiles.sheets[0].rows = rows.to_vec();
    format_profiles(&profiles)
}

pub fn format_profiles(profiles: &Profiles) -> String {
    let mut out = String::from(
        "# weapon_param_id = bumper_spell, trigger_spell, y_trigger_spell, y_bumper_spell\n\
         # y_trigger_spell is cast by holding Y and pulling the trigger: Y+R2 (right weapon)\n\
         # and Y+L2 (left weapon). y_bumper_spell is cast by holding Y and pressing the bumper:\n\
         # Y+R1 and Y+L1, which are only ours when the class port is absent or the character\n\
         # is not one of its classes. Two- and three-entry lines from older versions are accepted.\n",
    );
    out.push_str(LAYOUT_MARKER);
    out.push('\n');
    out.push_str(PROFILES_MARKER);
    out.push('\n');
    for sheet in &profiles.sheets {
        out.push_str(&format!("\n[profile {}]\n", sheet.name));
        if let Some(balance) = sheet.balance {
            out.push_str(&format!("balance = {}\n", balance.as_str()));
        }
        if let Some(skip) = sheet.skip_assigned {
            out.push_str(&format!("skip_assigned = {}\n", if skip { "on" } else { "off" }));
        }
        if let Some(digit) = sheet.hotkey {
            out.push_str(&format!("hotkey = {digit}\n"));
        }
        let mut rows: Vec<&(i32, Category)> = sheet.rows.iter().collect();
        rows.sort_by_key(|(id, _)| *id);
        for (id, spells) in rows {
            out.push_str(&format!(
                "{id} = {}, {}, {}, {}\n",
                spells[0], spells[1], spells[2], spells[3]
            ));
        }
    }
    out
}

/// `on`/`off`, with the spellings players actually type.
fn parse_on_off(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Give the default profile a hotkey when nothing in the file has one.
///
/// A file written before hotkeys existed, and a fresh install, both parse to a `default` sheet with
/// no digit - which then made the *next* profile take Ctrl+1, so the first thing a player created
/// answered to the key the default should have had. The default is always reachable, so it gets the
/// first digit whenever no sheet claims one. Returns whether it changed anything.
pub fn ensure_default_hotkey(profiles: &mut Profiles) -> bool {
    if profiles.sheets.iter().any(|sheet| sheet.hotkey.is_some()) {
        return false;
    }
    let index = profiles
        .index_of(DEFAULT_PROFILE)
        .unwrap_or(0);
    if profiles.sheets.get(index).is_none() {
        return false;
    }
    profiles.sheets[index].hotkey = Some(1);
    true
}

/// The lowest digit no profile is using, for a newly created profile.
pub fn autobind_digit(profiles: &Profiles) -> Option<u8> {
    (1..=MAX_PROFILES as u8).find(|digit| profiles.for_digit(*digit).is_none())
}

/// Bind `digit` to the profile at `index`. Refuses a digit another profile already holds: silently
/// stealing it would make one hotkey switch two sheets depending on file order.
pub fn bind_digit(profiles: &mut Profiles, index: usize, digit: u8) -> Result<(), &'static str> {
    if !(1..=MAX_PROFILES as u8).contains(&digit) {
        return Err("a profile hotkey is a digit from 1 to 9");
    }
    if let Some(holder) = profiles.for_digit(digit) {
        if holder != index {
            return Err("another profile already uses that key");
        }
    }
    let Some(sheet) = profiles.sheets.get_mut(index) else {
        return Err("no such profile");
    };
    sheet.hotkey = Some(digit);
    Ok(())
}

/// Rename the profile at `index`. `default` keeps its name: it is the fallback every unknown
/// active name resolves to, so it has to stay findable.
pub fn rename_profile(
    profiles: &mut Profiles,
    index: usize,
    new_name: &str,
) -> Result<(), &'static str> {
    let Some(sheet) = profiles.sheets.get(index) else {
        return Err("no such profile");
    };
    if sheet.name == DEFAULT_PROFILE {
        return Err("the default profile cannot be renamed");
    }
    if !profile_name_ok(new_name) {
        return Err("a name is printable ASCII, up to 24 characters, without separators");
    }
    if profiles.sheets.iter().any(|s| s.name == new_name) {
        return Err("a profile with that name already exists");
    }
    profiles.sheets[index].name = new_name.to_string();
    Ok(())
}

/// Remove the profile at `index`, returning the name that is live afterwards.
///
/// `default` cannot be removed, and removing the active profile lands on `default` rather than on
/// whatever happens to be first: the player just deleted the sheet they were using, so the safe
/// answer is the one every install has.
pub fn delete_profile(
    profiles: &mut Profiles,
    index: usize,
    active_name: &str,
) -> Result<String, &'static str> {
    let Some(sheet) = profiles.sheets.get(index) else {
        return Err("no such profile");
    };
    if sheet.name == DEFAULT_PROFILE {
        return Err("the default profile cannot be deleted");
    }
    let removed = sheet.name.clone();
    let was_active = removed == active_name;
    profiles.sheets.remove(index);
    if was_active || profiles.get(active_name).is_none() {
        Ok(DEFAULT_PROFILE.to_string())
    } else {
        Ok(active_name.to_string())
    }
}

/// Which modifier the profile hotkeys use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyModifier {
    Ctrl,
    Alt,
    Shift,
    Super,
}

impl HotkeyModifier {
    pub fn as_str(self) -> &'static str {
        match self {
            HotkeyModifier::Ctrl => "Ctrl",
            HotkeyModifier::Alt => "Alt",
            HotkeyModifier::Shift => "Shift",
            HotkeyModifier::Super => "Super",
        }
    }
}

pub fn parse_hotkey_modifier(text: &str) -> Option<HotkeyModifier> {
    match text.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(HotkeyModifier::Ctrl),
        "alt" | "menu" => Some(HotkeyModifier::Alt),
        "shift" => Some(HotkeyModifier::Shift),
        "super" | "win" | "windows" => Some(HotkeyModifier::Super),
        _ => None,
    }
}

/// The label the panel shows next to the active profile, e.g. `Ctrl+2`.
pub fn hotkey_label(modifier: HotkeyModifier, digit: u8) -> String {
    format!("{}+{digit}", modifier.as_str())
}

/// Whether a key press may switch profiles now.
///
/// Three refusals, each with its reason: the panel owns the keyboard while it is open; a cast in
/// flight must not have its sheet swapped underneath it; and a chord held down repeats the key,
/// which must not cycle through every profile at the OS repeat rate.
pub fn hotkey_allowed(
    ui_open: bool,
    cast_in_flight: bool,
    now_ms: u32,
    last_press_ms: u32,
) -> bool {
    !ui_open
        && !cast_in_flight
        && now_ms.wrapping_sub(last_press_ms) > HOTKEY_DEBOUNCE_MS
}


#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    fn assigned(ids: &[i32]) -> HashSet<i32> {
        ids.iter().copied().collect()
    }

    #[test]
    fn the_wheel_hides_what_the_buttons_own_and_what_the_hands_cannot_cast() {
        use crate::School;
        // A staff in hand: sorceries yes, incantations no.
        let staff = crate::CatalystSchools::SORCERY;
        let spells = [4000, 5000, 6000];
        let school = |id: i32| match id {
            4000 => School::Sorcery,
            5000 => School::Incantation,
            _ => School::Unknown,
        };
        let owned = assigned(&[4000]);
        let hidden = crate::wheel_hidden(&spells, &owned, staff, school);
        // The assigned sorcery and the uncastable incantation are both out; the unknown stays in.
        assert!(hidden.contains(&4000), "{hidden:?}");
        assert!(hidden.contains(&5000), "{hidden:?}");
        assert!(!hidden.contains(&6000), "unknown school must stay offered");
        // With nothing equipped, only assignments hide anything: the wheel is the game's again.
        let none = crate::wheel_hidden(&spells, &owned, crate::CatalystSchools::NONE, school);
        assert_eq!(none, owned);
        // Empty entries are not spells and never enter the set.
        let empty = crate::wheel_hidden(&[-1, -1], &owned, staff, school);
        assert!(empty.is_empty(), "{empty:?}");
    }

    #[test]
    fn a_skipped_run_wraps_and_skips_the_whole_run() {
        // Eight memorised spells; 100 and 300 are ours.
        let spells = [100, 101, 102, 300, 104, 105, 106, 107];
        let ours = assigned(&[100, 300]);
        // Landing on 300 skips to 104.
        assert_eq!(crate::next_unassigned(3, &spells, &ours), Some(4));
        // Wrapping needs the tail to be ours: from the last entry, the only free spell is 100,
        // which is at the front.
        let tail = [100, 300, 301];
        let ours_tail = assigned(&[300, 301]);
        assert_eq!(crate::next_unassigned(2, &tail, &ours_tail), Some(0));
    }

    #[test]
    fn a_free_spell_returns_its_own_index() {
        let spells = [100, 101, 102];
        let ours = assigned(&[100]);
        // 101 is free, so the answer is where we already are.
        assert_eq!(crate::next_unassigned(1, &spells, &ours), Some(1));
    }

    #[test]
    fn everything_assigned_has_nowhere_to_go() {
        // Paired with the mixed case on purpose: `None` is also what a function that does nothing
        // returns, so the all-assigned assertion only means something beside one that must find a
        // free entry.
        let mixed = [100, 101, 102];
        assert_eq!(crate::next_unassigned(0, &mixed, &assigned(&[100])), Some(1));
        let spells = [100, 101];
        let ours = assigned(&[100, 101]);
        assert_eq!(crate::next_unassigned(0, &spells, &ours), None);
        assert_eq!(crate::skip_target(0, &spells, &ours), None);
    }

    #[test]
    fn an_empty_list_and_a_bad_index_do_nothing() {
        let ours = assigned(&[100]);
        assert_eq!(crate::next_unassigned(0, &[], &ours), None);
        assert_eq!(crate::skip_target(9, &[100, 101], &ours), None);
        // An empty entry is never a destination, even though it is not "ours" either.
        assert_eq!(crate::next_unassigned(0, &[-1, 101], &ours), Some(1));
        assert_eq!(crate::next_unassigned(0, &[-1, -1], &ours), None);
    }

    #[test]
    fn the_decision_only_moves_off_an_assigned_spell() {
        let spells = [100, 101, 102];
        let ours = assigned(&[100]);
        // On an assigned spell: move.
        assert_eq!(crate::skip_target(0, &spells, &ours), Some(1));
        // On a free spell: leave it alone, even though other entries are ours.
        assert_eq!(crate::skip_target(1, &spells, &ours), None);
    }

    #[test]
    fn the_assigned_set_comes_from_every_row_and_ignores_empty_ones() {
        let mut rows: HashMap<i32, crate::Category> = HashMap::new();
        rows.insert(33130000, [100, -1, 102, -1]);
        rows.insert(33090000, [100, 105, -1, -1]);
        // A per-copy override filed on the full id.
        rows.insert(33090010, [-1, -1, 900, -1]);
        let ours = crate::assigned_spells(&rows);
        assert_eq!(ours, assigned(&[100, 102, 105, 900]));
        assert!(!ours.contains(&-1));
    }

    #[test]
    fn copy_rows_respects_both_pitches() {
        // A staging texture padded to 8 bytes a row, a DIB padded to 4: a straight copy of the
        // pixels would shear every row by four bytes.
        let src = [
            1u8, 2, 3, 4, 0xAA, 0xAA, 0xAA, 0xAA, //
            5, 6, 7, 8, 0xAA, 0xAA, 0xAA, 0xAA,
        ];
        let mut dst = [0u8; 8];
        assert_eq!(crate::copy_rows(&src, 8, &mut dst, 4, 2, 4), Some(2));
        assert_eq!(dst, [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn copy_rows_refuses_buffers_it_cannot_fill() {
        let src = [0u8; 4];
        let mut dst = [0u8; 4];
        // Two rows wanted, one available.
        assert_eq!(crate::copy_rows(&src, 4, &mut dst, 4, 2, 4), None);
        // A row wider than either pitch.
        assert_eq!(crate::copy_rows(&src, 4, &mut dst, 4, 1, 8), None);
        // Zero height is nothing to do, and reports it rather than pretending it copied.
        assert_eq!(crate::copy_rows(&src, 4, &mut dst, 4, 0, 4), None);
    }

    #[test]
    fn passthrough_note_names_the_class_and_its_own_chord() {
        let right = crate::passthrough_note("Duchess", true).join("\n");
        let left = crate::passthrough_note("Duchess", false).join("\n");
        assert!(right.contains("Duchess class"), "{right}");
        assert!(right.contains("Y+R1"), "{right}");
        assert!(!right.contains("Y+L1"), "{right}");
        assert!(left.contains("Y+L1"), "{left}");
        assert!(!left.contains("Y+R1"), "{left}");
    }

    #[test]
    fn passthrough_note_states_the_cost_and_the_way_to_four() {
        let note = crate::passthrough_note("Recluse", true).join("\n");
        // The cost: three, not four. The remedy: the ini key and the rebind, both named.
        assert!(note.contains("3 slots"), "{note}");
        assert!(note.contains("y_bumper = on"), "{note}");
        assert!(note.contains("for 4"), "{note}");
        // Never the port's keyboard fallback: on the pad the chord goes to its own Y+RB / Y+LB.
        assert!(!note.contains("4 key"), "{note}");
        assert!(note.is_ascii(), "{note}");
    }

    use super::*;

    #[test]
    fn the_plan_brackets_itself_with_baselines() {
        // First and last are the same settings: a run measures a moving system, and without this a
        // machine that sped up halfway turned "everything back on" into the fastest phase of all.
        assert_eq!(PHASES[0].1, 0);
        assert_eq!(PHASES[PHASES.len() - 1].1, 0);
        assert_eq!(PHASES[PHASES.len() - 2].1, 0);
    }

    #[test]
    fn the_plan_measures_every_subsystem_alone() {
        for bit in [PAD, RAW, POINTER, THREADS] {
            assert!(
                PHASES.iter().any(|(_, off)| *off == bit),
                "no phase switches off only {bit:#x}"
            );
        }
        assert!(
            PHASES
                .iter()
                .any(|(_, off)| *off == PAD | RAW | POINTER | THREADS),
            "no phase switches everything off"
        );
    }

    #[test]
    fn a_throttle_lets_the_first_run_through_and_holds_the_next() {
        // Never run: due immediately, whatever the interval.
        assert!(due(1_000, 0, 5_000));
        // Ran a moment ago: held.
        assert!(!due(1_050, 1_000, 100));
        // Exactly at the interval is due, not one tick later.
        assert!(due(1_100, 1_000, 100));
        // Unthrottled handlers always run.
        assert!(due(1_001, 1_000, 0));
    }

    #[test]
    fn a_throttle_survives_the_millisecond_counter_wrapping() {
        // The clock is a u32 of milliseconds and wraps every 49 days. A wrapping subtraction is the
        // whole reason this works, so pin the case: last run just before the wrap, now just after.
        let last = u32::MAX - 50;
        assert!(due(60, last, 100));      // 111 ms later, across the wrap
        assert!(!due(20, last, 100));     // 71 ms later, still inside the interval
    }

    #[test]
    fn the_pad_cost_line_carries_all_five_figures() {
        let line = pad_cost_line("adjust_inner", 25439, 0.6, 427, 23, 250);
        assert_eq!(
            line,
            "  adjust_inner: 25439 calls, mean 0.6 us, worst 427 us, 23 over 250 us"
        );
    }

    fn held(buttons: u16, right: bool, left: bool, rb: bool, lb: bool) -> Held {
        Held {
            buttons,
            right_pull: right,
            left_pull: left,
            right_bump: rb,
            left_bump: lb,
        }
    }


    #[test]
    fn parses_three_value_lines() {
        let parsed = parse_slots("33090010 = 4431, 4060, 4710\n");
        assert_eq!(parsed, vec![(33090010, [4431, 4060, 4710, -1])]);
    }

    #[test]
    fn accepts_two_value_lines_from_older_versions() {
        let parsed = parse_slots("33240009 = 4080, 4040\n");
        assert_eq!(parsed, vec![(33240009, [4080, 4040, -1, -1])]);
    }

    #[test]
    fn keeps_negatives_and_ignores_noise() {
        let text = "\
# a comment line
   
hud_mirror = off
4101 = -1, 4000,   -1
not a slot
999 = abc, def
42 = 1
";
        let parsed = parse_slots(text);
        assert_eq!(parsed, vec![(4101, [-1, 4000, -1, -1])]);
    }

    #[test]
    fn trailing_comments_are_stripped() {
        assert_eq!(parse_slots("7 = 1, 2, 3  # my staff\n"), vec![(7, [1, 2, 3, -1])]);
    }

    #[test]
    fn duplicate_ids_are_returned_in_file_order() {
        let parsed = parse_slots("7 = 1, 2, 3\n7 = 4, 5, 6\n");
        assert_eq!(parsed, vec![(7, [1, 2, 3, -1]), (7, [4, 5, 6, -1])]);
    }

    #[test]
    fn format_sorts_and_round_trips() {
        let entries = vec![(20, [1, 2, 3, -1]), (10, [4, 5, -1, -1])];
        let text = written(&entries);
        assert!(text.contains("10 = 4, 5, -1"));
        assert!(text.find("10 =").unwrap() < text.find("20 =").unwrap());
        assert_eq!(
            parse_profiles(&text).sheets[0].rows,
            vec![(10, [4, 5, -1, -1]), (20, [1, 2, 3, -1])]
        );
    }

    #[test]
    fn idle_when_nothing_is_held() {
        assert_eq!(route(&held(0, false, false, false, false), true), Route::Idle);
    }

    #[test]
    fn plain_inputs_cast_their_own_slot() {
        assert_eq!(
            route(&held(0, true, false, false, false), true),
            Route::Cast { right_hand: true, slot: 1, label: "R2" }
        );
        assert_eq!(
            route(&held(BTN_RIGHT_SHOULDER, false, false, true, false), true),
            Route::Cast { right_hand: true, slot: 0, label: "R1" }
        );
        assert_eq!(
            route(&held(0, false, true, false, false), true),
            Route::Cast { right_hand: false, slot: 1, label: "L2" }
        );
        assert_eq!(
            route(&held(BTN_LEFT_SHOULDER, false, false, false, true), true),
            Route::Cast { right_hand: false, slot: 0, label: "L1" }
        );
    }

    #[test]
    fn ash_of_war_wins_over_everything() {
        let all = held(BTN_Y, true, true, true, true);
        assert_eq!(route(&all, true), Route::Chord { right_hand: true, slot: 2, label: "Y+R2" });
    }

    #[test]
    fn y_trigger_is_the_third_slot() {
        assert_eq!(
            route(&held(BTN_Y, true, false, false, false), true),
            Route::Chord { right_hand: true, slot: 2, label: "Y+R2" }
        );
        assert_eq!(
            route(&held(BTN_Y, false, true, false, false), true),
            Route::Chord { right_hand: false, slot: 2, label: "Y+L2" }
        );
    }

    #[test]
    fn y_without_a_trigger_is_not_a_chord() {
        // Y on its own is two-handing; it must not be swallowed.
        assert_eq!(route(&held(BTN_Y, false, false, false, false), true), Route::Idle);
    }

    #[test]
    fn mouse_mapping_matches_the_pad() {
        assert_eq!(mouse_route(true, false, false, false, false, true), Some((true, 0, "LMB")));
        assert_eq!(mouse_route(false, true, false, false, false, true), Some((false, 0, "RMB")));
        assert_eq!(mouse_route(true, false, false, true, false, true), Some((true, 2, "ctrl + LMB")));
        assert_eq!(mouse_route(false, true, false, true, false, true), Some((false, 2, "ctrl + RMB")));
        assert_eq!(mouse_route(true, false, false, false, true, true), Some((true, 3, "alt + LMB")));
        assert_eq!(mouse_route(false, true, false, false, true, true), Some((false, 3, "alt + RMB")));
        assert_eq!(mouse_route(true, false, true, false, false, true), Some((true, 1, "shift + LMB")));
        assert_eq!(mouse_route(false, true, true, false, false, true), Some((false, 1, "shift + RMB")));
        assert_eq!(mouse_route(true, true, false, false, false, true), None);
        assert_eq!(mouse_route(false, false, false, false, false, true), None);
        // On a Recluse character the Alt chords belong to the class port: nothing to steer.
        assert_eq!(mouse_route(true, false, false, false, true, false), None);
        assert_eq!(mouse_route(false, true, false, false, true, false), None);
        // Every other combination is still ours there.
        assert_eq!(
            mouse_route(true, false, false, true, false, false),
            Some((true, 2, "ctrl + LMB"))
        );
    }

    #[test]
    fn y_plus_bumper_is_the_fourth_slot_only_when_we_own_it() {
        let chord = |rb: bool, lb: bool| held(BTN_Y, false, false, rb, lb);
        assert_eq!(
            route(&chord(true, false), true),
            Route::Chord {
                right_hand: true,
                slot: 3,
                label: "Y+R1"
            }
        );
        assert_eq!(
            route(&chord(false, true), true),
            Route::Chord {
                right_hand: false,
                slot: 3,
                label: "Y+L1"
            }
        );
        // Recluse owns them on a Recluse character: the report must reach the game untouched.
        assert_eq!(route(&chord(true, false), false), Route::Idle);
        assert_eq!(route(&chord(false, true), false), Route::Idle);
    }

    #[test]
    fn the_y_trigger_chord_does_not_need_the_bumper_slot() {
        let held = held(BTN_Y, true, false, true, false);
        assert_eq!(
            route(&held, false),
            Route::Chord {
                right_hand: true,
                slot: 2,
                label: "Y+R2"
            }
        );
    }

    #[test]
    fn parses_four_value_lines_and_fills_short_ones() {
        let parsed = parse_slots("33090010 = 4431, 4060, 4710, 4800\n4101 = 1, 2\n");
        assert_eq!(
            parsed,
            vec![
                (33090010, [4431, 4060, 4710, 4800]),
                (4101, [1, 2, -1, -1])
            ]
        );
        let text = written(&[(33090010, [1, 2, 3, 4])]);
        assert!(text.contains("33090010 = 1, 2, 3, 4"));
        assert_eq!(parse_profiles(&text).sheets[0].rows, vec![(33090010, [1, 2, 3, 4])]);
    }

    #[test]
    fn upgrade_levels_fold_onto_the_base_weapon() {
        assert_eq!(base_weapon_id(33090010), 33090000);
        assert_eq!(base_weapon_id(33090000), 33090000);
        // Somber weapons stop at +10, standard ones at +25; both fit in the low two digits.
        assert_eq!(base_weapon_id(33240025), 33240000);
        // "No weapon" is passed through, never wrapped.
        assert_eq!(base_weapon_id(0), 0);
        assert_eq!(base_weapon_id(-1), -1);
        // A short id from a hand-edited file is not a weapon param id: leave it alone rather
        // than folding it onto a neighbour.
        assert_eq!(base_weapon_id(4101), 4101);
    }

    #[test]
    fn canonicalize_merges_the_same_catalyst_at_two_upgrade_levels() {
        let entries = vec![
            (33090010, [4431, -1, -1, -1]),
            (33090000, [-1, 4060, -1, -1]),
            (4101, [1, 2, -1, -1]),
        ];
        let out = canonicalize(&entries);
        assert_eq!(
            out,
            vec![(4101, [1, 2, -1, -1]), (33090000, [4431, 4060, -1, -1])]
        );
        // The first line keeps its own value when both assign the same slot.
        let out = canonicalize(&[(1, [10, -1, -1, -1]), (1, [20, -1, -1, -1])]);
        assert_eq!(out, vec![(1, [10, -1, -1, -1])]);
        assert!(canonicalize(&[]).is_empty());
    }

    #[test]
    fn bind_gesture_uses_the_cast_mapping() {
        // No spell input held: nothing to bind, even with the modifier down.
        assert_eq!(bind_route(&held(0, false, false, false, false)), None);
        assert_eq!(
            bind_route(&held(0, false, false, true, false)),
            Some((true, 0, "R1"))
        );
        assert_eq!(
            bind_route(&held(0, false, true, false, false)),
            Some((false, 1, "L2"))
        );
        // The Y chords keep their slots, so L3 + Y + R2 binds the third.
        assert_eq!(
            bind_route(&held(BTN_Y, true, false, false, false)),
            Some((true, 2, "Y+R2"))
        );
        assert_eq!(
            bind_route(&held(BTN_Y, false, false, false, true)),
            Some((false, 3, "Y+L1"))
        );
    }
}

/// Format one crash line into `buf`, returning its length.
///
/// Lives here because it must be allocation-free (it runs in an exception handler) and because
/// the format is worth a test: this is the one artifact a user's bug report will contain.
///
/// `access` is the (operation, target address) pair of an access violation: `0` reading,
/// `1` writing, `8` executing.
// Nine fields of a fault record, formatted in a fixed order into a caller's buffer. A struct would
// add a copy on the one path that must not touch the heap, and this signature is mirrored by
// `logging::log_fault` in the DLL.
#[allow(clippy::too_many_arguments)]
pub fn fault_line(
    buf: &mut [u8],
    ms: u64,
    code: u32,
    address: usize,
    thread: u32,
    module_base: usize,
    offset: usize,
    module: &str,
    access: Option<(usize, usize)>,
) -> usize {
    let mut writer = Writer { buf, len: 0 };
    writer.push(b"[");
    writer.decimal(ms);
    writer.push(b"] crash: exception 0x");
    writer.hex(code as u64, 8);
    writer.push(b" at 0x");
    writer.hex(address as u64, 1);
    if !module.is_empty() {
        writer.push(b" in ");
        writer.push(module.as_bytes());
        writer.push(b" (base 0x");
        writer.hex(module_base as u64, 1);
        writer.push(b", offset 0x");
        writer.hex(offset as u64, 1);
        writer.push(b")");
    }
    writer.push(b" thread ");
    writer.decimal(thread as u64);
    if let Some((operation, target)) = access {
        writer.push(match operation {
            0 => b" reading 0x".as_slice(),
            1 => b" writing 0x",
            8 => b" executing 0x",
            _ => b" accessing 0x",
        });
        writer.hex(target as u64, 1);
    }
    writer.push(b"\n");
    writer.len
}

struct Writer<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl Writer<'_> {
    fn push(&mut self, text: &[u8]) {
        for byte in text {
            if self.len < self.buf.len() {
                self.buf[self.len] = *byte;
                self.len += 1;
            }
        }
    }

    /// Lowercase hex, zero-padded to at least `digits`.
    fn hex(&mut self, value: u64, digits: usize) {
        const TABLE: &[u8; 16] = b"0123456789abcdef";
        let mut started = false;
        for shift in (0..16).rev() {
            let digit = ((value >> (shift * 4)) & 0xF) as usize;
            if digit != 0 || started || shift == 0 || shift < digits {
                started = true;
                self.push(&[TABLE[digit]]);
            }
        }
    }

    fn decimal(&mut self, value: u64) {
        let mut digits = [0u8; 20];
        let mut count = 0;
        let mut value = value;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            let byte = digits[count];
            self.push(&[byte]);
        }
    }
}

#[cfg(test)]
mod fault_tests {
    use super::fault_line;

    fn render(
        code: u32,
        address: usize,
        thread: u32,
        base: usize,
        offset: usize,
        module: &str,
        access: Option<(usize, usize)>,
    ) -> String {
        let mut buf = [0u8; 512];
        let len = fault_line(&mut buf, 1234, code, address, thread, base, offset, module, access);
        String::from_utf8_lossy(&buf[..len]).to_string()
    }

    #[test]
    fn names_the_module_and_offset() {
        let line = render(
            0xC000_0005,
            0x0014_0123_4567,
            4321,
            0x1_4000_0000,
            0x123_4567,
            "eldenring.exe",
            None,
        );
        assert_eq!(
            line,
            "[1234] crash: exception 0xc0000005 at 0x1401234567 in eldenring.exe \
             (base 0x140000000, offset 0x1234567) thread 4321\n"
        );
    }

    #[test]
    fn says_what_an_access_violation_was_doing() {
        let line = render(
            0xC000_0005,
            0x6FFF_FA40_2690,
            17,
            0x6FFF_FA00_0000,
            0x40_2690,
            "recluse_input.dll",
            Some((1, 0xDEAD_BEEF)),
        );
        assert!(line.contains(" writing 0xdeadbeef"), "{line}");
        assert!(line.contains("in recluse_input.dll"), "{line}");
    }

    #[test]
    fn survives_a_missing_module_and_a_short_buffer() {
        let line = render(0xC000_0374, 0x1000, 1, 0, 0x1000, "", None);
        assert_eq!(line, "[1234] crash: exception 0xc0000374 at 0x1000 thread 1\n");

        let mut tiny = [0u8; 12];
        let len = fault_line(&mut tiny, 7, 0xC000_0005, 0x140_0000_0000, 9, 0, 0, "a.dll", None);
        assert_eq!(len, 12);
        assert!(std::str::from_utf8(&tiny).is_ok());
    }

}

#[cfg(test)]
mod raw_tests {
    use super::*;

    /// Build a raw record the way `GetRawInputData` lays it out on x64.
    fn record(kind: u32, size: usize, body: &[(usize, u8)]) -> Vec<u8> {
        let mut bytes = vec![0u8; size.max(RAW_HEADER)];
        bytes[0..4].copy_from_slice(&kind.to_le_bytes());
        bytes[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        for (at, byte) in body {
            if *at < bytes.len() {
                bytes[*at] = *byte;
            }
        }
        bytes
    }

    #[test]
    fn reads_a_mouse_record_and_its_button_edges() {
        let bytes = record(RAW_TYPE_MOUSE, RAW_MOUSE_LEN, &[(RAW_MOUSE_BUTTONS_AT, 0x01)]);
        assert_eq!(raw_kind(&bytes), Some(RAW_TYPE_MOUSE));
        assert_eq!(raw_size(&bytes), Some(RAW_MOUSE_LEN));
        assert_eq!(raw_mouse_buttons(&bytes), Some(RI_MOUSE_LEFT_BUTTON_DOWN));
    }

    #[test]
    fn character_slugs_separate_names_and_slots() {
        let (base, slug) = character_slug("Warlock", 1);
        assert_eq!(base, "warlock");
        assert!(slug.starts_with("warlock-"), "{slug}");
        // Same name, other slot: a different file.
        assert_ne!(slug, character_slug("Warlock", 0).1);
        // Names that normalise identically still differ.
        assert_eq!(character_slug("Test 2", 0).0, character_slug("test-2", 0).0);
        assert_ne!(character_slug("Test 2", 0).1, character_slug("test-2", 0).1);
        // Empty or symbol-only names still produce something usable.
        let (base, slug) = character_slug("  ", 3);
        assert_eq!(base, "character");
        assert!(slug.starts_with("character-"));
        // Long names are capped, and the base never ends in a dash.
        let (base, _) = character_slug(&"a".repeat(64), 0);
        assert_eq!(base.len(), 24);
        assert!(!character_slug("Test!", 0).0.ends_with('-'));
    }

    #[test]
    fn rewrite_buttons_in_place() {
        let mut bytes = record(RAW_TYPE_MOUSE, RAW_MOUSE_LEN, &[(RAW_MOUSE_BUTTONS_AT, 0x05)]);
        assert!(raw_set_mouse_buttons(&mut bytes, RI_MOUSE_RIGHT_BUTTON_UP));
        assert_eq!(raw_mouse_buttons(&bytes), Some(RI_MOUSE_RIGHT_BUTTON_UP));
        // Not a mouse record, and too short: left untouched, reported as not written.
        let mut keyboard = record(RAW_TYPE_KEYBOARD, RAW_KEY_LEN, &[]);
        assert!(!raw_set_mouse_buttons(&mut keyboard, 0));
        let mut short = record(RAW_TYPE_MOUSE, 8, &[]);
        assert!(!raw_set_mouse_buttons(&mut short, 0));
    }

    #[test]
    fn refuses_records_that_are_not_mouse_or_are_truncated() {
        let keyboard = record(RAW_TYPE_KEYBOARD, RAW_KEY_LEN, &[]);
        assert_eq!(raw_mouse_buttons(&keyboard), None);
        // Header says mouse, dwSize says 8: reading the buttons would run off the end.
        let short = record(RAW_TYPE_MOUSE, 8, &[]);
        assert_eq!(raw_mouse_buttons(&short), None);
        // Short buffer as well as short dwSize.
        assert_eq!(raw_mouse_buttons(&[0u8; 4]), None);
        assert_eq!(raw_keyboard(&[0u8; 20]), None);
    }

    #[test]
    fn reads_a_keyboard_record_and_hides_shift() {
        let mut bytes = record(RAW_TYPE_KEYBOARD, RAW_KEY_LEN, &[(RAW_KEY_VKEY_AT, 0xA0)]);
        assert_eq!(raw_keyboard(&bytes), Some((0, 0xA0)));
        assert!(raw_hide_shift(&mut bytes));
        assert_eq!(raw_keyboard(&bytes), Some((RI_KEY_BREAK, 0xA0)));
        // Already released is not rewritten a second time.
        assert!(!raw_hide_shift(&mut bytes));
    }

    #[test]
    fn leaves_other_keys_and_truncated_records_alone() {
        let mut other = record(RAW_TYPE_KEYBOARD, RAW_KEY_LEN, &[(RAW_KEY_VKEY_AT, 0x20)]);
        assert!(!raw_hide_shift(&mut other));
        assert_eq!(raw_keyboard(&other), Some((0, 0x20)));
        let mut short = record(RAW_TYPE_KEYBOARD, 8, &[]);
        assert!(!raw_hide_shift(&mut short));
    }

    #[test]
    fn an_unset_deadline_is_never_armed() {
        assert!(!armed_until(0, 0));
        assert!(!armed_until(1_700_000_000, 0));
        // The value that made Y go dead at a Site of Grace: high bit set, subtraction positive,
        // i32 reading negative.
        assert!(!armed_until(0x8000_0000, 0));
        assert!(!armed_until(0xFFFF_FFFF, 0));
    }

    #[test]
    fn a_deadline_ahead_is_armed_and_one_behind_is_not() {
        assert!(armed_until(1_000, 1_700));
        assert!(!armed_until(1_700, 1_000));
        assert!(!armed_until(1_700, 1_700));
    }

    #[test]
    fn a_deadline_still_reads_correctly_across_the_clock_wrapping() {
        // Armed 48 ms before the counter wraps, 64 ms later: still ahead.
        assert!(armed_until(0xFFFF_FFF0, 0x20));
        // Armed 0x40 before the wrap, now 0x50 past it: long gone.
        assert!(!armed_until(0x50, 0xFFFF_FFC0));
    }

    #[test]
    fn ui_scale_reads_auto_percentages_and_multipliers() {
        assert_eq!(ui_scale("auto"), Some((None, "auto".to_string())));
        assert_eq!(ui_scale("1.5"), Some((Some(1.5), "150%".to_string())));
        assert_eq!(ui_scale("150"), Some((Some(1.5), "150%".to_string())));
        assert_eq!(ui_scale("150%"), Some((Some(1.5), "150%".to_string())));
        // `100` and `1.0` are the same request spelled two ways.
        assert_eq!(ui_scale("100"), Some((Some(1.0), "100%".to_string())));
        assert_eq!(ui_scale("1.0"), Some((Some(1.0), "100%".to_string())));
        assert_eq!(ui_scale("  200 "), Some((Some(2.0), "200%".to_string())));
    }

    #[test]
    fn ui_scale_rejects_what_it_cannot_honour() {
        // Out of range rather than clamped: the caller keeps its previous value and logs it.
        assert_eq!(ui_scale("0.5"), None);
        assert_eq!(ui_scale("25%"), None);
        assert_eq!(ui_scale("4"), None);
        assert_eq!(ui_scale("big"), None);
        assert_eq!(ui_scale(""), None);
    }

    #[test]
    fn panel_size_reads_the_saved_geometry() {
        assert_eq!(panel_size("auto"), Some((0, 0)));
        assert_eq!(panel_size("1800x1240"), Some((1800, 1240)));
        // Hand-edited spellings are not worth rejecting.
        assert_eq!(panel_size("1800X1240"), Some((1800, 1240)));
        assert_eq!(panel_size("1800,1240"), Some((1800, 1240)));
        assert_eq!(panel_size(" 1350 x 930 "), Some((1350, 930)));
        // Anything the layout cannot use falls back to auto at the caller.
        assert_eq!(panel_size("800x600"), None);
        assert_eq!(panel_size("1800"), None);
        assert_eq!(panel_size("1800x"), None);
        assert_eq!(panel_size("huge"), None);
    }

    /// The shape below is the one a reporter's log carried; every value in it is a placeholder.
    ///
    /// Never paste a real log line into a test: a path names an account and a profile, and a test
    /// is a file that gets committed and read by strangers. `<name>`, `<manager>`, `<profile>` and
    /// `<mod>` keep the shape that the rule depends on and carry nothing of anybody's machine.
    #[test]
    fn a_mod_manager_path_keeps_only_the_me3_tail() {
        let text = r"[1] config: debug_log=false files in C:\Users\<name>\AppData\Local\<manager>\me3\config\profiles\<profile>\<mod>\nr_casting";
        let out = crate::redact_paths(text);
        assert!(
            out.contains(r"me3\config\profiles\<profile>\<mod>\nr_casting"),
            "{out}"
        );
        // Nothing above `me3` survives: no drive, no account, no manager name.
        for gone in ["Users", "<name>", "AppData", "Local", "<manager>", "C:"] {
            assert!(!out.contains(gone), "{gone} survived: {out}");
        }
    }

    #[test]
    fn a_path_without_me3_is_replaced_whole() {
        let text = r"[1] XInputGetState: caller stack includes C:\Program Files (x86)\Steam\steamapps\common\ELDEN RING\Game\eldenring.exe";
        let out = crate::redact_paths(text);
        assert!(out.ends_with("<path>"), "{out}");
        assert!(!out.contains("Steam"), "{out}");
        assert!(out.starts_with("[1] XInputGetState: caller stack includes "), "{out}");
    }

    #[test]
    fn the_linux_me3_tail_is_kept_the_same_way() {
        let out = crate::redact_paths("/home/u/.local/share/me3/config/profiles/p/nr_casting");
        assert_eq!(out, "me3/config/profiles/p/nr_casting");
    }

    #[test]
    fn a_line_without_a_path_is_untouched() {
        let text = "[1] panel: ready (hotkey registered)";
        assert_eq!(crate::redact_paths(text), text);
    }

    #[test]
    fn redaction_leaves_a_path_without_a_user_segment_alone() {
        let text = r"C:\SteamLibrary\steamapps\common\ELDEN RING\Game\eldenring.exe";
        assert_eq!(redact_user_paths(text), text);
    }

    #[test]
    fn redaction_covers_case_and_both_path_spellings() {
        // The three spellings the diagnostics writer supplies for one home directory: the host
        // path, its Windows-drive form, and the preface-less form a log can print.
        let needles = vec![
            ("C:\\users\\<name>".to_string(), "%USERPROFILE%".to_string()),
            ("/var/home/<name>".to_string(), "%HOME%".to_string()),
            ("\\var\\home\\<name>".to_string(), "%HOME%".to_string()),
        ];
        let text = "log at C:\\Users\\<name>\\AppData\\x.log and Z:\\var\\home\\<name>\\y and /var/home/<name>/z";
        let out = redact(text, &needles);
        assert!(!out.contains("<name>"), "{out}");
        // The paths are gone entirely now; neither placeholder survives either, because replacing
        // the account segment alone left `%USERPROFILE%\\AppData\\...` in the file, which is still
        // the layout of somebody's disk. A path runs to the end of its line, so three paths on one
        // line collapse into one placeholder - the alternative is guessing where a Windows path
        // ends when its directory names contain spaces, and guessing short leaves half a path.
        assert_eq!(out, "log at <path>", "{out}");
    }





    #[test]
    fn every_marker_is_at_least_six_bytes() {
        // The test that would have caught two false positives: a four-byte needle matches by chance
        // in a large binary, which is how a DLSS DLL was labelled DXVK and two NVIDIA DLLs ENB.
        let markers: [&[u8]; 10] = [
            b"dxvk-cache",
            b"dxvk_config",
            b"dxvk_hud",
            b"reshade",
            b"special k",
            b"specialk",
            b"vkcreateinstance",
            b"nvngx_dlssg",
            b"d3d12core",
            b"sl.interposer",
        ];
        for marker in markers {
            assert!(marker.len() >= 6, "{} is too short", String::from_utf8_lossy(marker));
        }
        // The exact bytes that produced the wrong label: base64 noise containing "DXvK".
        let noise = b"bNINCcwaiUktTEzASj0NLS3rigptkyyQqzgL1gr6DXvKo61SjFxuLUonDFgMLg0wa3xs7aq1KaONd60O";
        assert!(wrapper_markers(noise).is_empty(), "{:?}", wrapper_markers(noise));
    }

    #[test]
    fn wrapper_markers_need_six_bytes_to_be_worth_anything() {
        // A real DXVK export table mentions the name and its entry points.
        let dxvk = b"...DXVK... state cache: .dxvk-cache ... vkCreateInstance ...";
        assert_eq!(wrapper_markers(dxvk), vec!["DXVK", "Vulkan"]);
        // Three bytes of noise hit by chance in a big binary; nothing here is a wrapper.
        let noise: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        assert!(wrapper_markers(&noise).is_empty(), "{:?}", wrapper_markers(&noise));
        assert_eq!(wrapper_markers(b"special k"), vec!["Special K"]);
        assert_eq!(wrapper_markers(b"SpecialK"), vec!["SpecialK"]);
    }

    #[test]
    fn pe_build_timestamp_reads_the_header_or_refuses() {
        let mut image = vec![0u8; 0x200];
        image[0..2].copy_from_slice(b"MZ");
        image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x88..0x8c].copy_from_slice(&1_700_000_000u32.to_le_bytes());
        assert_eq!(pe_build_timestamp(&image), Some(1_700_000_000));
        // No stamp written by the linker.
        image[0x88..0x8c].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(pe_build_timestamp(&image), None);
        // Not a PE at all, and a truncated one.
        assert_eq!(pe_build_timestamp(b"not a pe"), None);
        let mut elf = image.clone();
        elf[0] = b'\x7f';
        assert_eq!(pe_build_timestamp(&elf), None);
    }

    #[test]
    fn phase_lines_says_how_often_as_well_as_how_bad() {
        let line = phase_lines(&PhaseSummary {
            index: 4,
            phases: 6,
            label: "pad casting off",
            off: "pad hooks",
            stats: gap_stats(&[14_500, 66_000, 0, 15_000]),
            hitches: 7,
            seconds_with_hitches: 1,
            seconds: 4,
            worst_at: 2,
            ticks: 1800,
        });
        assert!(line.starts_with("phase 4/6 pad casting off (off: pad hooks): "), "{line}");
        assert!(line.contains("4 s sampled (1 with no pad poll)"), "{line}");
        assert!(line.contains("1800 ticks, 450/s"), "{line}");
        assert!(line.contains("7 hitches over 32ms in 1 of 4 seconds"), "{line}");
        assert!(line.contains("worst 66ms at second 2"), "{line}");
    }

    #[test]
    fn civil_date_matches_known_dates() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(1_700_000_000), "2023-11-14");
        assert_eq!(civil_date(1_782_000_000), "2026-06-21");
        // A PE timestamp is a u32 count of seconds, and 2106 is the wrap.
        assert_eq!(civil_date(4_294_967_295), "2106-02-07");
    }

    #[test]
    fn phase_clock_walks_the_phases_and_finishes_on_the_last() {
        let mut clock = PhaseClock::new(2, 3);
        assert_eq!(clock.total(), 6);
        assert_eq!(clock.left(), 3);
        assert_eq!(clock.tick(), PhaseTick::Second);
        assert_eq!(clock.tick(), PhaseTick::Second);
        assert_eq!(clock.tick(), PhaseTick::NextPhase);
        assert_eq!((clock.phase(), clock.left()), (1, 3));
        assert_eq!(clock.tick(), PhaseTick::Second);
        assert_eq!(clock.tick(), PhaseTick::Second);
        // The last second of the last phase ends the run; it must not open a seventh phase.
        assert_eq!(clock.tick(), PhaseTick::Finished);
        assert_eq!((clock.phase(), clock.left()), (1, 1));
        clock.restart();
        assert_eq!((clock.phase(), clock.left()), (0, 3));
    }

    #[test]
    fn phase_clock_counts_every_second_once() {
        let mut clock = PhaseClock::new(6, 60);
        let mut seconds = 0;
        let mut phases = 1;
        loop {
            match clock.tick() {
                PhaseTick::Second => seconds += 1,
                PhaseTick::NextPhase => {
                    seconds += 1;
                    phases += 1;
                }
                PhaseTick::Finished => {
                    seconds += 1;
                    break;
                }
            }
        }
        assert_eq!(seconds, 360, "six minutes");
        assert_eq!(phases, 6);
    }

    #[test]
    fn gap_stats_ignores_quiet_seconds_and_keeps_the_worst() {
        let stats = gap_stats(&[16_000, 0, 66_000, 40_000, 16_000, 0]);
        assert_eq!(stats.sampled, 4);
        assert_eq!(stats.quiet, 2);
        assert_eq!(stats.max, 66_000);
        assert_eq!(stats.over_32ms, 2);
        // sorted: 16, 16, 40, 66 - the lower median slot, as the old inline code did.
        assert_eq!(stats.p50, 16_000);
        assert_eq!(gap_stats(&[]), GapStats::default());
        assert_eq!(gap_stats(&[0, 0]).sampled, 0);
        assert!(gap_line(&gap_stats(&[0])).contains("none with a pad poll"));
        assert!(gap_line(&gap_stats(&[66_000])).contains("1 seconds over 32ms"));
    }

    #[test]
    fn redact_paths_takes_the_whole_path_and_nothing_else() {
        // Verbatim from a report: the account segment was already replaced, and the disk layout
        // was still in the file.
        let line = "slots: character <character> (id 0) -> %USERPROFILE%\\AppData\\Local\\garyttierney\\me3\\config\\profiles\\eldenring-mods\\nr-casting-v0.2.7\\nr_casting\\nr_casting_slots_<character>-399e89de.txt";
        // The manager path keeps its tail from `me3` on - profile, mod, file - and the account
        // segment, the manager's own folder and the placeholder above it all go.
        assert_eq!(
            redact_paths(line),
            "slots: character <character> (id 0) -> me3\\config\\profiles\\eldenring-mods\\nr-casting-v0.2.7\\nr_casting\\nr_casting_slots_<character>-399e89de.txt"
        );
        // Windows paths contain spaces, so a whitespace split loses half of them.
        let line = "XInputGetState: caller stack includes C:\\SteamLibrary\\steamapps\\common\\ELDEN RING\\Game\\eldenring.exe";
        assert_eq!(
            redact_paths(line),
            "XInputGetState: caller stack includes <path>"
        );
        // The same line under Proton names the host's home, and the prefix path.
        assert_eq!(
            redact_paths("files in: /var/home/<name>/.local/share/Steam/steamapps/compatdata/1245620/pfx"),
            "files in: <path>"
        );
        assert_eq!(
            redact_paths("STEAM_COMPAT_DATA_PATH=Z:\\var\\home\\<name>\\.local\\share\\Steam"),
            "STEAM_COMPAT_DATA_PATH=<path>"
        );
    }

    #[test]
    fn redact_paths_leaves_everything_else_alone() {
        for line in [
            "modules: eldenring.exe@0x140000000",
            "input: R1/R2/Y+R1/Y+R2 cast the right-hand catalyst",
            "panel: 1194x601 client, 1194x601 buffer, scale 1.25, content 579px",
            "perf: 1200 polls in 10.0s, gap mean 8.33ms max 16.9ms (0 over 32ms)",
            "probe.stats = ok (76, 18, 9)",
            "4000 = Glintstone Pebble | Sorcery | 10 | 0 | 0 | 7",
        ] {
            assert_eq!(redact_paths(line), line);
        }
    }

    #[test]
    fn redaction_keeps_the_slots_scheme_but_drops_the_character() {
        let out = redact_character_slug("nr_casting_slots_hamilcar-399e89de.txt");
        // The hash stays: it is a per-character id, not a name.
        assert_eq!(out, "nr_casting_slots_<character>-399e89de.txt");
        // A line with the name inside a sentence, twice.
        let line = "loaded nr_casting_slots_Warlock-2badd3.txt, also nr_casting_slots_warlock-2badd3.txt";
        let out = redact_character_slug(line);
        assert!(!out.to_lowercase().contains("warlock"), "{out}");
        assert_eq!(out.matches("<character>").count(), 2);
    }

    #[test]
    fn panel_size_accepts_the_size_the_panel_saves() {
        // The panel saves its outer size, and a content-fitted panel is shorter than the 620 design
        // height. Rejecting it meant the saved geometry never applied: every launch logged a
        // warning and sized the window from the screen again.
        assert_eq!(panel_size("1200x607"), Some((1200, 607)));
        assert_eq!(panel_size("1194x601"), Some((1194, 601)));
        assert_eq!(panel_size("900x400"), Some((900, 400)));
        // Still refuses what the window cannot be.
        assert_eq!(panel_size("899x600"), None);
        assert_eq!(panel_size("1200x399"), None);
        assert_eq!(panel_size("1200x9000"), None);
    }

    #[test]
    fn panel_pos_reads_a_saved_corner() {
        assert_eq!(panel_pos("auto"), Some((0, 0)));
        assert_eq!(panel_pos("120,80"), Some((120, 80)));
        assert_eq!(panel_pos(" 1920 , 40 "), Some((1920, 40)));
        // A monitor to the left of the primary has a negative origin.
        assert_eq!(panel_pos("-1920,120"), Some((-1920, 120)));
        assert_eq!(panel_pos("1920x40"), Some((1920, 40)));
        assert_eq!(panel_pos("left"), None);
        assert_eq!(panel_pos("120"), None);
        assert_eq!(panel_pos("100000,80"), None);
    }

    #[test]
    fn fit_scale_fits_the_axis_that_binds() {
        // The case that produced scrollbars on both sides: 1764x792 is 1.96 wide but only 1.27
        // tall, so the height decides and the floor puts it a step under.
        assert_eq!(fit_scale(1764.0, 792.0, DESIGN_HEIGHT), 1.25);
        // 4K: the clamp, not the fit, is what stops it.
        assert_eq!(fit_scale(3840.0, 2160.0, DESIGN_HEIGHT), MAX_UI_SCALE);
        // A window at the design size cannot fit the design height plus its frame, so it is the
        // floor, not 0.98.
        assert_eq!(fit_scale(DESIGN_WIDTH, DESIGN_HEIGHT, DESIGN_HEIGHT), MIN_UI_SCALE);
    }

    #[test]
    fn fit_scale_never_exceeds_the_fit_on_either_axis() {
        for width in [900.0, 1200.0, 1764.0, 1920.0, 2560.0, 3840.0] {
            for height in [600.0, 620.0, 792.0, 1080.0, 1440.0, 2160.0] {
                for natural in [0.0, 400.0, 620.0, 900.0] {
                    let scale = fit_scale(width, height, natural);
                    let natural = if natural > 1.0 { natural } else { DESIGN_HEIGHT };
                    if scale == MIN_UI_SCALE {
                        // The floor, not the fit, decided — matching the window's minimum size,
                        // which keeps the panel from shrinking into unreadable text. Nothing to
                        // prove about fitting here: the window is simply smaller than the design.
                        continue;
                    }
                    // Within a step of the fit, never over it by more than the step.
                    assert!(
                        DESIGN_WIDTH * scale <= width + DESIGN_WIDTH * 0.05,
                        "{width}x{height} natural {natural}: {scale}"
                    );
                    assert!(
                        natural * scale <= height + DESIGN_HEIGHT * 0.05,
                        "{width}x{height} natural {natural}: {scale}"
                    );
                }
            }
        }
    }

    #[test]
    fn fit_scale_is_a_step_and_inside_its_bounds() {
        for tenth in 0..=300 {
            let scale = fit_scale(900.0 + tenth as f32 * 10.0, 620.0 + tenth as f32 * 7.0, 620.0);
            assert!((MIN_UI_SCALE..=MAX_UI_SCALE).contains(&scale), "{scale}");
            let steps = scale * SCALE_STEPS;
            assert!((steps - steps.round()).abs() < 1e-3, "{scale} is not a 5% step");
        }
    }

    #[test]
    fn fit_scale_survives_a_degenerate_window() {
        // Hidden, mid-drag or not yet created: a zero rect must not panic or return a zero scale.
        assert_eq!(fit_scale(0.0, 0.0, 0.0), MIN_UI_SCALE);
        assert_eq!(fit_scale(-5.0, 3.0, f32::NAN), MIN_UI_SCALE);
    }
}


#[cfg(test)]
mod balance_tests {
    use super::*;

    // ---- parse_balance -------------------------------------------------------------------

    #[test]
    fn balance_accepts_its_two_names() {
        assert_eq!(parse_balance("free"), Some(Balance::Free));
        assert_eq!(parse_balance("attuned"), Some(Balance::Attuned));
    }

    #[test]
    fn balance_ignores_case_and_surrounding_space() {
        // The ini is hand-edited, so " Attuned " is a player being tidy, not an error.
        assert_eq!(parse_balance("  ATTUNED "), Some(Balance::Attuned));
        assert_eq!(parse_balance("\tFree\r"), Some(Balance::Free));
    }

    #[test]
    fn balance_rejects_booleans() {
        // on/off reads like a switch, but this key has two named modes; accepting a boolean would
        // leave the ini saying "on" about a value that is not either mode's name.
        for value in ["on", "off", "true", "false", "1", "0"] {
            assert_eq!(parse_balance(value), None, "{value}");
        }
    }

    #[test]
    fn balance_rejects_anything_else() {
        for value in ["", "balanced", "attune", "attuned!", "free2"] {
            assert_eq!(parse_balance(value), None, "{value}");
        }
    }

    // ---- classify ------------------------------------------------------------------------

    #[test]
    fn an_empty_row_is_empty_whatever_the_memory_holds() {
        let attuned = [10, 11];
        assert_eq!(classify(-1, School::Sorcery, CatalystSchools::SORCERY, &attuned), RowState::Empty);
        assert_eq!(classify(0, School::Unknown, CatalystSchools::BOTH, &attuned), RowState::Empty);
        assert_eq!(classify(-1, School::Unknown, CatalystSchools::BOTH, &[]), RowState::Empty);
    }

    #[test]
    fn an_attuned_spell_of_the_right_school_casts() {
        assert_eq!(
            classify(11, School::Sorcery, CatalystSchools::SORCERY, &[10, 11]),
            RowState::Attuned
        );
        assert_eq!(
            classify(40, School::Incantation, CatalystSchools::INCANTATION, &[40]),
            RowState::Attuned
        );
    }

    #[test]
    fn a_catalyst_that_casts_both_accepts_either_school() {
        // The Staff of the Great Beyond: a staff id that casts incantations too. The band rule
        // would grey these rows, and they cast perfectly well in game.
        assert_eq!(
            classify(40, School::Incantation, CatalystSchools::BOTH, &[40]),
            RowState::Attuned
        );
        assert_eq!(
            classify(11, School::Sorcery, CatalystSchools::BOTH, &[11]),
            RowState::Attuned
        );
        // Attunement still decides on a both-school catalyst.
        assert_eq!(
            classify(40, School::Incantation, CatalystSchools::BOTH, &[11]),
            RowState::NotAttuned
        );
    }

    #[test]
    fn a_weapon_that_cannot_cast_greys_every_assignment() {
        // Unarmed, a dagger, a shield: nothing casts, so nothing is live - even a spell that is
        // still attuned. The row is kept and stays editable; it simply cannot come out here.
        assert_eq!(
            classify(10, School::Sorcery, CatalystSchools::NONE, &[10]),
            RowState::WrongSchool
        );
        assert_eq!(classify(-1, School::Sorcery, CatalystSchools::NONE, &[10]), RowState::Empty);
        assert_eq!(balance_notice(CatalystSchools::NONE, &[School::Sorcery]), None);
    }

    #[test]
    fn a_both_school_catalyst_is_never_out_of_one_school() {
        // Attuned only sorceries, on a staff that also casts incantations: nothing to warn about.
        assert_eq!(
            balance_notice(CatalystSchools::BOTH, &[School::Sorcery]),
            None
        );
        assert_eq!(
            balance_notice(CatalystSchools::BOTH, &[School::Sorcery, School::Incantation]),
            None
        );
    }

    #[test]
    fn the_label_names_what_the_catalyst_offers() {
        assert_eq!(CatalystSchools::SORCERY.label(), "sorceries only");
        assert_eq!(CatalystSchools::INCANTATION.label(), "incantations only");
        assert_eq!(CatalystSchools::BOTH.label(), "sorceries and incantations");
        assert_eq!(CatalystSchools::NONE.label(), "not applicable");
        assert!(!CatalystSchools::NONE.any());
        assert!(CatalystSchools::BOTH.any());
        assert!(!CatalystSchools::NONE.wants(School::Sorcery));
        assert!(!CatalystSchools::NONE.wants(School::Incantation));
        // An unclassified spell is always offered.
        assert!(CatalystSchools::SORCERY.wants(School::Unknown));
    }

    #[test]
    fn a_school_mismatch_outranks_attunement() {
        // The spell is attuned and still cannot be cast from this catalyst: the fix is a different
        // spell, not a grace visit, so it must not read as "not attuned".
        assert_eq!(
            classify(40, School::Incantation, CatalystSchools::SORCERY, &[40]),
            RowState::WrongSchool
        );
        assert_eq!(
            classify(11, School::Sorcery, CatalystSchools::INCANTATION, &[11]),
            RowState::WrongSchool
        );
    }

    #[test]
    fn an_unattuned_spell_is_not_attuned() {
        assert_eq!(
            classify(99, School::Sorcery, CatalystSchools::SORCERY, &[10, 11]),
            RowState::NotAttuned
        );
        assert_eq!(
            classify(99, School::Sorcery, CatalystSchools::SORCERY, &[]),
            RowState::NotAttuned
        );
    }

    #[test]
    fn an_unclassified_spell_is_never_a_mismatch() {
        // A class port can put an odd row in a staff: without a school there is nothing to compare,
        // and greying a spell that casts would be worse than not filtering it.
        assert_eq!(
            classify(7, School::Unknown, CatalystSchools::SORCERY, &[7]),
            RowState::Attuned
        );
        assert_eq!(
            classify(7, School::Unknown, CatalystSchools::SORCERY, &[8]),
            RowState::NotAttuned
        );
    }

    #[test]
    fn an_unclassified_catalyst_never_filters() {
        assert_eq!(
            classify(40, School::Incantation, CatalystSchools::BOTH, &[40]),
            RowState::Attuned
        );
        assert_eq!(
            classify(11, School::Sorcery, CatalystSchools::BOTH, &[11]),
            RowState::Attuned
        );
    }

    #[test]
    fn only_the_two_dormant_states_are_dormant() {
        assert!(!RowState::Empty.dormant());
        assert!(!RowState::Attuned.dormant());
        assert!(RowState::NotAttuned.dormant());
        assert!(RowState::WrongSchool.dormant());
        assert_eq!(RowState::Attuned.label(), "attuned");
        assert_eq!(RowState::NotAttuned.label(), "not attuned");
        assert_eq!(RowState::WrongSchool.label(), "wrong school for this catalyst");
        assert_eq!(RowState::Empty.label(), "");
    }

    // ---- counts --------------------------------------------------------------------------

    #[test]
    fn a_full_attuned_catalyst_counts_four_live() {
        let rows = [RowState::Attuned; 4];
        assert_eq!(counts(&rows), Counts { assigned: 4, live: 4, stale: 0 });
    }

    #[test]
    fn a_mixed_catalyst_splits_live_and_stale() {
        let rows = [RowState::Attuned, RowState::Empty, RowState::NotAttuned, RowState::WrongSchool];
        assert_eq!(counts(&rows), Counts { assigned: 3, live: 1, stale: 2 });
    }

    #[test]
    fn all_stale_is_all_stale() {
        let rows = [RowState::NotAttuned; 4];
        assert_eq!(counts(&rows), Counts { assigned: 4, live: 0, stale: 4 });
    }

    #[test]
    fn an_empty_catalyst_counts_nothing() {
        assert_eq!(counts(&[RowState::Empty; 4]), Counts::default());
        assert_eq!(counts(&[]), Counts::default());
    }

    #[test]
    fn assigned_is_always_live_plus_stale() {
        // This identity is what lets a reader check the report against the panel by eye.
        for a in 0..4 {
            for live in 0..=a {
                for stale in 0..=(a - live) {
                    let mut rows = Vec::new();
                    rows.extend(std::iter::repeat_n(RowState::Attuned, live as usize));
                    rows.extend(std::iter::repeat_n(RowState::NotAttuned, stale as usize));
                    rows.extend(std::iter::repeat_n(RowState::Empty, 4 - a as usize));
                    let c = counts(&rows);
                    assert_eq!(c.assigned, live + stale, "{rows:?}");
                    assert_eq!(c.assigned, c.live + c.stale);
                }
            }
        }
    }


    // ---- layout marker -------------------------------------------------------------------

    #[test]
    fn a_written_file_says_it_understands_per_copy_rows() {
        let text = written(&[(33_090_000, [1, 2, 3, 4])]);
        assert!(per_copy_rows(&text));
        // And the marker is a comment, so an older version parsing it still reads the same rows.
        assert_eq!(parse_profiles(&text).sheets[0].rows, vec![(33_090_000, [1, 2, 3, 4])]);
    }

    #[test]
    fn a_file_from_before_overrides_is_folded_instead() {
        // No marker: a row on 33090010 is an upgraded id to be folded onto its base, not a layout.
        let legacy = "33090010 = 1, 2, 3, 4\n";
        assert!(!per_copy_rows(legacy));
        assert_eq!(canonicalize(&parse_slots(legacy)), vec![(33_090_000, [1, 2, 3, 4])]);
        // With the marker the same row survives as an override.
        let current = format!("{LAYOUT_MARKER}\n33090010 = 1, 2, 3, 4\n");
        assert!(per_copy_rows(&current));
    }

    #[test]
    fn a_roundtrip_keeps_an_override_row() {
        let rows = [(33_090_000, [1, 2, 3, 4]), (33_090_010, [5, 6, 7, 8])];
        let text = written(&rows);
        assert_eq!(parse_profiles(&text).sheets[0].rows, rows);
    }



    // ---- catalyst flags ------------------------------------------------------------------

    #[test]
    fn the_flag_bits_split_staves_seals_and_dual_catalysts() {
        // Measured from the running game: 12 staves, 11 seals, and the two dual catalysts.
        assert_eq!(schools_from_flags(0x87, 0x10), CatalystSchools::SORCERY);
        assert_eq!(schools_from_flags(0x07, 0x12), CatalystSchools::INCANTATION);
        assert_eq!(schools_from_flags(0x87, 0x12), CatalystSchools::BOTH);
        assert_eq!(schools_from_flags(0x00, 0x00), CatalystSchools::NONE);
        // A row that is not a catalyst at all reads as none, and the caller falls back.
        assert_eq!(schools_from_flags(0x07, 0x10), CatalystSchools::NONE);
    }

    // ---- resolve_row_with: the store's lookup, testable --------------------------------

    #[test]
    fn the_store_lookup_prefers_the_copy_then_the_type() {
        // Same rule as `resolve_row`, but over a caller's own lookup so the store can use it
        // without building a map-shaped copy of the rule.
        let rows = [(33_090_000, [1, 2, 3, 4]), (33_090_010, [9, 8, 7, 6])];
        let find = |id: i32| rows.iter().find(|(row, _)| *row == id).map(|(_, s)| *s);
        assert_eq!(resolve_row_with(33_090_000, 33_090_010, find), Some([9, 8, 7, 6]));
        assert_eq!(resolve_row_with(33_090_000, 33_090_025, find), Some([1, 2, 3, 4]));
        assert_eq!(resolve_row_with(12_345_678, 12_345_678, find), None);
    }

    #[test]
    fn the_store_lookup_returns_an_empty_default_as_an_answer() {
        let rows = [(33_090_000, [-1; 4])];
        let find = |id: i32| rows.iter().find(|(row, _)| *row == id).map(|(_, s)| *s);
        assert_eq!(resolve_row_with(33_090_000, 33_090_010, find), Some([-1; 4]));
    }

    #[test]
    fn the_store_lookup_asks_full_first_and_base_second() {
        // Order is the rule: asking base first would hand a customised copy its type's layout.
        let asked = std::cell::RefCell::new(Vec::new());
        let find = |id: i32| {
            asked.borrow_mut().push(id);
            None
        };
        assert_eq!(resolve_row_with(33_090_000, 33_090_010, find), None);
        assert_eq!(asked.into_inner(), vec![33_090_010, 33_090_000]);
    }

    // ---- store_target: which row an edit lands on --------------------------------------

    #[test]
    fn an_edit_lands_on_the_row_that_was_shown() {
        // Override present: the edit belongs to that copy.
        assert_eq!(store_target(33_090_000, 33_090_010, true), 33_090_010);
        // No override: it belongs to the type's default, so every copy of it changes.
        assert_eq!(store_target(33_090_000, 33_090_010, false), 33_090_000);
        // A weapon at its base id has no copy of its own to override.
        assert_eq!(store_target(33_090_000, 33_090_000, true), 33_090_000);
        assert_eq!(store_target(33_090_000, 33_090_000, false), 33_090_000);
    }

    // ---- coverage: the report's two counts --------------------------------------------

    #[test]
    fn coverage_counts_what_is_assigned_and_what_can_cast() {
        let attuned = [10, 11, 12];
        // Three rows, all attuned: three assigned, three reachable.
        assert_eq!(
            coverage(&[10, 11, 12], &attuned),
            Coverage { assigned: 3, reachable: 3, not_attuned: 0 }
        );
        // One stale, one empty: the empty row is not an assignment.
        assert_eq!(
            coverage(&[10, 99, -1], &attuned),
            Coverage { assigned: 2, reachable: 1, not_attuned: 1 }
        );
        // Nothing at all.
        assert_eq!(coverage(&[], &attuned), Coverage::default());
        assert_eq!(coverage(&[-1, -1], &attuned), Coverage::default());
    }

    #[test]
    fn coverage_counts_a_spell_once_even_on_two_slots() {
        // The number answers "how many of my attuned spells are reachable", not "how many inputs
        // are filled": the same spell in two slots is still one spell.
        let attuned = [10, 11];
        assert_eq!(
            coverage(&[10, 10, 11], &attuned),
            Coverage { assigned: 3, reachable: 2, not_attuned: 0 }
        );
    }

    #[test]
    fn coverage_is_zero_when_nothing_is_attuned() {
        assert_eq!(
            coverage(&[10, 11], &[]),
            Coverage { assigned: 2, reachable: 0, not_attuned: 2 }
        );
    }

    // ---- resolve_row ---------------------------------------------------------------------

    const BASE: i32 = 33_090_000;
    const PLUS_TEN: i32 = 33_090_010;

    #[test]
    fn an_override_on_this_copy_wins() {
        let rows = vec![
            (BASE, [1, 2, 3, 4]),
            (PLUS_TEN, [9, 8, 7, 6]),
        ];
        assert_eq!(resolve_row(BASE, PLUS_TEN, &rows), Some([9, 8, 7, 6]));
    }

    #[test]
    fn an_upgraded_weapon_falls_back_to_the_types_default() {
        // The whole point: upgrading at a smith changes the full id, and the layout must survive.
        let rows = vec![(BASE, [1, 2, 3, 4])];
        assert_eq!(resolve_row(BASE, PLUS_TEN, &rows), Some([1, 2, 3, 4]));
        // A second upgrade level reads the same default.
        assert_eq!(resolve_row(BASE, 33_090_025, &rows), Some([1, 2, 3, 4]));
    }

    #[test]
    fn an_unknown_weapon_resolves_to_nothing() {
        assert_eq!(resolve_row(BASE, PLUS_TEN, &[]), None);
        assert_eq!(resolve_row(BASE, PLUS_TEN, &[(12_345_678, [1, 2, 3, 4])]), None);
    }

    #[test]
    fn a_row_on_the_full_id_alone_is_found() {
        // Files can hold an override without a default (the default was reset): it must still work.
        let rows = vec![(PLUS_TEN, [9, 8, 7, 6])];
        assert_eq!(resolve_row(BASE, PLUS_TEN, &rows), Some([9, 8, 7, 6]));
    }

    #[test]
    fn an_unbound_default_is_returned_as_it_is() {
        // A default of all -1 is a real answer, not a miss: the caller must not fall through to
        // some other row on the strength of it being empty.
        let rows = vec![(BASE, [-1; 4]), (PLUS_TEN, [9, 8, 7, 6])];
        assert_eq!(resolve_row(BASE, PLUS_TEN, &rows), Some([9, 8, 7, 6]));
        assert_eq!(resolve_row(BASE, 33_090_001, &rows), Some([-1; 4]));
    }

    // ---- balance_line --------------------------------------------------------------------

    #[test]
    fn the_line_prints_four_figures_in_attuned_mode() {
        // No denominator on `attuned`: how many memory slots a character has unlocked is not
        // readable, and a guessed "/12" would be wrong for every character that lacks a stone.
        assert_eq!(
            balance_line(Balance::Attuned, 9, 8, 4, 1),
            "balance=attuned attuned=9 assigned=8 reachable=4 not_attuned=1"
        );
    }

    #[test]
    fn the_line_says_only_free_in_free_mode() {
        // Free mode has no gate to report; printing zeroes would read as a broken count.
        assert_eq!(balance_line(Balance::Free, 0, 0, 0, 0), "balance=free");
        assert_eq!(balance_line(Balance::Free, 9, 8, 4, 1), "balance=free");
    }

    // ---- balance_notice ------------------------------------------------------------------

    #[test]
    fn nothing_attuned_asks_for_a_grace_visit() {
        assert_eq!(
            balance_notice(CatalystSchools::SORCERY, &[]),
            Some(ATTUNE_FIRST)
        );
        assert_eq!(balance_notice(CatalystSchools::BOTH, &[]), Some(ATTUNE_FIRST));
    }

    #[test]
    fn a_staff_needs_a_sorcery() {
        assert_eq!(
            balance_notice(CatalystSchools::SORCERY, &[School::Incantation, School::Incantation]),
            Some("no attuned sorcery left for this staff")
        );
        assert_eq!(balance_notice(CatalystSchools::SORCERY, &[School::Sorcery]), None);
    }

    #[test]
    fn a_seal_needs_an_incantation() {
        assert_eq!(
            balance_notice(CatalystSchools::INCANTATION, &[School::Sorcery]),
            Some("no attuned incantation left for this seal")
        );
        assert_eq!(balance_notice(CatalystSchools::INCANTATION, &[School::Incantation]), None);
    }

    #[test]
    fn an_unclassified_catalyst_is_never_told_why() {
        assert_eq!(balance_notice(CatalystSchools::BOTH, &[School::Unknown]), None);
        assert_eq!(balance_notice(CatalystSchools::BOTH, &[School::Sorcery]), None);
    }

    #[test]
    fn the_notices_are_plain_ascii() {
        // ProggyClean covers 0x20-0xFF; a smart quote renders as a box in the panel.
        for text in [
            ATTUNE_FIRST,
            "no attuned sorcery left for this staff",
            "no attuned incantation left for this seal",
        ] {
            assert!(text.is_ascii(), "{text}");
            assert!(!text.contains('\u{2019}'));
        }
    }
}


#[cfg(test)]
mod profile_tests {
    use super::*;

    fn sheet(name: &str, rows: &[(i32, Category)]) -> Sheet {
        Sheet {
            name: name.to_string(),
            balance: None,
            skip_assigned: None,
            hotkey: None,
            rows: rows.to_vec(),
        }
    }

    const ROW_A: (i32, Category) = (33090010, [4431, 4430, -1, 4400]);
    const ROW_B: (i32, Category) = (33240000, [4381, -1, -1, 4710]);

    #[test]
    fn a_file_without_sections_is_the_default_profile() {
        let text = "# weapon_param_id = bumper, trigger, y_trigger, y_bumper\n\
                    33090010 = 4431, 4430, -1, 4400\n\
                    33240000 = 4381, -1, -1, 4710\n";
        let profiles = parse_profiles(text);
        assert_eq!(profiles.sheets.len(), 1);
        assert_eq!(profiles.sheets[0].name, DEFAULT_PROFILE);
        assert_eq!(profiles.sheets[0].rows, vec![ROW_A, ROW_B]);
    }

    #[test]
    fn sections_do_not_leak_rows_into_each_other() {
        let text = "[profile default]\n33090010 = 4431, 4430, -1, 4400\n\
                    [profile boss]\n33240000 = 4381, -1, -1, 4710\n";
        let profiles = parse_profiles(text);
        assert_eq!(profiles.sheets.len(), 2);
        assert_eq!(profiles.sheets[0].rows, vec![ROW_A]);
        assert_eq!(profiles.sheets[1].rows, vec![ROW_B]);
        assert_eq!(profiles.sheets[1].name, "boss");
    }

    #[test]
    fn a_round_trip_keeps_rows_flags_and_bindings() {
        let mut profiles = Profiles::default();
        let mut first = sheet(DEFAULT_PROFILE, &[ROW_A]);
        first.balance = Some(Balance::Attuned);
        first.skip_assigned = Some(true);
        first.hotkey = Some(1);
        let mut second = sheet("boss", &[ROW_B]);
        second.balance = Some(Balance::Free);
        second.hotkey = Some(2);
        profiles.sheets.push(first);
        profiles.sheets.push(second);

        let text = format_profiles(&profiles);
        assert_eq!(parse_profiles(&text), profiles);
    }

    #[test]
    fn flags_a_sheet_does_not_state_are_left_to_the_ini() {
        let profiles = parse_profiles("[profile default]\n33090010 = 1, 2, -1, -1\n");
        assert_eq!(profiles.sheets[0].balance, None);
        assert_eq!(profiles.sheets[0].skip_assigned, None);
    }

    #[test]
    fn the_default_profile_claims_the_first_digit_when_nothing_has_one() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[ROW_A]));
        assert!(ensure_default_hotkey(&mut profiles));
        assert_eq!(profiles.sheets[0].hotkey, Some(1));
        // A second call is a no-op: something already holds a digit.
        assert!(!ensure_default_hotkey(&mut profiles));
    }

    #[test]
    fn a_hotkey_already_in_the_file_is_left_alone() {
        let mut profiles = Profiles::default();
        let mut boss = sheet("boss", &[ROW_A]);
        boss.hotkey = Some(2);
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[]));
        profiles.sheets.push(boss);
        assert!(!ensure_default_hotkey(&mut profiles));
        assert_eq!(profiles.sheets[1].hotkey, Some(2));
    }

    #[test]
    fn a_new_profile_takes_the_lowest_free_digit() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[ROW_A]));
        assert_eq!(autobind_digit(&profiles), Some(1));
        profiles.sheets[0].hotkey = Some(1);
        assert_eq!(autobind_digit(&profiles), Some(2));
        profiles.sheets[0].hotkey = Some(3);
        profiles.sheets.push(sheet("second", &[]));
        profiles.sheets[1].hotkey = Some(1);
        assert_eq!(autobind_digit(&profiles), Some(2));
    }

    #[test]
    fn nine_profiles_exhaust_the_digits() {
        let mut profiles = Profiles::default();
        for digit in 1..=9u8 {
            let mut s = sheet(&format!("p{digit}"), &[]);
            s.hotkey = Some(digit);
            profiles.sheets.push(s);
        }
        assert_eq!(autobind_digit(&profiles), None);
    }

    #[test]
    fn an_explicit_binding_survives_a_reload() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[ROW_A]));
        profiles.sheets.push(sheet("boss", &[ROW_B]));
        bind_digit(&mut profiles, 1, 7).expect("free digit");
        let reloaded = parse_profiles(&format_profiles(&profiles));
        assert_eq!(reloaded.sheets[1].hotkey, Some(7));
        assert_eq!(reloaded.for_digit(7), Some(1));
    }

    #[test]
    fn a_digit_another_profile_holds_is_refused() {
        let mut profiles = Profiles::default();
        let mut first = sheet(DEFAULT_PROFILE, &[]);
        first.hotkey = Some(1);
        profiles.sheets.push(first);
        profiles.sheets.push(sheet("boss", &[]));
        assert!(bind_digit(&mut profiles, 1, 1).is_err());
        assert!(bind_digit(&mut profiles, 1, 0).is_err());
        assert!(bind_digit(&mut profiles, 1, 10).is_err());
        assert!(bind_digit(&mut profiles, 9, 2).is_err());
        assert!(bind_digit(&mut profiles, 1, 2).is_ok());
    }

    #[test]
    fn renaming_keeps_the_rows_and_the_binding() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[ROW_A]));
        let mut boss = sheet("boss", &[ROW_B]);
        boss.hotkey = Some(2);
        profiles.sheets.push(boss);
        rename_profile(&mut profiles, 1, "spellblade").expect("valid name");
        assert_eq!(profiles.sheets[1].name, "spellblade");
        assert_eq!(profiles.sheets[1].rows, vec![ROW_B]);
        assert_eq!(profiles.sheets[1].hotkey, Some(2));
    }

    #[test]
    fn names_are_narrow_and_unique() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[]));
        profiles.sheets.push(sheet("boss", &[]));
        assert!(rename_profile(&mut profiles, 1, "explore").is_ok());
        assert!(rename_profile(&mut profiles, 1, "default").is_err());
        assert!(rename_profile(&mut profiles, 0, "rename me").is_err());
        assert!(rename_profile(&mut profiles, 1, "").is_err());
        assert!(rename_profile(&mut profiles, 1, "a/b").is_err());
        assert!(rename_profile(&mut profiles, 1, "[x]").is_err());
        assert!(rename_profile(&mut profiles, 1, &"x".repeat(PROFILE_NAME_MAX + 1)).is_err());
        assert!(rename_profile(&mut profiles, 1, &"x".repeat(PROFILE_NAME_MAX)).is_ok());
    }

    #[test]
    fn deleting_the_active_profile_lands_on_default() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[ROW_A]));
        profiles.sheets.push(sheet("boss", &[ROW_B]));
        let fallback = delete_profile(&mut profiles, 1, "boss").expect("deletable");
        assert_eq!(fallback, DEFAULT_PROFILE);
        assert_eq!(profiles.sheets.len(), 1);

        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[]));
        profiles.sheets.push(sheet("boss", &[]));
        let kept = delete_profile(&mut profiles, 1, DEFAULT_PROFILE).expect("deletable");
        assert_eq!(kept, DEFAULT_PROFILE);
    }

    #[test]
    fn the_default_profile_is_protected() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[]));
        assert!(delete_profile(&mut profiles, 0, DEFAULT_PROFILE).is_err());
        assert!(rename_profile(&mut profiles, 0, "main").is_err());
        assert_eq!(profiles.sheets.len(), 1);
    }

    #[test]
    fn an_unknown_active_name_falls_back_to_the_default_and_says_so() {
        let mut profiles = Profiles::default();
        profiles.sheets.push(sheet(DEFAULT_PROFILE, &[]));
        profiles.sheets.push(sheet("boss", &[]));
        assert_eq!(profiles.active("boss"), Some((1, false)));
        assert_eq!(profiles.active("gone"), Some((0, true)));
        assert_eq!(Profiles::default().active("default"), None);
    }

    #[test]
    fn a_hotkey_is_refused_while_the_panel_is_open_a_cast_runs_or_the_key_repeats() {
        assert!(hotkey_allowed(false, false, 5_000, 0));
        assert!(!hotkey_allowed(true, false, 5_000, 0));
        assert!(!hotkey_allowed(false, true, 5_000, 0));
        assert!(!hotkey_allowed(false, false, 5_100, 5_000));
        assert!(hotkey_allowed(false, false, 5_000 + HOTKEY_DEBOUNCE_MS + 1, 5_000));
    }

    #[test]
    fn the_label_is_what_the_panel_shows() {
        assert_eq!(hotkey_label(HotkeyModifier::Ctrl, 2), "Ctrl+2");
        assert_eq!(parse_hotkey_modifier(" control "), Some(HotkeyModifier::Ctrl));
        assert_eq!(parse_hotkey_modifier("SUPER"), Some(HotkeyModifier::Super));
        assert_eq!(parse_hotkey_modifier("hyper"), None);
    }
}

// ---- panel key batches ------------------------------------------------------

/// Apply a frame's key events to ImGui's key table, one frame late where a tap needs it.
///
/// The panel drains its key queue once per frame, so a press shorter than a frame arrives as
/// press *and* release in the same batch. Writing only the last event tells ImGui the key was never
/// pressed — arrows looked fine because they are held, Enter did not. A release that follows its own
/// press in the same batch is therefore held back one frame, so every press is visible once.
///
/// `deferred` carries those keys to the next call; it is drained here and never grows beyond the
/// number of keys pressed in a frame.
pub fn apply_key_batch(
    keys_down: &mut [bool],
    batch: &mut Vec<(usize, bool)>,
    deferred: &mut Vec<usize>,
) {
    for key in deferred.drain(..) {
        if let Some(slot) = keys_down.get_mut(key) {
            *slot = false;
        }
    }
    // Only a press followed by its own release inside this batch loses the press; a key held across
    // frames releases immediately, so nothing pays a frame of latency for this rule.
    let mut pressed_here: Vec<usize> = Vec::with_capacity(8);
    for (key, down) in batch.drain(..) {
        let Some(slot) = keys_down.get_mut(key) else {
            continue;
        };
        if down {
            *slot = true;
            pressed_here.push(key);
        } else if pressed_here.contains(&key) {
            deferred.push(key);
        } else {
            *slot = false;
        }
    }
}

#[cfg(test)]
mod key_batch_tests {
    use super::apply_key_batch;

    const ENTER: usize = 40;

    #[test]
    fn a_tap_inside_one_batch_is_visible_once() {
        let mut down = [false; 512];
        let mut deferred = Vec::new();
        let mut batch = vec![(ENTER, true), (ENTER, false)];
        apply_key_batch(&mut down, &mut batch, &mut deferred);
        assert!(down[ENTER], "the press must be visible to ImGui");
        assert_eq!(deferred, vec![ENTER]);
        apply_key_batch(&mut down, &mut Vec::new(), &mut deferred);
        assert!(!down[ENTER], "and released on the next frame");
        assert!(deferred.is_empty());
    }

    #[test]
    fn a_held_key_passes_straight_through() {
        let mut down = [false; 512];
        let mut deferred = Vec::new();
        let mut batch = vec![(ENTER, true)];
        apply_key_batch(&mut down, &mut batch, &mut deferred);
        assert!(down[ENTER]);
        assert!(deferred.is_empty());
        let mut batch = vec![(ENTER, false)];
        apply_key_batch(&mut down, &mut batch, &mut deferred);
        assert!(!down[ENTER], "a held key releases without a frame of lag");
        assert!(deferred.is_empty());
    }

    #[test]
    fn a_release_with_no_press_does_not_defer() {
        let mut down = [false; 512];
        let mut deferred = Vec::new();
        let mut batch = vec![(ENTER, false)];
        apply_key_batch(&mut down, &mut batch, &mut deferred);
        assert!(!down[ENTER]);
        assert!(deferred.is_empty());
    }
}
