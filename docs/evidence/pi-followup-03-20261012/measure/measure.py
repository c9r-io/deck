#!/usr/bin/env python3
"""measure.py — TEST ONLY. The measurement of residual risk 1 (Pi follow-up 03), inside an experiment root.

    measure.py <root> series <node|bun> [TAG]    every item for one runtime, each until its count is reached;
                                                 TAG is put before every name, so that a second series keeps the first
    measure.py <root> one <name> <node|bun> KEY=VALUE ...   one process, for trying things out

One ATTEMPT is one process of the runtime running src/probe.cjs for one item (R1_PHASE, R1_API, R1_COUNT),
with two listeners of its own. Everything an attempt starts has its executable under <root> and is registered
in the root's experiment registry (scripts/edr_runtime.py) before anything is measured; what is left at the
end is stopped through that registry and through nothing else. The runtime gets an environment of its own: a
HOME and a TMPDIR inside the root, and nothing of the caller's.

probe.cjs holds every other thread of its process while it measures. A held thread may hold something the
measuring thread needs; then nothing moves any more. The attempt is stopped when its progress file and its
results have not changed for STALL seconds, what it had finished stays, and a new attempt is started for what
is left of the item. A stopped attempt is recorded as stopped, with the run it was about to make.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import sys
import time
from pathlib import Path

REGISTRY = Path("/Users/chenhan/c9r-io/deck/scripts/edr_runtime.py")
STALL = 6.0
LIMIT = 300.0
ATTEMPTS = 12

COMMON = [
    ("identity", None, 1),
    ("control", None, 3),
    ("connect", None, 300),
    ("spawn", "child_process.spawnSync", 40),
    ("spawn", "child_process.spawn", 40),
    ("spawn", "child_process.spawn pipes+detached", 40),
    ("spawn", "child_process.execFile", 40),
    ("spawn", "child_process.exec", 40),
    ("spawn", "child_process.execFileSync", 40),
    ("spawn", "child_process.execSync", 40),
    ("forkers", None, 5),
    ("worker", None, 100),
    ("race", None, 1000),
]
NODE_ONLY = [("race-fork", None, 1000)]
BUN_ONLY = [("spawn", "Bun.spawnSync", 40), ("spawn", "Bun.spawn", 40), ("editor-race", None, 1000)]
CHUNKED = ("connect", "spawn")


def registry(root: Path, action: str, *more: str) -> tuple[int, str]:
    done = subprocess.run([sys.executable, str(REGISTRY), "experiment", action, "--root", str(root), *more],
                          capture_output=True, text=True)
    return done.returncode, (done.stdout + done.stderr).strip()


def rows(path: Path) -> list[dict]:
    if not path.exists():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def attempt(root: Path, name: str, runtime: str, extra: dict[str, str]) -> dict:
    out = root / "out"
    if (out / f"{name}.jsonl").exists():
        raise SystemExit(f"{name} was already run: take another name")
    for needed in ("bin/listen", "bin/kid", "bin/only.node", "src/probe.cjs"):
        if not (root / needed).is_file():
            raise SystemExit(f"{root / needed} is missing")
    (root / "work").mkdir(exist_ok=True)
    for fifo in ("ctl", "ack"):
        path = root / fifo
        if path.exists():
            path.unlink()
        os.mkfifo(path, 0o600)
    for stale in ("s.sock", "a.sock"):
        if (root / stale).exists():
            (root / stale).unlink()

    env = {"HOME": str(root / "home"), "TMPDIR": str(root / "tmp"), "PATH": "/usr/bin:/bin", "LANG": "en_US.UTF-8",
           "R1_ROOT": str(root), "R1_NAME": name, **extra}
    program = [str(root / "bin/node"), str(root / "src/probe.cjs")]
    if runtime == "bun":
        # the runtime inside the official standalone Pi, asked to be the runtime and nothing else
        env["BUN_BE_BUN"] = "1"
        program = [str(root / "bin/pi-standalone"), str(root / "src/probe.cjs")]
    report: dict = {"name": name, "runtime": runtime, "item": extra, "registered": []}
    started: list[tuple[str, subprocess.Popen]] = []

    def start(label: str, argv: list[str], log: str) -> subprocess.Popen:
        with open(out / f"{name}.{log}.stdout", "wb") as so, open(out / f"{name}.{log}.stderr", "wb") as se:
            process = subprocess.Popen(argv, cwd=root / "work", env=env, stdin=subprocess.DEVNULL, stdout=so, stderr=se)
        code, said = registry(root, "add-process", "--pid", str(process.pid))
        report["registered"].append({"what": label, "pid": process.pid, "exit": code})
        if code != 0:
            process.kill()
            raise SystemExit(f"{label} could not be registered: {said}")
        started.append((label, process))
        return process

    def moved() -> tuple:
        seen = []
        for suffix in ("progress", "jsonl"):
            try:
                st = (out / f"{name}.{suffix}").stat()
                seen.append((st.st_size, st.st_mtime_ns))
            except FileNotFoundError:
                seen.append(None)
        return tuple(seen)

    try:
        start("listener (answers whether a connection waits)",
              [str(root / "bin/listen"), "probe", str(root / "s.sock"), str(root / "ctl"), str(root / "ack"),
               str(out / f"{name}.listen.jsonl")], "listen")
        start("listener (takes whatever comes)",
              [str(root / "bin/listen"), "all", str(root / "a.sock"), str(out / f"{name}.listen-all.jsonl")], "listen-all")
        until = time.monotonic() + 5
        while not ((root / "s.sock").exists() and (root / "a.sock").exists()):
            if time.monotonic() > until:
                raise SystemExit("the listeners did not come up")
            time.sleep(0.02)
        runtime_process = start(f"{runtime} running probe.cjs", program, "runtime")
        # registered: now it may begin
        (out / f"{name}.go").write_text("go\n")
        began = time.monotonic()
        last, last_at = moved(), began
        while runtime_process.poll() is None:
            time.sleep(0.05)
            now, seen = time.monotonic(), moved()
            if seen != last:
                last, last_at = seen, now
            elif now - last_at > STALL or now - began > LIMIT:
                try:
                    about = (out / f"{name}.progress").read_text().strip()
                except FileNotFoundError:
                    about = ""
                code, said = registry(root, "stop", "--pid", str(runtime_process.pid))
                report["stopped_because_nothing_moved"] = {"seconds_without_change": round(now - last_at, 1),
                                                           "was_about_to": about, "registry_stop_exit": code}
                runtime_process.wait(timeout=20)
        report["wall_seconds"] = round(time.monotonic() - began, 2)
        report["runtime_exit"] = runtime_process.returncode
    finally:
        # the answering listener is asked to end; whatever is left is stopped through the registry
        try:
            fd = os.open(root / "ctl", os.O_RDWR | os.O_NONBLOCK)
            os.write(fd, b"q\n")
            os.close(fd)
        except OSError:
            pass
        time.sleep(0.2)
        report["stopped_through_registry"] = []
        for label, process in started:
            if process.poll() is None:
                code, _ = registry(root, "stop", "--pid", str(process.pid))
                report["stopped_through_registry"].append({"what": label, "pid": process.pid, "exit": code})
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    report["stopped_through_registry"][-1]["still_running"] = True
        report["exits"] = {label: process.returncode for label, process in started}
    (out / f"{name}.run.json").write_text(json.dumps(report, indent=1) + "\n")
    return report


def add(total: dict, row: dict) -> None:
    """Sum one chunk into an item's totals: counts add up, a maximum stays a maximum, names are collected."""
    for key, value in row.items():
        if key in ("phase", "upto", "api", "odd", "done"):
            continue
        if key == "odd":
            continue
        if key.endswith("_max"):
            total[key] = max(total.get(key, 0), value)
        elif key.endswith("_min"):
            total[key] = min(total.get(key, value), value) if value is not None else total.get(key)
        elif key == "charged_names":
            total[key] = sorted(set(total.get(key, [])) | set(value))
        elif isinstance(value, bool):
            total[key] = total.get(key, True) and value
        elif isinstance(value, (int, float)):
            total[key] = total.get(key, 0) + value
    total.setdefault("odd", []).extend(row.get("odd", []))


