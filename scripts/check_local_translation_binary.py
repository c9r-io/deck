#!/usr/bin/env python3
"""Inspect the final Mach-O: no dynamic translation engine or Apple MT load.

Bergamot is built from pinned repository source into a static archive. This
artifact check rejects a future dynamic/helper replacement or accidental
Apple Translation/FoundationModels framework dependency. It intentionally
examines Mach-O load commands, not source text or linker flags.
"""
from __future__ import annotations

import plistlib
import subprocess
import sys
from pathlib import Path

FORBIDDEN = ("/Translation.framework/", "/FoundationModels.framework/",
             "libbergamot", "libmarian", "libdeck_bergamot")
STATIC_MARKERS = ("bergamot-mode", "int8shiftAlphaAll")
# native/SmokeBridge.swift (own-window event injection, webview snapshots,
# pasteboard guard) is compiled into debug builds only.
DEBUG_ONLY_SYMBOLS = ("_deck_smoke_mouse", "_deck_smoke_scroll", "_deck_smoke_key",
                      "_deck_smoke_snapshot", "_deck_smoke_pb_guard_begin")
DEBUG_ONLY_MARKER = "io.c9r.deck.smoke.translation."


def dependencies(otool_text: str) -> list[str]:
    return [line.strip().removeprefix("name ").split(" (offset", 1)[0]
            for line in otool_text.splitlines() if line.strip().startswith("name ")]


def executable_at(path: Path) -> Path:
    if not path.is_dir():
        return path
    with (path / "Contents/Info.plist").open("rb") as source:
        name = plistlib.load(source)["CFBundleExecutable"]
    return path / "Contents/MacOS" / name


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: check_local_translation_binary.py deck.app|executable", file=sys.stderr)
        return 2
    executable = executable_at(Path(sys.argv[1]))
    result = subprocess.run(["otool", "-l", str(executable)], capture_output=True,
                            text=True, check=True)
    found = [name for name in dependencies(result.stdout)
             if any(forbidden in name for forbidden in FORBIDDEN)]
    if found:
        print(f"forbidden dynamic translation dependency: {found}", file=sys.stderr)
        return 1
    contents = subprocess.run(["strings", "-a", str(executable)], capture_output=True,
                              text=True, check=True).stdout
    if any(marker not in contents for marker in STATIC_MARKERS):
        print("in-process Bergamot bridge markers are absent", file=sys.stderr)
        return 1
    symbols = subprocess.run(["nm", "-j", str(executable)], capture_output=True, text=True).stdout.split()
    leaked = [symbol for symbol in DEBUG_ONLY_SYMBOLS if symbol in symbols]
    leaked += [DEBUG_ONLY_MARKER] if DEBUG_ONLY_MARKER in contents else []
    if leaked:
        print(f"debug-only smoke driver present in this executable: {leaked}", file=sys.stderr)
        return 1
    if Path(sys.argv[1]).is_dir():
        helpers = [p.name for p in Path(sys.argv[1]).glob("Contents/MacOS/*")
                   if p != executable and any(word in p.name.lower()
                   for word in ("bergamot", "marian", "translation"))]
        if helpers:
            print(f"unexpected translation helper in app bundle: {helpers}", file=sys.stderr)
            return 1
    print(f"static Bergamot markers present; no dynamic translation dependency in {executable}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
