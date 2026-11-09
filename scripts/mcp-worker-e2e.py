#!/usr/bin/env python3
"""Scripted orchestrator for the worker-card tools of an isolated Deck smoke
run (app/SMOKE.md "Worker cards", WKWebView mode `workers`, part A).

Deck must already be open in the `workers` mode and past `workers-armed`.
`--adapter` must be a DEBUG deck-mcp build (the --socket / --credential-fd
seams). The credential is handed to the adapter on an inherited
descriptor, never on its argv.

The run: tools/list (20 tools), deck_worker_list (no worker yet), the
automation listed and triggered with a first line, the operation committed
with its cardId, the card's turn ending waited for (deck_worker_wait) and
its reply read; a line written after a wrong episode (EPISODE_MOVED) and
with a wrong sequence (STALE_REQUEST); the second line sent, delivered and
its reply read. Then the page takes the card over: the third line is
accepted, stays unsent and starts no turn while the card is held; after the
page hands the card back it is delivered and its reply read. Last, the page
withdraws the worker choice: deck_worker_list answers PERMISSION_DENIED.

Prints closed facts only: codes, ids, counts, episodes and seconds; never
the credential and never a reply beyond whether it carries the expected
token. Exit 0 when every expectation held, 1 otherwise, 2 for usage.
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
    for name in ("adapter", "socket", "client-id", "credential-file", "automation", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--tag", default=str(int(time.time())), help="unique suffix of request ids and tokens")
    p.add_argument("--card", help="continue with this worker card (its first line already sent) instead of starting one")
    p.add_argument("--first-token", help="with --card: the token the card's first line asked for")
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

    def call(name, arguments=None):
        ok, out = adapter.call(name, arguments)
        return ok, out

    def code(out):
        return (out.get("error") or {}).get("code")

    def worker(card):
        ok, out = call("deck_worker_list")
        if not ok:
            return None, out
        return next((w for w in out.get("workers", []) if w.get("cardId") == card), None), out

    def wait_for(card, test, seconds, seen_episode):
        """deck_worker_wait in rounds of 5 s until `test(view)` holds."""
        deadline = time.time() + seconds
        view = None
        while time.time() < deadline:
            ok, out = call("deck_worker_wait", {"cards": [{"card_id": card, "episode": seen_episode}], "wait_ms": 5000})
            if not ok:
                return None, out
            view = next((w for w in out.get("workers", []) if w.get("cardId") == card), None)
            if view and test(view):
                return view, out
            if view:
                seen_episode = view["agent"]["episode"]
        return view, None

    def turn_done_after(card, episode, seconds):
        return wait_for(card, lambda v: v["agent"]["state"] == "turn-done" and v["agent"]["episode"] != episode, seconds, episode)

    def reply_has(card, episode, token):
        ok, out = call("deck_worker_reply", {"card_id": card, "episode": episode})
        return ok and token in out.get("text", ""), out

    sequence = 0

    def send(card, after, text, rid, seq=None):
        return call("deck_worker_send", {"request_id": rid, "send_sequence": sequence if seq is None else seq,
                                         "card_id": card, "after_episode": after, "text": text})

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-worker-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        names = [tool["name"] for tool in adapter.request("tools/list")["result"]["tools"]]
        step("tools-list", len(names) == TOOLS and all(n in names for n in
             ("deck_worker_list", "deck_worker_wait", "deck_worker_reply", "deck_worker_send")), count=len(names))
        ok, out = call("deck_worker_list")
        step("worker-list-empty", ok and out.get("workers") == [] and out.get("mayReadReplies") is True,
             next=out.get("nextSendSequence"))
        sequence = out.get("nextSendSequence", 0)

        # one worker through the automation (or the one a previous run started)
        ok, listed = call("deck_automation_list")
        auto = next((a for a in listed.get("automations", []) if a.get("id") == args.automation), None)
        step("automation-listed", ok and auto is not None)
        token1 = args.first_token or f"WORKER-{tag}-ONE"
        if args.card:
            trig = None
        else:
          ok, trig = call("deck_automation_trigger", {
            "request_id": f"wtrig_{tag}", "trigger_sequence": listed.get("nextTriggerSequence", 0),
            "automation_id": args.automation, "expected_revision": auto["revision"],
            "text": f"Reply with exactly: {token1}. Do not use tools.", "from": "worker smoke", "where": "#deck-smoke"})
        op = trig.get("operationId") if trig else None
        card = args.card
        deadline = time.time() + (0 if args.card else 120)
        while time.time() < deadline:
            ok, got = call("deck_operation_get", {"operation_id": op})
            if got.get("state") == "committed":
                card = (got.get("result") or {}).get("cardId")
                break
            if got.get("state") == "rejected":
                break
            time.sleep(2)
        step("worker-started", card is not None, card=card)
        record["card"] = card

        # episodes are 1-based: 0 is "no observation yet"
        view, _ = turn_done_after(card, 0, 300)
        e1 = view["agent"]["episode"] if view else None
        step("first-turn-ended", view is not None and view["agent"]["state"] == "turn-done", episode=e1,
             takenOver=view and view.get("takenOver"))
        ok, out = reply_has(card, e1, token1)
        step("first-reply", ok, truncated=out.get("truncated"), code=code(out))

        # refused lines: a moved episode, a stale sequence
        ok, out = send(card, e1 + 1000, "never sent", f"wbad_{tag}")
        step("episode-moved", not ok and code(out) == "EPISODE_MOVED" and out["error"].get("currentEpisode") == e1)
        ok, out = send(card, e1, "never sent", f"wstale_{tag}", seq=sequence + 7)
        step("stale-sequence", not ok and code(out) == "STALE_REQUEST")

        # the second line, delivered after the boundary
        token2 = f"WORKER-{tag}-TWO"
        ok, out = send(card, e1, f"Reply with exactly: {token2}. Do not use tools.", f"wsend2_{tag}")
        step("second-accepted", ok and out.get("state") == "committed", op=out.get("operationId"))
        replay_ok, replay = send(card, e1, f"Reply with exactly: {token2}. Do not use tools.", f"wsend2_{tag}")
        step("second-replayed", replay_ok and replay.get("operationId") == out.get("operationId"))
        sequence += 1
        view, _ = turn_done_after(card, e1, 300)
        e2 = view["agent"]["episode"] if view else None
        step("second-turn-ended", view is not None, episode=e2)
        ok, out = reply_has(card, e2, token2)
        step("second-reply", ok, code=code(out))

        # the page takes the card over
        view, _ = wait_for(card, lambda v: v.get("takenOver") is True, 240, e2)
        step("taken-over-seen", view is not None and view.get("takenOver") is True)
        current = view["agent"]["episode"] if view else e2
        token3 = f"WORKER-{tag}-THREE"
        ok, out = send(card, current, f"Reply with exactly: {token3}. Do not use tools.", f"wsend3_{tag}")
        step("third-accepted-while-held", ok and out.get("state") == "committed")
        sequence += 1
        time.sleep(30)
        held, _ = worker(card)
        step("third-held", held is not None and held.get("takenOver") is True and held.get("unsent") == 1
             and held["agent"]["episode"] == current, unsent=held and held.get("unsent"))

        # handed back: it goes out, and its reply is read
        view, _ = wait_for(card, lambda v: v.get("takenOver") is False, 300, current)
        step("handed-back-seen", view is not None)
        view, _ = turn_done_after(card, current, 300)
        e3 = view["agent"]["episode"] if view else None
        step("third-turn-ended", view is not None and view.get("unsent") == 0, episode=e3)
        ok, out = reply_has(card, e3, token3)
        step("third-reply", ok, code=code(out))
        states = [s.get("state") for s in (view or {}).get("sends", [])]
        step("sends-delivered", states.count("delivered") >= 2, states=",".join(states))

        # the page withdraws the choice
        deadline = time.time() + 300
        denied = None
        while time.time() < deadline:
            ok, out = call("deck_worker_list")
            if not ok and code(out) == "PERMISSION_DENIED":
                denied = out
                break
            time.sleep(3)
        step("withdrawn-denied", denied is not None)
    except Exception as error:  # a failed step is reported, never hidden
        step("exception", False, kind=type(error).__name__, text=str(error)[:160])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["finishedAt"] = int(time.time())
        record["failures"] = failures
        os.makedirs(args.evidence, exist_ok=True)
        with open(os.path.join(args.evidence, f"worker-e2e-{tag}.json"), "w") as out:
            json.dump(record, out, indent=2)
    print("PASS" if not failures else f"FAIL {len(failures)}: {','.join(failures)}", flush=True)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
