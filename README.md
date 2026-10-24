# Nightreign-style casting for ELDEN RING

**R1/R2** cast the right-hand weapon's first two spells and **L1/L2** the left-hand weapon's,
while leaving Elden Ring's own cast path intact — real FP costs, animations, i-frames and
requirements. Holding **Y** turns each trigger into a third slot (**Y+R2**, **Y+L2**), so two
staves carry six spells.

It reproduces Nightreign's casting layout — one spell on the bumper, one on the trigger, per staff —
using only Elden Ring's data. The Y chord is the one addition: two-handing a staff does nothing, so
that input is spent on a spell instead.

Assignment happens in game: press the panel hotkey, pick a slot, choose a spell. Nothing in the
game's data is edited.

## Scope

- **Elden Ring data only.** Names, requirements and ownership come from your own `regulation.bin`;
  the cast is the game's own.
- **Offline only.** Launch through me3 with online play disabled so EAC never loads. Saves are
  never touched.
- No data edits: assignments live in `nr_casting/nr_casting_slots.txt` beside the DLL, and nothing in
  `regulation.bin` is modified.
- **Non-destructive.** No save file is ever written and no game data is modified. While casting, the
  mod borrows the spell slot the game already has selected and restores it as soon as the input is
  released, so nothing lingers in memory either. Remove the DLL and the game behaves exactly as it
  did before; delete `nr_casting/` and every trace of the mod is gone.

## How it works

Elden Ring already casts on R1 and L1 with a catalyst in that hand — it casts whatever spell is
*selected*. R2 (heavy attack) and L2 (ash of war) do not cast. So:

| input | mouse / keyboard | vanilla | with this mod |
|---|---|---|---|
| R1 (right hand) | left click | cast the selected spell | cast **slot 1** of the right-hand weapon |
| L1 (left hand) | right click | cast the selected spell | cast **slot 1** of the left-hand weapon |
| R2 | shift + left click | heavy attack | cast **slot 2** of the right-hand weapon |
| L2 | shift + right click | ash of war | cast **slot 2** of the left-hand weapon |
| **Y + R2** | ctrl + left click | two-hand stance, then heavy attack | cast **slot 3** of the right-hand weapon |
| **Y + L2** | ctrl + right click | — | cast **slot 3** of the left-hand weapon |
| **R3 + L2** | — | — | **unchanged vanilla L2**, so the ash of war still fires (R3 itself is swallowed) |

Two-handing a staff is pointless, so the two-hand chord is spent on a third spell per catalyst
instead: **Y + R2** and **Y + L2** carry their own slot, giving six spells across two staves.
That press is not rewritten into a shoulder press while Y is held — the mod clears Y for the
frame so the game performs a plain cast, which also keeps a class-port mod's `Y + RB` / `Y + LB`
skill chords working (they read the raw pad, beneath our hook).

The mouse bindings are the same six slots, with no separate configuration: the game performs the
attack from its own click and the mod steers the selection, exactly as it does for a pad press.
Clicks are ignored while the panel is open or another window has focus.

Two pieces make that work:

1. **`XInputGetState` is detoured**, so R2/L2 are rewritten into shoulder presses before the game
   picks an action. The game then runs its normal cast — casting is never reimplemented. Holding
   R3 skips the rewrite for that press, which is how the ash of war keeps its own input.
2. **The selection is steered per press**: `EquipMagicData.entries[].param_id` and
   `selected_slot` are pointed at the bound spell as the input is read. Steering has to be
   press-scoped, because two inputs on the same weapon must cast different spells.

The panel is Dear ImGui drawn into a borderless window of the mod's own, with its own D3D11 device
created the first time you open it. It is deliberately *not* an overlay on the game's swap chain:
a second `Present` hook stops a co-resident class port's HUD from drawing (verified with
`ERNativeUI.dll`), even when that hook draws nothing at all.

## Install

**me3** is the supported loader (Mod Engine 2 is discontinued upstream; development moved to
me3). The shipped profile needs no editing:

```
mods/nr-casting/
├── nr-casting.me3      path = "nr_casting.dll"  ← relative to this file
├── nr_casting.dll
├── nr_casting.ini
└── INSTALL.txt
```

```bash
me3 launch -g eldenring -p "C:\mods\nr-casting\nr-casting.me3"
```

me3 resolves a relative native path against the **profile's own folder**, so keeping the DLL
next to the profile is all it takes. Passing the profile by path also means it does not have to
live in me3's profile directory.

If you would rather keep the profile with me3's others (`%APPDATA%\me3\profiles\`), give the
`path` line the DLL's absolute location — in **single quotes**, so Windows backslashes stay
literal:

```toml
path = 'C:\mods\nr-casting\nr_casting.dll'     # single quotes: one backslash each
path = "C:/mods/nr-casting/nr_casting.dll"       # or forward slashes
```

