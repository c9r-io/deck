#!/usr/bin/env python3
"""Signal Integrity real-agent candidate verdict (FR-SI-05).

Offline and manual: a person runs each scenario of app/SMOKE.md's
"Signal candidate" checklist on an INSTALLED Deck and notes, per case, the
card's session tag (`sess-xxxxx`, as app.log prints it) and the time window.
This tool then judges each case from Deck's own content-free app.log lines
only — never an agent transcript, prompt, output or path — and prints one
machine-readable verdict (schema `deck-signal-candidate/1`).

Every case ends `pass`, `fail` (the log contradicts the contract) or
`insufficient-evidence` (the log does not prove it). Missing evidence is
never a pass. Exit status: 0 only when every case passed; 1 otherwise; 2 on
usage errors. CLI versions are recorded as evidence, never checked against
an allowlist.

  scripts/signal-candidate --log ~/.deck/app.log --plan plan.json \\
      --deck-version 0.7.13 --deck-build <sha> \\
      --claude-version 2.1.282 --codex-version 0.157.0

plan.json: [{"case": "claude-normal", "session": "sess-ab12c", "since": <epoch>, "until": <epoch>}]

A Deck restart re-seeds app.log's session tags (the same tmux session is
logged under a new `sess-…` afterwards), so a case spanning a restart names
every tag it was logged under: "session": ["sess-before", "sess-after"].
A single string stays the normal form.
"""
import argparse
import json
import re
import sys

SCHEMA = "deck-signal-candidate/1"
HELPER_PROTOCOL = 2

ACCEPTED = re.compile(
    r"^(?P<t>\d+) \[agent-status\] (?P<source>claude-code|codex) (?P<state>working|needs-input|turn-done)"
    r" s=(?P<s>sess-[0-9a-f]+) target=(?P<target>[01]) v=(?P<v>[12]) e=(?P<e>\d+)$"
)
DROPPED = re.compile(r"^(?P<t>\d+) \[agent-status\] dropped \((?P<reason>[a-z-]+)\)$")
SUMMARY = re.compile(r"^(?P<t>\d+) \[agent-status\] drops (?P<counts>(?:[a-z-]+=\d+ ?)+)$")
ABSENT = re.compile(r"^(?P<t>\d+) \[agent-status\] (?P<source>[a-z-]+) identity-absent$")
RUN_CLOSED = re.compile(r"^(?P<t>\d+) \[inbound\] run closed$")
BOOT = re.compile(r"^(?P<t>\d+) \[notify\] boot [a-z-]+$")
NOTIFY = re.compile(r"^(?P<t>\d+) \[notify\] (?P<kind_>posted [a-z-]+|viewed|suppressed viewed-episode) s=(?P<s>sess-[0-9a-f]+) e=\d+$")

CASES = {
    # agent, required accepted word order, whether an automation with
    # "close the card" must NOT have closed its run inside the window,
    # whether a Deck restart must lie between events
    "claude-normal": ("claude-code", ["working", "turn-done"], False, False),
    "claude-permission": ("claude-code", ["working", "needs-input", "turn-done"], False, False),
    "claude-background-resume": ("claude-code", ["working", "turn-done", "working", "turn-done"], True, False),
    "claude-restart": ("claude-code", ["working", "turn-done"], False, True),
    "codex-normal": ("codex", ["working", "turn-done"], False, False),
    "codex-permission": ("codex", ["working", "needs-input", "turn-done"], False, False),
    "codex-interrupt": ("codex", ["working", "turn-done"], False, False),
    "codex-background-interrupt": ("codex", ["working", "turn-done"], True, False),
    "codex-rapid": ("codex", ["working", "turn-done", "working", "turn-done"], False, False),
}


def parse(lines):
    events = []
    for raw in lines:
        line = raw.rstrip("\n")
        for kind, pattern in (("accepted", ACCEPTED), ("dropped", DROPPED), ("summary", SUMMARY),
                              ("absent", ABSENT), ("closed", RUN_CLOSED), ("boot", BOOT), ("notify", NOTIFY)):
            m = pattern.match(line)
            if m:
                event = {"kind": kind, **m.groupdict()}
                event["t"] = int(event["t"])
                events.append(event)
                break
    return events


def subsequence(words, required):
    """Index positions of `required` as an ordered subsequence of `words`."""
    at, positions = 0, []
    for want in required:
        while at < len(words) and words[at] != want:
            at += 1
        if at == len(words):
            return None
        positions.append(at)
        at += 1
    return positions


