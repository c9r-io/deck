"""Closed-contract tests for scripts/ce_verdict.py (CE1 certification aggregator)."""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ce_verdict  # noqa: E402

REPOSITORY = Path(__file__).resolve().parent.parent
FROZEN_PLAN = REPOSITORY / "scripts" / "ce" / "plan-full-local-1.json"
FROZEN_PLAN_V2 = REPOSITORY / "scripts" / "ce" / "plan-full-local-2.json"


def case(case_id, compare="capability", lanes=("A1", "B-direct"), track="full_local_parity", mandatory=True, env="ci"):
    return {"id": case_id, "track": track, "mandatory": mandatory, "evidence_env": env,
            "compare": compare, "lanes": list(lanes), "probe": ["x"]}


def tool(identity="/t/x", version="x 1", launcher="native", interpreter=None, found=True):
    if not found:
        return {"kind": "tool", "found": False}
    return {"kind": "tool", "found": True, "identity": identity, "version": version,
            "launcher_class": launcher, "interpreter": interpreter}


class Harness:
    def __init__(self, cases):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        self.plan_path = self.root / "plan.json"
        self.plan_path.write_text(json.dumps({"schema": ce_verdict.PLAN_SCHEMA, "id": "t", "cases": cases}))
        self.digest = ce_verdict.load_plan(self.plan_path)[1]
        self.files = []

    def evidence(self, records, env="ci", lanes=("A1", "B-direct"), header=None, friction=True, cleanup=True):
        path = self.root / f"e{len(self.files)}.jsonl"
        lines = [header or {"kind": "run", "schema": ce_verdict.EVIDENCE_SCHEMA, "plan_digest": self.digest,
                            "evidence_env": env, "lanes": list(lanes)}]
        lines += records
        if friction:
            lines.append({"kind": "friction", "additional_prompts": 0})
        if cleanup:
            lines.append({"kind": "cleanup", "tmux_server_gone": True, "work_dir_removed": True})
        path.write_text("".join(json.dumps(line) + "\n" for line in lines))
        self.files.append(path)
        return path

    def verdict(self):
        plan, digest = ce_verdict.load_plan(self.plan_path)
        return ce_verdict.aggregate(plan, digest, ce_verdict.load_evidence(self.files, digest))


def obs(case_id, lane, observation=None, error=None):
    record = {"kind": "observation", "case": case_id, "lane": lane}
    if error:
        record["error"] = error
    else:
        record["observation"] = observation
    return record


