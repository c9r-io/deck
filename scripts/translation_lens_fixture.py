#!/usr/bin/env python3
"""Deterministic interactive stand-in for an Agent answer and its /copy.

Used only by scripts/translation-lens-verify.py inside an isolated Deck smoke
session. It replaces the *content provider* (a fixed synthetic English
answer), never the Lens, the pasteboard gate or the translation provider.
`/copy <version>` really transfers the current answer to the macOS general
pasteboard, as an Agent CLI would, but only as a compare-and-write: it
refuses unless the pasteboard is still at <version> (the smoke guard's
permit), and it leaves a receipt — the changeCount its own clearContents()
returned — in `copy-receipt` next to this file. Without a version it writes
nothing. The check and the write are two steps (NSPasteboard has no
cross-process compare-and-swap), so a narrow race window remains. This is a
controlled /copy workflow, not a vendor Agent implementation.

Line mode (default) commands: /copy <version>, /next, /stream N, /history N,
/clear, /quit. `--mouse` starts an alternate-screen viewer with SGR mouse reporting
that scrolls its own history on wheel sequences (an Agent-style history
scroll that reaches Deck only as PTY redraws).
"""

import os
import subprocess
import sys
import termios
import time
import tty

ANSWERS = [
    "The build completed successfully and all tests passed. "
    "The configuration file is stored in src/intelligence/translation.rs, "
    "and the report is available at https://example.com/reports/build-42. "
    "Run `cargo test --workspace` again after you change the provider. "
    "The function translation_translate returns a typed error when the text is empty.",
    "The second answer explains how to restart the server safely. "
    "Save your work first, then stop the process and start it again. "
    "The log file is written to /tmp/deck-example/server.log every minute.",
]
HISTORY = [f"History line {i:03d}: the service processed request number {i} without errors."
           for i in range(1, 201)]


def say(text=""):
    sys.stdout.write(text + "\n")
    sys.stdout.flush()


WRITE = """function run(argv) {
  ObjC.import('AppKit');
  const board = $.NSPasteboard.generalPasteboard;
  if (Number(board.changeCount) !== Number(argv[1])) return 'refused';
  const receipt = board.clearContents;
  board.setStringForType($(argv[0]), $.NSPasteboardTypeString);
  return String(receipt);
}"""
RECEIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "copy-receipt")


def copy(text, version):
    """Compare-and-write; returns the receipt or None when refused."""
    if not version.isdigit():
        return None
    out = subprocess.run(["osascript", "-l", "JavaScript", "-e", WRITE, text, version],
                         capture_output=True, text=True, check=False).stdout.strip()
    if not out.isdigit():
        return None
    with open(RECEIPT + ".tmp", "w") as receipt:
        receipt.write(out)
    os.replace(RECEIPT + ".tmp", RECEIPT)
    return out


def line_mode():
    current = 0
    say("Fixture agent (controlled /copy stand-in).")
    say()
    say(ANSWERS[current])
    while True:
        sys.stdout.write("> ")
        sys.stdout.flush()
        line = sys.stdin.readline()
        if not line:
            return
        command, _, arg = line.strip().partition(" ")
        if command == "/copy":
            if copy(ANSWERS[current], arg.strip()):
                say(f"Copied the answer ({len(ANSWERS[current])} characters).")
            else:
                say("Refused: the pasteboard is not at the permitted version.")
        elif command == "/next":
            current = (current + 1) % len(ANSWERS)
            say(ANSWERS[current])
        elif command == "/stream":
            for i in range(1, int(arg or "50") + 1):
                say(f"Stream line {i:03d}: the worker finished step {i} and saved the result.")
                time.sleep(0.1)
        elif command == "/history":
            for row in HISTORY[: int(arg or "120")]:
                say(row)
        elif command == "/clear":
            sys.stdout.write("\033[2J\033[H")
            sys.stdout.flush()
        elif command == "/quit":
            return
        elif command:
            say(f"Unknown command: {command}")


def mouse_mode():
    fd = sys.stdin.fileno()
    saved = termios.tcgetattr(fd)
    offset = len(HISTORY)
    out = sys.stdout

    def draw():
        rows = os.get_terminal_size().lines
        top = max(0, min(offset, len(HISTORY)) - rows)
        out.write("\033[H\033[2J" + "\r\n".join(HISTORY[top:top + rows]))
        out.flush()

    try:
        tty.setraw(fd)
        out.write("\033[?1049h\033[?1000h\033[?1006h")
        draw()
        buffer = b""
        while True:
            chunk = os.read(fd, 64)
            if not chunk or b"q" in chunk:
                return
            buffer += chunk
            while b"\033[<" in buffer and (b"M" in buffer or b"m" in buffer):
                start = buffer.index(b"\033[<")
                ends = [i for i in (buffer.find(b"M", start), buffer.find(b"m", start)) if i != -1]
                if not ends:
                    break
                end = min(ends)
                button = buffer[start + 3:end].split(b";")[0]
                buffer = buffer[end + 1:]
                if button == b"64":
                    offset = max(os.get_terminal_size().lines, offset - 3)
                elif button == b"65":
                    offset = min(len(HISTORY), offset + 3)
                draw()
    finally:
        out.write("\033[?1006l\033[?1000l\033[?1049l")
        out.flush()
        termios.tcsetattr(fd, termios.TCSADRAIN, saved)


if __name__ == "__main__":
    mouse_mode() if "--mouse" in sys.argv else line_mode()
