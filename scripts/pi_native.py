#!/usr/bin/env python3
"""The native Pi run: Deck's production Pi ingress against real interactive Pi.

Two real Pi processes, an npm install on Node and the official standalone
build, run in panes of an isolated tmux server. Deck's side is
`agent_pi::tests::native::tap` (app/src-tauri/src/agent_pi/tests.rs): the production
status listener, Pi transport, store, Board poll body and Settings commands,
started from a stand-in bundle under a relocated home. A deterministic local
provider (`scripts/pi/fixture.ts`) answers in place of a model; Pi itself
retries, queues, continues, settles, raises its dialogs and tears its runtime
down. Nothing here calls a Deck handler in Pi's place.

Everything lives in ONE experiment root under /private/tmp: home, Pi's agent
directory, sessions, working directory, tmux socket directory, Deck's data
directory. No setting or credential of the account is read. Every server and
process is registered with `edr_runtime.py experiment` when it starts and is
stopped only through it; the run ends with that tool's cleanup.

  pi_native.py --test-binary FILE --standalone DIR [--root DIR] [--only a,b]

The summary names the build it ran: the commit, whether the working tree
differed from it and where, one hash over the sources as they were on disk
(`scripts/pi_build_identity.py`), and the hash of the test executable.

`--test-binary` is the deck-app unit-test executable (`cargo test --no-run`),
`--standalone` a verified, unpacked official Pi build (the directory that
holds `pi`). The npm install and Node are found on this machine and only read.

`<root>/logs/` keeps the private trace: it holds native session ids and is
never committed. `<root>/result.json` is the summary: closed words, counts and
relations only. Exit 0 when every check passed and the cleanup found nothing.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO / "scripts"))
import pi_build_identity  # noqa: E402  (beside this file)

RUNTIME = REPO / "scripts/edr_runtime.py"
RESOURCES = REPO / "app/src-tauri/resources/pi"
FIXTURES = REPO / "scripts/pi"
TMUX = REPO / "app/src-tauri/binaries/tmux-aarch64-apple-darwin"
NPM_PACKAGE = Path.home() / ".local/lib/node_modules/@earendil-works/pi-coding-agent"
ENTRY = "Contents/Resources/pi/signal.ts"
OBSERVER_FILES = ("observer.ts", "signal.ts", "lifecycle.ts", "both.ts")

# What the account's own Pi settings could look like: other keys, the user's
# own entries, Pi's own two-space layout. Deck's entry is added by the real
# Settings command and by nothing in this file.
USER_SETTINGS = """{
  "defaultProjectTrust": "never",
  "quietStartup": true,
  "retry": {
    "enabled": true,
    "maxRetries": 2,
    "baseDelayMs": 50
  },
  "extensions": [
    "-builtin:mcp",
    "-builtin:llama.cpp",
    "-builtin:codemode",
    "-builtin:tool-search"
  ]
}
"""


class Failed(Exception):
    """A step did not happen in time: the run cannot go on."""


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def status(signal: bool, lifecycle: bool, installation: str) -> dict:
    """Pi's part of the Settings status reply."""
    return {"pi": signal, "pi_lifecycle": lifecycle, "pi_installation": installation}


def alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


