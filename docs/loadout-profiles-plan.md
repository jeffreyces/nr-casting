# Loadout profiles — assignments only

Status: **settled design, not implemented.** Target version: **0.3.0**. Complements `docs/rebound-modifier-plan.md` (independent
work; neither blocks the other).

## What a profile is

A named **assignment sheet**: which spell each of the eight inputs casts, plus that sheet's
`balance` and `skip assigned in wheel` values. Nothing else. It does not touch the player's
equipment, memory slots, inventory or save: the weapons a profile applies to are whatever the player
already has equipped, exactly as today.

Global across characters: one set of sheets, so switching to an alt does not lose the set, and the
class port's own Y-chord rows stay the same for every character in that sheet.

## Settled decisions

| # | Decision |
|---|---|
| 1 | **Display name only.** The panel title and the docs/Nexus page say **Nightreign Casting Plus**. The DLL, zip, repo and `me3` profile names keep `nr_casting` — renaming those breaks every existing install's profile path, the update path, and the settings/slot file prefixes. |
| 2 | **Grey profile block, top right**, on the same band as the title, right-aligned, in the same faded grey as the "not applicable" rows. Reads `Profile: default (Ctrl+1)`; the parenthesised binding is the profile's own. |
| 3 | **Hotkeys autobind.** `default` is `Ctrl+1`; a new profile takes the lowest free digit. Only an explicit user change alters a profile's binding, and only then does the panel text change. |
| 4 | **Active profile lives in the ini** (`profile = default`); the sheets live in the existing per-character slot file, because the ini is settings and the slot file is assignment data. |
| 5 | **Nine profiles.** `default` always exists and cannot be deleted or renamed. Deleting the active profile switches to `default` first. |
| 6 | **Keyboard switching only.** No pad chord for this feature. |
| 7 | **Wheel skip unchanged** — it already reads the assigned set through the cache, which the switch invalidates. |
| 8 | Edits autosave into the active profile (today's behaviour); `save as` copies the current sheet to a new name; there is no "unsaved changes" state. |
| 9 | Profile names: ASCII printable, ≤ 24 chars, no path separators. Rename allowed for non-default profiles. |
| 10 | A weapon with no row in a profile has **no assignment** (rows greyed). Never a substituted spell. |
| 11 | Per-copy weapon overrides are stored in the sheet like today, same row format. |
| 13 | **Every sheet holds concrete switch values.** A sheet with no `balance`/`skip_assigned` line is stamped with the live values once, on load. "As-is" made a switch inherit whatever the previous profile published - turn both switches off on one, switch to `default`, and `default` looked switched off too without anything having been written to it. `create new` starts at balance **free** and wheel skip **off**, i.e. a fresh install, and publishes that pair when it becomes live. |
| 12 | **The switches belong to the sheet, in both directions.** `save as` copies the live `balance` and wheel-skip values into the new sheet, `create new` starts from the live ones, and flipping either switch while a sheet is live records it **into that sheet** (`store::record_active_flags`). Without the last half the ini and the sheet drifted, and switching away and back reverted the player's change - found in the first playtest. |

## Files

### Slot file (`nr_casting_slots_<character>.txt`)

```
# format = 2
# weapon_param_id = bumper, trigger, y_trigger, y_bumper

[profile default]
balance = attuned
skip_assigned = on
33090010 = 4431, 4430, -1, 4400
33240000 = 4381, -1, -1, 4710

[profile boss]
balance = free
skip_assigned = off
33090010 = 4400, 4430, 4431, 4710
```

- **Migration:** a file without `# format =` has no sections and is read as `[profile default]`;
  every row in it keeps working, and the first save writes the marker and the section header.
- **Downgrade caveat:** an older build of the mod ignores lines it cannot parse, so it would read the
  *union* of all profiles as one sheet. Acceptable (the old build cannot switch profiles anyway), but
  it must be written down in the README's troubleshooting rather than discovered.

### ini

```
[ui]
; Which profile is live. Only the name lives here; the rows and the per-profile
; balance/skip flags live in the slot file beside the DLL.
profile = default
; Modifier for the profile hotkeys (Ctrl + digit). Ctrl, Alt, Shift or Super.
profile_modifier = Ctrl
```

## Runtime

- `store.rs` grows a `Sheet { rows, balance, skip }` and a small `Profiles` map. `store_get` /
  `store_set` read and write the **active** sheet only; nothing else in the mod knows profiles
  exist.
- Switching a profile bumps `generation()`, which the panel, the wheel skip and the assigned-set
  cache already key on. So the skip and the panel follow the switch with no extra wiring.
- Switching publishes the sheet's `balance` and `skip_assigned` into the live config **and** persists
  the ini, the same path the panel's own toggles take. A profile therefore reproduces the whole
  gameplay setup, not just the rows.
- Refusals: a switch is ignored while the panel is open or a cast is in flight, and a second press
  within 250 ms is ignored (key repeat). Both log once per session, not per press.

## Input

- `rawinput.rs` already polls `GetAsyncKeyState` for Ctrl/Shift/Alt/Super and has the world-vs-menu
  guard the bind gesture uses. Profile switching is one more branch on that same poll: modifier held
  + digit `1..9` → switch to the profile whose binding is that digit.
- The modifier is `profile_modifier` in the ini (default `Ctrl`), live-reloadable.
- Not read: the game's own keybinds. If a player has assigned `Ctrl+1` to something in Elden Ring,
  both act. Documented in the README, not guessed around.

## Panel

- Title: **Nightreign Casting Plus**.
- Top right, same band as the title: `Profile: boss (Ctrl+2)` in the faded grey already used for
  disabled rows.
- Below the `balanced` checkbox, a profile row: `[ profile dropdown ]  save as  rename  delete`, and
  **a separator line under that row**, so the two gameplay-specific rows (`balanced`, profile) are
  visually separated from the other controls.
- The dropdown lists sheets in file order with their bindings; `save as` prompts for a name in the
  panel (ImGui input text, no native dialog).

## Tests to write first (`nr_casting_pure`, red before any wiring)

| Behaviour | Red before |
|---|---|
| a file without `# format` parses as `[profile default]` with every row intact | yes |
| two sections parse into two sheets, no rows leak between them | yes |
| write → read round-trips rows and flags | yes |
| missing `balance` / `skip_assigned` in a sheet inherits the ini values | yes |
| a new profile autobinds the lowest free digit; the tenth profile gets none | yes |
| an explicit binding wins and survives a reload | yes |
| a binding already taken by another profile is refused, not stolen | yes |
| rename keeps the rows and the binding | yes |
| deleting the active profile falls back to `default` | yes |
| `default` cannot be deleted or renamed | yes |
| active name from the ini that no longer exists falls back to `default` | yes |
| hotkey dispatch: ignored while the panel is open, while a cast is in flight, and within 250 ms | yes |
| switching bumps the generation, so the assigned set and the wheel skip see the new sheet | yes |

## Implementation order (one commit each)

1. Pure crate: sheets, parse/write with the format marker, autobind, active resolution, hotkey
   dispatch decision. Tests first.
2. `store.rs`: sections, migration, generation bump, `store_get`/`store_set` against the active sheet.
3. Config: `profile`, `profile_modifier`, publishing a sheet's flags.
4. `rawinput.rs`: the hotkey branch with its guards.
5. Panel: title, grey block, profile row + separator, `save as` / `rename` / `delete`.
6. Docs: README, NEXUS, ini template, release notes; troubleshooting note about downgrades.
7. Version bump to **`0.3.0`** once the user has played it (new feature + a file-format change).

## Principle alignment

- **SRP:** profiles are a data/selection concern; the cast path keeps asking `store_get`/`store_set`
  and learns nothing new.
- **DRY:** one sheet type, one parser (the existing `id = ...` parser extended to sections), one
  binding allocator shared by creation and reload.
- **Open/closed:** a new per-profile setting is a field on `Sheet`, not a new branch in the cast path.
- **Robustness:** every refusal (panel open, cast in flight, key repeat, taken digit, missing active
  profile) has a defined behaviour and a log line; a malformed sheet is skipped, not fatal.
- **Testability:** the whole feature is pure data plus one dispatch decision; only the ini write and
  the ImGui widgets are untestable on the dev host.
- **YAGNI:** no equipment swapping, no controller chord, no per-profile UI scale or geometry, no
  cloud/sharing of sheets.

Deliberately left alone: the slot file's per-character scoping (profiles are global, the file stays
per character), and the existing `balance`/`vanilla_skips_assigned` ini keys (they remain the source
of truth for the *live* values; a profile switch just publishes into them).

