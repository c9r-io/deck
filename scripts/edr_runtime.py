#!/usr/bin/env python3
"""Inventory and explicitly clean isolated Deck development tmux servers.

The default mode is read-only and exits 1 when a reviewed development/smoke
server remains. Cleanup is deliberately opt-in, validates the server metadata,
and refuses non-shell foreground processes unless the caller explicitly accepts
that those processes will be terminated.

A tmux server is NEVER addressed by its socket name here. `tmux -L <name>` is
resolved in the caller's `TMUX_TMPDIR`, so the same words reach a different
server from a different environment; a release build names its socket `deck`
wherever it runs. On 2026-10-11 an experiment's cleanup ran
`tmux -L deck kill-server` from a shell that did not carry the experiment's
socket directory and stopped the shell service of a formally installed Deck:
its sessions ended and could not be brought back. Every server this tool
stops is therefore reached by its FULL socket path, and only after the server
that answers on that path has said it is the process the caller means.

`experiment` is the entry for anything started outside this repository's
debug bundles (a release carrier, a relocated home, another machine):

  edr_runtime.py experiment init --root DIR
  edr_runtime.py experiment add-server --root DIR --socket PATH --tmux FILE
  edr_runtime.py experiment add-process --root DIR --pid N
  edr_runtime.py experiment show --root DIR
  edr_runtime.py experiment stop --root DIR --pid N
  edr_runtime.py experiment cleanup --root DIR

A run registers what it starts, when it starts it, in `DIR/experiment.json`:
for a server its full socket path (inside DIR), the pid the server itself
reports on that path, that process's start time and executable; for a
process its pid, start time and executable.

`stop` stops ONE registered resource and leaves the rest running (an
application between two launches on one server). A server the experiment
replaces on the same socket (a restart under test) is registered again; the
earlier entry then reads as gone.

`cleanup` stops only what is registered, and only after EVERY entry was
verified again: the root is a directory of this user under a temporary
parent; the socket is still inside it; the pid still is the same process
(start time and executable) or is gone; a server that answers on the socket
is that pid. Anything else (a missing field, another process under the pid, a
path that leaves the root, a socket, executable or root that belongs to an
installed Deck or to a person's home) refuses the WHOLE cleanup and stops
nothing: exit 3. "Gone" needs a process table that answered. A cleanup that
stopped everything registered still exits 1 while a process whose program is
a file inside DIR keeps running: it was never registered, so it is reported,
not stopped.

On another machine the same file is read from standard input:
`ssh host "python3 - experiment cleanup --root DIR" < scripts/edr_runtime.py`.
"""

from __future__ import annotations

import argparse
import json
import os
import pwd
import shlex
import signal
import stat
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path


SHELLS = {"bash", "dash", "fish", "nu", "pwsh", "sh", "zsh"}
# read from standard input on another machine (`python3 -`) there is no file
REPOSITORY = Path(globals().get("__file__", "scripts/edr_runtime.py")).resolve().parent.parent
DEBUG_BUNDLES = (REPOSITORY / "app" / "src-tauri" / "target" / "debug").resolve()


@dataclass(frozen=True)
class Process:
    pid: int
    ppid: int
    elapsed: str
    argv: tuple[str, ...]


@dataclass(frozen=True)
class Server:
    pid: int
    ppid: int
    elapsed: str
    executable: str
    socket: str
    source: str
    panes: tuple[tuple[str, str, str], ...]


@dataclass(frozen=True)
class ManagedApp:
    pid: int
    ppid: int
    elapsed: str
    socket: str


def parse_ps_line(line: str) -> Process | None:
    fields = line.strip().split(None, 3)
    if len(fields) != 4:
        return None
    try:
        argv = tuple(shlex.split(fields[3]))
        return Process(int(fields[0]), int(fields[1]), fields[2], argv)
    except (ValueError, IndexError):
        return None


CHANNEL_BUNDLE = "deck-channel-smoke-"
CHANNEL_SOCKET = "deck-smoke-channel-"


def smoke_bundle(bundle: str, socket: str) -> bool:
    """Whether `bundle` is a smoke carrier that owns `socket`.

    The channel first-send carrier (`app/run.sh`) has a bundle of its own per
    run, `deck-channel-smoke-<suffix>.app`, and owns exactly the socket
    `deck-smoke-channel-<suffix>`.
    """
    if not bundle.endswith(".app"):
        return False
    if bundle.startswith(CHANNEL_BUNDLE):
        suffix = bundle[len(CHANNEL_BUNDLE) : -len(".app")]
        return bool(suffix) and socket == CHANNEL_SOCKET + suffix
    return bundle.startswith("deck-smoke")


