# v0.2.22 — catalysts that cast both schools, and a Recluse setting that never worked

Two things a player ran into, both in how the mod decides what a catalyst can do.

**Upgrade is the DLL.** One ini key is gone, and nothing needs replacing it.

## Fixed

- **A staff that casts incantations had its incantations hidden.** The mod inferred a catalyst's
  schools from the vanilla id band — a glintstone staff casts sorceries, a sacred seal casts
  incantations — which is right for the base game's 42 catalysts and wrong for the ones built to
  cast both. The Staff of the Great Beyond is vanilla and casts both, and a class port's staff does
  too, so its incantations never appeared in the picker. The answer was in the weapon row all along,
  in two flag bits the paramdef does not name: `0x80` in `bits_105` is the sorcery capability and
  `0x02` in `bits_106` the incantation one. Measured across 12 staves, 11 seals and both dual
  catalysts — staves read `0x87`/`0x10`, seals `0x07`/`0x12`, and the two dual catalysts `0x87`/`0x12`
  — so a catalyst's schools are now read from its own row, and "both" is a real answer rather than a
  guess. The bytes are read by offset, because the bindings generate them without accessors, so they
  are validated once per process against a known staff and a known seal; if a patch moves them the
  mod falls back to a shipped exception table and then to the id band, and says which it used.
- **`recluse_chr_ids` never matched anything, and is removed.** The setting was compared against
  `ChrIns.character_id`, which reads 0 on current builds, so a class port's character was only ever
  recognised by its own catalyst being missing from the vanilla weapon table. That signal is
  dynamic, needs no configuration, and is what the code now uses on its own. An existing ini keeps
  working: the key is simply ignored. The character's `NpcParam` row is logged on load instead, so a
  measured rule can replace the guess later rather than another list nobody can fill in.

- **A weapon that cannot cast offered every spell in the catalogue.** Unarmed, and any weapon the
  flags say casts nothing, came back as "offers both" — the same value that means a dual catalyst —
  so a dagger (or the Duchess's single-handed daggers) listed sorceries and incantations and accepted
  an assignment that could never cast. It now reads `not applicable`, the four rows are disabled, and
  such a weapon is no longer treated as a catalyst for the Y-chords either, even if an old
  assignment is still sitting on it.

## Changed

- **The picker label says what the weapon offers**: `sorceries only`, `incantations only`,
  `sorceries and incantations`, or `not applicable`.
- **The passthrough row names the class and says what it costs**: `Duchess class: Y+R1 is its skill -
  3 slots here`, not a hardcoded "Recluse" and not "its 4 key" — that is the port's keyboard fallback,
  while on the pad it owns `Y+RB`. A second line gives the remedy (`y_bumper = on` plus a rebind of
  the class skills in the game's own button settings), because the choice between the class skill and
  the fourth slot is otherwise invisible. The character itself cannot be asked — its id fields read 0
  on current builds — so the class is named from the port weapon in hand, in
  `nr_casting_class_ports.txt`, a text file beside the DLL you can extend for another port.
- **The panel follows the equipped pair while it is open**, so opening it during a load (or swapping a
  weapon with it up) no longer shows the hands it opened with.
- In balanced mode, the "no attuned sorcery left for this staff" notice only appears when the
  catalyst genuinely cannot cast that school — a both-school staff with nothing but incantations
  attuned is a different build, not a problem.

## Notes

Every number behind these decisions was read from the running game, and is written down with the
method used, in `docs/probed-game-facts.md`: the two catalyst flag bytes and their offsets, the row
size, the twelve staves and eleven seals they were checked against, and the nine character fields
that turned out to be zero. The rule that produced it is in there too — probe and diff two known
cases instead of trusting a field name, and validate any offset read before believing it.

## Verified

112 unit tests green (5 for the flag bits, the both-school case and the cannot-cast case; 2 for the
passthrough wording, whose figures are derived rather than typed). `cargo xwin check
--target x86_64-pc-windows-msvc` clean, no warnings. The class-port path was exercised with the
port's own staff equipped on a port character; the vanilla both-school staff was **not** tested in
game — the id and its capability come from the shipped table, and the filtering rule is unit-tested.
**Not tested on Windows.**
