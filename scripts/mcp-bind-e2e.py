#!/usr/bin/env python3
"""Scripted orchestrator for the `worker-bind` WKWebView smoke
(app/SMOKE.md "Worker lines bound to the instance", `bind_mode: instance`).

Run it INSIDE the page's plain-shell card "Orch I" (the operator types it
there after `interject-armed`): Deck reads which card a call comes from out
of the kernel. `--adapter` must be a DEBUG deck-mcp build (the --socket /
--credential-fd seams).

The field case it replays: a Codex worker hands in its report, the
orchestrator wakes on it while the worker is still `working`, the worker's
own turn ends (`turn-done`, a new episode), and the orchestrator's
interjection written against the episode it woke on is refused with
EPISODE_MOVED. The run:
  1. tools/list: deck_worker_interject and deck_worker_send carry the
     `bind_mode` enum (episode, instance);
  2. one Codex worker started from this card: run `python3 report.py` (the
     fixture's: it calls deck_worker_report from the worker's own shell,
     then waits 20 s), then reply and end the turn;
  3. deck_worker_wait until: report wakes this client; it keeps the
     episode, instance and lastLine it woke on;
  4. it waits until the worker's own turn ended (a newer episode);
  5. the episode-bound interjection of step 3's episode: EPISODE_MOVED,
     `queued: false`, its expected and current episode;
  6. the instance-bound interjection of step 3's instance and lastLine
     (create bind-<tag>.txt): accepted with no further read; its replay is
     the same operation; deck_operation_get follows that one line to
     `delivered`, and the worker creates the file;
  7. a second instance-bound line against step 3's (now old) lastLine:
     LAST_LINE_MOVED, `queued: false`;
  8. once the worker's turn ended: close_mode instance with the current
     instance and lastLine closes it (the page waits for that).

Prints closed facts only: codes, ids, states, stages, episodes, seconds and
whether files exist. Exit 0 when every expectation held, 1 otherwise.
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
    p.add_argument("--tag", default=str(int(time.time())))
    p.add_argument("--type", default="codex", help="the worker type the page listed")
    return p.parse_args()


def main():
    args = parse()
    tag = args.tag
    record = {"startedAt": int(time.time()), "tag": tag, "steps": []}
    failures = []
    os.makedirs(args.evidence, exist_ok=True)
    t0 = time.time()

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), "at": round(time.time() - t0, 1), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        save()
        return ok

    def save():
        # after every step: the page closes this card once the worker is
        # closed, which may end this process before its last line
        record["failures"] = failures
        with open(os.path.join(args.evidence, f"bind-e2e-{tag}.json"), "w") as f:
            json.dump(record, f, indent=1)

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call
    code = lambda out: (out.get("error") or {}).get("code")

    def view(card):
        ok, out = call("deck_worker_list")
        row = next((w for w in out.get("workers", []) if w.get("cardId") == card), None) if ok else None
        return row, out

    def until(test, seconds, every=1.0):
        deadline = time.time() + seconds
        while time.time() < deadline:
            value = test()
            if value:
                return value
            time.sleep(every)
        return None

    def settled(operation):
        def done():
            ok, got = call("deck_operation_get", {"operation_id": operation})
            return got if ok and got.get("state") in ("committed", "rejected", "ambiguous") else None
        return until(done, 180) or {}

    def sequence():
        return call("deck_worker_list")[1].get("nextSendSequence", 0)

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-bind-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        tools = adapter.request("tools/list")["result"]["tools"]
        schema = {t["name"]: t.get("inputSchema", {}) for t in tools}
        modes = {name: ((schema.get(name, {}).get("properties") or {}).get("bind_mode") or {}).get("enum")
                 for name in ("deck_worker_interject", "deck_worker_send")}
        step("tools-list-bind-mode", len(tools) == TOOLS and all(m == ["episode", "instance"] for m in modes.values()),
             count=len(tools), modes=json.dumps(modes))
        ok, caps = call("deck_capabilities", {})
        step("caller-is-a-card", ok and bool((caps.get("orchestratorCard") or {}).get("cardId"))
             and caps.get("mayInterjectWorkers") is True)
        work = ((caps.get("authorizedWorkspaces") or [{}])[0].get("roots") or [""])[0]
        # the worker's report goes through Deck's adapter from the worker's
        # own shell (report.py, the fixture's): the same client, the card
        # proven from the kernel
        with open(os.path.join(work, "report-args.json"), "w") as f:
            json.dump({"adapter": args.adapter, "socket": args.socket, "clientId": args.client_id,
                       "credentialFile": args.credential_file, "helper": os.path.join(HERE, "mcp-agent-e2e.py"),
                       "tag": tag}, f)
        task = (f"Task {tag}: run exactly this shell command in the foreground and wait until it finishes: "
                f"python3 report.py . Then reply with exactly REPORTED-{tag} and stop. Run no other command "
                f"unless a later message asks you to.")
        ok, started = call("deck_worker_start", {"request_id": f"bs_{tag}", "send_sequence": sequence(),
                                                 "type": args.type, "text": task, "title": "bind worker", "dir": work})
        card = (settled(started.get("operationId")).get("result") or {}).get("cardId") if ok else None
        step("worker-started", bool(card), code=code(started))
        if not card:
            raise RuntimeError("no worker")
        # 3. the report wakes this client
        answer = adapter.request("tools/call", {"name": "deck_worker_wait", "arguments": {
            "cards": [{"card_id": card, "episode": 0, "reports": 0}], "until": "report",
            "wait_ms": 900_000, "include_reports": True}}, timeout=960)
        result = answer.get("result") or {}
        ok, woke = not result.get("isError", True), result.get("structuredContent") or {}
        # what the client decides on is what the wake itself said
        row = next((w for w in woke.get("workers", []) if w.get("cardId") == card), None)
        reason = woke.get("reason")
        at_wake = {"episode": ((row or {}).get("agent") or {}).get("episode"),
                   "state": ((row or {}).get("agent") or {}).get("state"),
                   "instance": (row or {}).get("instance"), "lastLine": (row or {}).get("lastLine")}
        step("report-woke", ok and reason == "report" and bool(at_wake["instance"]), reason=reason,
             episode=at_wake["episode"], state=at_wake["state"], instance=bool(at_wake["instance"]))
        # 4. the worker's own turn ends after its report
        def ended():
            r, _ = view(card)
            a = (r or {}).get("agent") or {}
            return r if a.get("state") == "turn-done" and a.get("episode") != at_wake["episode"] else None
        after = until(ended, 300, 0.5)
        now = ((after or {}).get("agent") or {})
        step("worker-stop-after-report", bool(after), episode=now.get("episode"),
             same_instance=(after or {}).get("instance") == at_wake["instance"])
        # 5. the episode the client woke on: refused, nothing queued
        ok, moved = call("deck_worker_interject", {
            "request_id": f"be_{tag}", "send_sequence": sequence(), "card_id": card,
            "after_episode": at_wake["episode"], "text": f"Create the file episode-{tag}.txt."})
        err = moved.get("error") or {}
        step("episode-bound-refused", not ok and code(moved) == "EPISODE_MOVED" and err.get("queued") is False
             and err.get("afterEpisode") == at_wake["episode"] and err.get("currentEpisode") == now.get("episode"),
             code=code(moved), queued=err.get("queued"), expected=err.get("afterEpisode"),
             current=err.get("currentEpisode"), current_state=err.get("currentState"))
        # 6. the instance and line base it woke on: accepted with no new read
        bound = {"request_id": f"bi_{tag}", "send_sequence": sequence(), "card_id": card, "bind_mode": "instance",
                 "instance": at_wake["instance"], "last_line": at_wake["lastLine"],
                 "text": f"Now create the file bind-{tag}.txt containing the word instance, then reply with exactly BOUND-{tag}."}
        sent_at = time.time()
        ok, accepted = call("deck_worker_interject", bound)
        operation = accepted.get("operationId")
        step("instance-bound-accepted", ok and bool(operation), code=code(accepted), state=accepted.get("state"))
        ok_r, replay = call("deck_worker_interject", bound)
        step("replay-is-the-same-operation", ok_r and replay.get("operationId") == operation)

        def delivered():
            ok, got = call("deck_operation_get", {"operation_id": operation})
            d = (got or {}).get("delivery") or {}
            return d if ok and d.get("state") in ("delivered", "uncertain", "canceled") else None
        delivery = until(delivered, 240, 0.5) or {}
        step("delivery-exact", delivery.get("state") == "delivered", state=delivery.get("state"),
             seconds=round(time.time() - sent_at, 1))
        made = until(lambda: os.path.exists(os.path.join(work, f"bind-{tag}.txt")), 300, 1.0)
        step("worker-followed-the-line", bool(made), episode_file=os.path.exists(os.path.join(work, f"episode-{tag}.txt")))
        # 7. the old line base again: refused, nothing queued
        stale = dict(bound, request_id=f"bl_{tag}", send_sequence=sequence(), text="Create the file late.txt.")
        ok, late = call("deck_worker_interject", stale)
        err = late.get("error") or {}
        step("old-line-base-refused", not ok and code(late) == "LAST_LINE_MOVED" and err.get("queued") is False
             and err.get("currentLastLine") == operation, code=code(late))
        # 8. close the worker once its turn ended
        final = until(lambda: (lambda r: r if ((r or {}).get("agent") or {}).get("state") == "turn-done"
                               and not any(s.get("state") in ("queued", "sending") for s in (r or {}).get("sends", []))
                               else None)(view(card)[0]), 300, 1.0)
        ok, closing = call("deck_worker_close", {"request_id": f"bc_{tag}", "send_sequence": sequence(), "card_id": card,
                                                 "close_mode": "instance", "instance": (final or {}).get("instance"),
                                                 "last_line": (final or {}).get("lastLine")})
        closed = settled(closing.get("operationId")) if ok else {}
        step("closed-by-instance", ok and closed.get("state") == "committed", code=code(closing),
             state=closed.get("state"))
    except Exception as error:  # a closed fact, never a traceback with paths
        step("client-error", False, kind=type(error).__name__)
    finally:
        record["stderrTail"] = adapter.close()[-400:]
    record["finished"] = True
    save()
    print("PASS" if not failures else "FAIL " + ",".join(failures), flush=True)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
