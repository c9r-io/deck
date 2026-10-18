#!/bin/bash
# cxctl.sh — the isolated real-Codex re-entry transaction (2026-10-18).
#
# One campaign = one experiment root /private/tmp/deck-cx-<campaign> (short:
# a unix socket path is limited) holding EVERYTHING the run starts or
# writes: the Codex home (h/, with its own config, hooks.json and trust
# state), the two working directories (w/a, w/b), the pane shells' startup
# directory (zdot/), the controller's status socket, Board and lifecycle
# file (base/), the controller's data directory and HOME (data/, ctlhome/),
# the loopback stub provider (bin/stub, from the 2026-10-17 experiment's
# source), the tmux socket directory (t/) and the evidence (ev/). The
# user's own ~/.codex, ~/.deck and ~/.claude are never read or written.
#
#   cxctl.sh init     <campaign> ordinary|reapply-bypass
#   cxctl.sh server   <campaign>        private tmux server, sessions a and b
#   cxctl.sh start    <campaign> a|b    type that card's ONE start line, register the client
#   cxctl.sh ids      <campaign>        the thread id each client holds (its rollout), digested
#   cxctl.sh transact <campaign>        the production transaction ONCE (the test controller)
#   cxctl.sh observe  <campaign>        read the new panes: argv, /status, rollouts; register the new clients
#   cxctl.sh end      <campaign>        stop every registered resource through the registry, verified
#   cxctl.sh destroy  <campaign>        remove the root, and only it
#
# Limits: one start per card, one transaction per campaign, no second input
# but the operator's own `/status` after the transaction. Every process is
# registered with scripts/edr_runtime.py when started and stopped only
# through it; a pid is read from ONE process-table query and must be the
# exactly-one codex child of the pane's shell.
#
# Exit codes: 0 done · 2 refused, nothing changed · 3 a failed check · 4
# cleanup not verified: everything is KEPT.
set -u
CMD=${1:?command}; CAMPAIGN=${2:?campaign}; shift 2
case $CAMPAIGN in *[!a-z0-9]*|'') echo "campaign: lowercase letters and digits"; exit 2;; esac
REPO=/Users/chenhan/c9r-io/deck
SRC=$REPO/app/src-tauri
TMUXSRC=$SRC/binaries/tmux-aarch64-apple-darwin
HELPER=$SRC/binaries/deck-lifecycle-helper-aarch64-apple-darwin
CODEX=/opt/homebrew/Caskroom/codex/0.163.0/bin/codex
PROBE=${CX_PROBE:?the deck-app test binary (CX_PROBE)}
STUB_SRC=/private/tmp/cxr-report-20261017/tools/stub.rs
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=/private/tmp/deck-cx-$CAMPAIGN
BASE=$ROOT/base; EV=$ROOT/ev; REG=$ROOT/.cx
SOCK=deck-smoke-cx-$CAMPAIGN
# the registry stops only programs that are files inside the root: the
# bundled tmux is copied there (its content is the sidecar's, byte for byte)
TMUXBIN=$ROOT/bin/tmux
SOCKPATH=$ROOT/t/tmux-$(id -u)/$SOCK
REGISTRY="python3 $REPO/scripts/edr_runtime.py experiment"

say() { echo "[$CAMPAIGN] $*" | tee -a "$EV/log.txt" 2>/dev/null || echo "[$CAMPAIGN] $*"; }
refuse() { say "refused: $*"; exit 2; }
have() { [ -f "$REG/$1" ]; }
get() { cat "$REG/$1" 2>/dev/null; }
root_checked() {
  [ "$ROOT" = "/private/tmp/deck-cx-$CAMPAIGN" ] && [ -d "$ROOT" ] && [ ! -L "$ROOT" ] && [ -d "$REG" ] || return 1
  [ "$(cd -P "$ROOT" && pwd -P)" = "$ROOT" ] || return 1
  return 0
}
# The private server: the clients' HOME is the isolated home, the pane
# shells' startup directory and the status socket are the root's, the
# socket directory is inside the root. The same names the controller hands
# to the server IT starts (TMUX_TMPDIR through DECK_M1_ENV).
T() { env -i HOME="$ROOT/h" ZDOTDIR="$ROOT/zdot" DECK_STATUS_SOCK="$BASE/status.sock" TMUX_TMPDIR="$ROOT/t" \
      PATH=/usr/bin:/bin:/usr/sbin:/sbin TERM=xterm-256color SHELL=/bin/zsh "$TMUXBIN" -f /dev/null -L "$SOCK" "$@"; }
