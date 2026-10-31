# v0.2.7 — a panel you can read, and a report you can send

The release is mostly the panel. It scales, it remembers where you put it, it no longer loses its
border when you drag it small, and it can write you a diagnostics file that a bug report can actually
use. The one casting bug — a first press failing while the wheel caught up — is fixed too.

**Upgrade is the DLL.** The ini, your assignments and the cached catalogue are reused; the new keys
(`ui_scale`, `panel_size`, `panel_pos`, `perf_log`) fall back to their defaults if your file predates
them.

## Fixed

- **The first press of a slot could fail.** A cast on the trigger slot steered the game's shared spell
  wheel and pressed in the same report, so the game sometimes read the spell that was selected
  *before* the steer and played the casting-failed gesture; a second press worked because the wheel
  had caught up. The steer is written first now and the press is swallowed if it did not take, with
  the borrowed entry held 250 ms past the release so a slow frame still reads the right spell.
- **The panel's border vanished below a certain size and the window could not be dragged afterwards.**
  Three separate causes: ImGui's border is drawn on a rect expanded half a pixel outward and was
  clipped at the buffer edge, the window manager's modal sizing loop leaves the client rect and the
  swap chain out of step (`WM_SIZE` can lag), and a swap chain that stopped presenting left the last
  picture frozen. The frame is drawn on the window's own draw list inside the buffer, the client
  rect is reconciled at frame time, the panel paints from `WM_PAINT` while a drag is in progress, and
  a one-second no-present watchdog rebuilds the swap chain.
- **Text and widgets did not follow a resize.** The scale is now a font size: `auto` is the largest
  scale that fits the window (`min(width ÷ 900, (height − 8) ÷ content height)`, floored to 5%), a
  corner drag rebuilds the atlas while you drag, and `100%`…`200%` in the dropdown — up to `300%` in
  the ini — pins it. The fit was wrong twice before this: an average overflowed the axis that bound
  (scrollbars both ways), and a round sat half a percent over the fit (a scrollbar in the last row).
  The formula lives in the tested crate now.
- **The panel's size and corner were saved but not restored.** `panel_size` rejected the size the
  panel itself writes — a content-fitted panel is 607 tall and the parser demanded 620 — so every
  launch warned and sized from the screen while the panel re-saved the same value. The floor is the
  window minimum now, both are written when you stop dragging, and `reset layout` restores first-run
  geometry.
- **The pointer was invisible over the world.** Under Proton 11 with the Steam Linux Runtime the
  game re-sets its own cursor over the overlay's arrow about 150 ms later; no `SetCursor` from this
  process wins that. The pointer is confined to the panel while it is open instead, which is enough
  to use it. (`PROTON_ENABLE_WAYLAND=0` or Proton 9.0 avoid the regression itself.)
- **Every `auto` label showed `?` where a dash belonged.** The panel draws with Dear ImGui's embedded
  font, which covers Latin-1 only; the em dashes were outside it. The UI is plain ASCII now.
- **A staff offered incantations and a seal offered sorceries.** Each catalyst now filters to its own
  school.
- **The panel opened one pixel inside the display**, and the first open had a band of empty space
  under the last row: the window is fitted to its content once, then the geometry is the player's.
- **Reporting:** five self-contradictions in the diagnostics file (uptime printed time since *boot*,
  the catalogue section showed 2 of 316 spells, seconds with no pad poll counted as 0 ms gaps inside
  the percentiles, the saved size rejected, two pointer lines that contradicted each other), and the
  character's name was left in the file twice. See below.

## Added

- **Diagnostics & report.** One button writes a single text file beside the DLL — log tail, resolved
  offsets, the generated spell catalogue, configuration, loaded modules, GPU/CPU, and a per-second
  record of frame gaps over the last five minutes — redacts your user name (`%USER%`) and character
  name (`<character>`), puts the path on the clipboard, opens the folder with the file selected and
  the issue page with the version and numbers prefilled. It waits for five minutes of the *current
  character* first and counts down on the button, so "300 s" means 300 seconds. Nothing is uploaded.
- **`ui_scale`, `panel_size`, `panel_pos`, `perf_log`** in the ini, all documented in the README and
  on this page.
- **`reset layout`** beside `reset slots`.

## Performance

- The panel thread registers the hotkey and blocks on its message queue; it no longer wakes 60 times a
  second to read one key. Log timestamps use a tick counter instead of a syscall. The frame series is
  sampled from a one-second window timer (so a keyboard-and-mouse player's report fills too) and
  restarts when a character loads.
- `perf_log` writes a line when a window has something to say — a lost frame, a slow poll, the panel
  opening or closing — and otherwise once every five minutes, instead of six identical lines a minute.
- Measured in this build: our input detour costs 12-113 µs at its worst poll in a ten-second window,
  against in-game stalls of 100-2,900 ms from the engine itself.

## Internals

- 52 unit tests in the pure crate (input routing, slot files, ini parsing, the scale fit, redaction);
  the scale formula moved there from the panel, where it could not be tested.
- `window.rs` was split: `panel_geometry.rs` (screen, saved corner, the fit) and `panel_shell.rs`
  (browser, file manager, clipboard).

## Verified

Cold launch on a 3440×1440 display: the panel opens at 125% (1194×601), the border holds when dragged
to its minimum, the corner and size come back after a restart, a staff lists sorceries and a seal
lists incantations, the report button counts down from 300 s, and the written file contains no user
name. 52 tests green.