The path ends at the `.dll` file, not at the folder holding it. Adding the DLL through
**me3 Manager** works too: the manager writes its own profile.

A release is four files — `nr_casting.dll`, `nr-casting.me3`, `nr_casting.ini`, `INSTALL.txt`.
Everything else the mod needs it generates itself in an **`nr_casting/` subfolder beside the
DLL**, so a shared mods directory never collects our files and uninstalling is one delete.
Files from older builds (beside the DLL, or beside `eldenring.exe`) are moved in once.

| file | what it is | created by |
|---|---|---|
| `nr_casting/nr_casting_catalogue.txt` | spell catalogue read from the running game | mod, first launch |
| `nr_casting/nr_casting_slots.txt` | weapon → two spell ids | mod, on first bind |
| `nr_casting/nr_casting_build.txt` | executable fingerprint the catalogue belongs to | mod |
| `nr_casting/nr_casting_probe.log` | log, rotated at 4 MiB | mod |
| `nr_casting/nr_casting_offsets.txt` | resolved RVA table + validation results | mod |
| `nr_casting.ini` | configuration (beside the DLL, as shipped) | package |
| `nr_casting_spells.txt` | optional override: spell names | `tools/gen-spell-names.py` |
| `nr_casting_weapons.txt` | optional override: weapon names | `tools/gen-weapon-names.py` |

The name tables are compiled into the DLL and only exist as files if you drop an override
beside it. Settings are read from the shipped `nr_casting.ini` first, so editing it always
takes effect even if an older copy sits in the subfolder.

### Installing through the ME3 Manager

