#!/usr/bin/env python3
"""Scripted orchestrator for the `worker-close` WKWebView smoke
(app/SMOKE.md "Closing a worker that reported", round 2 of 2026-11-28).

Run it INSIDE the page's plain-shell card "Orch C" (the runner types it there
after `close-armed`): Deck reads which card a call comes from out of the
kernel, so the script names nothing about itself. `--adapter` must be a DEBUG
deck-mcp build (the --socket / --credential-fd seams). The worker is a real
Claude Code with this instance's adapter as its project MCP server (the
runner's .mcp.json in the work directory) and the report tool allowed.

The run, the field report made concrete:
  1. tools/list: 40 tools, `deck_worker_reports_ack` among them, the report
     status an `enum` that holds `handoff`, `deck_worker_close` with its
     `close_mode` enum;
  2. one worker started from this card; its task: call deck_worker_report
     FIRST (status handoff), then a 40 s foreground program (`phase1.py`),
     then end its turn;
  3. ONE long wait (`until: report`, include_reports) returns with the
     report's text, while the agent still works; the orchestrator decides
     there and then to close: it keeps the episode, the instance and the
     last line it saw;
  4. it acknowledges the report by the sequence of the text it was given
     (the page sees the card's label go from "not yet received" back);
  5. the worker ends its turn on its own (a new episode, the same instance);
  6. the close bound to the episode it decided on is refused at its first
     check, EPISODE_MOVED with stage and what Deck saw — the field symptom;
  7. the close bound to the instance of the same decision is accepted, and
     its operation ends `committed`: the card is gone; the exact request
     again is the same operation.

Prints closed facts only: codes, ids, states, episodes, seconds. Exit 0 when
every expectation held, 1 otherwise.
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
    record = {"startedAt": int(time.time()), "tag": tag, "steps": []}
    failures = []
    os.makedirs(args.evidence, exist_ok=True)

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), "at": round(time.time(), 1), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        # written after every step: a hang-up of the card loses nothing
        with open(os.path.join(args.evidence, f"close-e2e-{tag}.json"), "w") as out:
            json.dump({**record, "failures": failures, "partial": True}, out, indent=1)
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call
    code = lambda out: (out.get("error") or {}).get("code")

    def call_long(name, arguments, timeout):
        answer = adapter.request("tools/call", {"name": name, "arguments": arguments}, timeout=timeout)
        if "error" in answer:
            raise RuntimeError(f"{name}: protocol error {answer['error'].get('code')}")
        result = answer["result"]
        return (not result.get("isError", False)), result.get("structuredContent") or {}

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

    def settled(operation, seconds=180):
        def done():
            ok, got = call("deck_operation_get", {"operation_id": operation})
            return got if ok and got.get("state") in ("committed", "rejected", "ambiguous") else None
        return until(done, seconds) or {}

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-close-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        tools = adapter.request("tools/list")["result"]["tools"]
        by_name = {tool["name"]: tool for tool in tools}
        status = find_property(by_name.get("deck_worker_report", {}).get("inputSchema"), "status") or {}
        mode = find_property(by_name.get("deck_worker_close", {}).get("inputSchema"), "close_mode") or {}
        step("tools-list", len(tools) == TOOLS and "deck_worker_reports_ack" in by_name
             and "handoff" in (status.get("enum") or []) and mode.get("enum") == ["episode", "instance"],
             count=len(tools), status_enum=",".join(status.get("enum") or []), close_modes=",".join(mode.get("enum") or []))
        ok, caps = call("deck_capabilities", {})
        step("caller-is-a-card", ok and bool((caps.get("orchestratorCard") or {}).get("cardId")))
        work = ((caps.get("authorizedWorkspaces") or [{}])[0].get("roots") or [""])[0]
        ok, listed = call("deck_worker_list")
        lane = listed.get("laneEpoch")
        task = (f"Task {tag}. Step 1, before anything else: call the deck_worker_report tool with the text "
                f"HANDOFF-{tag} and fields {{\"status\": \"handoff\", \"task_ref\": \"{tag}\"}}. Step 2: then run "
                f"exactly this shell command in the foreground (not in the background) and wait until it finishes: "
                f"python3 phase1.py . Step 3: reply with exactly DONE-{tag}. Run no other command.")
        ok, started = call("deck_worker_start", {"request_id": f"cs_{tag}", "lane_epoch": lane, "type": "claude",
                                                 "text": task, "title": "close worker", "dir": work})
        done = settled(started.get("operationId")) if ok else {}
        card = (done.get("result") or {}).get("cardId")
        step("worker-started", bool(card), state=done.get("state"), code=code(started))
        if not card:
            return

        # ONE wait for the report (the adapter strings Deck's bounded waits)
        t_wait = time.time()
        ok, waited = call_long("deck_worker_wait", {"cards": [{"card_id": card, "episode": 0, "reports": 0}],
                                                    "until": "report", "wait_ms": 900_000, "include_reports": True}, 960)
        row = next((w for w in waited.get("workers", []) if w.get("cardId") == card), {}) if ok else {}
        inline = next((c for c in (waited.get("reportsInline") or {}).get("cards", []) if c.get("cardId") == card), {})
        given = (inline.get("reports") or [{}])[-1]
        decided = {"episode": (row.get("agent") or {}).get("episode"), "state": (row.get("agent") or {}).get("state"),
                   "instance": row.get("instance"), "lastLine": row.get("lastLine")}
        step("report-wait-returned", ok and waited.get("reason") == "report" and given.get("sequence") == 1
             and (given.get("fields") or {}).get("status") == "handoff" and given.get("text") == f"HANDOFF-{tag}",
             reason=waited.get("reason"), seconds=round(time.time() - t_wait, 1), sequence=given.get("sequence"),
             status=(given.get("fields") or {}).get("status"))
        step("decided-while-working", decided["state"] == "working" and bool(decided["instance"]),
             state=decided["state"], episode=decided["episode"], instance=bool(decided["instance"]))
        step("report-awaits-ack", (row.get("reports") or {}).get("awaitingAck") is True,
             acknowledged=(row.get("reports") or {}).get("acknowledged"))

        # the page reads the label while it says "not yet received"
        time.sleep(6)
        ok, acked = call("deck_worker_reports_ack", {"card_id": card, "sequence": given.get("sequence") or 0})
        row, _ = view(card)
        step("acknowledged", ok and acked.get("acknowledged") == 1 and acked.get("awaitingAck") is False
             and (row or {}).get("reports", {}).get("awaitingAck") is False, code=code(acked))
        ok, future = call("deck_worker_reports_ack", {"card_id": card, "sequence": 9})
        step("future-sequence-refused", not ok and code(future) == "REPORT_NOT_FOUND", code=code(future))

        # the worker ends its turn on its own: a new episode, the same instance
        ended = until(lambda: (lambda r: r if r and r["agent"]["state"] == "turn-done" else None)(view(card)[0]), 600, 2)
        step("worker-turn-ended", bool(ended) and ended["agent"]["episode"] != decided["episode"]
             and ended.get("instance") == decided["instance"],
             episode=(ended or {}).get("agent", {}).get("episode"), same_instance=(ended or {}).get("instance") == decided["instance"])
        ok, listed = call("deck_worker_list")
        lane = listed.get("laneEpoch")

        # the field symptom: the close bound to the episode of the decision
        ok_e, by_episode = call("deck_worker_close", {"request_id": f"ce_{tag}", "lane_epoch": lane, "card_id": card,
                                                      "after_episode": decided["episode"]})
        err = by_episode.get("error") or {}
        step("episode-close-refused", not ok_e and err.get("code") == "EPISODE_MOVED" and err.get("stage") == "precheck"
             and err.get("accepted") is False and err.get("currentEpisode") == (ended or {}).get("agent", {}).get("episode"),
             code=err.get("code"), stage=err.get("stage"), current=err.get("currentEpisode"),
             observed_state=(err.get("observed") or {}).get("state"))

        # the same decision bound to the instance: closes
        close_args = {"request_id": f"ci_{tag}", "lane_epoch": lane, "card_id": card, "close_mode": "instance",
                      "instance": decided["instance"]}
        if decided["lastLine"]:
            close_args["last_line"] = decided["lastLine"]
        ok_i, by_instance = call("deck_worker_close", close_args)
        step("instance-close-accepted", ok_i and by_instance.get("state") in ("accepted", "committed")
             and (by_instance.get("result") or {}).get("closeMode") == "instance",
             state=by_instance.get("state"), code=code(by_instance))
        final = settled(by_instance.get("operationId"), 240) if ok_i else {}
        step("instance-close-committed", final.get("state") == "committed", state=final.get("state"),
             code=final.get("code"), refusal=json.dumps((final.get("result") or {}).get("refusal"), sort_keys=True))
        ok_r, replay = call("deck_worker_close", close_args)
        step("replay-is-the-same-operation", ok_r and replay.get("operationId") == by_instance.get("operationId")
             and replay.get("state") == "committed", state=replay.get("state"))
        gone = until(lambda: view(card)[0] is None, 30)
        step("card-gone", bool(gone))
        record["card"] = card
    except Exception as error:  # noqa: BLE001 — the run's own failure is a step
        step("exception", False, error=type(error).__name__, detail=str(error)[:200])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["failures"] = failures
        with open(os.path.join(args.evidence, f"close-e2e-{tag}.json"), "w") as out:
            json.dump(record, out, indent=1)
        print("PASS" if not failures else f"FAIL {failures}", flush=True)
        sys.exit(0 if not failures else 1)


if __name__ == "__main__":
    main()
