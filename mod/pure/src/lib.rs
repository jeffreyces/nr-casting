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

/// Render a slot file: sorted by weapon id, with the header explaining the format.
pub fn format_slots(entries: &[(i32, Category)]) -> String {
    let mut out = String::from(
        "# weapon_param_id = bumper_spell, trigger_spell, y_trigger_spell, y_bumper_spell\n\
         # y_trigger_spell is cast by holding Y and pulling the trigger: Y+R2 (right weapon)\n\
         # and Y+L2 (left weapon). y_bumper_spell is cast by holding Y and pressing the bumper:\n\
         # Y+R1 and Y+L1, which are only ours when the Recluse mod is absent or the character\n\
         # is not a Recluse. Two- and three-entry lines from older versions are still accepted.\n",
    );
    let mut sorted: Vec<&(i32, Category)> = entries.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    for (id, spells) in sorted {
        out.push_str(&format!(
            "{id} = {}, {}, {}, {}\n",
            spells[0], spells[1], spells[2], spells[3]
        ));
    }
    out
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
pub fn replace_ignore_case(text: &str, needle: &str, replacement: &str) -> String {
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
pub fn redact_character_slug(text: &str) -> String {
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

/// Redact the account segment of every user path, whatever the environment says.
///
/// `USERPROFILE` and `HOME` can be missing, or point somewhere that is not the path the log
/// carries — under Proton, `USERPROFILE` is the Wine account while the paths are
/// `/var/home/<user>` and `Z:\\var\\home\\<user>`. The *shape* of the path is the reliable signal, so
/// this works the same on Windows (`C:\\Users\\<name>\\`) and on Linux (`/home/<name>/`,
/// `/var/home/<name>/`).
pub fn redact_user_paths(text: &str) -> String {
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
                let (before, _) = line.split_at(at);
                let newline = if line.ends_with('\n') { "\n" } else { "" };
                format!("{before}<path>{newline}")
            }
            None => line.to_string(),
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let text = format_slots(&entries);
        assert!(text.contains("10 = 4, 5, -1"));
        assert!(text.find("10 =").unwrap() < text.find("20 =").unwrap());
        assert_eq!(parse_slots(&text), vec![(10, [4, 5, -1, -1]), (20, [1, 2, 3, -1])]);
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
        let text = format_slots(&[(33090010, [1, 2, 3, 4])]);
        assert!(text.contains("33090010 = 1, 2, 3, 4"));
        assert_eq!(parse_slots(&text), vec![(33090010, [1, 2, 3, 4])]);
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
            0x1_4012_3456_7,
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



    #[test]
    fn redaction_works_by_path_shape_on_both_platforms() {
        // Windows: the account segment goes, whatever the environment claims.
        let out = redact_user_paths(r"C:\Users\<name>\AppData\Local\garyttierney\me3\config");
        assert!(!out.contains("<name>"), "{out}");
        assert!(out.contains(r"C:\Users\%USER%\AppData"), "{out}");
        // Linux and Proton's drive-letter spelling of it.
        for text in [
            "/home/<name>/.local/share/Steam",
            "/var/home/<name>/Documents/Github",
            r"Z:\var\home\<name>\Documents\Github",
        ] {
            let out = redact_user_paths(text);
            assert!(!out.contains("<name>"), "{out} -> {out}");
            assert!(out.contains("%USER%"), "{out}");
        }
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
        assert!(!out.contains("<name>"), "{out}");
        // The paths are gone entirely now; neither placeholder survives either, because replacing
        // the account segment alone left `%USERPROFILE%\\AppData\\...` in the file, which is still
        // the layout of somebody's disk. A path runs to the end of its line, so three paths on one
        // line collapse into one placeholder - the alternative is guessing where a Windows path
        // ends when its directory names contain spaces, and guessing short leaves half a path.
        assert_eq!(out, "log at <path>", "{out}");
    }


    #[test]
    fn redact_paths_takes_the_whole_path_and_nothing_else() {
        // His lines, verbatim from a report: the account segment was already replaced, and the
        // layout of his disk was still in the file.
        let line = "slots: character <character> (id 0) -> %USERPROFILE%\\AppData\\Local\\garyttierney\\me3\\config\\profiles\\eldenring-mods\\nr-casting-v0.2.7\\nr_casting\\nr_casting_slots_<character>-399e89de.txt";
        assert_eq!(
            redact_paths(line),
            "slots: character <character> (id 0) -> <path>"
        );
        // Windows paths contain spaces, so a whitespace split loses half of them.
        let line = "XInputGetState: caller stack includes C:\\SteamLibrary\\steamapps\\common\\ELDEN RING\\Game\\eldenring.exe";
        assert_eq!(
            redact_paths(line),
            "XInputGetState: caller stack includes <path>"
        );
        // The same line under Proton names the host's home, and the prefix path.
        assert_eq!(
            redact_paths("files in: /var/home/bob/.local/share/Steam/steamapps/compatdata/1245620/pfx"),
            "files in: <path>"
        );
        assert_eq!(
            redact_paths("STEAM_COMPAT_DATA_PATH=Z:\\var\\home\\bob\\.local\\share\\Steam"),
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
