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

/// Spells per catalyst: bumper, trigger, and the Y chord on the trigger.
pub const SLOTS: usize = 3;

/// One weapon's three spell ids, `-1` for unbound.
pub type Category = [i32; SLOTS];

/// XInput button bits used by the routing rules.
pub const BTN_LEFT_SHOULDER: u16 = 0x0100;
pub const BTN_RIGHT_SHOULDER: u16 = 0x0200;
pub const BTN_RIGHT_THUMB: u16 = 0x0080;
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
    /// R3 + L2: give the trigger back to the game so the ash of war fires.
    AshOfWar,
    /// The bind modifier plus an input: assign the wheel's selection instead of casting.
    Capture {
        right_hand: bool,
        slot: usize,
        label: &'static str,
    },
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
/// Order is the contract and matches the DLL: the ash-of-war escape wins over everything,
/// binding beats casting (holding the modifier must never also cast), and the Y chord is checked
/// before the plain trigger because under Y the trigger means the third slot.
pub fn route(held: &Held, modifier: u16) -> Route {
    if held.buttons & BTN_RIGHT_THUMB != 0 && held.left_pull {
        return Route::AshOfWar;
    }
    if !held.any() {
        return Route::Idle;
    }
    if modifier != 0 && held.buttons & modifier != 0 {
        let (right_hand, slot, label) = if held.right_pull {
            (true, 1, "R2")
        } else if held.right_bump {
            (true, 0, "R1")
        } else if held.left_pull {
            (false, 1, "L2")
        } else {
            (false, 0, "L1")
        };
        return Route::Capture {
            right_hand,
            slot,
            label,
        };
    }
    if held.buttons & BTN_Y != 0 && (held.right_pull || held.left_pull) {
        return if held.right_pull {
            Route::Chord {
                right_hand: true,
                slot: 2,
                label: "Y+R2",
            }
        } else {
            Route::Chord {
                right_hand: false,
                slot: 2,
                label: "Y+L2",
            }
        };
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
/// Same six slots as the pad. Ctrl is tested before shift so a held Ctrl wins, and a click with
/// both buttons down is not a spell input at all.
pub fn mouse_route(
    left: bool,
    right: bool,
    shift: bool,
    ctrl: bool,
) -> Option<(bool, usize, &'static str)> {
    match (left, right) {
        (true, false) if ctrl => Some((true, 2, "ctrl + LMB")),
        (true, false) if shift => Some((true, 1, "shift + LMB")),
        (true, false) => Some((true, 0, "LMB")),
        (false, true) if ctrl => Some((false, 2, "ctrl + RMB")),
        (false, true) if shift => Some((false, 1, "shift + RMB")),
        (false, true) => Some((false, 0, "RMB")),
        _ => None,
    }
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
                [ids[0], ids[1], ids.get(2).copied().unwrap_or(-1)],
            ));
        }
    }
    out
}

