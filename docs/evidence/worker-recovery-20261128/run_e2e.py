#!/usr/bin/env python3
"""A WKWebView smoke of this checkout's debug build on a test Mac (default air2).
2026-11-28 (close and queue reliability): mode `worker-recovery`, two phases
around a relaunch of the carrier on the same data and tmux server; its
scripted orchestrator (scripts/mcp-recovery-e2e.py) runs INSIDE "Orch R".
2026-11-28 (round 2): mode `worker-close`, its scripted orchestrator run INSIDE
the page's "Orch C" card (scripts/mcp-close-e2e.py), a real Claude worker that
reports (handoff) and ends its turn, then the instance-bound close.
2026-11-28 (usage quota): modes `quota-live` (the host's signed-in Claude Code
and a Codex home of the run's own) and `quota-switch` (the Settings switch on
the global files of a home of the run's own, the carrier at
<home>/Applications/deck.app).
2026-11-27 (interjected worker lines): mode `worker-interject`, its scripted
orchestrator run INSIDE the page's "Orch I" card (scripts/mcp-interject-e2e.py).
Earlier:
2026-11-17 (card snapshot round): `attention`, or `cards-mcp` with its scripted
client and a real Claude Code orchestrator. Adapted from
~/c9r-io/deck-ux-layers-20261008/smoke_run.py, with the AGENTS.md experiment
registry: everything the run starts lives under ONE unique /private/tmp root,
the carrier's tmux socket directory included (open --env TMUX_TMPDIR), each
server and process is registered as it starts, and the registry's own
`cleanup` is the only thing that stops them. Isolation is passed as launch
ARGUMENTS and checked within 20 s. The machine's own ~/.deck, its installed
Deck and its `deck` tmux server are recorded before and after and never
touched. The app.log is fetched and judged locally (the mini has no node).
"""
import argparse, json, os, plistlib, shlex, subprocess, sys, tempfile, time, uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
ap = argparse.ArgumentParser()
ap.add_argument("--host", default="air2")
ap.add_argument("--mode", default="workers")
ap.add_argument("--label", default="")
ap.add_argument("--timeout", type=int, default=420)
ap.add_argument("--codex", action="store_true", help="cards-mcp: also a codex --no-daemon card")
ap.add_argument("--run", default="", help="reuse this run id (its root prepared by --phase prepare)")
ap.add_argument("--phase", default="all", choices=["all", "prepare", "run"],
                help="workers-account: prepare (root, fixture, account folder) / run (after the sign-in)")
OPTS = ap.parse_args()
HOST = OPTS.host
SSH = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", HOST]
WORKTREE = Path("/Users/chenhan/c9r-io/deck"); TAURI = WORKTREE / "app/src-tauri"
# built in its own target directory on the external disk (the gate runs in the default one)
BIN = Path("/Volumes/Yotta/deck-round2-target/debug/deck-app")
EDR = WORKTREE / "scripts/edr_runtime.py"
SIDECARS = ["deck-status-helper", "deck-lifecycle-helper", "deck-mcp", "deck-mcp-runner"]
PI_RESOURCES = sorted(p.name for p in (TAURI / "resources/pi").iterdir() if p.is_file())
RUN = OPTS.run or ("v" + uuid.uuid4().hex[:8])
ROOT = f"/private/tmp/deck-verify-{RUN}"
APP = f"{ROOT}/deck-smoke.app"; EXE = f"{APP}/Contents/MacOS/deck"; TMUX = f"{APP}/Contents/MacOS/tmux"
TMUX_TMPDIR = f"{ROOT}/t"; SOCKET = f"deck-smoke-{RUN}"; UID = 501
SOCKET_PATH = f"{TMUX_TMPDIR}/tmux-{UID}/{SOCKET}"
DATA = f"{ROOT}/run/data"
MODE, TERMINAL, TIMEOUT = OPTS.mode, "done", OPTS.timeout
if MODE == "quota-switch":
    # hooks are written only from <home>/Applications/deck.app
    APP = f"{ROOT}/home/Applications/deck.app"; EXE = f"{APP}/Contents/MacOS/deck"; TMUX = f"{APP}/Contents/MacOS/tmux"
OUT = HERE / f"{HOST}-smoke-{OPTS.label + '-' if OPTS.label else ''}{MODE}-{RUN}"
FIXTURE = f"{DATA}/translation-fixture"
APP_MCP = f"{APP}/Contents/MacOS/deck-mcp"
HELPER = f"{APP}/Contents/MacOS/deck-status-helper"
# the second account's config folder: under the run root, signed in by the
# person between the phases (workers-account) or left unsigned (reentry-shapes)
ACCOUNT = f"{ROOT}/account"


def remote(script, check=True, timeout=180):
    p = subprocess.run(SSH + ["sh -c " + shlex.quote(script)], capture_output=True, text=True, timeout=timeout)
    if check and p.returncode != 0:
        raise RuntimeError(f"remote failed ({p.returncode}): {p.stderr.strip() or p.stdout.strip()}")
    return p.stdout


def registry(sub, extra=""):
    """edr_runtime.py on the mini, read from stdin as AGENTS.md says."""
    with open(EDR) as stdin:
        p = subprocess.run(SSH + [f"python3 - experiment --root {shlex.quote(ROOT)} {extra} {sub}"],
                           stdin=stdin, capture_output=True, text=True, timeout=120)
    return p.returncode, (p.stdout + p.stderr).strip()


def machine_state():
    return remote("ls -lad ~/.deck 2>/dev/null | cut -c1-60 | sed \"s|$HOME|~|\"; "
                  "ls /private/tmp/tmux-$(id -u)/ 2>/dev/null | tr '\\n' ' '; echo; "
                  "pgrep -lf '/Applications/deck.app/Contents/MacOS' | cut -c1-90", check=False).strip()


def front():
    return remote("lsappinfo info -only name \"$(lsappinfo front)\" 2>/dev/null | head -1 | sed -E 's/^\"([^\"]*)\".*/\\1/'",
                  check=False).strip()


def build_carrier(work):
    b = work / "deck-smoke.app"; (b / "Contents/MacOS").mkdir(parents=True); (b / "Contents/Resources/pi").mkdir(parents=True)
    cp = lambda s, d: subprocess.run(["cp", str(s), str(d)], check=True)
    cp(BIN, b / "Contents/MacOS/deck")
    cp(TAURI / "binaries/tmux-aarch64-apple-darwin", b / "Contents/MacOS/tmux")
    for name in SIDECARS:
        cp(TAURI / f"binaries/{name}-aarch64-apple-darwin", b / "Contents/MacOS" / name)
    for name in PI_RESOURCES:
        cp(TAURI / "resources/pi" / name, b / "Contents/Resources/pi" / name)
    cp(TAURI / "icons/icon.png", b / "Contents/Resources/icon.png"); cp(TAURI / "icons/icon.icns", b / "Contents/Resources/deck.icns")
    ver = json.load(open(TAURI / "tauri.conf.json"))["version"]
    plistlib.dump({"CFBundleName": "deck smoke", "CFBundleDisplayName": "deck smoke", "CFBundleExecutable": "deck",
        "CFBundleIdentifier": "io.c9r.deck.smoke", "CFBundleVersion": ver, "CFBundleShortVersionString": ver,
        "CFBundleIconFile": "deck", "CFBundlePackageType": "APPL", "NSHighResolutionCapable": True,
        "NSMicrophoneUsageDescription": "Record your voice to compose a prompt using on-device recognition.",
        "NSSpeechRecognitionUsageDescription": "Convert your voice into an editable prompt on this Mac.",
        "LSMinimumSystemVersion": "11.0",
        "NSLocalNetworkUsageDescription": "Terminal tools running in deck may connect to local services and devices you choose."},
        open(b / "Contents/Info.plist", "wb"))
    subprocess.run(["codesign", "--force", "--sign", "-", "--entitlements", str(TAURI / "Entitlements.plist"), str(b)], check=True, capture_output=True)
    return ver


def checkpoint(name):
    out = remote(f"grep -c 'smoke-check {name} ' {DATA}/app.log 2>/dev/null; true", check=False).strip()
    return int((out or "0").splitlines()[-1] or 0)


