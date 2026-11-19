#!/usr/bin/env python3
"""Scripted orchestrator for the `projects` WKWebView smoke (app/SMOKE.md
"One client, all projects").

Run it INSIDE an orchestrator card's pane ("Orch A" in project A, "Orch B"
in project B; the page opened both after `projects-armed`). `--adapter`
must be a DEBUG deck-mcp build (the --socket / --credential-fd seams).

Phase `first`: deck_capabilities says allProjects, lists A and B among
authorizedWorkspaces (each with its default directory) and the first
project, which has none, in projectsWithoutDirectory; a start named there
is refused; a start without a project opens in this card's project, in
that project's directory. In "Orch A" only, where its worker cards open
(the client may organize cards, so it may name a column): a start in B
naming no column decides none (the page sees it where the user's new cards
open in B), one of A's columns for B is COLUMN_NOT_FOUND listing exactly
B's columns, and B's third column named is the one the start records.
Phase `close` (in "Orch A", once the page shows "A to B queued" in a pane):
B's worker and "Orch B" itself are WORKER_NOT_FOUND (not cards A started),
the shown card's close is accepted and then rejected `card-shown`, and "A to
B" closes (committed). The shown card goes first and is settled before the
second close, so the page's own wait cannot race it.
Phase `after` (in "Orch B", once the page narrowed the client to A): A
alone is reached, a start from B's card is refused, and B's own worker
reads as taken over by the user.

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
Adapter, PROTOCOL = agent_e2e.Adapter, agent_e2e.PROTOCOL


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--phase", choices=("first", "close", "after"), default="first")
    p.add_argument("--tag", default=str(int(time.time())))
    return p.parse_args()


def main():
    args = parse()
    record = {"phase": args.phase, "steps": []}
    failures = []
    os.makedirs(args.evidence, exist_ok=True)

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call
    code = lambda out: (out.get("error") or {}).get("code")
    role = "?"
    try:
        adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                       "clientInfo": {"name": "deck-projects-e2e", "version": "1"}})
        adapter.notify("notifications/initialized")
        ok, caps = call("deck_capabilities")
        card = caps.get("orchestratorCard") or {}
        role = card.get("title", "?")[-1:]
        spaces = caps.get("authorizedWorkspaces") or []
        ok_l, listed = call("deck_worker_list")

        def start(request, project=None, column=None, title=None):
            # this card's lane moves with every accepted start: read it each time
            sequence = call("deck_worker_list")[1].get("nextSendSequence", 0)
            return call("deck_worker_start", dict(
                {"request_id": f"{request}_{role}_{args.tag}", "send_sequence": sequence, "type": "claude",
                 "title": title or f"{role} worker", "text": f"Orchestrator {role}: say hello."},
                **({"project_id": project} if project else {}), **({"column_id": column} if column else {})))
        if args.phase == "first":
            without = caps.get("projectsWithoutDirectory") or []
            step("all-projects", ok and caps.get("allProjects") is True and len(spaces) >= 2 and len(without) >= 1
                 and all(s.get("roots") for s in spaces), reached=len(spaces), without=len(without))
            step("orchestrator-card", role in ("A", "B"), role=role)
            ok, refused = start("nodir", without[0] if without else "none")
            step("no-directory-refused", not ok and code(refused) == "PERMISSION_DENIED", code=code(refused))
            ok, started = start("start")
            result = started.get("result") or {}
            mine = next((s for s in spaces if s["projectId"] == result.get("projectId")), {})
            step("started-in-own-project", ok and bool(mine) and result.get("dir") == (mine.get("roots") or [""])[0]
                 and result.get("dir", "").endswith(f"/work/{role.lower()}"), dir=os.path.basename(result.get("dir", "")))
            if role == "A":
                ok_s, snap = call("deck_cards_snapshot")
                layout = {p["projectId"]: [c["columnId"] for c in p.get("columns", [])] for p in snap.get("projects", [])}
                other = next((s["projectId"] for s in spaces
                              if (s.get("roots") or [""])[0].endswith("/work/b")), None)
                a_columns, b_columns = layout.get(result.get("projectId"), []), layout.get(other, [])
                step("columns-visible", ok_s and len(a_columns) >= 1 and len(b_columns) >= 3,
                     a=len(a_columns), b=len(b_columns))
                ok, cross = start("cross", other, title="A to B")
                placed = cross.get("result") or {}
                step("cross-project-no-column", ok and placed.get("projectId") == other
                     and placed.get("columnId") is None, decided=placed.get("columnId") is not None)
                ok, wrong = start("wrong", other, column=(a_columns or ["none"])[0])
                offered = [c.get("columnId") for c in (wrong.get("error") or {}).get("columns") or []]
                step("other-projects-column-refused", not ok and code(wrong) == "COLUMN_NOT_FOUND"
                     and offered == b_columns, code=code(wrong), offered=len(offered))
                third = b_columns[2] if len(b_columns) >= 3 else "none"
                ok, named = start("named", other, column=third, title="A to B queued")
                placed = named.get("result") or {}
                step("named-column", ok and placed.get("projectId") == other and placed.get("columnId") == third,
                     state=named.get("state"))
        elif args.phase == "close":
            ok_s, snap = call("deck_cards_snapshot")
            by_title = {c.get("title"): c.get("cardId") for c in snap.get("cards", [])}
            mine = {w.get("title"): w for w in call("deck_worker_list")[1].get("workers", [])}

            def close(request, card, episode=0):
                sequence = call("deck_worker_list")[1].get("nextSendSequence", 0)
                return call("deck_worker_close", {"request_id": f"{request}_{role}_{args.tag}", "send_sequence": sequence,
                                                  "card_id": card or "none", "after_episode": episode})

            def settle(out, seconds=90):
                deadline = time.time() + seconds
                while time.time() < deadline:
                    got = call("deck_operation_get", {"operation_id": out.get("operationId")})[1]
                    if got.get("state") in ("committed", "rejected", "ambiguous"):
                        return got
                    time.sleep(1)
                return {}

            episode = lambda title: ((mine.get(title) or {}).get("agent") or {}).get("episode", 0)
            step("close-cards-visible", ok_s and all(t in by_title for t in ("B worker", "Orch B", "A to B", "A to B queued"))
                 and all(t in mine for t in ("A to B", "A to B queued")), cards=len(by_title))
            ok, out = close("closeb", by_title.get("B worker"))
            step("other-orchestrators-card-refused", not ok and code(out) == "WORKER_NOT_FOUND", code=code(out))
            ok, out = close("closeo", by_title.get("Orch B"))
            step("orchestrator-card-refused", not ok and code(out) == "WORKER_NOT_FOUND", code=code(out))
            ok, out = close("closes", by_title.get("A to B queued"), episode("A to B queued"))
            got = settle(out) if ok else {}
            step("shown-card-kept", ok and got.get("state") == "rejected" and got.get("code") == "card-shown",
                 state=got.get("state"), code=got.get("code"))
            ok, out = close("closec", by_title.get("A to B"), episode("A to B"))
            got = settle(out) if ok else {}
            step("own-card-closed", ok and got.get("state") == "committed", state=got.get("state"))
        else:
            step("narrowed", ok and caps.get("allProjects") is False and len(spaces) == 1, reached=len(spaces))
            ok, refused = start("after")
            step("out-of-reach-refused", not ok and code(refused) == "PERMISSION_DENIED", code=code(refused))
            own = [w for w in listed.get("workers", []) if w.get("title") == f"{role} worker"]
            step("own-worker-held", ok_l and len(own) == 1 and own[0].get("takenOver") is True)
    except Exception as error:  # a failed step is reported, never hidden
        step("exception", False, kind=type(error).__name__, text=str(error)[:160])
    finally:
        adapter.close()
        record["failures"] = failures
        with open(os.path.join(args.evidence, f"projects-e2e-{role}-{args.phase}-{args.tag}.json"), "w") as out:
            json.dump(record, out, indent=2)
    print("PASS" if not failures else f"FAIL {len(failures)}: {','.join(failures)}", flush=True)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
