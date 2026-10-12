#!/bin/zsh
# run-hunts.sh — TEST ONLY. The hunts, one after another (they share the root's fifos).
R2=/private/tmp/deck-pi-r1b-OyV13W
hunt() { python3 "$R2/src/hunt.py" "$R2" "$@"; echo "hunt exit $? :: $*"; }
hunt bun connect - 40 60 h-bun-connect --until 4
hunt bun spawn child_process.exec 40 50 h-bun-exec --until 4
hunt node connect - 120 40 h-node-connect --until 3
hunt node connect - 300 20 h-node-connect-nopause --until 3 R1_PAUSE_MS=0
hunt bun spawn "child_process.spawn pipes+detached" 40 30 h-bun-spawn --until 2
hunt node spawn "child_process.spawn pipes+detached" 40 30 h-node-spawn --until 2
echo "ALL HUNTS DONE"