def tmux_candidate(process: Process) -> tuple[str, str] | None:
    if not process.argv:
        return None
    executable = process.argv[0]
    path = Path(executable)
    if not path.is_absolute():
        return None
    try:
        relative = path.resolve(strict=False).relative_to(DEBUG_BUNDLES)
    except ValueError:
        return None
    if len(relative.parts) != 4 or relative.parts[1:] != ("Contents", "MacOS", "tmux"):
        return None
    bundle = relative.parts[0]
    try:
        socket_at = process.argv.index("-L")
        socket = process.argv[socket_at + 1]
    except (ValueError, IndexError):
        return None
    if bundle != "deck-dev.app" and not smoke_bundle(bundle, socket):
        return None
    if socket == "deck-dev" or socket.startswith("deck-smoke-"):
        return executable, socket
    return None


def cleanup_allowed(socket: str, source: str) -> bool:
    return (socket.startswith("deck-smoke-") and source == "smoke") or (
        socket == "deck-dev" and source == "development"
    )


def managed_app(process: Process) -> ManagedApp | None:
    if not process.argv:
        return None
    executable = process.argv[0]
    path = Path(executable)
    if not path.is_absolute():
        return None
    try:
        relative = path.resolve(strict=False).relative_to(DEBUG_BUNDLES)
    except ValueError:
        return None
    if (
        len(relative.parts) != 4
        or relative.parts[1:3] != ("Contents", "MacOS")
        or relative.parts[3] not in {"deck", "deck-app"}
    ):
        return None
    bundle = relative.parts[0]
    if bundle == "deck-dev.app":
        return ManagedApp(process.pid, process.ppid, process.elapsed, "deck-dev")
    try:
        at = process.argv.index("--smoke-tmux-socket")
        socket = process.argv[at + 1]
    except (ValueError, IndexError):
        return None
    if smoke_bundle(bundle, socket) and socket.startswith("deck-smoke-"):
        return ManagedApp(process.pid, process.ppid, process.elapsed, socket)
    return None


def run(args: list[str]) -> subprocess.CompletedProcess[str]:
    # Without the caller's own tmux: this tool is often run from inside a Deck
    # card, where `TMUX` names the installed Deck's server. Every tmux command
    # here says `-S <path>`; one that ever forgot to would then reach the
    # caller's server. The C locale keeps `ps` start times one fixed shape.
    environment = {name: value for name, value in os.environ.items() if name not in ("TMUX", "TMUX_PANE")}
    environment["LC_ALL"] = "C"
    return subprocess.run(args, text=True, capture_output=True, timeout=4, check=False, env=environment)


def processes() -> list[Process]:
    result = run(["/bin/ps", "-axo", "pid=,ppid=,etime=,command="])
    if result.returncode != 0:
        raise RuntimeError("process inventory unavailable")
    return [parsed for line in result.stdout.splitlines() if (parsed := parse_ps_line(line))]


def default_socket(name: str) -> str:
    """Where `tmux -L name` looks from THIS environment: the full path."""
    directory = os.path.realpath(os.environ.get("TMUX_TMPDIR") or "/tmp")
    return f"{directory}/tmux-{os.getuid()}/{name}"


def answering_pid(tmux: str, socket_path: str) -> int | None:
    """The pid of the server that answers on `socket_path`, by its own word."""
    try:
        result = run([tmux, "-S", socket_path, "display-message", "-p", "#{pid}"])
    except (OSError, subprocess.TimeoutExpired):
        return None
    return int(result.stdout.strip()) if result.returncode == 0 and result.stdout.strip().isdigit() else None


def bound_socket(process: Process, executable: str, socket: str) -> str:
    """The full path of `socket`, proven to be the server `process` is."""
    path = default_socket(socket)
    answered = answering_pid(executable, path)
    if answered != process.pid:
        raise RuntimeError(
            f"{socket}: pid {process.pid} is not the server that answers on {path} "
            f"({'pid ' + str(answered) if answered else 'nothing'} does); nothing was stopped"
        )
    return path