class ClassificationTests(unittest.TestCase):
    def test_equal_tool_identity_is_parity_and_exit_zero_alone_is_not(self):
        c = case("t", compare="tool")
        self.assertEqual(ce_verdict.classify(c, {"A1": {"observation": tool()}, "B-direct": {"observation": tool()}})["actual"], "PARITY")
        drift = ce_verdict.classify(c, {"A1": {"observation": tool(version="x 1")}, "B-direct": {"observation": tool(version="x 2")}})
        self.assertEqual((drift["status"], drift["subreason"], drift["differs"]), ("fail", "semantic-environment-drift", ["version"]))

    def test_interpreter_drift_of_an_env_shebang_is_drift(self):
        c = case("t", compare="tool")
        a = tool(launcher="script-env", interpreter={"identity": "/r/i", "version": "i 2"})
        b = tool(launcher="script-env", interpreter={"identity": "/l/i", "version": "i 1"})
        self.assertEqual(ce_verdict.classify(c, {"A1": {"observation": a}, "B-direct": {"observation": b}})["differs"], ["interpreter"])

    def test_missing_tool_in_b_is_capability_missing(self):
        result = ce_verdict.classify(case("t", compare="tool"), {"A1": {"observation": tool()}, "B-direct": {"observation": tool(found=False)}})
        self.assertEqual((result["actual"], result["subreason"]), ("FULL_LOCAL_PARITY_REGRESSION", "capability-missing"))

    def test_a_baseline_that_cannot_do_it_is_unknown_never_pass_or_fail(self):
        result = ce_verdict.classify(case("c"), {"A1": {"observation": {"capable": False}}, "B-direct": {"observation": {"capable": False}}})
        self.assertEqual((result["status"], result["subreason"]), ("unknown", "baseline-invalid"))
        result = ce_verdict.classify(case("t", compare="tool"), {"A1": {"observation": tool(found=False)}, "B-direct": {"observation": tool(found=False)}})
        self.assertEqual(result["status"], "unknown")

    def test_lane_error_missing_lane_and_conflict(self):
        c = case("c")
        self.assertEqual(ce_verdict.classify(c, {"A1": {"observation": {"capable": True}}, "B-direct": None})["status"], "missing")
        self.assertEqual(ce_verdict.classify(c, {"A1": {"observation": {"capable": True}}, "B-direct": {"error": "job-timeout"}})["subreason"], "lane-error")
        self.assertEqual(ce_verdict.classify(c, {"A1": {"conflict": True}, "B-direct": {"observation": {"capable": True}}})["subreason"], "conflicting-evidence")

    def test_env_coordinates_compare_digests(self):
        c = case("e", compare="env-coord")
        same = {"digests": {"HOME": "sha256:a"}}
        self.assertEqual(ce_verdict.classify(c, {"A1": {"observation": same}, "B-direct": {"observation": same}})["actual"], "PARITY")
        other = {"digests": {"HOME": "sha256:b"}}
        self.assertEqual(ce_verdict.classify(c, {"A1": {"observation": same}, "B-direct": {"observation": other}})["subreason"], "semantic-environment-drift")
        gone = {"digests": {"HOME": None}}
        self.assertEqual(ce_verdict.classify(c, {"A1": {"observation": same}, "B-direct": {"observation": gone}})["subreason"], "capability-missing")

    def test_deck_internal_coordinates_must_not_reach_b(self):
        c = case("d", compare="b-absent", lanes=("B-direct",))
        clean = {"present": {"TMUX": False}}
        self.assertEqual(ce_verdict.classify(c, {"B-direct": {"observation": clean}})["actual"], "TRANSPORT_DIFFERENCE")
        leak = ce_verdict.classify(c, {"B-direct": {"observation": {"present": {"TMUX": True}}}})
        self.assertEqual((leak["status"], leak["actual"], leak["present"]), ("fail", "TRANSPORT_CONTRACT_VIOLATION", ["TMUX"]))


