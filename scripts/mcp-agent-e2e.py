#!/usr/bin/env python3
"""Independent JSON-RPC client for the Agent handover trigger of an isolated
Deck smoke run (app/SMOKE.md "Agent handover: delegated MCP client").

Deck must already be open in the `agent-live` WKWebView mode and past
`agent-live-armed`. `--adapter` must be a DEBUG deck-mcp build: a release
adapter refuses the --socket / --credential-fd seams. The credential is
handed to the adapter on an inherited descriptor, never on its argv.

Default run: initialize, tools/list (16 tools), deck_capabilities,
deck_automation_list, a wrong revision (RULE_CHANGED with the current one),
a wrong sequence (STALE_REQUEST), the real trigger, the operation polled to
`committed` with its cardId, the same request replayed (same operation), the
same request id with other text (REQUEST_ID_CONFLICT), and the sequence
advanced by one. `--replay-only` repeats the recorded request after the page
closed the card and expects the same committed operation back.

Prints closed facts only: codes, ids, counts and seconds; never the
credential. The evidence file holds the same, plus the item as given.
Exit 0 when every expectation held, 1 otherwise, 2 for usage.
"""
import argparse
import json
import os
import select
import subprocess
import sys
import time

PROTOCOL = "2025-06-18"
TOOLS = 16


