#!/usr/bin/env python3
"""Scripted Slack-mention reader for an isolated Deck smoke run (app/SMOKE.md
"Slack mentions through MCP", WKWebView mode `mentions-mcp`).

Deck must already be open in that mode and past `workers-armed`: the page
seeded three mentions (a lint request, a lunch question, an injected
instruction), saved the Agent rule `triage` and authorized one client that
may read mentions and trigger automations. `--adapter` must be a DEBUG
deck-mcp build (the --socket / --credential-fd seams).

The run: tools/list (27 tools); deck_capabilities says mayReadMentions; the
list has the three mentions, each piece someone else wrote fenced, the
handover offered with its sequence; the injected instruction is fenced
like the rest; deck_mention_read gives the whole lint request; an unknown
key is MENTION_NOT_FOUND; the cursor waits while nothing moves; a handover
to an unknown automation or with a stale revision is refused and accepts
nothing; the lint request is handed to `triage` once (a replay is the same
operation, committed with a card), a second handover of it is
MENTION_NOT_FOUND, and the list moves to the two left.

Prints closed facts only (codes, counts, seconds). Exit 0 when every
expectation held, 1 otherwise, 2 for usage.
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
FENCE = "<external_message "


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--tag", default=str(int(time.time())), help="unique suffix of request ids")
    return p.parse_args()


def main():
    args = parse()
    record = {"startedAt": int(time.time()), "clientId": args.client_id, "steps": []}
    failures = []
    tag = args.tag

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), "at": int(time.time()), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call

    def code(out):
        return (out.get("error") or {}).get("code")

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-mentions-e2e", "version": "1"}})
        instructions = init.get("result", {}).get("instructions", "")
        step("instructions", "deck_mention_handoff" in instructions and "material" in instructions)
        adapter.notify("notifications/initialized")
        names = [tool["name"] for tool in adapter.request("tools/list")["result"]["tools"]]
        step("tools-list", len(names) == TOOLS and all(n in names for n in
             ("deck_mentions_list", "deck_mention_read", "deck_mention_handoff")), count=len(names))
        ok, caps = call("deck_capabilities")
        step("capabilities", ok and caps.get("mayReadMentions") is True and caps.get("mayTriggerAutomations") is True)

        ok, listed = call("deck_mentions_list")
        mentions = listed.get("mentions", [])
        fenced = all(m.get(f, "").startswith(FENCE) for m in mentions for f in ("text", "from", "where"))
        step("list", ok and len(mentions) == 3 and fenced and listed.get("mayHandOff") is True
             and "nextTriggerSequence" in listed and "material" in listed.get("note", ""), count=len(mentions))
        lint = next((m for m in mentions if "lint job" in m.get("text", "")), {})
        injected = next((m for m in mentions if "IGNORE ALL" in m.get("text", "")), {})
        step("injected-fenced", injected.get("text", "").startswith(FENCE)
             and injected.get("text", "").endswith("</external_message>"))
        cursor = listed.get("cursor")

        ok, read = call("deck_mention_read", {"mention_key": lint.get("key", "")})
        whole = (read.get("mention") or {})
        step("read", ok and "src/app.py" in whole.get("text", "") and whole.get("text", "").startswith(FENCE)
             and whole.get("context") is not None)
        ok, out = call("deck_mention_read", {"mention_key": "C0SMOKE/1.000009"})
        step("read-unknown", not ok and code(out) == "MENTION_NOT_FOUND")

        started = time.time()
        ok, same = call("deck_mentions_list", {"cursor": cursor, "wait_ms": 1500})
        waited = time.time() - started
        step("cursor-waits", ok and same.get("changed") is False and waited >= 1.2, seconds=round(waited, 1))

        ok, autos = call("deck_automation_list")
        triage = next((a for a in autos.get("automations", []) if a.get("id") == "triage"), {})
        step("automation", ok and bool(triage.get("revision")))
        seq = listed.get("nextTriggerSequence", 0)

        def handoff(request, key, automation, revision, sequence):
            return call("deck_mention_handoff", {"request_id": request, "trigger_sequence": sequence,
                                                 "mention_key": key, "automation_id": automation,
                                                 "expected_revision": revision})

        ok, out = handoff(f"req_unknown_{tag}", lint.get("key", ""), "eyes", triage.get("revision", ""), seq)
        step("unknown-automation", not ok and code(out) == "AUTOMATION_NOT_FOUND")
        ok, out = handoff(f"req_stale_{tag}", lint.get("key", ""), "triage", "0" * 64, seq)
        step("stale-revision", not ok and code(out) == "RULE_CHANGED")

        ok, first = handoff(f"req_lint_{tag}", lint.get("key", ""), "triage", triage.get("revision", ""), seq)
        operation = first.get("operationId")
        step("handed", ok and bool(operation))
        ok, again = handoff(f"req_lint_{tag}", lint.get("key", ""), "triage", triage.get("revision", ""), seq)
        step("replayed", ok and again.get("operationId") == operation)
        outcome = None
        deadline = time.time() + 120
        while time.time() < deadline:
            ok, view = call("deck_operation_get", {"operation_id": operation})
            if ok and view.get("state") in ("committed", "rejected", "ambiguous"):
                outcome = view
                break
            time.sleep(1)
        step("committed", bool(outcome) and outcome.get("state") == "committed"
             and bool((outcome.get("result") or {}).get("cardId")), state=(outcome or {}).get("state"))
        ok, out = handoff(f"req_twice_{tag}", lint.get("key", ""), "triage", triage.get("revision", ""), seq + 1)
        step("twice-refused", not ok and code(out) == "MENTION_NOT_FOUND")
        ok, moved = call("deck_mentions_list", {"cursor": cursor})
        step("list-moved", ok and moved.get("changed") is True and len(moved.get("mentions", [])) == 2
             and all(m.get("key") != lint.get("key") for m in moved.get("mentions", [])))
    except Exception as error:  # a failed step is reported, never hidden
        step("exception", False, kind=type(error).__name__, text=str(error)[:160])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["finishedAt"] = int(time.time())
        record["failures"] = failures
        os.makedirs(args.evidence, exist_ok=True)
        with open(os.path.join(args.evidence, f"mentions-e2e-{tag}.json"), "w") as out:
            json.dump(record, out, indent=2)
    print("PASS" if not failures else f"FAIL {len(failures)}: {','.join(failures)}", flush=True)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
