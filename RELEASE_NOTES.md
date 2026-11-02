# v0.2.19 — the timers became events, and two fixes came out of the review

A release about where the work happens rather than how fast it is. The mod's own hot path was
already microseconds per poll; this removes the things that were *asking a clock a question* when an
event could answer it, and it fixes two defects that the change itself exposed.

**Upgrade is the DLL.** No new ini keys; the ini is now reloaded while the game runs, which is a
behaviour change worth knowing about (below).

## Changed

- **The ini is watched, not read once.** Saving it while the game runs applies `debug_log`,
  `perf_log`, `hud_mirror`, `show_all`, `ui_scale`, `bind_modifier` and `hotkey` within a second.
  The switches that must exist before a hook is installed (`overlay`, the hook groups,
  `present_clock`) say in the log that they take effect next launch instead of being silently
  ignored. The mod's own writes to that file (panel size, corner, scale) are recognised by their
  content and never trigger a reload.
- **One dispatcher owns time-gated work.** Hooks mark what happened; the dispatcher runs it at most
  once per declared interval, served from the pad poll or the frame clock. A mark that arrives inside
  the interval is dropped rather than queued - for these jobs the newest state is the only state that
  matters. The report gains a `==== dispatch ====` section with runs, cost, and how many marks each
  job dropped inside its own throttle.
- **Three caches are now nets rather than timers.** The weapon pair drops on any input change (press
  *and* release, buttons *and* triggers) and on opening the panel, with a 10 s backstop. The character
  block is re-checked on a change of character with a 5 s backstop. The HUD mirror marks its wish and
  the dispatcher runs it at 10 Hz instead of the pad path checking a clock.
- **The perf writer waits instead of sleeping.** Two wakeups a second became zero until a window
  closes.

## Fixed

- **Swapping to the seal and pressing R2 quickly cast from the weapon just put away.** The press edge
  that invalidates the weapon cache watched the digital buttons only, and R2/L2 are analogue - so the
  edge never fired for the input that mattered. The edge now covers a trigger crossing the cast
  threshold, and treats the release as an edge too: the game applies an equipment swap a few frames
  after the button, so a read on the press can still see the old weapon and would have stamped it as
  fresh. Related: opening the panel now invalidates the pair, because a panel that shows which weapon
  is in hand is a read of the world, not a cached answer.
- **A dispatch handler could be reached without its guards.** Once the frame clock could serve the
  same marks, the HUD mirror became reachable from a thread that had never run it, without the checks
  its old caller applied - and it writes the game's selection. Each handler now declares which clock
  may run it and checks its own preconditions.
- **The selected-spell read is off the panel thread.** It moved there in 0.2.17 to get it off the poll
  path, which traded a stutter for a lifetime race against a pointer the game can free. It is
  requested when the panel opens and served by the poll thread; the report prints the reading's age.
- **A fast weapon swap then cast used the catalyst it had just left** - or swallowed the press with no
  spell and no failed-cast gesture at all. This one is subtle and was made repeatable by the longer
  cache: the read taken at the moment of the press returns the *pre-swap* weapon, because the game
  applies an equipment change in its own frame, after the mod has already handed over that frame's
  report. The old pair came back stamped as fresh, and the chord a frame later trusted it. The cache
  is now disbelieved for 200 ms after any input edge, which is microseconds per poll and only while
  the player is pressing things.
- **`debug_log = on` had no effect on debug lines.** The macro reads one atomic rather than the
  config (the check has to come before the `format!`), and that atomic was filled once at load - so
  the new live reload applied everywhere except the thing it was turned on for. A reload now pushes
  the cached flags. A chord swallowed because the slot is unbound also logs the hand, weapon and slot
  it decided were empty, so "I pressed it and nothing happened" is diagnosable from the file.
- **Two clocks could run the same handler at once**, and the ini watcher could spin at a full core if
  its notification failed to re-arm. Both were found in a twelve-principles pass over these changes.

## Report

- `mod: nr_casting 0.2.19 (built 2026-11-02T...)` - the build time is stamped in, so two builds of one
  version can be told apart.
- Sections say which window they cover: the per-second series is the last five minutes, the pad-path
  and dispatch tables are counts since load.
- `character recheck` replaces `pointer recheck` (it measures the character block), and the header
  says that row is per check rather than per frame.
- A job that costs less than the microsecond clock can resolve prints `under 1 us` rather than
  `0 us`.

## Verified

67 unit tests green; `cargo xwin check --target x86_64-pc-windows-msvc` clean, no warnings.

On the development machine, one nine-phase run:

| phase | worst-gap p50 | over 32 ms |
|---|---|---|
| 1 everything on | 18.93 ms | 0 |
| 2 raw input off | 19.05 ms | 0 |
| 3 cursor hooks out | 19.03 ms | 0 |
| 4 pad casting off | 19.15 ms | 0 |
| 5 housekeeping off | 19.11 ms | 0 |
| 6 pad only | 19.18 ms | 0 |
| 7 every group off | 18.97 ms | 0 |
| 8 everything back on | 19.00 ms | 0 |
| 9 drift check | 18.80 ms | 0 |

Spread across nine minutes 0.38 ms; the two identical closing phases agreed to 0.20 ms, so nothing
drifted mid-run. The pad path measured `adjust_inner 0.7 us mean, 1 slow poll of 32,655`, and the
character recheck fell from about 0.8 calls a second to 0.17.

The swap-then-cast fix was verified from the log rather than by feel: with `debug_log` on, a session
recorded 611 cast decisions and zero swallowed chords, every line naming the weapon it read
(`R2 -> hand right weapon 34060000 (pair L=33090010 R=34060000) slot 1 spell 6810`).

## Not tested here

`present_clock` is still off by default: its address discovery faults inside `dxgi.dll` under Proton
and has only ever run on a native Windows machine, so it was not re-tested for this release. The live
ini reload was exercised on Linux under Proton; nothing in it is platform-specific, but it has not
been tried on Windows.