class AggregationTests(unittest.TestCase):
    def all_tracks_passing(self):
        cases = [case(f"c-{track}", track=track) for track in ce_verdict.TRACKS]
        harness = Harness(cases)
        records = []
        for item in cases:
            records.append(obs(item["id"], "A1", {"capable": True}))
            records.append(obs(item["id"], "B-direct", {"capable": True}))
        return harness, records

    def test_overall_pass_requires_every_track_and_every_counter(self):
        harness, records = self.all_tracks_passing()
        harness.evidence(records)
        verdict = harness.verdict()
        self.assertEqual(verdict["overall"], "pass")
        self.assertTrue(all(track["status"] == "pass" for track in verdict["tracks"].values()))

    def test_unconfirmed_cleanup_or_unrecorded_friction_blocks_a_pass(self):
        for kwargs, counter in (({"cleanup": False}, "cleanup_unconfirmed"), ({"friction": False}, "friction_unrecorded")):
            harness, records = self.all_tracks_passing()
            harness.evidence(records, **kwargs)
            verdict = harness.verdict()
            self.assertEqual(verdict["counters"][counter], 1)
            self.assertNotEqual(verdict["overall"], "pass")

    def test_an_unexpected_prompt_blocks_a_pass(self):
        harness, records = self.all_tracks_passing()
        path = harness.evidence(records)
        text = path.read_text().replace('"additional_prompts": 0', '"additional_prompts": 2')
        path.write_text(text)
        verdict = harness.verdict()
        self.assertEqual(verdict["counters"]["unexpected_prompts_baseline"], 2)
        self.assertNotEqual(verdict["overall"], "pass")

    def test_a_track_without_mandatory_cases_is_blocked_not_pass(self):
        harness = Harness([case("only")])
        harness.evidence([obs("only", "A1", {"capable": True}), obs("only", "B-direct", {"capable": True})])
        verdict = harness.verdict()
        self.assertEqual(verdict["tracks"]["full_local_parity"]["status"], "pass")
        self.assertEqual(verdict["tracks"]["protected_security"]["status"], "blocked")
        self.assertEqual(verdict["overall"], "blocked")

    def test_fail_dominates_missing_and_non_mandatory_cases_never_decide(self):
        harness = Harness([case("f"), case("m"), case("opt", mandatory=False)])
        harness.evidence([
            obs("f", "A1", {"capable": True}), obs("f", "B-direct", {"capable": False}),
            obs("m", "A1", {"capable": True}),
            obs("opt", "A1", {"capable": True}), obs("opt", "B-direct", {"capable": False}),
        ])
        verdict = harness.verdict()
        self.assertEqual(verdict["tracks"]["full_local_parity"]["status"], "fail")
        self.assertEqual(verdict["counters"]["missing_mandatory"], 1)
        self.assertEqual(verdict["overall"], "fail")
        harness = Harness([case("m"), case("opt", mandatory=False)])
        harness.evidence([obs("m", "A1", {"capable": True}), obs("m", "B-direct", {"capable": True}),
                          obs("opt", "A1", {"capable": True}), obs("opt", "B-direct", {"capable": False})])
        self.assertEqual(harness.verdict()["tracks"]["full_local_parity"]["status"], "pass")

    def test_ci_evidence_never_substitutes_for_designated_evidence(self):
        harness = Harness([case("c"), case("d", lanes=("A2", "B-designated"), env="designated")])
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": True})])
        track = harness.verdict()["tracks"]["full_local_parity"]
        self.assertEqual(track["by_evidence_env"]["ci"]["status"], "pass")
        self.assertEqual(track["by_evidence_env"]["designated"]["status"], "blocked")
        self.assertEqual(track["status"], "blocked")

    def test_duplicate_observations_become_unknown(self):
        harness = Harness([case("c")])
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": True})])
        harness.evidence([obs("c", "B-direct", {"capable": True})])
        self.assertEqual(harness.verdict()["cases"][0]["subreason"], "conflicting-evidence")

    def test_unplanned_observations_are_counted_and_ignored(self):
        harness = Harness([case("c")])
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": True}), obs("zzz", "A1", {"capable": True})])
        verdict = harness.verdict()
        self.assertEqual(verdict["counters"]["unplanned_observations"], 1)
        self.assertEqual(verdict["cases"][0]["status"], "pass")

    def test_run_errors_block_a_pass(self):
        harness, records = self.all_tracks_passing()
        harness.evidence(records + [{"kind": "run-error", "error": "RuntimeError"}])
        self.assertNotEqual(harness.verdict()["overall"], "pass")


