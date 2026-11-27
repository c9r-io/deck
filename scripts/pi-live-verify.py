#!/usr/bin/env python3
"""Operator of the two Pi WKWebView smokes: `pi-live` (Pi work status in the real
window, with a real Pi and the production backend) and `pi-layout` (what Pi's
rows, notes and reasons look like, from scripted backend answers).

The page (`app/ui/test/pi-live-smoke.mjs`) clicks the Settings switch, starts a
real interactive Pi through a clock automation, types into it, sends the held
row with Send now and turns the switch off while Pi runs. This script does
what the page cannot: it builds the experiment root, places this checkout's
debug carrier at `<root>/home/Applications/deck.app` (the Settings switch
writes Pi's settings only from a stable install, and under this relocated
home that is the ordinary rule, not an exemption), launches it through
LaunchServices, copies Pi's settings file and the Pi pane after every
checkpoint, judges those copies, runs `scripts/smoke-verdict`, and stops
everything it started through `edr_runtime.py experiment`.

Nothing of the account is read or written: home, Pi's agent directory,
sessions, working directory, tmux socket directory and Deck's data directory
are inside one new root under /private/tmp. The model is the local fixture
provider (`scripts/pi/fixture.ts`).

  cargo build --manifest-path app/src-tauri/Cargo.toml     # this checkout's debug build
  pi-live-verify.py --runtime npm|standalone [--standalone DIR] [--host SSH_ALIAS]
  pi-live-verify.py --mode pi-layout [--host SSH_ALIAS]
  pi-live-verify.py --mode pi-interject --runtime npm|standalone --model default [--host SSH_ALIAS]

`pi-interject` (`deck_worker_interject`): the page opens one orchestrator
card and this script types `scripts/mcp-interject-e2e.py --type pi` into it
(only a call from that card may interject); the client starts the Pi worker
on a long task and interjects while the model writes and while its program
runs (about 15 minutes; the display is kept awake that long).

`pi-layout` starts no Pi: the page paints the real dialogs from answers it
scripts itself, and this script only launches the carrier, waits for its
verdict and brings the window snapshots back. Both modes write, into
`result.json`, which build they ran: the commit, what differed from it on
disk, one hash over the sources (`scripts/pi_build_identity.py`), the hash of
every executable and observer file of the carrier, and the snapshots they
kept. A carrier that was built BEFORE the newest source file is refused: its
embedded frontend and backend would not be what the checkout holds.

The window is opened in front and takes keyboard focus while the run lasts
(about two minutes); a locked screen stops the page's timers and the run
with them. With `--host` the root is built here, moved as it is to the same
path on that machine (an Apple Silicon Mac with a console login; AGENTS.md,
"Mac mini test resource"), run there by this same file, and its logs are
fetched back; nothing of that machine's own Deck is read or changed, and its
identity is never printed. Exit 0: the verdict is green, every operator
check passed and the cleanup found nothing left. `<root>/logs/` is private
(pane captures, Pi's event log with native session ids) and is never
committed; `<root>/result.json` is the summary.
"""

from __future__ import annotations

import argparse
import hashlib
import select
import json
import os
import plistlib
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# the file is also run from standard input on another machine, where there is no checkout
REPO = Path(__file__).resolve().parent.parent if "__file__" in globals() and __file__ != "<stdin>" else Path("/nonexistent")
TAURI = REPO / "app/src-tauri"
FIXTURES = REPO / "scripts/pi"

# `pi-interject`: the worker's step 2, a 100 s program; its two marker files
# tell the scripted orchestrator which phase a line landed in
PHASE1 = """import time
open("p1-started", "w").close()
for i in range(10):
    print("phase1", i * 10, "s", flush=True)
    time.sleep(10)
open("p1-done", "w").close()
print("phase1 done", flush=True)
"""
ENTRY = "Contents/Resources/pi/signal.ts"
CHECKS = ("pi-clean", "pi-enable", "pi-run-created", "pi-connected", "pi-hold", "pi-turn-done", "pi-attention",
          "pi-send-now", "pi-disabled", "pi-reload", "pi-replies", "pi-shown", "pi-review", "pi-narrowed",
          "done")
# what decides the carrier's behaviour and is compiled or copied into it
BUILT_FROM = ("app/extensions", "scripts/extension_catalog.py",
              "app/src-tauri/src", "app/src-tauri/resources", "app/src-tauri/build.rs", "app/src-tauri/build",
              "app/src-tauri/Cargo.toml", "app/src-tauri/Cargo.lock", "app/src-tauri/tauri.conf.json", "app/ui")