def parse():
    p = argparse.ArgumentParser()
    for name in ("adapter", "socket", "client-id", "credential-file", "automation", "request-id", "evidence"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--text", default="Please look at this handed-over mention.")
    p.add_argument("--from", dest="from_", default="patrol smoke")
    p.add_argument("--where", default="#deck-smoke")
    p.add_argument("--link", default="")
    p.add_argument("--replay-only", action="store_true")
    p.add_argument("--sequence", type=int, help="the recorded trigger sequence of the request replayed")
    p.add_argument("--operation", help="the recorded operation id the replay must return")
    p.add_argument("--wait", type=int, default=120, help="seconds to wait for the operation outcome")
    args = p.parse_args()
    if args.replay_only and (args.sequence is None or not args.operation):
        p.error("--replay-only needs --sequence and --operation")
    return args


class Adapter:
    def __init__(self, path, client_id, socket, credential_file):
        fd = os.open(credential_file, os.O_RDONLY)
        if fd != 3:
            os.dup2(fd, 3)
            os.close(fd)
        self.proc = subprocess.Popen([path, "--client-id", client_id, "--socket", socket, "--credential-fd", "3"],
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, pass_fds=(3,))
        os.close(3)
        self.next_id = 1
        self.buffer = b""

    def notify(self, method, params=None):
        self.proc.stdin.write((json.dumps({"jsonrpc": "2.0", "method": method, "params": params or {}}) + "\n").encode())
        self.proc.stdin.flush()

    def request(self, method, params=None, timeout=15.0):
        rid = self.next_id
        self.next_id += 1
        self.proc.stdin.write((json.dumps({"jsonrpc": "2.0", "id": rid, "method": method, "params": params or {}}) + "\n").encode())
        self.proc.stdin.flush()
        deadline = time.time() + timeout
        while True:
            at = self.buffer.find(b"\n")
            while at >= 0:
                line, self.buffer = self.buffer[:at], self.buffer[at + 1:]
                at = self.buffer.find(b"\n")
                if not line.strip():
                    continue
                message = json.loads(line)
                if message.get("id") == rid:
                    return message
            remaining = deadline - time.time()
            if remaining <= 0:
                raise TimeoutError(f"no answer to {method}")
            ready, _, _ = select.select([self.proc.stdout], [], [], remaining)
            if not ready:
                raise TimeoutError(f"no answer to {method}")
            chunk = os.read(self.proc.stdout.fileno(), 65536)
            if not chunk:
                raise RuntimeError("adapter closed its output")
            self.buffer += chunk

    def call(self, name, arguments=None):
        answer = self.request("tools/call", {"name": name, "arguments": arguments or {}})
        if "error" in answer:
            raise RuntimeError(f"{name}: protocol error {answer['error'].get('code')}")
        result = answer["result"]
        structured = result.get("structuredContent") or {}
        return (not result.get("isError", False)), structured

    def close(self):
        try:
            self.proc.stdin.close()
        except OSError:
            pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        return self.proc.stderr.read().decode(errors="replace")


def main():
    args = parse()
    record = {"startedAt": int(time.time()), "clientId": args.client_id, "automation": args.automation,
              "requestId": args.request_id, "replayOnly": args.replay_only, "steps": [], "item": {
                  "text": args.text, "from": args.from_, "where": args.where, "link": args.link}}
    failures = []

    def step(name, ok, **facts):
        record["steps"].append({"name": name, "ok": bool(ok), **facts})
        print(f"{'ok  ' if ok else 'FAIL'} {name} " + " ".join(f"{k}={v}" for k, v in facts.items()), flush=True)
        if not ok:
            failures.append(name)
        return ok

    adapter = Adapter(args.adapter, args.client_id, args.socket, args.credential_file)
    try:
        init = adapter.request("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                              "clientInfo": {"name": "deck-agent-e2e", "version": "1"}})
        step("initialize", init.get("result", {}).get("serverInfo", {}).get("name") == "deck-mcp")
        adapter.notify("notifications/initialized")
        listed = adapter.request("tools/list")
        names = [tool["name"] for tool in listed.get("result", {}).get("tools", [])]
        step("tools-list", len(names) == TOOLS and "deck_automation_list" in names and "deck_automation_trigger" in names,
             count=len(names))
        ok, capabilities = adapter.call("deck_capabilities")
        step("capabilities", ok and capabilities.get("mayTriggerAutomations") is True,
             mayTriggerAutomations=capabilities.get("mayTriggerAutomations"), nextTriggerSequence=capabilities.get("nextTriggerSequence"))
        ok, automations = adapter.call("deck_automation_list")
        mine = next((a for a in automations.get("automations", []) if a.get("id") == args.automation), None)
        sequence = automations.get("nextTriggerSequence")
        step("automation-list", ok and mine is not None and isinstance(sequence, int) and len(mine.get("revision") or "") == 64,
             automations=len(automations.get("automations", [])), nextTriggerSequence=sequence)
        if mine is None:
            raise RuntimeError("the automation is not listed")
        revision = mine["revision"]
        item = {"automation_id": args.automation, "text": args.text, "from": args.from_, "where": args.where}
        if args.link:
            item["link"] = args.link

        if args.replay_only:
            ok, replay = adapter.call("deck_automation_trigger", {"request_id": args.request_id, "trigger_sequence": args.sequence,
                                                                  "expected_revision": revision, **item})
            step("replay-after-close", ok and replay.get("operationId") == args.operation and replay.get("state") == "committed",
                 state=replay.get("state"), sameOperation=replay.get("operationId") == args.operation)
            ok, again = adapter.call("deck_automation_list")
            step("sequence-unchanged", ok and again.get("nextTriggerSequence") == sequence, nextTriggerSequence=again.get("nextTriggerSequence"))
        else:
            # V15: a wrong revision is refused before anything is accepted
            ok, wrong = adapter.call("deck_automation_trigger", {"request_id": args.request_id + "_rev", "trigger_sequence": sequence,
                                                                 "expected_revision": "0" * 64, **item})
            error = wrong.get("error", {})
            step("rule-changed", not ok and error.get("code") == "RULE_CHANGED" and error.get("currentRevision") == revision,
                 code=error.get("code"), currentMatches=error.get("currentRevision") == revision)
            # a wrong sequence is refused
            ok, stale = adapter.call("deck_automation_trigger", {"request_id": args.request_id + "_seq", "trigger_sequence": sequence + 7,
                                                                 "expected_revision": revision, **item})
            step("stale-sequence", not ok and stale.get("error", {}).get("code") == "STALE_REQUEST", code=stale.get("error", {}).get("code"))
            # V01: the handover
            request = {"request_id": args.request_id, "trigger_sequence": sequence, "expected_revision": revision, **item}
            t0 = time.time()
            ok, accepted = adapter.call("deck_automation_trigger", request)
            operation = accepted.get("operationId")
            step("trigger", ok and bool(operation) and accepted.get("state") == "accepted", state=accepted.get("state"), operation=operation)
            record["sequence"], record["operation"], record["revision"] = sequence, operation, revision
            outcome = None
            deadline = time.time() + args.wait
            while time.time() < deadline:
                ok, view = adapter.call("deck_operation_get", {"operation_id": operation})
                if ok and view.get("state") in ("committed", "rejected", "ambiguous"):
                    outcome = view
                    break
                time.sleep(1)
            card = (outcome or {}).get("result", {}).get("cardId") if outcome else None
            step("operation-committed", outcome is not None and outcome.get("state") == "committed" and bool(card),
                 state=(outcome or {}).get("state"), code=(outcome or {}).get("code"), seconds=round(time.time() - t0, 1), card=card)
            record["cardId"] = card
            # V03: the same request again is the same operation; other text under the same id conflicts
            ok, replay = adapter.call("deck_automation_trigger", request)
            step("replay-same-operation", ok and replay.get("operationId") == operation, sameOperation=replay.get("operationId") == operation,
                 state=replay.get("state"))
            ok, conflict = adapter.call("deck_automation_trigger", {**request, "text": args.text + " (changed)"})
            step("request-id-conflict", not ok and conflict.get("error", {}).get("code") == "REQUEST_ID_CONFLICT",
                 code=conflict.get("error", {}).get("code"))
            ok, after = adapter.call("deck_automation_list")
            step("sequence-advanced", ok and after.get("nextTriggerSequence") == sequence + 1, nextTriggerSequence=after.get("nextTriggerSequence"))
    except Exception as error:  # noqa: BLE001 — the record must say what stopped the run
        step("exception", False, kind=type(error).__name__, text=str(error)[:160])
    finally:
        stderr = adapter.close()
        record["adapterStderrBytes"] = len(stderr)
        record["failures"] = failures
        record["finishedAt"] = int(time.time())
        os.makedirs(args.evidence, mode=0o700, exist_ok=True)
        name = "replay" if args.replay_only else "trigger"
        path = os.path.join(args.evidence, f"agent-e2e-{name}.json")
        with open(path, "w") as handle:
            json.dump(record, handle, indent=2)
        os.chmod(path, 0o600)
        print(f"evidence {path} failures={len(failures)} adapterStderrBytes={len(stderr)}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