def inspect(process: Process, executable: str, socket: str) -> Server:
    path = bound_socket(process, executable, socket)
    metadata_result = run([executable, "-S", path, "show", "-gqv", "@deck-server-metadata"])
    if metadata_result.returncode != 0:
        raise RuntimeError(f"{socket}: metadata unavailable")
    try:
        metadata = json.loads(metadata_result.stdout)
        source = metadata["source"]
    except (json.JSONDecodeError, KeyError, TypeError) as error:
        raise RuntimeError(f"{socket}: metadata invalid") from error
    if not cleanup_allowed(socket, source):
        raise RuntimeError(f"{socket}: metadata/source mismatch")
    panes_result = run(
        [
            executable,
            "-S",
            path,
            "list-panes",
            "-a",
            "-F",
            "#{session_name}|#{pane_pid}|#{pane_current_command}",
        ]
    )
    panes: list[tuple[str, str, str]] = []
    if panes_result.returncode == 0:
        for line in panes_result.stdout.splitlines():
            fields = line.split("|", 2)
            if len(fields) == 3:
                panes.append(tuple(fields))
    return Server(
        process.pid,
        process.ppid,
        process.elapsed,
        executable,
        socket,
        source,
        tuple(panes),
    )


def inventory(
    selected_socket: str | None = None,
) -> tuple[list[Process], list[Server], list[ManagedApp]]:
    found_processes = processes()
    found: list[Server] = []
    # a server and the clients attached to it have the same executable and
    # socket argument: the server is the one that answers on the socket's path
    by_socket: dict[str, list[tuple[Process, str]]] = {}
    for process in found_processes:
        candidate = tmux_candidate(process)
        if candidate is None:
            continue
        executable, socket = candidate
        if selected_socket is not None and socket != selected_socket:
            continue
        by_socket.setdefault(socket, []).append((process, executable))
    for socket, candidates in by_socket.items():
        answered = answering_pid(candidates[0][1], default_socket(socket))
        server = next((item for item in candidates if item[0].pid == answered), candidates[0])
        found.append(inspect(server[0], server[1], socket))
    apps = [
        app
        for process in found_processes
        if (app := managed_app(process)) is not None
        and (selected_socket is None or app.socket == selected_socket)
    ]
    return (
        found_processes,
        sorted(found, key=lambda server: server.socket),
        sorted(apps, key=lambda app: (app.socket, app.pid)),
    )


def cleanup(server: Server, include_foreground: bool) -> None:
    foreground = sorted(
        {command for _, _, command in server.panes if command and command not in SHELLS}
    )
    if foreground and not include_foreground:
        raise RuntimeError(
            f"{server.socket}: non-shell foreground processes present: {','.join(foreground)}"
        )
    # by full path, and only while that path still answers as this very process
    path = bound_socket(
        Process(server.pid, server.ppid, server.elapsed, ()), server.executable, server.socket
    )
    stopped = run([server.executable, "-S", path, "kill-server"])
    if stopped.returncode != 0 and "no server running" not in stopped.stderr.lower():
        raise RuntimeError(f"{server.socket}: kill-server failed")


# ---- experiments: registered resources, verified again before anything stops ----

REGISTRY = "experiment.json"


class Refused(Exception):
    """A registered resource is not provably this experiment's: nothing may be stopped."""


def account_home() -> Path:
    # the account's home by the account database: an experiment may relocate HOME
    return Path(os.path.realpath(pwd.getpwuid(os.getuid()).pw_dir))


def formal(path: Path) -> str | None:
    """Why `path` belongs to an installed Deck, the default tmux directory or a person's home."""
    home = account_home()
    owned = [Path("/Applications"), Path("/Library"), Path("/System"), home,
             Path(f"/private/tmp/tmux-{os.getuid()}")]
    for place in owned:
        if path == place or place in path.parents:
            return f"{path} is inside {place}"
    return None


def temporary_parents() -> list[Path]:
    return [Path("/private/tmp"), Path(os.path.realpath(tempfile.gettempdir()))]


def experiment_root(given: str) -> Path:
    root = Path(os.path.realpath(given))
    if not Path(given).is_absolute() or not root.is_dir():
        raise Refused(f"the experiment root {given} is not an absolute path to a directory")
    if not any(parent in root.parents for parent in temporary_parents()):
        raise Refused(f"the experiment root {root} is not a directory of its own under a temporary directory")
    reason = formal(root)
    if reason:
        raise Refused(f"the experiment root is not an experiment's: {reason}")
    if root.stat().st_uid != os.getuid():
        raise Refused(f"the experiment root {root} belongs to another user")
    return root


