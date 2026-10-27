# Nightreign-style casting for ELDEN RING

**R1/R2** cast the right-hand weapon's first two spells and **L1/L2** the left-hand weapon's,
while leaving Elden Ring's own cast path intact — real FP costs, animations, i-frames and
requirements. Holding **Y** adds two more: **Y+R2 / Y+L2** for the third slot and **Y+R1 / Y+L1**
for the fourth, so two staves carry eight spells.

It reproduces Nightreign's casting layout — one spell on the bumper, one on the trigger, per staff —
using only Elden Ring's data. The Y chords are the addition: **Y + R2 / Y + L2** is a third slot and
**Y + R1 / Y + L1** a fourth. On a staff or seal all four of those chords are the mod's — Y is
dropped from the report and the game sees a plain cast — while **any other weapon keeps the game's
press entirely, so two-handing works exactly as it always did**, mid-fight, without opening a menu.

The fourth slot is shared with [Recluse](https://www.nexusmods.com/eldenring/mods/11410), which
uses Y+R1 / Y+L1 itself. `y_bumper = auto` (the default) takes those chords **unless** the Recluse
mod is loaded *and* the character is a Recluse, so you get eight spells everywhere else and the
class port keeps its own chords on its own character. `on` claims them regardless, `off` leaves
them to the game.

Assignment happens in game, two ways: press the panel hotkey and pick a slot, or hold **L3** (or
**Super**, on mouse and keyboard) and press the input you want to bind. Nothing in the game's data
is edited.

## Scope

- **Elden Ring data only.** Names, requirements and ownership come from your own `regulation.bin`;
  the cast is the game's own.
- **Offline only.** Launch through me3 with online play disabled so EAC never loads. Saves are
  never touched.
- No data edits: assignments live in `nr_casting/nr_casting_slots_<character>.txt` beside the DLL, and nothing in
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
| Pad | Mouse | Without the mod | With the mod |
|---|---|---|---|
| R1 | left click | cast the selected spell | cast **slot 1** of the right-hand weapon |
| R2 | shift + left click | heavy attack | cast **slot 2** of the right-hand weapon |
| **Y + R2** | ctrl + left click | two-hand stance, then heavy attack | cast **slot 3** of the right-hand weapon |
| **Y + R1** | alt + left click | two-hand the right-hand weapon | cast **slot 4** of the right-hand weapon |
| L1 | right click | cast the selected spell | cast **slot 1** of the left-hand weapon |
| L2 | shift + right click | ash of war | cast **slot 2** of the left-hand weapon |
| **Y + L2** | ctrl + right click | — | cast **slot 3** of the left-hand weapon |
| **Y + L1** | alt + right click | two-hand the left-hand weapon | cast **slot 4** of the left-hand weapon |

Two-handing a staff is pointless, so those chords are spent on spells instead: **Y + R2 / Y + L2**
carry a third slot and **Y + R1 / Y + L1** a fourth, giving eight spells across two staves. A chord
that carries a spell is never handed to the game as Y plus a shoulder press — the mod drops Y from
the report and lets the game see a plain cast.

**Two-handing** is decided by what is in the hand, not by what is bound to a slot. A catalyst
(a glintstone staff or a sacred seal — param id bands `33xxxxxx` / `34xxxxxx`) never two-hands: all four of its chords belong to the mod, so a bound slot casts and
an empty one does nothing. Any other weapon keeps the game's press entirely — Y + R1/R2 two-hands
the right-hand weapon and Y + L1/L2 the left, exactly as without the mod, even if a stray assignment
sits on its slots.

The fourth slot is shared with [Recluse](https://www.nexusmods.com/eldenring/mods/11410), which uses
Y+R1 / Y+L1 itself. `y_bumper = auto` takes those chords **unless** the Recluse mod is loaded and
the character is a Recluse, where they pass straight through to the class port instead — eight
spells everywhere else, its own chords on its own character.

The mouse uses the same eight slots with no separate configuration, and it *always* uses its own
layer: the mod steers the selected spell and the game's own click performs the cast, with or without
a controller attached. Nothing is synthesised for a click, so one click is one action. Shift is
hidden from the game while the mod owns it (the game binds Shift + click to the strong attack, which
does not cast); Ctrl and Alt are ignored by the game already.
Clicks are ignored while the panel is open or another window has focus. With **no controller
connected** the mod hides Shift from the game (`GetAsyncKeyState` and its raw keyboard records), so
Shift + click arrives as a plain click and casts instead of being read as the game's strong attack;
Ctrl and Alt are ignored by the game already, which is why they can carry slots too.

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
| `nr_casting/nr_casting_slots_<character>.txt` | weapon → four spell ids, per character | mod, when you assign in the panel |
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
- **Updating a mod replaces its folder**, so `nr_casting.ini` and the `nr_casting_slots_*.txt` files go with
  it. Copy them somewhere safe before updating, or keep the assignments in a folder you point me3
  at yourself.

## Configuration — `nr_casting.ini`

```ini
[ui]
overlay = on             ; on | off — the panel window (casting works either way)
hotkey = F8              ; F1-F24, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, TAB
show_all = off           ; list spells the character cannot cast yet (planning mode)
hud_mirror = off         ; on = force the game's wheel to slot 1 (fights your own scrolling)

[input]
bind_modifier = L3       ; L3 | R3 | LB | RB | DPAD_UP | DPAD_DOWN | none
bind_modifier_kb = Super ; Super | Menu | F9 | F10 | F11 | F12 | none — held with a click
chord_stage = 2          ; reports a chord spends with Y released before casting (2 safe, 1 faster)

[recluse]
y_bumper = auto          ; auto | on | off — who gets Y+R1 / Y+L1
recluse_chr_ids = 8131   ; character ids that count as a Recluse

[logging]
debug_log = off          ; log every cast, not just captures and errors
```

Malformed values are logged and the default kept.

## Using the panel

Assignments are made in the panel or with the bind gesture; both write the same per-character file.

**The bind gesture** is the quick way, and the only way to assign a spell your character can cast
because of a Great Rune or a stat talisman: while held, the mod reads the spell *the game's own
wheel* has selected and writes it to the slot you press.

| input | mouse / keyboard |
|---|---|
| hold **L3** + R1 / R2 / L1 / L2 | hold the keyboard modifier + the same click |
| hold **L3** + Y + R2 / Y + L2 | modifier + ctrl + click |
| hold **L3** + Y + R1 / Y + L1 | modifier + alt + click |

The press is swallowed, so binding never also casts, and the modifier itself does not reach the
game. The two modifiers are configured separately — `bind_modifier` for the pad, `bind_modifier_kb`
for the keyboard (default **Super**, but KDE and GNOME claim that key, so `Menu` or `F9`-`F12` is the
safer choice on Linux). Either can be `none`, and the keyboard gesture works regardless of whether a
controller is attached.

`overlay` decides whether the hotkey can open the panel at all:

| value | behaviour |
|---|---|
| `on` (default) | the hotkey opens the panel window |
| `off` | never opens; casting only |

The window is created on the first keypress and destroyed on exit. It is topmost and created as a
tool window, so it never appears in the Windows alt-tab list or on the taskbar — switching away
behaves as it would from the game, and Alt closes the panel so focus always lands back in Elden
Ring. It is entirely separate from the game — no swap-chain hook, no render-target grab — so
another overlay or a class port's HUD keeps drawing while it is open. The trade is a second GPU
device the first time you open it (a few hundred milliseconds), which is why it is lazy.

With the panel installed:

- **The hotkey** opens the panel; while it is open the game's pointer is released and its
  mouse-look starved, and both are handed back when it closes. **Drag the title band** to move the
  window anywhere on screen; the edges resize it.
- **Left column**: the left-hand weapon with dropdowns for L1 / L2 / Y+L1 / Y+L2. **Right
  column**: the right-hand weapon with R1 / R2 / Y+R1 / Y+R2 (the fourth row first, matching
  slot order). Each row shows its mouse binding beside the pad one.
  **Middle**: the character whose assignments are being edited, your INT/FAI/ARC, the spell
  currently selected in game, search, `refresh` and `reset slots`. A footer spells out both
  mappings.
- Dropdowns list only spells the character **owns** and **can cast** — requirements are checked
  against INT/FAI/ARC *including active effects*, so a Great Rune or a stat talisman counts and the
  stats line says when it is doing the work. `show everything` bypasses both for planning a build.
- **Assignments survive upgrades.** They are filed on the weapon's base id (`33090000`, not
  `33090010`), so taking a staff to the smith no longer empties it. Slots files written by older
  versions are folded onto base ids the first time they are read.
- **On a Recluse character** the fourth row is greyed out and labelled *passthrough*: the chord
  goes to the class port, which binds it to its own `4` (Y+R1) and `5` (Y+L1) keys. With a
  controller the pad report is simply left alone, so the port sees Y+R1 / Y+L1 itself; on
  keyboard and mouse the Alt click is swallowed and `4` / `5` is pressed in its place — passing
  the click through would only make the game perform a plain attack, since it ignores Alt.
- **The HUD mirror is off by default.** With `hud_mirror = on` the right-hand weapon's first slot is
  forced onto the game's own spell wheel so its HUD shows what R1 will cast — but that rewrites
  your selection ~30 times a second, so the wheel visibly cycles and a D-pad scroll is fought every
  frame. Leave it off if you pick spells yourself (the default).

## Assignments are per character

Spells are filed under the character's name, not just the weapon: `nr_casting_slots_<character>.txt`.
The same catalyst on two characters therefore keeps two different sets of spells, which matters
because the second character may not meet the requirements of the first one's picks — the panel's
"owns and can cast" filter is only meaningful for the character you are playing.

The panel's character line shows the name, so you can see which file is being edited. A new
character starts with no assignments.

The identity is the character's **name plus save slot** and nothing else. That is deliberate: the
game exposes no per-character id and no creation timestamp (`packed_time_stamp` reads 0, and
`character_id` is the slot index), and the fields that do differ — level, stats, appearance —
change while you play, so using them would orphan the file the first time you levelled up.

Two consequences:

- **Deleting a character and creating one with the same name reuses its file.** That is the
  accepted trade for never losing assignments mid-play.
- **Assignments a character cannot cast are dropped when it loads**, which is what makes the
  shared-file case harmless: a fresh character never inherits spells its stats cannot use. The drop
  is skipped while `show everything` is enabled, since that is planning mode and uncastable picks
  are the point. That toggle is saved to the ini (`show_all`), so planning mode survives a restart.

`reset slots` in the panel clears the loaded character's assignments outright, for when you want a
character to start from nothing.

## Recluse compatibility

[Recluse](https://www.nexusmods.com/eldenring/mods/11410) uses **Y+R1 / Y+L1** on the pad and
**4 / 5** on the keyboard for its own actions. Those are the same inputs this mod would use for the
fourth spell, so ownership is decided per character:

| Input | Recluse installed, normal character | Recluse installed, Recluse character |
|---|---|---|
| Y+R1 (pad) | cast slot 4 | pad report left untouched — the port's own Y+R1 |
| Y+L1 (pad) | cast slot 4 (left hand) | pad report left untouched — the port's own Y+L1 |
| Alt + left click | cast slot 4 | click swallowed, the port's `4` key is pressed |
| Alt + right click | cast slot 4 (left hand) | click swallowed, the port's `5` key is pressed |
| Everything else | nr-casting | nr-casting |

A click cannot simply be passed through on keyboard and mouse: the game ignores Alt, so it would
perform a plain attack. Instead the click is removed from the report and the port's key is sent in
its place. With a controller there is nothing to translate — the port reads the pad itself.

A character counts as a Recluse when the `recluse_input.dll` / `recluse_ui.dll` modules are loaded
**and** either the character id matches `recluse_chr_ids` (default `8131`) or an equipped catalyst
is missing from the vanilla weapon table, which is how the class port's own catalysts are
recognised.

```ini
[recluse]
y_bumper = auto          ; auto | on | off
recluse_chr_ids = 8131
```

`auto` is the table above; `on` claims the chords even on a Recluse character (the class port then
loses them); `off` never claims them, so the chords stay the game's two-hand command everywhere.

## Repository layout

```
mod/       the DLL:
  src/lib.rs        entry point, init sequence, module bookkeeping
  src/input.rs      the XInput hook, hook chaining and the cast/steer logic
  src/rawinput.rs   the keyboard/mouse path: raw input, Shift hiding, mouse steering
  src/stats.rs      input counters for the log
  src/game.rs       reads of the player, magic slots, inventory and stats
  src/store.rs      per-weapon assignments and their text file
  src/logging.rs    the log file and the debug gate
  src/window.rs     the panel host (Win32 + D3D11 + ImGui)
  src/overlay.rs    the panel's model: weapons, spells, filters
  src/pointer.rs    pointer ownership while the panel is open
  src/probe.rs      structure validation, version/RVA resolution
  src/params.rs     live parameter reads
  src/config.rs     nr_casting.ini
  pure/             nr_casting_core: slot parsing and input routing, unit-tested
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

That covers which input means which slot, which input wins when several are held (the Y chords
before a plain cast), the mouse mapping, the raw-input record layout and Shift hiding, the
character-file naming, and slot files written by older versions. Everything else needs the live game.

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
says which). It writes that context on every launch, whether or not `debug_log` is on; debug only
adds per-cast chatter. Three lines answer most reports:

```
modules: ersc.dll@0x180000000, d3d12.dll@0x6ffffa680000, eldenring.exe@0x140000000, ...
files in: C:\mods\nr-casting\nr_casting
crash: exception 0xc0000005 at 0x6ffffa4026a0 in recluse_input.dll (base 0x6ffffa000000, offset 0x4026a0) thread 7212 writing 0x18
```

`modules` lists every DLL loaded besides Windows' own, with its load base, so a proxy DLL, another
overlay or a class-port mod shows up by name — and a `crash:` line's `base`/`offset` points into
exactly one of them. Faults are recorded automatically (the handler observes and lets the game's
own handling continue, so a crash reporter still works), as are panics and the mod's own errors;
you never have to turn debug on first.

If the panel never appears, look for `panel:` — `panel: no panel graphics; staying closed` or
`panel: D3D11 device creation failed` mean the window could not get a device (casting is
unaffected), and `panel: first frame presented` confirms it drew. If nothing loads at all, the
profile path is the first suspect: the log will not exist.