class RefusalTests(unittest.TestCase):
    def test_foreign_plan_digest_unknown_lane_and_headerless_files_are_refused(self):
        harness = Harness([case("c")])
        bad_digest = harness.evidence([], header={"kind": "run", "schema": ce_verdict.EVIDENCE_SCHEMA, "plan_digest": "sha256:other", "evidence_env": "ci"})
        with self.assertRaises(ce_verdict.UsageError):
            ce_verdict.load_evidence([bad_digest], harness.digest)
        harness.files.clear()
        unknown_lane = harness.evidence([obs("c", "B-telepathy", {"capable": True})])
        with self.assertRaises(ce_verdict.UsageError):
            ce_verdict.load_evidence([unknown_lane], harness.digest)
        wrong_env = harness.evidence([obs("c", "A2", {"capable": True})])
        with self.assertRaises(ce_verdict.UsageError):
            ce_verdict.load_evidence([wrong_env], harness.digest)
        headerless = harness.root / "headerless.jsonl"
        headerless.write_text(json.dumps(obs("c", "A1", {"capable": True})) + "\n")
        with self.assertRaises(ce_verdict.UsageError):
            ce_verdict.load_evidence([headerless], harness.digest)

    def test_malformed_plans_are_refused(self):
        for bad in (
            [case("x"), case("x")],
            [case("x", track="speed")],
            [case("x", compare="vibes")],
            [case("x", lanes=("A1",))],
            [case("x", lanes=("A1", "B-designated"))],
            [{**case("x"), "mandatory": "yes"}],
        ):
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory) / "plan.json"
                path.write_text(json.dumps({"schema": ce_verdict.PLAN_SCHEMA, "cases": bad}))
                with self.assertRaises(ce_verdict.UsageError):
                    ce_verdict.load_plan(path)

    def test_cli_exit_codes(self):
        harness, records = AggregationTests().all_tracks_passing()
        path = harness.evidence(records)
        self.assertEqual(ce_verdict.main(["--plan", str(harness.plan_path), "--evidence", str(path), "--out", str(harness.root / "v.json")]), 0)
        harness2 = Harness([case("c")])
        path2 = harness2.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": False})])
        self.assertEqual(ce_verdict.main(["--plan", str(harness2.plan_path), "--evidence", str(path2), "--out", str(harness2.root / "v.json")]), 1)
        self.assertEqual(ce_verdict.main(["--plan", str(harness2.root / "missing.json")]), 2)


class CalibratedCompareTests(unittest.TestCase):
    """Plan v2 compare kinds: a session-base lane and a single capable lane."""

    def test_tool_exact_treats_a_tool_the_base_lacks_as_a_legitimate_baseline(self):
        c = case("t", compare="tool-exact", lanes=("A1-base", "B-direct"))
        absent = tool(found=False)
        self.assertEqual(ce_verdict.classify(c, {"A1-base": {"observation": absent}, "B-direct": {"observation": absent}})["actual"], "PARITY")
        injected = ce_verdict.classify(c, {"A1-base": {"observation": absent}, "B-direct": {"observation": tool()}})
        self.assertEqual((injected["status"], injected["subreason"]), ("fail", "semantic-environment-drift"))
        lost = ce_verdict.classify(c, {"A1-base": {"observation": tool()}, "B-direct": {"observation": absent}})
        self.assertEqual((lost["status"], lost["subreason"]), ("fail", "capability-missing"))
        other = ce_verdict.classify(c, {"A1-base": {"observation": tool(identity="/usr/bin/x")}, "B-direct": {"observation": tool(identity="/opt/homebrew/bin/x")}})
        self.assertEqual(other["differs"], ["identity"])

    def test_b_capable_needs_one_capable_lane(self):
        c = case("h", compare="b-capable", lanes=("B-shell",))
        self.assertEqual(ce_verdict.classify(c, {"B-shell": {"observation": {"capable": True}}})["actual"], "PARITY")
        self.assertEqual(ce_verdict.classify(c, {"B-shell": {"observation": {"capable": False}}})["status"], "fail")
        self.assertEqual(ce_verdict.classify(c, {"B-shell": {"error": "job-timeout"}})["status"], "unknown")

    def test_an_unknown_class_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "plan.json"
            path.write_text(json.dumps({"schema": ce_verdict.PLAN_SCHEMA, "cases": [{**case("x"), "class": "vibes"}]}))
            with self.assertRaises(ce_verdict.UsageError):
                ce_verdict.load_plan(path)


