# v0.2.15 — the report named a cost, and it was ours

0.2.14 added a clock inside the pad path. One reporter's first report on it read:

    adjust_inner:     14328 calls, mean 1146.8 us, worst 75693 us
    selection sample: 14328 calls, mean  501.0 us, worst 46767 us

That is 1.1 ms of our code on every poll and a 76 ms stall - four and a half frames at 60 Hz, and
exactly the shape of the stutter he had been reporting for days. This release takes both causes out.

**Upgrade is the DLL.** Nothing else changes; no new keys.

## Fixed

- **The weapon pair was cached for 8 ms "because that is the pad poll interval".** That is true on a
  fast machine. His polls arrive every ~28 ms, so the cache expired before the next call and every
  poll re-walked `WorldChrMan -> main_player -> PlayerGameData`. The cache is 250 ms now: a weapon
  swap is a menu or a deliberate input, not a per-frame event, and the walks drop from ~36 a second
  to four. A **button press edge still forces a fresh read**, so swapping to a staff and pressing R1
  inside that window is decided from the weapon actually in hand.
- **The selection sample ran on every poll to keep one line of the panel fresh - with the panel
  closed.** 501 us of mean and 47 ms of worst per sample on a machine whose pages are not resident.
  It now runs only while the panel is open, which is the only thing that reads it.

The underlying reason those reads were expensive is a page fault, not arithmetic: a pointer chase
into memory that is not in RAM stalls the input thread. Reading less often is what shrinks the tail;
the fix is to stop reading per poll, not to make the read faster.

## Verified

62 unit tests green; `cargo xwin check --target x86_64-pc-windows-msvc` clean. On a local 300 s run
the same section now reads:

    adjust_inner:     16223 calls, mean 2.1 us, worst 741 us
    selection sample:   180 calls, mean 2.6 us, worst 163 us

Mean down ~550x and worst down ~100x, with the rolling series at p50 18.95 ms worst-gap per second
and 2 seconds over 32 ms out of 300. Casting is unchanged: the press-edge read keeps the chord
decision honest.

## On the Present clock

`present_clock = on` works on Windows, where it gives a real once-per-frame clock; it still ships
**off** because address discovery faults inside `dxgi.dll` under Proton (`0xc0000005` at
`dxgi+0x2bb80`). Turn it on when reporting a stutter from Windows.
