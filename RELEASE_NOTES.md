# v0.2.4 — modded items get their real names

A small release with one visible effect: a weapon or spell that a mod adds now shows the name the
game shows, instead of `mod weapon 33790000`.

Nothing about casting changes. Same four slots per catalyst, same chords, same files —
**the upgrade is the DLL and nothing else.**

Offline only, as before.

## Highlights

- **Names come from the game, not from a table.** Item names are not in the params and not in
  `regulation.bin`: the game reads them from FMG text (`msg/<language>/*.msgbnd.dcx`), and a mod
  that adds items ships its own copy of those files. The mod now reads that text out of the
  running game, so a class port's own staff reads **Recluse's Staff** instead of an id, and a total
  conversion's weapons read whatever the conversion calls them. This also picks up DLC weapons and
  infusion variants the compiled table never had (`Magic Silver Grooved Shield`,
  `Blood Reed Great Katana`).
- **Every message layer is read, not just the base game's.** The runtime keeps one text table per
  layer (base, `_dlc01`, `_dlc02`) and merges them at lookup time; the mod walks them in the same
  order, which is where overhaul mods put their added strings.
- **The compiled tables stay as the fallback.** Nothing is used until two known rows resolve to
  their known text (`Scimitar`, `Adula's Moonblade`); if a future game build changes the layout,
  the mod says so in the log and keeps naming from the table it shipped with. A row whose text is
  the game's own `[ERROR]` placeholder is treated as nameless rather than shown as a title.

## Changed

- `feat(names): read item names from the game's own text tables` — the repository walk, proven
  against known rows before it is used for anything.
- `feat(names): reject the game's [ERROR] marker, fill names before the catalogue is written` —
  the cached `nr_casting_catalogue.txt` now carries game-supplied spell names.
- `fix(panel): retry the text-table name fill on every panel open` — a panel opened before the
  tables were readable no longer keeps placeholders for the session.

## Upgrading

Replace `nr_casting.dll`. The ini, the loader profile and your assignments are unchanged. The
cached spell catalogue is rebuilt once on the first launch after upgrading (its format marker
changed), which happens on its own.

## Notes

- The lookup costs about 10 ms on the first panel open of a session and nothing after that:
  answers and misses are both cached, and the panel's render path only ever reads that cache.
- Play-tested on ELDEN RING 1.17.2 with the Recluse class port loaded: 3637 live weapon rows,
  3448 named from the shipped table, 81 more named from the game's own text.
