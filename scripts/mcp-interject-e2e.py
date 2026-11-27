#!/usr/bin/env python3
"""Scripted orchestrator for the `worker-interject` WKWebView smoke
(app/SMOKE.md "Interjected worker lines", `deck_worker_interject`).

Run it INSIDE the page's plain-shell card "Orch I" (the operator types it
there after `interject-armed`): Deck reads which card a call comes from out
of the kernel, so the script names nothing about itself. `--adapter` must be
a DEBUG deck-mcp build (the --socket / --credential-fd seams).

The run, with one worker (Claude: the fixture's hooks, only the fixture's
`python3 phase1.py` allowed of the shell; Codex: `--type codex`):
  1. deck_capabilities names this card and says it may interject;
  2. one worker started from this card with a task in two phases: a long
     story in its reply (model output, no tool), then a 100 s foreground
     program of the fixture's (`phase1.py`, which writes `p1-started` and
     `p1-done`), then a file `result-<tag>.txt`;
  3. past the session gap after the first line, while the story is
     written: a QUEUED line (`deck_worker_send`: a later file) and an
     INTERJECTED line (`deck_worker_interject`: write
     `result-interject-<tag>.txt` instead). The interjected line is
     delivered during the output phase while the queued one waits at
     `quiet`;
  4. the request's identity: a replay is the same operation, the same id as
     a queued line is a conflict, a stale episode is EPISODE_MOVED, a
     control character is INVALID_ARGUMENTS;
  5. past the gap after it, while the program runs: a second interjected
     line (an extra file), delivered during the tool phase;
  6. the agent takes both in its own time: the interjected file name, not
     the original, and the extra file; the queued line goes after the
     turn and its file follows;
  7. an interjected line that needs the agent's permission dialog: the
     dialog comes (needs-input, said by PermissionRequest), and a further
     interjected line waits at `agent`, is not delivered, and the dialog is
     not answered (no file). Not with `--no-dialog` (Pi: Deck sees no
     question of Pi's, and a Pi worker's lines are never typed).

Prints closed facts only: codes, ids, states, stages, seconds and whether
files exist. Exit 0 when every expectation held, 1 otherwise.
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

GAP = 60


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--tag", default=str(int(time.time())))
    p.add_argument("--type", default="claude", choices=("claude", "codex", "pi"),
                   help="the worker type the page listed")
    p.add_argument("--no-dialog", action="store_true",
                   help="skip step 7: the agent raises no permission dialog Deck can see (Pi)")
    p.add_argument("--paragraphs", type=int, default=70, help="the story's length: long enough to outlast the gap")
    p.add_argument("--outside", help="a directory outside the worker's workspace: the line that needs the agent's "
                                     "permission writes there (Codex asks only outside its sandbox)")
    return p.parse_args()


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
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call
    code = lambda out: (out.get("error") or {}).get("code")
    sequence = 0

    def view(card):
        ok, out = call("deck_worker_list")
        row = next((w for w in out.get("workers", []) if w.get("cardId") == card), None) if ok else None
        return row, out

    def send_of(row, operation):
        return next((s for s in (row or {}).get("sends", []) if s.get("operationId") == operation), {})

    def until(test, seconds, every=1.0):
        deadline = time.time() + seconds
        while time.time() < deadline:
            value = test()
            if value:
                return value
            time.sleep(every)
        return None

    def committed(operation):
        def done():
            ok, got = call("deck_operation_get", {"operation_id": operation})
            return got if ok and got.get("state") in ("committed", "rejected", "ambiguous") else None
        return until(done, 180) or {}

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-interject-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        names = [tool["name"] for tool in adapter.request("tools/list")["result"]["tools"]]
        step("tools-list", len(names) == TOOLS and "deck_worker_interject" in names, count=len(names))
        ok, caps = call("deck_capabilities", {})
        card_of_caller = (caps.get("orchestratorCard") or {}).get("cardId")
        step("caller-is-a-card", ok and bool(card_of_caller) and caps.get("mayInterjectWorkers") is True,
             card=bool(card_of_caller), may=caps.get("mayInterjectWorkers"))
        work = ((caps.get("authorizedWorkspaces") or [{}])[0].get("roots") or [""])[0]
        ok, listed = call("deck_worker_list")
        sequence = listed.get("nextSendSequence", 0)
        # step 1 is model output (a long story, no tool); step 2 a 100 s
        # foreground program of the fixture's (`phase1.py` writes
        # `p1-started` and `p1-done`: which phase a line landed in is read
        # from them); Claude Code refuses a standalone `sleep`
        task = (f"Task {tag}: step 1: write, in your reply, a story about a lighthouse keeper in "
                f"{args.paragraphs} numbered paragraphs of about 80 words each; use no tool for this step. "
                f"Step 2: then run exactly this shell command in the foreground (not in the background) and wait "
                f"until it finishes: python3 phase1.py . Step 3: when it is done, create the file result-{tag}.txt "
                f"containing the word original. Then reply with exactly DONE-{tag}. Run no other command.")
        ok, started = call("deck_worker_start", {"request_id": f"is_{tag}", "send_sequence": sequence, "type": args.type,
                                                 "text": task, "title": "interject worker", "dir": work})
        sequence += 1
        done = committed(started.get("operationId")) if ok else {}
        card = (done.get("result") or {}).get("cardId")
        step("worker-started", bool(card), state=done.get("state"))
        if not card:
            return
        head = started.get("operationId")
        marker = lambda name: os.path.exists(os.path.join(work, name))
        phase = lambda: "tool" if marker("p1-started") and not marker("p1-done") else (
            "after-tool" if marker("p1-done") else "output")

        # the first line goes; the agent starts writing
        row = until(lambda: (lambda r: r if send_of(r, head).get("state") == "delivered" else None)(view(card)[0]), 300)
        head_at = time.time()
        step("first-line-delivered", bool(row))
        working = until(lambda: (lambda r: r if r and r["agent"]["state"] == "working" else None)(view(card)[0]), 120)
        step("agent-working", bool(working), episode=(working or {}).get("agent", {}).get("episode"))
        if not working:
            return

        def interject(request, text, episode):
            nonlocal sequence
            ok_i, out = call("deck_worker_interject", {"request_id": request, "send_sequence": sequence, "card_id": card,
                                                       "after_episode": episode, "text": text})
            sequence += 1
            return ok_i, out

        def delivered_in_phase(operation, limit=45):
            """Wait until the line is delivered; its phase and the agent's
            state at the first view that shows it delivered."""
            sent = time.time()
            r = until(lambda: (lambda v: v if send_of(v, operation).get("state") == "delivered" else None)(view(card)[0]),
                      limit, 0.5)
            return r, phase(), round(time.time() - sent, 1)

        # past the session gap after the first line, while the story is written
        time.sleep(max(0.0, head_at + GAP + 5 - time.time()))
        row, _ = view(card)
        episode = row["agent"]["episode"]
        queued_text = (f"Queued line {tag}: after everything else is done, create the file "
                       f"queued-{tag}.txt containing the word later.")
        ok_a, a = call("deck_worker_send", {"request_id": f"iq_{tag}", "send_sequence": sequence, "card_id": card,
                                            "after_episode": episode, "text": queued_text})
        sequence += 1
        interject_args = {"request_id": f"ii_{tag}", "send_sequence": sequence, "card_id": card, "after_episode": episode,
                          "text": (f"INTERJECT {tag}: change of plan for step 3: create the file result-interject-{tag}.txt "
                                   f"instead of result-{tag}.txt, with the same content. Do not create result-{tag}.txt.")}
        phase_at_send = phase()
        ok_b, b = call("deck_worker_interject", interject_args)
        sequence += 1
        step("lines-accepted", ok_a and ok_b and a.get("state") == "committed" and b.get("state") == "committed"
             and (b.get("result") or {}).get("mode") == "interject" and not (a.get("result") or {}).get("mode"),
             queued=a.get("state"), interjected=b.get("state"), mode=(b.get("result") or {}).get("mode"),
             phase=phase_at_send)
        qa, qb = a.get("operationId"), b.get("operationId")
        row, _ = view(card)
        first_look = {"queued": send_of(row, qa), "interjected": send_of(row, qb)}
        row, landed, seconds = delivered_in_phase(qb)
        b_at = time.time()
        step("interjected-during-output", bool(row) and row["agent"]["state"] == "working" and landed == "output"
             and send_of(row, qa).get("state") == "queued" and send_of(row, qb).get("mode") == "interject",
             seconds=seconds, phase=landed, agent=(row or {}).get("agent", {}).get("state"),
             queued_waiting=send_of(row or {}, qa).get("waiting"), first_look=json.dumps(first_look, sort_keys=True))
        # a Pi worker line waits for the pause between two of Pi's runs: Pi's
        # form of the quiet time (its stage `pi-handover`)
        quiet_words = ("quiet", "pi-handover") if args.type == "pi" else ("quiet",)
        step("queued-waited-for-quiet", first_look["queued"].get("waiting") in quiet_words
             and first_look["queued"].get("mode") == "queued", waiting=first_look["queued"].get("waiting"))

        # the request's identity
        ok_r, replay = call("deck_worker_interject", interject_args)
        step("replay-is-the-same-operation", ok_r and replay.get("operationId") == qb)
        ok_c, conflict = call("deck_worker_send", interject_args)
        step("same-id-as-a-queued-line-conflicts", not ok_c and code(conflict) == "REQUEST_ID_CONFLICT", code=code(conflict))
        ok_s, stale = call("deck_worker_interject", {**interject_args, "request_id": f"ix_{tag}", "send_sequence": sequence,
                                                     "after_episode": episode + 1000})
        step("a-stale-episode-is-refused", not ok_s and code(stale) == "EPISODE_MOVED", code=code(stale))
        ok_e, escaped = call("deck_worker_interject", {**interject_args, "request_id": f"ie_{tag}", "send_sequence": sequence,
                                                       "text": "go\u001b[201~"})
        step("control-characters-are-refused", not ok_e and code(escaped) == "INVALID_ARGUMENTS", code=code(escaped))

        # a second interjected line while the program of step 2 runs: past
        # the gap after the first one, once the program started
        until(lambda: time.time() >= b_at + GAP + 5 and phase() != "output", 400, 1)
        row, _ = view(card)
        phase_at_send = phase()
        ok_t, t = interject(f"it_{tag}", (f"INTERJECT {tag}: also, after step 3, create the file extra-{tag}.txt "
                                          f"containing the word extra."), row["agent"]["episode"])
        row, landed, seconds = delivered_in_phase(t.get("operationId"))
        step("interjected-during-tool", ok_t and bool(row) and row["agent"]["state"] == "working" and landed == "tool"
             and send_of(row, qa).get("state") == "queued",
             seconds=seconds, phase=landed, sent_in=phase_at_send, agent=(row or {}).get("agent", {}).get("state"))

        # the agent reads them in its own time: the interjected names, not the original
        ended = until(lambda: (lambda r: r if r and r["agent"]["state"] == "turn-done" else None)(view(card)[0]), 600, 2)
        files = lambda: {name: os.path.exists(os.path.join(work, name))
                         for name in (f"result-interject-{tag}.txt", f"result-{tag}.txt", f"extra-{tag}.txt",
                                      f"queued-{tag}.txt")}
        until(lambda: files()[f"result-interject-{tag}.txt"] and files()[f"extra-{tag}.txt"], 60)
        seen = files()
        step("the-agent-changed-its-plan", bool(ended) and seen[f"result-interject-{tag}.txt"] and not seen[f"result-{tag}.txt"]
             and seen[f"extra-{tag}.txt"], ended=bool(ended), files=json.dumps(seen, sort_keys=True))
        # the queued line goes after the turn (quiet, then the gap), and its file follows
        row = until(lambda: (lambda r: r if send_of(r, qa).get("state") == "delivered" else None)(view(card)[0]), 300, 2)
        queued_after = round(time.time() - b_at, 1)
        until(lambda: files()[f"queued-{tag}.txt"], 180, 2)
        step("the-queued-line-went-later", bool(row) and files()[f"queued-{tag}.txt"], seconds=queued_after)

        if args.no_dialog:
            record["card"] = card
            return
        # a dialog of the agent's: a line that needs permission, then one that must wait
        settled = until(lambda: (lambda r: r if r and r["agent"]["state"] == "turn-done" else None)(view(card)[0]), 240, 2)
        episode = (settled or view(card)[0])["agent"]["episode"]
        # a command the agent must ask for: Claude Code's accept-edits mode
        # approves file commands in its folder by itself, so a program of
        # its own; Codex asks for a write outside its sandbox
        perm = os.path.join(args.outside or work, f"perm-{tag}.txt")
        command = (f"touch {perm}" if args.outside
                   else f"python3 -c \"open('{os.path.basename(perm)}', 'w').close()\"")
        ok_p, ask = call("deck_worker_interject", {"request_id": f"ip_{tag}", "send_sequence": sequence, "card_id": card,
                                                   "after_episode": episode,
                                                   "text": f"Task {tag}: run exactly this shell command: {command}"})
        sequence += 1
        asking = until(lambda: (lambda r: r if r and r["agent"]["state"] == "needs-input" else None)(view(card)[0]), 240, 1)
        step("the-agent-asks", ok_p and bool(asking), state=(asking or {}).get("agent", {}).get("state"))
        if asking:
            ok_h, held = call("deck_worker_interject", {"request_id": f"ih_{tag}", "send_sequence": sequence, "card_id": card,
                                                        "after_episode": asking["agent"]["episode"],
                                                        "text": f"INTERJECT {tag}: also reply with the word HELD."})
            sequence += 1
            # the session gap after the line that raised the dialog runs out
            # first; from then on the agent's question alone holds the line
            at_agent = until(lambda: (lambda r: r if send_of(r, held.get("operationId")).get("waiting") == "agent"
                                      else None)(view(card)[0]), 120, 1)
            time.sleep(30)
            row, _ = view(card)
            line = send_of(row, held.get("operationId"))
            step("held-while-the-agent-asks", ok_h and bool(at_agent) and line.get("state") == "queued"
                 and line.get("waiting") == "agent" and row["agent"]["state"] == "needs-input" and not os.path.exists(perm),
                 reached_agent=bool(at_agent), state=line.get("state"), waiting=line.get("waiting"),
                 agent=row["agent"]["state"], dialog_answered=os.path.exists(perm))
        record["card"] = card
    except Exception as error:  # noqa: BLE001 — the run's own failure is a step
        step("exception", False, error=type(error).__name__, detail=str(error)[:200])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["failures"] = failures
        with open(os.path.join(args.evidence, f"interject-e2e-{tag}.json"), "w") as out:
            json.dump(record, out, indent=1)
        print("PASS" if not failures else f"FAIL {failures}", flush=True)
        # here, so that a step that ends the run early still exits by its result
        sys.exit(0 if not failures else 1)


if __name__ == "__main__":
    main()