def wait_checkpoint(name, seconds):
    end = time.time() + seconds
    while time.time() < end:
        if checkpoint(name) > 0:
            return True
        if checkpoint("workers-exception") > 0 or checkpoint(TERMINAL) > 0:
            return False
        time.sleep(3)
    return False


TRUST = """import json,os,sys
p=os.path.expanduser('~/.claude.json'); d=json.load(open(p)); w=sys.argv[2]
pr=d.setdefault('projects',{})
if sys.argv[1]=='add':
    pr[w]=dict(pr.get(w,{}), hasTrustDialogAccepted=True, hasCompletedProjectOnboarding=True)
else:
    pr.pop(w,None)
t=p+'.deck-smoke'; json.dump(d,open(t,'w'),indent=2); os.chmod(t,0o600); os.replace(t,p)
print(sys.argv[1], w in pr)"""


def trust(action):
    """Trust exactly this run's work directory in the host's Claude Code
    config (added before the launch, that one entry removed after)."""
    return remote(f"python3 -c {shlex.quote(TRUST)} {action} {shlex.quote(FIXTURE + '/work')}", check=False).strip()


CODEX_BIN_DIR = "$HOME/.nvm/versions/node/v22.23.4/bin"


def prepare_codex():
    """A Codex of its own for the "Codex done" card: CODEX_HOME under the
    root with a copy of the host's sign-in (deleted after the run), the work
    directory trusted, and Deck's Codex Signal hooks in the forwarding form
    calling this carrier's helper (as Deck writes them, shell form)."""
    home = f"{ROOT}/codex-home"
    cmd = lambda state: {"type": "command", "command": f'"{HELPER}" codex {state}', "timeout": 10, "async": True}
    hooks = {"hooks": {
        "UserPromptSubmit": [{"hooks": [cmd("working")]}],
        "PermissionRequest": [{"hooks": [cmd("needs-input")]}],
        "Stop": [{"hooks": [cmd("turn-done --reply workers")]}],
        "Interrupt": [{"hooks": [cmd("turn-done")]}],
    }}
    config = f'model_reasoning_effort = "low"\n[projects."{FIXTURE}/work"]\ntrust_level = "trusted"\n'
    script = (f"#!/bin/sh\nexport PATH={CODEX_BIN_DIR}:$PATH CODEX_HOME={home}\n"
              f"exec codex --no-daemon --dangerously-bypass-hook-trust 'Do not use any tool. Reply with exactly this line and nothing else: Finished. CODEX-DONE-OK'\n")
    remote(f"mkdir -m 700 -p {home} && install -m 600 ~/.codex/auth.json {home}/auth.json && "
           f"printf %s {shlex.quote(config)} > {home}/config.toml && printf %s {shlex.quote(json.dumps(hooks))} > {home}/hooks.json && "
           f"printf %s {shlex.quote(script)} > {FIXTURE}/codex.sh && printf cards-codex > {FIXTURE}/scenario")


def prepare_report_fixture():
    """`workers`, scenario `types`: the worker type's Claude Code reads the
    fixture's settings (Signal hooks to this carrier, the project's MCP
    servers enabled, the one report tool allowed) and, once the page is
    armed, a project .mcp.json naming this instance's adapter."""
    hook = lambda *state: {"type": "command", "command": HELPER, "args": ["claude-code", *state], "timeout": 10}
    settings = {"hooks": {
        "UserPromptSubmit": [{"hooks": [hook("working")]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [hook("needs-input")]}],
        "Stop": [{"hooks": [hook("turn-done", "--reply", "workers")]}],
    }, "enableAllProjectMcpServers": True, "permissions": {"allow": ["mcp__deck__deck_worker_report"]}}
    remote(f"mkdir -p {FIXTURE}/work && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && printf types > {FIXTURE}/scenario && "
           f"printf %s {shlex.quote(json.dumps(settings))} > {FIXTURE}/worker-hooks.json")
    remote(f"mkdir -p {ROOT}/scripts")
    for name in ("mcp-worker-e2e.py", "mcp-agent-e2e.py"):
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(WORKTREE / "scripts" / name), f"{HOST}:{ROOT}/scripts/{name}"], check=True)


def prepare_account_fixture():
    """workers-account / workers-capacity / reentry-shapes: copy_agent.py,
    work/, the Signal hooks file, and `account-root` naming ACCOUNT (an
    empty folder until the person signs it in)."""
    hook = lambda *state: {"type": "command", "command": HELPER, "args": ["claude-code", *state], "timeout": 10}
    settings = {"hooks": {
        "UserPromptSubmit": [{"hooks": [hook("working")]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [hook("needs-input")]}],
        "Stop": [{"hooks": [hook("turn-done", "--reply", "workers")]}],
    }}
    remote(f"mkdir -p {FIXTURE}/work && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && "
           f"mkdir -m 700 -p {ACCOUNT} && printf %s {ACCOUNT} > {FIXTURE}/account-root && "
           f"printf %s {shlex.quote(json.dumps(settings))} > {FIXTURE}/worker-hooks.json")
    remote(f"mkdir -p {ROOT}/scripts")
    for name in ("mcp-worker-e2e.py", "mcp-agent-e2e.py", "mcp-capacity-e2e.py"):
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(WORKTREE / "scripts" / name), f"{HOST}:{ROOT}/scripts/{name}"], check=True)


CLOSE_PHASE1 = """import time
for i in range(4):
    print("phase1", i * 10, "s", flush=True)
    time.sleep(10)
print("phase1 done", flush=True)
"""


def prepare_close_fixture():
    """`worker-close`: the report fixture of `workers` (Deck's Claude hooks to
    this carrier, PermissionRequest and the after-tool word included, the
    project's MCP server enabled, the report tool and the program allowed)
    and a 40 s foreground program."""
    hook = lambda *state: {"type": "command", "command": HELPER, "args": ["claude-code", *state], "timeout": 10}
    settings = {"hooks": {
        "UserPromptSubmit": [{"hooks": [hook("working")]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [hook("needs-input")]}],
        "PermissionRequest": [{"hooks": [hook("needs-input")]}],
        "PostToolUse": [{"hooks": [hook("working", "--after-tool")]}],
        "Stop": [{"hooks": [hook("turn-done", "--reply", "workers")]}],
    }, "enableAllProjectMcpServers": True,
        "permissions": {"allow": ["mcp__deck__deck_worker_report", "Bash(python3 phase1.py:*)"]}}
    remote(f"mkdir -p {FIXTURE}/work && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && "
           f"printf %s {shlex.quote(json.dumps(settings))} > {FIXTURE}/worker-hooks.json && "
           f"printf %s {shlex.quote(CLOSE_PHASE1)} > {FIXTURE}/work/phase1.py")
    remote(f"mkdir -p {ROOT}/scripts")
    for name in ("mcp-close-e2e.py", "mcp-agent-e2e.py"):
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(WORKTREE / "scripts" / name), f"{HOST}:{ROOT}/scripts/{name}"], check=True)


RECOVERY_PHASE1 = """import time
open("p1-started", "w").close()
for i in range(58):
    print("phase1", i * 10, "s", flush=True)
    time.sleep(10)
open("p1-done", "w").close()
print("phase1 done", flush=True)
"""


def prepare_recovery_fixture():
    """`worker-recovery`: Deck's Claude hooks to this carrier (as
    `worker-close`, without the MCP server) and a ~9.5 minute foreground
    program, the only command allowed."""
    hook = lambda *state: {"type": "command", "command": HELPER, "args": ["claude-code", *state], "timeout": 10}
    settings = {"hooks": {
        "UserPromptSubmit": [{"hooks": [hook("working")]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [hook("needs-input")]}],
        "PermissionRequest": [{"hooks": [hook("needs-input")]}],
        "PostToolUse": [{"hooks": [hook("working", "--after-tool")]}],
        "Stop": [{"hooks": [hook("turn-done", "--reply", "workers")]}],
    }, "permissions": {"allow": ["Bash(python3 phase1.py:*)"]}}
    remote(f"mkdir -p {FIXTURE}/work && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && "
           f"printf %s {shlex.quote(json.dumps(settings))} > {FIXTURE}/worker-hooks.json && "
           f"printf %s {shlex.quote(RECOVERY_PHASE1)} > {FIXTURE}/work/phase1.py")
    remote(f"mkdir -p {ROOT}/scripts")
    for name in ("mcp-recovery-e2e.py", "mcp-agent-e2e.py"):
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(WORKTREE / "scripts" / name), f"{HOST}:{ROOT}/scripts/{name}"], check=True)


