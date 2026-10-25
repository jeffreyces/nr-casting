# v0.2.1 — crashes that can be reported, and three ways the mod could cause them

This release is about the one line that matters in a bug report. A field report described the game
crashing "randomly": as me3 launched it, at the main menu, or while loading into the world. Three
of those paths were ours — all of them writes or patches landing at a moment the game was not
expecting — and the mod could not have told you which one, because a fault left no trace unless
debug logging happened to be on. Both are fixed: the causes are gone, and a fault is now recorded
automatically with the module and offset that faulted.

Offline only, as before: me3 does not load EAC, no save file is written, and nothing in the game's
files is redistributed.

## Highlights

- **A crash now writes itself down.** A vectored exception handler records the exception, the
  faulting address, the DLL that owns it, the offset inside that DLL, the thread, and — for an
  access violation — whether it was reading, writing or executing and which address it touched.
  It never handles the exception (`EXCEPTION_CONTINUE_SEARCH`), so the game's own handling, a
  debugger and any crash reporter behave exactly as before. No debug flag required.
- **The mod no longer writes game memory from its own threads.** Restoring a borrowed spell slot on
  the heartbeat or from the panel is what turned a reload or a warp into a heap corruption that
  surfaced later, somewhere unrelated.
- **The pad hook is taken back without rewriting a running function.** When another input mod owns
  the export's prologue, the game's own import slot is pointed at our detour instead: one aligned
  pointer store, no instruction bytes touched, and the displaced value becomes our trampoline so
  the other mod's hook still runs.

## Fixed

- `fix(reload): never write game memory from a timer or the panel thread` — `release_borrowed` ran
  on the heartbeat ("character gone") and on the panel thread ("panel closed"), writing into the
  very `EquipMagicData` a load, a warp or a death is freeing. `VirtualQuery` proves a page is
  committed, not that the character still owns it; the write landed in freed heap and the crash
  arrived minutes later in unrelated code. Restoration is now the pad poll's job only (the game's
  own thread), teardown drops the snapshot, and the panel touches no game memory at all. The
  restore also checks the entry still holds the spell we steered before putting the original back,
  so it can no longer clobber a value the game or another selection mod wrote.
- `harden(hooks): take the pad through the import slot, guard the raw-input reads` — the panel's
  frame loop had no panic boundary (only the window procedure did), so one bad frame killed the
  thread that owns the hotkey *and* the per-frame capture upkeep; it is caught, logged, closed,
  and the next open rebuilds the device. The raw-input detours dereferenced the caller's buffer
  without checking the size it reported. Input queues are bounded.
- `refactor(input): find the pad's import slot, and say why when it cannot` — the import walk only
  matched imports by name, and the game imports `XInputGetState` by **ordinal** (`XINPUT1_4` #2),
  so it never found the slot and every install fell back to patching the export. It now accepts
  the ordinal and reports what the walk saw. Under Proton the walk cannot work at all — the PE
  header is intact on disk (`e_lfanew 0x168`) but garbled in memory (`0x45d33ac0`) — so the log
  says which route ran instead of failing silently.
- `perf(input): stop walking the import table every two seconds when idle` — with no controller
  connected the watcher re-walked the import table forever; it now re-checks every 30 seconds.

## Added

- `feat(logging): always record a crash, with the module and offset that faulted` — the fault
  observer above; allocation-free (it runs on a stack that is already broken), never blocks
  (`try_lock`: the fault may be inside the logger), capped at eight lines, and ignores the codes
  that are normal noise (breakpoints, C++ throws, `OutputDebugString`, and the thread-name
  exception Wine raises constantly). Panics are logged with thread and location, and the previous
  hook still runs, so another mod's handler is not silently replaced.
- The `modules:` startup line now carries each module's load base
  (`ersc.dll@0x180000000, d3d12.dll@0x6ffffa680000, …`), which is what maps a `crash:` line back
  to a DLL.

## Changed

- The previous writer of the panel's panic hook is gone: `std::panic::set_hook` replaced any other
  mod's handler and relabelled every panic in the process as ours. Panics are now logged and the
  chain is preserved.

## Tests

- 17 host-side unit tests (up from 14): the existing slot-file and routing suites, plus three over
  the crash-line format — module and offset, the access-violation verb and target, and a truncated
  buffer that must not overrun.

## Upgrading

Replace the DLL. Nothing else changes: `nr_casting.ini`, `nr_casting_slots.txt` and the profile are
read as before, and assignments keep working.

If you are coming from v0.1.0, also read the v0.2.0 notes: the panel is a real window now,
`overlay = auto` is read as `on`, and each catalyst has three slots.

## Compatibility

me3 (including the ME3 Manager), frame-generation and upscaling proxies, class ports with their own
HUDs, other overlay mods, and Seamless Co-op loaded as a me3 native — verified with Seamless Co-op
2.0.1, the Recluse port and this DLL in one process, through world loads and quit-to-menu. Direct
conflict remains with mods that write the game's magic selection. Online play is not supported.

## Reporting a crash

Attach these lines from `nr_casting\nr_casting_probe.log` (or `%APPDATA%\nr_casting\`, whichever
the `files in:` line names):

```
modules: ...
crash: exception 0xc0000005 at 0x… in <dll> (base 0x…, offset 0x…) thread … writing 0x…
```

The first says what was loaded and where; the second says which of those DLLs faulted and where
inside it.
