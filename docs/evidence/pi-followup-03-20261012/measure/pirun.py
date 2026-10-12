#!/usr/bin/env python3
"""pirun.py — TEST ONLY. One print-mode run of a REAL Pi that runs its own shell tool once.

    pirun.py <root> <name> <npm|standalone>

`npm`: the copy of Node in the root runs the Pi package installed on this machine (read, never written).
`standalone`: the official standalone Pi, copied into the root (pi-dist/). Pi's own configuration and sessions
are directories inside the root, its HOME and TMPDIR too, and nothing of the caller's environment reaches it.
The Pi process is registered in the root's experiment registry when it is started and is stopped through
that registry if it has not ended by itself after 60 seconds. What is measured is said in src/r1-ext.ts.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from measure import registry, rows  # noqa: E402

NPM_CLI = Path.home() / ".local/lib/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js"


def main() -> int:
    root, name, flavour = Path(sys.argv[1]), sys.argv[2], sys.argv[3]
    out = root / "out"
    if (out / f"{name}.pi.jsonl").exists():
        raise SystemExit(f"{name} was already run: take another name")
    home = root / "pi" / name
    for directory in (home / "agent", home / "sessions", out / "kids", root / "work"):
        directory.mkdir(parents=True, exist_ok=True)
    kid = out / "kids" / f"{name}-tool.json"
    program = [str(root / "bin/node"), str(NPM_CLI)] if flavour == "npm" else [str(root / "pi-dist/pi")]
    argv = [*program, "--no-skills", "--no-prompt-templates", "--no-themes", "-e", str(root / "src/r1-ext.ts"),
            "--provider", "deck-fixture", "--model", "fixture", "--session-dir", str(home / "sessions"), "-p", "go"]
    env = {"HOME": str(root / "home"), "TMPDIR": str(root / "tmp"), "PATH": "/usr/bin:/bin", "LANG": "en_US.UTF-8",
           "TERM": "xterm-256color", "PI_CODING_AGENT_DIR": str(home / "agent"), "PI_SKIP_VERSION_CHECK": "1",
           "PI_OFFLINE": "1", "PI_TELEMETRY": "0", "R1_ROOT": str(root), "R1_NAME": name,
           "R1_TOOL": f"'{root / 'bin/kid'}' '{kid}'"}
    report: dict = {"name": name, "flavour": flavour, "program": program,
                    "sha256": {Path(item).name: hashlib.sha256(Path(item).read_bytes()).hexdigest() for item in program}}
    report["sha256"]["r1-ext.ts"] = hashlib.sha256((root / "src/r1-ext.ts").read_bytes()).hexdigest()
    with open(out / f"{name}.pi.stdout", "wb") as so, open(out / f"{name}.pi.stderr", "wb") as se:
        process = subprocess.Popen(argv, cwd=root / "work", env=env, stdin=subprocess.DEVNULL, stdout=so, stderr=se)
    code, said = registry(root, "add-process", "--pid", str(process.pid))
    report["registered"] = {"pid": process.pid, "exit": code}
    if code != 0:
        process.kill()
        raise SystemExit(f"Pi could not be registered: {said}")
    began = time.monotonic()
    while process.poll() is None and time.monotonic() - began < 60:
        time.sleep(0.05)
    if process.poll() is None:
        code, _ = registry(root, "stop", "--pid", str(process.pid))
        report["stopped_after_60_seconds"] = {"registry_stop_exit": code}
        process.wait(timeout=20)
    report["wall_seconds"] = round(time.monotonic() - began, 2)
    report["exit"] = process.returncode
    report["tool_child"] = json.loads(kid.read_text()) if kid.exists() else None
    report["events"] = rows(out / f"{name}.pi.jsonl")
    (out / f"{name}.pi.run.json").write_text(json.dumps(report, indent=1) + "\n")
    print(json.dumps({key: report.get(key) for key in ("name", "exit", "wall_seconds", "registered", "stopped_after_60_seconds", "tool_child")}, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