def inside(root: Path, given: str, what: str) -> Path:
    if not isinstance(given, str) or not given.startswith("/"):
        raise Refused(f"{what} is not an absolute path")
    path = Path(os.path.realpath(given))
    if root not in path.parents:
        raise Refused(f"{what} {given} leaves the experiment root {root}")
    reason = formal(path)
    if reason:
        raise Refused(f"{what} is not an experiment's: {reason}")
    return path


def identity(pid: int) -> tuple[str, str, str] | None:
    """(start time, executable, whole command) of `pid`; None once it is gone.
    A process table that does not answer is an error, never "gone"."""
    for _ in range(3):
        # two readings of one process: the executable (a path may hold spaces,
        # so it is a column of its own) and the whole command
        program = run(["/bin/ps", "-o", "pid=,lstart=,comm=", "-p", str(pid)]).stdout.strip().split(None, 6)
        whole = run(["/bin/ps", "-o", "pid=,lstart=,command=", "-p", str(pid)]).stdout.strip().split(None, 6)
        if not program and not whole:
            probe = run(["/bin/ps", "-o", "pid=", "-p", str(os.getpid())])
            if probe.returncode != 0 or probe.stdout.strip() != str(os.getpid()):
                raise RuntimeError("process inventory unavailable")
            return None
        if len(program) == 7 and len(whole) == 7 and program[0] == str(pid) and program[:6] == whole[:6]:
            return " ".join(program[1:6]), program[6], whole[6]
    raise RuntimeError(f"process inventory gave no steady answer about pid {pid}")


def not_formal_process(executable: str, command: str) -> None:
    # A program started by its bare name (`caffeinate`, `sleep`) shows no path. There is no place to judge
    # then, and resolving the name against this tool's own directory would invent one; its pid, start time
    # and name are still its identity.
    if executable.startswith("/"):
        reason = formal(Path(os.path.realpath(executable)))
        if reason:
            raise Refused(f"the process is not an experiment's: {reason}")
    if f"{account_home()}/.deck/" in command:
        raise Refused("the process names the account's own ~/.deck: it is an installed Deck's")


def read_registry(root: Path) -> dict:
    try:
        document = json.loads((root / REGISTRY).read_text())
    except (OSError, ValueError) as error:
        raise Refused(f"the registry of {root} cannot be read ({error})") from None
    if not isinstance(document, dict) or document.get("root") != str(root) or not isinstance(document.get("resources"), list):
        raise Refused(f"the registry of {root} is not this root's registry")
    return document


def write_registry(root: Path, document: dict) -> None:
    staged = root / (REGISTRY + ".writing")
    staged.write_text(json.dumps(document, indent=1, sort_keys=True) + "\n")
    os.replace(staged, root / REGISTRY)


def checked(root: Path, entry: object, registered: frozenset[tuple[str, int]] = frozenset()) -> tuple[dict, str]:
    """`entry` verified against the machine as it is now: (entry, "running" | "gone").
    Raises Refused when it is not provably the registered thing. `registered` holds
    (socket, pid) of every server entry: a server the experiment itself replaced on
    the same socket is a later entry, not a stranger."""
    if not isinstance(entry, dict) or entry.get("kind") not in ("tmux-server", "process"):
        raise Refused("a registry entry has no known kind")
    for field, kind in (("pid", int), ("started", str), ("executable", str)):
        if not isinstance(entry.get(field), kind) or isinstance(entry.get(field), bool) or not entry.get(field):
            raise Refused(f"a {entry['kind']} entry has no {field}")
    socket_path = None
    if entry["kind"] == "tmux-server":
        socket_path = inside(root, entry.get("socket"), "the socket")
        if not isinstance(entry.get("tmux"), str) or not os.access(entry["tmux"], os.X_OK):
            raise Refused("a tmux-server entry names no tmux program that can be run")
    now = identity(entry["pid"])
    if now is None:
        if socket_path is not None:
            answered = answering_pid(entry["tmux"], str(socket_path))
            if answered is not None and (str(socket_path), answered) not in registered:
                raise Refused(f"pid {answered}, which was never registered, answers on {socket_path}")
        return entry, "gone"
    started, executable, command = now
    if started != entry["started"] or executable != entry["executable"]:
        raise Refused(
            f"pid {entry['pid']} is another process now (started {started}, {executable}); "
            f"registered was {entry['started']}, {entry['executable']}"
        )
    not_formal_process(executable, command)
    if socket_path is not None:
        answered = answering_pid(entry["tmux"], str(socket_path))
        if answered is not None and answered != entry["pid"]:
            raise Refused(f"pid {answered} answers on {socket_path}, registered was pid {entry['pid']}")
    return entry, "running"


