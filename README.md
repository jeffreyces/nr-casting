# Nightreign Casting Plus — Nightreign-style casting for ELDEN RING

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
uses Y+R1 / Y+L1 itself. `y_bumper = auto` (the default) takes those chords **unless** the class port
is loaded *and* the character is one of its classes, so you get eight spells everywhere else and the
class port keeps its own chords on its own character. `on` claims them regardless, `off` leaves them
to the game.

**Why `auto` costs the fourth slot on a class character.** The port's default chords are `Y+RB` /
`Y+LB` — the same two — and it hides those buttons from the game while it uses them, so only one mod
can own them. `auto` yields: three spells on that hand, class skills untouched. To get four, set
`y_bumper = on` and move the class skills off the chord in `Options > Button Settings > Origin
Skills` (`X+RB` / `X+LB` is the usual choice). The mod cannot decide this for you: where you put
Origin Skills lives in the game's own button settings and cannot be read.

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
Y+R1 / Y+L1 itself. `y_bumper = auto` takes those chords **unless** the class port is loaded and
the character is one of its classes, where they pass straight through to the class port instead —
eight spells everywhere else, its own chords on its own character, and three rather than four on
that hand. Four needs `y_bumper = on` plus a rebind of the class skills; see above.

**Keeping assigned spells out of the vanilla wheel.** If you assign most of your spells to the
panel's buttons and keep one or two for the game's own default cast, the wheel still lists all of
them. `vanilla_skips_assigned = on` makes the game's own cycling step over the spells the panel owns,
so pressing up (or the keyboard cycle key) only lands on the ones you have not assigned. "Owns" means
any of the six weapon slots the character is carrying — not just the two in your hands — so switching
between your staves mid-fight does not change what the wheel offers, and a spell bound on your spare
staff stays out of it. Nothing is
removed from your memory slots: those spells stay attuned and still cast from their own buttons. It
also hides the spells the catalysts **in your hands cannot cast** — incantations while you hold only a
staff — so unequipping a seal takes its incantations out of the wheel rather than leaving spells you
can never cast. If nothing is left to offer, the wheel cycles normally. It is independent of
`balance`, applies immediately, and defaults to off.

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
created the first time you open it. The mouse is not the only way in: every list answers the arrow
keys, Enter commits the highlighted row and Escape backs out, so a spell list longer than its box can
be worked from the keyboard alone. The pad is not wired to the panel. It is deliberately *not* an overlay on the game's swap chain:
a second `Present` hook stops a co-resident class port's HUD from drawing (verified with
`ERNativeUI.dll`), even when that hook draws nothing at all.

## Install

**me3** is the supported loader (Mod Engine 2 is discontinued upstream; development moved to
me3). The shipped loader profile needs no editing:

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

me3 resolves a relative native path against the **loader profile's own folder**, so keeping the DLL
next to the loader profile is all it takes. Passing the loader profile by path also means it does not have to
live in me3's loader-profile directory.

If you would rather keep the loader profile with me3's others (`%APPDATA%\me3\profiles\`), give the
`path` line the DLL's absolute location — in **single quotes**, so Windows backslashes stay
literal:

```toml
path = 'C:\mods\nr-casting\nr_casting.dll'     # single quotes: one backslash each
path = "C:/mods/nr-casting/nr_casting.dll"       # or forward slashes
```

The path ends at the `.dll` file, not at the folder holding it. Adding the DLL through
**me3 Manager** works too: the manager writes its own loader profile.

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

Names are read from the running game's own text tables on first launch and cached in
`nr_casting_catalogue.txt`; the optional override files above replace that source. Settings are read from the shipped `nr_casting.ini` first, so editing it always
takes effect even if an older copy sits in the subfolder.

### Installing through the ME3 Manager