cap() { T capture-pane -p -J -t "$1" -S -200 | sed -e 's/[[:space:]]*$//' | grep -v '^$'; }
# the exactly-one codex child of the pane's shell, from ONE table query
fgpid() {
  local sh table n
  sh=$(T display-message -p -t "$1" '#{pane_pid}') || return 2
  table=$(/bin/ps -axo pid=,ppid=,comm=) || return 2
  n=$(echo "$table" | awk -v p="$sh" -v c="$CODEX" '$2==p && $3==c {print $1}')
  [ "$(echo "$n" | grep -c .)" = 1 ] || { echo "NOT-ONE:[$n]" >&2; return 3; }
  echo "$n"
}
ident() { /bin/ps -o pid=,lstart=,comm= -p "$1" 2>/dev/null; }
# every process whose PROGRAM is a file inside the root (as the registry
# judges it) — never a shell whose command text merely names the root
running_from_root() { local table; table=$(/bin/ps -axo pid=,comm=) || return 2; echo "$table" | awk -v r="$ROOT/" 'index($2, r)==1'; }
digest() { printf '%s:%s' "$1" "$2" | /usr/bin/shasum -a 256 | cut -c1-16; }

case $CMD in
init)
  POLICY=${1:?policy}
  case $POLICY in ordinary|reapply-bypass) ;; *) refuse "policy: ordinary|reapply-bypass";; esac
  [ ! -e "$ROOT" ] || refuse "$ROOT exists"
  [ -x "$CODEX" ] && [ -x "$HELPER" ] && [ -x "$TMUXSRC" ] && [ -x "$PROBE" ] && [ -f "$STUB_SRC" ] || refuse "a fixed tool is missing"
  mkdir -p "$ROOT"/{h/.codex,w/a,w/b,zdot,base,data,ctlhome,ev,bin,t} "$REG" || exit 2
  cp "$TMUXSRC" "$TMUXBIN" && chmod 755 "$TMUXBIN" || exit 2
  chmod 700 "$ROOT" "$ROOT/h" "$ROOT/h/.codex" "$REG"
  : > "$ROOT/zdot/.zshrc"
  echo "$POLICY" > "$REG/policy"
  /usr/bin/uuidgen | tr 'A-Z' 'a-z' > "$REG/salt"
  say "init root=$ROOT policy=$POLICY"
  $REGISTRY init --root "$ROOT" || exit 2
  # the loopback stub provider, compiled inside the root and registered
  rustc -O --edition 2021 -o "$ROOT/bin/stub" "$STUB_SRC" >"$EV/stub-build.log" 2>&1 || { say "stub build failed"; exit 2; }
  # (not `cd && nohup … &`: that backgrounds the whole list, whose subshell
  # then waits for the stub and holds the caller's pipe open)
  ( cd "$ROOT" || exit 2; nohup "$ROOT/bin/stub" "$EV" </dev/null >/dev/null 2>&1 & )
  for i in $(seq 1 50); do [ -s "$EV/stub.port" ] && break; /bin/sleep 0.1; done
  [ -s "$EV/stub.port" ] || { say "stub did not come up"; exit 2; }
  # the stub's own pid from ONE table query: the exactly-one process that
  # executes the root's stub (a shell wrapper's pid is not the stub's)
  table=$(/bin/ps -axo pid=,comm=) || exit 2
  stubpid=$(echo "$table" | awk -v c="$ROOT/bin/stub" '$2==c {print $1}')
  [ "$(echo "$stubpid" | grep -c .)" = 1 ] || { say "stub: not exactly one process"; exit 2; }
  echo "$stubpid" > "$REG/stub-pid"
  $REGISTRY add-process --root "$ROOT" --pid "$stubpid" || exit 2
  PORT=$(cat "$EV/stub.port")
  # Deck's two lifecycle hooks exactly as production writes them for Codex
  # (HookStyle::ShellBrief: one simple command, timeout 1, no async)
  cat > "$ROOT/h/.codex/hooks.json" <<EOF
{
  "hooks": {
    "SessionStart": [
      { "hooks": [ { "type": "command", "command": "\"$HELPER\" codex session-start", "timeout": 1 } ] }
    ],
    "SessionEnd": [
      { "hooks": [ { "type": "command", "command": "\"$HELPER\" codex session-end", "timeout": 1 } ] }
    ]
  }
}
EOF
  {
    cat <<EOF
model = "stub-model"
model_provider = "stub"
check_for_update_on_startup = false
cli_auth_credentials_store = "file"
mcp_oauth_credentials_store = "file"

[analytics]
enabled = false

[feedback]
enabled = false

[model_providers.stub]
name = "stub"
base_url = "http://127.0.0.1:$PORT/v1"
wire_api = "responses"
requires_openai_auth = false
request_max_retries = 0
stream_max_retries = 0

[projects."$ROOT/w/a"]
trust_level = "trusted"

[projects."$ROOT/w/b"]
trust_level = "trusted"

[tui]
screen_reader_detection_done = true

EOF
    python3 -I "$HERE/hookstate.py" "$ROOT/h/.codex/hooks.json" "$HELPER"
  } > "$ROOT/h/.codex/config.toml" || exit 2
  chmod 600 "$ROOT/h/.codex/config.toml" "$ROOT/h/.codex/hooks.json"
  /usr/bin/shasum -a 256 "$CODEX" "$HELPER" "$TMUXSRC" "$TMUXBIN" "$PROBE" "$STUB_SRC" "$ROOT/bin/stub" > "$EV/tools.sha256"
  "$CODEX" --version > "$EV/codex-version.txt" 2>&1
  say "init done: stub port=$PORT, hooks and trust state written"
  ;;
