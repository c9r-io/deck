#!/usr/bin/env python3
"""Inventory and explicitly clean isolated Deck development tmux servers.

The default mode is read-only and exits 1 when a reviewed development/smoke
server remains. Cleanup is deliberately opt-in, validates the server metadata,
and refuses non-shell foreground processes unless the caller explicitly accepts
that those processes will be terminated.
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import signal
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


SHELLS = {"bash", "dash", "fish", "nu", "pwsh", "sh", "zsh"}
REPOSITORY = Path(__file__).resolve().parent.parent
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
    if bundle != "deck-dev.app" and not (
        bundle.startswith("deck-smoke") and bundle.endswith(".app")
    ):
        return None
    try:
        socket_at = process.argv.index("-L")
        socket = process.argv[socket_at + 1]
    except (ValueError, IndexError):
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
    if not (bundle.startswith("deck-smoke") and bundle.endswith(".app")):
        return None
    try:
        at = process.argv.index("--smoke-tmux-socket")
        socket = process.argv[at + 1]
        if socket.startswith("deck-smoke-"):
            return ManagedApp(process.pid, process.ppid, process.elapsed, socket)
        return None
    except (ValueError, IndexError):
        return None


def run(args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, text=True, capture_output=True, timeout=4, check=False)


def processes() -> list[Process]:
    result = run(["/bin/ps", "-axo", "pid=,ppid=,etime=,command="])
    if result.returncode != 0:
        raise RuntimeError("process inventory unavailable")
    return [parsed for line in result.stdout.splitlines() if (parsed := parse_ps_line(line))]


def inspect(process: Process, executable: str, socket: str) -> Server:
    metadata_result = run([executable, "-L", socket, "show", "-gqv", "@deck-server-metadata"])
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
            "-L",
            socket,
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
    seen: set[str] = set()
    for process in found_processes:
        candidate = tmux_candidate(process)
        if candidate is None:
            continue
        executable, socket = candidate
        if selected_socket is not None and socket != selected_socket:
            continue
        if socket in seen:
            continue
        found.append(inspect(process, executable, socket))
        seen.add(socket)
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
    stopped = run([server.executable, "-L", server.socket, "kill-server"])
    if stopped.returncode != 0 and "no server running" not in stopped.stderr.lower():
        raise RuntimeError(f"{server.socket}: kill-server failed")


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
