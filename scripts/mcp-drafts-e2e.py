#!/usr/bin/env python3
"""Scripted drafting client for an isolated Deck smoke run (app/SMOKE.md
"Worker cards", WKWebView mode `workers`, scenario `drafts`).

Deck must already be open in the `workers` mode, scenario `drafts`, and past
`workers-armed`. `--adapter` must be a DEBUG deck-mcp build (the --socket /
--credential-fd seams); the credential goes on an inherited descriptor.

The run: tools/list (24 tools); deck_config_describe (the project, its
column, workflow and examples); a draft with problems is refused with a
field and a fix each (validate_only, then a submission: nothing recorded);
a valid draft previews, is submitted once (a replay returns the same
proposal, an identical second submission is DUPLICATE_PROPOSAL), and the
page approves it: deck_automation_list then lists the rule. A change to that
rule is submitted and the page saves it for editing (`edited`); a third
draft is rejected (`rejected`); a fourth waits while the page withdraws the
draft choice: it becomes `withdrawn`, deck_config_describe answers
PERMISSION_DENIED, and once the page grants the choice again the fourth
still reads `withdrawn`.

Prints closed facts only (codes, ids, statuses, counts, seconds). Exit 0
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
Adapter, PROTOCOL, TOOLS = agent_e2e.Adapter, agent_e2e.PROTOCOL, agent_e2e.TOOLS


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "work", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--tag", default=str(int(time.time())), help="unique suffix of request ids")
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
    call = adapter.call

    def code(out):
        return (out.get("error") or {}).get("code")

    def status_until(proposal, wanted, seconds):
        deadline = time.time() + seconds
        last = None
        while time.time() < deadline:
            ok, out = call("deck_config_proposal_get", {"proposal_id": proposal})
            last = out.get("status") if ok else code(out)
            if last == wanted:
                return True, last
            time.sleep(3)
        return False, last

    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-drafts-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        instructions = init.get("result", {}).get("instructions", "")
        step("instructions", "deck_config_describe" in instructions and "validate_only" in instructions)
        adapter.notify("notifications/initialized")
        names = [tool["name"] for tool in adapter.request("tools/list")["result"]["tools"]]
        step("tools-list", len(names) == TOOLS and all(n in names for n in
             ("deck_config_describe", "deck_config_propose", "deck_config_proposal_get")), count=len(names))
        ok, caps = call("deck_capabilities")
        step("capabilities", ok and caps.get("mayDraftConfig") is True)

        ok, described = call("deck_config_describe")
        project = (described.get("projects") or [{}])[0]
        column = ((project.get("columns") or [{}])[0]).get("id")
        step("describe", ok and bool(column) and len(described.get("examples", [])) >= 2
             and len(described.get("workflow", [])) >= 4, columns=len(project.get("columns", [])))
        pid = project.get("project_id")

        # problems: each with its field and fix; a submission records nothing
        bad = {"project_id": pid, "template": {"name": "Code review", "steps": ["{{msg.text}}"]},
               "rule": {"automation_id": "code-review", "column_id": column, "cmd": "codex", "template": "Code review"}}
        ok, out = call("deck_config_propose", {**bad, "validate_only": True})
        codes = sorted(p.get("code") for p in out.get("problems", []))
        step("validate-problems", ok and out.get("valid") is False
             and "STEP_LEADS_WITH_MESSAGE" in codes and "COMMAND_CODEX_DAEMON" in codes
             and all(p.get("field") and p.get("fix") for p in out.get("problems", [])), codes=",".join(codes))
        ok, out = call("deck_config_propose", {**bad, "request_id": f"req_bad_{tag}"})
        step("submit-invalid-refused", not ok and code(out) == "INVALID_DRAFT")

        # a valid draft: preview, one record, replay and duplicate
        good = {"project_id": pid, "note": "Reviews pull requests in three steps",
                "template": {"name": "Code review", "steps": [
                    "Review request: {{msg.text}} ({{msg.link}}). Read the change.",
                    "Run the tests and linters; list failures with file and line.",
                    "Write the review to review.md: blocking issues first."],
                    "follow_ups": ["Stop here; wait for me."]},
                "rule": {"automation_id": "code-review", "name": "Code review", "column_id": column,
                         "dir": args.work, "cmd": "claude", "template": "Code review",
                         "auto_continue": True, "external": True, "first_send": True}}
        ok, out = call("deck_config_propose", {**good, "validate_only": True})
        step("validate-ok", ok and out.get("valid") is True
             and len(out.get("preview", {}).get("whatHappens", [])) >= 3)
        ok, first = call("deck_config_propose", {**good, "request_id": f"req_good_{tag}"})
        proposal = first.get("proposalId")
        step("submitted", ok and first.get("status") == "pending" and bool(proposal))
        ok, again = call("deck_config_propose", {**good, "request_id": f"req_good_{tag}"})
        step("replayed", ok and again.get("proposalId") == proposal)
        ok, dup = call("deck_config_propose", {**good, "request_id": f"req_dup_{tag}"})
        step("duplicate-refused", not ok and code(dup) == "DUPLICATE_PROPOSAL"
             and (dup.get("error") or {}).get("proposalId") == proposal)

        # the page approves it
        started = time.time()
        done, last = status_until(proposal, "approved", 600)
        step("approved", done, status=last, seconds=int(time.time() - started))
        ok, listed = call("deck_automation_list")
        auto = next((a for a in listed.get("automations", []) if a.get("id") == "code-review"), None)
        step("listed", ok and auto is not None and bool(auto.get("revision")))

        # a change to that rule: the page saves it for editing
        change = {"project_id": pid, "rule": {**good["rule"], "cmd": "claude --permission-mode acceptEdits"}}
        ok, out = call("deck_config_propose", {**change, "request_id": f"req_change_{tag}"})
        changed = out.get("proposalId")
        fields = [c.get("field") for c in out.get("preview", {}).get("changes", [])]
        step("change-submitted", ok and "rule.cmd" in fields, fields=",".join(fields))
        done, last = status_until(changed, "edited", 600)
        step("edited", done, status=last)

        # a third: rejected
        third = {"project_id": pid, "template": {"name": "Scratch", "steps": ["Say hello."]}}
        ok, out = call("deck_config_propose", {**third, "request_id": f"req_third_{tag}"})
        rejected = out.get("proposalId")
        step("third-submitted", ok and bool(rejected))
        done, last = status_until(rejected, "rejected", 600)
        step("rejected", done, status=last)

        # a fourth waits while the page withdraws the choice
        fourth = {"project_id": pid, "template": {"name": "Later", "steps": ["Wait."]}}
        ok, out = call("deck_config_propose", {**fourth, "request_id": f"req_fourth_{tag}"})
        waiting = out.get("proposalId")
        step("fourth-submitted", ok and bool(waiting))
        deadline = time.time() + 600
        denied = None
        while time.time() < deadline:
            ok, out = call("deck_config_describe")
            if not ok and code(out) == "PERMISSION_DENIED":
                denied = out
                break
            time.sleep(1)
        step("withdrawn-denied", denied is not None)
        # once the page grants the choice again the fourth reads withdrawn
        done, last = status_until(waiting, "withdrawn", 120)
        step("withdrawn-status", done, status=last)
    except Exception as error:  # a failed step is reported, never hidden
        step("exception", False, kind=type(error).__name__, text=str(error)[:160])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["finishedAt"] = int(time.time())
        record["failures"] = failures
        os.makedirs(args.evidence, exist_ok=True)
        with open(os.path.join(args.evidence, f"drafts-e2e-{tag}.json"), "w") as out:
            json.dump(record, out, indent=2)
    print("PASS" if not failures else f"FAIL {len(failures)}: {','.join(failures)}", flush=True)
    return 0 if not failures else 1


if __name__ == "__main__":
    sys.exit(main())
