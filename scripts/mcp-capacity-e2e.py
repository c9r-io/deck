#!/usr/bin/env python3
"""Scripted client of the `workers-capacity` WKWebView smoke (app/SMOKE.md
"Worker capacity"): one orchestrator's worker capacity, end to end.

Deck must already be open in the `workers-capacity` mode and past
`workers-armed`. `--adapter` must be a DEBUG deck-mcp build (the --socket /
--credential-fd seams), as for `mcp-worker-e2e.py`.

The run: `--capacity` + 1 starts of the client's one worker type, in a row,
from no Deck card (callers from no card count together): the first
`--capacity` are accepted, the last is refused `CAPACITY_EXCEEDED` at once
and its message names the bound; every accepted start commits with its own
card; deck_worker_list lists exactly that many; one deck_worker_wait
follows all of them, and one naming more cards than a client may have is
refused `INVALID_ARGUMENTS`. The workers' first lines are never sent (the
client has no first-send choice), so no worker takes a turn.

Prints closed facts only: codes, counts and seconds. Exit 0 when every
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
Adapter, PROTOCOL = agent_e2e.Adapter, agent_e2e.PROTOCOL


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--tag", default=str(int(time.time())), help="unique suffix of request ids")
    p.add_argument("--type", default="claude", help="the worker type to start")
    p.add_argument("--capacity", type=int, default=16, help="worker cards one orchestrator may have")
    p.add_argument("--client-capacity", type=int, default=64, help="worker cards one client may have")
    return p.parse_args()


def main():
    args = parse()
    record = {"startedAt": int(time.time()), "clientId": args.client_id, "steps": []}
    failures = []

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), "at": int(time.time()), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)

    def code(out):
        return (out.get("error") or {}).get("code")

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-capacity-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        ok, out = adapter.call("deck_worker_list")
        types = [t.get("type") for t in out.get("workerTypes", [])]
        step("worker-list-empty", ok and out.get("workers") == [] and args.type in types, types=",".join(types))
        sequence = out.get("nextSendSequence", 0)
        _, caps = adapter.call("deck_capabilities", {})
        root = ((caps.get("authorizedWorkspaces") or [{}])[0].get("roots") or [""])[0]

        operations = []
        refused = None
        for n in range(args.capacity + 1):
            ok, out = adapter.call("deck_worker_start", {
                "request_id": f"cap_{args.tag}_{n}", "send_sequence": sequence, "type": args.type, "dir": root,
                "text": "Reply with exactly: CAPACITY-OK. Do not use tools.", "title": f"cap {n}"})
            if ok:
                operations.append(out.get("operationId"))
                sequence += 1
            else:
                refused = out
                break
        step("starts-accepted", len(operations) == args.capacity, accepted=len(operations))
        message = ((refused or {}).get("error") or {}).get("message") or ""
        step("next-start-refused", refused is not None and code(refused) == "CAPACITY_EXCEEDED"
             and f"already has {args.capacity} worker cards" in message, code=code(refused or {}))

        cards = {}
        deadline = time.time() + 600
        while time.time() < deadline and len(cards) < len(operations):
            for op in operations:
                if op in cards:
                    continue
                ok, got = adapter.call("deck_operation_get", {"operation_id": op})
                if got.get("state") == "committed":
                    cards[op] = (got.get("result") or {}).get("cardId")
                elif got.get("state") == "rejected":
                    cards[op] = None
            time.sleep(2)
        committed = [card for card in cards.values() if card]
        step("starts-committed", len(committed) == args.capacity and len(set(committed)) == len(committed),
             committed=len(committed))

        ok, out = adapter.call("deck_worker_list")
        listed = {w.get("cardId") for w in out.get("workers", [])}
        step("workers-listed", ok and listed == set(committed), listed=len(listed))

        started = time.time()
        ok, out = adapter.call("deck_worker_wait", {"cards": [{"card_id": card, "episode": 0} for card in committed],
                                                     "wait_ms": 1000})
        step("wait-follows-all", ok and len(out.get("workers", [])) == len(committed),
             seen=len(out.get("workers", [])), seconds=round(time.time() - started, 1))
        too_many = [{"card_id": f"W{n:015d}", "episode": 0} for n in range(args.client_capacity + 1)]
        ok, out = adapter.call("deck_worker_wait", {"cards": too_many, "wait_ms": 0})
        step("wait-bounded-by-client", not ok and code(out) == "INVALID_ARGUMENTS"
             and f"1 to {args.client_capacity} worker cards" in ((out.get("error") or {}).get("message") or ""),
             code=code(out))
    except Exception as error:  # noqa: BLE001 - a failed run says which step, not a traceback with ids
        step("exception", False, kind=type(error).__name__)
    finally:
        adapter.close()
    record["failures"] = failures
    with open(args.evidence, "w") as handle:
        json.dump(record, handle, indent=2)
    print("PASS" if not failures else f"FAIL {','.join(failures)}", flush=True)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