def servers(document: dict) -> frozenset[tuple[str, int]]:
    return frozenset(
        (entry["socket"], entry["pid"])
        for entry in document["resources"]
        if isinstance(entry, dict) and isinstance(entry.get("socket"), str) and type(entry.get("pid")) is int
    )


def wait_gone(entry: dict, seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        now = identity(entry["pid"])
        if now is None or now[0] != entry["started"]:
            return True
        time.sleep(0.2)
    return False


def stop(root: Path, entry: dict) -> None:
    """Stop one verified entry. Every signal is preceded by a fresh look at who the pid is."""
    def still_it() -> bool:
        now = identity(entry["pid"])
        return now is not None and now[0] == entry["started"] and now[1] == entry["executable"]

    if entry["kind"] == "tmux-server":
        socket_path = str(inside(root, entry["socket"], "the socket"))
        if answering_pid(entry["tmux"], socket_path) == entry["pid"]:
            run([entry["tmux"], "-S", socket_path, "kill-server"])
            wait_gone(entry, 5)
    for signum, patience in ((signal.SIGTERM, 5), (signal.SIGKILL, 3)):
        if not still_it():
            break
        os.kill(entry["pid"], signum)
        wait_gone(entry, patience)
    if still_it():
        raise RuntimeError(f"pid {entry['pid']} did not stop")
    if entry["kind"] == "tmux-server":
        socket_path = inside(root, entry["socket"], "the socket")
        if socket_path.is_socket() and answering_pid(entry["tmux"], str(socket_path)) is None:
            socket_path.unlink()


def running_from(root: Path) -> list[tuple[int, str]]:
    """Every process whose program is a file inside `root`, apart from this one."""
    table = run(["/bin/ps", "-axo", "pid=,comm="])
    if table.returncode != 0 or not table.stdout.strip():
        raise RuntimeError("process inventory unavailable")
    found = []
    for line in table.stdout.splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and fields[0].isdigit() and int(fields[0]) != os.getpid() and fields[1].startswith("/"):
            if root in Path(os.path.realpath(fields[1])).parents:
                found.append((int(fields[0]), fields[1]))
    return found


def experiment(args: argparse.Namespace) -> int:
    try:
        root = experiment_root(args.root)
        if args.action == "init":
            if (root / REGISTRY).exists():
                raise Refused(f"{root} has a registry already")
            write_registry(root, {"version": 1, "root": str(root), "resources": []})
            print(f"registry created: {root / REGISTRY}")
            return 0
        document = read_registry(root)
        if args.action in ("add-server", "add-process"):
            if args.action == "add-server":
                socket_path = inside(root, args.socket, "the socket")
                if not os.access(args.tmux, os.X_OK):
                    raise Refused(f"{args.tmux} cannot be run")
                pid = answering_pid(args.tmux, str(socket_path))
                if pid is None:
                    raise Refused(f"no server answers on {socket_path}")
                entry = {"kind": "tmux-server", "socket": str(socket_path), "tmux": os.path.realpath(args.tmux), "pid": pid}
            else:
                entry = {"kind": "process", "pid": args.pid}
            now = identity(entry["pid"])
            if now is None:
                raise Refused(f"pid {entry['pid']} is not running")
            entry["started"], entry["executable"] = now[0], now[1]
            not_formal_process(now[1], now[2])
            if any(known.get("pid") == entry["pid"] and known.get("started") == entry["started"]
                   for known in document["resources"] if isinstance(known, dict)):
                raise Refused(f"pid {entry['pid']} is registered already")
            document["resources"].append(entry)
            write_registry(root, document)
            print(f"registered {entry['kind']} pid {entry['pid']} started {entry['started']}")
            return 0
        if args.action == "stop":
            # one registered resource by its pid; whatever else the experiment runs stays up
            named = [entry for entry in document["resources"] if isinstance(entry, dict) and entry.get("pid") == args.pid]
            if len(named) != 1:
                raise Refused(f"pid {args.pid} is registered {len(named)} times; only a resource registered once is stopped")
            entry, state = checked(root, named[0], servers(document))
            print(f"{entry['kind']} pid {entry['pid']} started {entry['started']}: {state}")
            if state == "running":
                stop(root, entry)
                print(f"stopped {entry['kind']} pid {entry['pid']}")
            return 0
        # show and cleanup: every entry is verified before anything is done
        verdicts = [checked(root, entry, servers(document)) for entry in document["resources"]]
        for entry, state in verdicts:
            where = f" socket {entry['socket']}" if entry["kind"] == "tmux-server" else ""
            print(f"{entry['kind']} pid {entry['pid']} started {entry['started']}{where}: {state}")
        if args.action == "cleanup":
            # servers first: a client or an application that dies first can leave one orphaned
            for entry, state in sorted(verdicts, key=lambda item: item[0]["kind"] != "tmux-server"):
                if state == "running":
                    stop(root, entry)
                    print(f"stopped {entry['kind']} pid {entry['pid']}")
            # what was never registered is not stopped, and is not passed over in silence
            left = running_from(root)
            for pid, program in left:
                print(f"edr-runtime: STILL RUNNING from the root and never registered: pid {pid} {program}", file=sys.stderr)
            return 1 if left else 0
        return 0
    except Refused as refusal:
        print(f"edr-runtime: REFUSED, nothing was stopped: {refusal}", file=sys.stderr)
        return 3
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"edr-runtime: {error}", file=sys.stderr)
        return 2


