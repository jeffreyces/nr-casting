# v0.2.20 — balanced mode, and a crash stack that names the caller

An optional mode that puts the mod back inside the game's own spell economy, plus the diagnostics
change that came just before it: a fault now prints the chain of callers, not only the instruction
that touched the bad address.

**Upgrade is the DLL plus one optional ini key.** `balance = free` is the default, so an existing
installation behaves exactly as it did.

## New

- **`balance = free | attuned`.** In `attuned` mode only the spells the character has memorised in
  the game's own spell menu can be assigned. Vanilla's ceiling is the memory slots; this mod stores
  four spells per catalyst, so with the gate off the library grows with the number of catalysts
  owned. The gate makes the mod a better way to use your attuned spells rather than a way around
  them. Off by default, switchable in the panel, and it applies in both directions immediately.
- **The panel grows an attuned column only in that mode** — the memorised spells in wheel order and
  a line of three figures: attuned, assigned, stale. In `free` mode the panel is unchanged.
- **A row whose spell is no longer attuned keeps its spell and its dropdown.** It is greyed with the
  reason rather than cleared, so re-attuning that spell re-arms the row by itself, and a different
  spell can be assigned in one click. Until that happens its input belongs to the game again: the
  press is not rewritten, and the chord passes through exactly as an empty row does.
- **The dropdown in `attuned` mode offers the memorised set and nothing else.** `list everything`
  still restores planning — the rest appears marked `(not attuned)` — and a pick that is not attuned
  is refused with a message instead of being stored.
- **Per-copy layouts.** A catalyst's assignments are the default for its weapon type, so upgrading a
  staff at a smith keeps them. `customise this copy` gives the one in hand a layout of its own —
  which is how two copies of one catalyst hold different spells — and `use the type default` drops
  it again. The panel says which of the two you are editing.
- **The report prints the gate's state**: `balance=attuned attuned=10 assigned=8 reachable=8 stale=0`.
  Four figures, each naming what it counts. There is deliberately no "of 12": how many memory slots
  a character has unlocked is not readable, and a guessed denominator would be wrong for anyone who
  has not collected them all.

## Changed

- **Crash reports name the caller chain.** A fault line used to end at the instruction that faulted;
  it now prints module and offset for the frames behind it, which is the difference between "the
  game crashed" and "the game's own teardown, five of its own frames deep".
- **The memorised set is re-read once a second while the panel is open**, so a grace visit or a
  talisman swap under an open panel updates the rows instead of leaving them describing the set the
  panel opened with. With the panel closed, nothing polls.

## Verified

98 unit tests green, 26 of them written before the implementation. `cargo xwin check
--target x86_64-pc-windows-msvc` clean, no warnings.

The gate was exercised on one character across both catalysts, with the report line above and
`steering: 94 of 94 magic-entry writes read back as written`. **Not tested on Windows**: every panel
and cast check in this cycle ran under Proton.
