# Vanilla spell wheel: skip what the panel already owns

Plan for the request: "hide the spells I already assigned from the D-pad selector, or at least skip
them". Settled with the user, then tests, then code.

## Settled decisions

1. **Skip, not hide.** The wheel is the game iterating `EquipMagicData.entries`. Hiding means blanking
   `param_id` in real memory slots, which empties them for the game, shrinks the attuned list that
   balanced mode reads, and fights re-attuning. Skipping reaches the same outcome - pressing up only
   ever lands on an unassigned spell - without editing the player's character data.
2. **Scope is every weapon carried (six slots).** *Corrected twice by testing.* First version was
   the whole character's saved rows; second was the pair in hand; the settled rule is the character's
   `equipment_param_ids` slots 0..=5 - three left, three right - because switching hands mid-fight
   must not change what the wheel offers, and a spell bound to the spare staff is still spoken for.
   The pair in hand stays the *cast* path's question, not the wheel's. Original note: the first version counted every saved
   row for the character, and a player with two staves cleared a spell from the staff in hand and it
   still never appeared in the wheel — it was in a row belonging to the other staff, or to another
   copy's override. The wheel is asking "what do the buttons I am holding own", so the set is the
   resolved row of each equipped weapon (full-then-base, the same rule the cast path uses). Swapping
   to a weapon with no rows therefore makes the wheel offer everything, which is what a player
   expects when the panel owns nothing on it.
3. **Everything assigned ⇒ vanilla.** If no unassigned spell is left, the game cycles exactly as it
   does today, and the log says so once. The wheel must never lock.
4. **Pad and keyboard both.** Not by matching a button: by watching the *effect*. The selection index
   is read from the game each frame; when it changes to a spell we already own, we move it to the next
   free one. This needs no knowledge of the keyboard's cycle key, survives remaps, and is one rule
   instead of two.
5. **Independent of `balance`, default off**, live-reloadable like every other ini key.
6. **A spell the hands cannot cast is hidden too.** *Added after testing:* pair scope alone brought
   incantations back into the wheel when the seal was unequipped, which the reporter preferred
   hidden. The rule is one question - "would the game refuse this cast right now" - and the answer is
   already in the catalyst capability read from the weapon rows. Unknown school is never hidden, and
   with no catalyst equipped only assignments hide anything.

## Why effect-based rather than swallowing the input

- The input path would need the D-pad bits rewritten *and* the keyboard key identified - a probe
  (`GetAsyncKeyState` traffic) plus two code paths that can disagree.
- Watching `selected_slot` is input-agnostic by construction: any cycle, any remap, either device.
- Cost: the pointer is cached after the first read, so the check is one atomic load plus a struct
  field per frame, only while the toggle is on.
- Known cost of the approach: the assigned spell can be the selection for the frame in which it
  arrived. A counter records how often we had to move on, so the flicker is measurable rather than
  argued about. If it is visible in the playtest, the fallback is the button-swallow path for the pad
  and a key hook for the keyboard - recorded here so the trade is not re-litigated later.

## Tests first (red before any implementation)

Pure, in `mod/pure`:

| behaviour | cases |
|---|---|
| `next_unassigned(current, len, assigned)` | one assigned after current is skipped; a run of assigned is skipped; wraps past the end; current is free ⇒ returns current (no move); single free entry; every entry assigned ⇒ `None`; empty list ⇒ `None` |
| assigned set for the character | built from every layout row; includes per-copy overrides; ignores rows with `-1`; a weapon with stale assignments still contributes its spells |
| decision `should_skip` | toggle off ⇒ never; toggle on + changed + assigned ⇒ move; toggle on + changed + free ⇒ no move; toggle on + all assigned ⇒ no move (vanilla) |
| monotonic safety | the skip loop is bounded by the number of entries, so a corrupt assigned set cannot spin |

## Implementation order (one commit per step)

1. `next_unassigned` + tests (red, then green).
2. `assigned_spells(character)` beside the existing store resolution + tests.
3. `config`: `vanilla_skips_assigned = off`, live publish, printed in the state line.
4. `game`: a small `Selection` view - `current()`, `len()`, `set(index)` - over `magic_data()`, so the
   window and the skip path do not both reach through the pointer.
5. `dispatch`: one job on the frame clock (the existing pattern), gated on the toggle and the world
   being live; carries the "changed since last look" edge and the counter.
6. Panel: the checkbox beside `balance`/`UI scale`, and a report line `skip=on moved=12 all_assigned=0`.
7. Docs: README key, NEXUS if user-facing, release note when it ships.

## Alignment with the twelve, and what is left alone

- **KISS** - one predicate and one bounded loop. No new hooks: the frame clock and the dispatcher
  already exist.
- **DRY** - reuses `magic_data`, the store's layout resolution, the config publish path and the
  dispatch job shape. The assigned set is computed by the same rule the cast path uses, not a second
  copy of it.
- **SOLID-S** - the rule lives in the pure crate; `game` only reads and writes the selection; the panel
  only toggles. Adding the toggle changes no existing cast code.
- **OCP** - a new key and a new job; the cast path is untouched.
- **YAGNI** - no true hiding, no per-weapon scope, no menu UI, no new input hook.
- **SoC** - decision (pure), effect (game module), trigger (dispatch) stay separate.
- **Robustness** - unreadable magic ⇒ no action; all assigned ⇒ vanilla; bounded loop; toggle off ⇒
  byte-identical behaviour to today; a failed write is logged and counted, never retried in a loop.
- **Testability** - every rule above is a pure function with a table.
- **Documentation** - ini key, report counters, this plan.
- **Refactoring** - none required; the selection read already exists.
- **Abstraction / Demeter** - `Selection` keeps the pointer walk in one place instead of two call
  sites reaching through it.
- **Composition over inheritance** - not applicable; nothing here models a hierarchy.

Left alone deliberately: the cast path's steering and borrow machinery (it writes an entry per cast
and restores it; the skip path must not borrow), the balanced-mode gate (the two toggles are
independent by request), and the HUD mirror (it forces slot 1 to the weapon's first spell; a session
with the mirror on will show the mirror's choice, not the skip's, and that is existing behaviour).

## Manual test script

1. Attune 8 spells; assign 3 to R1/L1 rows on staff A. Toggle off: pressing up cycles all 8 (baseline).
2. Turn the toggle on in the panel: pressing up only lands on the 5 unassigned spells.
3. Press R1/L1: the assigned spells still cast (cast path untouched).
4. Assign every attuned spell: pressing up behaves exactly as before, and the log notes it once.
5. Swap to a staff with different rows: the skipped set is still the character's, so the wheel does not
   shuffle.
6. Repeat 1-3 on keyboard.
7. With the panel open, the wheel does nothing (the panel owns input).

## Risks and rollback

- **One-frame flicker** - measured by the `moved` counter; if visible, fall back to input swallowing.
- **Fighting the game** - if the game rewrites the selection every frame while a direction is held, the
  job sees repeated changes. Guard: only act on an edge, and stop acting for the rest of that hold.
- **Menu context** - the spell menu writes the selection while it is open. The job only runs with the
  world live and the panel closed; if the menu case turns out to matter, gate on the game's own
  "menu open" state rather than guessing.
- **Rollback** - the toggle is one ini key, default off; setting it back restores today's behaviour
  exactly, with no data written.