server)
  root_checked || refuse "no such campaign"
  have server && refuse "the server is already up"
  T new-session -d -s a -c "$ROOT/w/a" -x 120 -y 40 || exit 2
  T set-option -g exit-empty off || exit 2
  T new-session -d -s b -c "$ROOT/w/b" -x 120 -y 40 || exit 2
  $REGISTRY add-server --root "$ROOT" --socket "$SOCKPATH" --tmux "$TMUXBIN" || exit 2
  T display-message -p '#{pid}' > "$REG/server"
  for card in a b; do
    for i in $(seq 1 50); do [ "$(T display-message -p -t "$card" '#{bracket_paste_flag}')" = 1 ] && break; /bin/sleep 0.1; done
    [ "$(T display-message -p -t "$card" '#{bracket_paste_flag}')" = 1 ] || { say "card $card: no shell prompt"; exit 3; }
  done
  T show-environment -g > "$EV/server-env.txt"
  say "server pid=$(cat "$REG/server") socket=$SOCKPATH sessions=a,b"
  ;;
start)
  CARD=${1:?card}; case $CARD in a|b) ;; *) refuse "card: a|b";; esac
  root_checked && have server || refuse "no server"
  have "start-$CARD" && refuse "card $CARD was started already: once only"
  [ "$(fgpid "$CARD" 2>/dev/null)" = "" ] || refuse "card $CARD already has a codex child"
  case $CARD in a) LINE="'$CODEX' --no-daemon --yolo";; b) LINE="'$CODEX' --no-daemon";; esac
  date +%s > "$REG/start-$CARD"
  T send-keys -t "$CARD" -l "$LINE" && T send-keys -t "$CARD" Enter || exit 2
  P=""; for i in $(seq 1 100); do P=$(fgpid "$CARD" 2>/dev/null) && break; /bin/sleep 0.2; done
  [ -n "$P" ] || { say "card $CARD: no codex foreground"; cap "$CARD" > "$EV/screen-nostart-$CARD.txt"; exit 3; }
  $REGISTRY add-process --root "$ROOT" --pid "$P" || exit 3
  ident "$P" > "$REG/client-$CARD"
  for i in $(seq 1 150); do cap "$CARD" | grep -q "Ask Codex to do anything" && break; /bin/sleep 0.2; done
  cap "$CARD" | grep -v "⣿\|⠿\|⣀\|⣤" > "$EV/screen-start-$CARD.txt"
  if ! grep -q "Ask Codex to do anything" "$EV/screen-start-$CARD.txt"; then say "card $CARD: the TUI did not reach its input; screen kept"; exit 3; fi
  if grep -qi "trust\|untrusted\|hooks need" "$EV/screen-start-$CARD.txt"; then say "card $CARD: a trust notice is on the screen; stop"; exit 3; fi
  /bin/ps -o pid=,ppid=,pgid=,tty=,lstart=,command= -p "$P" | sed "s#$ROOT#<ROOT>#g" > "$EV/client-$CARD.txt"
  say "card $CARD started: $(cat "$REG/client-$CARD")"
  ;;
