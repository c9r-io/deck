#!/usr/bin/env python3 -I
"""The thread ids of an isolated Codex home, by working directory: the
`session_meta` first line of every rollout under `<CODEX_HOME>/sessions`.
Prints `<cwd>\t<id>\t<file>` lines, newest file last per cwd. Reads the
isolated home it is given and nothing else; the ids are the experiment's own.

usage: ids.py <CODEX_HOME>
"""
import json
import os
import sys


def main(argv):
    if len(argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    root = os.path.join(argv[1], "sessions")
    rows = []
    for dirpath, _, files in os.walk(root):
        for name in files:
            if not name.endswith(".jsonl"):
                continue
            path = os.path.join(dirpath, name)
            try:
                with open(path, encoding="utf-8") as handle:
                    first = json.loads(handle.readline())
            except (OSError, ValueError):
                continue
            if first.get("type") != "session_meta":
                continue
            payload = first.get("payload") or {}
            rows.append((payload.get("cwd", ""), payload.get("id") or payload.get("session_id", ""), path))
    rows.sort(key=lambda row: (row[0], os.path.basename(row[2])))
    for cwd, thread, path in rows:
        print(f"{cwd}\t{thread}\t{path}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
