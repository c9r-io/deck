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
that project's directory. Phase `after` (in "Orch B", once the page
narrowed the client to A): A alone is reached, a start from B's card is
refused, and B's own worker reads as taken over by the user.

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
    p.add_argument("--phase", choices=("first", "after"), default="first")
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
        seq = listed.get("nextSendSequence", 0)
        start = lambda request, project=None: call("deck_worker_start", dict(
            {"request_id": f"{request}_{role}_{args.tag}", "send_sequence": seq, "type": "claude",
             "title": f"{role} worker", "text": f"Orchestrator {role}: say hello."},
            **({"project_id": project} if project else {})))
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
