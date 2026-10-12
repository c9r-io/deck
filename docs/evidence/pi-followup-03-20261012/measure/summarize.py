#!/usr/bin/env python3
"""summarize.py <root> <tag> — one table of a finished series (read only)."""
import json
import sys
from pathlib import Path

root, tag = Path(sys.argv[1]), sys.argv[2]
for runtime in ("node", "bun"):
    path = root / "out" / f"{tag}{runtime}.summary.json"
    if not path.exists():
        continue
    summary = json.loads(path.read_text())
    print(f"===== {runtime}")
    for item in summary["items"]:
        label = item["api"] or item["phase"]
        stopped = [a for a in item["attempts"] if a["stopped_because_nothing_moved"]]
        head = f"{item['phase']:11} {label:36} asked {item['asked']:4} reached {item['reached']:4} attempts {len(item['attempts'])} stopped {len(stopped)}"
        t = item["totals"]
        if item["phase"] == "connect":
            print(head, "|", {k: t.get(k) for k in ("runs", "nothing_waiting_before", "waiting_at_return", "peer_is_this_process", "proven", "whole_holds", "a_thread_came_or_went", "others_moved_runs", "others_charged_runs", "charged_user_ns_max", "charged_system_ns_max", "held_min", "held_max", "failed")})
        elif item["phase"] == "spawn":
            print(head, "|", {k: t.get(k) for k in ("runs", "child_ran_under_hold", "child_of_this_process", "pid_returned", "pid_as_returned", "born_with_stdio_only", "proven", "whole_holds", "a_thread_came_or_went", "others_moved_runs", "charged_user_ns_max", "failed")})
        else:
            print(head)
            for row in item["results"]:
                row = dict(row)
                row.pop("phase", None)
                if "threads" in row:
                    row["threads"] = [x["name"] for x in row["threads"]]
                if "born_with" in row:
                    row = {k: row[k] for k in ("case", "i", "child_of_this_process", "born_with", "fork_calls", "forks_by_this_thread", "forks_by_another_thread")}
                print("     ", json.dumps(row)[:1500])
        for a in stopped:
            print("      STOPPED:", a["name"], a["stopped_because_nothing_moved"])
