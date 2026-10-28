# v0.2.5 — the name reader, hardened

A follow-up to v0.2.4. Nothing about casting, slots or chords changes, and nothing resolves to a
different name than it did in v0.2.4 — this release makes the new name reader behave under two
conditions that v0.2.4 handled badly.

**The upgrade is the DLL and nothing else.**

Offline only, as before.

## Fixed

- **A panel open during a load could stutter.** The panel asks the game for a weapon's name while
  it draws, and a lookup that found the text tables momentarily unreadable (which is what a
  reload looks like) cached nothing — so every frame re-walked and re-parsed three message layers,
  about ten thousand field reads each, on the render thread. A failed read is now remembered for
  250 ms before the next attempt, and a successful one clears it.
- **A corrupt text-table header could cost megabytes before being rejected.** The parser trusted
  the header's group count up to 262k, which meant a garbage header could make it allocate ~3 MB
  and walk ~780k fields before the structure check failed. The ceilings are now 65,536 groups and
  1M strings, and a table claiming more groups than strings is rejected outright — impossible for
  a real table, since every range covers at least one id.

## Changed

- `docs(nexus): earlier page copy from the user` — the mod page source, committed as written.

## Upgrading

Replace `nr_casting.dll`. The ini, the loader profile, your assignments and the cached spell
catalogue are all reused as they are; no file needs deleting and no setting changes.

## Notes

- The name reader still costs ~17 ms once per session (2,910 weapon and 214 spell rows on a
  Recluse install) on the maintenance thread, and nothing afterwards — answers and misses are
  cached, and the panel's render path only reads that cache.
- If the text tables cannot be read at all, every caller keeps its compiled table and unnamed
  rows show their id (`mod weapon 34070000`). The log distinguishes the two outcomes:
  `text tables proven` versus `no FMG at …: <reason>`.
- Verified on ELDEN RING 1.17.2 with the Recluse class port loaded, after the change: proof
  passes, 2,910 + 214 names in 17 ms, no fault.
