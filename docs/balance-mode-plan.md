# Balanced mode — implementation plan

Status: agreed design, not started. Target: the version after 0.2.19.

## What it does

Vanilla caps a caster's spell library through memory slots (up to 12). The mod adds four
spell inputs per catalyst and stores those assignments per weapon, so the real ceiling today is
four spells times every catalyst owned — capacity that scales with inventory instead of with
memory stones. Balanced mode makes the mod's assignments a way to _use_ your attuned spells
well, not a way to bypass attunement.

`balance = free | attuned`, default `free`. In `attuned` mode only spells in the character's
memorised set can be assigned. In `free` mode nothing about today's behaviour changes.

## Decisions that are settled (do not re-litigate in review)

- **Gate on the library, not on the count.** There is no assignment cap. Duplicates across
  catalysts are harmless because they cannot add a distinct spell.
- **Grey, never clear.** An assignment whose spell is no longer attuned stays in the file and
  stays visible, with its reason. Re-attuning the spell re-arms the row automatically.
- **Stale behaves like unassigned at cast time.** `cast_slot` returns false, the input report is
  not rewritten, and vanilla acts (R2 swings, Y+R2 two-hands). No dead buttons, no swallowed
  presses, no "I pressed it and nothing happened".
- **No automatic substitution.** Offering a same-school replacement is a button the player
  presses, never something the panel does for them.
- **Three numbers, each naming its scope:** `memory slots 9 of 12 used` · `attuned 9` ·
  `this staff 4 assigned`.
- **Keying:** base id is the default for the weapon type; the full param id is a per-copy
  override. Lookup tries full, then base. Upgrading a weapon therefore keeps its layout, and two
  copies at different upgrade levels can hold different sets. Identical copies still share —
  that is a hard ceiling, and nobody should promise past it.
- **Two interfaces, one decision.** The pad path and the mouse/keyboard path may keep their own
  plumbing; the "which spell does this input mean" decision lives in exactly one function.
- **`show_all` keeps its meaning** (include spells the character cannot cast yet) and has no
  interaction with the gate. In balanced mode unattuned entries are marked `(not attuned)` and
  selecting one is refused with `attune it in the game's spell menu first`.
- **Cast-time membership is authoritative.** The panel's cached set only draws the UI; it never
  decides whether a cast happens.
- **The attuned column exists only in balanced mode.** In `free` mode the panel renders exactly as
  it does today — no left column, no count line, no state markers — because a free-mode user has
  nothing to act on and an extra column is just noise in a window that has to stay readable at
  TV distance. The column, the count and the markers all appear together when the toggle goes on
  and disappear together when it goes off.

## Tests first (phase 1 — write these red, before touching the mod)

All of this is policy, so it belongs in `mod/pure/src/lib.rs` where `cargo test -p
nr_casting_core` runs on the development host. Expected first run: every new test red, one per
row below. A test that passes before its implementation is testing nothing.

| #   | Function                                                                            | Cases                                                                                                                                           |
| --- | ----------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| 1   | `parse_balance(&str) -> Option<Balance>`                                            | `free`, `attuned`, `ATTUNED`, `on`/`off` rejected with a logged default, unknown string → None                                                  |
| 2   | `classify(assigned: i32, attuned: &[i32], school: School) -> RowState`              | empty row; attuned+right school; attuned+wrong school; not attuned; spell id 0                                                                  |
| 3   | `counts(rows: &[RowState]) -> Counts`                                               | 4 attuned of 4; mixed stale/attuned; all stale; empty catalyst                                                                                  |
| 4   | `resolve_row(base: i32, full: i32, rows: &[(i32, [i32; 4])]) -> Option<[i32; 4]>`   | exact full-id override wins; unknown full id falls back to base; neither → None; base row present and full row absent (the upgrade case)        |
| 5   | `balance_line(...) -> String`                                                       | exact text for the report: on/off, attuned/slots, assigned, stale — five numbers in one string, so it is asserted verbatim                      |
| 6   | `balance_notice(rows: &[RowState], attuned_is_empty: bool) -> Option<&'static str>` | the two refusal strings (`attune it in the game's spell menu first`, `no attuned sorcery left for this staff`) and None when everything is fine |

Run `cargo test -p nr_casting_core` and record the red count. It must equal the number of cases,
not the number of test functions.

## Implementation order (phase 2 — one commit per step, compile and test after each)

1. **Extract the decision, behaviour-preserving.** Move "which spell does this input mean" out of
   `bound_spell` into one function that both the pad path and `rawinput` call, taking
   (weapon id, hand, input) and returning `Option<i32>`. No new behaviour; the 67 existing tests
   stay green and a manual cast still works. This is the refactor that makes the gate possible in
   one place — and it must land before the gate, not with it.
2. **Keying with fallback.** Store rows on the full param id, read full-then-base, and add
   `reset to default` (delete the override). Existing base-keyed files become defaults and keep
   working; no migration. Test #4 goes green here.
3. **`attuned_spells()` and `spell_is_attuned()` in `game.rs`.** Read the memorised entries from
   the `EquipMagicData` the cast path already touches; expose a membership test. Nothing on the
   poll path, nothing cached across a frame.
