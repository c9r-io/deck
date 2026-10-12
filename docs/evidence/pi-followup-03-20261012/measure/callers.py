#!/usr/bin/env python3
"""callers.py — which functions of an arm64 Mach-O executable call given libSystem functions.

    callers.py <executable> <symbol> [<symbol> ...]        e.g. callers.py node _fork _posix_spawn _socket _connect

The executable is only read. Every `bl`/`b` instruction of __TEXT,__text whose target is the stub of one of
the symbols (or, for a function of the executable itself, its address) is a call site; each is named by the nearest preceding symbol of the executable's own symbol table
(`nm -n`), when it has one. A stripped executable gives call sites without names: they are counted.
"""
from __future__ import annotations

import bisect
import json
import re
import struct
import subprocess
import sys


def run(*argv: str) -> str:
    return subprocess.run(argv, capture_output=True, text=True, check=True).stdout


def main() -> int:
    exe, wanted = sys.argv[1], sys.argv[2:]
    # sections: address, size, file offset
    sections: dict[tuple[str, str], tuple[int, int, int]] = {}
    current: dict[str, str] = {}
    for line in run("/usr/bin/otool", "-l", exe).splitlines():
        line = line.strip()
        for key in ("sectname", "segname", "addr", "size", "offset"):
            if line.startswith(key + " "):
                current[key] = line.split()[1]
        if line.startswith("offset ") and "sectname" in current:
            sections[(current["segname"], current["sectname"])] = (int(current["addr"], 16), int(current["size"], 16), int(current["offset"]))
            current = {}
    # stubs: the address of each wanted symbol's stub
    stubs: dict[int, str] = {}
    table = None
    for line in run("/usr/bin/otool", "-Iv", exe).splitlines():
        head = re.match(r"Indirect symbols for \((\w+),(\w+)\)", line)
        if head:
            table = head.group(2)
            continue
        parts = line.split()
        if table in ("__stubs", "__auth_stubs") and len(parts) == 3 and parts[0].startswith("0x") and parts[2] in wanted:
            stubs[int(parts[0], 16)] = parts[2]
    # the executable's own symbols, by address
    symbols: list[tuple[int, str]] = []
    for line in run("/usr/bin/nm", "-n", exe).splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[1] in "tT":
            symbols.append((int(parts[0], 16), parts[2]))
    starts = [address for address, _ in symbols]
    # a wanted name that is one of the executable's own functions is called at its address
    for at, name in symbols:
        if name in wanted and name not in stubs.values():
            stubs[at] = name
    address, size, offset = sections[("__TEXT", "__text")]
    with open(exe, "rb") as source:
        source.seek(offset)
        text = source.read(size)
    found: dict[str, dict[str, int]] = {name: {} for name in wanted}
    for index in range(0, len(text) - 3, 4):
        word = struct.unpack_from("<I", text, index)[0]
        if word & 0x7C000000 != 0x14000000:  # b or bl
            continue
        delta = word & 0x03FFFFFF
        if delta & 0x02000000:
            delta -= 0x04000000
        target = address + index + 4 * delta
        if target not in stubs:
            continue
        at = address + index
        where = bisect.bisect_right(starts, at) - 1
        name = symbols[where][1] if where >= 0 else "(no symbol)"
        callers = found[stubs[target]]
        callers[name] = callers.get(name, 0) + 1
    print(json.dumps({"executable": exe, "own_text_symbols": len(symbols), "stubs_found": sorted(stubs.values()),
                      "callers": {name: dict(sorted(sites.items())) for name, sites in found.items()}}, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