def judge(case_id, session, since, until, events):
    if case_id not in CASES:
        raise ValueError(f"unknown case {case_id}")
    tags = [session] if isinstance(session, str) else list(session)
    if not tags or not all(isinstance(t, str) and re.fullmatch(r"sess-[0-9a-f]+", t) for t in tags):
        raise ValueError(f"bad session tag(s) for {case_id}")
    source, required, no_close, needs_restart = CASES[case_id]
    window = [e for e in events if since <= e["t"] <= until]
    mine = [e for e in window if e["kind"] == "accepted" and e["s"] in tags]
    drops = {}
    for e in window:
        if e["kind"] == "dropped":
            drops[e["reason"]] = drops.get(e["reason"], 0) + 1
    result = {
        "id": case_id, "agent": source, "session": session,
        "observed": {"accepted": len(mine), "v2_accepted": sum(1 for e in mine if e["v"] == "2"),
                     "drops": drops, "identity_absent": any(e["kind"] == "absent" and e["source"] == source for e in window),
                     "notify": {kind: sum(1 for e in window if e["kind"] == "notify" and e["s"] in tags and e["kind_"].startswith(kind))
                                for kind in ("posted", "viewed", "suppressed")}},
        "assertions": {}, "timings_s": {},
    }
    reasons = []
    if not mine:
        result["verdict"] = "insufficient-evidence"
        result["reasons"] = ["no-accepted-events-for-session"]
        return result
    wrong_source = [e for e in mine if e["source"] != source]
    v1 = [e for e in mine if e["v"] != "2"]
    targeted = [e for e in mine if e["target"] == "1"]
    result["assertions"]["v2_admission"] = not v1
    result["assertions"]["expected_source"] = not wrong_source
    if v1:
        reasons.append("v1-event-accepted")
    if wrong_source:
        reasons.append("unexpected-source")
    words = [e["state"] for e in targeted]
    positions = subsequence(words, required)
    result["assertions"]["lifecycle_sequence"] = positions is not None
    fail = bool(v1 or wrong_source)
    insufficient = positions is None
    if positions is None:
        reasons.append("required-sequence-not-observed")
    else:
        first, last = targeted[positions[0]], targeted[positions[-1]]
        result["timings_s"]["sequence"] = last["t"] - first["t"]
        if len(required) == 4:
            result["timings_s"]["first_end_to_resume"] = targeted[positions[2]]["t"] - targeted[positions[1]]["t"]
            # a resume is a NEW interaction: the tracker would have refused
            # the same ended id, so an accepted working after the ending is it
            result["assertions"]["new_interaction_after_end"] = targeted[positions[2]]["e"] != targeted[positions[1]]["e"]
        if no_close:
            # Deliberately conservative: `[inbound] run closed` carries no
            # session or card tag, so ANY run closing inside the sequence is
            # taken as this run's premature close. An unrelated automation
            # closing in the same window yields a false FAIL — never a false
            # pass. Run auto-close cases with no other automation active.
            closed = [e for e in window if e["kind"] == "closed" and first["t"] <= e["t"] < last["t"]]
            result["assertions"]["no_premature_auto_close"] = not closed
            if closed:
                fail = True
                reasons.append("run-closed-while-agent-live")
        if needs_restart:
            # the agent survived a Deck restart AND the restarted Deck still
            # admits its signal: a boot between the session's first and last
            # accepted events, with an accepted event after that boot (not
            # necessarily inside the earliest matching pair — a turn may end
            # before the restart and the agent resume after it)
            boots = [e for e in window if e["kind"] == "boot" and targeted[0]["t"] < e["t"] < targeted[-1]["t"]]
            survived = any(any(ev["t"] > b["t"] for ev in targeted) for b in boots)
            result["assertions"]["survived_deck_restart"] = survived
            if not survived:
                insufficient = True
                reasons.append("no-deck-restart-inside-the-sequence")
    result["verdict"] = "fail" if fail else "insufficient-evidence" if insufficient else "pass"
    if reasons:
        result["reasons"] = reasons
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description="Signal Integrity candidate verdict from app.log")
    parser.add_argument("--log", required=True)
    parser.add_argument("--plan", required=True)
    parser.add_argument("--deck-version", required=True)
    parser.add_argument("--deck-build", default="")
    parser.add_argument("--claude-version", default="")
    parser.add_argument("--codex-version", default="")
    try:
        args = parser.parse_args(argv)
        plan = json.load(open(args.plan))
        with open(args.log, encoding="utf-8", errors="replace") as log:
            events = parse(log)
        cases = [judge(c["case"], c["session"], int(c["since"]), int(c["until"]), events) for c in plan]
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"signal-candidate: {error}", file=sys.stderr)
        return 2
    verdict = {
        "schema": SCHEMA,
        "deck": {"version": args.deck_version, "build": args.deck_build},
        "helper_protocol": HELPER_PROTOCOL,
        "agents": {"claude-code": args.claude_version, "codex": args.codex_version},
        "cases": cases,
        "verdict": "pass" if cases and all(c["verdict"] == "pass" for c in cases) else
                   "fail" if any(c["verdict"] == "fail" for c in cases) else "insufficient-evidence",
    }
    print(json.dumps(verdict, indent=1))
    return 0 if verdict["verdict"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
