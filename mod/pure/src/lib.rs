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

/// Spells per catalyst: bumper, trigger, the Y chord on the trigger, and the Y chord on the
/// bumper. The fourth only exists when the Y+bumper chords are ours to take (no Recluse mod, or
/// not on a Recluse character); otherwise the game and Recluse keep them.
pub const SLOTS: usize = 4;

/// One weapon's four spell ids, `-1` for unbound.
pub type Category = [i32; SLOTS];

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
/// Order is the contract and matches the DLL: the ash-of-war escape wins over everything, and
/// the Y chords are
/// checked before the plain inputs because under Y the trigger means the third slot and the
/// bumper the fourth.
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

/// The mouse mapping: click plus modifier to (right hand, slot, name).
///
/// The mouse mapping: click plus modifiers to (right hand, slot, name).
///
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
}