# The user's own Pi settings: other keys, their own entries (one of them the
# fixture provider, an extension of "theirs"), Pi's two-space layout. Deck's
# entry is added by the real switch and by nothing in this file.
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
    "-builtin:tool-search",
    "FIXTURE"
  ]
}
"""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def newest_source() -> tuple[float, str]:
    """The newest file the carrier is built from, among those git tracks or would track.

    A debug build embeds the whole frontend, the smoke pages under `app/ui/test` included: they are sources of the
    carrier like any module. Only what never reaches the application is left out: Rust unit tests and node tests."""
    listed = subprocess.run(["/usr/bin/git", "-C", str(REPO), "ls-files", "-z", "--cached", "--others", "--exclude-standard",
                             "--", *BUILT_FROM], capture_output=True, check=True).stdout.decode().split("\0")
    newest = (0.0, "")
    for name in listed:
        path = REPO / name
        if name and path.is_file() and not name.endswith(("tests.rs", ".test.mjs")):
            newest = max(newest, (path.stat().st_mtime, name))
    return newest


class Run:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        given = getattr(args, "root", None)
        self.root = Path(given) if given else Path(os.path.realpath(tempfile.mkdtemp(prefix="deck-pi-live-", dir="/private/tmp")))
        if self.root.parent != Path("/private/tmp") or not self.root.name.startswith("deck-pi-live-"):
            raise SystemExit("the root is not an experiment root of this script")
        self.suffix = self.root.name.rsplit("-", 1)[1].lower()
        # the registry tool travels with the root: the other machine has no checkout
        self.runtime_tool = self.root / "tools/edr_runtime.py"
        self.socket_name = "deck-smoke-pi-" + re.sub(r"[^a-z0-9]", "x", self.suffix)
        self.home = self.root / "home"
        self.app = self.home / "Applications/deck.app"
        self.tmux = self.app / "Contents/MacOS/tmux"
        self.agent, self.data = self.root / "agent", self.root / "data"
        self.settings = self.agent / "settings.json"
        self.pi_fixture = self.data / "pi-fixture"
        self.source_log = self.pi_fixture / "source.jsonl"
        self.work, self.sessions, self.logs = self.root / "work", self.root / "sessions", self.root / "logs"
        self.socket = self.root / "tt" / f"tmux-{os.getuid()}" / self.socket_name
        self.entry = str(self.app / ENTRY)
        self.marker = "deck-pi-live-step-" + self.suffix
        self.mode = getattr(args, "mode", None) or "pi-live"
        # `pi-worker --model PROVIDER/MODEL`: a real model instead of the fixture, so the worker's agent can be asked
        # to call `deck_worker_report`; the choice travels with the root to the machine that runs it
        chosen = getattr(args, "model", None)
        self.model = chosen or ((self.root / "model").read_text().strip() if (self.root / "model").exists() else None)
        # `--choose PROVIDER/MODEL`: the type is plain `pi` listing that model, and the orchestrator's start picks it
        # (`deck_worker_start` `model`); the run then reads which model answered from Pi's own session files
        # `pi-interject --paragraphs N`: the story the worker writes first; long enough that a fast model still
        # writes it past the session gap (it travels with the root as well)
        length = getattr(args, "paragraphs", None)
        self.paragraphs = length or (int((self.root / "paragraphs").read_text()) if (self.root / "paragraphs").exists() else 200)
        picked = getattr(args, "choose", None)
        self.choose = picked or ((self.root / "choose").read_text().strip() if (self.root / "choose").exists() else None)
        self.evidence = self.data / "evidence"
        self.results: list[dict] = []
        self.registered: set[int] = set()
        self.facts: dict = {}

    def registry(self, *args: str, check: bool = True) -> subprocess.CompletedProcess:
        done = subprocess.run([sys.executable, str(self.runtime_tool), "experiment", *args, "--root", str(self.root)],
                              capture_output=True, text=True)
        if check and done.returncode != 0:
            raise RuntimeError(f"the registry refused {args[0]}: {done.stderr.strip()}")
        return done

    def check(self, name: str, ok, **detail) -> None:
        self.results.append({"check": name, "ok": bool(ok), "detail": detail})
        print(f"{'ok  ' if ok else 'FAIL'} {name}" + ("" if ok else f"  {json.dumps(detail)}"), flush=True)

    def sh(self, function: str, *arguments: Path) -> None:
        """One function of app/bundle.sh, as app/run.sh calls it."""
        done = subprocess.run(["sh", "-c", f'. "$0/app/bundle.sh" && {function} "$@"', str(REPO), *map(str, arguments)],
                              capture_output=True, text=True)
        if done.returncode != 0:
            raise RuntimeError(f"{function} failed: {done.stderr.strip() or done.stdout.strip()}")

    def prepare(self) -> None:
        args = self.args
        for directory in (self.home, self.agent, self.data, self.pi_fixture, self.root / "tt", self.socket.parent,
                          self.root / "tmp", self.logs):
            directory.mkdir(parents=True, mode=0o700)
        for directory in (self.work, self.sessions, self.root / "fixture", self.root / "rt", self.root / "bin",
                          self.root / "tools", self.app / "Contents/Resources"):
            directory.mkdir(parents=True)
        shutil.copyfile(REPO / "scripts/edr_runtime.py", self.runtime_tool)
        self.registry("init")
        (self.root / "mode").write_text(self.mode + "\n")
        # the build this run is of; a debug executable older than its sources is not this checkout's build
        sys.path.insert(0, str(REPO / "scripts"))
        import pi_build_identity
        built, (changed, newest) = (TAURI / "target/debug/deck-app").stat().st_mtime, newest_source()
        self.facts["build"] = {**pi_build_identity.identity(REPO), "built_after_every_source": built >= changed,
                               "command": [Path(sys.argv[0]).name, *sys.argv[1:]]}
        if built < changed:
            raise SystemExit(f"target/debug/deck-app is older than {newest}: build this checkout first")
        # the carrier: this checkout's debug build, assembled and checked by the functions app/run.sh uses
        self.sh("deck_bundle_precheck", REPO, TAURI)
        self.sh("deck_bundle_copy", TAURI, self.app)
        # Before anything is signed: every executable in the carrier is the build output it was copied from, byte
        # for byte. (Signing the bundle rewrites the main executable, so its hash afterwards is not the build's.)
        as_built = {}
        for path in sorted((self.app / "Contents/MacOS").iterdir()):
            # `<name>-<architecture>-apple-darwin`, and no longer name that merely begins the same
            sources = [TAURI / "target/debug/deck-app"] if path.name == "deck" else [
                built for built in sorted((TAURI / "binaries").glob(f"{path.name}-*-apple-darwin"))
                if "-" not in built.name[len(path.name) + 1:-len("-apple-darwin")]]
            if len(sources) != 1:
                raise RuntimeError(f"no single build output for the carrier's {path.name}")
            as_built[f"Contents/MacOS/{path.name}"] = file_sha256(sources[0])
        copied_as_built = all(file_sha256(self.app / inside) == digest for inside, digest in as_built.items())
        shutil.copyfile(TAURI / "icons/icon.icns", self.app / "Contents/Resources/deck.icns")
        version = json.loads((TAURI / "tauri.conf.json").read_text())["version"]
        with (self.app / "Contents/Info.plist").open("wb") as out:
            plistlib.dump({
                "CFBundleName": "deck smoke", "CFBundleDisplayName": "deck smoke", "CFBundleExecutable": "deck",
                "CFBundleIdentifier": "io.c9r.deck.smoke", "CFBundleVersion": version,
                "CFBundleShortVersionString": version, "CFBundleIconFile": "deck", "CFBundlePackageType": "APPL",
                "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "11.0",
            }, out)
        subprocess.run(["/usr/bin/codesign", "--force", "--sign", "-", "--entitlements", str(TAURI / "Entitlements.plist"),
                        str(self.app)], check=True, capture_output=True)
        self.sh("deck_bundle_postcheck", REPO, TAURI, self.app)
        # the carrier as it was assembled: every executable and every observer file, by its hash
        carried = sorted(path for part in ("Contents/MacOS", "Contents/Resources/pi") for path in (self.app / part).iterdir()
                         if path.is_file())
        files = {str(path.relative_to(self.app)): file_sha256(path) for path in carried}
        observer_as_source = all(
            file_sha256(self.app / "Contents/Resources/pi" / name) == file_sha256(TAURI / "resources/pi" / name)
            for name in ("observer.ts", "signal.ts", "reply.ts"))
        self.facts["carrier"] = {
            # what ran: the carrier's files after it was signed ad hoc
            "files": files,
            # the build outputs they were copied from, and whether the copies were those bytes before the signing
            "as_built_sha256": as_built,
            "executables_copied_byte_for_byte_before_signing": copied_as_built,
            "rewritten_by_the_signing": sorted(name for name, digest in as_built.items() if files[name] != digest),
            "observer_files_are_the_sources": observer_as_source,
            "version": version,
        }
        self.check("the carrier is this checkout's debug build: its executables byte for byte before the ad-hoc signing, "
                   "which rewrote the main executable only, and the observer's files as the sources have them",
                   copied_as_built and observer_as_source
                   and self.facts["carrier"]["rewritten_by_the_signing"] == ["Contents/MacOS/deck"],
                   rewritten=self.facts["carrier"]["rewritten_by_the_signing"])
        if self.mode == "pi-layout":
            return
        # Pi: one of the two runtimes, reached as `pi` the way an install puts it on PATH
        for name in ("fixture.ts",):
            shutil.copyfile(FIXTURES / name, self.root / "fixture" / name)
        if args.runtime == "npm":
            node = Path(os.path.realpath(shutil.which("node") or "/nonexistent"))
            package = Path(args.npm_package)
            if not node.is_file() or not (package / "dist/bundle/cli.js").is_file():
                raise SystemExit("no node or no npm install of Pi on this machine")
            shutil.copy2(node, self.root / "rt/node")
            # a copy of the install, read and never changed, laid out as npm lays it out: bin/pi -> the package's cli
            copy = self.root / "lib/node_modules/@earendil-works/pi-coding-agent"
            shutil.copytree(package, copy, symlinks=True)
            (self.root / "bin/pi").symlink_to(Path("../lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js"))
            self.facts["runtime"] = {"kind": "npm", "package_version": json.loads((package / "package.json").read_text())["version"],
                                     "cli_sha256": file_sha256(copy / "dist/bundle/cli.js"),
                                     "node_sha256": file_sha256(self.root / "rt/node")}
        else:
            shutil.copytree(Path(args.standalone), self.root / "rt/pi", symlinks=True)
            (self.root / "bin/pi").symlink_to(self.root / "rt/pi/pi")
            self.facts["runtime"] = {"kind": "standalone", "pi_sha256": file_sha256(self.root / "rt/pi/pi")}
        self.settings.write_text(self.user_settings())
        self.settings.chmod(0o600)
        # what a person would type: Pi by its name (a rule's command holds at most 200 characters)
        provider, model = self.model.split("/", 1) if self.model and self.model != "default" else ("deck-fixture", "fixture")
        # `default`: Pi by its name alone, on the running machine's own default model (`borrow_credential`)
        command = "pi" if self.model == "default" or self.choose else shlex.join(["pi", "--provider", provider, "--model", model])
        if self.choose:
            (self.root / "choose").write_text(self.choose + "\n")
            (self.pi_fixture / "models").write_text(self.choose + "\n")
        if self.model:
            (self.root / "model").write_text(self.model + "\n")
            # Deck's MCP server for the worker's agent: this carrier's adapter, given the one client's credential on
            # a descriptor (the smoke keeps it in the data directory, never the Keychain); only the report tool is
            # declared to the model
            wrapper = (f'f=$(ls {shlex.quote(str(self.evidence))}/mcp-credential-client_* | head -1); exec 3<"$f"; '
                       f'exec {shlex.quote(str(self.app / "Contents/MacOS/deck-mcp"))} --client-id "${{f##*mcp-credential-}}" '
                       f'--socket {shlex.quote(str(self.data / "mcp-control.sock"))} --credential-fd 3')
            (self.agent / "mcp.json").write_text(json.dumps({"mcpServers": {"deck": {
                "command": "/bin/sh", "args": ["-c", wrapper], "exposure": "hidden",
                "toolExposure": {"deck_worker_report": "direct"},
                "description": "Deck: report the result of this worker card to its orchestrator"}}}, indent=2) + "\n")
        (self.pi_fixture / "command").write_text(command + "\n")
        (self.pi_fixture / "dir").write_text(str(self.work) + "\n")
        if self.mode == "pi-interject":
            # the scripted orchestrator travels with the root; its worker's step 2 is a 100 s program
            for name in ("mcp-interject-e2e.py", "mcp-agent-e2e.py"):
                shutil.copyfile(REPO / "scripts" / name, self.root / "tools" / name)
            (self.work / "phase1.py").write_text(PHASE1)
            (self.root / "paragraphs").write_text(f"{self.paragraphs}\n")
        (self.pi_fixture / "marker").write_text(self.marker + "\n")

    def formal(self) -> dict:
        """What this machine's own Deck looks like from outside, as closed facts: nothing of it is read."""
        table = subprocess.run(["/bin/ps", "-axo", "pid=,lstart=,comm="], capture_output=True, text=True).stdout
        own = sorted(line.strip() for line in table.splitlines() if "/Applications/deck.app/" in line)
        deck = Path.home() / ".deck"
        # names only: a Deck that is in use writes its own files while this runs
        listing = sorted(entry.name for entry in deck.iterdir() if not entry.name.endswith((".tmp", ".writing"))) if deck.is_dir() else None
        return {"processes": sha256("\n".join(own).encode()), "count": len(own),
                "data": sha256(json.dumps(listing).encode()) if listing is not None else "absent"}

    def user_settings(self) -> str:
        settings = USER_SETTINGS.replace("FIXTURE", str(self.root / "fixture/fixture.ts"))
        # a real model's worker reaches Deck's report tool through Pi's own MCP support
        return settings.replace('    "-builtin:mcp",\n', "") if getattr(self, "model", None) else settings

    def borrow_credential(self) -> bool:
        """With a real model: the one provider entry of THIS machine's own Pi credentials, copied into the root's
        agent directory (0600) for the run and removed right after (`return_credential`). Nothing else is read."""
        provider = self.model.split("/", 1)[0]
        if self.model == "default":
            # the running machine's own default model: its provider's entry, and that default in the root's settings
            try:
                own = json.loads((Path.home() / ".pi/agent/settings.json").read_text())
            except (OSError, ValueError):
                return False
            provider, model = own.get("defaultProvider"), own.get("defaultModel")
            if not provider or not model:
                return False
            settings = json.loads(self.settings.read_text())
            settings.update({"defaultProvider": provider, "defaultModel": model})
            self.settings.write_text(json.dumps(settings, indent=2) + "\n")
            self.facts["model"] = f"{provider}/{model}"
        elif self.choose:
            # the type's own command is plain `pi`: the named model is its default
            settings = json.loads(self.settings.read_text())
            settings.update({"defaultProvider": provider, "defaultModel": self.model.split("/", 1)[1]})
            self.settings.write_text(json.dumps(settings, indent=2) + "\n")
            self.facts["model"] = self.model
        providers = {provider} | ({self.choose.split("/", 1)[0]} if self.choose else set())
        source = Path.home() / ".pi/agent/auth.json"
        try:
            own = json.loads(source.read_text())
            entries = {name: own.get(name) for name in providers}
        except (OSError, ValueError):
            entries = {}
        if not entries or not all(entries.values()):
            return False
        target = self.agent / "auth.json"
        target.write_text(json.dumps(entries))
        target.chmod(0o600)
        return True

    def models_answered(self) -> list[str]:
        """Which models answered in this root's Pi sessions, from Pi's own session files: provider/model of each
        assistant message, nothing else of them."""
        seen: set[str] = set()
        for path in list(self.agent.rglob("*.jsonl")) + list(self.sessions.rglob("*.jsonl")):
            for line in path.read_text(errors="replace").splitlines():
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                message = entry.get("message") if isinstance(entry, dict) else None
                if isinstance(message, dict) and message.get("role") == "assistant":
                    seen.add(f"{message.get('provider')}/{message.get('model')}")
        return sorted(seen)

    def return_credential(self) -> None:
        try:
            (self.agent / "auth.json").unlink()
        except FileNotFoundError:
            pass

    def launch(self) -> None:
        env = {
            "HOME": str(self.home), "PATH": f"{self.root / 'bin'}:{self.root / 'rt'}:/usr/bin:/bin", "SHELL": "/bin/zsh",
            "LANG": "en_US.UTF-8", "TMPDIR": str(self.root / "tmp"), "TMUX_TMPDIR": str(self.root / "tt"),
            "PI_CODING_AGENT_DIR": str(self.agent), "PI_SKIP_VERSION_CHECK": "1", "PI_OFFLINE": "1",
            "PI_TELEMETRY": "0", "FIXTURE_LOG": str(self.source_log),
        }
        if self.model:
            # a real model: Pi may fetch its model catalog (the root holds none), and nothing else differs
            env.pop("PI_OFFLINE")
        passed = [part for name, value in env.items() for part in ("--env", f"{name}={value}")]
        # the display stays awake for the run; this is the run's own process and ends with it
        awake = subprocess.Popen(["/usr/bin/caffeinate", "-d", "-u", "-t", "2400" if self.mode == "pi-interject" else "480"])
        self.registry("add-process", "--pid", str(awake.pid), check=False)
        time.sleep(2)
        subprocess.run(["/usr/bin/env", "-i", "/usr/bin/open", "-n", *passed, str(self.app), "--args",
                        "--smoke-data-dir", str(self.data), "--smoke-tmux-socket", self.socket_name,
                        "--smoke-wkwebview", self.mode], check=True)
        # isolated or stopped: within twenty seconds the carrier writes ITS log, inside the root
        end = time.monotonic() + 20
        while time.monotonic() < end and not (self.data / "app.log").exists():
            time.sleep(0.25)
        if not (self.data / "app.log").exists():
            self.register_new()
            raise RuntimeError("the carrier did not write its isolated log: stopped")

    def owned(self) -> list[tuple[int, str]]:
        table = subprocess.run(["/bin/ps", "-axo", "pid=,comm="], capture_output=True, text=True).stdout
        found = []
        for line in table.splitlines():
            fields = line.strip().split(None, 1)
            if len(fields) == 2 and str(self.root) in fields[1]:
                found.append((int(fields[0]), fields[1]))
        return found

    def register_new(self) -> None:
        """Register what the app started, once each: the app, its tmux server, Pi."""
        if self.socket.exists() and not getattr(self, "server", False):
            done = self.registry("add-server", "--socket", str(self.socket), "--tmux", str(self.tmux), check=False)
            self.server = done.returncode == 0
        pids = {pid for pid, program in self.owned() if "/Contents/MacOS/tmux" not in program}
        if self.source_log.exists():
            for line in self.source_log.read_text().splitlines():
                pids.add(json.loads(line)["pid"])
        for pid in pids - self.registered:
            if self.registry("add-process", "--pid", str(pid), check=False).returncode == 0:
                self.registered.add(pid)

    def log_checks(self) -> list[str]:
        log = self.data / "app.log"
        if not log.exists():
            return []
        return re.findall(r"\[ui\] smoke-check (\S+) a=", log.read_text(errors="replace"))

    def pane(self) -> str:
        try:
            sessions = subprocess.run([str(self.tmux), "-S", str(self.socket), "list-panes", "-a", "-F",
                                       "#{session_name} #{pane_current_command}"], capture_output=True, text=True).stdout
            target = next((line.split()[0] for line in sessions.splitlines() if line.split()[1:] and line.split()[1] != "zsh"), None)
            if not target:
                return ""
            return subprocess.run([str(self.tmux), "-S", str(self.socket), "capture-pane", "-p", "-J", "-t", target, "-S", "-300"],
                                  capture_output=True, text=True).stdout
        except (OSError, IndexError):
            return ""

    def watch(self, timeout: float = 420) -> dict:
        """Copy Pi's settings and the Pi pane after every checkpoint the page reports."""
        seen: dict[str, dict] = {}
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            self.register_new()
            if self.mode != "pi-live":
                # nothing outside the page is judged per checkpoint: only its end is waited for
                if "done" in self.log_checks():
                    seen["done"] = {}
                    break
                time.sleep(0.5)
                continue
            for name in self.log_checks():
                if name in CHECKS and name not in seen:
                    time.sleep(0.4)
                    settings = self.settings.read_bytes()
                    pane = self.pane()
                    (self.logs / f"settings-after-{name}.json").write_bytes(settings)
                    (self.logs / f"pane-after-{name}.txt").write_text(pane)
                    seen[name] = {"settings": settings, "pane": pane, "source": self.source()}
                    print(f"checkpoint {name}", flush=True)
            if "done" in seen:
                break
            time.sleep(0.25)
        return seen

    def source(self) -> list[dict]:
        if not self.source_log.exists():
            return []
        return [json.loads(line) for line in self.source_log.read_text().splitlines()]

    def judge(self, seen: dict) -> None:
        original = self.user_settings().encode()
        element = ",\n    " + json.dumps(self.entry)
        last = json.dumps(str(self.root / "fixture/fixture.ts"))
        with_entry = self.user_settings().replace(last, last + element).encode()
        self.check("every checkpoint was reached", all(name in seen for name in CHECKS), reached=sorted(seen))
        at = lambda name: seen.get(name, {"settings": b"", "pane": "", "source": []})
        self.check("before the switch: Pi's settings are the user's, byte for byte", at("pi-clean")["settings"] == original)
        self.check("after the switch: the user's bytes plus Deck's one entry", at("pi-enable")["settings"] == with_entry)
        # Pi writes its own file once it runs: what Deck may touch is still its one entry
        before_off = at("pi-send-now")["settings"].decode(errors="replace")
        self.check("after the switch was turned off: Deck's entry is gone and every other byte stayed",
                   before_off.count(element) == 1 and at("pi-disabled")["settings"].decode(errors="replace") == before_off.replace(element, ""),
                   pi_rewrote_the_file=at("pi-send-now")["settings"] != with_entry)
        self.check("turned on again: the same bytes as before it was turned off",
                   at("pi-reload")["settings"].decode(errors="replace") == before_off)
        counts = lambda name, event: sum(1 for item in at(name)["source"] if item["event"] == event)
        self.check("held: Deck's automatic row never reached Pi (no text in the pane, no provider request, no run)",
                   self.marker not in at("pi-hold")["pane"] and counts("pi-hold", "fixture_provider_request") == 0
                   and counts("pi-hold", "agent_start") == 0 and counts("pi-hold", "session_start") == 1)
        self.check("still not after two typed turns", self.marker not in at("pi-attention")["pane"]
                   and counts("pi-attention", "agent_settled") == 2)
        self.check("Send now delivered the row's text once", at("pi-send-now")["pane"].count(self.marker) == 1
                   and counts("pi-send-now", "agent_settled") == 3)
        log = (self.data / "app.log").read_text(errors="replace") if (self.data / "app.log").exists() else ""
        both = json.dumps(self.entry.replace("signal.ts", "reply.ts"))
        self.check("the final message on: Deck's one entry is replaced where it stood by the one that reads it",
                   at("pi-replies")["settings"].decode(errors="replace") == before_off.replace(json.dumps(self.entry), both)
                   and at("pi-review")["settings"] == at("pi-replies")["settings"])
        self.check("the final message off again: the same bytes as before it was turned on",
                   at("pi-narrowed")["settings"] == at("pi-reload")["settings"])
        wanted = ("[agent-pi] signal install ok", "[agent-pi] observer bound s=", "[agent-status] pi working s=",
                  "[agent-status] pi turn-done s=", "(capability-disabled)", "[agent-pi] signal remove ok",
                  "[agent-pi] reply install ok", "signal=1 reply=1", "[summary] kept as written",
                  "[agent-pi] reply remove ok", "[agent-pi] observer narrowed s=")
        self.check("app.log holds the production's closed lines for each step",
                   all(text in log for text in wanted), missing=[text for text in wanted if text not in log])
        # the lease that lost the final message is the one that reported the next run: between the
        # narrowing and that run's ending no observer was bound and no lease ended
        lines = log.splitlines()
        narrowed = next((n for n, line in enumerate(lines) if "[agent-pi] observer narrowed s=" in line and "reply=0" in line), None)
        ending = next((n for n, line in enumerate(lines) if narrowed is not None and n > narrowed
                       and "[agent-status] pi turn-done s=" in line), None)
        between = lines[narrowed:ending] if narrowed is not None and ending is not None else []
        self.check("after the final message was taken from it, the SAME lease reported the next run",
                   ending is not None and any("[agent-status] pi working s=" in line for line in between)
                   and not any("[agent-pi] observer bound" in line or "[agent-pi] observer ended" in line for line in between),
                   lines_between=len(between))
        natives = sorted({item["id"] for item in self.source() if item.get("id")})
        leaks = [text for text in (self.marker, str(self.root), "fixture response", "hello fixture", *natives)
                 if text in log]
        self.check("and nothing of the prompt, the paths, Pi's answer or a native session id",
                   leaks == [] and len(natives) >= 1, leaked=len(leaks), sessions_seen=len(natives))
        kept = [path for path in self.data.rglob("*") if path.is_file() and self.pi_fixture not in path.parents
                and path.suffix != ".png"]
        self.check("no native session id and nothing Pi said is in Deck's data directory",
                   not any(text.encode() in path.read_bytes() for text in (*natives, "fixture response") for path in kept),
                   files=len(kept))
        self.snapshots(9)

    def snapshots(self, least: int) -> None:
        """The window snapshots the page asked for: real PNG files of a window that is at least 1280 wide."""
        shots = sorted(self.evidence.glob("*.png")) if self.evidence.is_dir() else []
        sizes = set()
        for path in shots:
            head = path.read_bytes()[:24]
            if head[:8] == b"\x89PNG\r\n\x1a\n":
                sizes.add((int.from_bytes(head[16:20], "big"), int.from_bytes(head[20:24], "big")))
        self.facts["snapshots"] = {path.name: file_sha256(path) for path in shots}
        self.facts["snapshot_pixel_sizes"] = sorted(f"{w}x{h}" for w, h in sizes)
        self.check("the window snapshots were written: PNG files, each of the whole window",
                   len(shots) >= least and len(sizes) >= 1 and all(w >= 1280 and h >= 700 for w, h in sizes),
                   written=len(shots), sizes=self.facts["snapshot_pixel_sizes"])

    # ---------- `pi-worker`: the operator is the orchestrator (agent_pi/handover.rs) ----------

    def tmux_pi(self, *args: str) -> str:
        """A tmux command at the pane where Pi runs (the one pane whose program is not the shell)."""
        panes = subprocess.run([str(self.tmux), "-S", str(self.socket), "list-panes", "-a", "-F",
                                "#{pane_id} #{pane_current_command}"], capture_output=True, text=True).stdout
        target = next((line.split()[0] for line in panes.splitlines() if line.split()[1:] and line.split()[1] != "zsh"), None)
        if not target:
            raise RuntimeError("no Pi pane")
        return subprocess.run([str(self.tmux), "-S", str(self.socket), *args[:1], "-t", target, *args[1:]],
                              capture_output=True, text=True).stdout

    def events(self, name: str) -> list[dict]:
        return [item for item in self.source() if item["event"] == name]

    def orchestrate(self) -> dict:
        """Wait for `piw-armed`, then drive the worker through this carrier's own adapter: start it, then send a
        line while an extension dialog is open in Pi and one while a draft sits in Pi's editor."""
        facts: dict = {}
        end = time.monotonic() + 240
        while time.monotonic() < end and "piw-armed" not in self.log_checks():
            self.register_new()
            time.sleep(0.5)
        if "piw-armed" not in self.log_checks():
            raise RuntimeError("the page never armed the client")
        credentials = sorted(self.evidence.glob("mcp-credential-client_*"))
        if len(credentials) != 1:
            raise RuntimeError("not one client credential")
        client = credentials[0].name[len("mcp-credential-"):]
        fd = os.open(credentials[0], os.O_RDONLY)
        adapter = subprocess.Popen([str(self.app / "Contents/MacOS/deck-mcp"), "--client-id", client,
                                    "--socket", str(self.data / "mcp-control.sock"), "--credential-fd", str(fd)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, pass_fds=(fd,))
        os.close(fd)
        self.registry("add-process", "--pid", str(adapter.pid), check=False)
        buffer = b""
        ids = iter(range(1, 10_000))

        def request(method: str, params: dict | None = None, timeout: float = 20) -> dict:
            nonlocal buffer
            rid = next(ids)
            adapter.stdin.write((json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}}) + "\n").encode())
            adapter.stdin.flush()
            deadline = time.time() + timeout
            while True:
                while b"\n" in buffer:
                    line, buffer = buffer.split(b"\n", 1)
                    if line.strip() and json.loads(line).get("id") == rid:
                        return json.loads(line)
                ready, _, _ = select.select([adapter.stdout], [], [], max(0.0, deadline - time.time()))
                if not ready:
                    raise RuntimeError(f"no answer to {method}")
                chunk = os.read(adapter.stdout.fileno(), 65536)
                if not chunk:
                    raise RuntimeError("the adapter closed its output")
                buffer += chunk

        def call(name: str, arguments: dict | None = None) -> tuple[bool, dict]:
            result = request("tools/call", {"name": name, "arguments": arguments or {}})["result"]
            return not result.get("isError", False), result.get("structuredContent") or {}

        def turn_done_after(card: str, episode: int, seconds: float) -> int | None:
            deadline = time.time() + seconds
            while time.time() < deadline:
                ok, out = call("deck_worker_wait", {"cards": [{"card_id": card, "episode": episode}], "wait_ms": 5000})
                view = next((w for w in out.get("workers", []) if w.get("cardId") == card), None) if ok else None
                if view and view["agent"]["state"] == "turn-done" and view["agent"]["episode"] != episode:
                    return view["agent"]["episode"]
                self.register_new()
            return None

        try:
            request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                   "clientInfo": {"name": "deck-pi-worker", "version": "1"}})
            adapter.stdin.write((json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}) + "\n").encode())
            adapter.stdin.flush()
            ok, listed = call("deck_worker_list")
            sequence = listed.get("nextSendSequence", 0)
            facts["types"] = [t.get("type") for t in listed.get("workerTypes", [])]
            first, second, third = (f"{self.marker}-{n}" for n in ("one", "two", "three"))
            token = f"REPORT-{self.suffix}"
            if self.model:
                first = (f"{first}: call the deck_worker_report tool once with exactly this text: {token}. "
                         f"Then reply with exactly: DONE. Use no other tool.")
                second, third = (f"{line}: reply with exactly: OK. Use no tool." for line in (second, third))
            # a start names its directory: this client's first project's
            _, caps = call("deck_capabilities", {})
            root = ((caps.get("authorizedWorkspaces") or [{}])[0].get("roots") or [""])[0]
            start = {"request_id": f"pw_{self.suffix}", "send_sequence": sequence,
                     "type": "pi", "text": first, "title": "pi worker", "dir": root}
            if self.choose:
                # first a model the type does not list: refused, nothing started, no sequence taken
                ok_bad, refused = call("deck_worker_start", {**start, "request_id": f"pwx_{self.suffix}",
                                                             "model": self.choose + "-unlisted"})
                facts["unlisted"] = {"refused": not ok_bad or (refused.get("error") or {}).get("code") == "MODEL_NOT_LISTED",
                                     "code": (refused.get("error") or {}).get("code"),
                                     "models": [t.get("models") for t in listed.get("workerTypes", [])]}
                start["model"] = self.choose
            ok, started = call("deck_worker_start", start)
            facts["start_model"] = (started.get("result") or {}).get("model")
            sequence += 1
            card = None
            deadline = time.time() + 120
            while ok and time.time() < deadline and not card:
                _, got = call("deck_operation_get", {"operation_id": started.get("operationId")})
                card = (got.get("result") or {}).get("cardId") if got.get("state") == "committed" else None
                time.sleep(1 if not card else 0)
            facts["card"] = bool(card)
            if not card:
                return facts
            e1 = turn_done_after(card, 0, 300)
            if self.model:
                _, rows = call("deck_worker_list")
                row = next((w for w in rows.get("workers", []) if w.get("cardId") == card), {})
                ok_read, read = call("deck_worker_reports", {"card_id": card})
                facts["report"] = {"summary": (row.get("reports") or {}),
                                   "texts_match": ok_read and [r.get("text") for r in read.get("reports", [])] == [token],
                                   "count": len(read.get("reports", [])) if ok_read else None,
                                   "token_in_listing": token in json.dumps(rows)}
            facts["first"] = {"turn": e1 is not None,
                              "pane": self.tmux_pi("capture-pane", "-p", "-J", "-S", "-300").count(first.split(":")[0]),
                              "requests": len(self.events("fixture_provider_request"))}
            # a dialog of an extension of Pi's is open: the line must not answer it
            self.tmux_pi("send-keys", "-l", "/fx-confirm")
            self.tmux_pi("send-keys", "Enter")
            deadline = time.time() + 15
            while time.time() < deadline and not self.events("ui_prompt_start"):
                time.sleep(0.25)
            opened = bool(self.events("ui_prompt_start"))
            prompts_before = len(self.events("ui_prompt_start"))
            runs_before = len(self.events("agent_start"))
            ok2, _ = call("deck_worker_send", {"request_id": f"pw2_{self.suffix}", "send_sequence": sequence,
                                               "card_id": card, "after_episode": e1, "text": second})
            sequence += 1
            deadline = time.time() + 150
            while time.time() < deadline and len(self.events("agent_start")) == runs_before:
                self.register_new()
                time.sleep(1)
            ran_during = len(self.events("agent_start")) > runs_before
            facts["dialog"] = {"opened": opened and prompts_before >= 1, "sent": ok2, "ran_while_open": ran_during,
                               "answered_by_the_line": bool(self.events("ui_prompt_end")) or any(
                                   e.get("command") == "confirm" for e in self.events("fixture_command_done")),
                               "pane": self.tmux_pi("capture-pane", "-p", "-J", "-S", "-300")}
            self.tmux_pi("send-keys", "Escape")
            e2 = turn_done_after(card, e1, 240)
            facts["dialog"]["turn"] = e2 is not None
            facts["dialog"]["closed_by_escape"] = any(e.get("command") == "confirm" for e in self.events("fixture_command_done"))
            # the user's half-typed draft sits in Pi's editor: the line must leave it there
            draft = f"draft-{self.suffix}-unsent"
            self.tmux_pi("send-keys", "-l", draft)
            time.sleep(1)
            ok3, _ = call("deck_worker_send", {"request_id": f"pw3_{self.suffix}", "send_sequence": sequence,
                                               "card_id": card, "after_episode": e2 or e1, "text": third})
            e3 = turn_done_after(card, e2 or e1, 240)
            pane = self.tmux_pi("capture-pane", "-p", "-J", "-S", "-300")
            tail = [line for line in pane.splitlines() if line.strip()][-6:]
            facts["draft"] = {"sent": ok3, "turn": e3 is not None, "third": pane.count(third),
                              "draft_once": pane.count(draft) == 1, "draft_at_editor": any(draft in line for line in tail)}
            (self.logs / "pane-after-worker.txt").write_text(pane)
            if self.choose:
                _, rows = call("deck_worker_list")
                facts["listed_model"] = next((w.get("model") for w in rows.get("workers", []) if w.get("cardId") == card), None)
                facts["answered"] = self.models_answered()
            return facts
        finally:
            try:
                adapter.stdin.close()
                adapter.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                adapter.kill()
            end = time.monotonic() + 480
            while time.monotonic() < end and "done" not in self.log_checks():
                self.register_new()
                time.sleep(1)

    # ---------- `pi-interject`: the orchestrator runs in a card (deck_worker_interject) ----------

    def interject(self) -> dict:
        """Wait for `pii-armed`, type the scripted orchestrator into the one card's pane (Deck proves the calling card
        from the kernel, so only there may it interject), and wait for its evidence and the page's end."""
        end = time.monotonic() + 240
        while time.monotonic() < end and "pii-armed" not in self.log_checks():
            self.register_new()
            time.sleep(0.5)
        if "pii-armed" not in self.log_checks():
            raise RuntimeError("the page never armed the client")
        credentials = sorted(self.evidence.glob("mcp-credential-client_*"))
        if len(credentials) != 1:
            raise RuntimeError("not one client credential")
        client = credentials[0].name[len("mcp-credential-"):]
        panes = subprocess.run([str(self.tmux), "-S", str(self.socket), "list-panes", "-a", "-F", "#{pane_id}"],
                               capture_output=True, text=True).stdout.split()
        if len(panes) != 1:
            raise RuntimeError(f"{len(panes)} panes before the worker started, not the orchestrator card's one")
        evidence = self.evidence / "interject"
        line = shlex.join([sys.executable, str(self.root / "tools/mcp-interject-e2e.py"),
                           "--adapter", str(self.app / "Contents/MacOS/deck-mcp"), "--socket", str(self.data / "mcp-control.sock"),
                           "--client-id", client, "--credential-file", str(credentials[0]), "--evidence", str(evidence),
                           "--tag", self.suffix, "--type", "pi", "--no-dialog", "--paragraphs", str(self.paragraphs)])
        subprocess.run([str(self.tmux), "-S", str(self.socket), "send-keys", "-t", panes[0], "-l", line], check=True)
        subprocess.run([str(self.tmux), "-S", str(self.socket), "send-keys", "-t", panes[0], "Enter"], check=True)
        result = evidence / f"interject-e2e-{self.suffix}.json"
        end = time.monotonic() + 1500
        while time.monotonic() < end and not result.exists():
            self.register_new()
            time.sleep(2)
        time.sleep(1)
        for target, name in ((panes[0], "orchestrator"), (None, "worker")):
            text = (subprocess.run([str(self.tmux), "-S", str(self.socket), "capture-pane", "-p", "-J", "-t", target, "-S", "-400"],
                                   capture_output=True, text=True).stdout if target else self.pane())
            (self.logs / f"interject-pane-{name}.txt").write_text(text)
        end = time.monotonic() + 120
        while time.monotonic() < end and "done" not in self.log_checks():
            time.sleep(0.5)
        return json.loads(result.read_text()) if result.exists() else {}

    def judge_interject(self, record: dict) -> None:
        self.check("the scripted orchestrator wrote its evidence", bool(record))
        for step in record.get("steps", []):
            detail = {key: value for key, value in step.items() if key not in ("name", "ok", "at", "first_look")}
            self.check(f"orchestrator: {step['name']}", step["ok"], **detail)
        self.check("the page reached its end", "done" in self.log_checks())
        if self.model:
            self.facts["answered"] = self.models_answered()

    def judge_worker(self, facts: dict) -> None:
        log = (self.data / "app.log").read_text(errors="replace") if (self.data / "app.log").exists() else ""
        first, dialog, draft = facts.get("first") or {}, facts.get("dialog") or {}, facts.get("draft") or {}
        self.check("the client sees its one Pi worker type and the worker card was started", facts.get("types") == ["pi"]
                   and facts.get("card"), types=facts.get("types"))
        self.check("the first line reached Pi through its own message path: shown once, one turn"
                   + ("" if self.model else ", one fixture request"),
                   first.get("turn") and first.get("pane") >= 1 and (self.model or first.get("requests", 0) >= 1), first=first)
        if self.model:
            report = facts.get("report") or {}
            self.check("Pi called deck_worker_report itself: the card's row counts one report, and the text read back "
                       "is exactly the token", report.get("texts_match") and (report.get("summary") or {}).get("count") == 1
                       and not report.get("token_in_listing"), report=report)
        if self.choose:
            unlisted = facts.get("unlisted") or {}
            self.check("a model the type does not list starts nothing", unlisted.get("refused")
                       and unlisted.get("code") == "MODEL_NOT_LISTED" and unlisted.get("models") == [[self.choose]],
                       unlisted=unlisted)
            self.check("the card runs the model the orchestrator chose from the type's list: the start and the list "
                       "say so, and every answer in Pi's session came from it",
                       facts.get("start_model") == self.choose and facts.get("listed_model") == self.choose
                       and facts.get("answered") == [self.choose],
                       start=facts.get("start_model"), listed=facts.get("listed_model"), answered=facts.get("answered"),
                       default=self.facts.get("model"))
        self.check("with an extension dialog open, the line did not answer it",
                   dialog.get("opened") and dialog.get("sent") and not dialog.get("answered_by_the_line"),
                   dialog={k: v for k, v in dialog.items() if k != "pane"})
        self.check("the dialog was still Pi's to close, and the line's turn ended", dialog.get("closed_by_escape")
                   and dialog.get("turn"))
        self.check("with a draft in Pi's editor, the line went out and the draft stayed in the editor, unsent",
                   draft.get("sent") and draft.get("turn") and draft.get("third") == 1 and draft.get("draft_once")
                   and draft.get("draft_at_editor"), draft=draft)
        wanted = ("[pi] worker line handed to Pi", "[queue] sent to")
        self.check("app.log holds the closed hand-over lines", all(text in log for text in wanted),
                   handed=log.count("[pi] worker line handed to Pi"))
        self.check("no worker line was typed into the pane, and none was left uncertain",
                   "explicit resolution required" not in log)
        leaks = [text for text in (self.marker, str(self.root), "fixture response", f"REPORT-{self.suffix}") if text in log]
        self.check("nothing of a line, a path or Pi's answer is in app.log", leaks == [], leaked=len(leaks))
        self.facts["pi_worker"] = {"ran_while_dialog_open": dialog.get("ran_while_open")}
        self.snapshots(2)

    def judge_layout(self, seen: dict) -> None:
        log = (self.data / "app.log").read_text(errors="replace") if (self.data / "app.log").exists() else ""
        self.check("the page reached its end", "done" in seen)
        self.check("no Pi, Claude or Codex was started for it: the layout is painted from scripted answers",
                   "[agent-pi] observer bound" not in log and "[agent-status] pi " not in log
                   and not self.source_log.exists())
        self.check("nothing wrote Pi's settings: the file was never created",
                   not self.settings.exists())
        self.snapshots(60)

    def carry_out(self) -> dict:
        """Launch, watch, judge and clean up, on the machine the root is on. The summary, written to the root."""
        failure = None
        before = self.formal()
        borrowed = False
        try:
            if self.model:
                borrowed = self.borrow_credential()
                self.check("this machine's Pi has a credential for the model's provider", borrowed,
                           model=self.facts.get("model", self.model))
                if not borrowed:
                    raise RuntimeError("no credential for the model's provider on this machine")
            self.launch()
            if self.mode == "pi-live":
                self.judge(self.watch())
            elif self.mode == "pi-worker":
                self.judge_worker(self.orchestrate())
            elif self.mode == "pi-interject":
                self.judge_interject(self.interject())
            else:
                self.judge_layout(self.watch(360))
        except (RuntimeError, OSError, subprocess.CalledProcessError, KeyError, ValueError) as error:
            failure = f"{type(error).__name__}: {error}"
            print(f"STOPPED: {failure}", flush=True)
        self.register_new()
        cleanup = self.registry("cleanup", check=False)
        if self.model:
            self.return_credential()
            self.check("the borrowed credential is gone from the root", not (self.agent / "auth.json").exists(),
                       borrowed=borrowed)
        left = self.owned()
        after = self.formal()
        self.check("this machine's own Deck is as it was: the same processes, the same file names in its data directory",
                   before == after, processes=before["count"])
        summary = {
            "root": str(self.root), "mode": self.mode, "checks": self.results, "stopped": failure, "facts": self.facts,
            "cleanup": {"exit": cleanup.returncode, "processes_from_root": len(left), "socket_left": self.socket.exists()},
        }
        (self.root / "result.json").write_text(json.dumps(summary, indent=1) + "\n")
        return summary

    def verdict(self) -> None:
        done = subprocess.run([str(REPO / "scripts/smoke-verdict"), str(self.data), self.mode], capture_output=True, text=True)
        (self.logs / "verdict.txt").write_text(done.stdout + done.stderr)
        self.check(f"scripts/smoke-verdict {self.mode} is green", done.returncode == 0, exit=done.returncode)