ids)
  # The thread each registered client holds: the one writer lock file it
  # keeps open (`<CODEX_HOME>/thread-writer-locks/<thread id>.lock`), read
  # from the kernel's descriptor table of that exact pid. An empty session
  # has no rollout yet; the lock is held from the thread's creation.
  root_checked || refuse "no such campaign"
  SALT=$(get salt)
  FIN=""
  : > "$REG/thread.private"; chmod 600 "$REG/thread.private"
  for card in a b; do
    pid=$(awk '{print $1}' "$REG/client-$card" 2>/dev/null); [ -n "$pid" ] || { say "card $card: no registered client"; exit 3; }
    locks=$(/usr/sbin/lsof -p "$pid" 2>/dev/null | awk '{print $NF}' | grep -F "/h/.codex/thread-writer-locks/" | grep -v "coordination" || true)
    n=$(echo "$locks" | grep -c .)
    [ "$n" = 1 ] || { say "card $card: $n writer locks held by its client, not one"; exit 3; }
    id=$(basename "$locks" .lock)
    case $id in [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]-*) ;; *) say "card $card: the lock is not named by a uuid"; exit 3;; esac
    echo "$card=$id" >> "$REG/thread.private"
    FIN="$FIN${FIN:+,}$card=$(digest "$SALT" "$id")"
  done
  echo "$FIN" > "$REG/finals"
  say "ids: one writer lock per client; finals (digests) recorded"
  ;;
transact)
  root_checked && have server || refuse "no server"
  have transacted && refuse "the transaction was already run: once only"
  [ -s "$REG/finals" ] || refuse "run ids first"
  for card in a b; do have "start-$card" || refuse "card $card was not started"; done
  POLICY=$(get policy); SALT=$(get salt)
  date +%s > "$REG/transacted"
  say "transaction begins at $(date +%H:%M:%S): policy=$POLICY cards a=bypass,b=ordinary expect started"
  out=$(env HOME="$ROOT/ctlhome" TMPDIR="$ROOT/data" DECK_M1_BASE="$BASE" DECK_M1_SOCKET="$SOCK" DECK_M1_HOME="$ROOT/h" DECK_M1_ZDOT="$ROOT/zdot" \
      DECK_M1_HELPER="$HELPER" DECK_M1_CLIENT="$CODEX" DECK_M1_CARDS="a=bypass,b=ordinary" DECK_M1_EXPECT="a=started,b=started" \
      DECK_M1_POLICY="$POLICY" DECK_M1_SALT="$SALT" DECK_M1_FINAL="$(cat "$REG/finals")" DECK_M1_DATA="$ROOT/data" DECK_M1_ENV="TMUX_TMPDIR=$ROOT/t" DECK_M1_TMUX="$TMUXBIN" \
      "$PROBE" --exact agent_reentry::tests::native_m1_transact --ignored --nocapture 2>"$EV/transact.stderr"); rc=$?
  printf '%s\n' "$out" > "$EV/transact.out"
  printf '%s\n' "$out" | grep '^M1 ' | sed 's/^/  /' | tee -a "$EV/log.txt"
  failed=$(printf '%s\n' "$out" | sed -n 's/^M1 result failed-checks=\([0-9]*\)$/\1/p')
  say "transaction ended at $(date +%H:%M:%S): controller exit=$rc failed-checks=${failed:-unknown}"
  [ $rc -eq 0 ] && [ "$failed" = 0 ] || exit 3
  ;;