def series(root: Path, runtime: str, tag: str = "") -> dict:
    out = root / "out"
    items = COMMON + (BUN_ONLY if runtime == "bun" else NODE_ONLY)
    summary: dict = {
        "runtime": runtime,
        "sha256": {item: hashlib.sha256((root / item).read_bytes()).hexdigest()
                   for item in ("bin/node", "bin/pi-standalone", "bin/only.node", "bin/kid", "bin/listen", "src/probe.cjs",
                                "src/only.c", "src/kid.c", "src/listen.c", "src/measure.py")},
        "items": [],
    }
    for phase, api, target in items:
        slug = re.sub(r"[^A-Za-z]+", "-", api or phase).strip("-")
        entry: dict = {"phase": phase, "api": api, "asked": target, "attempts": [], "totals": {}, "results": []}
        done = 0
        for number in range(1, ATTEMPTS + 1):
            if done >= target:
                break
            name = f"{tag}{runtime}-{slug}-{number}"
            extra = {"R1_PHASE": phase, "R1_COUNT": str(target - done)}
            if api:
                extra["R1_API"] = api
            report = attempt(root, name, runtime, extra)
            got = rows(out / f"{name}.jsonl")
            finished = any(row.get("done") for row in got)
            reported = 0
            for row in got:
                if row.get("done"):
                    continue
                if phase in CHUNKED:
                    add(entry["totals"], row)
                    reported += row.get("runs", 0)
                else:
                    entry["results"].append(row)
            if phase in CHUNKED:
                done += reported
            elif finished:
                done = target
            entry["attempts"].append({
                "name": name, "pid": report["registered"][-1]["pid"], "exit": report.get("runtime_exit"),
                "finished": finished, "runs_reported": reported, "wall_seconds": report.get("wall_seconds"),
                "stopped_because_nothing_moved": report.get("stopped_because_nothing_moved"),
            })
            print(f"{name}: exit {report.get('runtime_exit')} finished {finished} runs {reported} "
                  f"{'STOPPED ' + json.dumps(report['stopped_because_nothing_moved']) if report.get('stopped_because_nothing_moved') else ''}",
                  flush=True)
        entry["reached"] = done
        summary["items"].append(entry)
    (out / f"{tag}{runtime}.summary.json").write_text(json.dumps(summary, indent=1) + "\n")
    return summary


def main() -> int:
    root, mode = Path(sys.argv[1]), sys.argv[2]
    if mode == "series":
        series(root, sys.argv[3], sys.argv[4] if len(sys.argv) > 4 else "")
        return 0
    if mode == "one":
        extra = dict(item.split("=", 1) for item in sys.argv[5:])
        print(json.dumps(attempt(root, sys.argv[3], sys.argv[4], extra), indent=1))
        return 0
    raise SystemExit(__doc__)


if __name__ == "__main__":
    sys.exit(main())
