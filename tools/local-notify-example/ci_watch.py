#!/usr/bin/env python3
"""A reference adapter for Deck local notifications (docs/local-notifications.md).

It plays a CI watcher without any CI account: each command is one change of
one build, turned into a notification and handed to Deck through the bundled
`deck-notify`. It shows the parts a real adapter needs:

- a stable event id per change, so a retry after a lost receipt is
  idempotent (`duplicate`, never a second notification);
- one notification key per build, so later changes update the same row;
- a revision that grows with each change, so an old event delivered late
  never overwrites a newer state;
- retry on exit 5 (unavailable) and 6 (no receipt) with the SAME event id,
  and no retry on 3 (invalid) or 4 (conflict).

Usage:
  ci_watch.py fail  BUILD [--attempt N] [--title T] [--body B]
  ci_watch.py pass  BUILD [--attempt N]        # resolves the notification
  ci_watch.py cancel BUILD [--attempt N]       # withdraws it
  ci_watch.py status BUILD --attempt N [--status-kind fail|pass|cancel]

`DECK_NOTIFY` names the CLI (default: the one inside /Applications/deck.app);
`DECK_NOTIFY_SOCKET` points it at another Deck's data directory.
"""
import argparse
import json
import os
import subprocess
import sys
import time

PRODUCER = "ci-example"
DEFAULT_CLI = "/Applications/deck.app/Contents/MacOS/deck-notify"


def cli() -> str:
    return os.environ.get("DECK_NOTIFY", DEFAULT_CLI)


def run(args: list[str], payload: dict | None = None) -> tuple[int, dict]:
    """One deck-notify call: (exit code, the JSON line it printed)."""
    done = subprocess.run(
        [cli(), *args],
        input=json.dumps(payload).encode() if payload is not None else None,
        capture_output=True,
        timeout=30,
        check=False,
    )
    try:
        out = json.loads(done.stdout.decode() or "{}")
    except json.JSONDecodeError:
        out = {}
    if done.stderr:
        sys.stderr.write(done.stderr.decode())
    return done.returncode, out


def publish(payload: dict, attempts: int = 5) -> dict:
    """Publish with bounded retry; the event id never changes between tries."""
    delay = 0.5
    for attempt in range(attempts):
        code, out = run(["publish", "--json-stdin"], payload)
        if code == 0:
            return out
        if code in (5, 6) and attempt + 1 < attempts:
            time.sleep(delay)
            delay = min(delay * 2, 8)
            continue
        raise SystemExit(f"deck-notify exited {code}: {out.get('code')} {out.get('message', '')}")
    raise SystemExit("unreachable")


def event_id(build: str, kind: str, attempt: int) -> str:
    return f"build-{build}-{kind}-{attempt}"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("kind", choices=["fail", "pass", "cancel", "status"])
    parser.add_argument("build")
    parser.add_argument("--attempt", type=int, default=1)
    parser.add_argument("--title")
    parser.add_argument("--body", default="")
    parser.add_argument("--status-kind", dest="status_kind", default="fail", choices=["fail", "pass", "cancel"])
    parser.add_argument("--occurred-at", dest="occurred_at", type=int, help="when the change happened (epoch seconds)")
    args = parser.parse_args()
    key = f"build-{args.build}"
    if args.kind == "status":
        code, out = run(["status", "--producer", PRODUCER, "--event-id", event_id(args.build, args.status_kind, args.attempt)])
        print(json.dumps(out, ensure_ascii=False))
        raise SystemExit(code)
    base = {
        "version": 1,
        "producer": PRODUCER,
        "event_id": event_id(args.build, args.kind, args.attempt),
        "notification_key": key,
        # each attempt is a later change of the same build
        "revision": args.attempt * 10 + {"fail": 1, "pass": 2, "cancel": 3}[args.kind],
    }
    # every field is part of the event's fingerprint: a retry must send the
    # same bytes, so a time comes from the change itself, never from now
    if args.occurred_at is not None:
        base["occurred_at"] = args.occurred_at
    if args.kind == "fail":
        payload = {
            **base,
            "type": "build.failed",
            "title": args.title or f"Build {args.build} failed (attempt {args.attempt})",
            "body": args.body,
            "url": f"https://ci.example.invalid/builds/{args.build}",
            "labels": {"project": "deck", "environment": "development"},
        }
    else:
        payload = {**base, "operation": "resolve" if args.kind == "pass" else "withdraw"}
    print(json.dumps(publish(payload), ensure_ascii=False))


if __name__ == "__main__":
    main()
