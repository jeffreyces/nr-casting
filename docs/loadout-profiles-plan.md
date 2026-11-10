# Loadout profiles — assignments only

Status: **implemented in 0.3.0** and played. Shipped in six commits (`b2289e1` feature, `6937a73`,
`98ce27c`, `a4ed3e7`, `ef630d8`, `5d6baf3` from the playtest rounds), plus the audit pass recorded at
the end of this file. Complements `docs/rebound-modifier-plan.md` (independent work; neither blocks
the other). This document is the record of what the code does, corrected where the playtest
overturned an assumption — the old "to build" wording is gone.

## What a profile is

A named **assignment sheet**: which spell each of the eight inputs casts, plus that sheet's own
`balance` and `vanilla_skips_assigned` values. Nothing else. It never touches the player's equipment,
memory slots, inventory or save: the weapons a profile applies to are whatever the player already has
equipped.

Global in content — the same list of sheets is offered for every character — but each character keeps
its own file, because the spells it can cast differ.

## Settled decisions (as built)

| # | Decision |
|---|---|
| 1 | **Display name only.** The panel title and the docs/Nexus page say **Nightreign Casting Plus**. DLL, zip, repo and `me3` profile names keep `nr_casting`, so installs, profile paths and the update path are unaffected. |
| 2 | **Grey profile block, top right**, on the title band, right-aligned, in the faded grey the disabled rows use: `Profile: travel (Ctrl+4)`. State, not a control. |
| 3 | **Hotkeys autobind.** `default` holds `Ctrl+1` from the first launch (`ensure_default_hotkey`, stamped on load); a new profile takes the lowest free digit; a digit another profile holds is refused, never stolen. Changing one is the panel's `key` combo. |
| 4 | **The active name lives in the ini** (`profile = …`); the sheets and their switch values live in the per-character slot file. |
| 5 | **Nine profiles.** `default` cannot be renamed or deleted. Deleting the active profile lands on `default`. |
| 6 | **Keyboard switching only.** No pad chord. |
| 7 | **Wheel skip unchanged** — it reads the assigned set through the cache, which the switch invalidates. |
| 8 | Edits autosave into the active profile; there is no "unsaved changes" state. |
| 9 | Profile names: printable ASCII, ≤ 24 chars, no path separators, unique. Rename allowed for non-default. |
| 10 | A weapon with no row in a profile has **no assignment** (rows greyed). Never a substituted spell. |
| 11 | Per-copy weapon overrides are stored in the sheet like today, same row format. |
| 12 | **The switches belong to the sheet, in both directions.** `save as` copies the live pair; `create new` starts at free/off; and flipping either switch while a sheet is live records it **into that sheet** (`store::record_active_flags`). Playtest 1 found only the first half existed. |
| 13 | **Every sheet holds concrete switch values.** A sheet with neither line is stamped once, on load, from the live values. "As-is" made a switch inherit whatever the previous profile published — turn both off on one sheet, switch to `default`, and `default` looked switched off without anything being written to it. Playtest 2 found this. |
| 14 | **The active name is written by a switch and nothing else.** A name missing from the file in hand falls back to `default` **in memory only** and says so: the store starts on the shared pre-character file, so a name absent there can appear in the character's file a second later. Writing the fallback lost the player's choice on every restart. Playtest 3 found this. |
| 15 | **`create new` is a fresh sheet**: no rows, balance free, wheel skip off, like a new install — and it publishes that pair when it becomes live. `save as` is the one that copies. |
| 16 | **Enter in the name box confirms**, like `ok`. |
| 17 | **The panel's mode controls follow a switch**: the `balanced` checkbox and the wheel-skip checkbox read the live values, so a hotkey switch with the panel open cannot leave the panel describing the previous sheet. |

## Files

### Slot file (`nr_casting_slots_<character>.txt`)

