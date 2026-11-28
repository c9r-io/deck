#!/usr/bin/env python3
"""Scripted orchestrator for the `worker-recovery` WKWebView smoke
(app/SMOKE.md "Queued lines, a Deck restart and a gone session", 2026-11-28).

Run it INSIDE the page's plain-shell card "Orch R" (the runner types it there):
Deck reads which card a call comes from out of the kernel. `--adapter` must be
a DEBUG deck-mcp build (the --socket / --credential-fd seams). The worker is a
real Claude Code with the fixture's hooks.

--phase 1 (first launch), the field report made concrete:
  1. tools/list carries deck_worker_cancel and close_mode `gone`;
  2. one worker started; its task: a foreground program of ~9.5 minutes
     (`phase1.py`), messages meanwhile are notes to ignore, then DONE;
  3. while it runs, line A is queued (`deck_worker_send`): it waits for the
     card's quiet time;
  4. nine lines are interjected one after another, each followed until its
     delivery says `delivered`; after each, A is still listed, counted
     (`activeLines.queued`) and its own delivery (`deck_operation_get`) still
     says queued — past the eight-line history that used to hide it;
  5. A is canceled (`deck_worker_cancel`): canceled; the same request again
     is the same answer; then B is interjected with a new request id and is
     delivered once; A never is (also after the worker's turn ended).
--phase 2 (after the runner relaunched Deck on the same tmux server):
  6. the worker is alive, unobserved (`unavailable`, `no-turn-seen`), its
     foreground began before this Deck process;
  7. no close mode closes it: episode 0, its instance, and `gone` are refused
     (OBSERVATION_UNKNOWN, OBSERVATION_UNKNOWN, SESSION_PRESENT), none
     suggesting a weaker mode;
  8. its session is ended (tmux kill-session, the runner's tmux) and Deck
     sees it gone (liveness gone, instance null); `close_mode: gone` with
     its lastLine closes it (committed), the same request again is the same
     operation, the card leaves the list.

Prints closed facts only: codes, ids, states, seconds. Exit 0 when every
expectation held, 1 otherwise.
"""
import argparse
import importlib.util
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("agent_e2e", os.path.join(HERE, "mcp-agent-e2e.py"))
agent_e2e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(agent_e2e)
Adapter, PROTOCOL = agent_e2e.Adapter, agent_e2e.PROTOCOL

INTERJECTS = 9


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--phase", choices=["1", "2"], required=True)
    p.add_argument("--tmux", default="", help="phase 2: the carrier's tmux binary (the pane's $TMUX names the server)")
    p.add_argument("--tag", default=str(int(time.time())))
    return p.parse_args()


def find_property(schema, name):
    if isinstance(schema, dict):
        found = (schema.get("properties") or {}).get(name)
        if found is not None:
            return found
        for value in schema.values():
            hit = find_property(value, name)
            if hit is not None:
                return hit
    if isinstance(schema, list):
        for value in schema:
            hit = find_property(value, name)
            if hit is not None:
                return hit
    return None