def public(server: Server) -> dict[str, object]:
    return {
        "pid": server.pid,
        "ppid": server.ppid,
        "elapsed": server.elapsed,
        "socket": server.socket,
        "source": server.source,
        "paneCount": len(server.panes),
        "foreground": sorted({pane[2] for pane in server.panes if pane[2]}),
    }


def main() -> int:
    if sys.argv[1:2] == ["experiment"]:
        registry = argparse.ArgumentParser(prog="edr_runtime.py experiment")
        registry.add_argument("action", choices=("init", "add-server", "add-process", "show", "stop", "cleanup"))
        registry.add_argument("--root", required=True)
        registry.add_argument("--socket")
        registry.add_argument("--tmux")
        registry.add_argument("--pid", type=int)
        chosen = registry.parse_args(sys.argv[2:])
        if chosen.action == "add-server" and not (chosen.socket and chosen.tmux):
            registry.error("add-server needs --socket and --tmux")
        if chosen.action in ("add-process", "stop") and chosen.pid is None:
            registry.error(f"{chosen.action} needs --pid")
        return experiment(chosen)
    parser = argparse.ArgumentParser()
    parser.add_argument("--cleanup", action="store_true")
    parser.add_argument("--include-foreground", action="store_true")
    parser.add_argument("--socket")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()
    if args.socket is not None and not (
        args.socket == "deck-dev" or args.socket.startswith("deck-smoke-")
    ):
        parser.error("--socket must be deck-dev or start with deck-smoke-")
    try:
        _, servers, apps = inventory(args.socket)
        if args.cleanup:
            if not args.include_foreground:
                blocked = [
                    server.socket
                    for server in servers
                    if any(
                        command and command not in SHELLS
                        for _, _, command in server.panes
                    )
                ]
                if blocked:
                    raise RuntimeError(
                        "non-shell foreground processes present on: " + ",".join(blocked)
                    )
            for app in apps:
                os.kill(app.pid, signal.SIGTERM)
            if apps:
                time.sleep(0.5)
            for server in servers:
                cleanup(server, args.include_foreground)
            _, remaining_servers, remaining_apps = inventory(args.socket)
            if remaining_servers or remaining_apps:
                raise RuntimeError("reviewed app or server remained after cleanup")
        if args.json:
            print(
                json.dumps(
                    {
                        "apps": [
                            {
                                "pid": app.pid,
                                "ppid": app.ppid,
                                "elapsed": app.elapsed,
                                "socket": app.socket,
                            }
                            for app in apps
                        ],
                        "servers": [public(server) for server in servers],
                    },
                    sort_keys=True,
                )
            )
        else:
            for app in apps:
                print(
                    f"app {app.socket} pid={app.pid} ppid={app.ppid} age={app.elapsed}"
                )
            for server in servers:
                foreground = ",".join(public(server)["foreground"])
                print(
                    f"{server.socket} pid={server.pid} ppid={server.ppid} "
                    f"age={server.elapsed} panes={len(server.panes)} foreground={foreground or '-'}"
                )
        return 0 if args.cleanup or not (servers or apps) else 1
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"edr-runtime: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
