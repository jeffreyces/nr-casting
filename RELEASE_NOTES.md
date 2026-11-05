# v0.2.24 — the game's own wheel can skip the spells your buttons already own

One feature, one fix that matters, and the panel's settings row.

**Upgrade is the DLL.**

## New

- **The game's own spell wheel can skip the spells your buttons own.** `vanilla_skips_assigned = on`
  (off by default, applies immediately, independent of `balance`). Pressing up - pad or keyboard -
  only lands on the spells you kept for the default cast, so the spells bound across your staves no
  longer crowd the wheel you use for the rest. Nothing is written into your memory slots: the
  assigned spells stay attuned and still cast from their own buttons.
  - "Assigned" means any of the six weapon slots you are **carrying**, not just the two in your
    hands: switching weapons mid-fight does not change what the wheel offers, and a spell bound to
    your spare staff stays out of it.
  - It also steps over what the catalysts in your hands cannot cast - incantations while you hold a
    staff, sorceries while you hold a seal - because the game would list them and then refuse the
    cast. Unequipping a seal takes its incantations out of the wheel.
  - If every attuned spell is assigned, the wheel cycles normally and the log says so once.
  - Casting from a bound button leaves the wheel on the spell you had selected, so the next press of
    up carries on from where you were.
- **A slot can be left empty on purpose.** Every dropdown now ends with `none`, so one slot can be
  unassigned without clearing the others.

## Fixed

- **A memorised spell could be displaced when two casts overlapped.** The mod points the game's
  selected magic slot at the spell a button casts and puts the original back when the input is
  released. That bookkeeping held **one** record, so two casts close enough together - both
  shoulders pressed at once - meant the second record replaced the first and the first slot was
  never put back: it kept the mod's spell until it was re-attuned by hand.
  It is now one record per steered slot, the write and its record happen under a single lock, every
  restore is read back, and a slot that changes while no cast is in flight is named in the log. A
  spell sitting in two memory slots at once - which the game's own menu cannot produce - is reported
  on sight, because it is the fingerprint of exactly this failure.
- **The wheel no longer jumps to another spell after a cast.** Casting makes the game re-point the
  wheel at the spell just cast, which is by definition an assigned spell, so the skip stepped off it
  and left the wheel on a spell the player did not choose. The selection is now put back where the
  player left it.

## Changed

- **The panel's own settings sit on their own row**, above the buttons: `balanced`,
  `skip assigned in wheel`, a divider, then `UI scale`. Every section break is the same distance and
  the panel's height is measured from it, so the help text and the bottom row no longer clip when the
  window is resized.
- **Diagnostics say what the cast path did**, one number per meaning: magic-slot writes that read
  back, writes that did not, casts refused while others were still in flight, restores that did not
  land, and restores that found the slot already changed. The wheel line reports moves, the
  everything-assigned case and pauses; opening the panel logs the wheel plus the six carried weapon
  slots.

## Verified

125 unit tests green. `cargo xwin check --target x86_64-pc-windows-msvc` clean. Played locally
through the whole cycle: the selected spell stayed selected through repeated casts of a bound spell,
the 11th memory slot held its spell through overlapping presses, and the wheel skipped assigned and
uncastable spells with a spare staff equipped and then removed.
