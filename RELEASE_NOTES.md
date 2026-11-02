# v0.2.18 — a borrow in flight is not a read

One line in the report was wrong, in the direction that flatters us least: it over-reported.

0.2.17 added `idle passes that touched game memory: N of M` so that "the idle path reads nothing"
would be a measurement rather than a claim. The counter incremented whenever a steered spell was
still borrowed - which includes the hold window after a cast, where the code checks a deadline and
touches nothing. A run reported 462 idle reads where the real number was the handful of restores
that actually walked game memory.

**Upgrade is the DLL.** Nothing else changes; no new keys.

## Fixed

- `release_borrowed` now returns whether it touched game memory, and only that increments the
  counter. A non-zero count means what the line says: an idle pass really did read, because a
  restore was due. A borrow that is merely in flight is a deadline check, not a read.
- The unused `borrow_active` helper went with it.

## Verified

62 unit tests green; `cargo xwin check --target x86_64-pc-windows-msvc` clean, no warnings.

Nothing else changed. Read this release as a correction to the instrument, not to the fix: 0.2.17's
numbers - `adjust_inner` mean 19.8 us with 7 slow polls out of 17,558, and a frame median that fell
from 46.9 ms to 21.0 ms on the machine that reported the stutter - stand as measured.