```
# weapon_param_id = bumper_spell, trigger_spell, y_trigger_spell, y_bumper_spell
# layout 2: a row on the full param id overrides that copy only
# format 3: [profile <name>] sections; weapon rows belong to the section above them

[profile default]
hotkey = 1
balance = free
skip_assigned = off
33090000 = -1, 4110, 4710, 4400

[profile boss]
hotkey = 2
balance = attuned
skip_assigned = on
33090000 = 4301, 4080, 4021, 4200
```

- **Migration:** a file with no sections is read as `[profile default]`, rewritten once with the
  markers, and its rows and switch values are stamped (decision 13). Nothing is lost.
- **Downgrade caveat:** an older build ignores lines it cannot parse and so reads the *union* of all
  sheets as one. It cannot switch profiles anyway; documented in the README's troubleshooting.

### ini

```
[ui]
profile = default          ; which sheet is live (rows and switches live in the slot file)
profile_modifier = Ctrl    ; Ctrl | Alt | Shift | Super, held with a digit to switch
```

## Runtime

- `store::Store` holds three things: `profiles` (the file model, ordered, last row wins), `active`
  (index), and `rows` (the live sheet as a map, which is what the cast path reads). `store_get` /
  `store_set` touch only `rows`; `save_store` folds `rows` back into `profiles.sheets[active]` before
  formatting, so the two shapes cannot disagree on disk.
- Every write goes through `commit` under the store lock: save, then bump `generation()`. The panel,
  the wheel skip and the input path's `bound_mask` cache all key on that generation, so a switch needs
  no other wiring.
- `switch_profile` publishes the sheet's switch values into the live config (the same path the panel's
  toggles take), writes the ini, and logs one line: name, row count, hotkey, both switch values.
- No game-memory read happens on a switch: `refresh_attuned` was deliberately removed from this path,
  because a switch can be triggered from the panel thread. The memorised list did not change anyway.

## Input

- `rawinput::profile_hotkey_tick`, called from the frame clock: one `GetAsyncKeyState` for the
  modifier, then the nine digits only while it is held.
- Refusals, all from `hotkey_allowed`: **a cast in flight** and **key repeat (250 ms)**. The panel
  being open is deliberately *not* a refusal — switching with the panel open is how a player checks a
  sheet (decision 17 makes the panel follow).
- Not read: the game's own keybinds. If the player bound `Ctrl+1` in Elden Ring, both act; documented.

## Panel

- Title **Nightreign Casting Plus**; the live profile right-aligned on that band in faded grey.
- Under the `balanced` toggle, its own band (same gap as the diagnostics row) holds: the profile
  dropdown (each entry labelled with its hotkey and its switch values), `key` combo, `save as`,
  `rename`, `delete`, then a rule separating the two gameplay rows from the panel's other controls.
- `create new...` is the last entry of the dropdown. `delete` asks for confirmation. `default`'s
  rename/delete render as disabled text.
- Any refusal or store error is rendered on the row (`profile_msg`), not just logged.

## Tests (17 pure tests in `nr_casting_core`)

| Behaviour | Test |
|---|---|
| old file with no sections parses as `default`, every row intact | `a_file_without_sections_is_the_default_profile` |
| sections do not leak rows between each other | `sections_do_not_leak_rows_into_each_other` |
| write → read round-trips rows, switches and bindings | `a_round_trip_keeps_rows_flags_and_bindings` |
| an unstated switch is left to the caller (then stamped by the store) | `flags_a_sheet_does_not_state_are_left_to_the_ini` |
| `default` claims digit 1 when nothing has one | `the_default_profile_claims_the_first_digit_when_nothing_has_one` |
| an existing binding is left alone | `a_hotkey_already_in_the_file_is_left_alone` |
| a new profile takes the lowest free digit | `a_new_profile_takes_the_lowest_free_digit` |
| nine profiles exhaust the digits | `nine_profiles_exhaust_the_digits` |
| an explicit binding survives a reload | `an_explicit_binding_survives_a_reload` |
| a taken digit is refused; out-of-range refused | `a_digit_another_profile_holds_is_refused` |
| rename keeps rows and binding | `renaming_keeps_the_rows_and_the_binding` |
| names are narrow, unique, and `default` is protected | `names_are_narrow_and_unique`, `the_default_profile_is_protected` |
| deleting the active profile lands on `default` | `deleting_the_active_profile_lands_on_default` |
| an unknown active name falls back and reports it | `an_unknown_active_name_falls_back_to_the_default_and_says_so` |
| hotkey refused on cast-in-flight and on repeat | `a_hotkey_is_refused_while_the_panel_is_open_a_cast_runs_or_the_key_repeats` |
| the label the panel shows is `Ctrl+2` | `the_label_is_what_the_panel_shows` |

