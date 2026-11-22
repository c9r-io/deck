#!/usr/bin/env python3
"""Scripted Agent leaving notes for the user, for an isolated Deck smoke run
(app/SMOKE.md "Agent notes through MCP", WKWebView mode `notes-mcp`).

Run it INSIDE a Deck card's pane (the operator types it into the plain-shell
card "Agent" the page opened after `notes-armed`): Deck tells which card a
call comes from by the adapter's process, so the script passes nothing
about itself. `--adapter` must be a DEBUG deck-mcp build (the --socket /
--credential-fd seams).

Phase `write`: tools/list carries both note tools; deck_capabilities says
mayLeaveNotes and names the card "Agent"; a note without a title is
INVALID_ARGUMENTS; notes q1 (acted, with text) and q2 are written, q2 is
revised under its key, q3 is written and withdrawn, each operation polled
to `committed`; deck_card_notes then lists q1 and q2 open (q2 revised), no
q3. Phase `read` (after the page handled q1 and answered q2): both are
`handled`, writing q1 again is NOTE_HANDLED, and a new note q4 commits.

Prints closed facts only. Exit 0 when every expectation held, 1 otherwise.
"""
import argparse
import importlib.util
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("agent_e2e", os.path.join(HERE, "mcp-agent-e2e.py"))
agent_e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(agent_e2e)
Adapter, PROTOCOL, TOOLS = agent_e2e.Adapter, agent_e2e.PROTOCOL, agent_e2e.TOOLS


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--phase", choices=("write", "read"), default="write")
    p.add_argument("--tag", default=str(int(time.time())))
    return p.parse_args()


def main():
    args = parse()
    record = {"startedAt": int(time.time()), "phase": args.phase, "steps": []}
    failures = []
    tag = args.tag
    os.makedirs(args.evidence, exist_ok=True)

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call
    code = lambda out: (out.get("error") or {}).get("code")

    def settled(answer):
        ok, out = answer
        operation = out.get("operationId")
        if not ok or not operation:
            return out.get("state") if ok else code(out)
        deadline = time.time() + 120
        while time.time() < deadline:
            done, view = call("deck_operation_get", {"operation_id": operation})
            if done and view.get("state") in ("committed", "rejected", "ambiguous"):
                return view.get("state") if view.get("state") == "committed" else f"{view.get('state')}:{view.get('code')}"
            time.sleep(1)
        return "timeout"

    def note(key, request, **fields):
        return settled(call("deck_card_note", {"request_id": f"{request}_{tag}", "key": key, **fields}))

    def notes():
        ok, out = call("deck_card_notes")
        return ok, {n["key"]: n for n in out.get("notes", [])}

    try:
        adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                       "clientInfo": {"name": "deck-notes-e2e", "version": "1"}})
        adapter.notify("notifications/initialized")
        if args.phase == "write":
            listed = adapter.request("tools/list")
            names = {t["name"] for t in listed.get("result", {}).get("tools", [])}
            step("tools-list", len(names) == TOOLS and {"deck_card_note", "deck_card_notes"} <= names, count=len(names))
        ok, caps = call("deck_capabilities")
        card = caps.get("orchestratorCard") or {}
        step("in-card", ok and caps.get("mayLeaveNotes") is True and card.get("title") == "Agent")
        if args.phase == "write":
            ok, bad = call("deck_card_note", {"request_id": f"bad_{tag}", "key": "q0"})
            step("needs-title", not ok and code(bad) == "INVALID_ARGUMENTS", code=code(bad))
            step("q1", note("q1", "q1", title="Which port should the dev server use?",
                            text="I went ahead with 8080 (the README's default). 9090 is free too.", acted=True) == "committed")
            step("q2", note("q2", "q2", title="Keep the old API?") == "committed")
            step("q2-revised", note("q2", "q2b", title="Keep the old v1 API for one release?",
                                    text="Removing it breaks two callers.") == "committed")
            step("q3", note("q3", "q3", title="Temporary question") == "committed")
            step("q3-withdrawn", note("q3", "q3w", withdraw=True) == "committed")
            ok, mine = notes()
            q1, q2 = mine.get("q1", {}), mine.get("q2", {})
            step("listed", ok and set(mine) == {"q1", "q2"} and q1.get("state") == "open" and q1.get("acted") is True
                 and q2.get("title") == "Keep the old v1 API for one release?" and q2.get("state") == "open",
                 keys=",".join(sorted(mine)))
        else:
            ok, mine = notes()
            step("handled", ok and mine.get("q1", {}).get("state") == "handled" and mine.get("q2", {}).get("state") == "handled",
                 states=",".join(f"{k}:{v.get('state')}" for k, v in sorted(mine.items())))
            ok, closed = call("deck_card_note", {"request_id": f"q1again_{tag}", "key": "q1", "title": "Again?"})
            step("closed-to-writer", not ok and code(closed) == "NOTE_HANDLED", code=code(closed))
            step("q4", note("q4", "q4", title="One more decision after your answers") == "committed")
    except Exception as error:  # noqa: BLE001 - a closed word, not the text
        step("exception", False, kind=type(error).__name__)
    finally:
        adapter.close()
        record["failures"] = failures
        with open(os.path.join(args.evidence, f"notes-{args.phase}-{tag}.json"), "w") as f:
            json.dump(record, f)
    print("NOTES-E2E", "PASS" if not failures else "FAIL", flush=True)
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
