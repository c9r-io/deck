#!/usr/bin/env python3
"""Deck Controlled Execution certification verdict (schema deck-ce-certification/1).

The ONE authority that turns calibration evidence into a verdict
(governance r2, deck-ce-governance-2026-09-27-r2 §C, §H). Harnesses
(scripts/ce_parity.py, the deck-app `ce1_probe_*` test, later lanes) only
record observations; nothing else may compute a pass.

  scripts/ce_verdict.py --plan scripts/ce/plan-full-local-1.json \\
      --evidence run.jsonl [--evidence run.jsonl.app.jsonl ...] [--out verdict.json]

Contract (closed; changing it is a governance change):

- Plan (`deck-ce-plan/1`): frozen cases, each with id, track, mandatory,
  evidence_env (`ci` | `designated`), compare kind and lanes. Its SHA-256 is
  the plan digest every evidence file must name.
- Evidence (`deck-ce-evidence/1`, JSONL): each file starts with one `run`
  header (plan digest, evidence_env, lanes, builds), then `observation`
  records ({case, lane, observation} or {case, lane, error}), `friction`,
  `run-error` and exactly one `cleanup` record. A malformed file, a missing
  header, a foreign plan digest or an unknown lane is a usage error (exit 2):
  the aggregator refuses to judge evidence it cannot attribute.
- Case outcome (`actual`, closed): PARITY, TRANSPORT_DIFFERENCE,
  EXPECTED_CONTAINMENT (pass); FULL_LOCAL_PARITY_REGRESSION,
  TRANSPORT_CONTRACT_VIOLATION, UTILITY_REGRESSION (fail);
  BACKEND_UNSUPPORTED (unsupported); UNKNOWN (unknown); and `missing` when a
  planned lane has no observation. Subreasons (closed): capability-missing,
  semantic-environment-drift, deck-internal-exposure, baseline-invalid,
  lane-error, conflicting-evidence.
- Compare kinds: `tool` (identity, version, launcher class and resolved
  interpreter must all be equal — exit 0 alone is never parity);
  `tool-exact` (as `tool`, against a session-BASE lane: a tool the base does
  not have is a legitimate baseline, so B must match found/not-found too — B
  finding a tool the base lacks is drift); `capability`; `env-coord` (value
  digests equal); `b-absent` (the named Deck-internal coordinates must NOT
  reach the B lane); `b-capable` (one B lane must be capable, e.g. the
  documented shell invocation completes unattended). A lane-A baseline that
  is not capable is UNKNOWN (`baseline-invalid`), never a pass or a fail.
- Case `class` (optional, plan v2+): host-capability | shell-semantic |
  transport; reported per case, never changes the rules above.
- Track status (`pass | fail | blocked`) over MANDATORY cases only: any fail
  → fail; otherwise any unknown/missing/unsupported, or no mandatory case at
  all → blocked; otherwise pass. Reported per evidence_env too (`ci`,
  `designated`); CI evidence never substitutes for designated evidence.
- Counters: unknown_mandatory, missing_mandatory, silent_fallback,
  unexpected_prompts_baseline, friction_unrecorded, cleanup_unconfirmed,
  run_errors, unplanned_observations (reported only).
- overall = pass iff all five tracks pass AND every counter above except
  unplanned_observations is 0. Exit 0 only when overall = pass; 1 on
  fail/blocked; 2 on usage errors.
- `--gate TRACK:ENV` (the CI gate uses `full_local_parity:ci`): the exit
  code follows that ONE track's status over ONE evidence environment and
  that environment's own run counters instead of `overall` — pass only when
  the sub-status is pass and every zero-counter of the environment is 0;
  blocked is a fail. Evidence of the other environment never helps it.

Privacy: the verdict repeats only what evidence already holds (closed codes,
synthetic identities or path classes plus digest prefixes); it adds nothing.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

PLAN_SCHEMA = "deck-ce-plan/1"
EVIDENCE_SCHEMA = "deck-ce-evidence/1"
VERDICT_SCHEMA = "deck-ce-certification/1"
TRACKS = ("full_local_parity", "protected_security", "protected_utility", "authority_fail_closed", "edr_operational")
LANES = {
    "A1": "ci", "A1-base": "ci", "B-direct": "ci", "B-login": "ci", "B-shell": "ci", "B-admission": "ci",
    "A2": "designated", "A2-base": "designated", "B2-direct": "designated", "B2-shell": "designated",
    "B-designated": "designated",
}
COMPARES = ("tool", "tool-exact", "capability", "env-coord", "b-absent", "b-capable")
SINGLE_LANE = ("b-absent", "b-capable")
CLASSES = ("host-capability", "shell-semantic", "transport")
EVIDENCE_ENVS = ("ci", "designated")
PASS = {"PARITY", "TRANSPORT_DIFFERENCE", "EXPECTED_CONTAINMENT"}
FAIL = {"FULL_LOCAL_PARITY_REGRESSION", "TRANSPORT_CONTRACT_VIOLATION", "UTILITY_REGRESSION"}
RUN_COUNTERS = ("silent_fallback", "unexpected_prompts_baseline", "friction_unrecorded", "cleanup_unconfirmed", "run_errors")
ZERO_COUNTERS = ("unknown_mandatory", "missing_mandatory", *RUN_COUNTERS)
TOOL_FIELDS = ("identity", "version", "launcher_class", "interpreter")


class UsageError(Exception):
    pass


def load_plan(path: Path) -> tuple[dict, str]:
    raw = path.read_bytes()
    try:
        plan = json.loads(raw)
    except json.JSONDecodeError as error:
        raise UsageError(f"plan is not JSON: {error}") from None
    if plan.get("schema") != PLAN_SCHEMA:
        raise UsageError("plan schema is not " + PLAN_SCHEMA)
    seen = set()
    for case in plan.get("cases", []):
        if case.get("id") in seen:
            raise UsageError(f"duplicate case id {case.get('id')}")
        seen.add(case.get("id"))
        if case.get("track") not in TRACKS:
            raise UsageError(f"{case.get('id')}: unknown track")
        if case.get("compare") not in COMPARES:
            raise UsageError(f"{case.get('id')}: unknown compare kind")
        if case.get("evidence_env") not in EVIDENCE_ENVS:
            raise UsageError(f"{case.get('id')}: unknown evidence_env")
        if not isinstance(case.get("mandatory"), bool):
            raise UsageError(f"{case.get('id')}: mandatory must be a boolean")
        if "class" in case and case["class"] not in CLASSES:
            raise UsageError(f"{case.get('id')}: unknown class")
        lanes = case.get("lanes")
        expected_lanes = 1 if case["compare"] in SINGLE_LANE else 2
        if not isinstance(lanes, list) or len(lanes) != expected_lanes or any(lane not in LANES for lane in lanes):
            raise UsageError(f"{case.get('id')}: lanes must be {expected_lanes} known lanes")
        if any(LANES[lane] != case["evidence_env"] for lane in lanes):
            raise UsageError(f"{case.get('id')}: a lane belongs to another evidence_env")
    return plan, "sha256:" + hashlib.sha256(raw).hexdigest()


def load_evidence(paths: list[Path], digest: str) -> list[dict]:
    runs = []
    for path in paths:
        try:
            lines = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
        except (OSError, json.JSONDecodeError) as error:
            raise UsageError(f"{path}: unreadable evidence ({error})") from None
        if not lines or lines[0].get("kind") != "run" or lines[0].get("schema") != EVIDENCE_SCHEMA:
            raise UsageError(f"{path}: the first record must be a {EVIDENCE_SCHEMA} run header")
        header = lines[0]
        if header.get("plan_digest") != digest:
            raise UsageError(f"{path}: evidence names another plan digest")
        if header.get("evidence_env") not in EVIDENCE_ENVS:
            raise UsageError(f"{path}: unknown evidence_env")
        for record in lines[1:]:
            if record.get("kind") == "observation" and record.get("lane") not in LANES:
                raise UsageError(f"{path}: unknown lane {record.get('lane')}")
            if record.get("kind") == "observation" and LANES[record["lane"]] != header["evidence_env"]:
                raise UsageError(f"{path}: lane {record['lane']} in a {header['evidence_env']} run")
        runs.append({"file": str(path), "header": header, "records": lines[1:]})
    return runs


def outcome(actual: str, reason: str | None = None, subreason: str | None = None, **extra) -> dict:
    status = "pass" if actual in PASS else "fail" if actual in FAIL else "unsupported" if actual == "BACKEND_UNSUPPORTED" else "unknown"
    result = {"actual": actual, "status": status}
    if reason:
        result["reason"] = reason
    if subreason:
        result["subreason"] = subreason
    result.update(extra)
    return result


def regression(subreason: str, **extra) -> dict:
    return outcome("FULL_LOCAL_PARITY_REGRESSION", "full-local-parity-regression", subreason, **extra)


def classify(case: dict, lanes: dict[str, dict | None]) -> dict:
    """Pure classification of one case from its lane observations."""
    names = case["lanes"]
    for lane in names:
        if lanes.get(lane) is None:
            return {"actual": "MISSING", "status": "missing", "missing_lanes": [n for n in names if lanes.get(n) is None]}
    for lane in names:
        record = lanes[lane]
        if record.get("conflict"):
            return outcome("UNKNOWN", subreason="conflicting-evidence", lane=lane)
        if "error" in record or not isinstance(record.get("observation"), dict):
            return outcome("UNKNOWN", subreason="lane-error", lane=lane, error=record.get("error", "no-observation"))
    observations = [lanes[lane]["observation"] for lane in names]
    kind = case["compare"]
    if kind == "b-absent":
        present = [name for name, value in observations[0].get("present", {}).items() if value is True]
        if present:
            return outcome("TRANSPORT_CONTRACT_VIOLATION", "deck-internal-exposure", "deck-internal-exposure", present=present)
        return outcome("TRANSPORT_DIFFERENCE")
    if kind == "b-capable":
        if observations[0].get("capable") is True:
            return outcome("PARITY")
        return regression("capability-missing", detail=observations[0].get("detail"))
    a, b = observations
    if kind == "tool-exact":
        # The A lane is the session base: a tool it lacks is a legitimate
        # baseline, so B must match it exactly — found or not found alike.
        if a.get("found") is not b.get("found"):
            return regression("capability-missing" if a.get("found") else "semantic-environment-drift", differs=["found"])
        if a.get("found") is not True:
            return outcome("PARITY")
        differs = [field for field in TOOL_FIELDS if a.get(field) != b.get(field)]
        return regression("semantic-environment-drift", differs=differs) if differs else outcome("PARITY")
    if kind == "capability":
        if a.get("capable") is not True:
            return outcome("UNKNOWN", subreason="baseline-invalid")
        if b.get("capable") is True:
            return outcome("PARITY")
        return regression("capability-missing", detail=b.get("detail"))
    if kind == "env-coord":
        a_digests, b_digests = a.get("digests", {}), b.get("digests", {})
        if not a_digests or any(value is None for value in a_digests.values()):
            return outcome("UNKNOWN", subreason="baseline-invalid")
        missing = [name for name in a_digests if b_digests.get(name) is None]
        if missing:
            return regression("capability-missing", missing=missing)
        differs = [name for name in a_digests if b_digests.get(name) != a_digests[name]]
        if differs:
            return regression("semantic-environment-drift", differs=differs)
        return outcome("PARITY")
    # tool
    if a.get("found") is not True:
        return outcome("UNKNOWN", subreason="baseline-invalid")
    if b.get("found") is not True:
        return regression("capability-missing")
    differs = [field for field in TOOL_FIELDS if a.get(field) != b.get(field)]
    if differs:
        return regression("semantic-environment-drift", differs=differs)
    return outcome("PARITY")


def track_status(outcomes: list[dict]) -> tuple[str, str | None]:
    mandatory = [item for item in outcomes if item["mandatory"]]
    if not mandatory:
        return "blocked", "no-mandatory-cases"
    if any(item["status"] == "fail" for item in mandatory):
        return "fail", None
    if any(item["status"] != "pass" for item in mandatory):
        return "blocked", "unknown-missing-or-unsupported"
    return "pass", None


def counts(outcomes: list[dict]) -> dict:
    result: dict[str, int] = {}
    for item in outcomes:
        if item["mandatory"]:
            result[item["status"]] = result.get(item["status"], 0) + 1
    return result


def aggregate(plan: dict, digest: str, runs: list[dict]) -> dict:
    observed: dict[tuple[str, str], dict] = {}
    unplanned = 0
    planned_lanes = {(case["id"], lane) for case in plan["cases"] for lane in case["lanes"]}
    run_counters = {env: dict.fromkeys(RUN_COUNTERS, 0) for env in EVIDENCE_ENVS}
    for run in runs:
        records = run["records"]
        tally = run_counters[run["header"]["evidence_env"]]
        has_b = any(lane.startswith("B") and lane != "B-admission" for lane in run["header"].get("lanes", []))
        friction = [record for record in records if record.get("kind") == "friction"]
        if has_b and not friction:
            tally["friction_unrecorded"] += 1
        for record in friction:
            prompts = record.get("additional_prompts")
            if not isinstance(prompts, int):
                tally["friction_unrecorded"] += 1
            else:
                tally["unexpected_prompts_baseline"] += prompts
        cleanup = [record for record in records if record.get("kind") == "cleanup"]
        confirmed = len(cleanup) == 1 and all(value is True for key, value in cleanup[0].items() if key != "kind")
        if not confirmed:
            tally["cleanup_unconfirmed"] += 1
        tally["run_errors"] += sum(1 for record in records if record.get("kind") == "run-error")
        tally["silent_fallback"] += sum(1 for record in records if record.get("kind") == "fallback")
        for record in records:
            if record.get("kind") != "observation":
                continue
            key = (record.get("case"), record.get("lane"))
            if key not in planned_lanes:
                unplanned += 1
                continue
            if key in observed:
                observed[key] = {"conflict": True}
            else:
                observed[key] = record

    cases = []
    for case in plan["cases"]:
        result = classify(case, {lane: observed.get((case["id"], lane)) for lane in case["lanes"]})
        cases.append({
            "id": case["id"], "class": case.get("class"), "track": case["track"], "mandatory": case["mandatory"],
            "evidence_env": case["evidence_env"], "compare": case["compare"], "lanes": case["lanes"],
            **result,
            "evidence": {lane: observed.get((case["id"], lane)) for lane in case["lanes"]},
        })

    tracks = {}
    for track in TRACKS:
        members = [item for item in cases if item["track"] == track]
        status, reason = track_status(members)
        by_env = {}
        for env in EVIDENCE_ENVS:
            subset = [item for item in members if item["evidence_env"] == env]
            env_status, env_reason = track_status(subset)
            by_env[env] = {"status": env_status, "cases": counts(subset)}
            if env_reason:
                by_env[env]["reason"] = env_reason
        tracks[track] = {"status": status, "cases": counts(members), "by_evidence_env": by_env}
        if reason:
            tracks[track]["reason"] = reason

    mandatory = [item for item in cases if item["mandatory"]]

    def case_counters(subset: list[dict]) -> dict:
        return {
            "unknown_mandatory": sum(1 for item in subset if item["status"] in ("unknown", "unsupported")),
            "missing_mandatory": sum(1 for item in subset if item["status"] == "missing"),
        }

    counters = {
        **case_counters(mandatory),
        **{name: sum(run_counters[env][name] for env in EVIDENCE_ENVS) for name in RUN_COUNTERS},
        "unplanned_observations": unplanned,
    }
    counters_by_env = {
        env: {**case_counters([item for item in mandatory if item["evidence_env"] == env]), **run_counters[env]}
        for env in EVIDENCE_ENVS
    }
    overall = "pass" if all(tracks[t]["status"] == "pass" for t in TRACKS) and all(counters[c] == 0 for c in ZERO_COUNTERS) else (
        "fail" if any(tracks[t]["status"] == "fail" for t in TRACKS) else "blocked"
    )

    def distinct(field: str) -> list:
        return sorted({str(run["header"].get(field)) for run in runs if run["header"].get(field) is not None})

    return {
        "schema": VERDICT_SCHEMA,
        "candidate": {
            "contract": plan.get("contract"),
            "deck_build": distinct("deck_build"),
            "runner_build": distinct("runner_build"),
        },
        "plan": {"id": plan.get("id"), "digest": digest, "cases": len(plan["cases"]), "mandatory_cases": len(mandatory)},
        "evidence": [{"file": run["file"], "evidence_env": run["header"]["evidence_env"], "lanes": run["header"].get("lanes")} for run in runs],
        "tracks": tracks,
        "counters": counters,
        "counters_by_evidence_env": counters_by_env,
        "overall": overall,
        "cases": cases,
    }


def gate(verdict: dict, spec: str) -> dict:
    """One track's status over ONE evidence environment, plus that
    environment's counters. `pass` only when the sub-status passes and every
    zero-counter of that environment is 0; anything else is `fail`."""
    track, _, env = spec.partition(":")
    if track not in TRACKS or env not in EVIDENCE_ENVS:
        raise UsageError(f"--gate must be TRACK:ENV, got {spec}")
    status = verdict["tracks"][track]["by_evidence_env"][env]["status"]
    nonzero = {name: value for name, value in verdict["counters_by_evidence_env"][env].items() if value}
    return {"spec": spec, "track_status": status, "nonzero_counters": nonzero,
            "status": "pass" if status == "pass" and not nonzero else "fail"}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--plan", required=True, type=Path)
    parser.add_argument("--evidence", action="append", default=[], type=Path)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--gate", help="TRACK:ENV — exit 0 only when that track passes on that evidence environment")
    args = parser.parse_args(argv)
    try:
        plan, digest = load_plan(args.plan)
        runs = load_evidence(args.evidence, digest)
        verdict = aggregate(plan, digest, runs)
        if args.gate:
            verdict["gate"] = gate(verdict, args.gate)
    except (UsageError, OSError) as error:
        print(f"ce-verdict: {error}", file=sys.stderr)
        return 2
    text = json.dumps(verdict, indent=2, sort_keys=True) + "\n"
    if args.out:
        args.out.write_text(text)
    else:
        sys.stdout.write(text)
    if args.gate:
        print(f"ce-verdict gate {args.gate}: {verdict['gate']['status']}", file=sys.stderr)
        return 0 if verdict["gate"]["status"] == "pass" else 1
    return 0 if verdict["overall"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