RECOVERY_PANE = {}


def recovery_operator(t0, phase):
    """Phase 1 after `recovery-armed` (the page's one pane is "Orch R"),
    phase 2 after `recovery-restored` (the same pane, still alive in the same
    server): the client typed into it, its evidence awaited."""
    facts = {}
    armed = "recovery-armed" if phase == 1 else "recovery-restored"
    if not wait_checkpoint(armed, 420):
        facts["armed"] = False
        return facts
    client, cred, sock = client_args()
    env = f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)}"
    panes = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} list-panes -a -F '#{{session_name}} #{{pane_id}} #{{pane_current_command}}'",
                   check=False).strip().splitlines()
    facts["panesAtArmed"] = panes
    if phase == 1:
        if len(panes) != 1:
            facts["error"] = "not exactly one pane at armed"
            return facts
        RECOVERY_PANE["pane"] = panes[0].split()[1]
    pane = RECOVERY_PANE.get("pane")
    if not pane or not any(p.split()[1] == pane for p in panes):
        facts["error"] = "Orch R's pane is not there"
        return facts
    evidence = f"{DATA}/evidence/recovery"
    out = f"{evidence}/recovery-e2e-{RUN}-phase{phase}.json"
    line = (f"python3 {ROOT}/scripts/mcp-recovery-e2e.py --phase {phase} --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
            f"--client-id {client} --credential-file {shlex.quote(cred)} --evidence {evidence} --tag {RUN} --tmux {shlex.quote(TMUX)}")
    remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} -l {shlex.quote(line)} && "
           f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} Enter")
    facts["clientTypedAt"] = round(time.time() - t0, 1)
    end = time.time() + (2400 if phase == 1 else 900)
    while time.time() < end:
        if remote(f"test -f {out} && ! grep -q '\"partial\": true' {out} && echo y", check=False).strip() == "y":
            break
        time.sleep(10)
    facts["evidence"] = remote(f"cat {out} 2>/dev/null", check=False)[-16000:]
    facts["orchPane"] = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} capture-pane -p -J -t {pane} -S -120 2>/dev/null | grep -v '^[[:space:]]*$' | tail -80",
                               check=False)[-9000:]
    facts["ledger"] = remote(f"python3 -c \"import json;d=json.load(open('{DATA}/mcp.json'));print(d.get('version'), [(o['kind'], o['state'], o.get('code')) for o in d.get('operations', []) if o['kind'] in ('worker-close', 'worker-cancel')])\"", check=False).strip()
    facts["queueOps"] = remote(f"python3 -c \"import json;d=json.load(open('{DATA}/queue.json'));print([(o['id'][-6:], o['state']) for o in d.get('operations', [])])\"", check=False).strip()
    return facts


SF_SERVER = """import http.server, json, sys
class H(http.server.BaseHTTPRequestHandler):
    def _r(self):
        n = int(self.headers.get('content-length') or 0)
        self.rfile.read(n)
        with open(sys.argv[2], 'a') as f:
            f.write(self.command + ' ' + self.path + chr(10))
        body = json.dumps({"type": "error", "error": {"type": "invalid_request_error", "message": "deck probe: forced failure"}}).encode()
        self.send_response(400)
        self.send_header('content-type', 'application/json')
        self.send_header('content-length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    do_POST = _r
    do_GET = _r
    def log_message(self, *a):
        pass
http.server.ThreadingHTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
"""


def stopfailure_operator():
    """Phase 3, in "Orch R"'s pane while the page still holds it: a loopback
    API that answers every request 400, an isolated Claude config folder
    (a fake API key, this carrier's helper as the UserPromptSubmit and
    StopFailure hooks, plus a tee keeping the payload), and one `claude -p`
    turn typed into the pane. No real credential, folder or quota is used."""
    facts = {}
    pane = RECOVERY_PANE.get("pane")
    if not pane:
        return {"error": "no pane"}
    sf = f"{ROOT}/sf"; port = 18000 + int(RUN[1:5], 16) % 1000
    hook = lambda *a: {"type": "command", "command": HELPER, "args": ["claude-code", *a], "timeout": 10}
    tee = {"type": "command", "command": "/bin/sh", "args": ["-c", f"cat > {sf}/out/stopfailure.json"], "timeout": 10}
    settings = {"hooks": {"UserPromptSubmit": [{"hooks": [hook("working")]}],
                          "StopFailure": [{"hooks": [hook("turn-failed"), tee]}],
                          "Stop": [{"hooks": [hook("turn-done")]}]}}
    remote(f"mkdir -p -m 700 {sf}/cfg {sf}/home {sf}/proj {sf}/out && printf %s {shlex.quote(SF_SERVER)} > {sf}/server.py && "
           f"printf %s {shlex.quote(json.dumps(settings))} > {sf}/cfg/settings.json")
    spid = remote(f"nohup python3 {sf}/server.py {port} {sf}/requests.log >/dev/null 2>&1 & echo $!").strip()
    facts["serverPid"] = spid
    facts["registerServer"] = registry("add-process", f"--pid {spid}")
    time.sleep(2)
    env = f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)}"
    line = (f"cd {sf}/proj && HOME={sf}/home CLAUDE_CONFIG_DIR={sf}/cfg ANTHROPIC_API_KEY=sk-ant-deck-probe-fake "
            f"ANTHROPIC_BASE_URL=http://127.0.0.1:{port} CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 DISABLE_AUTOUPDATER=1 "
            f"claude -p 'say hi'; echo SF-EXIT=$?")
    lines_before = int(remote(f"wc -l < {DATA}/app.log", check=False).strip() or 0)
    remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} -l {shlex.quote(line)} && "
           f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} Enter")
    end = time.time() + 150
    while time.time() < end:
        if "SF-EXIT=" in remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} capture-pane -p -J -t {pane} -S -20 2>/dev/null", check=False):
            break
        time.sleep(3)
    facts["pane"] = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} capture-pane -p -J -t {pane} -S -12 2>/dev/null | grep -v '^[[:space:]]*$' | tail -6", check=False)[-1500:]
    payload = remote(f"cat {sf}/out/stopfailure.json 2>/dev/null", check=False)
    try:
        p = json.loads(payload)
        facts["payload"] = {k: (v if k in ("hook_event_name", "error") else (type(v).__name__ if v is not None else None)) for k, v in p.items()}
    except ValueError:
        facts["payload"] = None
    facts["requests"] = remote(f"wc -l < {sf}/requests.log 2>/dev/null", check=False).strip()
    facts["agentStatus"] = remote(f"tail -n +{lines_before + 1} {DATA}/app.log | grep -a 'agent-status' | cut -c1-160 | tail -8", check=False).strip().splitlines()
    facts["stopServer"] = registry("stop", f"--pid {spid}")
    return facts


def close_operator(t0):
    """After `close-armed`: the worker's project MCP server (this instance's
    adapter) is written, and the client is typed into the page's one card."""
    facts = {}
    if not wait_checkpoint("close-armed", 420):
        facts["armed"] = False
        return facts
    client, cred, sock = client_args()
    mcp = {"mcpServers": {"deck": {"type": "stdio", "command": "/bin/sh", "args": [
        "-c", 'exec "$0" --client-id "$1" --socket "$2" --credential-fd 3 3<"$3"', APP_MCP, client, sock, cred]}}}
    remote(f"printf %s {shlex.quote(json.dumps(mcp))} > {FIXTURE}/work/.mcp.json && chmod 600 {FIXTURE}/work/.mcp.json")
    env = f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)}"
    panes = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} list-panes -a -F '#{{session_name}} #{{pane_id}} #{{pane_current_command}}'",
                   check=False).strip().splitlines()
    facts["panesAtArmed"] = panes
    if len(panes) != 1:
        facts["error"] = "not exactly one pane at armed"
        return facts
    pane = panes[0].split()[1]
    evidence = f"{DATA}/evidence/close"
    line = (f"python3 {ROOT}/scripts/mcp-close-e2e.py --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
            f"--client-id {client} --credential-file {shlex.quote(cred)} --evidence {evidence} --tag {RUN}")
    remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} -l {shlex.quote(line)} && "
           f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} Enter")
    facts["clientTypedAt"] = round(time.time() - t0, 1)
    end = time.time() + 1500
    while time.time() < end:
        # written after every step: done once it no longer says partial
        if remote(f"test -f {evidence}/close-e2e-{RUN}.json && ! grep -q '\"partial\": true' {evidence}/close-e2e-{RUN}.json && echo y",
                  check=False).strip() == "y":
            break
        time.sleep(5)
    facts["evidence"] = remote(f"cat {evidence}/close-e2e-{RUN}.json 2>/dev/null", check=False)[-12000:]
    facts["orchPane"] = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} capture-pane -p -J -t {pane} -S -80 2>/dev/null | grep -v '^[[:space:]]*$' | tail -60",
                               check=False)[-8000:]
    facts["ledger"] = remote(f"python3 -c \"import json;d=json.load(open('{DATA}/mcp.json'));print(d.get('version'), [(w['cardId'], w.get('reported'), w.get('acknowledged')) for w in d.get('workers', [])], [(o['kind'], o['state'], o.get('code')) for o in d.get('operations', []) if o['kind'] == 'worker-close'])\"", check=False).strip()
    return facts