def main():
    args = parse()
    tag = args.tag
    record = {"startedAt": int(time.time()), "tag": tag, "phase": args.phase, "steps": []}
    failures = []
    os.makedirs(args.evidence, exist_ok=True)
    path = os.path.join(args.evidence, f"recovery-e2e-{tag}-phase{args.phase}.json")

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), "at": round(time.time(), 1), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        with open(path, "w") as out:
            json.dump({**record, "failures": failures, "partial": True}, out, indent=1)
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call
    code = lambda out: (out.get("error") or {}).get("code")

    def listing():
        ok, out = call("deck_worker_list")
        return out if ok else {}

    def view(card):
        out = listing()
        return next((w for w in out.get("workers", []) if w.get("cardId") == card), None), out

    def until(test, seconds, every=1.0):
        deadline = time.time() + seconds
        while time.time() < deadline:
            value = test()
            if value:
                return value
            time.sleep(every)
        return None

    def settled(operation, seconds=180):
        def done():
            ok, got = call("deck_operation_get", {"operation_id": operation})
            return got if ok and got.get("state") in ("committed", "rejected", "ambiguous") else None
        return until(done, seconds) or {}

    def delivery(operation):
        ok, got = call("deck_operation_get", {"operation_id": operation})
        return (got.get("delivery") or {}) if ok else {}

    def line_of(row, operation):
        return next((l for l in (row or {}).get("sends", []) if l.get("operationId") == operation), None)

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-recovery-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        tools = adapter.request("tools/list")["result"]["tools"]
        by_name = {tool["name"]: tool for tool in tools}
        mode = find_property(by_name.get("deck_worker_close", {}).get("inputSchema"), "close_mode") or {}
        step("tools-list", "deck_worker_cancel" in by_name and "gone" in (mode.get("enum") or []),
             count=len(tools), close_modes=",".join(mode.get("enum") or []))
        ok, caps = call("deck_capabilities", {})
        step("caller-is-a-card", ok and bool((caps.get("orchestratorCard") or {}).get("cardId")))
        work = ((caps.get("authorizedWorkspaces") or [{}])[0].get("roots") or [""])[0]

        if args.phase == "1":
            lane = listing().get("laneEpoch")
            task = (f"Task {tag}. Step 1: run exactly this shell command in the foreground (not in the background), "
                    f"with a 600000 ms timeout, and wait until it finishes: python3 phase1.py . While it runs you may "
                    f"receive short messages starting with NOTE: they are test notes, take no action on them and do not "
                    f"answer them. Step 2: reply with exactly DONE-{tag}. Run no other command.")
            ok, started = call("deck_worker_start", {"request_id": f"rs_{tag}", "lane_epoch": lane, "type": "claude",
                                                     "text": task, "title": "recovery worker", "dir": work})
            done = settled(started.get("operationId")) if ok else {}
            card = (done.get("result") or {}).get("cardId")
            record["card"] = card
            step("worker-started", bool(card), state=done.get("state"), code=code(started))
            if not card:
                return
            began = until(lambda: os.path.exists(os.path.join(work, "p1-started")), 600, 2)
            row = until(lambda: (lambda r: r if r and (r.get("agent") or {}).get("state") == "working" else None)(view(card)[0]), 120, 2)
            step("worker-busy", bool(began) and bool(row), state=((row or {}).get("agent") or {}).get("state"))
            episode = ((row or {}).get("agent") or {}).get("episode", 0)
            lane = listing().get("laneEpoch")
            ok, sent_a = call("deck_worker_send", {"request_id": f"ra_{tag}", "lane_epoch": lane, "card_id": card,
                                                  "after_episode": episode, "text": f"NOTE A {tag}: nothing to do."})
            op_a = sent_a.get("operationId")
            step("line-a-queued", ok and sent_a.get("state") == "committed", state=sent_a.get("state"), code=code(sent_a))
            time.sleep(4)
            d = delivery(op_a)
            step("line-a-waits", d.get("state") == "queued" and d.get("cancelable") is True,
                 state=d.get("state"), waiting=d.get("waiting"), quiet=d.get("quietRemaining"), checked=bool(d.get("checkedAt")))
            record["interjects"] = []
            for n in range(1, INTERJECTS + 1):
                row, out = view(card)
                episode = ((row or {}).get("agent") or {}).get("episode", 0)
                ok, sent = call("deck_worker_interject", {"request_id": f"ri{n}_{tag}", "lane_epoch": out.get("laneEpoch"),
                                                          "card_id": card, "after_episode": episode,
                                                          "text": f"NOTE {n} {tag}: no action."})
                op = sent.get("operationId")
                got = until(lambda: (lambda x: x if x.get("state") in ("delivered", "uncertain", "canceled") else None)(delivery(op)),
                            150, 2) if ok else None
                row, _ = view(card)
                a_line = line_of(row, op_a)
                a_now = delivery(op_a)
                fact = {"n": n, "state": (got or {}).get("state"), "code": code(sent),
                        "aListed": (a_line or {}).get("state"), "aQueued": ((row or {}).get("activeLines") or {}).get("queued"),
                        "aDelivery": a_now.get("state"), "aWaiting": a_now.get("waiting"), "sends": len((row or {}).get("sends", []))}
                record["interjects"].append(fact)
                step(f"interject-{n}", (got or {}).get("state") == "delivered" and (a_line or {}).get("state") == "queued"
                     and fact["aQueued"] == 1 and a_now.get("state") == "queued", **fact)
            row, _ = view(card)
            settled_lines = [l for l in (row or {}).get("sends", []) if l.get("state") == "delivered" and l.get("mode") == "interject"]
            step("a-past-the-history", len(settled_lines) >= 8 and line_of(row, op_a) is not None,
                 delivered_shown=len(settled_lines), unsent=(row or {}).get("unsent"))
            # cancel A, then its replay, then B with a NEW request id
            ok, canceled = call("deck_worker_cancel", {"request_id": f"rc_{tag}", "card_id": card, "operation_id": op_a})
            outcome = (canceled.get("result") or {}).get("outcome")
            step("a-canceled", ok and canceled.get("state") == "committed" and outcome == "canceled"
                 and (canceled.get("delivery") or {}).get("state") == "canceled", outcome=outcome, code=code(canceled))
            ok, again = call("deck_worker_cancel", {"request_id": f"rc_{tag}", "card_id": card, "operation_id": op_a})
            step("cancel-replay", ok and again.get("operationId") == canceled.get("operationId")
                 and (again.get("result") or {}).get("outcome") == "canceled", replayed=again.get("replayed"))
            ok, other = call("deck_worker_cancel", {"request_id": f"rc_{tag}", "card_id": card, "operation_id": "op_other"})
            step("cancel-id-conflict", not ok and code(other) == "REQUEST_ID_CONFLICT", code=code(other))
            row, out = view(card)
            episode = ((row or {}).get("agent") or {}).get("episode", 0)
            ok, sent_b = call("deck_worker_interject", {"request_id": f"rb_{tag}", "lane_epoch": out.get("laneEpoch"),
                                                        "card_id": card, "after_episode": episode,
                                                        "text": f"NOTE B {tag}: this replaces note A; no action."})
            op_b = sent_b.get("operationId")
            got_b = until(lambda: (lambda x: x if x.get("state") in ("delivered", "uncertain") else None)(delivery(op_b)), 150, 2)
            step("b-delivered", (got_b or {}).get("state") == "delivered", state=(got_b or {}).get("state"))
            # the same send request again is the same line, not a second one
            ok, b_again = call("deck_worker_interject", {"request_id": f"rb_{tag}", "lane_epoch": out.get("laneEpoch"),
                                                         "card_id": card, "after_episode": episode,
                                                         "text": f"NOTE B {tag}: this replaces note A; no action."})
            step("b-replay-is-one-line", ok and b_again.get("operationId") == op_b, replayed=b_again.get("replayed"))
            ended = until(lambda: (lambda r: r if r and (r.get("agent") or {}).get("state") == "turn-done" else None)(view(card)[0]), 1200, 5)
            row, _ = view(card)
            a_final, b_final = delivery(op_a), delivery(op_b)
            b_lines = [l for l in (row or {}).get("sends", []) if l.get("operationId") == op_b]
            step("after-the-turn", bool(ended) and a_final.get("state") == "canceled" and b_final.get("state") == "delivered"
                 and len(b_lines) == 1 and ((row or {}).get("activeLines") or {}) == {"queued": 0, "sending": 0, "uncertain": 0},
                 a=a_final.get("state"), b=b_final.get("state"), active=json.dumps((row or {}).get("activeLines")))
            record["instance"] = (row or {}).get("instance")
            record["lastLine"] = (row or {}).get("lastLine")
            return

        # ---- phase 2: after the relaunch ----
        out = listing()
        rows = out.get("workers", [])
        row = rows[0] if len(rows) == 1 else None
        card = (row or {}).get("cardId")
        record["card"] = card
        agent, fg = (row or {}).get("agent") or {}, (row or {}).get("foreground") or {}
        step("restart-facts", bool(row) and row.get("liveness") == "alive" and agent.get("state") == "unavailable"
             and agent.get("why") == "no-turn-seen" and fg.get("kind") == "program" and fg.get("beforeDeckStarted") is True
             and bool(out.get("deckStartedAt")) and bool(row.get("instance")),
             liveness=(row or {}).get("liveness"), state=agent.get("state"), why=agent.get("why"), kind=fg.get("kind"),
             before=fg.get("beforeDeckStarted"), deck=out.get("deckStartedAt"), started=fg.get("startedAt"))
        if not card:
            return
        lane = out.get("laneEpoch")
        refused = {}
        for name, extra in (("episode-0", {"after_episode": 0}),
                            ("instance", {"close_mode": "instance", "instance": row.get("instance"),
                                          **({"last_line": row["lastLine"]} if row.get("lastLine") else {})}),
                            ("gone", {"close_mode": "gone", **({"last_line": row["lastLine"]} if row.get("lastLine") else {})})):
            ok, got = call("deck_worker_close", {"request_id": f"rx{name}_{tag}", "lane_epoch": lane, "card_id": card, **extra})
            err = got.get("error") or {}
            refused[name] = (err.get("code"), err.get("stage"), "close_mode: episode" in (err.get("nextAction") or ""))
        step("no-mode-closes-an-unobserved-agent",
             refused["episode-0"][:2] == ("OBSERVATION_UNKNOWN", "precheck") and refused["instance"][:2] == ("OBSERVATION_UNKNOWN", "precheck")
             and refused["gone"][:2] == ("SESSION_PRESENT", "precheck") and not any(v[2] for v in refused.values()),
             refusals=json.dumps(refused))
        # the session ends (not a verified exit: the card stays, stopped)
        server = (os.environ.get("TMUX") or "").split(",")[0]
        sessions = subprocess.run([args.tmux, "-S", server, "list-sessions", "-F", "#{session_name}"],
                                  capture_output=True, text=True, timeout=10).stdout.split()
        mine = subprocess.run([args.tmux, "-S", server, "display-message", "-p", "#{session_name}"],
                              capture_output=True, text=True, timeout=10).stdout.strip()
        others = [s for s in sessions if s != mine]
        step("one-worker-session", len(others) == 1, sessions=len(sessions))
        if len(others) == 1:
            subprocess.run([args.tmux, "-S", server, "kill-session", "-t", others[0]], timeout=10)
        gone = until(lambda: (lambda r: r if r and r.get("liveness") == "gone" else None)(view(card)[0]), 60, 2)
        step("seen-gone", bool(gone) and gone.get("instance") is None and (gone.get("agent") or {}).get("why") == "session-gone",
             liveness=(gone or {}).get("liveness"), instance=(gone or {}).get("instance"))
        close = {"request_id": f"rg_{tag}", "lane_epoch": listing().get("laneEpoch"), "card_id": card, "close_mode": "gone"}
        if (gone or {}).get("lastLine"):
            close["last_line"] = gone["lastLine"]
        ok, accepted = call("deck_worker_close", close)
        step("gone-close-accepted", ok and accepted.get("state") in ("accepted", "committed"),
             state=accepted.get("state"), code=code(accepted))
        final = settled(accepted.get("operationId"), 240) if ok else {}
        step("gone-close-committed", final.get("state") == "committed", state=final.get("state"), code=final.get("code"))
        ok, replay = call("deck_worker_close", close)
        step("gone-replay-same", ok and replay.get("operationId") == accepted.get("operationId"), state=replay.get("state"))
        step("card-left-the-list", bool(until(lambda: view(card)[0] is None, 30)))
    except Exception as error:  # noqa: BLE001 — the run's own failure is a step
        step("exception", False, error=type(error).__name__, detail=str(error)[:200])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["failures"] = failures
        with open(path, "w") as out:
            json.dump(record, out, indent=1)
        print("PASS" if not failures else f"FAIL {failures}", flush=True)
        sys.exit(0 if not failures else 1)


if __name__ == "__main__":
    main()
