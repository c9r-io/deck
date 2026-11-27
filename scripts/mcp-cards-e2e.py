#!/usr/bin/env python3
"""Scripted card-snapshot reader for an isolated Deck smoke run (app/SMOKE.md
"Card snapshot through MCP", WKWebView mode `cards-mcp`).

Deck must already be open in that mode and past `cards-armed`: the page
authorized one client that may read the card snapshot and opened three
cards in its project: "Ask" (a real Claude Code whose turn ended on a
question), "Done" (one whose turn ended on `CARDS-DONE-OK`) and "Shell" (a
plain shell). `--adapter` must be a DEBUG deck-mcp build (the --socket /
--credential-fd seams).

The run: tools/list (32 tools); deck_capabilities says mayReadCards; the
snapshot lists the three, Claude Code's last words fenced (Done's carries
the token, Ask's ends on a question), the shell `no-agent`; the whole reply
of Done carries the token, a wrong episode is EPISODE_MOVED, an unknown card
CARD_NOT_FOUND; an unknown mark word is refused, the shell cannot be marked
(no episode); Ask is marked needs-you and Done can-close, and the snapshot
carries both as this client's own. Then the page withdraws the choice: the
client waits until it is PERMISSION_DENIED.

Prints closed facts only (codes, counts, seconds), never a reply. Exit 0
when every expectation held, 1 otherwise, 2 for usage.
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
TOOLS = 39
FENCE = "<external_message "
TOKEN = "CARDS-DONE-OK"


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--codex", action="store_true",
                   help="the page also opened \"Codex done\": a codex --no-daemon card forwarding its final message")
    p.add_argument("--withdrawn-wait", type=int, default=600,
                   help="seconds to wait for the page to withdraw the choice")
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

    def call(name, arguments=None):
        return adapter.call(name, arguments)

    def code(out):
        return (out.get("error") or {}).get("code")

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-cards-e2e", "version": "1"}})
        adapter.notify("notifications/initialized")
        step("initialize", "result" in init)
        tools = adapter.request("tools/list")["result"]["tools"]
        names = {t["name"] for t in tools}
        step("tools-list", len(tools) == TOOLS and {"deck_cards_snapshot", "deck_card_reply", "deck_card_mark"} <= names,
             tools=len(tools))
        mark_tool = next((t for t in tools if t["name"] == "deck_card_mark"), {})
        snap_tool = next((t for t in tools if t["name"] == "deck_cards_snapshot"), {})
        step("annotations", mark_tool.get("annotations", {}).get("readOnlyHint") is False
             and snap_tool.get("annotations", {}).get("readOnlyHint") is True)

        ok, caps = call("deck_capabilities")
        step("capabilities", ok and caps.get("mayReadCards") is True)

        t0 = time.time()
        ok, snap = call("deck_cards_snapshot")
        cards = snap.get("cards", []) if ok else []
        by = {c.get("title"): c for c in cards}
        step("snapshot", ok and {"Ask", "Done", "Shell"} <= set(by) and snap.get("nextCursor") is None,
             cards=len(cards), seconds=round(time.time() - t0, 2))
        ask, done, shell = by.get("Ask", {}), by.get("Done", {}), by.get("Shell", {})
        for name, card in (("ask", ask), ("done", done)):
            agent = card.get("agent") or {}
            step(f"{name}-observed", agent.get("kind") == "claude-code" and agent.get("state") == "turn-done"
                 and isinstance(agent.get("episode"), int) and isinstance(agent.get("since"), int),
                 state=agent.get("state"))
        done_end = (done.get("reply") or {}).get("end", "")
        ask_end = (ask.get("reply") or {}).get("end", "")
        step("done-reply-fenced", TOKEN in done_end and done_end.startswith(FENCE))
        step("ask-reply-question", ask_end.startswith(FENCE) and "?" in ask_end.rsplit("</external_message>", 1)[0][-200:])
        step("shell-no-agent", (shell.get("reply") or {}).get("unavailable") == "no-agent"
             and shell.get("agent") is None and shell.get("alive") is True)
        step("not-self", not any(c.get("self") for c in cards))
        if args.codex:
            codex = by.get("Codex done", {})
            agent = codex.get("agent") or {}
            end = (codex.get("reply") or {}).get("end", "")
            step("codex-reply", agent.get("kind") == "codex" and agent.get("state") == "turn-done"
                 and "CODEX-DONE-OK" in end and end.startswith(FENCE), kind=agent.get("kind"),
                 reason=(codex.get("reply") or {}).get("unavailable"))
            ok, whole = call("deck_card_reply", {"card_id": codex.get("cardId", ""), "episode": agent.get("episode", 0)})
            step("codex-reply-whole", ok and "CODEX-DONE-OK" in whole.get("text", ""))
            ok, _ = call("deck_card_mark", {"card_id": codex.get("cardId", ""), "episode": agent.get("episode", 0),
                                            "mark": "can-close", "note": "Codex reported it finished"})
            step("codex-marked", ok)

        episode = (done.get("agent") or {}).get("episode", 0)
        ok, whole = call("deck_card_reply", {"card_id": done.get("cardId", ""), "episode": episode})
        step("reply-whole", ok and TOKEN in whole.get("text", "") and whole.get("text", "").startswith(FENCE))
        ok, moved = call("deck_card_reply", {"card_id": done.get("cardId", ""), "episode": episode + 1_000_000})
        step("reply-episode-moved", not ok and code(moved) == "EPISODE_MOVED", code=code(moved))
        ok, missing = call("deck_card_reply", {"card_id": "no-such-card", "episode": 1})
        step("reply-unknown-card", not ok and code(missing) == "CARD_NOT_FOUND", code=code(missing))

        ok, bad = call("deck_card_mark", {"card_id": done.get("cardId", ""), "episode": episode, "mark": "close-now"})
        step("mark-bad-word", not ok and code(bad) == "INVALID_ARGUMENTS", code=code(bad))
        ok, shell_mark = call("deck_card_mark", {"card_id": shell.get("cardId", ""), "episode": 0, "mark": "can-close"})
        step("mark-shell-refused", not ok and code(shell_mark) == "EPISODE_MOVED", code=code(shell_mark))
        ask_episode = (ask.get("agent") or {}).get("episode", 0)
        ok_a, _ = call("deck_card_mark", {"card_id": ask.get("cardId", ""), "episode": ask_episode,
                                          "mark": "needs-you", "note": "Asks whether to update the iOS app too"})
        ok_d, _ = call("deck_card_mark", {"card_id": done.get("cardId", ""), "episode": episode,
                                          "mark": "can-close", "note": "Reported it finished"})
        step("marked", ok_a and ok_d)
        ok, again = call("deck_cards_snapshot")
        marks = {c.get("title"): c.get("mark") for c in again.get("cards", [])} if ok else {}
        step("snapshot-marks", (marks.get("Ask") or {}).get("mark") == "needs-you" and (marks.get("Ask") or {}).get("mine") is True
             and (marks.get("Done") or {}).get("mark") == "can-close" and marks.get("Shell") is None)

        # the page clears Ask's mark and then withdraws the choice
        deadline = time.time() + args.withdrawn_wait
        denied = None
        while time.time() < deadline:
            ok, out = call("deck_cards_snapshot")
            if not ok:
                denied = code(out)
                break
            time.sleep(1)
        step("withdrawn-denied", denied == "PERMISSION_DENIED", code=denied)
    except Exception as error:  # noqa: BLE001 — a crash is a failure with its class
        step("exception", False, kind=type(error).__name__, detail=str(error)[:120])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
    record["finishedAt"] = int(time.time())
    record["failures"] = failures
    os.makedirs(args.evidence, exist_ok=True)
    with open(os.path.join(args.evidence, "mcp-cards-e2e.json"), "w") as out:
        json.dump(record, out, indent=2)
    print(("PASS" if not failures else "FAIL") + f" {len(record['steps']) - len(failures)}/{len(record['steps'])}")
    sys.exit(0 if not failures else 1)


if __name__ == "__main__":
    main()