## Manual test script

1. Fresh install, no slot file: panel opens on `Profile: default (Ctrl+1)`, assignments behave as
   today.
2. Assign two spells, press `Ctrl+2`-free `save as` "boss" → new sheet autobinds `Ctrl+2`, panel
   block shows `Profile: boss (Ctrl+2)`.
3. Change one row in "boss", press `Ctrl+1` → the original rows come back; press `Ctrl+2` → the edit is
   there.
4. Set "boss" to `balanced` off and skip on, switch to `default` and back → both flags follow.
5. `Ctrl+2` while the panel is open → nothing happens; while a cast is being pressed → deferred, one
   line in the log.
6. Delete "boss" while it is active → lands on `default`, the grey block says so.
7. Restart the game → the last active profile is live, bindings unchanged.
8. Hand-edit the ini `profile = nonexistent` → falls back to `default` with one log line.
9. Wheel skip, with the skip on in one profile and off in the other → follows the switch.

## Risks

- **Downgrade merges sheets** (see Files) — documented, not prevented.
- **Ini churn:** every switch persists the ini. That is one small write per hotkey press; if it ever
  shows up in the perf log, the switch can publish the live values without touching the file.
- **A switch mid-combat** changes what the next press casts. Deferred while a cast is in flight, and
  the grey block always shows which sheet is live.
- **Profile names are user text** — they stay inside the slot file and the local log; the diagnostics
  report prints `profile=active (+count)` rather than the name, so a pasted report cannot carry
  whatever the player typed.
- **Two things now hold "the same" toggle:** the ini keys and the active sheet. The switch path writes
  both, so they cannot disagree for more than the duration of one publish.
