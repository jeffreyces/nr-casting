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
- **Names follow your language, not ours.** The compiled tables are English, so a German, French
  or Japanese install used to read English names in the panel. Every row the game can name is now
  taken from the game's own text at startup — about 3,100 rows in 17 ms, once per session — and the
  table is only the fallback for rows the game does not name. That is also what makes a content
  mod's items read whatever the mod calls them, in whatever language the mod ships.
- **The compiled tables stay as the fallback.** Nothing is used until two known rows resolve to
  two distinct, non-empty strings from a coherent group table; if a future game build changes the
  layout, the mod says so in the log and keeps naming from the table it shipped with. A row whose
  text is the game's own `[ERROR]` placeholder is treated as nameless rather than shown as a title.

## Changed

- `feat(names): read item names from the game's own text tables` — the repository walk, proven
  against known rows before it is used for anything.
- `feat(names): reject the game's [ERROR] marker, fill names before the catalogue is written` —
  the cached `nr_casting_catalogue.txt` now carries game-supplied spell names.
- `fix(panel): retry the text-table name fill on every panel open` — a panel opened before the
  tables were readable no longer keeps placeholders for the session.

## Upgrading

Replace `nr_casting.dll`. The ini, the loader profile and your assignments are unchanged. The
cached spell catalogue is rebuilt once after upgrading, and again whenever the executable **or the
regulation the game actually loads** changes — that is what keeps a swapped `regulation.bin` (how
every overhaul ships its data) from leaving a mod's added spells out of the panel. The file is
otherwise reused untouched: it holds only what the params say, never names.

## Notes

- The lookup costs about 10 ms on the first panel open of a session and nothing after that:
  answers and misses are both cached, and the panel's render path only ever reads that cache.
- If the text tables cannot be read — an unknown future game build, a reload, a mod that moved
  them — nothing breaks: every caller keeps its compiled table and rows without a name show their
  id (`mod weapon 34070000`), exactly as before this release. The log says which of the two
  happened (`text tables proven` versus `no FMG at …` with the reason).
- Play-tested on ELDEN RING 1.17.2 with the Recluse class port loaded: 3637 live weapon rows,
  3448 named from the shipped table, 81 more named from the game's own text.
