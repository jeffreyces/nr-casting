# v0.2.17 — the last periodic read of game memory, gone

0.2.16 shipped counters instead of a worst case, and the first report back read like this:

    adjust_inner:     18095 calls, mean 250.7 us, worst 67542 us, 153 over 250 us
    idle path:        18095 calls, mean   0.1 us, worst    38 us, 0 over 250 us
    selection sample: 18095 calls, mean 250.0 us, worst 67541 us, 153 over 250 us
    idle passes that touched game memory: 0 of 18095

153 slow polls out of 18,095 carried every microsecond of that mean: about 29 ms each, 4.4 seconds
of stall in 7.4 minutes - one hitch every couple of seconds. The selection sample accounted for all
of it, case for case (its worst, 67541 us, against adjust_inner's 67542 us). Everything the idle
path does is now free and provably reads nothing.

**Upgrade is the DLL.** Nothing else changes; no new keys.

## Fixed

- **The player's selected spell is no longer read on the input path at all.** It is read when a
  report is built and not before, after re-checking the pointer, because a report can be asked for
  at any moment including while the world is going away. That number feeds one line of a report;
  paying for it on the pad poll was paying a page fault on the frame thread every two seconds.
- The pad path's `selection sample` row is gone from the report, because there is nothing left to
  time. A zero row is worse than no row.

## Why this matters only on some machines

The same read costs microseconds where the game's pages are resident, and tens of milliseconds where
they are not: touching a page that has been paged out forces the operating system to bring it back,
and the cost lands on the thread that touched it. One report showed the game with 17.5 GiB of
private commit against 5.7 GiB resident - most of what it has booked is not in RAM - while the same
mod on a machine with everything resident measured 3 slow reads in 15,265 polls.

The conclusion that follows is a rule rather than a tuning: **nothing periodic reads the game's
memory from the input thread.** Reading less often only shrinks the tail; not reading removes it.

## Verified

62 unit tests green; `cargo xwin check --target x86_64-pc-windows-msvc` clean. Nothing periodic
touches game memory from the poll thread now: the stance read is behind `debug_log`, and the
selection read belongs to report generation. The `idle passes that touched game memory` counter and
`over 250 us` column are what a report should be checked against - both should be at or near zero
while playing.

## On the Present clock

`present_clock = on` works on Windows, where it gives a real once-per-frame clock; it still ships
**off** because address discovery faults inside `dxgi.dll` under Proton (`0xc0000005` at
`dxgi+0x2bb80`). Turn it on when reporting a stutter from Windows.