## Principle alignment — the audit, run after the playtest

Findings from reviewing the shipped diff (the same rubric as any other change, including the code
written an hour earlier):

| Principle | Finding | Action |
|---|---|---|
| DRY / KISS | Three `Sheet { … }` literals in `store.rs` (save-as, create-new) duplicating the field list | **Fixed:** `Sheet::new` / `Sheet::with_switches` in the core crate; the store uses them. |
| KISS / Documentation | `save_profile_as` logged through a nested `format!` inside a `format!` | **Fixed:** one format string. |
| Documentation / Robustness | The diagnostics report said nothing about which sheet was live, although the plan said it would | **Fixed:** `profile=active (N weapon rows, M profiles in this file)` — sizes only, because a profile name is text the player typed and the report is pasted into public issues. |
| Documentation | The log line `slots: character … -> <path> / profile <name>` was swallowed after `<path>`: the redactor treats a bare `/…` run as another path | **Fixed:** separator is `|`. The redactor itself is left as it is (see below). |
| KISS | `reload_store` had three booleans feeding one rewrite condition | Left as is: each is a distinct log line, and folding them would hide *why* the file was rewritten. |
| Parameter count | `profile_row` takes six values and returns an action | Left as is: `hand_column` already takes eight in this file, and a struct for one call site is more code than it saves. |
| Robustness (accepted trade) | A hotkey switch writes the slot file **and** the ini on the frame clock | Left as is, and named in Risks: it is one small write on a deliberate keypress, and the perf log would show it as a lost frame if it ever mattered. The alternative (queue to the panel thread) is machinery for a case we have not measured. |
| Separation of Concerns | `store.rs` is now ~700 lines (assignments + profiles) | Left as is: rows and profiles are one concern — the rows *belong* to a sheet — and the split would put the "fold the map back before formatting" invariant across two files. |

## Manual test script (and what the playtest actually found)

1. Fresh install → `Profile: default (Ctrl+1)` and the panel opens as before. *(passed; found default
   had no hotkey, so the first `save as` took Ctrl+1 — decision 3)*
2. `save as` → autobinds the next digit; the block and the list agree. *(passed)*
3. Edit rows, switch away and back → the rows follow. *(passed)*
4. Flip the switches on one sheet, switch away and back → they stay. *(failed first: only the ini was
   written — decision 12)*
5. Switch to `default` after flipping them off elsewhere → default must not look flipped.
   *(failed first: it inherited — decision 13)*
6. `create new` → fresh sheet: no rows, free, skip off. *(failed first: it inherited — decision 15)*
7. Switch with the panel open; press Enter in the name box. *(both were requests: decisions 16, 17)*
8. Restart the game → the last sheet is live. *(failed first: the fallback overwrote the name —
   decision 14)*
9. Delete the live sheet → lands on `default`. *(passed)*
10. Wheel skip follows the sheet's own value. *(passed)*

## Risks

- **Downgrade merges sheets** — documented, not prevented.
- **Ini churn:** every switch persists the ini (one small write per keypress).
- **A switch mid-combat** changes what the next press casts. Deferred only while a cast is in flight.
- **Profile names are user text**: they stay in the slot file and the local log; the report carries
  counts only.
- **Two holders of "the same" toggle:** the ini keys and the live sheet. Every path that changes one
  writes the other, so they cannot disagree for longer than one publish.

## Left for later

- `docs/rebound-modifier-plan.md` still waits on the reporter's remap list; it is unrelated to this
  feature.
- Publishing 0.3.0 (tag + CI + asset check) has not been done.
