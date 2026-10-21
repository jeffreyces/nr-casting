# Two spells per catalyst for ELDEN RING

Two staves, four spells: **R1/R2** cast the right-hand weapon's two spells and **L1/L2** the
left-hand weapon's, while leaving Elden Ring's own cast path intact — real FP costs, animations,
i-frames and requirements.

It reproduces Nightreign's casting layout — one spell on the bumper, one on the trigger, per staff —
using only Elden Ring's data.

Assignment happens in game: press the panel hotkey, pick a slot, choose a spell. Nothing in the
game's data is edited.

## Scope

- **Elden Ring data only.** Names, requirements and ownership come from your own `regulation.bin`;
  the cast is the game's own.
- **Offline only.** Launch through me3 with online play disabled so EAC never loads. Saves are
  never touched.
- No data edits: assignments live in `nr_casting/nr_casting_slots.txt` beside the DLL, and nothing in
  `regulation.bin` is modified.

## How it works

Elden Ring already casts on R1 and L1 with a catalyst in that hand — it casts whatever spell is
*selected*. R2 (heavy attack) and L2 (ash of war) do not cast. So:

| input | vanilla | with this mod |
|---|---|---|
| R1 / L1 | cast the selected spell | cast **slot 1** of that hand's weapon |
| R2 | heavy attack | cast **slot 2** of the right-hand weapon |
| L2 | ash of war | cast **slot 2** of the left-hand weapon |
| **R3 + L2** | — | **unchanged vanilla L2**, so the ash of war still fires (R3 itself is swallowed) |

Two pieces make that work:

1. **`XInputGetState` is detoured**, so R2/L2 are rewritten into shoulder presses before the game
   picks an action. The game then runs its normal cast — casting is never reimplemented. Holding
   R3 skips the rewrite for that press, which is how the ash of war keeps its own input.
2. **The selection is steered per press**: `EquipMagicData.entries[].param_id` and
   `selected_slot` are pointed at the bound spell as the input is read. Steering has to be
   press-scoped, because two inputs on the same weapon must cast different spells.

The panel is ImGui (hudhook) over the game's DX12 swap chain.

## Install

**me3** is the supported loader (Mod Engine 2 is discontinued upstream; development moved to
me3). Put the DLL in a folder and point a profile at it:

```toml
# ~/.config/me3/profiles/nr-casting.me3   (Linux; %APPDATA%\me3\profiles on Windows)
profileVersion = "v1"

[[supports]]
game = "eldenring"

[[natives]]
path = "C:\\mods\\nr-casting\\nr_casting.dll"
optional = false
enabled = true
```

Then `me3 launch -g eldenring -p nr-casting`.

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

## Configuration — `nr_casting.ini`

```ini
[ui]
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

- **The hotkey** opens the panel; while it is open the game's pointer is released and its
  mouse-look starved, and both are handed back when it closes.
- **Left column**: the left-hand weapon with dropdowns for L1/L2. **Right column**: the right-hand
  weapon with R1/R2. **Middle**: your INT/FAI/ARC, the spell currently selected in game, search,
  the HUD mirror toggle.
- Dropdowns list only spells the character **owns** and **can cast** (requirements met).
  `show everything` bypasses both for planning a build.
- **Binding without the panel**: hold the configured modifier and press R1/R2/L1/L2 to bind
  whatever spell is selected in the game's own wheel.

## Repository layout

```
mod/       the DLL: Cargo.toml, src/{lib,overlay,pointer,probe,config}.rs
tools/     gen-spell-names.py, gen-weapon-names.py, package.sh
vendor/    fromsoftware-rs, trimmed to the crates the DLL needs (see Version tolerance)
dist/      build output: the release folder produced by tools/package.sh
NEXUS.md   the mod-page description, written in BBCode for Nexus
```

Build output and unpacked game data are never stored: `mod/target/` and `work/` are generated,
and `dist/` is what a release uploads.

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
| Mod Engine 2 | discontinued upstream; the DLL loads via `LoadLibrary` and needs nothing special |
| other overlay mods | both hook DX12 Present and the window procedure — expect contention for input |
| spells/selection mods | direct conflict: both write the magic selection |
| regulation mods | no conflict today (no param edits shipped) |
| online / EAC | not supported by design |