def on_host(run: Run, host: str) -> dict:
    """The root as it is, on `host`, run there by this file; its logs come back. Nothing of `host` is printed."""
    quiet = {"stdout": subprocess.DEVNULL, "stderr": subprocess.DEVNULL}
    ssh = ["/usr/bin/ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host]
    free = subprocess.run([*ssh, f"test ! -e {shlex.quote(str(run.root))}"], **quiet)
    if free.returncode != 0:
        raise RuntimeError("the other machine cannot be reached, or holds this root already")
    pack = subprocess.Popen(["/usr/bin/tar", "-C", "/private/tmp", "-cf", "-", run.root.name], stdout=subprocess.PIPE)
    unpack = subprocess.run([*ssh, "/usr/bin/tar -C /private/tmp -xf -"], stdin=pack.stdout, **quiet)
    if pack.wait() != 0 or unpack.returncode != 0:
        raise RuntimeError("the root could not be moved to the other machine")
    source = Path(__file__).read_bytes()
    there = subprocess.run([*ssh, f"/usr/bin/python3 - --carry-out {shlex.quote(str(run.root))}"], input=source,
                           capture_output=True)
    print(there.stdout.decode(errors="replace"), end="", flush=True)
    # what the run wrote comes back over what was sent; the carrier and the runtimes stay there for the trash
    for part in ("logs", "data/app.log", "data/evidence", "data/pi-fixture/source.jsonl", "result.json", "experiment.json",
                 "agent/settings.json"):
        fetch = subprocess.run([*ssh, f"/usr/bin/tar -C {shlex.quote(str(run.root))} -cf - {part}"], capture_output=True)
        if fetch.returncode == 0:
            subprocess.run(["/usr/bin/tar", "-C", str(run.root), "-xf", "-"], input=fetch.stdout, **quiet)
    summary = json.loads((run.root / "result.json").read_text())
    # the root leaves the other machine's temporary directory for a uniquely named place in its Trash
    trashed = subprocess.run([*ssh, f"test -z \"$(/bin/ps -axo comm= | /usr/bin/grep -F {shlex.quote(str(run.root))})\" && "
                              f"/bin/mv {shlex.quote(str(run.root))} \"$HOME/.Trash/{run.root.name}-$(/bin/date +%s)\""], **quiet)
    summary["moved_to_trash_there"] = trashed.returncode == 0
    return summary


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--mode", choices=("pi-live", "pi-layout", "pi-worker", "pi-interject"), default="pi-live")
    parser.add_argument("--runtime", choices=("npm", "standalone"))
    parser.add_argument("--model", help="pi-worker only: PROVIDER/MODEL, or `default` for the running machine's own Pi "
                        "default; a real model whose credential entry is borrowed for the run, and the worker is asked "
                        "to call deck_worker_report")
    parser.add_argument("--choose", help="pi-worker with --model only: PROVIDER/MODEL the type lists and the start "
                        "picks (the type's command is then plain `pi`, its default the --model one)")
    parser.add_argument("--paragraphs", type=int, help="pi-interject only: the length of the worker's story (default 200)")
    parser.add_argument("--standalone", help="the unpacked official build (the directory that holds pi)")
    parser.add_argument("--npm-package", default=str(Path.home() / ".local/lib/node_modules/@earendil-works/pi-coding-agent"))
    parser.add_argument("--host", help="an SSH alias: run on that machine instead of this one")
    parser.add_argument("--carry-out", dest="root", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.root:
        # the other machine's half: the root is there already, and says which mode it was built for
        args.mode = (Path(args.root) / "mode").read_text().strip()
        summary = Run(args).carry_out()
        return 0 if not summary["stopped"] and summary["cleanup"]["exit"] == 0 else 1
    if args.choose and not (args.mode == "pi-worker" and args.model and "/" in args.choose):
        parser.error("--choose PROVIDER/MODEL is for --mode pi-worker with --model")
    if args.mode in ("pi-live", "pi-worker", "pi-interject"):
        if not args.runtime:
            parser.error("--runtime is needed")
        if args.runtime == "standalone" and not (args.standalone and (Path(args.standalone) / "pi").is_file()):
            parser.error("--runtime standalone needs --standalone DIR")
    run = Run(args)
    # what was checked here before the root left for another machine (there, the run starts its own list)
    prepared: list[dict] = []
    try:
        run.prepare()
        prepared = list(run.results) if args.host else []
        summary = on_host(run, args.host) if args.host else run.carry_out()
    except (RuntimeError, OSError, subprocess.CalledProcessError, KeyError, ValueError) as error:
        summary = {"root": str(run.root), "checks": [], "stopped": f"{type(error).__name__}: {error}",
                   "cleanup": {"exit": run.registry("cleanup", check=False).returncode, "processes_from_root": len(run.owned()),
                               "socket_left": run.socket.exists()}}
    run.results = prepared + list(summary["checks"])
    run.verdict()
    # what was known here, before the root left: the build and the carrier; what the run found: the snapshots
    facts = {**run.facts, **{key: value for key, value in (summary.get("facts") or {}).items() if key.startswith("snapshot")}}
    summary.update({"mode": run.mode, "runtime": args.runtime, "where": "another machine" if args.host else "this machine",
                    "facts": facts, "checks": run.results, "failed": [r["check"] for r in run.results if not r["ok"]]})
    (run.root / "result.json").write_text(json.dumps(summary, indent=1) + "\n")
    print(json.dumps({k: summary.get(k) for k in ("root", "mode", "runtime", "where", "failed", "stopped", "cleanup",
                                                   "moved_to_trash_there")}, indent=1), flush=True)
    clean = summary["cleanup"]["exit"] == 0 and not summary["cleanup"]["processes_from_root"]
    return 0 if not summary["stopped"] and not summary["failed"] and clean else 1


if __name__ == "__main__":
    sys.exit(main())