The graphical [ME3 Manager](https://github.com/2Pz/me3-manager) is a front end over the same me3:
it writes the `.me3` profile for you and lists the DLL under `[[natives]]` (relative to the
profile, or by absolute path when the file lives outside its mods folder). Nothing in the DLL
reads the profile, so a Manager install and a hand-written one behave identically — the mod finds
its own folder from its module path and writes its files there.

Two Manager behaviours are worth knowing:

- **`start_online` is a profile field.** me3 keeps matchmaking off by default; if you turn that
  option on in the Manager, the game runs online and this mod is not supported there.
- **Updating a mod replaces its folder**, so `nr_casting.ini` and `nr_casting_slots.txt` go with
  it. Copy them somewhere safe before updating, or keep the assignments in a folder you point me3
  at yourself.

## Configuration — `nr_casting.ini`

```ini
[ui]
overlay = on             ; on | off — the panel window (casting works either way)
hotkey = F8              ; F1-F24, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, TAB

[input]
bind_modifier = L3       ; L3, R3, LB, RB, DPAD_UP, DPAD_DOWN

[display]
hud_mirror = off         ; show slot 1 on the game HUD (locks the D-pad wheel while on)

[logging]
debug_log = off          ; log every cast, not just captures and errors
```

Malformed values are logged and the default kept.

## Using the panel

The panel is optional. Assignment also works without it: select the spell you want in the game's
menu, then hold the bind modifier and press the bumper/trigger you want it on. The panel exists to
save that trip through the menu.

`overlay` decides whether the hotkey can open the panel at all:

| value | behaviour |
|---|---|
| `on` (default) | the hotkey opens the panel window |
| `off` | never opens; casting only |

The window is created on the first keypress and destroyed on exit. It is topmost but entirely
separate from the game — no swap-chain hook, no render-target grab — so another overlay or a class
port's HUD keeps drawing while it is open. The trade is a second GPU device the first time you open
it (a few hundred milliseconds), which is why it is lazy.

With the panel installed:

- **The hotkey** opens the panel; while it is open the game's pointer is released and its
  mouse-look starved, and both are handed back when it closes.
- **Left column**: the left-hand weapon with dropdowns for L1 / L2 / Y+L2. **Right column**: the
  right-hand weapon with R1 / R2 / Y+R2. Each row shows its mouse binding beside the pad one.
  **Middle**: your INT/FAI/ARC, the spell currently selected in game, search, the HUD mirror
  toggle. A footer spells out both mappings.
- Dropdowns list only spells the character **owns** and **can cast** (requirements met).
  `show everything` bypasses both for planning a build.
- **Binding without the panel**: hold the configured modifier and press R1/R2/L1/L2 to bind
  whatever spell is selected in the game's own wheel. The third slot is bound from the panel only —
  the modifier gesture would collide with Y.

## Repository layout

```
mod/       the DLL: Cargo.toml, src/{lib,window,overlay,pointer,probe,config}.rs
tools/     gen-spell-names.py, gen-weapon-names.py, package.sh
vendor/    fromsoftware-rs trimmed to the crates the DLL needs, and imgui-dx11-renderer with its
           shaders compiled at runtime instead of by `fxc` in a build script
dist/      build output: the release folder produced by tools/package.sh
NEXUS.md   the mod-page description, written in BBCode for Nexus
```

Build output and unpacked game data are never stored: `mod/target/` and `work/` are generated,
and `dist/` is what a release uploads.

### Tests

The two pieces worth testing — the slot-file format and the input routing table — live in
`mod/pure` (`nr_casting_core`), which has no Win32 and no game dependency, so it runs anywhere:

```bash
cd mod/pure && cargo test
```

That covers which input means which slot, which input wins when several are held (ash of war,
then binding, then the Y chord, then a plain cast), the mouse mapping, and slot files written by
older versions. Everything else needs the live game.

## Building

```bash
./tools/package.sh     # build + assemble dist/nr_casting (the release folder)
```

Or by hand:

```bash
cd mod
cargo xwin build --release --target x86_64-pc-windows-msvc   # Linux cross-compile
cargo build --release --target x86_64-pc-windows-msvc       # native Windows
```

### Releases

Pushing a `v*` tag builds and publishes: `.github/workflows/release.yml` cross-compiles the DLL,
runs `tools/package.sh`, and attaches `nr-casting-<tag>.zip` to a GitHub release. The tag has to
match `version` in `mod/Cargo.toml` — the job stops before building if it does not. Running the
workflow by hand uploads the same archive as a build artifact without a release.

The job runs on `ubuntu-slim` (1 vCPU container, free on public repos) rather than a 4 vCPU VM,
with `cargo-xwin`, its MSVC payload and the dependency graph cached, so a warm run is mostly
download time. A cold single-threaded build of this crate measures ~48 s, comfortably inside that
runner's 15-minute cap.

`fromsoftware-rs` is vendored in [`vendor/`](vendor/) and wired in through `[patch]` in
`mod/Cargo.toml`; the vendor copy differs from upstream only in version handling (below).

### Generating the name tables

Both come from your own unpacked regulation, so they match your game version:

```bash
WitchyBND --passive regulation.bin                        # → regulation-bin/*.param.xml
python3 tools/gen-spell-names.py  regulation-bin/Magic.param.xml            nr_casting_spells.txt
python3 tools/gen-weapon-names.py regulation-bin/EquipParamWeapon.param.xml nr_casting_weapons.txt
```

Names come from the Paramdex annotation on each row (`paramdexName`), which is why no FMG
extraction, encrypted archive handling or Oodle decompression is involved. Without the tables
the mod still works and the panel shows bare ids.

## Version tolerance

`fromsoftware-rs` maps an exact exe version to an RVA table and panics otherwise. The vendored
copy **falls back to the newest known table** for an unrecognised version and records what it
detected; nothing is written to game memory until a structural probe passes (weapon ids in
range, stats plausible, 14 magic slots with a valid selection). On failure the mod stays out of
the way and `nr_casting_offsets.txt` names the check that failed. `NR_CASTING_FORCE_RVA=1`
exercises the fallback on a known-good build.

Built against ELDEN RING exe `2.7.2.0` (RVA table `Ww2720`).

## Compatibility

| layer | notes |
|---|---|
| me3 | supported, tested |
| frame-generation / upscaling proxies (FSR/FG `d3d12.dll`, `dxgi.dll`, `erss-fg.dll`) | they own frame presentation; the panel may not appear, casting still works |
| class ports with their own HUD (`ERNativeUI.dll` and friends) | no conflict: the panel is its own window, so their HUD keeps drawing |
| Mod Engine 2 | discontinued upstream; the DLL loads via `LoadLibrary` and needs nothing special |
| other overlay mods | no swap-chain conflict; both drive the pointer while their window is open, so expect contention there |
| spells/selection mods | direct conflict: both write the magic selection |
| regulation mods | no conflict today (no param edits shipped) |
| online / EAC | not supported by design |

## Troubleshooting

The mod logs what it found, in order, to `nr_casting/nr_casting_probe.log` beside the DLL (or
`%APPDATA%\nr_casting\` if that folder is not writable — the log's second line, `files in:`,
says which). Two lines answer most reports:

```
modules: amd_ags_x64.dll, d3d12.dll, dxgi.dll, erss-fg.dll, me3_mod_host.dll, ...
files in: C:\mods\nr-casting\nr_casting
```

`modules` lists every DLL loaded besides Windows' own, so a proxy DLL, another overlay or a
class-port mod shows up by name. If the panel never appears, look for `panel:` —
`panel: no panel graphics; staying closed` or `panel: D3D11 device creation failed` mean the window
could not get a device (casting is unaffected), and `panel: first frame presented` confirms it
drew. If nothing loads at all, the profile path is
the first suspect: the log will not exist.
