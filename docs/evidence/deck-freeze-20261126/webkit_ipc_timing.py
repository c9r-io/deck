#!/usr/bin/env python3
"""Derive the incident's IPC and event-delivery timing from an exported
unified-log JSON (`log show --style json`), content-free.

usage: python3 -I webkit_ipc_timing.py <system-events.json> <ui-pid>

Prints (1) every WebKit URL-scheme task (= one Tauri IPC request) of the UI
process with its start time and duration, a missing end marked PENDING, and
(2) per-10-second counts of `runJavaScriptInFrameInScriptWorld` activities,
which is how a Tauri `emit` reaches the page."""
import collections, datetime, json, re, sys

events = json.load(open(sys.argv[1]))
pid = int(sys.argv[2])
ts = lambda s: datetime.datetime.strptime(s[:26], "%Y-%m-%d %H:%M:%S.%f")
start, rows = {}, []
for e in events:
    if e.get("processID") != pid:
        continue
    m = re.search(r"taskID=(\d+)\] WebURLSchemeTaskProxy::(startLoading|didComplete)", e.get("eventMessage", ""))
    if not m:
        continue
    t = ts(e["timestamp"])
    if m.group(2) == "startLoading":
        start[m.group(1)] = t
    elif m.group(1) in start:
        began = start.pop(m.group(1))
        rows.append((began, m.group(1), (t - began).total_seconds() * 1000))
for task, t in start.items():
    rows.append((t, task, None))
print("start(JST)        task      ms")
for t, task, ms in sorted(rows):
    print(t.strftime("%H:%M:%S.%f")[:12], task.rjust(9), "PENDING" if ms is None else f"{ms:7.1f}")
evals = collections.Counter(
    e["timestamp"][11:18] + "x"
    for e in events
    if e.get("processID") == pid
    and "runJavaScript" in e.get("eventMessage", "")
    and "Starting" in e.get("eventMessage", "")
)
print("\nrunJavaScript (Tauri emit) activities per 10 s:")
for k in sorted(evals):
    print(k, evals[k])
