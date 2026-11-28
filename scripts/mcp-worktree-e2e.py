#!/usr/bin/env python3
"""Scripted orchestrator for the `worktree-switch-relaunch` WKWebView smoke
(app/SMOKE.md "Linked-worktree switch"), run INSIDE the page's "Orch W" card.
`--adapter` must be a DEBUG deck-mcp build (the --socket / --credential-fd
seams).

Before it runs, the runner made (with git, a test tool; Deck runs none) a
linked worktree of the fixture's repository at <fixture>/disk2/wt1 and one of
the second repository at <fixture>/disk3/wt2, both AFTER the switch was saved
and Deck relaunched, plus a separate clone at <fixture>/disk2/clone.

The run: the real tools/list says how starts take linked worktrees;
deck_capabilities says the switch is on, anchors both projects ("Repo", and
"Later", which was made after the switch) and keeps authorizedWorkspaces the
project directories; a worker start in wt1 and one in wt2/src, neither naming
a project, are committed in their projects; a start in the clone is refused
(worktree-unverified, separate-repository) and one in a plain folder
(outside-scope). Exit 0 when every expectation held, 1 otherwise.
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
    for name in ("adapter", "socket", "client-id", "credential-file", "evidence", "fixture"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--tag", default=str(int(time.time())))
    return p.parse_args()


def main():
    args = parse()
    tag, fx = args.tag, args.fixture
    record = {"startedAt": int(time.time()), "tag": tag, "steps": []}
    failures = []
    os.makedirs(args.evidence, exist_ok=True)
    out_file = os.path.join(args.evidence, f"worktree-e2e-{tag}.json")

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), "at": round(time.time(), 1), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        with open(out_file, "w") as out:
            json.dump({**record, "failures": failures, "partial": True}, out, indent=1)
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    call = adapter.call
    code = lambda out: (out.get("error") or {}).get("code")

    def settled(operation, seconds=180):
        deadline = time.time() + seconds
        while time.time() < deadline:
            ok, got = call("deck_operation_get", {"operation_id": operation})
            if ok and got.get("state") in ("committed", "rejected", "ambiguous"):
                return got
            time.sleep(1)
        return {}

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-worktree-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        tools = {t["name"]: t for t in adapter.request("tools/list")["result"]["tools"]}
        start_text = tools.get("deck_worker_start", {}).get("description", "")
        step("tools-list", len(tools) == TOOLS and "linked worktree" in start_text and "autoLinkedWorktrees" in start_text,
             count=len(tools))
        ok, caps = call("deck_capabilities", {})
        auto = caps.get("autoLinkedWorktrees") or {}
        anchored = sorted(p.get("root") for p in auto.get("projects", []) if p.get("anchored"))
        roots = sorted(r for w in caps.get("authorizedWorkspaces", []) for r in w.get("roots", []))
        step("capabilities", ok and auto.get("enabled") is True and f"{fx}/repo" in anchored and f"{fx}/repo2" in anchored
             and not any("/disk" in r for r in roots) and bool((caps.get("orchestratorCard") or {}).get("cardId")),
             enabled=auto.get("enabled"), anchored=",".join(anchored), roots=",".join(roots))
        projects = {p.get("root"): p.get("projectId") for p in auto.get("projects", [])}

        def start(request, directory):
            ok_l, listed = call("deck_worker_list")
            ok_s, out = call("deck_worker_start", {"request_id": f"{request}_{tag}", "lane_epoch": listed.get("laneEpoch"),
                                                   "type": "claude", "title": request, "text": f"Task {tag}: say hi.",
                                                   "dir": directory})
            return ok_s, out

        for name, directory, repo in (("wt1", f"{fx}/disk2/wt1", f"{fx}/repo"), ("wt2", f"{fx}/disk3/wt2/src", f"{fx}/repo2")):
            ok_s, out = start(name, directory)
            done = settled(out.get("operationId")) if ok_s else {}
            result = done.get("result") or {}
            step(f"start-{name}", ok_s and out.get("result", {}).get("cwdGrant") == "auto-worktree"
                 and done.get("state") == "committed" and result.get("projectId") == projects.get(repo)
                 and result.get("dir") == directory and (result.get("launch") or {}).get("grant") == "auto-worktree",
                 state=done.get("state"), code=code(out) or done.get("code"), project_ok=result.get("projectId") == projects.get(repo),
                 launch=json.dumps(result.get("launch"), sort_keys=True))
        ok_c, clone = start("clone", f"{fx}/disk2/clone")
        err = clone.get("error") or {}
        step("clone-refused", not ok_c and err.get("code") == "PATH_NOT_ACCESSIBLE" and err.get("reason") == "worktree-unverified"
             and err.get("detail") == "separate-repository", code=err.get("code"), reason=err.get("reason"), detail=err.get("detail"))
        ok_p, plain = start("plain", f"{fx}/plain")
        err = plain.get("error") or {}
        step("plain-refused", not ok_p and err.get("reason") == "outside-scope", code=err.get("code"), reason=err.get("reason"))
    except Exception as error:  # noqa: BLE001 — the run's own failure is a step
        step("exception", False, error=type(error).__name__, detail=str(error)[:200])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["failures"] = failures
        with open(out_file, "w") as out:
            json.dump(record, out, indent=1)
        print("PASS" if not failures else f"FAIL {failures}", flush=True)
        sys.exit(0 if not failures else 1)


if __name__ == "__main__":
    main()
