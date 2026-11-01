# v0.2.13 — the instrument says what it knows

Two reports in a row contained sentences that described our intent rather than the evidence we
had: an empty phase table that blamed a dead frame clock, and a probe line that promised a write
check nobody ever saw the result of. Those are fixed, and the timing line no longer costs a frame
every ten seconds.

**Upgrade is the DLL.** One new ini key, `present_clock`, documented and off by default.

## Fixed

- **The perf line was written on the game's thread.** Formatting and flushing a file inside the
  per-frame hook cost up to 17 ms once per ten seconds (`our detour max 17055us` in a reporter's
  log) — a stutter we were causing ourselves. The hook now only stores six numbers; a thread that
  is allowed to block writes them.
- **CPU was attributed per second, not per frame.** A second at 21 fps and a second at 55 fps were
  compared as if they were the same quantity. Each second's CPU is now divided by the frames that
  second actually produced, and the report gives the frame thread's share of one core plus the
  share of each frame it spent working.
- **Phase tick counts came from a counter that resets.** They printed `0 ticks, 0/s` for phases 2-6
  while all six claimed 60 s, which silently disabled the report's own "not comparable" guard.
  They now come from a lifetime counter.
- **A plain report claimed the frame clock was dead.** Pressing `diagnostics & report` printed six
  empty phases under "nothing was feeding the frame clock — load a character and run this again",
  when the bundle had simply never been run and the character had been loaded the whole time. It
  now says so in one line and stops.
- **A phase recorded at a different tick rate was not flagged.** The header promised that check and
  nothing performed it; a real run had phase 1 at 74 ticks/s against exactly 60/s in phases 2-6
  (the panel was still tearing down) and presented all six as one measurement. A spread over 10%
  now prints a line naming both phases.
- **`neutralise_mouse` wrote past the buffer.** It zeroed every byte from the header to the end of
  the slice it was handed; it is now clamped to the record's own declared size.
- Two input regressions from 0.2.11's work that made every raw-input record read as "no input" are
  reverted: one detour per function, and the cursor hooks stay installed.

## Added

- **`present_clock = on|off`** (default `off`). A pass-through `IDXGISwapChain::Present` timer is
  the only clock that is genuinely once per frame; every other clock is a proxy. It stays off
  because address discovery faults inside `dxgi.dll` under Proton (`0xc0000005` at `dxgi+0x2bb80`);
  it needs a real Windows test. When it is live, the pad and pump clocks stand down.
- **The guided run bisects hook groups** — everything on, raw input off, cursor hooks removed, pad
  casting off, every group off, back on — with one pinned clock for all six phases and a tick count
  per phase.
- **The pad path's average cost**, not just its worst. A 200 µs worst case every ten seconds is
  noise; a 200 µs average over 120 polls a second is 2.4% of the frame budget.
- **The cursor detours come out when the panel closes** (they are kept as removable patches, not
  permanently installed), so a closed panel costs nothing.
- **`steering: N of M magic-entry writes read back as written`** in the report. The write check used
  to be a sentence; now it is a count.

## Verified

62 unit tests green; `cargo xwin check --target x86_64-pc-windows-msvc` clean. Local bundle run, all
six phases with a wired pad and a pad poll in every second: worst-gap p50 18.93-19.18 ms across the
six phases (0.25 ms spread), **zero seconds over 32 ms in any phase**, and zero frames over 32 ms in
the rolling 300 s series. Present clock remains unproven on Windows, which is why it ships off.

