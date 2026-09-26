#!/usr/bin/env python3
"""Tests for the content-free Codex terminal-title probe."""

from __future__ import annotations

import importlib.util
import io
import json
import pathlib
import sys
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("codex_title_probe.py")
SPEC = importlib.util.spec_from_file_location("codex_title_probe", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
probe_module = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = probe_module
SPEC.loader.exec_module(probe_module)


class ParseTitleTests(unittest.TestCase):
    def test_accepts_exact_closed_vocabulary(self) -> None:
        expected = {
            "Ready": ("ready", False, False),
            "⠋ Starting": ("starting", True, False),
            "⠙ Working": ("working", True, False),
            "⠹ Thinking": ("thinking", True, False),
            "⠸ Waiting": ("waiting", True, False),
            "[ ! ] Action Required": (None, False, True),
            "[ . ] Action Required": (None, False, True),
        }
        for title, values in expected.items():
            with self.subTest(title=title):
                parsed = probe_module.parse_title(title)
                self.assertIsNotNone(parsed)
                assert parsed is not None
                self.assertEqual(
                    (parsed.run_state, parsed.activity, parsed.action_required), values
                )

    def test_accepts_every_upstream_spinner_frame(self) -> None:
        for frame in probe_module.SPINNER_FRAMES:
            for state in ("Starting", "Working", "Thinking", "Waiting"):
                with self.subTest(frame=frame, state=state):
                    parsed = probe_module.parse_title(f"{frame} {state}")
                    self.assertIsNotNone(parsed)
                    assert parsed is not None
                    self.assertTrue(parsed.activity)

    def test_rejects_mixed_free_text_and_near_matches(self) -> None:
        rejected = (
            "deck | Ready",
            "Ready | project",
            "Working",
            "⠋ Ready",
            "Action Required",
            "[ ! ] Action Required | 0199abcd...",
            "ready",
            " Ready",
            "Ready\n",
            "● ⠋ Working",
            "",
        )
        for title in rejected:
            with self.subTest(title=title):
                self.assertIsNone(probe_module.parse_title(title))


class GenerationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.probe = probe_module.TitleProbe()

    def observe(self, pane: str, generation: str, title: str, foreground=True):
        return self.probe.observe(
            pane=pane,
            generation=generation,
            codex_foreground=foreground,
            title=title,
        )

    def test_first_title_is_cache_baseline_until_change(self) -> None:
        first = self.observe("pane-a", "gen-1", "Ready")
        same = self.observe("pane-a", "gen-1", "Ready")
        changed = self.observe("pane-a", "gen-1", "⠋ Working")
        current = self.observe("pane-a", "gen-1", "⠋ Working")
        self.assertEqual(first["status"], "cached-baseline")
        self.assertEqual(same["status"], "cached-baseline")
        self.assertFalse(same["established_in_generation"])
        self.assertEqual(changed["status"], "changed")
        self.assertTrue(changed["established_in_generation"])
        self.assertEqual(current["status"], "current")

    def test_spinner_and_action_blink_establish_changes(self) -> None:
        self.observe("pane-a", "gen-1", "⠋ Working")
        spinner = self.observe("pane-a", "gen-1", "⠙ Working")
        action = self.observe("pane-a", "gen-1", "[ ! ] Action Required")
        blink = self.observe("pane-a", "gen-1", "[ . ] Action Required")
        self.assertEqual(spinner["status"], "changed")
        self.assertEqual(action["status"], "changed")
        self.assertTrue(action["action_required"])
        self.assertEqual(blink["status"], "changed")

    def test_generation_foreground_and_unrecognized_title_reset_continuity(self) -> None:
        self.observe("pane-a", "gen-1", "Ready")
        self.observe("pane-a", "gen-1", "⠋ Working")
        self.assertEqual(
            self.observe("pane-a", "gen-2", "Ready")["status"],
            "cached-baseline",
        )
        self.assertEqual(
            self.observe("pane-a", "gen-2", "Ready", foreground=False)["status"],
            "unavailable",
        )
        self.assertEqual(
            self.observe("pane-a", "gen-2", "Ready")["status"],
            "cached-baseline",
        )
        self.assertEqual(
            self.observe("pane-a", "gen-2", "project | Ready")["status"],
            "unrecognized",
        )
        self.assertEqual(
            self.observe("pane-a", "gen-2", "Ready")["status"],
            "cached-baseline",
        )

    def test_panes_are_independent(self) -> None:
        self.observe("pane-a", "gen-1", "Ready")
        self.observe("pane-b", "gen-1", "Ready")
        self.assertEqual(
            self.observe("pane-a", "gen-1", "⠋ Working")["status"], "changed"
        )
        self.assertEqual(
            self.observe("pane-b", "gen-1", "Ready")["status"],
            "cached-baseline",
        )


class JsonlTests(unittest.TestCase):
    def test_output_is_closed_and_does_not_echo_sources(self) -> None:
        source = io.StringIO(
            json.dumps(
                {
                    "pane": "private-pane-name",
                    "generation": "private-generation",
                    "codex_foreground": True,
                    "title": "Ready",
                }
            )
            + "\n"
        )
        sink = io.StringIO()
        self.assertEqual(probe_module.run_jsonl(source, sink), 0)
        output = sink.getvalue()
        self.assertNotIn("private", output)
        self.assertNotIn("Ready", output)
        record = json.loads(output)
        self.assertEqual(
            set(record),
            {
                "status",
                "run_state",
                "activity",
                "action_required",
                "established_in_generation",
            },
        )

    def test_malformed_input_is_content_free_and_nonzero(self) -> None:
        sink = io.StringIO()
        self.assertEqual(probe_module.run_jsonl(io.StringIO("not-json\n"), sink), 2)
        self.assertEqual(json.loads(sink.getvalue())["status"], "malformed-input")
        self.assertNotIn("not-json", sink.getvalue())

    def test_oversized_line_is_drained_before_next_record(self) -> None:
        oversized = "x" * (probe_module.MAX_LINE_BYTES * 2) + "\n"
        valid = json.dumps(
            {
                "pane": "pane-a",
                "generation": "gen-1",
                "codex_foreground": True,
                "title": "Ready",
            }
        ) + "\n"
        sink = io.StringIO()
        self.assertEqual(probe_module.run_jsonl(io.StringIO(oversized + valid), sink), 2)
        records = [json.loads(line) for line in sink.getvalue().splitlines()]
        self.assertEqual(
            [record["status"] for record in records],
            ["malformed-input", "cached-baseline"],
        )

    def test_bounded_reader_accepts_final_record_without_newline(self) -> None:
        record = json.dumps(
            {
                "pane": "pane-a",
                "generation": "gen-1",
                "codex_foreground": True,
                "title": "Ready",
            }
        )
        sink = io.StringIO()
        self.assertEqual(probe_module.run_jsonl(io.StringIO(record), sink), 0)
        self.assertEqual(json.loads(sink.getvalue())["status"], "cached-baseline")

    def test_duplicate_field_resets_previous_continuity(self) -> None:
        records = [
            '{"pane":"p","generation":"g","codex_foreground":true,"title":"Ready"}',
            '{"pane":"p","generation":"g","codex_foreground":true,"title":"⠋ Working"}',
            '{"pane":"p","pane":"p","generation":"g","codex_foreground":true,"title":"⠋ Working"}',
            '{"pane":"p","generation":"g","codex_foreground":true,"title":"⠋ Working"}',
        ]
        sink = io.StringIO()
        self.assertEqual(
            probe_module.run_jsonl(io.StringIO("\n".join(records) + "\n"), sink), 2
        )
        output = [json.loads(line) for line in sink.getvalue().splitlines()]
        self.assertEqual(
            [record["status"] for record in output],
            ["cached-baseline", "changed", "malformed-input", "cached-baseline"],
        )


if __name__ == "__main__":
    unittest.main()
