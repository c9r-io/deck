#!/bin/zsh -f
# Deck Controlled Execution calibration probe (CE1).
#
#   probe.zsh [--hashed] <out.json> <kind> [args...]
#
# Runs INSIDE a lane (a Deck-like terminal, or an MCP job) and writes one
# JSON observation to <out.json> (atomically: a temporary name, then mv). It
# observes; it never decides. Classification belongs to scripts/ce_verdict.py.
#
# Privacy: no environment value, secret, argv or terminal output is ever
# written. Environment variables are reported by presence; the few
# coordinates compared by value (`env-coord`) are reported as SHA-256
# digests. Executable identities are full paths in the synthetic CI lanes
# and, with --hashed (designated lanes), a path class plus a SHA-256 prefix.
#
# `zsh -f` keeps this probe from reading any startup file of its own, so it
# sees exactly the environment of the lane that started it. Every helper it
# needs is named by absolute path; only the tool under observation is
# resolved through the lane's PATH.
emulate -L zsh
setopt pipe_fail

hashed=0
if [[ ${1-} == --hashed ]]; then hashed=1; shift; fi
out=$1 kind=$2
shift 2

# JSON string: control characters dropped, backslash and quote escaped.
je() {
  local s=${1-}
  s=${s//[[:cntrl:]]/}
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  print -rn -- "\"$s\""
}

emit() {
  print -r -- "$1" > "$out.tmp" && /bin/mv -f "$out.tmp" "$out"
}

path_class() {
  local p=$1 home=${HOME-}
  if [[ -n $home && $p == $home/* ]]; then print -rn user-home
  elif [[ $p == /opt/homebrew/* || $p == /usr/local/* ]]; then print -rn homebrew-prefix
  elif [[ $p == /usr/* || $p == /bin/* || $p == /sbin/* || $p == /System/* || $p == /Library/* ]]; then print -rn system
  else print -rn other
  fi
}

identity() {
  if (( hashed )); then
    local digest=$(print -rn -- "$1" | /usr/bin/shasum -a 256)
    je "sha256:${digest[1,16]}"
  else
    je "$1"
  fi
}

first_line_of_version() {
  "$1" --version </dev/null 2>/dev/null | /usr/bin/head -n 1
}

# Launcher class from the file itself, never from a tool-manager brand:
# native (Mach-O), script-env (#!/usr/bin/env X), script-absolute (#!/path),
# or other. For scripts the interpreter is resolved one level, in this lane.
describe_launcher() {
  local real=$1 magic
  magic=$(/usr/bin/od -An -tx1 -N4 -- "$real" 2>/dev/null | /usr/bin/tr -d ' \n')
  L_CLASS=other L_INTERP=""
  if [[ $magic == 2321* ]]; then
    local line words interp=""
    IFS= read -r line < "$real"
    line=${line#\#!}
    words=(${=line})
    if [[ ${words[1]-} == /usr/bin/env ]]; then
      L_CLASS=script-env
      local w
      for w in "${(@)words[2,-1]}"; do
        [[ $w == -* ]] && continue
        interp=$w; break
      done
      [[ -n $interp ]] && L_INTERP=$(whence -p -- "$interp" 2>/dev/null)
    else
      L_CLASS=script-absolute
      L_INTERP=${words[1]-}
    fi
  elif [[ $magic == cffaedfe || $magic == cafebabe || $magic == feedfacf ]]; then
    L_CLASS=native
  fi
}

# One tool observation: $1 logical name, $2 the resolved path (empty = not found).
report_tool() {
  local name=$1 found=$2 real version interp_json=null
  if [[ -z $found ]]; then
    emit "{\"kind\":\"tool\",\"tool\":$(je "$name"),\"found\":false}"
    return
  fi
  real=${found:A}
  version=$(first_line_of_version "$found")
  describe_launcher "$real"
  if [[ -n $L_INTERP && -x $L_INTERP ]]; then
    local ireal=${L_INTERP:A}
    interp_json="{\"identity\":$(identity "$ireal"),\"path_class\":$(je "$(path_class "$ireal")"),\"version\":$(je "$(first_line_of_version "$ireal")")}"
  elif [[ $L_CLASS == script-* ]]; then
    interp_json="{\"identity\":null,\"path_class\":null,\"version\":null}"
  fi
  emit "{\"kind\":\"tool\",\"tool\":$(je "$name"),\"found\":true,\"identity\":$(identity "$real"),\"path_class\":$(je "$(path_class "$real")"),\"version\":$(je "$version"),\"launcher_class\":$(je "$L_CLASS"),\"interpreter\":$interp_json}"
}

case $kind in
  (tool)
    # resolved through this lane's PATH
    report_tool "$1" "$(whence -p -- "$1" 2>/dev/null)"
    ;;
  (script)
    # a script named by ABSOLUTE path, as a structured direct launch names it;
    # only its #!/usr/bin/env interpreter is resolved through this lane's PATH
    if [[ -x $1 ]]; then report_tool "${1:t}" "$1"; else report_tool "${1:t}" ""; fi
    ;;
  (noop)
    emit "{\"kind\":\"noop\",\"capable\":true}"
    ;;
  (ssh-agent-reach)
    # reachability only: connect and close, no agent request, no key material
    local present=false capable=false detail=no-variable
    if [[ -n ${SSH_AUTH_SOCK-} ]]; then
      present=true detail=connect-failed
      if zmodload zsh/net/socket 2>/dev/null && zsocket "$SSH_AUTH_SOCK" 2>/dev/null; then
        local fd=$REPLY
        exec {fd}>&-
        capable=true detail=connect-ok
      fi
    fi
    emit "{\"kind\":\"ssh-agent-reach\",\"capable\":$capable,\"variable_present\":$present,\"detail\":\"$detail\"}"
    ;;
  (env)
    local name fields=() all=true
    for name in "$@"; do
      if [[ -n ${(P)name+set} ]]; then fields+=("$(je "$name"):true")
      else fields+=("$(je "$name"):false"); all=false
      fi
    done
    emit "{\"kind\":\"env\",\"capable\":$all,\"present\":{${(j:,:)fields}}}"
    ;;
  (env-coord)
    local name fields=()
    for name in "$@"; do
      if [[ -n ${(P)name+set} ]]; then
        local digest=$(print -rn -- "${(P)name}" | /usr/bin/shasum -a 256)
        fields+=("$(je "$name"):$(je "sha256:${digest[1,16]}")")
      else
        fields+=("$(je "$name"):null")
      fi
    done
    emit "{\"kind\":\"env-coord\",\"digests\":{${(j:,:)fields}}}"
    ;;
  (ssh-agent)
    local present=false rc
    [[ -n ${SSH_AUTH_SOCK+set} ]] && present=true
    /usr/bin/ssh-add -l </dev/null >/dev/null 2>&1
    rc=$?
    local capable=false
    (( rc == 0 || rc == 1 )) && capable=true
    emit "{\"kind\":\"ssh-agent\",\"capable\":$capable,\"variable_present\":$present,\"detail\":\"ssh-add-exit-$rc\"}"
    ;;
  (tmp)
    local d capable=false
    d=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/ce-probe.XXXXXX" 2>/dev/null) \
      && /bin/mkdir -p "$d/a/b/c" \
      && print -r x > "$d/a/b/c/f" \
      && [[ $(<"$d/a/b/c/f") == x ]] \
      && /bin/rm -rf "$d" \
      && [[ ! -e $d ]] && capable=true
    emit "{\"kind\":\"tmp\",\"capable\":$capable}"
    ;;
  (read)
    local capable=false
    [[ -r $1 && $(<"$1") == "$2" ]] && capable=true
    emit "{\"kind\":\"read\",\"capable\":$capable}"
    ;;
  (tcp)
    local capable=false line=""
    if zmodload zsh/net/tcp 2>/dev/null && ztcp 127.0.0.1 "$1" 2>/dev/null; then
      local fd=$REPLY
      IFS= read -r -t 5 line <&$fd
      ztcp -c "$fd" 2>/dev/null
      [[ $line == "$2" ]] && capable=true
    fi
    emit "{\"kind\":\"tcp\",\"capable\":$capable}"
    ;;
  (unix)
    local capable=false line=""
    if zmodload zsh/net/socket 2>/dev/null && zsocket "$1" 2>/dev/null; then
      local fd=$REPLY
      IFS= read -r -t 5 line <&$fd
      exec {fd}>&-
      [[ $line == "$2" ]] && capable=true
    fi
    emit "{\"kind\":\"unix\",\"capable\":$capable}"
    ;;
  (tree)
    local d capable=false
    d=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/ce-tree.XXXXXX") || d=""
    if [[ -n $d ]]; then
      # child → grandchild → marker, each a separate process
      /bin/sh -c '/bin/sh -c "echo ok > \"\$1\"" grandchild "$1"' child "$d/marker"
      [[ -f $d/marker && $(<"$d/marker") == ok ]] && capable=true
      /bin/rm -rf "$d"
    fi
    emit "{\"kind\":\"tree\",\"capable\":$capable}"
    ;;
  (git)
    local d capable=false git
    git=$(whence -p git 2>/dev/null)
    d=$(/usr/bin/mktemp -d "${TMPDIR:-/tmp}/ce-git.XXXXXX") || d=""
    if [[ -n $git && -n $d ]]; then
      ( cd "$d" \
        && "$git" init -q \
        && print -r ce > f \
        && "$git" add f \
        && "$git" commit -q -m ce \
        && "$git" rev-parse --verify -q HEAD ) >/dev/null 2>&1 && capable=true
    fi
    [[ -n $d ]] && /bin/rm -rf "$d"
    emit "{\"kind\":\"git\",\"capable\":$capable}"
    ;;
  (compose)
    local capable=false
    [[ $(print -r abc | /usr/bin/tr a z) == zbc ]] && capable=true
    emit "{\"kind\":\"compose\",\"capable\":$capable}"
    ;;
  (cwd)
    local capable=false
    [[ ${PWD:A} == ${1:A} ]] && capable=true
    emit "{\"kind\":\"cwd\",\"capable\":$capable}"
    ;;
  (https-head)
    local capable=false
    /usr/bin/curl -sS -o /dev/null -I --max-time 10 -- "$1" >/dev/null 2>&1 && capable=true
    emit "{\"kind\":\"https-head\",\"capable\":$capable}"
    ;;
  (docker-version)
    local capable=false docker
    docker=$(whence -p docker 2>/dev/null)
    [[ -n $docker ]] && "$docker" --version >/dev/null 2>&1 && capable=true
    emit "{\"kind\":\"docker-version\",\"capable\":$capable}"
    ;;
  (*)
    emit "{\"kind\":\"error\",\"error\":\"unknown-probe-kind\"}"
    exit 64
    ;;
esac
