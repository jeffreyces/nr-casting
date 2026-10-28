# v0.2.6 — Y comes back

Two bugs, one cause: a saved deadline of `0` was read as "the deadline is the epoch", which on a
32-bit millisecond clock looks like it is still in the future for half of every 49.7-day wrap.

**Upgrade is the DLL only.** Nothing else changes.

## Fixed

- **Y stopped working at a Site of Grace until you had cast once.** Every pad report without a
  trigger or bumper had Y stripped from it, because the "quiet Y" window was armed by a zero
  timestamp. The first cast replaced it with a real deadline and Y behaved from then on, which is
  why it looked like a mod-specific quirk rather than a clock bug.
- **A mouse cast could keep the borrowed player entry held indefinitely.** The same zero sentinel
  guarded the 400 ms hold that keeps the character readable across a mouse cast, so the entry was
  never considered expired once the clock's high bit was set.

Both now go through one tested predicate, `armed_until(now, until)`, where `0` means "nothing is
armed" and wrap-around is handled explicitly. Three new unit tests cover the unset case, both
sides of the boundary, and a deadline that spans the wrap.

## Why it appeared now

Nothing in v0.2.4 or v0.2.5 touched the input path (`mod/src/input.rs` is unchanged since v0.2.3).
The bug has been latent since the "quiet Y" window was introduced; it needs the epoch-millisecond
clock's bit 31 set, which happens for 24.8 days out of every 49.7. It crossed over during this
play session.

## Upgrading

Replace `nr_casting.dll`. Assignments, the ini, the loader profile and the cached spell catalogue
are reused as they are.

## Verified

Cold launch with the Recluse port loaded, no cast beforehand: Y works at a Site of Grace, chords
still cast, 2,910 weapon and 214 spell names read from the game's text (18 ms), 39 unit tests green.
