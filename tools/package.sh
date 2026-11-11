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

# The ini is one template file in the repo, shared with the mod's own fallback (config.rs includes
# the same bytes), so the shipped defaults cannot drift from the ones a missing file gets. Written
# only when absent: this folder is also the live install and holds the player's settings.
# The template is `include_str!`d by the crate, so it must be IN the repository, not just on this
# disk: an ignore rule matched its name, `git add -A` skipped it silently, and every local build
# passed while CI failed to compile the moment it checked out the repo. Fail here instead.
if ! git -C "$ROOT" ls-files --error-unmatch mod/data/nr_casting.ini >/dev/null 2>&1; then
	echo "error: mod/data/nr_casting.ini is not tracked by git - the shipped build would not compile" >&2
	exit 1
fi
if [[ ! -f "$OUT/nr_casting.ini" ]]; then
	cp "$ROOT/mod/data/nr_casting.ini" "$OUT/nr_casting.ini"
fi

cat >"$OUT/nr-casting.me3" <<'PROFILE'
profileVersion = "v1"

[[supports]]
game = "eldenring"

# Keep this file next to nr_casting.dll and the relative path below works as-is: me3 resolves
# it against the loader profile's own folder, so nothing needs editing.
#
# Only if you move the DLL somewhere else, put its absolute path here. Use single quotes so
# Windows backslashes stay literal — 'C:\mods\nr-casting\nr_casting.dll' — or forward slashes.
[[natives]]
path = "nr_casting.dll"
optional = false
enabled = true
PROFILE

cat >"$OUT/INSTALL.txt" <<'TXT'
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
      is deleted when the archive is closed, and the loader profile then points at a
      file that no longer exists.

   2. Launch, passing the loader profile BY PATH:

         me3 launch -g eldenring -p "C:\mods\nr-casting\nr-casting.me3"

      The DLL path inside the loader profile is relative, and me3 resolves it against the
      loader profile's own folder. That is why the two files must stay together, and why
      nothing needs editing.

      Linux:  me3 launch -g eldenring -p "/home/you/mods/nr-casting/nr-casting.me3"


INSTALL — the long way (loader profile inside me3's profile folder)

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
   writes its own loader profile, so nr-casting.me3 is not needed there.

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
       loader profile pointing into a temporary extraction folder (7-Zip, WinRAR) that
       has since been deleted - extract the zip somewhere permanent and pass
       the loader profile from there.
     - the panel could not get a D3D11 device. Casting still works. Look for a
       "panel:" line saying "no panel graphics" or "device creation failed".
     - a mod that also ships an overlay, or rewrites the spell wheel, can fight
       the panel and the cast steering.

   Nothing here touches your save file or the game's data: remove the DLL and the
   loader profile and the game is back to vanilla.

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
