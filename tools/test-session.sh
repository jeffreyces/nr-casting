#!/usr/bin/env bash
# Launch one me3 profile, watch the session, and collect the crash evidence.
#
#   tools/test-session.sh <profile> [seconds]
#
# <profile> is a name in ~/.config/me3/profiles (seamless-ours, seamless-all, ...) or a path to a
# .me3 file. [seconds] is how long to let it run before stopping it; default 0 = wait for the game
# to exit on its own (i.e. until it crashes or you quit it).
#
# Where to look afterwards, in one place:
#   /tmp/nr-casting-session-<profile>-<stamp>/me3.log         launcher output
#   .../probe.log                                             the mod's own log (crash + modules)
#   .../new-dumps.txt                                         crash dumps written during the session
#   .../summary.txt                                           what this script concluded
#
# Runs me3 from the host: the container has no Steam runtime, the host does.
set -u

PROFILE=${1:?usage: test-session.sh <profile> [seconds]}
SECONDS_TO_RUN=${2:-0}
STAMP=$(date +%Y%m%d-%H%M%S)
GAME="$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game"
OUT="/tmp/nr-casting-session-${PROFILE//\//_}-$STAMP"
ME3="$HOME/.tools/me3/bin/me3"
DIST="$HOME/Documents/Github/nr-casting/dist/nr_casting"

mkdir -p "$OUT"
printf 'profile: %s\nstarted: %s\n' "$PROFILE" "$(date -Is)" > "$OUT/summary.txt"

# Baseline: which dumps exist before the run, so "new" means new.
ls "$GAME/SeamlessCoop/crashdumps/reports/" 2>/dev/null | sort > "$OUT/dumps-before.txt"
: > "$OUT/new-dumps.txt"

# The mod writes its log next to the DLL it was loaded from (dist), not into the game folder.
LOG="$DIST/nr_casting/nr_casting_probe.log"
cp "$LOG" "$OUT/probe.log.before" 2>/dev/null || true
LOG_BEFORE=$(stat -c %s "$LOG" 2>/dev/null || echo 0)

echo "session dir: $OUT"
echo "launching $PROFILE ..."

# me3 itself must run on the host (Steam runtime, Proton, DISPLAY).
distrobox-host-exec bash -lc "'$ME3' launch -g eldenring -p '$PROFILE' > '$OUT/me3.log' 2>&1" &
HOST_PID=$!

# Wait for closure: either the game exits (crash/quit) or the caller's time budget runs out.
deadline=$(( $(date +%s) + SECONDS_TO_RUN ))
while :; do
    if ! distrobox-host-exec bash -lc 'pgrep -x me3 >/dev/null'; then
        echo "me3 exited" >> "$OUT/summary.txt"
        break
    fi
    if [ "$SECONDS_TO_RUN" -gt 0 ] && [ "$(date +%s)" -ge "$deadline" ]; then
        echo "time budget reached, stopping" >> "$OUT/summary.txt"
        distrobox-host-exec bash -lc 'pkill -x me3; pkill -x eldenring.exe' || true
        break
    fi
    sleep 5
done

wait "$HOST_PID" 2>/dev/null || true

# Only this session's lines: everything past the offset recorded before launch.
tail -c +$((LOG_BEFORE + 1)) "$LOG" > "$OUT/probe.log" 2>/dev/null || : > "$OUT/probe.log"

# Dumps that appeared while this session was running.
ls "$GAME/SeamlessCoop/crashdumps/reports/" 2>/dev/null | sort > "$OUT/dumps-after.txt"
comm -13 "$OUT/dumps-before.txt" "$OUT/dumps-after.txt" > "$OUT/new-dumps.txt"

{
    echo "--- crash lines ---"
    grep -E "crash: exception 0x[0-9a-f]+" "$OUT/probe.log" 2>/dev/null | tail -5 || echo "(none)"
    echo "--- input warnings ---"
    grep -E "input: no pad polls" "$OUT/probe.log" 2>/dev/null | tail -2 || echo "(none)"
    echo "--- new crash dumps ---"
    cat "$OUT/new-dumps.txt" || true
} >> "$OUT/summary.txt"

cat "$OUT/summary.txt"
