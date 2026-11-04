# v0.2.21 — the correctness pass over balanced mode

Everything here is a defect in 0.2.20's balanced mode, found by holding that code to the same
review the rest of the mod gets. Free mode is untouched, and no setting changes meaning.

**Upgrade is the DLL.** Nothing new in the ini.

## Fixed

- **A spell attuned a moment ago was refused for up to a second.** The memorised set is cached, and
  the cached "not attuned" was believed without a second look — so after attuning a spell at a grace
  the next press could do nothing at all. A positive answer from a slightly old snapshot is harmless
  (the game refuses a spell it can no longer cast), but a negative one costs the player an input, so
  the negative is now re-read before it is believed: wrong for at most a quarter second, at four
  reads a second rather than one per poll.
- **A weapon whose assignments were all dormant still claimed its Y-chords.** The mask that decides
  whether Y+R1 / Y+L1 belong to the mod was built from the store alone, so a row that could never
  cast still counted as bound and the chord was swallowed instead of two-handing. The mask now
  applies the same gate the press does, through the same helper.
- **The panel kept its own copy of the mode.** Edit `balance` in the ini with the panel open and the
  checkbox, the attuned column and the candidate list stayed on the old mode while casting obeyed
  the new one. The panel now takes the mode in force every frame, whichever way it changed.
- **Switching modes leaked a configuration per click.** The toggle re-read the whole ini, and a
  re-read publishes a new `Config` that is never freed. The mode is now published as one value, so a
  click costs nothing and applies on the next input.
- **The report's fifth figure was named for something it did not measure.** `stale=` counted rows
  whose spell is not attuned, while the panel's "stale" also counts a spell the catalyst cannot cast
  — two different numbers sharing one word. The report's figure is now `not_attuned=`.

## Changed internally

- **The store's lookup calls the tested rule.** `store_get` / `store_set` carried their own copy of
  "this copy's override first, then the type's default"; they now call the same functions the unit
  tests cover, and the report's counts come from a tested `coverage()` instead of a private loop.
- 105 unit tests (7 written before this pass, red first).

## Verified

`cargo xwin check --target x86_64-pc-windows-msvc` clean, no warnings; 105 tests green. The counts,
the keying rule and the report line are covered by tests; the panel and the gate were re-checked on
one character under Proton. **Not tested on Windows.**
