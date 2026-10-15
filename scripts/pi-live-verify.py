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
ENTRY = "Contents/Resources/pi/signal.ts"
CHECKS = ("pi-clean", "pi-enable", "pi-run-created", "pi-connected", "pi-hold", "pi-turn-done", "pi-attention",
          "pi-send-now", "pi-disabled", "pi-reload", "pi-session", "pi-shown", "pi-review", "pi-narrowed",
          "pi-unsupported", "done")
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
            for name in ("observer.ts", "signal.ts", "lifecycle.ts", "both.ts"))
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
        command = shlex.join(["pi", "--provider", "deck-fixture", "--model", "fixture"])
        (self.pi_fixture / "command").write_text(command + "\n")
        (self.pi_fixture / "dir").write_text(str(self.work) + "\n")
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
        return USER_SETTINGS.replace("FIXTURE", str(self.root / "fixture/fixture.ts"))

    def launch(self) -> None:
        env = {
            "HOME": str(self.home), "PATH": f"{self.root / 'bin'}:{self.root / 'rt'}:/usr/bin:/bin", "SHELL": "/bin/zsh",
            "LANG": "en_US.UTF-8", "TMPDIR": str(self.root / "tmp"), "TMUX_TMPDIR": str(self.root / "tt"),
            "PI_CODING_AGENT_DIR": str(self.agent), "PI_SKIP_VERSION_CHECK": "1", "PI_OFFLINE": "1",
            "PI_TELEMETRY": "0", "FIXTURE_LOG": str(self.source_log),
        }
        passed = [part for name, value in env.items() for part in ("--env", f"{name}={value}")]
        # the display stays awake for the run; this is the run's own process and ends with it
        awake = subprocess.Popen(["/usr/bin/caffeinate", "-d", "-u", "-t", "480"])
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
        both = json.dumps(self.entry.replace("signal.ts", "both.ts"))
        self.check("session observation on: Deck's one entry is replaced where it stood by the one that carries both",
                   at("pi-session")["settings"].decode(errors="replace") == before_off.replace(json.dumps(self.entry), both)
                   and at("pi-review")["settings"] == at("pi-session")["settings"])
        self.check("session observation off again: the same bytes as before it was turned on",
                   at("pi-narrowed")["settings"] == at("pi-reload")["settings"])
        wanted = ("[agent-pi] signal install ok", "[agent-pi] observer bound s=", "[agent-status] pi working s=",
                  "[agent-status] pi turn-done s=", "(capability-disabled)", "[agent-pi] signal remove ok",
                  "[agent-pi] lifecycle install ok", "signal=1 lifecycle=1", "[agent-pi] session start-reload s=",
                  "[agent-pi] lifecycle remove ok", "[agent-pi] observer narrowed s=",
                  "[agent-pi] session unsupported-native-id s=")
        self.check("app.log holds the production's closed lines for each step",
                   all(text in log for text in wanted), missing=[text for text in wanted if text not in log])
        # the lease that lost session observation is the one that reported the next run: between the
        # narrowing and that run's ending no observer was bound and no lease ended
        lines = log.splitlines()
        narrowed = next((n for n, line in enumerate(lines) if "[agent-pi] observer narrowed s=" in line and "lifecycle=0" in line), None)
        ending = next((n for n, line in enumerate(lines) if narrowed is not None and n > narrowed
                       and "[agent-status] pi turn-done s=" in line), None)
        between = lines[narrowed:ending] if narrowed is not None and ending is not None else []
        self.check("after session observation was taken from it, the SAME lease reported the next run",
                   ending is not None and any("[agent-status] pi working s=" in line for line in between)
                   and not any("[agent-pi] observer bound" in line or "[agent-pi] observer ended" in line for line in between),
                   lines_between=len(between))
        natives = sorted({item["id"] for item in self.source() if item.get("id")})
        leaks = [text for text in (self.marker, str(self.root), "fixture response", "hello fixture", "kkkkkkkk", *natives)
                 if text in log]
        self.check("and nothing of the prompt, the paths, Pi's answer or a native session id",
                   leaks == [] and len(natives) >= 1, leaked=len(leaks), sessions_seen=len(natives))
        self.check("the Pi with the 300-byte session id really had that id, by Pi's own log",
                   any(len(native) == 300 for native in natives), longest=max(map(len, natives), default=0))
        kept = [path for path in self.data.rglob("*") if path.is_file() and self.pi_fixture not in path.parents
                and path.suffix != ".png"]
        self.check("no native session id is in Deck's data directory",
                   not any(native.encode() in path.read_bytes() for native in natives for path in kept), files=len(kept))
        self.snapshots(13)

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
        try:
            self.launch()
            if self.mode == "pi-live":
                self.judge(self.watch())
            else:
                self.judge_layout(self.watch(360))
        except (RuntimeError, OSError, subprocess.CalledProcessError, KeyError, ValueError) as error:
            failure = f"{type(error).__name__}: {error}"
            print(f"STOPPED: {failure}", flush=True)
        self.register_new()
        cleanup = self.registry("cleanup", check=False)
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
    parser.add_argument("--mode", choices=("pi-live", "pi-layout"), default="pi-live")
    parser.add_argument("--runtime", choices=("npm", "standalone"))
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
    if args.mode == "pi-live":
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
