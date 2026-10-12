#!/usr/bin/env python3
"""show.py — TEST ONLY, read-only. Prints what the witness wrote: where the holder stood, and the held threads.

    show.py [--map <linker map of the same build>] [--image NAME] [--depth N] [--others N] <witness.jsonl> ...

A frame in a system library or in Node is named by the process itself (dladdr). The Bun inside the standalone
Pi carries no names; with --map they are taken from the linker map that the same release publishes with its
symbol-bearing build (same Mach-O UUID, same bytes in every code section: checked before it is used).
"""
import bisect
import json
import subprocess
import sys

SYSCALLS = {515: "ulock_wait", 544: "ulock_wait2", 301: "psynch_mutexwait", 305: "psynch_cvwait", 363: "kevent64",
            369: "kevent_qos", 375: "kevent_id", 93: "select", 230: "poll", 3: "read", 4: "write", 396: "read_nocancel",
            397: "write_nocancel", 334: "semwait_signal", 423: "semwait_signal_nocancel", 368: "workq_kernreturn",
            -36: "semaphore_wait_trap", -38: "semaphore_timedwait_trap", -31: "mach_msg_trap", -47: "mach_msg2_trap",
            -32: "mach_msg_overwrite_trap", 0: "(none)"}
STATES = {1: "running", 2: "stopped", 3: "waiting", 4: "uninterruptible", 5: "halted"}


def load_map(path):
    starts, names = [], []
    inside = False
    with open(path, errors="replace") as lines:
        for line in lines:
            if line.startswith("# Symbols:"):
                inside = True
                continue
            if line.startswith("# Dead Stripped"):
                break
            if not inside or not line.startswith("0x"):
                continue
            parts = line.rstrip("\n").split("\t", 2)
            if len(parts) < 3:
                continue
            name = parts[2].split("] ", 1)[-1]
            starts.append(int(parts[0], 16))
            names.append((int(parts[1], 16), name))
    order = sorted(range(len(starts)), key=starts.__getitem__)
    return [starts[i] for i in order], [names[i] for i in order]


def main():
    args = sys.argv[1:]
    options = {"--map": None, "--image": "pi-standalone", "--depth": "26", "--others": "10"}
    while args and args[0] in options:
        options[args[0]] = args[1]
        args = args[2:]
    table = load_map(options["--map"]) if options["--map"] else None
    depth, others_depth = int(options["--depth"]), int(options["--others"])

    def name_of(f):
        if "image" not in f:
            return f"{f['a']}  (no image: generated code)"
        if table and f["image"] == options["--image"]:
            address = 0x100000000 + int(f["off"], 16)
            at = bisect.bisect_right(table[0], address) - 1
            if at >= 0 and address < table[0][at] + max(table[1][at][0], 1):
                return f"{f['image']}  \x00{table[1][at][1]}\x00 + {address - table[0][at]}"
            return f"{f['image']} + 0x{f['off']}"
        if "sym" in f and f["plus"] < 0x8000:
            return f"{f['image']}  \x00{f['sym']}\x00 + {f['plus']}"
        return f"{f['image']} + 0x{f['off']}"

    out = []

    def thread(t, limit):
        call = SYSCALLS.get(t["x16"], str(t["x16"]))
        out.append(f"    tid {t['tid']} port {t['port']:#x} \"{t['name']}\"  {STATES.get(t['run_state'], t['run_state'])}  suspend {t['suspend']}"
                   f"  x16={call}  x0..x3 = {t['x'][0]} {t['x'][1]} {t['x'][2]} {t['x'][3]}  word at x1 = {t['word_at_x1']}")
        for f in t["frames"][:limit]:
            out.append(f"        {name_of(f)}")

    for path in args:
        for line in open(path):
            r = json.loads(line)
            out.append(f"== {path.rsplit('/', 1)[-1]} rescue {r['rescue']}  pid {r['pid']}  the hold had lasted {r['hold_lasted_ns'] / 1e6:.0f} ms  held {r['held']}")
            a, b = r["holder_first"], r["holder_200ms_later"]
            out.append("  HOLDER, first reading:")
            thread(a, depth)
            if b:
                same = a["pc"] == b["pc"] and a["sp"] == b["sp"]
                out.append(f"  HOLDER 200 ms later: {'at the same place' if same else 'ELSEWHERE'}; charged user {b['user_ns'] - a['user_ns']} ns,"
                           f" system {b['system_ns'] - a['system_ns']} ns; {STATES.get(b['run_state'], b['run_state'])}")
            ports = {t["port"] | 1: t for t in r["others"]}
            tids = {t["tid"]: t for t in r["others"]}
            call = a["x16"]
            owner = None
            if call in (515, 544) and a["word_at_x1"]:
                word = int(a["word_at_x1"], 16) & 0xffffffff
                owner = ports.get(word | 1)
                out.append(f"  THE LOCK: ulock at {a['x'][1]}, its word {word:#x} names thread port {(word | 1):#x}: "
                           + (f"tid {owner['tid']} \"{owner['name']}\", held: {owner['held']}" if owner else "no thread of this process"))
            if call == 301:
                owner = tids.get(int(a["x"][3], 16))
                out.append(f"  THE LOCK: pthread mutex at {a['x'][0]}, owner tid {int(a['x'][3], 16)}: "
                           + (f"\"{owner['name']}\", held: {owner['held']}" if owner else "no thread of this process"))
            out.append("  HELD THREADS:" if others_depth else "")
            for t in r["others"]:
                if others_depth:
                    thread(t, others_depth)
                elif owner is t:
                    out.append("  THE OWNER:")
                    thread(t, depth)
    text = "\n".join(out)
    # the names between the marks, made readable
    pieces = text.split("\x00")
    mangled = sorted({p for p in pieces[1::2]})
    if mangled:
        plain = subprocess.run(["/usr/bin/c++filt", "-n"], input="\n".join(m[1:] if m.startswith("__Z") else m for m in mangled),
                               capture_output=True, text=True).stdout.splitlines()
        table2 = dict(zip(mangled, plain)) if len(plain) == len(mangled) else {}
        pieces[1::2] = [table2.get(p, p) for p in pieces[1::2]]
    print("".join(pieces))


main()
