# v0.2.3 — two-handing back, the bind gesture back, and a panel you can move

A fix release, driven by a test character that has been on the mod since v0.1.0. Four of the five
reports were regressions introduced by v0.2.2; the fifth had been there since the first build.
Nothing about the slot layout changes: still four spells per catalyst on the bumper, the trigger,
Y + trigger and Y + bumper.

Offline only, as before: me3 does not load EAC, no save file is written, and nothing in the game's
files is redistributed.

## Highlights

- **Two-handing works again.** Y + R1/R2 and Y + L1/L2 are the game's two-hand command as well as
  the mod's fourth and third slots. v0.2.2 claimed every chord and swallowed it even when the slot
  behind it was empty, so a weapon could not be two-handed anywhere. A chord is now only the mod's
  when it carries a spell; an empty chord is the game's press and reaches it untouched.
- **The bind gesture is back**, on the pad and on mouse/keyboard: hold **L3** and press the input
  you want to bind, or hold **Super** (the Windows key) and click. It binds whatever the game's own
  wheel has selected, so spells the character can only cast *with* a Great Rune or a talisman are
  assignable again.
- **The F8 panel moves again.** Drag its title band; the window edges still resize it.
- **Upgrading a catalyst keeps its spells.** Assignments are filed on the weapon's base id, and
  existing files are folded onto base ids on the next load.
- **Casting should hitch less.** The selection is steered once per press instead of on every pad
  poll of a held trigger.

## Fixed

- `fix(input): hand an empty Y chord back to the game` — a chord with no spell behind it was
  swallowed, which silently took the two-hand command away from every weapon in the game, not just
  from catalysts. Reported as "I can no longer two-hand weapons in any way at all, even without a
  casting tool equipped". The whole-of-hold Y hiding that v0.2.2 added for the two-hand latch went
  with it: it was the other half of the same bug, and its trade (no two-handing anywhere while a
  chord is bound) is worse than the latch it removed. Per-press Y clearing and the one-report
  staged release stay.
- `fix(input): steer the selection once per press, not once per poll` — steering is a write to game
  memory behind two readability checks and two locks, and it ran on every full pass for as long as
  the button was held, up to 125 times a second, for an answer that never changed. Reported as
  "performance still has hitches when actively casting". The bound-slot answer the poll asks for is
  now cached per weapon pair and store generation, so an unbound input costs one atomic read.
- `fix(input): bring back the bind gesture, plus a Super + click for mouse and keyboard` — v0.2.2
  removed it to cut the number of chords; that left buffed-stat spells with no way to be assigned
  at all, because the panel's requirement filter cannot see a Great Rune. The modifier is
  configurable (`bind_modifier`, default `L3`) and can be turned off.
- `fix(panel): make the window draggable again` — the header band still returned `HTCAPTION`, but
  the modal move loop that follows depends on the mouse capture the mod gives away on purpose while
  the panel is open, so the drag never tracked. The drag is now handled directly, from
  `GetCursorPos` and the button's own state.
- `fix(store): file assignments on the base weapon id` — the game treats every upgrade level as its
  own param row, so `33090010` (+10) and `33090000` were different weapons to the store and a trip
  to the smith emptied the slots. Existing files are folded on load, merged slot by slot, and
  rewritten once.
- `fix(panel): list spells the character can cast with active effects` — requirements are checked
  against INT/FAI/ARC including active `SpEffect` bonuses (`change_magic_point`,
  `change_faith_point`, `change_luck_point`), so a Great Rune or a stat talisman is enough. The
  strip says when effects are doing the work, and the load-time prune uses the same numbers so it
  stops dropping an assignment the character can actually cast.

## Upgrading

- `nr_casting.ini` gains a `[input]` section with `bind_modifier` (default `L3`; `none` disables the
  gesture). An ini without it keeps working — the default is the same as the old v0.1 behaviour.
- Slot files are rewritten once on load: `33090010 = …` becomes `33090000 = …`. Two lines for the
  same catalyst at different upgrade levels are merged, keeping the first spell each line assigns.
- The Y + bumper chords remain Recluse's on a Recluse character (`y_bumper = auto`). Super + click
  works with or without a controller attached; it is checked before the pad path, so a player with
  a pad plugged in but a mouse in hand can still bind.
