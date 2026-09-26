#!/usr/bin/env python3
"""Offline, content-free classifier for the Codex 0.157.1 title prototype.

The accepted grammar assumes the exact Codex configuration
``tui.terminal_title = ["activity", "run-state"]``.  It deliberately does
not accept mixed titles, thread names, project names, or thread identifiers.
The command-line interface reads bounded JSON Lines from stdin and emits only
closed classifications; it never echoes a pane id, generation, or title.
"""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass
from typing import TextIO


MAX_LINE_BYTES = 4096
MAX_RECORDS = 10_000
MAX_TRACKED_PANES = 512
MAX_KEY_CHARS = 128
MAX_TITLE_CHARS = 240

SPINNER_FRAMES = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
_ACTIVE_TITLE = re.compile(
    rf"^(?P<spinner>[{SPINNER_FRAMES}]) (?P<state>Starting|Working|Thinking|Waiting)$"
)
_ACTION_TITLES = frozenset(("[ ! ] Action Required", "[ . ] Action Required"))


@dataclass(frozen=True)
class ParsedTitle:
    run_state: str | None
    activity: bool
    action_required: bool
    canonical: str


@dataclass
class _PaneState:
    generation: str
    last_title: str
    established: bool = False


def parse_title(title: str) -> ParsedTitle | None:
    """Parse only the closed title grammar emitted by the prototype config."""
    if not isinstance(title, str) or not title or len(title) > MAX_TITLE_CHARS:
        return None
    if title == "Ready":
        return ParsedTitle("ready", False, False, title)
    if title in _ACTION_TITLES:
        return ParsedTitle(None, False, True, title)
    match = _ACTIVE_TITLE.fullmatch(title)
    if match is None:
        return None
    return ParsedTitle(match.group("state").lower(), True, False, title)


class TitleProbe:
    """Track title changes independently for each pane/process generation."""

    def __init__(self, max_panes: int = MAX_TRACKED_PANES) -> None:
        self._panes: dict[str, _PaneState] = {}
        self._max_panes = max_panes

    def reset(self) -> None:
        """Drop all continuity proof after an input-stream gap or corruption."""
        self._panes.clear()

    @staticmethod
    def _result(
        status: str,
        parsed: ParsedTitle | None = None,
        *,
        established: bool = False,
    ) -> dict[str, object]:
        return {
            "status": status,
            "run_state": parsed.run_state if parsed else None,
            "activity": parsed.activity if parsed else False,
            "action_required": parsed.action_required if parsed else False,
            "established_in_generation": established,
        }

    def observe(
        self,
        *,
        pane: str,
        generation: str,
        codex_foreground: bool,
        title: str,
    ) -> dict[str, object]:
        """Classify one caller-supplied tmux snapshot without returning source data."""
        if (
            not isinstance(pane, str)
            or not pane
            or len(pane) > MAX_KEY_CHARS
            or not isinstance(generation, str)
            or not generation
            or len(generation) > MAX_KEY_CHARS
            or type(codex_foreground) is not bool
            or not isinstance(title, str)
        ):
            return self._result("malformed-input")

        if not codex_foreground:
            self._panes.pop(pane, None)
            return self._result("unavailable")

        parsed = parse_title(title)
        if parsed is None:
            # An uncontrolled title interrupts continuity.  A later allowed
            # title must start from a new baseline rather than looking fresh.
            self._panes.pop(pane, None)
            return self._result("unrecognized")

        previous = self._panes.get(pane)
        if previous is None or previous.generation != generation:
            if previous is None and len(self._panes) >= self._max_panes:
                return self._result("capacity-exceeded")
            self._panes[pane] = _PaneState(generation, parsed.canonical)
            return self._result("cached-baseline", parsed)

        if previous.last_title != parsed.canonical:
            previous.last_title = parsed.canonical
            previous.established = True
            return self._result("changed", parsed, established=True)

        status = "current" if previous.established else "cached-baseline"
        return self._result(status, parsed, established=previous.established)


def _malformed_result() -> dict[str, object]:
    return TitleProbe._result("malformed-input")


def _object_without_duplicates(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate field")
        result[key] = value
    return result


def _bounded_lines(source: TextIO):
    """Yield one bounded line and whether it exceeded the byte limit."""
    while True:
        chunk = source.readline(MAX_LINE_BYTES + 1)
        if chunk == "":
            return
        oversized = len(chunk.encode("utf-8")) > MAX_LINE_BYTES
        if chunk.endswith("\n"):
            yield None if oversized else chunk, oversized
            continue
        tail = source.readline(MAX_LINE_BYTES + 1)
        if tail == "":
            yield None if oversized else chunk, oversized
            return
        oversized = True
        while tail and not tail.endswith("\n"):
            tail = source.readline(MAX_LINE_BYTES + 1)
        yield None, oversized


def run_jsonl(source: TextIO, sink: TextIO) -> int:
    """Run the bounded JSONL interface. Returns nonzero after malformed input."""
    probe = TitleProbe()
    had_error = False
    for index, (line, oversized) in enumerate(_bounded_lines(source)):
        if index >= MAX_RECORDS:
            sink.write(json.dumps(TitleProbe._result("record-limit")) + "\n")
            return 2
        if oversized:
            probe.reset()
            sink.write(json.dumps(_malformed_result()) + "\n")
            had_error = True
            continue
        assert line is not None
        try:
            record = json.loads(line, object_pairs_hook=_object_without_duplicates)
            if not isinstance(record, dict):
                raise ValueError("record is not an object")
            result = probe.observe(
                pane=record.get("pane"),
                generation=record.get("generation"),
                codex_foreground=record.get("codex_foreground"),
                title=record.get("title"),
            )
        except (json.JSONDecodeError, TypeError, ValueError):
            result = _malformed_result()
        if result["status"] in {"malformed-input", "capacity-exceeded"}:
            probe.reset()
        had_error |= result["status"] in {
            "malformed-input",
            "capacity-exceeded",
        }
        sink.write(json.dumps(result, separators=(",", ":")) + "\n")
    return 2 if had_error else 0


if __name__ == "__main__":
    raise SystemExit(run_jsonl(sys.stdin, sys.stdout))
