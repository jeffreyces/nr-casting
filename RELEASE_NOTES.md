# v0.2.0 — three spells per catalyst, and a panel that behaves like a window

This release doubles down on the original idea: every catalyst now carries **three** spells
instead of two, the assignment panel moved out of the game's swap chain into a real window of
its own, and the same six slots are reachable from **mouse and keyboard** without any extra
setup. Under the hood the input hook now *chains* other mods' hooks instead of replacing them,
every game-memory read is validated before it is dereferenced, and the crate was split into
focused modules with a unit-tested core.

Offline only, as before: me3 does not load EAC, no save file is written, and nothing in either
game's files is redistributed.

## Highlights

- **Six spells on two staves.** Bumper and trigger per hand, plus a third slot on the Y chord
  (**Y+R2**, **Y+L2**). Two-handing a staff does nothing, so that input now carries a spell.
- **The panel is its own window.** A borderless, resizable, draggable Win32 window with its own
  D3D11 device — no `Present` hook, so a class port's HUD and frame-generation mods keep working
  while it is open.
- **Mouse and keyboard parity.** LMB / RMB / Shift / Ctrl map to the same six slots; clicks are
  ignored in menus and while the game is unfocused.
- **Coexistence.** `XInputGetState` hooks chain: another mod's detour is captured as our
  trampoline and re-installed over, so neither mod goes dead.
- **Verified before it ships.** 14 host-side unit tests over slot parsing and input routing;
  warning-free clippy build; no `unwrap`/`expect` on any runtime path.

## Added

- `feat!: standalone panel, third spell slot, mouse and keyboard parity` — the panel is a
  standalone D3D11 window; every catalyst gains a third slot on Y+trigger; all six slots are
  reachable from mouse and keyboard.
- `feat(panel): resizable, draggable window without the empty lower half` — `WS_THICKFRAME`,
  a draggable header band, a 520×320 floor, and a default height that matches the content.

## Fixed

- `fix(panel): release the render target, pin the D3D11 feature levels, restore capture upkeep`
  — `ResizeBuffers` was called with a live render target (DXGI rejects it, and the error was
  discarded), the requested feature levels were never passed to `D3D11CreateDeviceAndSwapChain`,
  and the per-frame capture release that Wine needs was no longer running.
- `input: hook the pointer the game actually calls, re-target it when another mod moves it, and
  re-install after MinHook-style prologue overwrites` — fixing "the mod loads but triggers stop
  casting" when another input mod is present.
- `memory safety: validate every game pointer with VirtualQuery before deref` — player, game
  data, `EquipMagicData` and param files; the spell-slot borrow is fallible, so an empty list
  cannot panic, and the inventory and param walks are bounded.
- `reload safety: gate every game-structure read behind a readability check, release the borrow
  and log when the world vanishes, and never fabricate a shoulder press while Y is held` — a
  reload frees the character underneath the mod.
- `Chain other input hooks instead of replacing them; skip the panel when another overlay owns
  the frame` — the last of the coexistence work before the panel moved to its own window.

## Changed

- `refactor: split the DLL into focused modules` — `lib.rs` went from 1,160 lines to a 296-line
  facade over `input`, `game`, `store` and `logging`; visibility is `pub(crate)` only where
  another module needs it.
- `refactor(input): split adjust into named steps and unhook the panel columns' duplication` —
  routing, capture, the Y chord and the ash-of-war escape are named functions with a tested
  decision table.
- `compat + install: ship a profile with a relative path, fall back to %APPDATA% when the DLL
  folder is read-only, log the foreign DLLs loaded in-process, and stop writing the menu's magic
  selection`.

## Tests

- `test(core): extract slot parsing and input routing into a testable crate` — `mod/pure`
  (`nr_casting_core`) has no Win32 and no game dependency, so `cargo test` covers the slot file
  format (including legacy two-value lines) and the routing precedence: ash of war, then
  binding, then the Y chord, then a plain cast.

## Docs

- `docs: describe the standalone panel, the Y chord and the mouse mapping` — README and the Nexus
  page now carry a pad / mouse / what-it-casts table, the panel-window explanation, and the
  `panel:` log lines that replaced `overlay: FAILED`.
- Install note and README document the ME3 Manager: it writes the profile itself, `start_online`
  is a profile field (leave it off), and a Manager update replaces the mod folder — taking
  `nr_casting.ini` and `nr_casting_slots.txt` with it.

## Breaking changes

- **The panel is no longer an overlay on the game's swap chain.** Nothing else changes about
  casting, but another overlay no longer conflicts with it, and the panel can be moved and
  resized like a normal window.
- **`overlay = auto` is gone.** me3 has nothing to defer to any more, so the old default is read
  as `on` (and logged as such) instead of silently turning the panel off. New installs ship
  `overlay = on`. The value `off` still means casting-only.

## Upgrading

Replace the DLL and `nr_casting.ini`. Existing `nr_casting_slots.txt` files keep working: three
values per line are written from now on, and two-value lines are read with the third slot
unbound.

## Compatibility

me3 (including the ME3 Manager), frame-generation and upscaling proxies, class ports with their
own HUDs, and other overlay mods. Direct conflict remains with mods that write the game's magic
selection. Online play is not supported.
