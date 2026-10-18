#!/bin/bash
# run-campaign.sh <campaign> <policy> — one campaign of cxctl.sh end to end:
# init, server, the two starts, ids, the ONE transaction, the observation,
# and the registry cleanup whatever happened before it. A step that fails
# stops the chain; `end` still runs. Written as a bash file on purpose: the
# operator's own shell is zsh, which does not word-split a variable.
set -u
C=${1:?campaign}; P=${2:?policy}
D=$(cd "$(dirname "$0")" && pwd)/cxctl.sh
rc=0
for step in "init $C $P" "server $C" "start $C a" "start $C b" "ids $C" "transact $C" "observe $C"; do
  # shellcheck disable=SC2086
  bash "$D" $step 2>&1 | tail -40; s=${PIPESTATUS[0]}
  echo "step [$step] rc=$s"
  if [ "$s" != 0 ]; then rc=$s; break; fi
done
bash "$D" end "$C" 2>&1 | tail -6; e=${PIPESTATUS[0]}
echo "CHAIN-DONE campaign=$C policy=$P rc=$rc end-rc=$e"
exit $rc
