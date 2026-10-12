#!/usr/bin/env python3
"""hunt.py — TEST ONLY. Looks for the case in which a hold stops the holding thread, and has it observed.

    hunt.py <root> <node|bun> <phase> <api|-> <runs per process> <processes> <tag> [--until N] [KEY=VALUE ...]

Runs src/stall.cjs (probe.cjs with the witness of only2.c) in one short process after another, as measure.py
runs probe.cjs: two listeners of its own per process, everything started from <root> and registered in the
root's experiment registry before it may begin, an environment with a HOME and TMPDIR inside the root, and
stopped through that registry when nothing moves for STALL seconds. It ends when N stalls were observed
(default 3) or the processes are used up, and prints what it has: how many holds were made, how many of them
the witness had to end, whether the holding thread then went on by itself, and where it had stood.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

REGISTRY = Path("/Users/chenhan/c9r-io/deck/scripts/edr_runtime.py")
STALL = 6.0
LIMIT = 300.0


def registry(root: Path, action: str, *more: str) -> tuple[int, str]:
    done = subprocess.run([sys.executable, str(REGISTRY), "experiment", action, "--root", str(root), *more],
                          capture_output=True, text=True)
    return done.returncode, (done.stdout + done.stderr).strip()


def rows(path: Path) -> list[dict]:
    if not path.exists():
        return []
    out = []
    for line in path.read_text().splitlines():
        if line.strip():
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                out.append({"unreadable": line[:200]})
    return out


def attempt(root: Path, name: str, runtime: str, extra: dict[str, str]) -> dict:
    out = root / "out"
    if (out / f"{name}.jsonl").exists():
        raise SystemExit(f"{name} was already run: take another name")
    for needed in ("bin/listen", "bin/kid", "bin/only2.node", "src/stall.cjs"):
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
    program = [str(root / "bin/node"), str(root / "src/stall.cjs")]
    if runtime == "bun":
        env["BUN_BE_BUN"] = "1"
        program = [str(root / "bin/pi-standalone"), str(root / "src/stall.cjs")]
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
        for suffix in ("progress", "jsonl", "witness.jsonl"):
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
        runtime_process = start(f"{runtime} running stall.cjs", program, "runtime")
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


def main() -> int:
    args = [a for a in sys.argv[1:]]
    until = 3
    if "--until" in args:
        at = args.index("--until")
        until = int(args[at + 1])
        del args[at:at + 2]
    root, runtime, phase, api, count, processes, tag = Path(args[0]), args[1], args[2], args[3], args[4], int(args[5]), args[6]
    more = dict(item.split("=", 1) for item in args[7:])
    out = root / "out"
    total = {"runtime": runtime, "phase": phase, "api": None if api == "-" else api, "runs_per_process": int(count),
             "environment": more, "processes": 0, "holds": 0, "whole_holds": 0, "stalls_the_witness_ended": 0,
             "of_them_the_holder_went_on_by_itself": 0, "processes_stopped_because_nothing_moved": 0,
             "witness_lines": 0, "stalls": []}
    for number in range(1, processes + 1):
        if total["stalls_the_witness_ended"] >= until:
            break
        name = f"{tag}-{number}"
        extra = {"R1_PHASE": phase, "R1_COUNT": count, **more}
        if api != "-":
            extra["R1_API"] = api
        report = attempt(root, name, runtime, extra)
        got = rows(out / f"{name}.jsonl")
        seen = rows(out / f"{name}.witness.jsonl")
        total["processes"] += 1
        total["holds"] += sum(row.get("runs", 0) for row in got if "runs" in row)
        total["whole_holds"] += sum(row.get("whole_holds", 0) for row in got if "runs" in row)
        stalls = [row for row in got if row.get("stall")]
        total["witness_lines"] += len(seen)
        total["stalls_the_witness_ended"] += len(seen)
        # a `stall` row is written by the holding thread AFTER the hold: it exists only if that thread went on
        total["of_them_the_holder_went_on_by_itself"] += len(stalls)
        if report.get("stopped_because_nothing_moved"):
            total["processes_stopped_because_nothing_moved"] += 1
        for row in stalls:
            total["stalls"].append({"process": name, "run": row.get("i"), "steps_ms": row.get("steps"),
                                    "after": {k: v for k, v in row.items() if k.endswith("_the_rescue")}})
        mark = f" STALLS {len(seen)} (went on: {len(stalls)})" if seen else ""
        stopped = f" STOPPED {json.dumps(report['stopped_because_nothing_moved'])}" if report.get("stopped_because_nothing_moved") else ""
        print(f"{name}: exit {report.get('runtime_exit')} runs {sum(row.get('runs', 0) for row in got if 'runs' in row)}{mark}{stopped}", flush=True)
    (out / f"{tag}.hunt.json").write_text(json.dumps(total, indent=1) + "\n")
    print(json.dumps({k: v for k, v in total.items() if k != "stalls"}))
    for stall in total["stalls"]:
        print(json.dumps(stall))
    return 0


if __name__ == "__main__":
    sys.exit(main())
