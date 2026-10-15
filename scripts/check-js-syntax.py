#!/usr/bin/env python3
"""Check native frontend modules recursively, including extension directories."""
import pathlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[1]
files = sorted(path for directory in (root / "app/ui/js", root / "app/ui/test")
               for path in directory.rglob("*") if path.suffix in (".js", ".mjs"))
for path in files:
    result = subprocess.run(["node", "--check", str(path)], capture_output=True, text=True)
    if result.returncode:
        print(result.stderr, file=sys.stderr)
        sys.exit(result.returncode)
print(f"JS syntax: {len(files)} modules passed")
