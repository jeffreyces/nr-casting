# v0.2.14 — the group was not the line

A reporter's bundle, run on the Present clock, is unambiguous that only one hook group costs him
anything: with the pad path bypassed his typical worst frame drops from 55.8 ms to 34.4 ms and the
number of hitchy seconds drops from 43 to 29 out of 60, while switching raw input off or removing
the cursor detours changes nothing at all. A group is not a line of code, so this release measures
inside the group.

**Upgrade is the DLL.** Nothing else changes; no new keys.

## Added

- **A `==== pad path ====` section in every report.** Four clocks inside the pad path, each with a
  call count, a mean and a worst: the whole `adjust_inner` pass, the idle fast path, the throttled
  selection sample, and the chord/cast path. Mean is what one frame pays; worst is what one hitch
  can be - a part that runs 190 times a second at 3 us is 0.06% of a core, while a part that runs
  once a second at 2 ms is four dropped frames at 60 Hz, and the two need different fixes. It also
  separates "our code is slow" from "the game is slow because of what our code did", which the
  detour's single average cost could not.
- The timers record on drop, so the early `return` paths - which is most of them - are measured too.
  Costs three relaxed adds per poll when the perf log is off.

## Fixed

- **CPU was attributed to the wrong thread.** The frame-thread share followed whoever called
  `PeekMessageW`, which is not the presenting thread on every machine. One reporter's report showed
  56 presents a second beside "46 ms of CPU per frame" and "the thread used 88% of each frame's
  time" - three numbers that cannot all be true. The Present detour now names the frame thread, so
  when `present_clock = on` the share-of-frame figure is measured on the thread that presents.

## Verified

62 unit tests green; `cargo xwin check --target x86_64-pc-windows-msvc` clean; the new section is
present in the built DLL.

## On the Present clock

`present_clock = on` works on Windows - a reporter ran a full six-phase bundle on it with the tick
rate holding at 55-59/s in every phase, which is what makes the phases comparable at all. It still
ships **off** because address discovery faults inside `dxgi.dll` under Proton (`0xc0000005` at
`dxgi+0x2bb80`), and Proton is where this mod is developed. On Windows it is worth turning on when
you report a stutter.