def prepare_worktree_fixture():
    """`worktree-switch`: copy_agent.py, two repositories with one commit
    each (git here is the runner's own tool; Deck runs none) and a plain
    folder. The worktrees are made only in phase 2, after the switch."""
    git = "git -c user.name=deck-test -c user.email=deck-test@invalid -c init.defaultBranch=main -c commit.gpgsign=false"
    remote(f"mkdir -p {FIXTURE}/repo {FIXTURE}/repo2 {FIXTURE}/plain && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && "
           f"for r in repo repo2; do cd {FIXTURE}/$r && {git} init -q && mkdir -p src && echo x > src/f && {git} add . && {git} commit -qm init; done")
    remote(f"mkdir -p {ROOT}/scripts")
    for name in ("mcp-worktree-e2e.py", "mcp-agent-e2e.py"):
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(WORKTREE / "scripts" / name), f"{HOST}:{ROOT}/scripts/{name}"], check=True)


def worktree_operator(t0):
    """Phase 2, after `wt-armed`: the worktrees (and a clone) made now, in
    folders of their own, then the client typed into "Orch W"."""
    facts = {}
    if not wait_checkpoint("wt-armed", 420):
        facts["armed"] = False
        return facts
    git = "git -c user.name=deck-test -c user.email=deck-test@invalid"
    facts["git"] = remote(f"mkdir -p {FIXTURE}/disk2 {FIXTURE}/disk3 && cd {FIXTURE}/repo && {git} worktree add -q {FIXTURE}/disk2/wt1 -b wt1 && "
                          f"cd {FIXTURE}/repo2 && {git} worktree add -q {FIXTURE}/disk3/wt2 -b wt2 && "
                          f"git clone -q {FIXTURE}/repo {FIXTURE}/disk2/clone && echo made", check=False).strip()
    client, cred, sock = client_args()
    env = f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)}"
    panes = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} list-panes -a -F '#{{session_name}} #{{pane_id}} #{{pane_current_command}}'",
                   check=False).strip().splitlines()
    facts["panesAtArmed"] = panes
    if len(panes) != 1:
        facts["error"] = "not exactly one pane at armed"
        return facts
    pane = panes[0].split()[1]
    evidence = f"{DATA}/evidence/worktree"
    line = (f"python3 {ROOT}/scripts/mcp-worktree-e2e.py --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
            f"--client-id {client} --credential-file {shlex.quote(cred)} --evidence {evidence} --tag {RUN} --fixture {FIXTURE}")
    remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} -l {shlex.quote(line)} && "
           f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} Enter")
    end = time.time() + 600
    while time.time() < end:
        if remote(f"test -f {evidence}/worktree-e2e-{RUN}.json && ! grep -q '\"partial\": true' {evidence}/worktree-e2e-{RUN}.json && echo y",
                  check=False).strip() == "y":
            break
        time.sleep(3)
    facts["evidence"] = remote(f"cat {evidence}/worktree-e2e-{RUN}.json 2>/dev/null", check=False)[-12000:]
    facts["ledger"] = remote(f"python3 -c \"import json;d=json.load(open('{DATA}/mcp.json'));print(d.get('version'), [(c['name'], c.get('autoWorktrees')) for c in d['config']['clients']], [(o['kind'], o['state'], o.get('code'), (o.get('result') or {{}}).get('cwdGrant')) for o in d.get('operations', []) if o['kind'] == 'worker-start'])\"", check=False).strip()
    return facts


def prepare_interject_fixture():
    """`worker-interject`: copy_agent.py, work/, and the worker's Claude Code
    settings: the Signal hooks of Deck's current Claude table calling this
    carrier's helper (PermissionRequest included, the Stop hook forwarding the
    final message) and two harmless shell commands allowed, nothing else."""
    hook = lambda *state: {"type": "command", "command": HELPER, "args": ["claude-code", *state], "timeout": 10}
    settings = {"hooks": {
        "UserPromptSubmit": [{"hooks": [hook("working")]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [hook("needs-input")]}],
        "PermissionRequest": [{"hooks": [hook("needs-input")]}],
        "PostToolUse": [{"hooks": [hook("working", "--after-tool")]}],
        "Stop": [{"hooks": [hook("turn-done", "--reply", "workers")]}],
    }, "permissions": {"allow": ["Bash(python3 phase1.py:*)"]}}
    remote(f"mkdir -p {FIXTURE}/work {ROOT}/outside && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && "
           f"printf %s {shlex.quote(json.dumps(settings))} > {FIXTURE}/worker-hooks.json && "
           f"printf %s {shlex.quote(PHASE1)} > {FIXTURE}/work/phase1.py"
           + (f" && printf codex > {FIXTURE}/scenario" if OPTS.codex else ""))
    if OPTS.codex:
        prepare_interject_codex()
    remote(f"mkdir -p {ROOT}/scripts")
    for name in ("mcp-interject-e2e.py", "mcp-agent-e2e.py"):
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(WORKTREE / "scripts" / name), f"{HOST}:{ROOT}/scripts/{name}"], check=True)


# the worker's step 2: a 100 s foreground program (Claude Code refuses a
# standalone `sleep`); its two marker files say which phase a line landed in
PHASE1 = """import time
open("p1-started", "w").close()
for i in range(10):
    print("phase1", i * 10, "s", flush=True)
    time.sleep(10)
open("p1-done", "w").close()
print("phase1 done", flush=True)
"""

CODEX_HOME = f"{ROOT}/codex-home"


def prepare_interject_codex():
    """A Codex home of the run's own for the Codex worker (the carrier hands
    CODEX_HOME to its panes): a copy of the host's sign-in (removed after the
    run), Deck's four Signal hooks calling this carrier's helper and nothing
    else (no MCP server, plugin or notifier of the user's), the update check
    off (0.169.0 opens on an update prompt whose default runs npm), the
    ordinary on-request approvals in the workspace-write sandbox, with /tmp
    and TMPDIR not writable (a write outside the workspace needs approval),
    the user's model and its availability notice already seen. The host's
    ~/.codex is only read."""
    cmd = lambda state: {"type": "command", "command": f'"{HELPER}" codex {state}', "timeout": 10, "async": True}
    hooks = {"hooks": {
        "UserPromptSubmit": [{"hooks": [cmd("working")]}],
        "PermissionRequest": [{"hooks": [cmd("needs-input")]}],
        "Stop": [{"hooks": [cmd("turn-done --reply workers")]}],
        "Interrupt": [{"hooks": [cmd("turn-done")]}],
    }}
    config = ('model = "gpt-6.1-sol"\nmodel_reasoning_effort = "low"\ncheck_for_update_on_startup = false\n'
              'approval_policy = "on-request"\nsandbox_mode = "workspace-write"\n\n'
              '[sandbox_workspace_write]\nexclude_slash_tmp = true\nexclude_tmpdir_env_var = true\n\n'
              '[notice]\nhide_rate_limit_model_nudge = true\n\n[tui]\nscreen_reader_detection_done = true\n\n'
              '[tui.model_availability_nux]\n"gpt-6.1-sol" = 1\n\n'
              f'[projects."{FIXTURE}/work"]\ntrust_level = "trusted"\n')
    remote(f"mkdir -m 700 -p {CODEX_HOME} && install -m 600 ~/.codex/auth.json {CODEX_HOME}/auth.json && "
           f"printf %s {shlex.quote(config)} > {CODEX_HOME}/config.toml && "
           f"printf %s {shlex.quote(json.dumps(hooks))} > {CODEX_HOME}/hooks.json")