The graphical [ME3 Manager](https://github.com/2Pz/me3-manager) is a front end over the same me3:
it writes the `.me3` loader profile for you and lists the DLL under `[[natives]]` (relative to the
loader profile, or by absolute path when the file lives outside its mods folder). Nothing in the DLL
reads the loader profile, so a Manager install and a hand-written one behave identically — the mod finds
its own folder from its module path and writes its files there.

Two Manager behaviours are worth knowing:

- **`start_online` is a loader profile field.** me3 keeps matchmaking off by default; if you turn that
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
balance = free           ; free | attuned — attuned = only memorised spells can be assigned
vanilla_skips_assigned = off  ; on = the game's wheel skips the spells this panel owns
profile = default        ; which assignment profile is live (the sheets live in the slot file)
profile_modifier = Ctrl  ; Ctrl | Alt | Shift | Super — the key held with a digit to switch
ui_scale = auto          ; auto | 125 | 150% | 1.5 | 200% — panel text and widget size (100%-300%)
panel_size = auto        ; auto | 1800x1240 — panel window size, saved when you resize it
panel_pos = auto         ; auto | 1140,431 — panel corner, saved when you move it
hud_mirror = off         ; on = force the game's wheel to slot 1 (fights your own scrolling)

[input]
bind_modifier = L3       ; L3 | R3 | LB | RB | DPAD_UP | DPAD_DOWN | none
bind_modifier_kb = Super ; Super | Menu | F9 | F10 | F11 | F12 | none — held with a click
chord_stage = 2          ; reports a chord spends with Y released before casting (2 safe, 1 faster)
pad_hook = on            ; on | off — install the XInput hook at all (diagnostic)

[recluse]
y_bumper = auto          ; auto | on | off — who gets Y+R1 / Y+L1

[logging]
hooks = on               ; on | off — install the input detours (off = comparison only)
raw_hook = on            ; on | off — mouse and keyboard raw input, part of hooks
pointer_hook = on        ; on | off — confine the pointer while the panel is open
present_clock = off      ; on | off — use dxgi Present as the frame clock (crashes under Proton)
debug_log = off          ; log every cast, not just captures and errors
perf_log = on            ; a timing line when a frame is lost, or every 5 minutes otherwise
```

Malformed values are logged and the default kept.

**The file is watched.** Save it while the game is running and the change is picked up within a
second — no restart. `debug_log`, `perf_log`, `hud_mirror`, `show_all`, `balance`, `ui_scale`,
`bind_modifier` and `hotkey` all take effect live; the switches that have to be in place before a hook is installed
(`overlay`, `hooks`, `pad_hook`, `raw_hook`, `pointer_hook`, `present_clock`) log that they will take
effect on the next launch instead of being silently ignored. The mod's own writes to this file (panel
size, corner, scale) are recognised and never reload it.

## Using the panel

Assignments are made in the panel or with the bind gesture; both write the same per-character file.

**Profiles** are named assignment sheets you can switch between without opening the panel. The row
under the `balanced` toggle holds the list, each sheet's hotkey, and `save as` / `rename` / `delete`;
`create new` at the bottom of the list starts an empty sheet, while `save as` copies the live one.
Every profile holds its own values - nothing inherits the previous profile's switches. `create new`
starts free with the wheel skip off, like a fresh install; `save as` copies the live pair. The list
marks each profile's mode (`default (Ctrl+1, free)`, `boss (Ctrl+2, attuned, skip)`) because
switching publishes that profile's `balanced` and wheel-skip values - a free-mode assignment can read
as unbound under an attuned profile. Flipping either switch while a profile is live records it into
that profile, so `save as` carries the pair and a switch away and back keeps your change. Typing a name and pressing Enter confirms it. `Ctrl+1` to `Ctrl+9` switch in the world *and* with the panel
open, and the live one is always shown top right (Ctrl+1 belongs to `default` unless you move it). A profile
carries its own `balanced` and `skip assigned in wheel` values, so a boss setup and an exploration
setup can differ in more than the spells. `default` cannot be renamed or deleted; deleting the sheet
you are on lands you back on `default`. Profiles never change your equipment, memory slots or save.

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
- **Size**: the panel opens at the scale the screen implies (screen height ÷ 1080), so a 4K TV gets
  a 1800×1240 window with 30 px text rather than a 900×620 one with 15 px text. Dragging a corner
  and letting go recomputes the scale from the window (the smaller of width and height against the
  900×620 design), so a bigger window means bigger text, not more empty background — and the size
  you settled on is written to `panel_size`, so the next session opens the panel you sized. The
  `UI scale` dropdown pins the text size to 100%–200% instead (a corner drag then gives the layout
  more room rather than bigger text), `ui_scale` in the ini does the same across sessions, and
  `auto` hands the text size back to the window. The window size and the text scale are separate
  settings: resizing always remembers the window, whatever the scale is pinned to.
- **Left column**: the left-hand weapon with dropdowns for L1 / L2 / Y+L1 / Y+L2. **Right
  column**: the right-hand weapon with R1 / R2 / Y+R1 / Y+R2 (the fourth row first, matching
  slot order). Each row shows its mouse binding beside the pad one.
  **Middle**: the character whose assignments are being edited, your effective INT/FAI/ARC (and a
  line saying when a rune or talisman is doing the work), a spell search box, how many spells the
  pickers are offering, and `list everything`. Under the three columns, a controls row holds
  `refresh`, `reset slots`, `reset layout`, `close`, the UI scale and `diagnostics & report`; below it
  a footer spells out both mappings.
- Dropdowns list only spells the character **owns** and **can cast** — requirements are checked
  against INT/FAI/ARC *including active effects*, so a Great Rune or a stat talisman counts and the
  stats line says when it is doing the work. `list everything` bypasses both for planning a build.
- **Assignments survive upgrades.** They are filed on the weapon's base id (`33090000`, not
  `33090010`), so taking a staff to the smith no longer empties it. Slots files written by older
  versions are folded onto base ids the first time they are read.
- **On a Recluse character** the fourth row is greyed out and labelled *passthrough*: the chord
  goes to the class port, which binds it to its own `4` (Y+R1) and `5` (Y+L1) keys. With a
  controller the pad report is simply left alone, so the port sees Y+R1 / Y+L1 itself; on
  keyboard and mouse the Alt click is swallowed and `4` / `5` is pressed in its place — passing
  the click through would only make the game perform a plain attack, since it ignores Alt.
- **Layout**: `reset layout` puts the window back where it first opened and re-fits its height to the
  content. The corner and the size are remembered per session in `panel_pos` and `panel_size`; reset
  is for the case a saved corner no longer lands anywhere you can reach after a monitor change.
- **The HUD mirror is off by default.** With `hud_mirror = on` the right-hand weapon's first slot is
  forced onto the game's own spell wheel so its HUD shows what R1 will cast — but that rewrites
  your selection ~30 times a second, so the wheel visibly cycles and a D-pad scroll is fought every
  frame. Leave it off if you pick spells yourself (the default).

## Reporting a problem

The panel's `diagnostics & report` button writes one text file beside the DLL — about 20-60 KB —
holding what a bug report actually needs: the log tail, the offsets the mod resolved against this
exe, the generated spell catalogue, the configuration, the modules loaded into the game, the GPU and
CPU, and a per-second record of the frame gaps over the last five minutes.

- **It waits for five minutes of the current character.** The button counts down from the moment a
  character loads and stays disabled until it has them, so a report that says "300 s" really has 300
  seconds in it; hovering it says how much is left. This is what makes a stutter report meaningful.
- **Nothing is uploaded.** There is no endpoint: the file is written locally, its path is put on the
  clipboard, the folder opens with the file selected, and the issue page opens with the version and
  the numbers already prefilled — drag the file in and submit.
- **It redacts on the way out — and the log does it too.** Every file path is replaced with `<path>`
  — directory, file name and all — with one deliberate exception: a path inside a mod manager's
  folder keeps its tail from `me3` on (`me3\config\profiles\<profile>\<mod>\nr_casting`), because
  that names the loader profile and the mod and says nothing about the machine. The account segment, the
  manager's own folder above it and the drive all go. Your user name becomes `%USER%` where it
  appears outside a path, and the character's name and the id in its slot file name become
  `<character>`. The same rule runs on `nr_casting_probe.log` as it is written, so a log pasted into
  an issue carries no path either. Neither `debug_log` nor `perf_log` needs to be on; the report
  collects what it needs at the moment you press it.
- **`extended bundle & report`** is the one for a stutter. It does the comparing for you: press it,
  load a character, stand somewhere quiet and still, and start. The mod records nine one-minute
  phases — everything on, then the raw-input hooks off, the cursor detours taken out, pad casting off
  (casting stops for that minute, which is expected), the housekeeping thread off, pad-only, every
  group off, and then **two phases with everything back on**. Those two closing baselines are the
  point: a run measures a machine that can change speed halfway through, and with one closing phase
  the phase that switched everything back on tends to look like the fastest of the run. Two identical
  phases make drift visible as drift. The report's first section puts the phases side by side: for
  each one, the per-second worst gap (p50/p90/max), how many seconds lost a frame, **how many
  individual hitches** there were over 32 ms, and which second the worst one fell in. It needs a
  frame clock, and takes the best one available: the pad poll when a controller is attached,
  otherwise the game's own message pump (`PeekMessageW`), which runs once a frame in the world and in
  every menu. The state section of the report says which clock was live. That is the
  difference between "it stutters" and "it stutters with our hooks loaded", which is usually the
  question worth answering. The panel closes itself when the run starts, because an open panel owns
  the input and the GPU it is meant to be measuring.
- **The report says which numbers cover what.** The per-second series is always the last five
  minutes; the pad-path and dispatch sections are counts since load, and say so with the session's
  length. `character recheck` is per check, not per frame. `spell selected in the game` carries the
  age of the reading, because it is read when the panel opens rather than polled. And the header
  carries `(built ...)` beside the version, so two builds of the same version can be told apart.
- **`perf_log = on`** additionally writes a timing line to the log — when a frame was lost, or every
  five minutes when nothing was. It is on in the shipped ini; it is quiet enough to leave there, and
  it is the only setting that records stutters as they happen.

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
  is skipped while `list everything` is enabled, since that is planning mode and uncastable picks
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
**and** a catalyst is equipped that is missing from the vanilla weapon table — which is how a class
port's own catalysts are recognised. There is no id list to fill in: the older `recluse_chr_ids`
setting was removed because `ChrIns.character_id` reads 0 on current builds, so the test could never
match, and the catalyst signal is what actually recognised the character.

```ini
[recluse]
y_bumper = auto          ; auto | on | off
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
detected; nothing is written to game memory until a structural probe passes (weapon ids in range,
stats plausible, 14 magic slots with a valid selection) and a write probe round-trips the one field
the mod writes (the selected slot: written, read back, restored, and the rest of the structure
compared byte for byte). That second check is what makes an unrecognised build trustworthy rather
than probable — it is the difference between "the fields we read look sane" and "the field we write
is where we think it is, and writing it moves nothing else". On failure the mod stays out of
the way and `nr_casting_offsets.txt` names the check that failed. `NR_CASTING_FORCE_RVA=1`
exercises the fallback on a known-good build.

Built against ELDEN RING exe `2.7.2.0` (RVA table `Ww2720`).

## Compatibility

| layer | notes |
|---|---|
| me3 | supported, tested |
| Vortex | reported working: it deploys the DLL like any native mod and the shipped `.me3` loader profile is not needed for it |
| frame-generation / upscaling proxies (FSR/FG `d3d12.dll`, `dxgi.dll`, `erss-fg.dll`) | they own frame presentation; the panel may not appear, casting still works |
| class ports with their own HUD (`ERNativeUI.dll` and friends) | no conflict: the panel is its own window, so their HUD keeps drawing |
| Mod Engine 2 | discontinued upstream; the DLL loads via `LoadLibrary` and needs nothing special |
| other overlay mods | no swap-chain conflict at all — the panel owns no swap chain, it presents through a DIB; both drive the pointer while their window is open, so expect contention there |
| [ERD Tools](https://www.nexusmods.com/eldenring/mods/9531) | supported — the panel owns no swap chain, so it cannot disturb ERD's present hooks (see "ERD Tools" under Troubleshooting) |
| spells/selection mods | direct conflict: both write the magic selection |
| regulation mods | no conflict today (no param edits shipped) |
| online / EAC | not supported by design |

## Troubleshooting

The first line of the log names the build — `=== nr_casting 0.2.22 (built 2026-11-04T20:18:21Z)` —
because a playtest runs several builds under one version, and "which one was that?" is the first
question a report has to answer. Paths in it are redacted as they are written (see Reporting a
problem).

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

### ERD Tools

Supported, after a fix that came from a report. The symptom was a crash inside ERD a second after
the panel opened, and the report said where:

```
crash: exception 0xc0000005 at 0x7ffd3e3d64f9 in erdGameTools.dll (offset 0xa64f9) thread 30856 reading 0x20
crash:   frame 3: ntdll.dll+0x165dee
crash:   frame 4: erdGameTools.dll+0xa64f9
crash:   frame 7: eldenring.exe+0x1ea0eac
```

The faulting address is inside ERD, reading field `0x20` of a null pointer, and the call comes from
the game's own per-frame code.

Reproduced locally by opening the panel on a timer, then bisected in five runs:

| what exists | result |
|---|---|
| window only | no crash |
| + a D3D11 device | no crash |
| + a swap chain that is never presented | no crash |
| + one `Present` | **crash** inside ERD |
| `hooks = off` (none of this mod's detours) | **crash** — the detours were never involved |

The trigger is a `Present` from the panel's window: ERD hooks the present path process-wide and its
per-frame code then reads stale state. The panel therefore has **no swap chain** any more — it renders
into a texture, copies that to a DIB and `BitBlt`s it into its window, which calls no `Present` at
all. Cost is one GPU-to-CPU copy per drawn frame, reported as `present ok (copy under 1 ms)`.

The bisect switches used to find this are still in the build, as environment variables rather than ini
keys: `NR_CASTING_AUTO_PANEL_MS`, `NR_CASTING_PANEL_NODEVICE`, `NR_CASTING_PANEL_DEVICE_ONLY`.

If the panel never appears, look for `panel:` — `panel: no panel graphics; staying closed` or
`panel: D3D11 device creation failed` mean the window could not get a device (casting is
unaffected), and `panel: first frame presented` confirms it drew. If nothing loads at all, the
loader profile path is the first suspect: the log will not exist.

## Support

This mod is free and stays free, and nothing in it is gated behind a donation. Thanks to everyone
who has used it, reported on it and supported it — Ko-fi is there if you would like to give
something back.

<https://ko-fi.com/jeffreycespedes>
