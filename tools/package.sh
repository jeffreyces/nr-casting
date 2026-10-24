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
    cargo xwin build --release --target "$TRIPLE"
fi

mkdir -p "$OUT"
# Never wipe the folder: it is also the live install, and it holds assignments and settings.
cp "$BUILD/nr_casting.dll" "$OUT/"

# Name tables are compiled into the DLL from mod/data/. Regenerate them (tools/gen-*.py) and
# rebuild when the game adds spells or weapons; they are not shipped as separate files.

if [[ ! -f "$OUT/nr_casting.ini" ]]; then
cat > "$OUT/nr_casting.ini" <<'INI'
; nr-casting — three spells per catalyst (bumper, trigger, Y + trigger)
[ui]
; Open the assignment panel on the hotkey. Set off for casting only (no window at all).
overlay = on
; Key that opens the assignment panel
hotkey = F8

[input]
; Hold this and press R1/R2/L1/L2 to bind the spell currently selected in game
bind_modifier = L3

[display]
; Mirror slot 1 onto the game's HUD while the panel is closed (locks the D-pad wheel)
hud_mirror = off

[logging]
; Log every cast, not just captures and errors
debug_log = off
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
nr-casting — three spells per catalyst for ELDEN RING (offline only)

WHAT IS IN THIS FOLDER
   nr_casting.dll    the mod
   nr-casting.me3    loader profile — already points at the DLL, nothing to edit
   nr_casting.ini    settings (panel key, bind key, HUD mirror, logging)
   INSTALL.txt       this file


INSTALL — the short way (no editing)

   1. Keep these four files together, in a folder outside the game directory.

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
       nr_casting_slots.txt with it. Copy them out first if you have
       assignments you care about.


IN GAME

   F8                      open/close the assignment panel
   R1 / R2                 cast slot 1 / slot 2 of the right-hand catalyst
   L1 / L2                 cast slot 1 / slot 2 of the left-hand catalyst
   Y + R2 / Y + L2         cast slot 3 of the right / left-hand catalyst
   R3 + L2                 plain L2, for the weapon's ash of war
   hold L3 + R1/R2/L1/L2   bind the spell selected in the wheel to that slot

   Mouse, same slots, no setup:
   left click              R1        shift + left click     R2
   right click             L1        shift + right click    L2
   ctrl + left click       Y+R2      ctrl + right click     Y+L2


WHERE THE MOD WRITES ITS FILES

   Everything it generates goes into a subfolder beside the DLL:

         nr_casting\nr_casting_catalogue.txt   spells read from your own game
         nr_casting\nr_casting_slots.txt       your assignments
         nr_casting\nr_casting_offsets.txt     resolved addresses (for bug reports)
         nr_casting\nr_casting_build.txt       which build the catalogue belongs to
         nr_casting\nr_casting_probe.log       what happened, in order

   If that folder cannot be created (for example the DLL sits in Program Files),
   the mod writes to %APPDATA%\nr_casting instead. The log's second line says
   which one it used: "files in: ...".


IF IT DOES NOT WORK

   Read nr_casting\nr_casting_probe.log first. It lists every DLL loaded besides
   Windows' own, and that is usually the answer:

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
