#!/bin/zsh
# run-hunts-node.sh — TEST ONLY. The Node hunts, in a root of their own so that they can run beside the Bun ones.
R3=/private/tmp/deck-pi-r1c-Lkingz
hunt() { python3 "$R3/src/hunt.py" "$R3" "$@"; echo "hunt exit $? :: $*"; }
hunt node connect - 300 16 n-node-connect-nopause --until 3 R1_PAUSE_MS=0
hunt node connect - 120 16 n-node-connect --until 2
hunt node spawn "child_process.spawn pipes+detached" 40 14 n-node-spawn --until 2
hunt node spawn child_process.exec 40 14 n-node-exec --until 2
echo "ALL NODE HUNTS DONE"
