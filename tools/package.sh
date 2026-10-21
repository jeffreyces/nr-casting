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
; nr-casting — two spells per catalyst
[ui]
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

[[natives]]
path = "REPLACE_WITH_FULL_PATH_TO\\nr_casting.dll"
optional = false
enabled = true
PROFILE

cat > "$OUT/INSTALL.txt" <<'TXT'
nr-casting — two spells per catalyst for ELDEN RING (offline only)

1. Put this folder anywhere outside the game directory.
2. Copy nr-casting.me3 to your me3 profiles folder and set the path in it:
      Linux   ~/.config/me3/profiles/
      Windows %APPDATA%\me3\profiles\
3. Launch:  me3 launch -g eldenring -p nr-casting
4. In game, F8 opens the panel. Hold L3 and press R1/R2/L1/L2 to bind the spell
   currently selected in the wheel.

This folder ships four files and nothing else: nr_casting.dll, nr-casting.me3,
nr_casting.ini, INSTALL.txt. Everything the mod needs beyond that it generates
itself, on first launch, inside an nr_casting\ subfolder beside the DLL:

   nr_casting\nr_casting_catalogue.txt   spell catalogue read from your own game
   nr_casting\nr_casting_slots.txt       your assignments (weapon = bumper,trigger)
   nr_casting\nr_casting_offsets.txt     resolved addresses, for troubleshooting
   nr_casting\nr_casting_build.txt       fingerprint of the build it belongs to
   nr_casting\nr_casting_probe.log       what it did, in order

The catalogue is built once and reused; if the game executable changes, the old
catalogue and log are discarded and rebuilt automatically.

Spell and weapon names are compiled into the DLL, because the game's params carry
numbers only. After a game patch that adds content, regenerate the tables with
tools/gen-spell-names.py and tools/gen-weapon-names.py and rebuild — or drop the
regenerated .txt files beside the DLL to override without a rebuild. The log says
when the game has weapons the table cannot name yet.

Nothing here touches your save file or the game's data: remove the DLL and the
profile and the game is back to vanilla. Settings live in nr_casting.ini; delete
the nr_casting\ subfolder to reset everything the mod generated.

Never launch with EAC / online play: this mod requires the offline launch.
TXT

echo "packaged: $OUT"
ls -1 "$OUT"