QUOTA_HOME = f"{ROOT}/home"
USER_LINE = "#!/bin/sh\ncat > /dev/null\necho QUOTA-USER-LINE\n"


def prepare_quota_live():
    """`quota-live`: the work dir, the Claude settings (Signal hooks and
    Deck's statusLine tee into the fixture's own user command), claude.sh
    and codex.sh, and a Codex home of the run's own (prepare_interject_codex)."""
    hook = lambda *state: {"type": "command", "command": HELPER, "args": ["claude-code", *state], "timeout": 10}
    settings = {"hooks": {
        "UserPromptSubmit": [{"hooks": [hook("working")]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [hook("needs-input")]}],
        "Stop": [{"hooks": [hook("turn-done")]}],
    }, "statusLine": {"type": "command", "command": f"'{HELPER}' --quota claude-code | /bin/sh '{FIXTURE}/user-line.sh'"}}
    claude = (f"#!/bin/sh\nexec claude --settings '{FIXTURE}/quota-settings.json' "
              "'Reply with exactly QUOTA-LIVE-OK and nothing else. Do not use any tool.'\n")
    codex = (f"#!/bin/sh\nexport PATH={CODEX_BIN_DIR}:$PATH CODEX_HOME={CODEX_HOME}\n"
             "exec codex --no-daemon --dangerously-bypass-hook-trust 'Do not use any tool. Reply with exactly this line and nothing else: QUOTA-CODEX-OK'\n")
    remote(f"mkdir -p {FIXTURE}/work && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && printf codex > {FIXTURE}/scenario && "
           f"printf %s {shlex.quote(json.dumps(settings))} > {FIXTURE}/quota-settings.json && "
           f"printf %s {shlex.quote(USER_LINE)} > {FIXTURE}/user-line.sh && printf %s {shlex.quote(claude)} > {FIXTURE}/claude.sh && "
           f"printf %s {shlex.quote(codex)} > {FIXTURE}/codex.sh && chmod 700 {FIXTURE}/user-line.sh {FIXTURE}/claude.sh {FIXTURE}/codex.sh")
    prepare_interject_codex()


SWITCH_CLAUDE = '{\n  "model": "opus",\n  "statusLine": {\n    "type": "command",\n    "command": "~/.claude/line.sh \\"$1\\"",\n    "padding": 1\n  },\n  "hooks": {}\n}\n'
SWITCH_AGY = '{\n  "trustedWorkspaces": [\n    "/Users/u/project"\n  ],\n  "model": "gemini-3"\n}\n'


def prepare_quota_switch():
    """`quota-switch`: a home of the run's own with the "user's" Claude and
    agy settings files."""
    remote(f"mkdir -m 700 -p {QUOTA_HOME}/.claude {QUOTA_HOME}/.gemini/antigravity-cli {QUOTA_HOME}/Applications && "
           f"printf %s {shlex.quote(SWITCH_CLAUDE)} > {QUOTA_HOME}/.claude/settings.json && "
           f"printf %s {shlex.quote(SWITCH_AGY)} > {QUOTA_HOME}/.gemini/antigravity-cli/settings.json")


def switch_operator(t0):
    """Copy both files after each checkpoint (the page holds 6 s)."""
    copies = {}
    for name in ("quota-sw-clean", "quota-sw-decline", "quota-sw-on", "quota-sw-off"):
        if not wait_checkpoint(name, 240):
            copies[name] = None
            continue
        copies[name] = {
            "claude": remote(f"cat {QUOTA_HOME}/.claude/settings.json", check=False),
            "agy": remote(f"cat {QUOTA_HOME}/.gemini/antigravity-cli/settings.json", check=False),
        }
    judged = {}
    helper = f"{APP}/Contents/MacOS/deck-status-helper"
    for name in ("quota-sw-clean", "quota-sw-decline"):
        c = copies.get(name)
        judged[name] = bool(c) and c["claude"] == SWITCH_CLAUDE and c["agy"] == SWITCH_AGY
    c = copies.get("quota-sw-on")
    if c:
        cl, ag = json.loads(c["claude"]), json.loads(c["agy"])
        judged["quota-sw-on"] = (
            cl["statusLine"]["command"] == f"'{helper}' --quota claude-code | ~/.claude/line.sh \"$1\""
            and cl["statusLine"]["padding"] == 1 and cl["model"] == "opus" and cl["hooks"] == {}
            and c["claude"].startswith('{\n  "model": "opus",\n  "statusLine": {\n    "type": "command",\n    "command": ')
            and ag["statusLine"] == {"type": "command", "command": f"'{helper}' --quota agy --quiet", "stack_with_default": True}
            and ag["trustedWorkspaces"] == ["/Users/u/project"] and c["agy"].startswith(SWITCH_AGY.rstrip("\n").rstrip("}").rstrip()))
    else:
        judged["quota-sw-on"] = False
    c = copies.get("quota-sw-off")
    judged["quota-sw-off"] = bool(c) and c["claude"] == SWITCH_CLAUDE and c["agy"] == SWITCH_AGY
    return {"copies": copies, "judged": judged}


def live_operator(t0):
    """After `quota-armed`: the panes, for the user's statusLine output."""
    facts = {}
    if not wait_checkpoint("quota-armed", TIMEOUT):
        facts["armed"] = False
    env = f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)}"
    panes = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} list-panes -a -F '#{{session_name}} #{{pane_id}} #{{pane_current_command}}'",
                   check=False).strip().splitlines()
    facts["panes"] = panes
    facts["userLine"] = {}
    for line in panes:
        parts = line.split()
        if len(parts) < 2:
            continue
        text = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} capture-pane -p -J -t {parts[1]} 2>/dev/null", check=False)
        facts["userLine"][parts[0]] = {"QUOTA-USER-LINE": "QUOTA-USER-LINE" in text,
                                       "reply": ("QUOTA-LIVE-OK" in text) or ("QUOTA-CODEX-OK" in text),
                                       "tail": [l for l in text.splitlines() if l.strip()][-6:]}
    facts["rollouts"] = remote(f"ls -R {CODEX_HOME}/sessions 2>/dev/null | grep -c rollout; true", check=False).strip()
    return facts


def interject_operator(t0):
    """After `interject-armed`: the one session of the instance is the page's
    "Orch I" card; the client is typed into its pane (its shell runs it, so
    Deck proves the card from the kernel), and its evidence is awaited."""
    facts = {}
    if not wait_checkpoint("interject-armed", 420):
        facts["armed"] = False
        return facts
    client, cred, sock = client_args()
    env = f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)}"
    panes = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} list-panes -a -F '#{{session_name}} #{{pane_id}} #{{pane_current_command}}'",
                   check=False).strip().splitlines()
    facts["panesAtArmed"] = panes
    if len(panes) != 1:
        facts["error"] = "not exactly one pane at armed"
        return facts
    pane = panes[0].split()[1]
    evidence = f"{DATA}/evidence/interject"
    line = (f"python3 {ROOT}/scripts/mcp-interject-e2e.py --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
            f"--client-id {client} --credential-file {shlex.quote(cred)} --evidence {evidence} --tag {RUN}"
            + (f" --type codex --outside {ROOT}/outside" if OPTS.codex else ""))
    remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} -l {shlex.quote(line)} && "
           f"{env} {shlex.quote(TMUX)} -L {SOCKET} send-keys -t {pane} Enter")
    facts["clientTypedAt"] = round(time.time() - t0, 1)
    end = time.time() + 1500
    while time.time() < end:
        if remote(f"test -f {evidence}/interject-e2e-{RUN}.json && echo y", check=False).strip() == "y":
            break
        time.sleep(5)
    facts["evidence"] = remote(f"cat {evidence}/interject-e2e-{RUN}.json 2>/dev/null", check=False)[-12000:]
    facts["orchPane"] = remote(f"{env} {shlex.quote(TMUX)} -L {SOCKET} capture-pane -p -J -t {pane} -S -80 2>/dev/null | grep -v '^[[:space:]]*$' | tail -60",
                               check=False)[-8000:]
    return facts