class Tap:
    """One life of Deck's backend half."""

    def __init__(self, run: "Run"):
        run.taps += 1
        self.run = run
        self.number = run.taps
        self.events: list[dict] = []
        self.lock = threading.Lock()
        self.asked = 0
        self.log = (run.logs / f"tap-{self.number}.jsonl").open("w")
        self.process = subprocess.Popen(
            [str(run.app), "agent_pi::tests::native::tap", "--ignored", "--exact", "--nocapture", "--test-threads=1"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=(run.logs / f"tap-{self.number}.stderr").open("w"),
            env=run.env, cwd=str(run.work), text=True, bufsize=1,
        )
        threading.Thread(target=self.read, daemon=True).start()
        run.registry("add-process", "--pid", str(self.process.pid))
        self.ready = self.first(lambda event: "ready" in event, "Deck's backend to be ready", 30)["ready"]

    def read(self) -> None:
        for line in self.process.stdout:
            if not line.startswith("@tap "):
                continue
            event = json.loads(line[5:])
            with self.lock:
                self.events.append(event)
                self.log.write(json.dumps(event) + "\n")
                self.log.flush()

    def mark(self) -> int:
        with self.lock:
            return len(self.events)

    def since(self, mark: int = 0) -> list[dict]:
        with self.lock:
            return self.events[mark:]

    def first(self, wanted, label: str, timeout: float = 20, mark: int = 0) -> dict:
        return self.run.until(label, lambda: next((e for e in self.since(mark) if wanted(e)), None), timeout)

    def ask(self, op: str, **fields) -> dict:
        self.asked += 1
        number = self.asked
        self.process.stdin.write(json.dumps({"op": op, "id": number, **fields}) + "\n")
        self.process.stdin.flush()
        return self.first(lambda event: event.get("re") == number, f"the answer to {op}", 30)

    def now(self) -> dict:
        """What the store, the transport and the diagnostics held at the last change."""
        with self.lock:
            for event in reversed(self.events):
                if "now" in event:
                    return event["now"]
        return {"store": {}, "leases": [], "ended": [], "sessions": [], "unsupported": [], "drops": {}, "announced": {},
                "streams": 0, "pending": 0}

    def session(self, name: str) -> dict | None:
        """What the live lease of this card's Pi said of its native session, if anything."""
        lease = self.lease(name)
        return next((item for item in self.now()["sessions"] if lease and item["lease"] == lease["lease"]), None)

    def ended(self, session: str) -> list[str]:
        """Why this card's leases ended, in order: the closed words production writes to app.log."""
        return [item["code"] for item in self.now()["ended"] if item["session"] == session]

    def lease(self, session: str) -> dict | None:
        return next((lease for lease in self.now()["leases"] if lease["session"] == session), None)

    def state(self, session: str) -> str | None:
        return (self.now()["store"].get(session) or {}).get("state")

    def announced(self, session: str) -> int:
        return self.now()["announced"].get(session, 0)

    def drops(self, reason: str) -> int:
        return self.now()["drops"].get(reason, 0)

    def changes(self, session: str, mark: int) -> list[tuple[str, int] | None]:
        """Every change of one card's stored observation since `mark`, in order."""
        before = None
        for event in self.since(0)[:mark]:
            if "now" in event:
                before = event["now"]["store"].get(session)
        seen, last = [], before
        for event in self.since(mark):
            if "now" not in event:
                continue
            entry = event["now"]["store"].get(session)
            if (entry or {}).get("state") != (last or {}).get("state") or (entry or {}).get("episode") != (last or {}).get("episode"):
                seen.append((entry["state"], entry["episode"]) if entry else None)
            last = entry
        return seen

    def episodes(self, mark: int = 0) -> list[int]:
        found = set()
        for event in self.since(mark):
            if "now" in event:
                found.update(entry["episode"] for entry in event["now"]["store"].values())
        return sorted(found)

    def words(self) -> set[str]:
        """Every state word any card showed in this backend's life."""
        found = set()
        for event in self.since(0):
            if "now" in event:
                found.update(entry["state"] for entry in event["now"]["store"].values())
            for session in (event.get("poll") or {}).get("sessions", []):
                if session.get("agent"):
                    found.add(session["agent"])
        return found

    def card(self, session: str) -> dict:
        """One Board poll now, as the webview would get it, for one card."""
        reply = self.ask("poll")
        poll = reply["poll"]
        if "error" in poll:
            raise Failed(f"the Board poll failed: {poll['error']}")
        card = next(item for item in poll["sessions"] if item["name"] == session)
        card["scheduler"] = poll["scheduler"].get(session, {})
        return card

    def quit(self) -> None:
        try:
            self.process.stdin.write(json.dumps({"op": "quit"}) + "\n")
            self.process.stdin.flush()
            self.process.wait(timeout=10)
        except (OSError, subprocess.TimeoutExpired):
            pass


class Pane:
    """One card's session and the Pi that runs in it."""

    def __init__(self, run: "Run", session: str, runtime: str):
        self.run, self.session, self.runtime = run, session, runtime
        self.pid: int | None = None
        self.target = session

    def command(self, extensions=("fixture.ts",), flags=()) -> str:
        run = self.run
        program = [str(run.rt / "node"), str(run.npm_cli)] if self.runtime == "npm" else [str(run.rt / "pi" / "pi")]
        loaded = [part for name in extensions for part in ("-e", str(run.fixture / name))]
        return shlex.join([
            *program, "--no-skills", "--no-prompt-templates", "--no-themes", "--no-tools", *loaded,
            "--provider", "deck-fixture", "--model", "fixture", "--session-dir", str(run.sessions), *flags,
        ])

    def shell(self) -> int:
        return int(self.run.tm("display-message", "-p", "-t", self.target, "#{pane_pid}"))

    def foreground(self) -> str:
        return self.run.tm("display-message", "-p", "-t", self.target, "#{pane_current_command}")

    def type(self, text: str) -> None:
        self.run.tm("send-keys", "-t", self.target, "C-u")
        self.run.tm("send-keys", "-t", self.target, "-l", text)
        self.run.tm("send-keys", "-t", self.target, "Enter")

    def key(self, *keys: str) -> None:
        self.run.tm("send-keys", "-t", self.target, *keys)

    def screen(self) -> str:
        return self.run.tm("capture-pane", "-p", "-t", self.target, "-S", "-60")

    def child(self) -> int | None:
        table = subprocess.run(["/bin/ps", "-axo", "pid=,ppid="], capture_output=True, text=True).stdout
        shell = self.shell()
        children = [int(line.split()[0]) for line in table.splitlines() if len(line.split()) == 2 and int(line.split()[1]) == shell]
        return children[0] if len(children) == 1 else None

    def launch(self, extensions=("fixture.ts",), flags=(), expect_start: bool = True) -> None:
        """Start Pi from the pane's shell and wait until its session started."""
        run = self.run
        mark = run.source_mark()
        self.type(self.command(extensions, flags))
        self.pid = run.until(f"{self.session}: a program in front of the shell", self.child, 20)
        if expect_start:
            run.until(f"{self.session}: Pi's session start",
                      lambda: any(e["event"] == "session_start" and e["pid"] == self.pid for e in run.source(mark)), 30)
        else:
            time.sleep(3.0)
        # registered once it carries the name it keeps: an npm Pi renames itself while it starts
        if alive(self.pid):
            run.registry("add-process", "--pid", str(self.pid))

    def events(self, mark: int) -> list[dict]:
        return [event for event in self.run.source(mark) if event.get("pid") == self.pid]

    def names(self, mark: int) -> list[str]:
        return [event["event"] for event in self.events(mark)]

    def gone(self) -> bool:
        return self.pid is None or not alive(self.pid)

    def quit(self) -> None:
        if self.gone():
            return
        self.type("/quit")
        self.run.until(f"{self.session}: Pi to exit", lambda: self.gone() and self.foreground() == "zsh", 20)
        self.pid = None


class Run:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        self.taps = 0
        self.results: list[dict] = []
        self.facts: dict = {}
        root = Path(args.root) if args.root else Path(tempfile.mkdtemp(prefix="deck-pi-", dir="/private/tmp"))
        self.root = Path(os.path.realpath(root))
        if not str(self.root).startswith("/private/tmp/") or any(self.root.iterdir()):
            raise SystemExit(f"{self.root} is not a new, empty directory under /private/tmp")
        self.home = self.root / "home"
        self.bundle = self.home / "Applications/deck.app"
        self.app = self.bundle / "Contents/MacOS/deck-app"
        self.tmux = self.bundle / "Contents/MacOS/tmux"
        self.agent = self.root / "agent"
        self.settings = self.agent / "settings.json"
        self.work, self.sessions = self.root / "work", self.root / "sessions"
        self.fixture, self.rt, self.logs = self.root / "fixture", self.root / "rt", self.root / "logs"
        self.socket = self.root / "tt" / f"tmux-{os.getuid()}" / "deck-dev"
        self.entry = str(self.bundle / ENTRY)
        self.source_log = self.logs / "source.jsonl"
        self.npm_cli = NPM_PACKAGE / "dist/bundle/cli.js"
        self.env = {
            "HOME": str(self.home), "PATH": "/usr/bin:/bin", "TERM": "xterm-256color", "SHELL": "/bin/zsh",
            "LANG": "en_US.UTF-8", "TMPDIR": str(self.root / "tmp"), "TMUX_TMPDIR": str(self.root / "tt"),
            "PI_CODING_AGENT_DIR": str(self.agent), "PI_SKIP_VERSION_CHECK": "1", "PI_OFFLINE": "1",
            "PI_TELEMETRY": "0", "FIXTURE_LOG": str(self.source_log), "DECK_PI_NATIVE_ROOT": str(self.root),
        }
        self.tap: Tap | None = None
        self.a = Pane(self, "deck-a", "npm")
        self.b = Pane(self, "deck-b", "standalone")

    # ---- plumbing ------------------------------------------------------------

    def note(self, kind: str, **fields) -> None:
        with (self.logs / "driver.jsonl").open("a") as out:
            out.write(json.dumps({"t": round(time.time(), 3), "kind": kind, **fields}) + "\n")

    def registry(self, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        done = subprocess.run([sys.executable, str(RUNTIME), "experiment", *args, "--root", str(self.root)],
                              capture_output=True, text=True)
        self.note("registry", args=list(args), exit=done.returncode, out=done.stdout.strip(), err=done.stderr.strip())
        if check and done.returncode != 0:
            raise Failed(f"the registry refused {args[0]}: {done.stderr.strip()}")
        return done

    def tm(self, *args: str) -> str:
        return subprocess.check_output([str(self.tmux), "-S", str(self.socket), *args], env=self.env, text=True).strip()

    def until(self, label: str, predicate, timeout: float = 20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            value = predicate()
            if value:
                return value
            time.sleep(0.05)
        for pane in (self.a, self.b):
            try:
                (self.logs / f"screen-{pane.session}.txt").write_text(pane.screen())
            except (subprocess.CalledProcessError, OSError):
                pass
        raise Failed(f"timed out waiting for {label}")

    def source_mark(self) -> int:
        return len(self.source())

    def source(self, mark: int = 0) -> list[dict]:
        if not self.source_log.exists():
            return []
        return [json.loads(line) for line in self.source_log.read_text().splitlines()][mark:]

    def check(self, ac: str, name: str, ok, **detail) -> bool:
        self.results.append({"ac": ac, "check": name, "ok": bool(ok), "detail": detail})
        print(f"{'ok  ' if ok else 'FAIL'} {ac:<8} {name}" + ("" if ok else f"  {json.dumps(detail)}"), flush=True)
        self.note("check", ac=ac, name=name, ok=bool(ok), detail=detail)
        return bool(ok)

    # ---- the root --------------------------------------------------------------

    def prepare(self) -> None:
        args = self.args
        for directory in (self.home, self.agent, self.root / "deck", self.root / "tt", self.socket.parent,
                          self.root / "tmp", self.logs):
            directory.mkdir(parents=True, mode=0o700)
        for directory in (self.work, self.sessions, self.fixture, self.rt, self.app.parent, (self.bundle / ENTRY).parent):
            directory.mkdir(parents=True)
        self.registry("init")
        test_binary, standalone = Path(args.test_binary), Path(args.standalone)
        node = Path(os.path.realpath(shutil.which("node") or "/nonexistent"))
        for needed in (test_binary, standalone / "pi", node, self.npm_cli, TMUX):
            if not needed.is_file():
                raise SystemExit(f"{needed} is not a file")
        shutil.copy2(test_binary, self.app)
        shutil.copy2(TMUX, self.tmux)
        for name in OBSERVER_FILES:
            shutil.copyfile(RESOURCES / name, (self.bundle / ENTRY).parent / name)
        for name in ("fixture.ts", "delayed-ui.ts", "exec-peer.c"):
            shutil.copyfile(FIXTURES / name, self.fixture / name)
        shutil.copy2(node, self.rt / "node")
        shutil.copytree(standalone, self.rt / "pi", symlinks=True)
        self.settings.write_text(USER_SETTINGS)
        self.settings.chmod(0o600)
        version = lambda *command: subprocess.run(command, capture_output=True, text=True, env=self.env).stdout.strip()
        self.facts["artifacts"] = {
            "npm": {
                "package_version": json.loads((NPM_PACKAGE / "package.json").read_text())["version"],
                "cli_sha256": sha256(self.npm_cli),
                "node_version": version(str(self.rt / "node"), "--version"),
                "node_sha256": sha256(self.rt / "node"),
            },
            "standalone": {
                "version": version(str(self.rt / "pi" / "pi"), "--version"),
                "pi_sha256": sha256(self.rt / "pi" / "pi"),
            },
            "observer_sha256": {name: sha256(RESOURCES / name) for name in OBSERVER_FILES},
            "tmux_sha256": sha256(TMUX),
        }
        # Which build this run is of: the commit, what differed from it on disk, the sources as they
        # were, and the very executable under test (a copy of it is what runs here).
        self.facts["build"] = {
            **pi_build_identity.identity(REPO),
            "test_binary": test_binary.name,
            "test_binary_sha256": sha256(test_binary),
            "test_binary_run_sha256": sha256(self.app),
            "command": [Path(sys.argv[0]).name, *sys.argv[1:]],
        }

    def backend(self) -> Tap:
        self.tap = Tap(self)
        self.tap.ask("names", names=[self.a.session, self.b.session])
        return self.tap

    def server(self) -> None:
        """The tmux server, with the server defaults Deck writes, and one session per card."""
        conf = self.tap.ready["conf"]
        for index, pane in enumerate((self.a, self.b)):
            start = ["-f", conf] if index == 0 else []
            subprocess.check_call([str(self.tmux), *start, "-S", str(self.socket), "new-session", "-d", "-s", pane.session,
                                   "-x", "160", "-y", "45", "-c", str(self.work), "/bin/zsh", "-f"], env=self.env)
            if index == 0:
                self.registry("add-server", "--socket", str(self.socket), "--tmux", str(self.tmux))
        self.tap.ask("poll")

    # ---- scenarios ---------------------------------------------------------------

    def bound(self, pane: Pane, timeout: float = 30) -> dict:
        return self.until(f"{pane.session}: an observer lease", lambda: self.tap.lease(pane.session), timeout)

    def settings_loop(self) -> None:
        tap = self.tap
        ready = tap.ready
        self.check("A08", "this backend is a stable install only under the relocated home",
                   ready["settings"] == status(False, False, "absent") and ready["socket"] == "deck-dev",
                   settings=ready["settings"])
        before = self.settings.read_bytes()
        self.facts["settings_before_sha256"] = hashlib.sha256(before).hexdigest()
        reply = tap.ask("signal", enable=True)
        after = self.settings.read_bytes()
        document = json.loads(after)
        expected = json.loads(before)
        expected["extensions"].append(self.entry)
        self.check("A08", "the Settings command wrote Deck's one entry into Pi's own settings",
                   reply["ok"] and reply["status"] == status(True, False, "current") and document == expected,
                   reply={"ok": reply["ok"], "error": reply["error"], "status": reply["status"]})
        inserted = ",\n    " + json.dumps(self.entry)
        self.check("A05", "every byte the user wrote is where it was; only the entry was added",
                   after.decode() == before.decode().replace('"-builtin:tool-search"', '"-builtin:tool-search"' + inserted)
                   and (self.settings.stat().st_mode & 0o777) == 0o600)
        again = tap.ask("signal", enable=True)
        self.check("A04", "asking again writes nothing", again["ok"] and self.settings.read_bytes() == after)
        self.check("A08", "configured is not connected: no observer exists before a Pi loads the entry",
                   tap.now()["leases"] == [] and tap.now()["streams"] == 0)
        card = tap.card(self.a.session)
        self.check("A14", "a shell is no Pi: nothing is said about it",
                   card["fg"] == "zsh" and card["agent"] is None and "pi_signal" not in card and not card["scheduler"].get("pi"),
                   card=card)
        self.original_settings, self.enabled_settings = before, after

    def start_both(self) -> None:
        tap = self.tap
        for pane in (self.a, self.b):
            pane.launch()
            lease = self.bound(pane)
            card = tap.card(pane.session)
            facts = subprocess.run(["/bin/ps", "-o", "pid=,ppid=,pgid=,tpgid=,tty=,comm=", "-p", f"{pane.pid},{pane.shell()}"],
                                   capture_output=True, text=True).stdout.split("\n")
            rows = {int(line.split()[0]): line.split(None, 5) for line in facts if line.strip()}
            pi, shell = rows[pane.pid], rows[pane.shell()]
            self.facts.setdefault("process", {})[pane.runtime] = {
                "tmux_command": card["fg"], "ps_comm": Path(pi[5]).name,
                "parent_is_pane_shell": int(pi[1]) == pane.shell(), "leads_its_group": pi[2] == pi[0],
                "is_terminal_foreground": pi[3] == pi[0], "same_terminal_as_shell": pi[4] == shell[4] and pi[4] != "??",
            }
            self.check("A09", f"{pane.runtime}: the observer's process is the pane's foreground leader, a child of the pane's shell, on its terminal",
                       all(v for k, v in self.facts["process"][pane.runtime].items() if k not in ("tmux_command", "ps_comm")),
                       **self.facts["process"][pane.runtime])
            self.check("A08", f"{pane.runtime}: Pi loaded the entry and its hello was accepted by the production listener",
                       lease["run"] == 0 and not lease["open"] and card["pi_signal"] == "connected", lease_run=lease["run"])
            self.check("A14", f"{pane.runtime}: a loaded, idle Pi is not a turn that ended",
                       card["agent"] is None and card["episode"] is None and tap.announced(pane.session) == 0, card=card)
            self.check("Q01", f"{pane.runtime}: the scheduler recognizes the target as Pi, by the tick's evidence and before firing",
                       card["scheduler"] == {"pi": True, "pi_signal": "connected", "pre_fire_pi": True}, scheduler=card["scheduler"])
        leases = tap.now()["leases"]
        self.check("A10", "two Pis in two panes of one working directory hold two leases",
                   len(leases) == 2 and leases[0]["lease"] != leases[1]["lease"] and {l["session"] for l in leases} == {"deck-a", "deck-b"})

    def turn(self, pane: Pane, text: str, settled: int = 1, timeout: float = 30, before_wait=None) -> dict:
        """Type one line into Pi and wait until Pi settled and Deck shows the ending."""
        tap = self.tap
        source, mark = self.source_mark(), tap.mark()
        announced, run_before = tap.announced(pane.session), (tap.lease(pane.session) or {}).get("run", 0)
        known = tap.episodes()
        pane.type(text)
        if before_wait:
            before_wait()
        self.until(f"{pane.session}: {settled} settlement(s) after {text!r}",
                   lambda: pane.names(source).count("agent_settled") >= settled, timeout)
        self.until(f"{pane.session}: the ending on the card", lambda: tap.state(pane.session) == "turn-done"
                   and len([c for c in tap.changes(pane.session, mark) if c and c[0] == "turn-done"]) >= settled, 10)
        time.sleep(0.4)
        changes = tap.changes(pane.session, mark)
        fresh = [episode for episode in tap.episodes(mark) if episode not in known]
        return {
            "source": pane.names(source),
            "events": pane.events(source),
            "states": [change[0] if change else None for change in changes],
            "episodes": [change[1] for change in changes if change],
            "announced": tap.announced(pane.session) - announced,
            "runs": (tap.lease(pane.session) or {}).get("run", 0) - run_before,
            "new_episodes": fresh,
            "contiguous": fresh == list(range(min(fresh), min(fresh) + len(fresh))) if fresh else True,
        }

    def one_run(self, ac: str, pane: Pane, label: str, text: str, **kw) -> dict:
        seen = self.turn(pane, text, **kw)
        source = [name for name in seen["source"] if name.startswith("agent_")]
        self.check(ac, f"{pane.runtime}: {label}: one run, working then turn-done, announced once",
                   seen["states"] == ["working", "turn-done"] and seen["announced"] == 1 and seen["runs"] == 1
                   and len(seen["new_episodes"]) == 2 and seen["contiguous"] and source.count("agent_settled") == 1,
                   source=source, states=seen["states"], announced=seen["announced"], runs=seen["runs"],
                   new_episodes=len(seen["new_episodes"]))
        return seen

    def boundaries(self) -> None:
        for pane in (self.a, self.b):
            seen = self.one_run("A14", pane, "an ordinary run", "hello fixture")
            self.check("A14", f"{pane.runtime}: an ordinary run is one start, one end, one settlement in Pi",
                       [n for n in seen["source"] if n.startswith("agent_")] == ["agent_start", "agent_end", "agent_settled"])
            for label, command in (("a retry after a provider error", "/fx-retry"), ("a queued follow-up", "/fx-follow"),
                                   ("a continuation before the settlement", "/fx-continue")):
                seen = self.one_run("A15", pane, label, command)
                source = [name for name in seen["source"] if name.startswith("agent_")]
                requests = seen["source"].count("fixture_provider_request")
                self.check("A15", f"{pane.runtime}: {label}: Pi asked the provider more than once and settled once; Deck reported one ending",
                           requests >= 2 and source.count("agent_settled") == 1 and source[-1] == "agent_settled",
                           source=source, provider_requests=requests)
            seen = self.turn(pane, "/fx-settled", settled=2)
            source = [name for name in seen["source"] if name.startswith("agent_")]
            self.check("A16", f"{pane.runtime}: a run started by the settlement handler is a second run: done(A), working(B), done(B)",
                       seen["states"] == ["working", "turn-done", "working", "turn-done"] and seen["announced"] == 2
                       and seen["runs"] == 2 and len(set(seen["episodes"])) == 4 and seen["contiguous"]
                       and source.count("agent_settled") == 2,
                       source=source, states=seen["states"], announced=seen["announced"], runs=seen["runs"])
            # an abort: the provider is slow, Escape interrupts it
            def interrupt(pane=pane):
                self.until(f"{pane.session}: the slow run to start", lambda: self.tap.state(pane.session) == "working", 10)
                time.sleep(0.3)
                pane.key("Escape")
            seen = self.turn(pane, "/fx-slow 20000", before_wait=interrupt)
            settled = [event for event in seen["events"] if event["event"] == "agent_settled"]
            self.check("A17", f"{pane.runtime}: an aborted run ends as an interaction boundary, nothing more",
                       seen["states"] == ["working", "turn-done"] and seen["announced"] == 1 and len(settled) == 1
                       and settled[0].get("aborted") is True, states=seen["states"], aborted=settled[0].get("aborted") if settled else None)
            seen = self.turn(pane, "/fx-error")
            source = [name for name in seen["source"] if name.startswith("agent_")]
            self.check("A17", f"{pane.runtime}: a run that ends in a provider error is the same boundary",
                       seen["states"] == ["working", "turn-done"] and seen["announced"] == 1 and source.count("agent_settled") == 1,
                       source=source, states=seen["states"])
            pane.type("/fx-ok")
            time.sleep(0.3)

    def viewed(self) -> None:
        tap, pane = self.tap, self.a
        entry = tap.now()["store"][pane.session]
        tap.ask("viewed", session=pane.session, episode=entry["episode"] + 1000)
        time.sleep(0.2)
        untouched = tap.now()["store"][pane.session]["viewed"] is False
        tap.ask("viewed", session=pane.session, episode=entry["episode"])
        self.until("the ending to read as viewed", lambda: tap.now()["store"][pane.session]["viewed"], 5)
        card = tap.card(pane.session)
        self.check("A26", "a Pi ending is viewed by its own episode, through the shared dismiss command",
                   untouched and card["agent"] == "turn-done" and card["episode_viewed"] is True and card["episode"] == entry["episode"])
        seen = self.one_run("A26", pane, "the next run after a viewed ending", "hello fixture")
        self.check("A26", "the next ending is a new, unviewed episode",
                   tap.now()["store"][pane.session]["viewed"] is False and seen["episodes"][-1] != entry["episode"])

    def dialogs(self) -> None:
        tap = self.tap
        for pane in (self.a, self.b):
            before = tap.now()["store"].get(pane.session)
            unrun = []
            for kind, close in (("confirm", ["Escape"]), ("input", ["Escape"]), ("select", ["Escape"]), ("editor", ["Escape"]),
                                ("custom", ["x"]), ("timed", [])):
                source, mark = self.source_mark(), tap.mark()
                pane.type(f"/fx-{kind}")
                done = lambda: any(e["event"] == "fixture_command_done" and e.get("command") == kind for e in pane.events(source))
                try:
                    self.until(f"{pane.session}: the {kind} dialog", lambda: "ui_prompt_start" in pane.names(source) or done(), 10)
                    time.sleep(0.4)
                    for key in close:
                        pane.key(key)
                        time.sleep(0.3)
                    self.until(f"{pane.session}: the {kind} dialog to close", done, 10)
                except Failed:
                    unrun.append(kind)
                    pane.key("Escape")
                    pane.key("C-c")
                    time.sleep(0.5)
                    continue
                time.sleep(0.5)
                names = pane.names(source)
                self.check("A18", f"{pane.runtime}: Pi raised a {kind} dialog; Deck's card did not change and never asked for input",
                           "ui_prompt_start" in names and "ui_prompt_end" in names and tap.changes(pane.session, mark) == []
                           and tap.now()["store"].get(pane.session) == before,
                           prompts=[n for n in names if n.startswith("ui_prompt")], changes=len(tap.changes(pane.session, mark)))
            self.facts.setdefault("dialogs_unrun", {})[pane.runtime] = unrun
            self.check("A18", f"{pane.runtime}: every dialog kind was raised", unrun == [], unrun=unrun)
            card = tap.card(pane.session)
            self.check("A18", f"{pane.runtime}: after the dialogs the card says what it said, with the limited-coverage word",
                       card["agent"] == "turn-done" and card["pi_signal"] == "connected", card=card)
            # Two dialogs at once. Pi 1.1.0 shows the second over the first, announces ONE start for both, and the
            # command that raised them does not return after both were dismissed; that Pi is replaced afterwards.
            source, mark = self.source_mark(), tap.mark()
            pane.type("/fx-nested")
            self.until(f"{pane.session}: the two dialogs", lambda: "ui_prompt_start" in pane.names(source), 10)
            for _ in range(2):
                time.sleep(0.5)
                pane.key("Escape")
            time.sleep(1.5)
            names = pane.names(source)
            self.facts.setdefault("two_dialogs_at_once", {})[pane.runtime] = {
                "starts_announced": names.count("ui_prompt_start"), "ends_announced": names.count("ui_prompt_end"),
                "command_returned": "fixture_command_done" in names,
            }
            self.check("A18", f"{pane.runtime}: two dialogs at once: Pi announces one start for both; Deck's card did not change",
                       names.count("ui_prompt_start") == 1 and tap.changes(pane.session, mark) == []
                       and tap.now()["store"].get(pane.session) == before, **self.facts["two_dialogs_at_once"][pane.runtime])
            pane.quit()
            pane.launch()
            self.bound(pane)
            self.one_run("A18", pane, "a fresh Pi after the dialogs", "hello fixture")
            # Pi's own dialogs raise no extension event at all
            for builtin in ("/model", "/resume"):
                source, mark = self.source_mark(), tap.mark()
                pane.type(builtin)
                time.sleep(0.8)
                pane.key("Escape")
                time.sleep(0.5)
                names = pane.names(source)
                self.check("A20", f"{pane.runtime}: Pi's built-in {builtin} dialog emits no prompt event; the card does not change",
                           not any(n.startswith("ui_prompt") for n in names) and tap.changes(pane.session, mark) == [],
                           events=names)

    def together(self) -> None:
        tap, a, b = self.tap, self.a, self.b
        mark = tap.mark()
        announced = {pane.session: tap.announced(pane.session) for pane in (a, b)}
        a.type("/fx-slow 5000")
        self.until("A working", lambda: tap.state(a.session) == "working", 10)
        b.type("/fx-slow 1500")
        self.until("B working", lambda: tap.state(b.session) == "working", 10)
        both = tap.state(a.session) == "working" and tap.state(b.session) == "working"
        self.until("B's ending", lambda: tap.state(b.session) == "turn-done", 15)
        a_still = tap.state(a.session) == "working"
        b_episode = tap.now()["store"][b.session]["episode"]
        self.until("A's ending", lambda: tap.state(a.session) == "turn-done", 15)
        time.sleep(0.4)
        self.check("A10", "two Pis at once: each card follows its own Pi, and one's ending does not end the other's run",
                   both and a_still and tap.now()["store"][b.session]["episode"] == b_episode
                   and [c[0] for c in tap.changes(a.session, mark)] == ["working", "turn-done"]
                   and [c[0] for c in tap.changes(b.session, mark)] == ["working", "turn-done"]
                   and all(tap.announced(p.session) - announced[p.session] == 1 for p in (a, b)))

    def inactive_pane(self) -> None:
        tap, pane = self.tap, self.a
        own = self.tm("display-message", "-p", "-t", pane.session, "#{pane_id}")
        self.one_run("A10", pane, "before the split", "hello fixture")
        self.tm("split-window", "-t", pane.session, "-c", str(self.work), "/bin/zsh", "-f")
        other = self.tm("display-message", "-p", "-t", pane.session, "#{pane_id}")
        card = tap.card(pane.session)
        self.check("A10", "another pane in front: the card says nothing of the Pi behind it",
                   other != own and card["agent"] is None and "pi_signal" not in card
                   and tap.lease(pane.session) is not None, card=card)
        announced, source, mark = tap.announced(pane.session), self.source_mark(), tap.mark()
        pane.target = own
        pane.type("hello fixture")
        self.until("the hidden Pi's settlement", lambda: "agent_settled" in pane.names(source), 20)
        time.sleep(1.0)
        hidden = tap.card(pane.session)
        self.check("A10", "a run in the pane that is not in front reaches no card surface",
                   hidden["agent"] is None and tap.changes(pane.session, mark) == [] and tap.announced(pane.session) == announced,
                   card=hidden)
        self.tm("select-pane", "-t", own)
        shown = tap.card(pane.session)
        self.check("A10", "back in front, the card shows what that pane's Pi last reported",
                   shown["agent"] == "turn-done" and shown["pi_signal"] == "connected" and tap.announced(pane.session) == announced + 1,
                   card=shown)
        self.tm("kill-pane", "-t", other)
        pane.target = pane.session

    def reload(self) -> None:
        tap = self.tap
        for pane in (self.a, self.b):
            self.one_run("A22", pane, "before the reload", "hello fixture")
            old, announced = tap.lease(pane.session)["lease"], tap.announced(pane.session)
            source, mark = self.source_mark(), tap.mark()
            pane.type("/reload")
            self.until(f"{pane.session}: a new lease after the reload",
                       lambda: (tap.lease(pane.session) or {}).get("lease", old) != old, 30)
            time.sleep(0.5)
            reasons = [(e["event"], e.get("reason")) for e in pane.events(source) if e["event"].startswith("session_")]
            card = tap.card(pane.session)
            self.check("A22", f"{pane.runtime}: a reload withdraws what the old runtime reported; the new one starts with nothing and replays nothing",
                       ("session_shutdown", "reload") in reasons and tap.changes(pane.session, mark) == [None]
                       and card["agent"] is None and card["pi_signal"] == "connected" and tap.announced(pane.session) == announced
                       and tap.lease(pane.session)["run"] == 0 and tap.ended(pane.session)[-1] == "reset-reload",
                       reasons=reasons, card=card, ended=tap.ended(pane.session)[-1:])

    def frozen(self) -> None:
        tap = self.tap
        panes = (self.a, self.b)
        for pane in panes:
            self.one_run("A22", pane, "before the silence", "hello fixture")
        stale, announced = tap.drops("pi-stale"), {p.session: tap.announced(p.session) for p in panes}
        marks = {p.session: tap.mark() for p in panes}
        started = time.monotonic()
        for pane in panes:
            pane.type("/fx-freeze 19000")
        self.until("both silent streams to be ended", lambda: tap.now()["leases"] == [], 25)
        took = time.monotonic() - started
        cards = {p.session: tap.card(p.session) for p in panes}
        self.check("A22", "a Pi that says nothing for fifteen seconds loses its observation: no ending is made up, the card says unavailable",
                   9 <= took <= 17 and tap.drops("pi-stale") == stale + 2
                   and all(tap.ended(p.session)[-1] == "stale" for p in panes)
                   and all(card["agent"] is None and card["pi_signal"] == "unavailable" for card in cards.values())
                   and all(tap.changes(p.session, marks[p.session]) == [None] for p in panes),
                   seconds=round(took, 1), stale=tap.drops("pi-stale") - stale, cards=cards)
        self.check("Q02", "an unavailable Pi is still a Pi to the scheduler",
                   all(card["scheduler"]["pi"] and card["scheduler"]["pre_fire_pi"] for card in cards.values()))
        for pane in panes:
            self.bound(pane, 40)
        time.sleep(0.5)
        cards = {p.session: tap.card(p.session) for p in panes}
        self.check("A22", "when Pi answers again its observer reconnects by itself and reports nothing of the past",
                   all(card["agent"] is None and card["pi_signal"] == "connected" for card in cards.values())
                   and all(tap.announced(p.session) == announced[p.session] for p in panes), cards=cards)

    def restart(self) -> None:
        a, b, old = self.a, self.b, self.tap
        self.one_run("A23", a, "before Deck stops", "hello fixture")
        source = self.source_mark()
        b.type("/fx-slow 25000")
        self.until("B working", lambda: old.state(b.session) == "working", 10)
        self.registry("stop", "--pid", str(old.process.pid))
        self.until("the old backend to be gone", lambda: old.process.poll() is not None, 10)
        time.sleep(1.5)
        tap = self.backend()
        for pane in (a, b):
            self.bound(pane, 40)
        self.until("B's run on the new backend", lambda: tap.state(b.session) == "working", 10)
        card_a, lease_b = tap.card(a.session), tap.lease(b.session)
        self.check("A23", "Deck restarted while Pi ran: an idle Pi's old ending is not replayed",
                   card_a["agent"] is None and card_a["pi_signal"] == "connected" and tap.announced(a.session) == 0, card=card_a)
        self.check("A23", "Deck restarted while Pi ran: an active run is reported again as working, on a new lease",
                   lease_b["run"] == 1 and lease_b["open"] and tap.state(b.session) == "working")
        self.until("B's settlement", lambda: "agent_settled" in b.names(source), 40)
        self.until("B's ending on the new backend", lambda: tap.state(b.session) == "turn-done", 10)
        self.check("A23", "and that run's ending is reported once",
                   [c[0] for c in tap.changes(b.session, 0)] == ["working", "turn-done"] and tap.announced(b.session) == 1)

    def disable(self) -> None:
        tap, a, b = self.tap, self.a, self.b
        self.one_run("A25", a, "before the switch is turned off", "hello fixture")
        b_source = self.source_mark()
        b.type("/fx-slow 6000")
        self.until("B working", lambda: tap.state(b.session) == "working", 10)
        # Pi wrote this file itself since Deck's entry went in (its own keys, its own layout): what Deck
        # may change is still its one entry, in whatever the file is now
        current = self.settings.read_bytes()
        element = ",\n    " + json.dumps(self.entry)
        self.facts["pi_rewrote_its_settings_meanwhile"] = current != self.enabled_settings
        refused, mark, asked = tap.drops("pi-capability-disabled"), tap.mark(), time.time() * 1000
        reply = tap.ask("signal", enable=False)
        withdrawn = tap.first(lambda e: "now" in e and e["now"]["leases"] == [] and e["now"]["store"] == {}, "the withdrawal", 5, mark)
        self.check("A25", "turning the switch off withdraws every Pi observation at once",
                   reply["ok"] and reply["status"] == status(False, False, "absent")
                   and withdrawn["wall"] - asked < 1000 and tap.changes(a.session, mark) == [None] and tap.changes(b.session, mark) == [None]
                   and all(tap.ended(p.session)[-1] == "capability-disabled" for p in (a, b)),
                   milliseconds=round(withdrawn["wall"] - asked))
        self.check("A05", "turning it off takes Deck's entry out and leaves every other byte, Pi's own later changes included",
                   current.decode().count(element) == 1 and self.settings.read_bytes().decode() == current.decode().replace(element, "")
                   and json.loads(self.settings.read_bytes())["extensions"] == json.loads(self.original_settings)["extensions"]
                   and (self.settings.stat().st_mode & 0o777) == 0o600,
                   pi_rewrote_the_file=self.facts["pi_rewrote_its_settings_meanwhile"])
        self.until("the two refusals to be counted", lambda: tap.drops("pi-capability-disabled") == refused + 2, 5)
        self.until("B's run to settle in Pi", lambda: "agent_settled" in b.names(b_source), 20)
        a_source = self.source_mark()
        a.type("hello fixture")
        self.until("A's run to settle in Pi", lambda: "agent_settled" in a.names(a_source), 20)
        time.sleep(2.5)
        cards = {p.session: tap.card(p.session) for p in (a, b)}
        self.check("A25", "a Pi that has not reloaded keeps running and writes nothing: no word, no stream, no new refusal",
                   tap.now()["store"] == {} and tap.now()["leases"] == [] and tap.now()["streams"] == 0
                   and tap.drops("pi-capability-disabled") == refused + 2
                   and all(card["agent"] is None and card["pi_signal"] == "unavailable" for card in cards.values()), cards=cards)
        self.check("Q02", "turned off, both are still Pi to the scheduler",
                   all(card["scheduler"]["pi"] and card["scheduler"]["pre_fire_pi"] for card in cards.values()))
        reply = tap.ask("signal", enable=True)
        time.sleep(3.0)
        self.check("A08", "turned on again: the file names the entry, the Pis that loaded before it are still not connected, "
                          "and Settings says that a running Pi needs its reload",
                   reply["ok"] and reply["status"] == status(True, False, "needs-reload")
                   and self.settings.read_bytes() == current and tap.now()["leases"] == [], status=reply["status"])
        a.type("/reload")
        self.bound(a)
        waiting = tap.ask("status")
        b.quit()
        b.launch()
        self.bound(b)
        cards = {p.session: tap.card(p.session) for p in (a, b)}
        self.check("A08", "a reload, or a new Pi, loads the entry and connects",
                   all(card["agent"] is None and card["pi_signal"] == "connected" for card in cards.values()), cards=cards)
        after = {key: tap.ask("status")[key] for key in ("pi", "pi_lifecycle", "pi_installation")}
        self.check("F03", "the reload note follows the Pis that run: still there while one has not reloaded, gone once none waits",
                   waiting["pi_installation"] == "needs-reload" and after == status(True, False, "current"),
                   while_one_waits=waiting["pi_installation"], after=after)

    def not_loaded(self) -> None:
        tap = self.tap
        resources = (self.bundle / ENTRY).parent
        observer = resources / "observer.ts"

        def expect_nothing(ac: str, pane: Pane, label: str, flags=()) -> None:
            mark, streams = tap.mark(), tap.drops("pi-capability-disabled")
            pane.launch(flags=flags, expect_start=False)
            time.sleep(1.0)
            running = not pane.gone()
            if running:
                source = self.source_mark()
                pane.type("hello fixture")
                try:
                    self.until(f"{pane.session}: a run without an observer", lambda: "agent_settled" in pane.names(source), 15)
                except Failed:
                    pass
                time.sleep(1.0)
            card = tap.card(pane.session)
            said = card.get("pi_signal") if running else None
            self.check(ac, f"{pane.runtime}: {label}: no hello, nothing reported, and the card never says connected",
                       tap.lease(pane.session) is None and tap.changes(pane.session, mark) == [] and card["agent"] is None
                       and said in (None, "unavailable") and (not running or card["scheduler"]["pi"]),
                       running=running, card=card)
            pane.quit()

        for pane in (self.a, self.b):
            pane.quit()
            expect_nothing("A21", pane, "started with --no-extensions", flags=("--no-extensions",))
        # the user's own entry disables Deck's: Pi does not load it, and Deck does not argue
        text = self.enabled_settings.decode()
        self.settings.write_text(text.replace(json.dumps(self.entry), json.dumps("-" + self.entry) + ",\n    " + json.dumps(self.entry)))
        status = tap.ask("status")
        refused = tap.ask("signal", enable=True)
        self.check("A06", "the user's own disabling entry wins: conflict, and the switch is refused without a write",
                   status["pi_installation"] == "conflict" and not refused["ok"] and refused["error"] == "pi-config-conflict")
        for pane in (self.a, self.b):
            expect_nothing("A21", pane, "the user's negative entry")
        self.settings.write_bytes(self.enabled_settings)
        # the bundle's files are gone, then broken
        resources.rename(resources.with_name("pi.away"))
        for pane in (self.a, self.b):
            expect_nothing("A21", pane, "the entry's file is missing")
        resources.with_name("pi.away").rename(resources)
        # Pi 1.1.0 refuses to start when an extension fails to load: there is then no Pi to say anything about
        kept = observer.read_bytes()
        observer.write_text('throw new Error("fixture: the observer fails to load");\n')
        for pane in (self.a, self.b):
            expect_nothing("A21", pane, "the entry throws while Pi loads it")
        observer.write_bytes(kept)
        self.check("A21", "the bundle's files are again what this build ships",
                   all((resources / name).read_bytes() == (RESOURCES / name).read_bytes() for name in OBSERVER_FILES))
        # a boot migration moves an entry of the other stable location and enrolls nobody
        self.settings.write_text(text.replace(json.dumps(self.entry), json.dumps("/Applications/deck.app/" + ENTRY)))
        moved = tap.ask("migrate")
        self.check("A06", "the boot migration rewrites an entry of the other stable location to this bundle, and nothing else",
                   (moved["pi"], moved["pi_installation"]) == (True, "current") and self.settings.read_bytes() == self.enabled_settings)
        self.settings.write_bytes(self.original_settings)
        untouched = tap.ask("migrate")
        self.check("A06", "without an entry the boot migration enrolls nobody",
                   untouched["pi_installation"] == "absent" and self.settings.read_bytes() == self.original_settings)
        self.settings.write_bytes(self.enabled_settings)
        tap.ask("status")

    def out_of_order(self) -> None:
        """Why Deck reports no input request for Pi: the upstream reorder, reproduced."""
        tap = self.tap
        for pane in (self.a, self.b):
            pane.quit()
            pane.launch(extensions=("delayed-ui.ts", "fixture.ts"))
            self.bound(pane)
            # a dialog that closed by itself: a later extension hears its end before its start
            source, mark = self.source_mark(), tap.mark()
            pane.type("/fx-timed")
            self.until(f"{pane.session}: the late start notification", lambda: "ui_prompt_start" in pane.names(source), 15)
            time.sleep(0.3)
            names = [n for n in pane.names(source) if n.startswith("ui_prompt")]
            self.check("A19", f"{pane.runtime}: a later extension hears the dialog's end BEFORE its start (the upstream reorder)",
                       names[:2] == ["ui_prompt_end", "ui_prompt_start"] and tap.changes(pane.session, mark) == [], heard=names)
            # the same across a whole run: the start arrives after the run has settled
            source, mark = self.source_mark(), tap.mark()
            pane.type("/fx-timed")
            self.until(f"{pane.session}: the dialog to close by itself",
                       lambda: any(e["event"] == "fixture_command_done" for e in pane.events(source)), 10)
            pane.type("hello fixture")
            self.until(f"{pane.session}: the stale start after the run", lambda: "ui_prompt_start" in pane.names(source)
                       and "agent_settled" in pane.names(source), 20)
            time.sleep(0.4)
            names = [n for n in pane.names(source) if n.startswith("ui_prompt") or n in ("agent_start", "agent_settled")]
            late = names.index("ui_prompt_start") > names.index("agent_settled") and names.index("ui_prompt_end") < names.index("agent_start")
            self.check("A19", f"{pane.runtime}: the start of a dialog that closed before a run arrives after that run settled",
                       late, heard=names)
            self.check("A19", f"{pane.runtime}: Deck's card shows that run and nothing else: no input request exists to be wrong",
                       [c[0] for c in tap.changes(pane.session, mark)] == ["working", "turn-done"], states=tap.changes(pane.session, mark))

    def endings(self) -> None:
        tap, a, b = self.tap, self.a, self.b
        # a quit says so; the process exit is the kernel's own fact
        self.one_run("A22", a, "before the quit", "hello fixture")
        source, mark, announced = self.source_mark(), tap.mark(), tap.announced(a.session)
        pid = a.pid
        a.quit()
        self.until("the lease to end", lambda: tap.lease(a.session) is None, 10)
        card = tap.card(a.session)
        reasons = [(e["event"], e.get("reason")) for e in self.source(source) if e.get("pid") == pid and e["event"].startswith("session_")]
        self.check("A22", "a quit withdraws the card's word; a shell in front is no Pi and nothing is made up",
                   ("session_shutdown", "quit") in reasons and tap.changes(a.session, mark) == [None] and card["fg"] == "zsh"
                   and card["agent"] is None and "pi_signal" not in card and not card["scheduler"].get("pi")
                   and tap.announced(a.session) == announced and tap.ended(a.session)[-1] == "reset-quit",
                   card=card, ended=tap.ended(a.session)[-1:])
        # a Pi that dies without a word: the stream's end is all Deck hears
        self.one_run("A22", b, "before the kill", "hello fixture")
        source, mark, announced = self.source_mark(), tap.mark(), tap.announced(b.session)
        pid = b.pid
        b.type("/fx-crash")
        self.until("the killed Pi to be gone", lambda: not alive(pid) and tap.lease(b.session) is None, 15)
        time.sleep(0.5)
        b.key("Enter")
        self.until("the shell to be in front again", lambda: b.foreground() == "zsh", 10)
        card = tap.card(b.session)
        said = [e["event"] for e in self.source(source) if e.get("pid") == pid and e["event"].startswith("session_")]
        self.check("A22", "a Pi killed without a shutdown: its observation is withdrawn, no ending and no quit are invented",
                   said == [] and tap.changes(b.session, mark) == [None] and card["agent"] is None and tap.announced(b.session) == announced
                   and tap.ended(b.session)[-1] == "eof", card=card, ended=tap.ended(b.session)[-1:])
        b.pid = None
        # a program that merely is Node is not Pi
        a.type(shlex.join([str(self.rt / "node"), "-e", "setTimeout(() => {}, 6000)"]))
        node = self.until("node in front", a.child, 10)
        self.registry("add-process", "--pid", str(node))
        card = tap.card(a.session)
        self.check("Q05", "a plain node process is not recognized as Pi by its name",
                   card["fg"] == "node" and not card["scheduler"].get("pi") and "pi_signal" not in card and card["agent"] is None, card=card)
        self.until("node to exit", lambda: not alive(node), 15)

    # ---- P2: the native session a Pi runtime is in --------------------------------

    def entry_of(self, name: str) -> str:
        return str((self.bundle / ENTRY).with_name(name))

    def started(self, pane: Pane, mark: int) -> dict:
        """The last session start Pi itself logged for this pane's process since `mark`."""
        return [e for e in pane.events(mark) if e["event"] == "session_start"][-1]

    def observed(self, pane: Pane, timeout: float = 30) -> dict:
        return self.until(f"{pane.session}: a session observation", lambda: self.tap.session(pane.session), timeout)

    def session_file(self, native: str) -> Path:
        named = [path for path in self.sessions.rglob("*.jsonl") if native in path.name]
        if named:
            return named[0]
        return next(path for path in self.sessions.rglob("*.jsonl") if native in path.read_text(errors="replace")[:600])

    def replaced(self, pane: Pane, command: str, reason: str, ac: str, label: str, same_id: bool | None = None) -> dict:
        """One in-process session change: Pi tears a runtime down and starts another."""
        tap = self.tap
        before, old = self.observed(pane), tap.lease(pane.session)["lease"]
        source, mark = self.source_mark(), tap.mark()
        pane.type(command)
        self.until(f"{pane.session}: a new lease after {command.split()[0]}",
                   lambda: (tap.lease(pane.session) or {}).get("lease", old) != old and tap.session(pane.session), 30)
        time.sleep(0.4)
        said = [(e["event"], e.get("reason")) for e in pane.events(source) if e["event"].startswith("session_")]
        native, now = self.started(pane, source), self.observed(pane)
        torn = any(item["lease"] == old and item["last"] == f"shutdown-{reason}"
                   for event in tap.since(mark) if "now" in event for item in event["now"]["sessions"])
        self.facts.setdefault("teardown_seen_before_the_reset", {})[f"{pane.runtime}:{reason}"] = torn
        same = now["id"] == before["id"]
        self.check(ac, f"{pane.runtime}: {label}: Pi's own reasons, the id Pi names, a new lease, the old one ended by its reset",
                   said == [("session_shutdown", reason), ("session_start", reason)] and now["id"] == native["id"]
                   and now["last"] == f"start-{reason}" and tap.ended(pane.session)[-1] == f"reset-{reason}"
                   and (same_id is None or same == same_id),
                   said=said, last=now["last"], same_id_as_before=same, ended=tap.ended(pane.session)[-1:])
        return now

    def native_sessions(self) -> None:
        tap, a, b = self.tap, self.a, self.b
        for pane in (a, b):
            pane.quit()
        every = [json.loads(line) for number in range(1, self.taps + 1)
                 for line in (self.logs / f"tap-{number}.jsonl").read_text().splitlines()]
        self.check("B01", "with work status alone no session was ever named: no id reached Deck",
                   all(not event["now"].get("sessions") for event in every if "now" in event))
        # the second switch: the ONE entry is replaced where it stood
        before = self.settings.read_text()
        reply = tap.ask("lifecycle", enable=True)
        self.check("B01", "session observation is a second switch on the same entry: one entry, now the one that carries both",
                   reply["ok"] and reply["status"] == status(True, True, "current")
                   and before.count(json.dumps(self.entry)) == 1
                   and self.settings.read_text() == before.replace(json.dumps(self.entry), json.dumps(self.entry_of("both.ts"))),
                   status=reply["status"])
        # a session started with Pi: startup
        source = self.source_mark()
        a.launch()
        self.bound(a)
        first, seen = self.started(a, source), self.observed(a)
        card = tap.card(a.session)
        self.check("B03", "npm: the session Pi started in is observed, with Pi's own reason, and the card says only that one is observed",
                   seen["id"] == first["id"] and first["reason"] == "startup" and seen["last"] == "start-startup"
                   and card.get("pi_session") == "observed" and card["pi_signal"] == "connected", last=seen["last"])
        self.one_run("B03", a, "a run in the first session", "hello fixture")
        self.check("B02", "a run changes nothing about the observed session", self.observed(a)["id"] == first["id"])
        ids = [first["id"]]
        second = self.replaced(a, "/fx-new", "new", "B03", "a new session", same_id=False)
        ids.append(second["id"])
        self.one_run("B03", a, "a run in the second session", "hello fixture")
        third = self.replaced(a, "/fx-fork", "fork", "B03", "a fork", same_id=False)
        ids.append(third["id"])
        self.check("B03", "new and fork each named a session of its own", len(set(ids)) == 3)
        back = self.replaced(a, f"/fx-resume {self.session_file(first['id'])}", "resume", "B03", "a resume while Pi runs")
        self.check("B03", "the resumed session is the first one, by the id Pi names", back["id"] == first["id"])
        self.replaced(a, "/reload", "reload", "B05", "a reload: another runtime in the same session", same_id=True)
        # the same changes inside a running Pi of the standalone build
        source = self.source_mark()
        b.launch()
        self.bound(b)
        b_first, b_seen = self.started(b, source), self.observed(b)
        self.check("B03", "standalone: the session Pi started in is observed, with Pi's own reason",
                   b_seen["id"] == b_first["id"] and b_first["reason"] == "startup" and b_seen["last"] == "start-startup"
                   and tap.card(b.session).get("pi_session") == "observed", last=b_seen["last"])
        self.one_run("B03", b, "a run in the first session", "hello fixture")
        b_ids = [b_first["id"]]
        b_ids.append(self.replaced(b, "/fx-new", "new", "B03", "a new session", same_id=False)["id"])
        self.one_run("B03", b, "a run in the second session", "hello fixture")
        b_ids.append(self.replaced(b, "/fx-fork", "fork", "B03", "a fork", same_id=False)["id"])
        b_back = self.replaced(b, f"/fx-resume {self.session_file(b_first['id'])}", "resume", "B03", "a resume while Pi runs")
        self.check("B03", "standalone: new and fork each named a session of its own, and the resumed one is the first",
                   len(set(b_ids)) == 3 and b_back["id"] == b_first["id"] and not set(b_ids) & set(ids))
        self.replaced(b, "/reload", "reload", "B05", "a reload: another runtime in the same session", same_id=True)
        ids.extend(b_ids)
        b.quit()
        # quit: the runtime's own word; then a session named on the command line starts as a startup
        source, mark, lease = self.source_mark(), tap.mark(), tap.lease(a.session)["lease"]
        pid = a.pid
        a.quit()
        self.until("the quit lease to end", lambda: tap.lease(a.session) is None, 10)
        said = [(e["event"], e.get("reason")) for e in self.source(source) if e.get("pid") == pid and e["event"].startswith("session_")]
        self.check("B07", "npm: a quit is the runtime's teardown, said by Pi and ended by its reset; nothing of it is kept",
                   said == [("session_shutdown", "quit")] and tap.ended(a.session)[-1] == "reset-quit"
                   and not any(item["lease"] == lease for item in tap.now()["sessions"]))
        source = self.source_mark()
        a.launch(flags=("--session", str(self.session_file(first["id"]))))
        self.bound(a)
        named, seen = self.started(a, source), self.observed(a)
        self.check("B04", "npm: a session named on the command line starts as a startup: Deck does not call it a resume",
                   named["reason"] == "startup" and named["id"] == first["id"] and seen["id"] == first["id"]
                   and seen["last"] == "start-startup", reason=named["reason"], last=seen["last"])
        # the same session in a second pane: two processes, two leases, two observations
        source = self.source_mark()
        b.launch(flags=("--session", str(self.session_file(first["id"]))))
        self.bound(b)
        other = self.observed(b)
        self.check("B06", "one session in two panes is two observations, each of its own process and lease",
                   other["id"] == first["id"] == self.observed(a)["id"] and other["lease"] != self.observed(a)["lease"]
                   and len(tap.now()["sessions"]) == 2)
        # a Pi that dies without a word: no teardown is observed or invented
        mark, lease, pid = tap.mark(), tap.lease(b.session)["lease"], b.pid
        b.type("/fx-crash")
        self.until("the killed Pi to be gone", lambda: not alive(pid) and tap.lease(b.session) is None, 15)
        b.key("Enter")
        self.until("the shell in front again", lambda: b.foreground() == "zsh", 10)
        b.pid = None
        torn = any(item["lease"] == lease and item["last"].startswith("shutdown")
                   for event in tap.since(mark) if "now" in event for item in event["now"]["sessions"])
        self.check("B07", "standalone: a Pi killed without a shutdown: the stream's end is all there is, no teardown and no quit",
                   not torn and tap.ended(b.session)[-1] == "eof" and len(tap.now()["sessions"]) == 1
                   and self.observed(a)["id"] == first["id"], ended=tap.ended(b.session)[-1:])
        # what the command line can name, on the standalone build
        for flags, label, expect in (
            (("--session", str(self.sessions / "not-there.jsonl")), "a session path that does not exist creates a session", "new"),
            (("--session-id", "fixture.custom-ID_123"), "a session id of the user's own, not a UUID, is kept as it is", "custom"),
            (("--session", "00000000-0000-4000-8000-00000000dead"), "a session id that does not exist starts no Pi", "none"),
        ):
            source = self.source_mark()
            b.launch(flags=flags, expect_start=expect != "none")
            if expect == "none":
                time.sleep(1.0)
                self.check("B04", f"standalone: {label}: no hello, no observation",
                           b.gone() and tap.lease(b.session) is None and len(tap.now()["sessions"]) == 1)
                b.pid = None
                continue
            self.bound(b)
            named, seen = self.started(b, source), self.observed(b)
            ok = seen["id"] == named["id"] and named["reason"] == "startup" and seen["last"] == "start-startup"
            if expect == "new":
                ok = ok and named["id"] not in ids
            else:
                ok = ok and named["id"] == "fixture.custom-ID_123"
            ids.append(named["id"])
            self.check("B04" if expect == "new" else "B02", f"standalone: {label}", ok, reason=named["reason"])
            b.quit()
        # session observation alone: no work status, and a run says nothing on the card
        a.quit()
        reply = tap.ask("signal", enable=False)
        self.check("B01", "work status off, session observation on: the one entry is the one that observes sessions only",
                   reply["ok"] and reply["status"] == status(False, True, "current")
                   and json.loads(self.settings.read_text())["extensions"][-1] == self.entry_of("lifecycle.ts"))
        source, mark = self.source_mark(), tap.mark()
        b.launch()
        self.bound(b)
        seen = self.observed(b)
        b.type("hello fixture")
        self.until("a run Deck is not told of", lambda: "agent_settled" in b.names(source), 20)
        time.sleep(1.0)
        card = tap.card(b.session)
        self.check("B01", "standalone: session observation alone reports a session and no run: the card has no word",
                   seen["id"] == self.started(b, source)["id"] and tap.changes(b.session, mark) == [] and card["agent"] is None
                   and card.get("pi_session") == "observed" and card["pi_signal"] == "unavailable"
                   and card["scheduler"]["pi"], card=card)
        # Deck restarts: nothing is restored; the running Pi says what its API says now, as a snapshot
        native = seen["id"]
        self.registry("stop", "--pid", str(tap.process.pid))
        self.until("the old backend to be gone", lambda: tap.process.poll() is not None, 10)
        time.sleep(1.5)
        tap = self.backend()
        self.check("B08", "a restarted Deck knows no session: nothing is read back from disk",
                   tap.now()["sessions"] == [])
        self.bound(b, 40)
        again = self.observed(b)
        self.check("B08", "the running Pi's observer reports the session it is in NOW, as a snapshot and never as a start",
                   again["id"] == native and again["last"] == "snapshot", last=again["last"])
        # off: the observation is gone at once, and the user's file is theirs again
        mark = tap.mark()
        reply = tap.ask("lifecycle", enable=False)
        gone = tap.first(lambda e: "now" in e and e["now"]["sessions"] == [] and e["now"]["leases"] == [], "the withdrawal", 5, mark)
        self.check("B08", "session observation turned off: every record is gone at once and Deck's entry with it",
                   reply["ok"] and reply["status"] == status(False, False, "absent") and bool(gone)
                   and tap.ended(b.session)[-1] == "capability-disabled"
                   and json.loads(self.settings.read_text())["extensions"] == json.loads(self.original_settings)["extensions"])
        b.quit()
        # the ids stayed where they may be: this run's private trace, Pi's own files, and nowhere else
        public = json.dumps(self.results) + json.dumps(self.facts) + self.settings.read_text()
        kept = [path for path in (self.root / "deck").rglob("*") if path.is_file()]
        leaked = [native for native in set(ids) if native in public
                  or any(native.encode() in path.read_bytes() for path in kept)]
        self.check("B08", "no native session id is in Deck's data directory, Pi's settings or this run's summary",
                   leaked == [] and len(set(ids)) >= 4, sessions_seen=len(set(ids)), leaked=len(leaked))

    # ---- follow-up 01: capabilities one by one, ids Deck does not keep, an exec in the observer's pid ----

    def switches(self, signal: bool, lifecycle: bool) -> dict:
        """Put the two Settings switches where a scenario needs them, through the Settings commands."""
        tap = self.tap
        now = tap.ask("status")
        if now["pi"] != signal:
            tap.ask("signal", enable=signal)
        if now["pi_lifecycle"] != lifecycle:
            tap.ask("lifecycle", enable=lifecycle)
        return self.settings_now()

    def settings_now(self) -> dict:
        reply = self.tap.ask("status")
        return {key: reply[key] for key in ("pi", "pi_lifecycle", "pi_installation")}

    def capabilities(self) -> None:
        """Each capability is taken from a running stream by itself: the other, the lease and the run go on."""
        tap, a, b = self.tap, self.a, self.b
        for pane in (a, b):
            pane.quit()
        self.check("F03", "both switches on before any Pi starts", self.switches(True, True) == status(True, True, "current"))
        for pane in (a, b):
            pane.launch()
            self.bound(pane)
            self.observed(pane)
        cards_of = lambda: {p.session: tap.card(p.session) for p in (a, b)}
        leases, ended = {}, {}

        def remember() -> None:
            leases.update({p.session: tap.lease(p.session)["lease"] for p in (a, b)})
            ended.update({p.session: len(tap.ended(p.session)) for p in (a, b)})

        same_leases = lambda: all((tap.lease(p.session) or {}).get("lease") == leases[p.session] for p in (a, b))
        no_ending = lambda: all(len(tap.ended(p.session)) == ended[p.session] for p in (a, b))
        refused = tap.drops("pi-capability-disabled")

        # ---- session observation off: A idle with an unread ending, B in the middle of a run
        self.one_run("F03", a, "a run before anything is turned off", "hello fixture")
        a_unread = dict(tap.now()["store"][a.session])
        b_source = self.source_mark()
        b.type("/fx-slow 6000")
        self.until("B working", lambda: tap.state(b.session) == "working", 10)
        b_open = dict(tap.now()["store"][b.session])
        remember()
        mark, asked = tap.mark(), time.time() * 1000
        reply = tap.ask("lifecycle", enable=False)
        forgotten = tap.first(lambda e: "now" in e and e["now"]["sessions"] == [], "the sessions to be forgotten", 5, mark)
        off = tap.mark()
        cards = cards_of()
        self.check("F03", "session observation off: both streams lose their session at once and nothing else; no stream ends",
                   reply["ok"] and reply["status"] == status(True, False, "current")
                   and forgotten["wall"] - asked < 1000 and same_leases() and no_ending()
                   and all("pi_session" not in card and card["pi_signal"] == "connected" for card in cards.values()),
                   milliseconds=round(forgotten["wall"] - asked), status=reply["status"])
        self.check("F03", "npm, idle: its unread ending is still there — the same episode, not viewed",
                   tap.now()["store"].get(a.session) == a_unread and cards[a.session]["agent"] == "turn-done"
                   and cards[a.session]["episode_viewed"] is False)
        self.check("F03", "standalone, in a run: the run is still the same working episode",
                   tap.now()["store"].get(b.session) == b_open and cards[b.session]["agent"] == "working")
        self.until("B's run to settle in Pi", lambda: "agent_settled" in b.names(b_source), 20)
        self.until("B's ending on the card", lambda: tap.state(b.session) == "turn-done", 10)
        self.check("F03", "standalone: the run that was open when its stream lost session observation ends on the same lease",
                   same_leases() and no_ending() and [c[0] for c in tap.changes(b.session, mark) if c] == ["turn-done"])
        for pane in (a, b):
            self.one_run("F03", pane, "a whole run after session observation was taken from its stream", "hello fixture")
        self.check("F03", "both are still on the lease they had, and nothing was refused for a capability",
                   same_leases() and no_ending() and tap.drops("pi-capability-disabled") == refused)
        # the observers stopped reading: a session change inside Pi names nothing, on its new stream either
        for pane in (a, b):
            old, source = tap.lease(pane.session)["lease"], self.source_mark()
            pane.type("/fx-new")
            self.until(f"{pane.session}: the new runtime's lease",
                       lambda: (tap.lease(pane.session) or {}).get("lease", old) != old, 30)
            self.until(f"{pane.session}: Pi's own new session",
                       lambda: any(e["event"] == "session_start" and e.get("reason") == "new" for e in pane.events(source)), 10)
        time.sleep(1.0)
        named = [event["now"]["sessions"] for event in tap.since(off) if "now" in event and event["now"]["sessions"]]
        cards = cards_of()
        self.check("F03", "a new session inside Pi: the new runtime's stream is bound for work status only and names no session",
                   named == [] and all("pi_session" not in card and card["pi_signal"] == "connected" for card in cards.values())
                   and self.settings_now() == status(True, False, "current") and tap.drops("pi-capability-disabled") == refused,
                   sessions_named_since=len(named))

        # ---- turned on again: nothing is given back to a running Pi
        reply = tap.ask("lifecycle", enable=True)
        time.sleep(1.5)
        self.check("F03", "session observation on again: Settings says the running Pis need a reload, and no session is observed",
                   reply["ok"] and reply["status"] == status(True, True, "needs-reload") and tap.now()["sessions"] == []
                   and all("pi_session" not in card for card in cards_of().values()), status=reply["status"])
        a.type("/reload")
        self.until("A's reloaded runtime to report its session", lambda: tap.session(a.session), 30)
        one = self.settings_now()["pi_installation"]
        b.type("/reload")
        self.until("B's reloaded runtime to report its session", lambda: tap.session(b.session), 30)
        both = self.settings_now()["pi_installation"]
        cards = cards_of()
        self.check("F03", "a reload gives it back, Pi by Pi: the note stays while one Pi has not reloaded",
                   one == "needs-reload" and both == "current"
                   and all(card.get("pi_session") == "observed" and card["pi_signal"] == "connected" for card in cards.values()),
                   after_one=one, after_both=both)

        # ---- work status off: A in the middle of a run, B idle with an unread ending
        self.one_run("F03", b, "a run before work status is turned off", "hello fixture")
        a_source = self.source_mark()
        a.type("/fx-slow 6000")
        self.until("A working", lambda: tap.state(a.session) == "working", 10)
        remember()
        sessions = {p.session: tap.session(p.session)["id"] for p in (a, b)}
        mark, asked = tap.mark(), time.time() * 1000
        reply = tap.ask("signal", enable=False)
        withdrawn = tap.first(lambda e: "now" in e and e["now"]["store"] == {}, "the words to be withdrawn", 5, mark)
        cards = cards_of()
        self.check("F03", "work status off: both streams lose their word and episode at once, and keep their lease and their session",
                   reply["ok"] and reply["status"] == status(False, True, "current") and withdrawn["wall"] - asked < 1000
                   and same_leases() and no_ending()
                   and tap.changes(a.session, mark) == [None] and tap.changes(b.session, mark) == [None]
                   and all(card["agent"] is None and card["episode"] is None and card["pi_signal"] == "unavailable"
                           and card.get("pi_session") == "observed" and card["scheduler"]["pi"] for card in cards.values())
                   and all((tap.session(p.session) or {}).get("id") == sessions[p.session] for p in (a, b)),
                   milliseconds=round(withdrawn["wall"] - asked), status=reply["status"])
        self.until("A's run to settle in Pi", lambda: "agent_settled" in a.names(a_source), 20)
        time.sleep(1.0)
        self.check("F03", "npm: the run that was open ends in Pi and writes nothing: no word, no refusal, the same lease",
                   tap.now()["store"] == {} and same_leases() and no_ending() and tap.drops("pi-capability-disabled") == refused)
        b_source = self.source_mark()
        b.type("hello fixture")
        self.until("B's run to settle in Pi", lambda: "agent_settled" in b.names(b_source), 20)
        time.sleep(1.0)
        self.check("F03", "standalone: a whole run after work status was taken: Deck is told nothing of it",
                   tap.now()["store"] == {} and same_leases() and no_ending())
        for pane in (a, b):
            self.replaced(pane, "/fx-new", "new", "F03", "a new session while only session observation is left", same_id=False)

        # ---- the other one off as well: nothing is left, and the streams end
        mark, asked = tap.mark(), time.time() * 1000
        reply = tap.ask("lifecycle", enable=False)
        over = tap.first(lambda e: "now" in e and e["now"]["leases"] == [] and e["now"]["sessions"] == [], "the streams to end", 5, mark)
        self.check("F03", "both off: every stream ends, for that reason",
                   reply["ok"] and reply["status"] == status(False, False, "absent") and over["wall"] - asked < 1000
                   and all(tap.ended(p.session)[-1] == "capability-disabled" for p in (a, b)))
        reply = tap.ask("signal", enable=True)
        time.sleep(3.0)
        self.check("F03", "on again after both were off: the observers do not come back by themselves, and Settings says so",
                   reply["status"] == status(True, False, "needs-reload") and tap.now()["leases"] == [] and tap.now()["streams"] == 0,
                   status=reply["status"])
        for pane in (a, b):
            pane.type("/reload")
            self.bound(pane)
        self.check("F03", "after their reload both are connected and nothing waits",
                   self.settings_now() == status(True, False, "current")
                   and all(card["pi_signal"] == "connected" for card in cards_of().values()))

        # ---- one switch after the other, quickly: every answer is what the running Pis can do
        remember()
        steps = (("lifecycle", True, status(True, True, "needs-reload")),
                 ("lifecycle", False, status(True, False, "current")),
                 ("lifecycle", True, status(True, True, "needs-reload")))
        answers = [tap.ask(op, enable=enable)["status"] for op, enable, _ in steps]
        kept = same_leases() and no_ending()
        self.check("F03", "session observation on, off, on: the running streams keep work status throughout, and the note follows",
                   answers == [expected for _, _, expected in steps] and kept and tap.now()["sessions"] == [], answers=answers)
        self.one_run("F03", a, "a run in the middle of the switching", "hello fixture")
        steps = (("signal", False, status(False, True, "needs-reload")), ("signal", True, status(True, True, "needs-reload")))
        answers = [tap.ask(op, enable=enable)["status"] for op, enable, _ in steps]
        time.sleep(1.0)
        self.check("F03", "work status off and on again: the streams, which loaded nothing else, ended and wait for a reload",
                   answers == [expected for _, _, expected in steps] and tap.now()["leases"] == [] and tap.now()["store"] == {}
                   and all(tap.ended(p.session)[-1] == "capability-disabled" for p in (a, b)), answers=answers)
        for pane in (a, b):
            pane.type("/reload")
            self.until(f"{pane.session}: the reloaded runtime's session", lambda: tap.session(pane.session), 30)
        cards = cards_of()
        self.check("F03", "after a reload each Pi does what the switches say: work status and its session",
                   self.settings_now() == status(True, True, "current")
                   and all(card["pi_signal"] == "connected" and card.get("pi_session") == "observed" for card in cards.values()))
        for pane in (a, b):
            self.one_run("F03", pane, "a run at the end", "hello fixture")
            pane.quit()
        self.check("F03", "both switches off again: the settings file holds the user's entries only",
                   self.switches(False, False) == status(False, False, "absent")
                   and json.loads(self.settings.read_text())["extensions"] == json.loads(self.original_settings)["extensions"])

    def unsupported_ids(self) -> None:
        """A session id Deck does not keep is said on the card — never cut short, never passed over — and runs go on."""
        tap, a, b = self.tap, self.a, self.b
        for pane in (a, b):
            pane.quit()
        self.check("B02", "both switches on", self.switches(True, True) == status(True, True, "current"))
        for length in (256, 257, 512, 513):
            supported = length <= 256
            for pane in (a, b):
                source = self.source_mark()
                # an in-memory session: Pi cannot name a session file after an id this long
                pane.launch(flags=("--no-session", "--session-id", "k" * length))
                lease = self.bound(pane)
                self.until(f"{pane.session}: what the stream said of its session",
                           lambda: tap.session(pane.session) or lease["lease"] in tap.now()["unsupported"], 20)
                named, card, kept = self.started(pane, source), tap.card(pane.session), tap.session(pane.session)
                self.one_run("B02", pane, f"a run beside a {length}-byte session id", "hello fixture")
                after = tap.card(pane.session)
                word = "observed" if supported else "unsupported"
                self.check("B02", f"{pane.runtime}: a {length}-byte session id is "
                           + ("observed whole" if supported else "said to be unsupported: not cut short, not passed over")
                           + ", and work status is not affected",
                           len(named["id"]) == length and card.get("pi_session") == word and after.get("pi_session") == word
                           and card["pi_signal"] == "connected" and after["agent"] == "turn-done"
                           and (kept is not None and len(kept["id"]) == length if supported else kept is None)
                           and (lease["lease"] in tap.now()["unsupported"]) == (not supported),
                           session_word=card.get("pi_session"), id_bytes_in_pi=len(named["id"]),
                           kept_bytes=len(kept["id"]) if kept else 0)
                if length == 257:
                    # another runtime of the same Pi, in a session whose id Deck keeps: a new lease observes again
                    old, source = lease["lease"], self.source_mark()
                    pane.type("/fx-new")
                    self.until(f"{pane.session}: the new runtime's observation",
                               lambda: (tap.lease(pane.session) or {}).get("lease", old) != old and tap.session(pane.session), 30)
                    again, fresh = tap.session(pane.session), self.started(pane, source)
                    self.check("B02", f"{pane.runtime}: a new runtime in a session Deck keeps: a new lease observes again",
                               again["id"] == fresh["id"] and len(fresh["id"]) <= 256 and again["lease"] != old
                               and tap.card(pane.session).get("pi_session") == "observed"
                               and old not in tap.now()["unsupported"] and tap.ended(pane.session)[-1] == "reset-new")
                pane.quit()
                self.check("B02", f"{pane.runtime}: {length} bytes: the quit ends the stream by its reset, with no refusal",
                           tap.ended(pane.session)[-1] == "reset-quit" and tap.now()["unsupported"] == [])
        refusals = {reason: count for reason, count in tap.now()["drops"].items() if reason in ("pi-bad-native-id", "pi-bad-lifecycle", "pi-bad-frame")}
        self.check("B02", "none of that was a refusal: no id was called malformed and no stream was ended for one",
                   refusals == {}, refusals=refusals)
        kept = [path for path in (self.root / "deck").rglob("*") if path.is_file()]
        self.check("B08", "no part of such an id is in Deck's data directory or in this run's summary",
                   not any(b"kkkkkkkk" in path.read_bytes() for path in kept)
                   and "kkkkkkkk" not in json.dumps(self.results) + json.dumps(self.facts))
        self.switches(False, False)

    def exec_peer(self) -> None:
        """A program that executes another image inside the observer's pid is not the observer any more."""
        tap, a, b = self.tap, self.a, self.b
        for pane in (a, b):
            pane.quit()
        self.check("A12", "work status on", self.switches(True, False) == status(True, False, "current"))
        observer, other, staged = self.rt / "exec-observer", self.rt / "exec-other", self.rt / "exec-staged"
        for path, tag in ((observer, "before"), (other, "after"), (staged, "staged")):
            subprocess.run(["/usr/bin/cc", "-O1", f'-DBUILD="{tag}"', "-o", str(path), str(self.fixture / "exec-peer.c")],
                           check=True, capture_output=True)
        self.facts["exec_peer"] = {"images_differ": len({sha256(observer), sha256(other), sha256(staged)}) == 3}
        marks = self.root / "tmp"
        server_pid = int(self.tm("display-message", "-p", "#{pid}"))
        line = lambda **body: json.dumps({"protocol": "pi-observer", "v": 1, "source": "pi", **body}, separators=(",", ":"))
        work, ping, done = line(seq=1, run=1, state="working"), line(seq=2), line(seq=3, run=1, state="turn-done")

        def identity(pid: int) -> tuple[str, str]:
            """(start time, program) of a pid, as `ps` reads them."""
            fields = subprocess.run(["/bin/ps", "-o", "lstart=,comm=", "-p", str(pid)], capture_output=True, text=True,
                                    env={"LC_ALL": "C"}).stdout.strip().split(None, 5)
            return (" ".join(fields[:5]), fields[5]) if len(fields) == 6 else ("", "")

        def start(pane: Pane, name: str, last: list[str]) -> Path:
            log = self.logs / f"exec-{name}.log"
            hello = line(socket=tap.ready["socket"], server_pid=server_pid,
                         pane=self.tm("display-message", "-p", "-t", pane.target, "#{pane_id}"), observerVersion=1,
                         piVersion="1.1.0", mode="tui", capabilities={"signal": True, "lifecycle": False})
            pane.type(shlex.join([str(observer), tap.ready["status_sock"], str(log), hello, work, "@fork", ping,
                                  f"@mark:{marks}/before-{name}", f"@wait:{marks}/go-{name}", *last]))
            pane.pid = self.until(f"{pane.session}: the peer in front of the shell", pane.child, 10)
            self.registry("add-process", "--pid", str(pane.pid))
            self.until(f"{name}: the peer's frames before it goes on", lambda: (marks / f"before-{name}").exists(), 15)
            return log

        def finish(pane: Pane, name: str) -> None:
            (marks / f"end-{name}").touch()
            self.until(f"{name}: the peer to end by itself", lambda: pane.gone() and pane.foreground() == "zsh", 20)
            pane.pid = None

        try:
            # A: the new image sends the run's ending on the connection it kept
            log = start(a, "a", [f"@exec:{other}", done, f"@mark:{marks}/after-a", f"@wait:{marks}/end-a"])
            before, said, card = identity(a.pid), log.read_text(), tap.card(a.session)
            self.check("A12", "a real program in the pane's foreground binds and reports a run; a fork and a child's exec "
                              "between two frames end nothing",
                       '"accepted":true' in said and '"ack":1' in said and '"ack":2' in said and card["agent"] == "working"
                       and card["pi_signal"] == "connected" and card["fg"] == "exec-observer" and tap.lease(a.session) is not None,
                       fg=card["fg"])
            mark = tap.mark()
            (marks / "go-a").touch()
            self.until("the answer the new image read", lambda: (marks / "after-a").exists(), 15)
            self.until("A's lease to end", lambda: tap.lease(a.session) is None, 10)
            after, said, card = identity(a.pid), log.read_text(), tap.card(a.session)
            self.check("A12", "the same pid and start time, another image: the frame it sends on the connection it kept is refused",
                       bool(before[0]) and before[0] == after[0] and Path(before[1]).name == "exec-observer"
                       and Path(after[1]).name == "exec-other" and '"refused":"peer-executed"' in said and '"ack":3' not in said
                       and tap.ended(a.session)[-1] == "peer-executed" and tap.drops("pi-peer-executed") == 1,
                       same_pid_and_start=before[0] == after[0], image_before=Path(before[1]).name, image_after=Path(after[1]).name)
            self.check("A12", "nothing of the old observer is shown for the program that runs now: no word, no coverage, no Pi hold",
                       card["agent"] is None and card["episode"] is None and "pi_signal" not in card and card["fg"] == "exec-other"
                       and card["scheduler"] == {"pi": False, "pi_signal": None, "pre_fire_pi": False}
                       and [change for change in tap.changes(a.session, mark) if change] == [], card=card)
            finish(a, "a")

            # B: the new image keeps the connection and says nothing; the poll notices without a frame
            log = start(b, "b", [f"@exec:{other}", f"@mark:{marks}/after-b", f"@wait:{marks}/end-b"])
            before = identity(b.pid)
            self.check("A12", "a second real program, in the other pane, bound the same way",
                       tap.card(b.session)["agent"] == "working" and tap.lease(b.session) is not None)
            (marks / "go-b").touch()
            self.until("B's new image", lambda: (marks / "after-b").exists(), 15)
            self.until("B's word and lease to be withdrawn by the poll", lambda: tap.state(b.session) is None
                       and tap.lease(b.session) is None, 10)
            after, card = identity(b.pid), tap.card(b.session)
            self.check("A12", "a silent exec is noticed without any frame: the lease ends and the card says nothing of a Pi",
                       before[0] == after[0] and Path(after[1]).name == "exec-other" and tap.ended(b.session)[-1] == "peer-executed"
                       and card["agent"] is None and "pi_signal" not in card and not card["scheduler"]["pi"]
                       and '"ack":3' not in log.read_text(), card=card)
            finish(b, "b")

            # C: the program's FILE is replaced on disk while it runs: that is no exec, and the instance goes on
            log = start(a, "c", [done, f"@mark:{marks}/after-c", f"@wait:{marks}/end-c"])
            before, lease, inode = identity(a.pid), tap.lease(a.session)["lease"], observer.stat().st_ino
            executed = tap.drops("pi-peer-executed")
            os.replace(staged, observer)
            (marks / "go-c").touch()
            self.until("the frame after the replacement", lambda: (marks / "after-c").exists(), 15)
            self.until("its ending on the card", lambda: tap.state(a.session) == "turn-done", 10)
            after, said, card = identity(a.pid), log.read_text(), tap.card(a.session)
            self.check("A12", "a file replaced on disk under the running program is not an exec: the same lease, its frame is written",
                       observer.stat().st_ino != inode and before == after and '"ack":3' in said and '"refused"' not in said
                       and (tap.lease(a.session) or {}).get("lease") == lease and card["agent"] == "turn-done"
                       and card["pi_signal"] == "connected" and tap.drops("pi-peer-executed") == executed,
                       new_inode=observer.stat().st_ino != inode, same_process=before == after, card=card)
            finish(a, "c")
            self.until("C's stream to end with its process", lambda: tap.lease(a.session) is None, 10)
            self.check("A12", "a program that ends is the end of its stream, by that word and no other",
                       tap.ended(a.session)[-1] == "eof" and tap.state(a.session) is None)
        finally:
            for name in ("a", "b", "c"):
                (marks / f"go-{name}").touch()
                (marks / f"end-{name}").touch()
            for pane in (a, b):
                if pane.pid is not None:
                    try:
                        self.until(f"{pane.session}: its peer to end", pane.gone, 70)
                    except Failed:
                        pass
                    pane.pid = None
        self.switches(False, False)

    SCENARIOS = ("settings_loop", "start_both", "boundaries", "viewed", "dialogs", "together", "inactive_pane", "reload",
                 "frozen", "restart", "disable", "not_loaded", "out_of_order", "endings", "native_sessions",
                 "capabilities", "unsupported_ids", "exec_peer")

    def all(self) -> None:
        only = set(self.args.only.split(",")) if self.args.only else None
        needed = {"settings_loop", "start_both"}
        for name in self.SCENARIOS:
            if only and name not in only and name not in needed:
                continue
            print(f"--- {name}", flush=True)
            getattr(self, name)()
        for number in range(1, self.taps + 1):
            words = set()
            for line in (self.logs / f"tap-{number}.jsonl").read_text().splitlines():
                event = json.loads(line)
                if "now" in event:
                    words.update(entry["state"] for entry in event["now"]["store"].values())
                words.update(s["agent"] for s in (event.get("poll") or {}).get("sessions", []) if s.get("agent"))
            self.check("A18", f"backend {number}: in its whole life no card ever showed a word but working and turn-done",
                       words <= {"working", "turn-done"}, words=sorted(words))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--test-binary", required=True)
    parser.add_argument("--standalone", required=True)
    parser.add_argument("--root")
    parser.add_argument("--only", help="comma-separated scenario names (the two setup scenarios always run)")
    run = Run(parser.parse_args())
    failure = None
    try:
        run.prepare()
        run.backend()
        run.server()
        run.all()
    except (Failed, subprocess.CalledProcessError, KeyError, IndexError, StopIteration, ValueError) as error:
        failure = f"{type(error).__name__}: {error}"
        print(f"STOPPED: {failure}", flush=True)
    finally:
        if run.tap:
            run.tap.quit()
        cleanup = run.registry("cleanup", check=False)
        left = subprocess.run(["/bin/ps", "-axo", "pid=,comm="], capture_output=True, text=True).stdout
        owned = [line.strip() for line in left.splitlines() if str(run.root) in line]
        summary = {
            "root": str(run.root),
            "facts": run.facts,
            "checks": run.results,
            "passed": sum(1 for r in run.results if r["ok"]),
            "failed": [f"{r['ac']} {r['check']}" for r in run.results if not r["ok"]],
            "stopped": failure,
            "cleanup": {"exit": cleanup.returncode, "processes_from_root": len(owned), "socket_left": run.socket.exists()},
        }
        (run.root / "result.json").write_text(json.dumps(summary, indent=1) + "\n")
        print(json.dumps({k: summary[k] for k in ("root", "passed", "failed", "stopped", "cleanup")}, indent=1), flush=True)
    return 0 if not failure and not summary["failed"] and cleanup.returncode == 0 and not owned else 1


if __name__ == "__main__":
    sys.exit(main())
