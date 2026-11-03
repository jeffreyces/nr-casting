#!/usr/bin/env bash
# Build the mod and assemble a release folder containing exactly what a user needs.
#
#   ./tools/package.sh            -> dist/nr_casting/
#
# The build tree (mod/target/...) holds hundreds of intermediate artefacts from the
# dependency graph; none of them ship. The release is one DLL (the name tables are compiled
# into it), an ini with defaults, a loader profile and a short install note.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
TRIPLE=x86_64-pc-windows-msvc
BUILD="$ROOT/mod/target/$TRIPLE/release"
OUT="$ROOT/dist/nr_casting"

cd "$ROOT/mod"
if [[ "$(uname -s)" == MINGW* || "$(uname -s)" == MSYS* ]]; then
    cargo build --release --target "$TRIPLE"
else
    # Stamped into the report: two builds can share a version while a fix is under test.
    NR_BUILD="$(date -u +%Y-%m-%dT%H:%M:%SZ)" cargo xwin build --release --target "$TRIPLE"
fi

mkdir -p "$OUT"
# Never wipe the folder: it is also the live install, and it holds assignments and settings.
cp "$BUILD/nr_casting.dll" "$OUT/"

# Name tables are compiled into the DLL from mod/data/. Regenerate them (tools/gen-*.py) and
# rebuild when the game adds spells or weapons; they are not shipped as separate files.

if [[ ! -f "$OUT/nr_casting.ini" ]]; then
cat > "$OUT/nr_casting.ini" <<'INI'
; nr-casting — four spells per catalyst (bumper, trigger, Y + trigger, Y + bumper)
[ui]
; Open the assignment panel on the hotkey. Set off for casting only (no window at all).
overlay = on
; Key that opens the assignment panel (F1-F24, INSERT, DELETE, HOME, END, PAGEUP, PAGEDOWN, TAB)
hotkey = F8
; List spells the character cannot cast yet (planning mode). Saved when you toggle it in the panel.
show_all = off
; Balanced mode. free = assign any spell the panel offers. attuned = only the spells you have
; memorised in the game's own spell menu can be assigned, so the mod uses your memory slots instead
; of replacing them; a row whose spell is no longer attuned is greyed, kept, and its input belongs
; to the game again. Saved when you toggle it in the panel.
balance = free
; Panel font size: auto picks the largest scale that fits the window, so dragging the corner grows
; the text with it; 100 / 125 / 150 / 175 / 200% pins it instead (the ini accepts up to 300%).
; Saved when you change it in the panel.
ui_scale = auto
; Panel window size: auto opens at the design size for the screen and fits its height to the content.
; Saved when you resize the panel.
panel_size = auto
; Panel window position: auto opens at the default spot. Saved when you drag the panel.
panel_pos = auto
; Force the game's own spell wheel to slot 1 so its HUD shows what R1 will cast. Off by default:
; on, it rewrites the selection ~30x a second and fights your D-pad scrolling.
hud_mirror = off

[input]
; Hold this and press R1/R2/L1/L2 to bind the spell currently selected in the game to that slot.
; L3, R3, LB, RB, DPAD_UP, DPAD_DOWN or none. On mouse and keyboard the same gesture is
; Super (Windows key) + click.
bind_modifier = L3
; Hold this and click to bind on mouse and keyboard. Super, Menu, F9, F10, F11, F12 or none.
; Super is grabbed by KDE/GNOME on Linux, so Menu or a function key is the safe choice there.
bind_modifier_kb = Super
; How many reports a Y chord spends with Y released before the shoulder press that casts. Higher is
; more forgiving of the game's own weapon-swap latch, lower is faster to cast. 2 is the default.
chord_stage = 2

[recluse]
; Who gets Y+R1 / Y+L1, the fourth spell. Recluse uses exactly those two chords.
; auto = ours unless the Recluse mod is loaded and this character is a Recluse,
;        in which case the chords pass through to the class port (its 4 / 5 keys)
; on   = always ours (even on a Recluse character)
; off  = never ours
y_bumper = auto
; Character ids that count as a Recluse: the port's character model is c8131.
recluse_chr_ids = 8131