def client_args():
    doc = json.loads(remote(f"cat {DATA}/mcp.json"))
    client = next(c["id"] for c in doc["config"]["clients"] if c["name"] == "orchestrator")
    return client, f"{DATA}/evidence/mcp-credential-{client}", f"{DATA}/mcp-control.sock"


def account_operator(t0):
    facts = {}
    if not wait_checkpoint("workers-armed", 420):
        facts["armed"] = False
        return facts
    client, cred, sock = client_args()
    cmd = (f"python3 {ROOT}/scripts/mcp-worker-e2e.py --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
           f"--client-id {client} --credential-file {shlex.quote(cred)} --automation worker "
           f"--evidence {DATA}/evidence/worker-e2e --start-type claude")
    a = subprocess.run(SSH + [cmd], capture_output=True, text=True, timeout=1500)
    facts["partA"] = {"exit": a.returncode, "out": a.stdout.strip().splitlines()[-60:], "err": a.stderr.strip()[-600:]}
    return facts


def capacity_operator(t0):
    facts = {}
    if not wait_checkpoint("workers-armed", 420):
        facts["armed"] = False
        return facts
    client, cred, sock = client_args()
    remote(f"mkdir -p {DATA}/evidence")
    cmd = (f"python3 {ROOT}/scripts/mcp-capacity-e2e.py --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
           f"--client-id {client} --credential-file {shlex.quote(cred)} --evidence {DATA}/evidence/capacity-e2e.json")
    a = subprocess.run(SSH + [cmd], capture_output=True, text=True, timeout=900)
    facts["client"] = {"exit": a.returncode, "out": a.stdout.strip().splitlines()[-40:], "err": a.stderr.strip()[-600:]}
    facts["memory"] = remote("vm_stat | head -6; pgrep -f '[c]laude --setting-sources project' | wc -l", check=False).strip()
    return facts


def report_operator(t0):
    facts = {}
    if not wait_checkpoint("workers-armed", 420):
        facts["armed"] = False
        return facts
    doc = json.loads(remote(f"cat {DATA}/mcp.json"))
    client = next(c["id"] for c in doc["config"]["clients"] if c["name"] == "orchestrator")
    cred = f"{DATA}/evidence/mcp-credential-{client}"
    sock = f"{DATA}/mcp-control.sock"
    mcp = {"mcpServers": {"deck": {"type": "stdio", "command": "/bin/sh", "args": [
        "-c", 'exec "$0" --client-id "$1" --socket "$2" --credential-fd 3 3<"$3"', APP_MCP, client, sock, cred]}}}
    remote(f"printf %s {shlex.quote(json.dumps(mcp))} > {FIXTURE}/work/.mcp.json && chmod 600 {FIXTURE}/work/.mcp.json")
    cmd = (f"python3 {ROOT}/scripts/mcp-worker-e2e.py --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
           f"--client-id {client} --credential-file {shlex.quote(cred)} --automation worker "
           f"--evidence {DATA}/evidence/worker-e2e --start-type claude --report")
    a = subprocess.run(SSH + [cmd], capture_output=True, text=True, timeout=1200)
    facts["partA"] = {"exit": a.returncode, "out": a.stdout.strip().splitlines()[-60:], "err": a.stderr.strip()[-600:]}
    env = f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)}"
    facts["panes"] = remote(f"for p in $({env} {shlex.quote(TMUX)} -L {SOCKET} list-panes -a -F '#{{pane_id}}' 2>/dev/null); do echo \"== $p\"; "
                            f"{env} {shlex.quote(TMUX)} -L {SOCKET} capture-pane -p -J -t $p -S -80 | grep -v '^[[:space:]]*$' | tail -40; done", check=False)[-6000:]
    facts["ledgerReports"] = remote(f"python3 -c \"import json;d=json.load(open('{DATA}/mcp.json'));print(d.get('version'), [(r['cardId'], r['sequence'], len(r['text'])) for r in d.get('reports', [])])\"", check=False).strip()
    return facts


def prepare_cards_fixture():
    """The `workers` live fixture: copy_agent.py (the page finds the fixture
    through it), work/, and Claude Code settings whose Signal hooks call this
    carrier's status helper (the Stop hook forwards the final message)."""
    hook = lambda *state: {"type": "command", "command": HELPER, "args": ["claude-code", *state], "timeout": 10}
    hooks = {"hooks": {
        "UserPromptSubmit": [{"hooks": [hook("working")]}],
        "Notification": [{"matcher": "permission_prompt", "hooks": [hook("needs-input")]}],
        "Stop": [{"hooks": [hook("turn-done", "--reply", "workers")]}],
    }}
    remote(f"mkdir -p {FIXTURE}/work && : > {FIXTURE}/copy_agent.py && chmod 700 {FIXTURE} && "
           f"printf %s {shlex.quote(json.dumps(hooks))} > {FIXTURE}/worker-hooks.json")
    remote(f"mkdir -p {ROOT}/scripts {ROOT}/agent && printf %s {shlex.quote(agent_script())} > {FIXTURE}/agent.sh")
    if OPTS.codex:
        prepare_codex()
    for name in ("mcp-cards-e2e.py", "mcp-agent-e2e.py"):
        subprocess.run(["scp", "-q", "-o", "BatchMode=yes", str(WORKTREE / "scripts" / name), f"{HOST}:{ROOT}/scripts/{name}"], check=True)


AGENT_PROMPT = ("Go over my Deck cards with the deck MCP tools. For each card whose turn ended, mark it: "
                "needs-you if it waits for me to answer a question or decide something, can-close if its work is "
                "finished and nothing waits. Give each mark a short reason. Do not do anything else.")


def agent_script():
    """What the page's "Orchestrator" card runs (part B): a real Claude Code
    with only this instance's adapter as its MCP server, inside the GUI
    session (its sign-in lives in the login keychain)."""
    tools = ",".join(f"mcp__deck__{t}" for t in ("deck_capabilities", "deck_cards_snapshot", "deck_card_reply", "deck_card_mark"))
    make_config = (
        "import json,sys\n"
        "d,a,out=sys.argv[1:4]\n"
        "cid=next(c['id'] for c in json.load(open(d+'/mcp.json'))['config']['clients'] if c['name']=='orchestrator')\n"
        "cfg={'mcpServers':{'deck':{'type':'stdio','command':'/bin/sh','args':['-c','exec \"$0\" --client-id \"$1\" --socket \"$2\" --credential-fd 3 3<\"$3\"',a,cid,d+'/mcp-control.sock',d+'/evidence/mcp-credential-'+cid]}}}\n"
        "json.dump(cfg,open(out,'w'))\n")
    return (f"#!/bin/sh\ncd {ROOT}/agent\n"
            f"python3 -c {shlex.quote(make_config)} {shlex.quote(DATA)} {shlex.quote(APP_MCP)} {ROOT}/agent/mcp.json\n"
            f"claude -p {shlex.quote(AGENT_PROMPT)} --mcp-config {ROOT}/agent/mcp.json --strict-mcp-config "
            f"--setting-sources project --allowedTools {shlex.quote(tools)} --output-format json "
            f"> {ROOT}/agent/out.json 2> {ROOT}/agent/err.txt\necho AGENT-EXIT $?\n")


def cards_operator(t0):
    """Part A (the scripted client) once the page is armed; part B runs in
    the page's own "Orchestrator" card (agent_script)."""
    facts = {}
    if not wait_checkpoint("cards-armed", 420):
        facts["armed"] = False
        return facts
    doc = json.loads(remote(f"cat {DATA}/mcp.json"))
    client = next(c["id"] for c in doc["config"]["clients"] if c["name"] == "orchestrator")
    cred = f"{DATA}/evidence/mcp-credential-{client}"
    sock = f"{DATA}/mcp-control.sock"
    cmd = (f"python3 {ROOT}/scripts/mcp-cards-e2e.py --adapter {shlex.quote(APP_MCP)} --socket {shlex.quote(sock)} "
           f"--client-id {client} --credential-file {shlex.quote(cred)} --evidence {DATA}/evidence/cards-e2e"
           + (" --codex" if OPTS.codex else ""))
    a = subprocess.run(SSH + [cmd], capture_output=True, text=True, timeout=900)
    facts["partA"] = {"exit": a.returncode, "out": a.stdout.strip().splitlines()[-40:], "err": a.stderr.strip()[-400:]}
    return facts


