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

## Changed

- **The picker label says what the catalyst offers**: `sorceries only`, `incantations only`, or
  `sorceries and incantations`.
- In balanced mode, the "no attuned sorcery left for this staff" notice only appears when the
  catalyst genuinely cannot cast that school — a both-school staff with nothing but incantations
  attuned is a different build, not a problem.

## Verified

109 unit tests green (4 of them for the flag bits and the both-school case). `cargo xwin check
--target x86_64-pc-windows-msvc` clean, no warnings. The class-port path was exercised with the
port's own staff equipped on a port character; the vanilla both-school staff was **not** tested in
game — the id and its capability come from the shipped table, and the filtering rule is unit-tested.
**Not tested on Windows.**
