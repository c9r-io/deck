#!/usr/bin/env python3
"""Scripted orchestrator for an isolated Deck smoke run (app/SMOKE.md
"Orchestrator cards", WKWebView mode `orchestrators`).

Run it INSIDE a Deck card's pane (the operator types it into the plain-shell
cards "Orch A" and "Orch B" the page opened after `orchestrators-armed`):
Deck tells which card a call comes from by the adapter's process, so the
script passes nothing about itself. `--adapter` must be a DEBUG deck-mcp
build (the --socket / --credential-fd seams).

Phase `first` (both cards at once): deck_capabilities names this card
(its title gives the role A or B); the script starts one worker of the
type `claude` and hands one item to the Agent rule `worker`, each on this
card's own sequence; writes its two worker cards to the evidence dir and
waits for the other role's; then lists exactly its own two, is refused a
send into the other's (WORKER_NOT_FOUND), and queues one line on its own
typed worker. Phase `after` (in "Orch A", after the page closed "Orch B"
and handed one of its workers back): the list holds A's two and exactly one
of B's.

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
    p.add_argument("--phase", choices=("first", "after"), default="first")
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
    role = "?"

    def committed(operation):
        deadline = time.time() + 180
        while time.time() < deadline:
            ok, view = call("deck_operation_get", {"operation_id": operation})
            if ok and view.get("state") in ("committed", "rejected", "ambiguous"):
                return view
            time.sleep(1)
        return {}

    def listed():
        ok, out = call("deck_worker_list")
        return ok, out, {w["cardId"]: w for w in out.get("workers", [])}

    def cards_of(other):
        path = os.path.join(args.evidence, f"orchestrator-{other}-{tag}.json")
        deadline = time.time() + 300
        while time.time() < deadline:
            if os.path.exists(path):
                with open(path) as f:
                    return json.load(f)
            time.sleep(1)
        return None

    try:
        adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                       "clientInfo": {"name": "deck-orchestrators-e2e", "version": "1"}})
        adapter.notify("notifications/initialized")
        ok, caps = call("deck_capabilities")
        card = caps.get("orchestratorCard") or {}
        title = card.get("title", "")
        role = title[-1:] if title in ("Orch A", "Orch B") else "?"
        record["role"] = role
        step("orchestrator-card", ok and role in ("A", "B") and bool(card.get("cardId")), role=role)
        if role == "?":
            raise RuntimeError("no orchestrator card")
        other = "B" if role == "A" else "A"
        if args.phase == "after":
            mine = cards_of(role)
            theirs = cards_of(other)
            ok, out, workers = listed()
            from_other = [c for c in theirs["cards"] if c in workers]
            step("after-list", ok and all(c in workers for c in mine["cards"]) and len(from_other) == 1,
                 own=sum(c in workers for c in mine["cards"]), other=len(from_other))
            raise SystemExit
        ok, out, _ = listed()
        send_seq = out.get("nextSendSequence", 0)
        ok_a, autos = call("deck_automation_list")
        auto = next((a for a in autos.get("automations", []) if a.get("id") == "worker"), {})
        trig_seq = autos.get("nextTriggerSequence", 0)
        step("own-sequences", ok and ok_a and send_seq == 0 and trig_seq == 0 and bool(auto.get("revision")),
             send=send_seq, trigger=trig_seq)
        ok, started = call("deck_worker_start", {"request_id": f"start_{role}_{tag}", "send_sequence": send_seq,
                                                 "type": "claude", "title": f"{role} typed",
                                                 "text": f"Orchestrator {role}: say hello."})
        step("start-accepted", ok and started.get("result", {}).get("orchestratorCardId") == card["cardId"])
        ok, handed = call("deck_automation_trigger", {"request_id": f"trig_{role}_{tag}", "trigger_sequence": trig_seq,
                                                      "automation_id": "worker", "expected_revision": auto.get("revision", ""),
                                                      "text": f"Orchestrator {role}: handover.", "from": "smoke",
                                                      "where": "#deck-smoke"})
        step("handover-accepted", ok and bool(handed.get("operationId")), code=code(handed))
        typed = committed(started.get("operationId", "")).get("result", {}).get("cardId")
        handover = committed(handed.get("operationId", "")).get("result", {}).get("cardId")
        step("committed", bool(typed) and bool(handover))
        with open(os.path.join(args.evidence, f"orchestrator-{role}-{tag}.json"), "w") as f:
            json.dump({"card": card["cardId"], "cards": [typed, handover], "typed": typed}, f)
        theirs = cards_of(other)
        step("other-ready", theirs is not None)
        ok, out, workers = listed()
        step("lists-own", ok and set(workers) == {typed, handover}, count=len(workers))
        ok, refused = call("deck_worker_send", {"request_id": f"cross_{role}_{tag}", "send_sequence": send_seq + 1,
                                                "card_id": (theirs or {}).get("typed", ""), "after_episode": 0,
                                                "text": "not yours"})
        step("other-refused", not ok and code(refused) == "WORKER_NOT_FOUND", code=code(refused))
        episode = (workers.get(typed, {}).get("agent") or {}).get("episode", 0)
        ok, sent = call("deck_worker_send", {"request_id": f"line_{role}_{tag}", "send_sequence": out.get("nextSendSequence", 0),
                                             "card_id": typed, "after_episode": episode,
                                             "text": f"Orchestrator {role}: one more line."})
        step("own-line", ok and bool(sent.get("operationId")), code=code(sent))
    except SystemExit:
        pass
    except Exception as error:  # a failed step is reported, never hidden
        step("exception", False, kind=type(error).__name__, text=str(error)[:160])
    finally:
        adapter.close()
        record["failures"] = failures
        with open(os.path.join(args.evidence, f"orchestrators-e2e-{role}-{args.phase}-{tag}.json"), "w") as out:
            json.dump(record, out, indent=2)
    print("PASS" if not failures else f"FAIL {len(failures)}: {','.join(failures)}", flush=True)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