class GateTests(unittest.TestCase):
    """`--gate TRACK:ENV`: one track over one evidence environment."""

    def gate_exit(self, harness, spec="full_local_parity:ci"):
        args = ["--plan", str(harness.plan_path), "--out", str(harness.root / "v.json"), "--gate", spec]
        for path in harness.files:
            args += ["--evidence", str(path)]
        return ce_verdict.main(args)

    def plan_with_designated(self):
        return Harness([case("c"), case("d", lanes=("A2", "B-designated"), env="designated")])

    def test_ci_gate_passes_without_designated_evidence_and_overall_stays_blocked(self):
        harness = self.plan_with_designated()
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": True})])
        self.assertEqual(self.gate_exit(harness), 0)
        verdict = json.loads((harness.root / "v.json").read_text())
        self.assertEqual(verdict["gate"]["status"], "pass")
        self.assertNotEqual(verdict["overall"], "pass")
        self.assertEqual(self.gate_exit(harness, "full_local_parity:designated"), 1, "missing designated evidence blocks")

    def test_a_failing_or_blocked_ci_case_fails_the_gate(self):
        harness = self.plan_with_designated()
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": False})])
        self.assertEqual(self.gate_exit(harness), 1)
        harness = self.plan_with_designated()
        harness.evidence([obs("c", "A1", {"capable": True})])
        self.assertEqual(self.gate_exit(harness), 1)

    def test_a_ci_run_counter_fails_the_gate_and_a_designated_one_does_not_leak_in(self):
        harness = self.plan_with_designated()
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": True})], cleanup=False)
        self.assertEqual(self.gate_exit(harness), 1)
        harness = self.plan_with_designated()
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": True})])
        harness.evidence([{"kind": "run-error", "error": "RuntimeError"}], env="designated", lanes=("A2", "B-designated"))
        self.assertEqual(self.gate_exit(harness), 0)
        self.assertEqual(self.gate_exit(harness, "full_local_parity:designated"), 1)

    def test_a_malformed_gate_is_a_usage_error(self):
        harness = self.plan_with_designated()
        harness.evidence([obs("c", "A1", {"capable": True}), obs("c", "B-direct", {"capable": True})])
        self.assertEqual(self.gate_exit(harness, "full_local_parity"), 2)
        self.assertEqual(self.gate_exit(harness, "speed:ci"), 2)


class FrozenPlanTests(unittest.TestCase):
    def test_the_frozen_v1_plan_is_valid_and_unchanged(self):
        plan, digest = ce_verdict.load_plan(FROZEN_PLAN)
        # v1 is historical evidence (accepted 2026-09-27): never edit it.
        self.assertEqual(digest, "sha256:d3dcd704e6e30f9957105cb9612f92b77aa9c856b12e7afcea88bfa52448d366")
        lanes = {lane for item in plan["cases"] for lane in item["lanes"]}
        self.assertEqual(lanes, {"A1", "B-direct", "B-login", "B-admission", "A2", "B-designated"})
        self.assertTrue(all(item["track"] == "full_local_parity" for item in plan["cases"]))

    def test_the_v2_plan_classifies_every_case_and_names_each_reclassification(self):
        plan, _ = ce_verdict.load_plan(FROZEN_PLAN_V2)
        self.assertTrue(all(item.get("class") in ce_verdict.CLASSES for item in plan["cases"]))
        self.assertTrue(all(item["reason"] for item in plan["reclassified_from_v1"]))
        self.assertTrue(all(item["reason"] for item in plan["registered_transport_differences"]))
        lanes = {lane for item in plan["cases"] for lane in item["lanes"]}
        self.assertNotIn("B-login", lanes, "-lc is not the Terminal-semantic invocation")
        for env in ("ci", "designated"):
            mandatory = [item for item in plan["cases"] if item["mandatory"] and item["evidence_env"] == env]
            self.assertGreaterEqual(len(mandatory), 20)
        self.assertEqual(sum(1 for item in plan["cases"] if "B-admission" in item["lanes"]), 1)


if __name__ == "__main__":
    unittest.main()
