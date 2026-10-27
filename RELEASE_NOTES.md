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
- `perf(input): the same audit, three more per-poll costs` — after fixing the steering, the rest of
  the poll path was measured the same way. The pad path asked the OS about the mouse buttons on
  every idle pass (two `GetAsyncKeyState` calls, each a wineserver round-trip under Wine, on the
  thread the game polls the pad from, at up to 125 Hz all session) — now 60 Hz, and the
  no-controller path is deliberately not throttled because there a skipped poll is a "controller
  disconnected" report in the middle of a held click. `weapon_ids()` walked
  `WorldChrMan → main_player → PlayerGameData` plus a clock read separately for every question
  asked about it in a pass; it is answered from a cache keyed to the 8 ms pass. And two
  `GetAsyncKeyState` calls ran on *every* raw mouse record — movement included — before anything
  checked whether a button was even down.
- `feat(input): mouse and keyboard keep their own layer` — a click no longer goes through the pad
  report when a controller is attached. The mod steers the selected spell and the game's own click
  performs the cast, exactly as it does with no pad, so one click is one action and its timing no
  longer depends on the pad poll. (v0.2.2 synthesised a pad press and left the click to the game,
  which is how one click became an attack *and* a cast.) Shift is hidden from the game
  unconditionally now, since ER keeps its mouse bindings live with a pad plugged in and reads
  Shift + click as the strong attack.
- `fix(input): a catalyst never two-hands, and everything else always can` — whether a Y chord was
  the mod's depended on "is a spell bound to that slot", which made an empty chord on a staff
  two-hand the staff and a stray assignment on a sword make the sword un-two-handable. It now
  depends on the weapon: the param id bands `33xxxxxx` (glintstone staffs) and `34xxxxxx` (sacred
  seals) hold every vanilla catalyst and nothing else — name matching was tried first and is a trap,
  because `Watchdog's Staff`, `Rotten Staff`, `Devourer's Scepter` and `Barbed Staff-Spear` are a
  colossal weapon, a colossal weapon, a hammer and a spear. A weapon outside those bands that the
  name table does not know falls back to "has assignments", so a port's own catalyst still works. On a catalyst all four chords are the mod's — cast when bound, swallowed
  when empty; on anything else the whole press is the game's, so two-hand works even on a weapon
  carrying assignments.
- `fix(input): a mouse click is the mod's cast, not an attack as well` — with a controller attached
  ER keeps its mouse bindings live, so a click reached the game *and* the mod synthesised a pad
  press: the click attacked, Shift + click arrived as the strong attack ("shift + RMB does a normal
  attack, not the spell"), and whichever animation won decided the outcome. The raw record's button
  bits are now cleared when the click addresses a slot that carries a spell, and only in the world
  (never in a menu, where the game shows its cursor and clicks belong to the UI).
- `fix(input): keep the borrowed spell for the whole click` — the new mouse-poll throttle meant some
  idle passes never asked about the mouse, and those passes fell through to `release_borrowed()`,
  putting the player's own selection back into the magic entry *mid-click*. The game then read the
  restored entry, which is exactly "a spell right after another uses the last one cast". A cast in
  flight now holds the borrow until the button comes up.
- `fix(panel): stop the HUD mirror rewriting your spell selection` — the mirror forced the wheel to
  slot 1 about 30 times a second, so the game's own "selected spell" line visibly cycled through
  spells and any action that read the selection got the mirror's spell instead of the player's. It
  is now opt-in: `hud_mirror = off` by default, `on` restores the old behaviour.
- `fix(panel): read the game's own effective INT/FAI/ARC` — requirements are checked against the
  values the game publishes after runes, talismans, physick and buffs (`effective_intelligence`,
  `effective_faith`, `effective_arcane`). Summing the `SpEffect` rows by hand came out 8 short of
  the character sheet (76 vs 84), so spells the character could actually cast stayed hidden and
  could not be assigned. The strip says when effects are doing the work, and the load-time prune
  uses the same numbers so it stops dropping an assignment the character can cast.

## The two-hand latch, root-caused

The report that carried this release was "Y + combo still two-hands the staff, and then the spell
never comes out". Three separate causes were found, each hidden behind the previous one, and all
three are fixed:

- **The mod was rewriting a pad report the game never read.** ELDEN RING's import table is packed, so
  the hook falls back to patching the export in `xinput1_4.dll` — where it patched ordinal 2,
  `XInputGetState`. FromSoft reads gameplay input through **ordinal 100, `XInputGetStateEx`**, a
  different function that also carries the guide button. Both are hooked now, each with its own
  trampoline. Everything the mod did to the pad was previously applied to a copy that only Steam's
  overlay and other mods consumed, which is why every input fix "worked" in the log and not in the
  game.
- **A feathered trigger was a swap input.** ER acts on any nonzero trigger with Y held, and buffers
  that swap. The chord floor was 20/255, so a trigger easing through 1-19 reached the game untouched
  as Y + trigger and two-handed the staff ~300 ms later. The floor is 1: with Y down, any trigger
  movement is a chord.
- **The cast press itself arms the buffered swap.** ER remembers the shoulder press as a swap
  candidate and completes it when it next sees Y — including a report where the player is holding Y
  on its own, which the mod passes through untouched. For 700 ms after a cast press, Y is therefore
  cleared from reports that carry no trigger, so the swap has nothing to complete with. Chords are
  unaffected: they carry a trigger or bumper, so they are routed normally inside that window.

The two-hand *correction* (detect a two-handed catalyst, write `arm_style` back, re-send the press)
that was written while chasing this is **removed**. It could not hold while Y was down — ER
re-applies its own state every frame, which the player saw as the staff flickering in and out of
both hands — and with the three causes above fixed it had nothing left to correct. One less thing
running on the pad thread.

## Upgrading

- `nr_casting.ini` gains a `[input]` section with `bind_modifier` (default `L3`; `none` disables the
  pad gesture), `bind_modifier_kb` (default `Super`; also `Menu`, `F9`-`F12` or `none` — Super is
  grabbed by KDE and GNOME, so the alternatives matter on Linux) and `chord_stage` (default 2;
  reports a Y chord spends with Y released before the cast press — lower is faster, higher is more
  forgiving of the game's own latch). An ini without any of them keeps working: the defaults are the
  old behaviour.
- Slot files are rewritten once on load: `33090010 = …` becomes `33090000 = …`. Two lines for the
  same catalyst at different upgrade levels are merged, keeping the first spell each line assigns.
- The Y + bumper chords remain Recluse's on a Recluse character (`y_bumper = auto`). Super + click
  works with or without a controller attached; it is checked before the pad path, so a player with
  a pad plugged in but a mouse in hand can still bind.
