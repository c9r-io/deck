#!/usr/bin/env python3 -I
"""Codex hook trust state for an isolated experiment: the `[hooks.state]`
TOML entries that mark Deck's two lifecycle hooks in a hooks.json as trusted,
computed as Codex 0.163.0 computes them (`hooks/src/engine/discovery.rs`
`hook_hash` → `config/src/fingerprint.rs` `version_for_toml`: the normalized
handler identity as JSON with sorted keys, SHA-256, `sha256:` prefix). The
replica was calibrated on 2026-10-18 against the three stored hashes of the
2026-10-17 isolated experiment (3/3 equal).

usage: hookstate.py <hooks.json path> <helper path>
Prints the TOML to append to config.toml. Reads nothing but its arguments.
"""
import hashlib
import json
import sys


def trust_hash(event_key, command, timeout_sec, is_async=False):
    group = {"hooks": [{"type": "command", "command": command, "timeout": timeout_sec, "async": is_async}]}
    identity = {"event_name": event_key, **group}
    text = json.dumps(identity, sort_keys=True, separators=(",", ":"))
    return "sha256:" + hashlib.sha256(text.encode()).hexdigest()


def main(argv):
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    hooks_json, helper = argv[1], argv[2]
    command = lambda event: f'"{helper}" codex {event}'
    # what Deck writes (`HookStyle::ShellBrief`): timeout 1, no async
    entries = [
        ("session_start", "session-start", 1),
        ("session_end", "session-end", 1),
    ]
    out = ["[hooks.state]", ""]
    for key, word, timeout in entries:
        out.append(f'[hooks.state."{hooks_json}:{key}:0:0"]')
        out.append(f'trusted_hash = "{trust_hash(key, command(word), timeout)}"')
        out.append("")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