observe)
  root_checked && have transacted || refuse "no transaction"
  have observed && refuse "observed already"
  date +%s > "$REG/observed"
  verdict=0
  # the replacement server the transaction started answers on the same
  # socket path: registered as its own resource (the old one is gone)
  $REGISTRY add-server --root "$ROOT" --socket "$SOCKPATH" --tmux "$TMUXBIN" || verdict=3
  T display-message -p '#{pid}' > "$REG/server-new"
  for card in a b; do
    P=""; for i in $(seq 1 50); do P=$(fgpid "$card" 2>/dev/null) && break; /bin/sleep 0.2; done
    if [ -z "$P" ]; then say "card $card: no new codex foreground"; cap "$card" > "$EV/screen-after-$card.txt"; verdict=3; continue; fi
    $REGISTRY add-process --root "$ROOT" --pid "$P" || verdict=3
    ident "$P" > "$REG/newclient-$card"
    id=$(sed -n "s/^$card=//p" "$REG/thread.private")
    # the started client's own arguments, with the id replaced by its label
    /bin/ps -o pid=,ppid=,command= -p "$P" | sed "s#$ROOT#<ROOT>#g; s#$id#<id-$card>#g" > "$EV/newclient-$card.txt"
    for i in $(seq 1 100); do cap "$card" | grep -q "Ask Codex to do anything" && break; /bin/sleep 0.2; done
    cap "$card" | grep -v "⣿\|⠿\|⣀\|⣤" | sed "s#$id#<id-$card>#g" > "$EV/screen-after-$card.txt"
    # the operator's one input: Codex's own status view
    T send-keys -t "$card" -l "/status"; /bin/sleep 0.6; T send-keys -t "$card" Enter; /bin/sleep 1.5
    cap "$card" | grep -E "Permissions:|Session:|YOLO|Directory:" | sed "s#$ROOT#<ROOT>#g" > "$EV/status-raw-$card.txt"
    if grep -q "Session: *$id" "$EV/status-raw-$card.txt"; then echo "session=matches-the-exited-one" > "$EV/status-$card.txt"; else echo "session=OTHER-OR-UNREAD" > "$EV/status-$card.txt"; verdict=3; fi
    grep -E "Permissions:" "$EV/status-raw-$card.txt" | sed 's/^ *//' >> "$EV/status-$card.txt"
    sed -i '' "s#$id#<id-$card>#g" "$EV/status-raw-$card.txt"
    say "card $card: new client $(cat "$REG/newclient-$card") $(cat "$EV/status-$card.txt" | tr '\n' ' ')"
  done
  python3 -I "$HERE/ids.py" "$ROOT/h/.codex" | awk -F'\t' '{print $1}' | sed "s#$ROOT#<ROOT>#g" | sort | uniq -c > "$EV/rollouts-after.txt"
  exit $verdict
  ;;
end)
  root_checked || refuse "no such campaign"
  say "end begins"
  $REGISTRY cleanup --root "$ROOT" | tee -a "$EV/log.txt"; rc=${PIPESTATUS[0]}
  /bin/sleep 1
  $REGISTRY show --root "$ROOT" | tee -a "$EV/log.txt"
  left=$(running_from_root)
  if [ -n "$left" ]; then say "STILL RUNNING from the root: $left"; exit 4; fi
  [ -S "$SOCKPATH" ] && rm -f "$SOCKPATH"
  say "end done: cleanup exit=$rc"
  [ "$rc" = 0 ] || exit 4
  ;;
destroy)
  root_checked || refuse "no such campaign"
  left=$(running_from_root)
  [ -z "$left" ] || refuse "something still runs from the root"
  rm -rf "$ROOT" && echo "[$CAMPAIGN] destroyed $ROOT"
  ;;
*) refuse "unknown command $CMD";;
esac
