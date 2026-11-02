# v0.2.16 — measure the read, and stop making it

0.2.15 cut our per-poll work by removing two caches that never hit. A report on 0.2.15 showed it was
not enough: 1054.9 us mean and 81946 us worst per poll, with the selection sample still running on
12041 of 15751 polls - and the log explained why. The panel had reopened **25 times inside a
six-minute guided run**, so the run measured a game with a window, a D3D11 device and the cursor in
front of it for much of every phase, and kept a per-poll memory read alive while it did.

**Upgrade is the DLL.** Nothing else changes; no new keys.

## Fixed

- **The panel no longer opens while a guided run is going**, and says so in the log when something
  asks. The run exists to measure the game without the panel; a flapping window both contaminates
  the phases and keeps our own reads running.
- **The stance debug line read the player's structure on every poll** to feed a line that is only
  written when `debug_log = on`. Behind that flag now. That single change took the idle path's
  worst case from 69 us to 11 us on a machine where every page is resident, and it is the same read
  that costs milliseconds where pages are not.
- **The player's selected spell was re-read every 100 ms.** Its only readers are the report's
  "spell selected in the game" line and one debug line - nothing on the input path consumes it. It
  is now re-read every two seconds, and the throttled path is one atomic load.

## Added

- **`N over 250 us` on every pad-path line.** A worst case cannot tell one unlucky read from a
  machine where most reads are slow. 250 us is about a page fault's price and far above any
  arithmetic in these functions, so the count separates "occasional stall" from "the pages are not
  in RAM" - which are different bugs with different fixes.
- **`idle passes that touched game memory: N of M`.** The idle path is supposed to read nothing at
  all; this makes that a measurement instead of a claim. A non-zero count means a borrow is being
  held longer than it should.
- The hotkey logs which way it asked, so a log can name the source of a panel toggle.

## Verified

62 unit tests green; `cargo xwin check --target x86_64-pc-windows-msvc` clean. On a local machine
with resident memory, after the change:

    adjust_inner:     15265 calls, mean 2.0 us, worst 370 us, 3 over 250 us
    idle path:        15263 calls, mean 0.0 us, worst  11 us, 0 over 250 us
    selection sample: 15263 calls, mean 1.3 us, worst 369 us, 3 over 250 us
    idle passes that touched game memory: 0 of 15263

Three reads over 250 us in fifteen thousand polls, and a six-phase bundle ran at p50 19.07-19.38 ms
across all six with 0-3 seconds over 32 ms per phase.

Two honest notes from that run: phase 1 ticked at 121/s against 60/s in the later phases, and the
report says so rather than presenting it as a comparison - the pad clock settles after the panel
closes. And on a machine without resident memory these numbers are the baseline to compare against,
not a result: the read count is what will say whether the remaining cost there is paging.

## On the Present clock

`present_clock = on` works on Windows, where it gives a real once-per-frame clock; it still ships
**off** because address discovery faults inside `dxgi.dll` under Proton (`0xc0000005` at
`dxgi+0x2bb80`). Turn it on when reporting a stutter from Windows.