[logging]
; Install the input detours at all. Leave on. off is a stutter experiment, not a way to play:
; with no detours the mod cannot cast, but the report then shows what the game's frame timing
; looks like with the DLL loaded and doing nothing - the comparison a loaded mod cannot make
; any other way. Use it for one session, compare the bundle with a normal run, then set it back.
; While bisecting a stutter, these two switch the *hook groups* on their own. Leave them on.
; pad_hook = on, raw_hook = on, pointer_hook = on is the normal build; turning one off is how a
; report can say which group's presence is responsible rather than which group's code.
hooks = on
raw_hook = on
pointer_hook = on
; Frame clock from IDXGISwapChain::Present (once per frame, more accurate than the pad poll).
; Off by default: the address discovery crashes inside dxgi.dll under Proton/vkd3d.
present_clock = off
; Log every cast, not just captures and errors
debug_log = off
; Perf log: one line when a window lost a frame and otherwise every 5 minutes, with the pad-poll
; interval (the game's frame time, as our hook sees it) and the longest time our own code spent
; inside it. Leave it on: it is the only setting that records stutter as it happens.
perf_log = on
INI
fi

cat > "$OUT/nr-casting.me3" <<'PROFILE'
profileVersion = "v1"

[[supports]]
game = "eldenring"

# Keep this file next to nr_casting.dll and the relative path below works as-is: me3 resolves
# it against the profile's own folder, so nothing needs editing.
#
# Only if you move the DLL somewhere else, put its absolute path here. Use single quotes so
# Windows backslashes stay literal — 'C:\mods\nr-casting\nr_casting.dll' — or forward slashes.
[[natives]]
path = "nr_casting.dll"
optional = false
enabled = true
PROFILE

cat > "$OUT/INSTALL.txt" <<'TXT'
nr-casting — four spells per catalyst for ELDEN RING (offline only)

WHAT IS IN THIS FOLDER
   nr_casting.dll    the mod
   nr-casting.me3    loader profile — already points at the DLL, nothing to edit
   nr_casting.ini    settings (panel key, bind key, Recluse chords, logging)
   INSTALL.txt       this file


INSTALL — the short way (no editing)

   1. Extract this zip into a real folder first (anywhere outside the game
      directory), then keep these four files together there. Do not launch from
      a 7-Zip or WinRAR preview window: those unpack to a temporary folder that
      is deleted when the archive is closed, and the profile then points at a
      file that no longer exists.

   2. Launch, passing the profile BY PATH:

         me3 launch -g eldenring -p "C:\mods\nr-casting\nr-casting.me3"

      The DLL path inside the profile is relative, and me3 resolves it against the
      profile's own folder. That is why the two files must stay together, and why
      nothing needs editing.

      Linux:  me3 launch -g eldenring -p "/home/you/mods/nr-casting/nr-casting.me3"


INSTALL — the long way (profile inside me3's profile folder)

   Copy nr-casting.me3 into:

         Windows  %APPDATA%\me3\profiles\
         Linux    ~/.config/me3/profiles/

   then edit the path line to the DLL's full location, in SINGLE quotes so the
   backslashes stay literal (in double quotes every \ has to be written as \\):

         path = 'C:\mods\nr-casting\nr_casting.dll'

   Forward slashes work too:   path = "C:/mods/nr-casting/nr_casting.dll"
   The path must end at the .dll FILE, not at the folder holding it.

   Then:  me3 launch -g eldenring -p nr-casting


INSTALL — me3 Manager (the graphical one)

   Add the mod through the manager and point it at nr_casting.dll. The manager
   writes its own profile, so nr-casting.me3 is not needed there.

   Two things to know about the manager:
     - it has a "start online" toggle. Leave it off; this mod is offline only.
     - updating a mod replaces its folder, which takes nr_casting.ini and
       nr_casting_slots_<character>.txt with it. Copy them out first if you have
       assignments you care about.


IN GAME

   F8                      open/close the assignment panel
   panel size              the panel scales to your screen and remembers the size you drag it
                           to; pick a UI scale in the panel to pin the text size instead
   R1 / R2                 cast slot 1 / slot 2 of the right-hand catalyst
   L1 / L2                 cast slot 1 / slot 2 of the left-hand catalyst
   Y + R2 / Y + L2         cast slot 3 of the right / left-hand catalyst
   Y + R1 / Y + L1         cast slot 4 of the right / left-hand catalyst
   Y + a shoulder          two-hand that hand, as usual, whenever the slot has no spell bound
   hold L3 + R1/R2/L1/L2   bind the spell selected in the wheel to that slot

   Mouse, same slots, no setup:
   left click              R1        shift + left click     R2
   right click             L1        shift + right click    L2
   ctrl + left click       Y+R2      ctrl + right click     Y+L2
   alt + left click        Y+R1      alt + right click      Y+L1
   super + any of those    bind the spell selected in the wheel to that slot


WHERE THE MOD WRITES ITS FILES

   Everything it generates goes into a subfolder beside the DLL:

         nr_casting\nr_casting_catalogue.txt   spells read from your own game
         nr_casting\nr_casting_slots_<character>.txt   your assignments, one file per character
         nr_casting\nr_casting_offsets.txt     resolved addresses (for bug reports)
         nr_casting\nr_casting_build.txt       which build the catalogue belongs to
         nr_casting\nr_casting_probe.log       what happened, in order

   If that folder cannot be created (for example the DLL sits in Program Files),
   the mod writes to %APPDATA%\nr_casting instead. The log's second line says
   which one it used: "files in: ...".


IF IT DOES NOT WORK

   Read nr_casting\nr_casting_probe.log first. It lists every DLL loaded besides
   Windows' own, and that is usually the answer:

     - NOTHING happened and there is no nr_casting\ folder at all: the DLL was
       never loaded, so the game ran vanilla. me3's own log says why, one line
       above the launch: "specified path does not exist or is inaccessible",
       and its attach config reads "natives: []". That is almost always a
       profile pointing into a temporary extraction folder (7-Zip, WinRAR) that
       has since been deleted - extract the zip somewhere permanent and pass
       the profile from there.
     - the panel could not get a D3D11 device. Casting still works. Look for a
       "panel:" line saying "no panel graphics" or "device creation failed".
     - a mod that also ships an overlay, or rewrites the spell wheel, can fight
       the panel and the cast steering.

   Nothing here touches your save file or the game's data: remove the DLL and the
   profile and the game is back to vanilla.

Never launch with EAC / online play: this mod requires the offline launch.
TXT

echo "packaged: $OUT"
ls -1 "$OUT"

# The release must be identifiable. A DLL with no resource section has no product name, no
# publisher and no version anywhere Windows or an antivirus UI can read - the state 0.2.9 shipped
# in, and part of why a scanner treated it as an anonymous payload. build.rs embeds the block, so
# this only fails when that silently stopped working (a missing resource compiler, say).
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/mod/Cargo.toml" | head -1)
if command -v objdump >/dev/null 2>&1; then
    resource=$(objdump -p "$OUT/nr_casting.dll" | awk '/Entry 2 /{print $3" "$4}')
    signature=$(objdump -p "$OUT/nr_casting.dll" | awk '/Entry 4 /{print $3" "$4}')
    if [[ "$resource" == "0000000000000000 00000000" ]]; then
        echo "ERROR: no VERSIONINFO in the packaged DLL — the release would be anonymous" >&2
        exit 1
    fi
    echo "identity: version $VERSION, resource $resource, signature ${signature%% *} (empty = unsigned)"
else
    echo "identity: cannot check (no objdump) — verify VERSIONINFO before shipping $VERSION" >&2
fi