def agent_outcome():
    out = remote(f"cat {ROOT}/agent/out.json 2>/dev/null; echo; echo ---; tail -c 400 {ROOT}/agent/err.txt 2>/dev/null", check=False)
    body, _, err = out.partition("\n---\n")
    facts = {"err": err.strip()}
    try:
        parsed = json.loads(body.strip())
        facts.update({"result": str(parsed.get("result", ""))[:1500], "turns": parsed.get("num_turns"),
                      "isError": parsed.get("is_error")})
    except ValueError:
        facts["raw"] = body.strip()[-600:]
    return facts


def count_done():
    out = remote(f"c=$(grep -c 'smoke-check {TERMINAL} ' {DATA}/app.log 2>/dev/null); echo ${{c:-0}}", check=False).strip()
    return int(out or 0)


def main():
    result = {"run": RUN, "root": ROOT, "socket": SOCKET_PATH, "mode": MODE,
              "head": subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=WORKTREE, text=True).strip(),
              "dirty": subprocess.check_output(["git", "status", "--porcelain"], cwd=WORKTREE, text=True).strip().splitlines(),
              "binMtime": time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(BIN.stat().st_mtime))}
    result["machineBefore"] = machine_state()
    pre = remote("caffeinate -u -t 6 >/dev/null 2>&1 & sleep 4; ioreg -c IOHIDSystem | awk '/HIDIdleTime/ {print int($NF/1000000000); exit}'; "
                 "ioreg -n Root -d1 | grep -E 'CGSSessionScreenIsLocked|IOConsoleLocked' | tr -d '\\n'; echo").split("\n")
    result["idleSeconds"], result["lockMarkers"], result["frontBefore"] = pre[0].strip(), pre[1].strip(), front()
    if "IOConsoleLocked\" = Yes" in result["lockMarkers"]:
        print(json.dumps(result, indent=2, ensure_ascii=False)); sys.exit("BLOCKED: the test machine is really locked")
    if MODE in ("cards-mcp", "workers", "workers-capacity", "reentry-shapes", "worker-interject", "quota-live", "worker-close", "worker-recovery"):
        # SSH cannot read the login keychain: the account record and the
        # keychain item say the GUI session's Claude Code is signed in
        auth = remote("python3 -c \"import json,os;print(bool(json.load(open(os.path.expanduser('~/.claude.json'))).get('oauthAccount')))\"; "
                      "security find-generic-password -s 'Claude Code-credentials' >/dev/null 2>&1 && echo item", check=False)
        result["claudeAuth"] = "True" in auth and "item" in auth
        if not result["claudeAuth"]:
            print(json.dumps(result, indent=2, ensure_ascii=False)); sys.exit("BLOCKED: Claude Code is not signed in on the host")
    OUT.mkdir(exist_ok=True)
    if OPTS.phase in ("all", "prepare"):
        remote(f"mkdir -p {DATA} {TMUX_TMPDIR} && chmod 700 {ROOT} {ROOT}/run {DATA} {TMUX_TMPDIR}")
        if MODE in ("workers-account", "workers-capacity", "reentry-shapes"):
            prepare_account_fixture()
        if MODE in ("workers-capacity", "reentry-shapes"):
            result["trustAdded"] = trust("add")
    if OPTS.phase == "prepare":
        print(json.dumps({"run": RUN, "root": ROOT, "account": ACCOUNT, "work": FIXTURE + "/work"}, indent=2))
        return
    if MODE == "workers-account":
        # the person signed the folder in: its own config file names an account
        signed = remote(f"python3 -c \"import json;print(bool(json.load(open('{ACCOUNT}/.claude.json')).get('oauthAccount')))\"", check=False)
        result["accountSignedIn"] = "True" in signed
        if not result["accountSignedIn"]:
            print(json.dumps(result, indent=2, ensure_ascii=False)); sys.exit("BLOCKED: the account folder is not signed in")
    work = Path(tempfile.mkdtemp(prefix="deck-mini-carrier-"))
    result["version"] = build_carrier(work)
    if MODE == "workers":
        prepare_report_fixture()
        result["trustAdded"] = trust("add")
    if MODE == "cards-mcp":
        prepare_cards_fixture()
        result["trustAdded"] = trust("add")
    if MODE == "worker-interject":
        prepare_interject_fixture()
        result["trustAdded"] = trust("add")
    if MODE == "worker-close":
        prepare_close_fixture()
        result["trustAdded"] = trust("add")
    if MODE == "worker-recovery":
        prepare_recovery_fixture()
        result["trustAdded"] = trust("add")
    if MODE == "worktree-switch":
        prepare_worktree_fixture()
    if MODE == "quota-live":
        prepare_quota_live()
        result["trustAdded"] = trust("add")
    if MODE == "quota-switch":
        prepare_quota_switch()

    tar = subprocess.Popen(["tar", "-C", str(work), "-cf", "-", "deck-smoke.app"], stdout=subprocess.PIPE)
    subprocess.run(SSH + [f"tar -C {ROOT} -xf -"], stdin=tar.stdout, check=True); tar.wait()
    if MODE == "quota-switch":
        remote(f"mv {ROOT}/deck-smoke.app {shlex.quote(APP)}")
    remote(f"codesign -v {shlex.quote(APP)}")
    result["registryInit"] = registry("init")
    # the display stays awake for the run; this is the run's own process, stopped by its pid
    awake = remote(f"nohup caffeinate -d -u -t {5400 if MODE == 'worker-recovery' else 2400} >/dev/null 2>&1 & echo $!").strip()
    result["caffeinatePid"] = awake
    args = " ".join(shlex.quote(a) for a in ["--smoke-data-dir", DATA, "--smoke-tmux-socket", SOCKET, "--smoke-wkwebview", MODE])
    codex_env = f" --env CODEX_HOME={shlex.quote(CODEX_HOME)}" if (MODE == "worker-interject" and OPTS.codex) or MODE == "quota-live" else ""
    if MODE == "quota-switch":
        remote(f"env -i /usr/bin/open -n --env HOME={QUOTA_HOME} --env TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)} --env DISABLE_AUTO_UPDATE=true "
               f"{shlex.quote(APP)} --args {args}")
    else:
        remote(f"open -n {shlex.quote(APP)} --env TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)} --env DISABLE_AUTO_UPDATE=true{codex_env} --args {args}")
    isolated = False
    for _ in range(20):
        time.sleep(1)
        if remote(f"test -f {DATA}/app.log && echo y", check=False).strip() == "y": isolated = True; break
    pid = remote(f"pgrep -f {shlex.quote('^' + EXE)} | head -1", check=False).strip()
    result["appPid"] = pid
    if pid:
        result["registerApp"] = registry("add-process", f"--pid {pid}")
    if not isolated:
        result["verdict"] = "NOT-ISOLATED (stopped through the registry)"
        result["stop"] = registry("stop", f"--pid {pid}") if pid else "no pid"
        print(json.dumps(result, indent=2, ensure_ascii=False)); sys.exit(3)
    # the smoke page must be visible: activate the running carrier by its exact path if needed
    time.sleep(3)
    f = front(); result["frontAfterLaunch"] = f
    if f != "deck smoke":
        remote(f"open {shlex.quote(APP)}", check=False); time.sleep(2); result["frontAfterActivate"] = front()
    # the server appears with the first session; register it as soon as it listens
    for _ in range(60):
        if remote(f"test -S {shlex.quote(SOCKET_PATH)} && echo y", check=False).strip() == "y":
            result["registerServer"] = registry("add-server", f"--socket {shlex.quote(SOCKET_PATH)} --tmux {shlex.quote(TMUX)}"); break
        time.sleep(1)
    t0 = time.time(); finished = False
    if MODE == "workers":
        result["operator"] = report_operator(t0)
    if MODE == "workers-account":
        result["operator"] = account_operator(t0)
    if MODE == "workers-capacity":
        result["operator"] = capacity_operator(t0)
    if MODE == "cards-mcp":
        result["operator"] = cards_operator(t0)
    if MODE == "worker-interject":
        result["operator"] = interject_operator(t0)
    if MODE == "worker-close":
        result["operator"] = close_operator(t0)
    if MODE == "worker-recovery":
        result["operator"] = recovery_operator(t0, 1)
    if MODE == "quota-live":
        result["operator"] = live_operator(t0)
    if MODE == "quota-switch":
        result["operator"] = switch_operator(t0)
    while time.time() - t0 < TIMEOUT:
        time.sleep(3)
        if count_done() > 0: finished = True; break
    result["seconds"], result["finished"] = round(time.time() - t0), finished
    if MODE == "worker-recovery" and finished:
        # Deck's GUI stops (the tmux server and its agents go on), then the
        # same data again: the agent alive, Deck's observation of it lost
        result["killApp"] = registry("stop", f"--pid {pid}") if pid else "no pid"
        result["phase1Lines"] = int(remote(f"wc -l < {DATA}/app.log", check=False).strip() or 0)
        time.sleep(5)
        result["panesAfterStop"] = remote(f"TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)} {shlex.quote(TMUX)} -L {SOCKET} list-panes -a -F '#{{session_name}} #{{pane_current_command}}'", check=False).strip().splitlines()
        args2 = " ".join(shlex.quote(a) for a in ["--smoke-data-dir", DATA, "--smoke-tmux-socket", SOCKET, "--smoke-wkwebview", "worker-recovery-relaunch"])
        remote(f"open -n {shlex.quote(APP)} --env TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)} --env DISABLE_AUTO_UPDATE=true --args {args2}")
        time.sleep(5)
        pid2 = remote(f"pgrep -f {shlex.quote('^' + EXE)} | head -1", check=False).strip()
        result["appPid2"] = pid2
        if pid2:
            result["registerApp2"] = registry("add-process", f"--pid {pid2}")
        t1 = time.time()
        result["operator2"] = recovery_operator(t1, 2)
        result["stopfailure"] = stopfailure_operator()
        result["relaunchFinished"] = False
        while time.time() - t1 < 900:
            time.sleep(3)
            if count_done() > 1:
                result["relaunchFinished"] = True; break
    if MODE == "worktree-switch" and finished:
        # the carrier stopped (no quit path), then the same data again
        result["killApp"] = registry("stop", f"--pid {pid}") if pid else "no pid"
        result["phase1Lines"] = int(remote(f"wc -l < {DATA}/app.log", check=False).strip() or 0)
        time.sleep(3)
        args2 = " ".join(shlex.quote(a) for a in ["--smoke-data-dir", DATA, "--smoke-tmux-socket", SOCKET, "--smoke-wkwebview", "worktree-switch-relaunch"])
        remote(f"open -n {shlex.quote(APP)} --env TMUX_TMPDIR={shlex.quote(TMUX_TMPDIR)} --env DISABLE_AUTO_UPDATE=true --args {args2}")
        time.sleep(5)
        pid2 = remote(f"pgrep -f {shlex.quote('^' + EXE)} | head -1", check=False).strip()
        result["appPid2"] = pid2
        if pid2:
            result["registerApp2"] = registry("add-process", f"--pid {pid2}")
        for _ in range(60):
            if remote(f"test -S {shlex.quote(SOCKET_PATH)} && echo y", check=False).strip() == "y":
                result["registerServer2"] = registry("add-server", f"--socket {shlex.quote(SOCKET_PATH)} --tmux {shlex.quote(TMUX)}"); break
            time.sleep(1)
        t1 = time.time()
        result["operator"] = worktree_operator(t1)
        result["relaunchFinished"] = False
        while time.time() - t1 < 900:
            time.sleep(3)
            if count_done() > 1:
                result["relaunchFinished"] = True; break
    subprocess.run(["scp", "-q", "-o", "BatchMode=yes", f"{HOST}:{DATA}/app.log", str(OUT / "app.log")], check=False)
    subprocess.run(f"ssh -o BatchMode=yes {HOST} " + shlex.quote(f"cd {DATA} && [ -d evidence ] && tar -cf - evidence") + " | tar -C " + shlex.quote(str(OUT)) + " -xf - 2>/dev/null", shell=True, check=False)
    if result.get("phase1Lines"):
        lines = open(OUT / "app.log", errors="replace").read().splitlines(keepends=True)
        for name, part, mode in (("phase1", lines[:result["phase1Lines"]], MODE),
                                 ("phase2", lines[result["phase1Lines"]:], MODE + "-relaunch")):
            (OUT / name).mkdir(exist_ok=True)
            (OUT / name / "app.log").write_text("".join(part))
            pv = subprocess.run([str(WORKTREE / "scripts/smoke-verdict"), str(OUT / name), mode], capture_output=True, text=True)
            (OUT / name / "verdict.txt").write_text(pv.stdout + pv.stderr)
            result[f"verdict-{name}"] = ("PASS" if pv.returncode == 0 else f"FAIL (exit {pv.returncode})",
                                         (pv.stdout + pv.stderr).strip().splitlines()[-6:])
    v = subprocess.run([str(WORKTREE / "scripts/smoke-verdict"), str(OUT / "phase1") if result.get("phase1Lines") else str(OUT), MODE],
                       capture_output=True, text=True)
    (OUT / "verdict.txt").write_text(v.stdout + v.stderr)
    result["verdict"] = "PASS" if v.returncode == 0 else f"FAIL (exit {v.returncode})"
    result["verdictDetail"] = (v.stdout + v.stderr).strip().splitlines()[-8:]
    result["checksOfInterest"] = [l.split("[ui] ")[-1] for l in open(OUT / "app.log", errors="replace")
                                  if "smoke-check cards-" in l or "smoke-check workers-" in l or "smoke-check interject-" in l or "smoke-check close-" in l or "smoke-check recovery-" in l or "smoke-check wt-" in l or "smoke-check capacity-" in l or "smoke-check shape-" in l or "smoke-check quota-" in l or "smoke-check attention-card" in l or "smoke-check done " in l
                                  or "smoke-check page-visible " in l or "smoke-check input-isolation-proof " in l]
    if MODE == "cards-mcp":
        result["agent"] = agent_outcome()
    result["registryShow"] = registry("show")
    result["cleanup"] = registry("cleanup")
    time.sleep(2)
    result["leftover"] = remote(f"pgrep -lf {shlex.quote(ROOT)} | cut -c1-120; ls {shlex.quote(TMUX_TMPDIR)}/tmux-{UID}/ 2>/dev/null; true", check=False).strip()
    caff = remote(f"ps -o command= -p {shlex.quote(awake)} 2>/dev/null", check=False).strip()
    if caff.startswith("caffeinate"):
        remote(f"kill {shlex.quote(awake)}", check=False)
    result["caffeinateStopped"] = caff
    if OPTS.codex or MODE == "quota-live":
        result["codexAuthRemoved"] = remote(f"rm -f {ROOT}/codex-home/auth.json && test ! -e {ROOT}/codex-home/auth.json && echo removed", check=False).strip()
    if MODE == "workers-account":
        # the folder holds a sign-in: signed out first, the keychain item gone
        result["accountLogout"] = remote(f"cd {FIXTURE}/work && CLAUDE_CONFIG_DIR={ACCOUNT} claude auth logout 2>&1 | tail -2; "
                                         f"CLAUDE_CONFIG_DIR={ACCOUNT} claude auth status 2>&1 | grep -o '\"loggedIn\": [a-z]*'", check=False).strip()
    if not result["leftover"] and result["cleanup"][0] == 0:
        remote(f"mkdir -p ~/.Trash/deck-verify-{RUN} && mv {ROOT} ~/.Trash/deck-verify-{RUN}/", check=False)
        result["rootTrashed"] = True
    if MODE in ("cards-mcp", "workers", "workers-capacity", "reentry-shapes", "worker-interject", "quota-live", "worker-close", "worker-recovery"):
        # after every process stopped: a Claude Code still exiting writes its
        # copy of the config back
        result["trustRemoved"] = trust("remove")
    result["machineAfter"] = machine_state()
    (OUT / "result.json").write_text(json.dumps(result, indent=2, ensure_ascii=False))
    print(json.dumps(result, indent=2, ensure_ascii=False))


main()