4. **The gate in the cast path**, behind `balance == attuned`: stale → return false, exactly like
   unassigned. Test #2's classification drives it.
5. **Config plumbing.** `balance` key: parse (test #1), default `free`, save from the panel,
   documented in the ini template. Live reload already pushes cached flags — extend it if the
   value is cached anywhere.
6. **Panel.** The attuned column and everything that belongs to it are conditional on the toggle:
   left column of attuned spells, per-row state marker and reason, refusal notices (test #6) and
   the count line (test #3, #5) appear only while balanced mode is on. In free mode the panel is
   byte-for-byte today's layout. The toggle itself sits next to the UI scale and is always
   visible; `show_all` marked entries apply only in balanced mode. Text only, ASCII, no
   colour-only signals. The conditional is one branch on the panel's layout, not a scattering of
   `if balanced` through the rows.
7. **Report.** One line in the report and one in the log when the toggle changes:
   `balance=attuned attuned=9/12 assigned=8 reachable=4 stale=1`.
8. **Docs.** `NEXUS.md`, `README.md` (configuration + reporting sections), ini template comment.

## Twelve-principles alignment

- **KISS** — three row states, one toggle, no budget arithmetic. The gate is a membership test.
- **DRY** — one resolver (step 1), one classifier, one counter, all in the pure crate. The panel
  and the cast path ask the same question of the same function.
- **SOLID-S** — policy in `nr_casting_core`; game reads in `game.rs`; drawing in `window.rs` /
  `panel_*.rs`. Each step above touches one of those.
- **SOLID-O** — `RowState` as an enum with a label table, so a fourth state (say "attuned but the
  catalyst is broken") is a variant plus a string, not an edit to every call site.
- **YAGNI** — no inventory-instance keying, no auto-fill, no HUD changes, no per-character
  overrides beyond the existing file. The reviewer's second request (grace-menu integration) is
  explicitly out of scope.
- **Separation of concerns** — the adapter that reads `EquipMagicData` is one function, so the
  tests never need a game.
- **Refactoring** — step 1 is a pure move, verified by the existing suite before any behaviour
  changes.
- **Documentation** — comments state the _why_ for the three decisions most likely to be
  "simplified" later: grey-not-clear, base-then-full lookup, cast-time authority.
- **Robustness** — nothing is deleted by the toggle in either direction; every refusal has a
  message; a memorised list that cannot be read means "cannot verify", handled as stale rather
  than as permission.
- **Testability** — every rule above is a pure function with a red-first test; the untested seam
  is a single adapter.
- **Abstraction** — `AttunedSet` newtype with `contains()`, so the cast path does not learn the
  shape of the memorised list.
- **Law of Demeter** — the panel hands ids to the policy and receives states back; it does not
  reach into game structs. Composition over inheritance is not applicable here.

Left alone, with the reasoning (so review does not re-raise them): the two interface paths
keeping separate plumbing (they have different threading and packet shapes); `config::load()`
reach-through (one atomic load of a process-wide pointer); and the existing per-character file
format (a rewrite buys nothing and risks existing layouts).

## Verification (phase 3)

Automated: `cargo test -p nr_casting_core` (all green, count recorded),
`cargo xwin check --target x86_64-pc-windows-msvc` clean with no warnings, `./tools/package.sh`.

Manual, with a real character (the user's test list):

1. **Free mode is untouched** — assign anything, cast, swap weapons. Nothing new on screen: no
   attuned column, no count line, no greyed rows. This is checked against the 0.2.19 panel, not
   from memory.
2. **Toggle balanced** — the attuned column and the count line appear in the same refresh; rows
   that were fine stay fine. Toggle back off and the panel returns to exactly today's layout with
   every assignment still live.
3. **Un-attune at a grace** — the row greys with `not attuned`; pressing that input passes
   through (R2 swings rather than doing nothing); re-attune and the row re-arms on its own.
4. **Moon of Nokstella** — unequip it mid-fight with rows depending on those two slots; they grey
   immediately and the press passes through. This is the case no grace visit explains.
5. **Copy with an override** — upgrade a staff: layout survives. Assign a different set on the
   upgraded copy: the two diverge, and the panel says `customised for this copy`. `reset to
default` brings the inherited set back.
6. **Mouse path** — the same rows, the same refusals, from keyboard and mouse.
7. **`show_all` in balanced mode** — unattuned spells listed and marked; selecting one is refused
   with the message, not silently ignored.
8. **Report** — the `balance=` line present and arithmetically consistent with the panel.

## Risks and rollback

- **Cast-path cost.** Membership reads the struct the steer already touches; expected
  0.1–0.5 µs. Measure with the existing `chord path` row before and after; if it moves by more
  than a microsecond, stop and look.
- **Keying change orphans a layout.** The base fallback is the protection; test #4 covers the
  upgrade path explicitly, and step 2 lands with no gate attached so it can be verified alone.
- **Two decision sites.** Prevented by step 1, which is why the refactor comes first and alone.
- **Rollback.** Each step is its own commit; the gate is removable by deleting one `if` because
  the classification and counting are additive. `balance = free` is the runtime rollback and
  needs no build.

## Out of scope

Grace-menu integration; Ashes of War; any cap on the number of assignments; separating
byte-identical duplicate weapons; changing the HUD mirror.
