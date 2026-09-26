"""Tests for scripts/signal_candidate.py over synthetic, content-free logs."""
import json
import os
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(__file__))
import signal_candidate as sc  # noqa: E402

S = "sess-ab12c"
OTHER = "sess-99999"


def line(t, source, state, s=S, v=2, e=1, target=1):
    return f"{t} [agent-status] {source} {state} s={s} target={target} v={v} e={e}"


def judge(case, lines, since=0, until=10_000, session=S):
    return sc.judge(case, session, since, until, sc.parse(lines))


class CandidateVerdicts(unittest.TestCase):
    def test_a_normal_v2_turn_passes(self):
        r = judge("claude-normal", [line(10, "claude-code", "working", e=1), line(12, "claude-code", "turn-done", e=2)])
        self.assertEqual(r["verdict"], "pass")
        self.assertTrue(r["assertions"]["v2_admission"])
        self.assertEqual(r["timings_s"]["sequence"], 2)

    def test_missing_evidence_is_never_a_pass(self):
        self.assertEqual(judge("claude-normal", [])["verdict"], "insufficient-evidence")
        # another session's events do not count
        r = judge("claude-normal", [line(10, "claude-code", "working", s=OTHER), line(11, "claude-code", "turn-done", s=OTHER)])
        self.assertEqual(r["verdict"], "insufficient-evidence")
        # outside the window does not count
        r = judge("claude-normal", [line(10, "claude-code", "working"), line(11, "claude-code", "turn-done")], since=20)
        self.assertEqual(r["verdict"], "insufficient-evidence")
        # an incomplete sequence (no ending) is not proven
        r = judge("claude-permission", [line(10, "claude-code", "working"), line(11, "claude-code", "needs-input")])
        self.assertEqual(r["verdict"], "insufficient-evidence")
        # inactive-pane events are not the card's Signal
        r = judge("claude-normal", [line(10, "claude-code", "working", target=0), line(11, "claude-code", "turn-done", target=0)])
        self.assertEqual(r["verdict"], "insufficient-evidence")

    def test_a_v1_event_or_wrong_source_fails(self):
        r = judge("codex-normal", [line(10, "codex", "working", v=1), line(11, "codex", "turn-done")])
        self.assertEqual(r["verdict"], "fail")
        self.assertIn("v1-event-accepted", r["reasons"])
        r = judge("codex-normal", [line(10, "claude-code", "working"), line(11, "claude-code", "turn-done")])
        self.assertEqual(r["verdict"], "fail")

    def test_background_resume_must_not_close_the_run_and_resumes_as_a_new_interaction(self):
        good = [line(10, "claude-code", "working", e=1), line(15, "claude-code", "turn-done", e=2),
                line(40, "claude-code", "working", e=3), line(45, "claude-code", "turn-done", e=4)]
        r = judge("claude-background-resume", good)
        self.assertEqual(r["verdict"], "pass")
        self.assertEqual(r["timings_s"]["first_end_to_resume"], 25)
        self.assertTrue(r["assertions"]["new_interaction_after_end"])
        closed = good[:2] + ["22 [inbound] run closed"] + good[2:]
        r = judge("claude-background-resume", closed)
        self.assertEqual(r["verdict"], "fail")
        self.assertIn("run-closed-while-agent-live", r["reasons"])
        # a close AFTER the whole sequence is not premature
        self.assertEqual(judge("claude-background-resume", good + ["50 [inbound] run closed"])["verdict"], "pass")

    def test_an_uncorrelated_run_close_fails_conservatively(self):
        # the close line has no session tag: an unrelated automation closing
        # inside the window is indistinguishable, so the case FAILS — the
        # tool may be wrong towards fail, never towards pass
        seq = [line(10, "codex", "working", e=1), line(15, "codex", "turn-done", e=2),
               line(40, "codex", "working", e=3), line(45, "codex", "turn-done", e=4)]
        unrelated = seq[:2] + ["20 [inbound] run closed"] + seq[2:]
        self.assertEqual(judge("codex-rapid", unrelated)["verdict"], "pass", "no auto-close assertion there")
        self.assertEqual(judge("claude-background-resume",
                               [l.replace("codex", "claude-code") for l in unrelated])["verdict"], "fail")

    def test_restart_needs_a_boot_between_the_events(self):
        seq = [line(10, "claude-code", "working"), line(30, "claude-code", "turn-done", e=2)]
        self.assertEqual(judge("claude-restart", seq)["verdict"], "insufficient-evidence")
        booted = seq[:1] + ["20 [notify] boot authorized"] + seq[1:]
        self.assertEqual(judge("claude-restart", booted)["verdict"], "pass")

    def test_a_restart_case_may_span_the_tags_of_both_deck_processes(self):
        # the tag is re-seeded at every Deck start: the same session is
        # sess-ab12c before the restart and sess-cd34e after it
        after = "sess-cd34e"
        seq = [line(10, "claude-code", "working"), "20 [notify] boot authorized",
               line(30, "claude-code", "turn-done", s=after, e=1)]
        self.assertEqual(judge("claude-restart", seq)["verdict"], "insufficient-evidence", "one tag sees half")
        self.assertEqual(judge("claude-restart", seq, session=[S, after])["verdict"], "pass")
        # still requires the boot BETWEEN the events
        no_boot = [seq[0], seq[2]]
        self.assertEqual(judge("claude-restart", no_boot, session=[S, after])["verdict"], "insufficient-evidence")
        # an unrelated session's events are never pulled in by a list
        other = [line(10, "claude-code", "working", s=OTHER), "20 [notify] boot authorized",
                 line(30, "claude-code", "turn-done", s=OTHER, e=1)]
        self.assertEqual(judge("claude-restart", other, session=[S, after])["verdict"], "insufficient-evidence")
        # the real 0.7.14 candidate shape: the first turn ended BEFORE the
        # restart, the agent resumed and ended again AFTER it (new tag)
        real = [line(10, "claude-code", "working", e=2), line(15, "claude-code", "turn-done", e=3),
                "25 [notify] boot not-determined",
                line(100, "claude-code", "working", s=after, e=2), line(102, "claude-code", "turn-done", s=after, e=3)]
        self.assertEqual(judge("claude-restart", real, session=[S, after])["verdict"], "pass")
        # a boot AFTER the last event proves nothing
        late = real[:2] + real[3:] + ["200 [notify] boot not-determined"]
        self.assertEqual(judge("claude-restart", late, session=[S, after])["verdict"], "insufficient-evidence")
        # malformed tags are a usage error, not an empty match
        with self.assertRaises(ValueError):
            judge("claude-restart", seq, session=[])
        with self.assertRaises(ValueError):
            judge("claude-restart", seq, session=["not-a-tag"])

    def test_drops_notifications_and_drift_are_recorded_as_evidence(self):
        lines = [line(10, "codex", "working"), "11 [agent-status] dropped (interaction-mismatch)",
                 "12 [agent-status] codex identity-absent", f"13 [notify] posted turn-done s={S} e=2",
                 f"14 [notify] suppressed viewed-episode s={S} e=2", line(15, "codex", "turn-done", e=2)]
        r = judge("codex-interrupt", lines)
        self.assertEqual(r["observed"]["drops"], {"interaction-mismatch": 1})
        self.assertTrue(r["observed"]["identity_absent"])
        self.assertEqual(r["observed"]["notify"], {"posted": 1, "viewed": 0, "suppressed": 1})

    def test_the_parser_reads_only_closed_lines(self):
        # lines with anything else — free text, paths — are ignored, never echoed
        events = sc.parse(["10 [agent-status] claude-code working s=sess-ab12c target=1 v=2 e=1 /Users/x",
                           "11 [ui] something /Users/me/secret", "not a line"])
        self.assertEqual(events, [])

    def test_the_cli_writes_the_schema_and_gates_by_exit_status(self):
        with tempfile.TemporaryDirectory() as tmp:
            log, plan = os.path.join(tmp, "app.log"), os.path.join(tmp, "plan.json")
            with open(log, "w") as f:
                f.write("\n".join([line(10, "codex", "working"), line(11, "codex", "turn-done", e=2)]) + "\n")
            json.dump([{"case": "codex-normal", "session": S, "since": 0, "until": 100}], open(plan, "w"))
            script = os.path.join(os.path.dirname(__file__), "signal-candidate")
            args = [script, "--log", log, "--plan", plan, "--deck-version", "0.7.13", "--codex-version", "0.157.0"]
            done = subprocess.run(args, capture_output=True, text=True)
            self.assertEqual(done.returncode, 0, done.stderr)
            verdict = json.loads(done.stdout)
            self.assertEqual(verdict["schema"], "deck-signal-candidate/1")
            self.assertEqual(verdict["agents"]["codex"], "0.157.0")
            self.assertEqual(verdict["verdict"], "pass")
            json.dump([{"case": "codex-normal", "session": OTHER, "since": 0, "until": 100}], open(plan, "w"))
            self.assertEqual(subprocess.run(args, capture_output=True).returncode, 1, "insufficient evidence gates")
            json.dump([{"case": "no-such-case", "session": S, "since": 0, "until": 1}], open(plan, "w"))
            self.assertEqual(subprocess.run(args, capture_output=True).returncode, 2)


if __name__ == "__main__":
    unittest.main()
