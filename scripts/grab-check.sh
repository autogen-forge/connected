#!/bin/sh
# Checks whether `capture --grab` really holds the cursor.
#
# The position the OS reports stays frozen while grabbing even if the drawn cursor moves, so
# position samples taken during the grab prove nothing. What does show it is the jump at
# release: the cursor snaps to wherever the mouse really took it.
#
#   scripts/grab-check.sh            run the check (move the mouse a lot, press Esc to end)
#   scripts/grab-check.sh --analyze LOG START_MS END_MS
#
# LOG holds `ms x y` lines from the watch_cursor example.

analyze() {
    log=$1 start=$2 end=$3
    awk -v s="$start" -v e="$end" '
        $1 <= e { px = $2; py = $3; have = 1; next }
        !after  { nx = $2; ny = $3; after = 1 }
        END {
            if (!have) { print "no samples before the release; the watcher did not start in time"; exit 2 }
            if (!after) {
                # The watcher logs only changes. A cursor that snaps at release would have
                # produced one, so no change means it did not move.
                printf "position at release: %d,%d, and it did not change afterwards\n", px, py
                print "VERDICT: cursor was held"
                exit 0
            }
            dx = nx - px; if (dx < 0) dx = -dx
            dy = ny - py; if (dy < 0) dy = -dy
            jump = dx > dy ? dx : dy
            printf "position at release: %d,%d -> first sample after: %d,%d (jump %dpx)\n", px, py, nx, ny, jump
            if (jump > 50) print "VERDICT: cursor was NOT held (it snapped to where the mouse really was)"
            else print "VERDICT: cursor was held"
        }' "$log"
}

if [ "$1" = "--analyze" ]; then
    analyze "$2" "$3" "$4"
    exit $?
fi

cd "$(dirname "$0")/.." || exit 1
cargo build -q -p connected-platform --examples || exit 1

log=$(mktemp)
./target/debug/examples/watch_cursor 16 > "$log" &
watcher=$!
sleep 1

echo "move the mouse a lot; press Esc to end"
out=$(./target/debug/examples/capture 10 --grab | grep -E "grabbing|released")
echo "$out"
wait "$watcher"

start=$(echo "$out" | sed -n 's/^grabbing input at \([0-9]*\) ms.*/\1/p')
end=$(echo "$out" | sed -n 's/^released at \([0-9]*\) ms.*/\1/p')
analyze "$log" "$start" "$end"
rm -f "$log"