/// Render a slot file: sorted by weapon id, with the header explaining the format.
pub fn format_slots(entries: &[(i32, Category)]) -> String {
    let mut out = String::from(
        "# weapon_param_id = bumper_spell, trigger_spell, y_chord_spell\n\
         # y_chord_spell is cast by holding Y and pulling the trigger: Y+R2 (right weapon)\n\
         # and Y+L2 (left weapon). Two-entry lines from older versions are still accepted.\n",
    );
    let mut sorted: Vec<&(i32, Category)> = entries.iter().collect();
    sorted.sort_by_key(|(id, _)| *id);
    for (id, spells) in sorted {
        out.push_str(&format!("{id} = {}, {}, {}\n", spells[0], spells[1], spells[2]));
    }
    out
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

    const MODIFIER: u16 = 0x0040; // L3

    #[test]
    fn parses_three_value_lines() {
        let parsed = parse_slots("33090010 = 4431, 4060, 4710\n");
        assert_eq!(parsed, vec![(33090010, [4431, 4060, 4710])]);
    }

    #[test]
    fn accepts_two_value_lines_from_older_versions() {
        let parsed = parse_slots("33240009 = 4080, 4040\n");
        assert_eq!(parsed, vec![(33240009, [4080, 4040, -1])]);
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
        assert_eq!(parsed, vec![(4101, [-1, 4000, -1])]);
    }

    #[test]
    fn trailing_comments_are_stripped() {
        assert_eq!(parse_slots("7 = 1, 2, 3  # my staff\n"), vec![(7, [1, 2, 3])]);
    }

    #[test]
    fn duplicate_ids_are_returned_in_file_order() {
        let parsed = parse_slots("7 = 1, 2, 3\n7 = 4, 5, 6\n");
        assert_eq!(parsed, vec![(7, [1, 2, 3]), (7, [4, 5, 6])]);
    }

    #[test]
    fn format_sorts_and_round_trips() {
        let entries = vec![(20, [1, 2, 3]), (10, [4, 5, -1])];
        let text = format_slots(&entries);
        assert!(text.contains("10 = 4, 5, -1"));
        assert!(text.find("10 =").unwrap() < text.find("20 =").unwrap());
        assert_eq!(parse_slots(&text), vec![(10, [4, 5, -1]), (20, [1, 2, 3])]);
    }

    #[test]
    fn idle_when_nothing_is_held() {
        assert_eq!(route(&held(0, false, false, false, false), MODIFIER), Route::Idle);
    }

    #[test]
    fn plain_inputs_cast_their_own_slot() {
        assert_eq!(
            route(&held(0, true, false, false, false), MODIFIER),
            Route::Cast { right_hand: true, slot: 1, label: "R2" }
        );
        assert_eq!(
            route(&held(BTN_RIGHT_SHOULDER, false, false, true, false), MODIFIER),
            Route::Cast { right_hand: true, slot: 0, label: "R1" }
        );
        assert_eq!(
            route(&held(0, false, true, false, false), MODIFIER),
            Route::Cast { right_hand: false, slot: 1, label: "L2" }
        );
        assert_eq!(
            route(&held(BTN_LEFT_SHOULDER, false, false, false, true), MODIFIER),
            Route::Cast { right_hand: false, slot: 0, label: "L1" }
        );
    }

    #[test]
    fn ash_of_war_wins_over_everything() {
        let all = held(BTN_RIGHT_THUMB | BTN_Y | MODIFIER, true, true, true, true);
        assert_eq!(route(&all, MODIFIER), Route::AshOfWar);
    }

    #[test]
    fn modifier_binds_instead_of_casting() {
        let held = held(MODIFIER | BTN_Y, true, false, false, false);
        assert_eq!(
            route(&held, MODIFIER),
            Route::Capture { right_hand: true, slot: 1, label: "R2" }
        );
    }

    #[test]
    fn y_trigger_is_the_third_slot() {
        assert_eq!(
            route(&held(BTN_Y, true, false, false, false), MODIFIER),
            Route::Chord { right_hand: true, slot: 2, label: "Y+R2" }
        );
        assert_eq!(
            route(&held(BTN_Y, false, true, false, false), MODIFIER),
            Route::Chord { right_hand: false, slot: 2, label: "Y+L2" }
        );
    }

    #[test]
    fn y_without_a_trigger_is_not_a_chord() {
        // Y on its own is two-handing; it must not be swallowed.
        assert_eq!(route(&held(BTN_Y, false, false, false, false), MODIFIER), Route::Idle);
    }

    #[test]
    fn mouse_mapping_matches_the_pad() {
        assert_eq!(mouse_route(true, false, false, false), Some((true, 0, "LMB")));
        assert_eq!(mouse_route(false, true, false, false), Some((false, 0, "RMB")));
        assert_eq!(mouse_route(true, false, true, false), Some((true, 1, "shift + LMB")));
        assert_eq!(mouse_route(false, true, true, false), Some((false, 1, "shift + RMB")));
        assert_eq!(mouse_route(true, false, false, true), Some((true, 2, "ctrl + LMB")));
        assert_eq!(mouse_route(false, true, false, true), Some((false, 2, "ctrl + RMB")));
    }

    #[test]
    fn ctrl_wins_over_shift_and_two_buttons_are_not_a_spell() {
        assert_eq!(mouse_route(true, false, true, true), Some((true, 2, "ctrl + LMB")));
        assert_eq!(mouse_route(true, true, false, false), None);
        assert_eq!(mouse_route(false, false, false, false), None);
    }
}
