# v0.2.2 — eight spells, keyboard and mouse, and assignments that follow the character

This release is about who owns an input and which character owns an assignment. The mod now casts
from a pad *or* from keyboard and mouse with no controller attached, carries a fourth spell per
catalyst on **Y+R1 / Y+L1**, and files its assignments per character instead of per weapon — so the
same staff on two characters no longer shares one set of spells.

It also stops fighting Recluse: that port uses Y+R1 / Y+L1 (and its `4` / `5` keys) for its own
actions, and on a Recluse character those inputs now go to it.

Offline only, as before: me3 does not load EAC, no save file is written, and nothing in the game's
files is redistributed.

## Highlights

- **Eight spells across two catalysts.** Y + R2 / Y + L2 carry the third slot and Y + R1 / Y + L1
  the fourth. Y never reaches the game while a chord is bound, so its two-hand latch cannot fire
  mid-fight — the trade is that two-handing is unavailable until `y_bumper = off`.
- **Keyboard and mouse, with or without a controller.** With no pad connected ELDEN RING stops
  polling XInput entirely, so a click cannot be answered by rewriting a pad report. The mouse is
  read through raw input instead: the click steers the selected spell and the game's own click
  performs the cast.
- **Assignments are per character.** `nr_casting_slots_<name>-<hash>.txt` per character, and any
  assignment a character cannot cast is dropped when it loads.
- **Coexists with Recluse.** Y+R1 / Y+L1 on the pad are left untouched on a Recluse character, and
  an Alt + click is translated into the port's own `4` / `5` key rather than being swallowed.

## Added

- `feat(input): a fourth spell per hand on Y+R1 / Y+L1, gated on Recluse` — `y_bumper = auto`
  takes the chords unless the Recluse mod is loaded *and* the character is a Recluse; `on` always
  takes them, `off` gives vanilla two-handing back. A Recluse character is recognised by
  `recluse_chr_ids` or by an equipped catalyst that is missing from the vanilla weapon table.
- `feat(input): mouse casting with no controller, via raw input` — `GetAsyncKeyState` showed 3038
  calls per 30 s with zero for LMB/RMB, while `GetRawInputData` carried the button edges, so raw
  input is the layer the mod drives.
- `feat(store): keep assignments per character` — the store switches file on every character load,
  from the 500 ms watcher as well as the load edge.
- `feat(panel): reset slots` and `feat(panel): remember planning mode across sessions` — a clear
  for the loaded character, and `show_all` persisted in the ini.
- The panel's character line names the character whose file is being edited; on a Recluse character
  the fourth row is greyed out and labelled *passthrough to its 4 / 5 key*.

## Changed

- **The bind gesture is gone**, on pad and on mouse. Assignment happens in the panel only: a
  hold-a-modifier chord collided with class ports and could not exist on keyboard and mouse.
  `bind_modifier` is removed from the ini.
- **The HUD mirror is always on**; `hud_mirror` is removed. The right-hand catalyst's first slot is
  mirrored onto the game's own wheel, so the game shows what R1 will cast.
- **Mouse modifiers** are click (slot 1), shift (slot 2), ctrl (slot 3) and alt (slot 4), left or
  right button. Shift is hidden from the game while it is used this way, because the game binds
  Shift + click to the strong attack.
- **The panel is a tool window**: no alt-tab entry, no taskbar icon, and Alt closes it so focus
  always lands back in the game.

## Fixed

- `fix(input): rewrite the pad on every report, not only the sampled one` — the repeat gate skipped
  the report rewrite too, so skipped reports handed the game Y+trigger and two-handed the staff.
- `fix(input): release Y for one report before the synthetic shoulder press`, then
  `fix(input): hide Y from the game while a chord is bound` — the game latches "Y held" and fires
  the swap on the next shoulder press, so clearing Y in one report was not enough.
- `fix(input): make a chord safe before the game-memory gate` — a chord polled while pointers were
  being re-validated (right after a cast or a load) went through unchanged.
- `fix(input): never let an empty Y chord reach the game as a two-hand`.
- `fix(input): mouse casts work with no controller connected`, `fix(input): hand the Alt chords to
  Recluse instead of dropping them`.
- `perf(game): cache the character pointers`, `perf(input): stop walking the import table twice a
  second`, `fix(input): make the first seconds after a load work` — the pointer chase, the import
  walk and the post-load dead period are gone.

## Upgrading

- `nr_casting.ini` loses `bind_modifier` and `hud_mirror`, and gains `show_all` and the `[recluse]`
  section. Existing files keep working; unknown keys are ignored.
- The old shared `nr_casting_slots.txt` is adopted by the first character that loads and renamed to
  `nr_casting_slots.migrated.txt`. Every later character starts empty.
- Two characters with the same name **in the same save slot** share a file — the game exposes no
  per-character id, and `packed_time_stamp` reads 0. What a character cannot cast is dropped on
  load, and `reset slots` clears everything for it.
