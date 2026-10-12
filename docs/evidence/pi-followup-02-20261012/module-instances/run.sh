#!/bin/bash
# one print-mode run of Pi under a wall clock; the process is this script's own child
D=$1; name=$2; shift 2
env -i HOME=$D/home PATH=/usr/bin:/bin TERM=xterm-256color LANG=en_US.UTF-8 TMPDIR=$D/tmp PI_CODING_AGENT_DIR=$D/agent PI_SKIP_VERSION_CHECK=1 PI_OFFLINE=1 PI_TELEMETRY=0 PROBE_LOG=$D/$name.jsonl FIXTURE_LOG=$D/$name-source.jsonl "$@" --no-skills --no-prompt-templates --no-themes --no-tools --provider deck-fixture --model fixture --session-dir $D/sessions -p "hello fixture" > $D/$name.out 2>&1 < /dev/null &
pid=$!
for _ in $(seq 1 40); do
  if ! kill -0 $pid 2>/dev/null; then wait $pid; echo "$name exit=$?"; cat $D/$name.jsonl 2>/dev/null; exit 0; fi
  sleep 0.5
done
echo "$name still running after 20 s: stopping this script's own child $pid"; kill $pid; wait $pid 2>/dev/null; cat $D/$name.jsonl 2>/dev/null
