#!/usr/bin/env python3
"""Read-only observer for the clock-live isolated product driver.

No queue writes, dispatch, terminal input, synthetic hooks, focus, retries or
rescue. Deadlines are frozen when saved rules are first observed, before due.
Only exact assistant messages count; the expected joined marker is absent
from the input. Credentials and complete Agent transcripts are never exported.
"""
import argparse
import hashlib
import json
import re
import subprocess
import time
from pathlib import Path


def read(path):
    try:
        value = json.loads(path.read_text())
        return value.get('data', value) if isinstance(value, dict) else value
    except (OSError, ValueError):
        return {}


def replies(root, expected):
    found = []
    for directory in ['claude/projects', 'codex/sessions']:
        for path in (root / directory).glob('**/*.jsonl'):
            for number, line in enumerate(path.read_text(errors='replace').splitlines(), 1):
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                message = record.get('message', record.get('payload', {}))
                if message.get('role') != 'assistant':
                    continue
                text = ''.join(part.get('text', '') for part in message.get('content', []) if isinstance(part, dict)).strip()
                if text in expected:
                    found.append({'marker': text, 'timestamp': record.get('timestamp'),
                                  'role': 'assistant', 'text': text,
                                  'transcript_reference': str(path.relative_to(root)), 'line': number})
    return found


def observe(root, data, socket, evidence):
    evidence.mkdir(parents=True, exist_ok=True)
    config = read(data / 'settings.json')
    rules = [rule for rule in config.get('inbound', {}).get('rules', []) if rule.get('name', '').startswith('clock-live-')]
    if not rules:
        raise SystemExit('No configured clock-live rules; setup must finish before observation.')
    trials = []
    for rule in rules:
        match = re.fullmatch(r'clock-live-(A|B|C1|C2|C3|D|E)-(\d+)', rule['name'])
        if not match:
            raise SystemExit('Unexpected test rule identity')
        track, due_ms = match.groups()
        due = int(due_ms) // 1000
        if time.time() >= due:
            raise SystemExit('Observation deadlines must be declared before the real trigger.')
        count = 3 if track.startswith('C') or track == 'D' else 1
        trials.append({'track': track, 'rule': rule['id'], 'configured_due_epoch': due,
                       'timezone': time.tzname, 'deadline_epoch': due + (780 if track.startswith('C') else 180),
                       'control_observation_seconds': 90 if track == 'A' else None,
                       'expected_markers': [f'CLOCK{due_ms}-{track}-REPLY{step}' for step in range(1, count + 1)],
                       'first_send_enabled': rule.get('firstSendWithoutReadiness') is True,
                       'review_each': rule.get('reviewEach') is True,
                       'user_interventions_after_arming': 0, 'tester_assist_actions_after_arming': 0,
                       'armed_at_epoch': time.time(), 'verdict': 'RUNNING'})
    (evidence / 'declared-trials.json').write_text(json.dumps(trials, indent=2))
    expected = {marker for trial in trials for marker in trial['expected_markers']}
    previous = None
    known = {}
    last_capture = 0
    while any(trial['verdict'] == 'RUNNING' for trial in trials):
        now = time.time()
        board, queue, inbound = [read(data / name) for name in ['deck.json', 'queue.json', 'inbound.json']]
        actual_replies = replies(root, expected)
        snapshot = {'observed_at_epoch': now, 'queue': queue, 'inbound': inbound,
                    'cards': [{key: card.get(key) for key in ['id', 'session', 'origin', 'cmd', 'dir', 'inboundPlan']}
                              for card in board.get('cards', [])]}
        digest = hashlib.sha256(json.dumps({k: v for k, v in snapshot.items() if k != 'observed_at_epoch'}, sort_keys=True).encode()).hexdigest()
        if digest != previous:
            with (evidence / 'runtime-timeline.jsonl').open('a') as handle:
                handle.write(json.dumps(snapshot).replace(str(root), '<isolated>') + '\n')
            previous = digest
        for trial in trials:
            if trial['verdict'] != 'RUNNING':
                continue
            cards = [card for card in board.get('cards', []) if card.get('origin', {}).get('badge') == trial['rule']]
            if len(cards) > 1:
                trial.update(verdict='FAIL', reason='duplicate cards')
                continue
            if cards:
                card = cards[0]
                trial['card'], trial['session'] = card['id'], card['session']
                trial.setdefault('card_first_observed_epoch', now)
                trial['native_slot'] = card['origin']['key']
                rows = [row for row in queue.get('items', []) if row.get('session') == card['session']]
                deliveries = [item for item in queue.get('deliveries', []) if item.get('session') == card['session']]
                trial['deliveries'], trial['remaining_rows'] = deliveries, rows
                for row in rows:
                    identity = row.get('binding')
                    if identity:
                        trial.setdefault('binding_first_observed_epoch', now)
                        trial.setdefault('first_binding', identity)
                        known[card['session']] = trial['track']
                observed = [reply for reply in actual_replies if reply['marker'] in trial['expected_markers']]
                trial['actual_replies'] = observed
                if any(sum(reply['marker'] == marker for reply in observed) > 1 for marker in trial['expected_markers']):
                    trial.update(verdict='FAIL', reason='duplicate assistant reply')
                elif any(delivery.get('manual') or delivery.get('assumed') for delivery in deliveries):
                    trial.update(verdict='FAIL', reason='manual or assumed delivery')
                elif any(row.get('state') in ['ambiguous', 'failed'] or row.get('attempts', 0) > 1 for row in rows):
                    trial.update(verdict='FAIL', reason='ambiguous, failed or retried delivery')
                elif trial['track'] == 'A' and now >= trial['configured_due_epoch'] + 90:
                    safe = not deliveries and not observed and len(rows) == 1 and rows[0].get('attempts', 0) == 0 and not rows[0].get('readiness_override') and bool(rows[0].get('binding'))
                    trial.update(verdict='PASS' if safe else 'FAIL', reason='default FirstInteraction control')
                elif trial['track'] == 'D' and len(observed) == 1 and len(deliveries) == 1 and any(row.get('state') == 'review' for row in rows):
                    trial.update(verdict='PASS', reason='configured review checkpoint held')
                elif trial['track'] not in ['A', 'D'] and len(observed) == len(trial['expected_markers']) and len(deliveries) == len(observed):
                    markers = [reply['marker'] for reply in sorted(observed, key=lambda reply: reply['timestamp'])]
                    flags = [bool(delivery.get('readiness_overridden')) for delivery in sorted(deliveries, key=lambda delivery: delivery['at'])]
                    ok = markers == trial['expected_markers'] and flags == [True] + [False] * (len(flags) - 1)
                    trial.update(verdict='PASS' if ok else 'FAIL', reason='processed replies and delivery order/audit')
            if trial['verdict'] == 'RUNNING' and now >= trial['deadline_epoch']:
                trial.update(verdict='FAIL', reason='declared timeout', timeout=True)
            if trial['verdict'] != 'RUNNING':
                trial['window_ended_epoch'] = now
                print(trial['track'], trial['verdict'], trial['reason'], flush=True)
        if known and now - last_capture >= 30:
            # Passive capture only; never attach, focus, redraw or send input.
            tmux = Path(__file__).resolve().parents[1] / 'app/src-tauri/target/debug/deck-smoke.app/Contents/MacOS/tmux'
            for session, track in known.items():
                result = subprocess.run([str(tmux), '-L', socket, 'capture-pane', '-p', '-t', '=' + session, '-S', '-80'], capture_output=True, text=True)
                if result.returncode == 0:
                    (evidence / f'{track}-terminal.txt').write_text(result.stdout.replace(str(root), '<isolated>').replace('/private<isolated>', '<isolated>'))
            last_capture = now
        (evidence / 'trial-results.json').write_text(json.dumps(trials, indent=2).replace(str(root), '<isolated>'))
        (evidence / 'assistant-replies.jsonl').write_text(''.join(json.dumps(reply) + '\n' for reply in actual_replies))
        time.sleep(2)
    (evidence / 'app.log').write_text((data / 'app.log').read_text())
    # Spaced real slots give each first-send worker an unambiguous native log interval.
    starts, ends = [], []
    for line in (data / 'app.log').read_text().splitlines():
        match = re.match(r'(\d+) \[queue\] first-send stabilization (started|finished) for (sess-[0-9a-f]+)(.*)', line)
        if match:
            record = {'epoch': int(match[1]), 'session_tag': match[3], 'detail': match[4]}
            (starts if match[2] == 'started' else ends).append(record)
    for trial in trials:
        if trial['first_send_enabled']:
            candidates = [start for start in starts if trial['configured_due_epoch'] <= start['epoch'] < trial['configured_due_epoch'] + 60]
            if len(candidates) == 1:
                start = candidates[0]
                trial['stabilization_start'] = start
                trial['stabilization_end'] = next((end for end in ends if end['session_tag'] == start['session_tag']), None)
    (evidence / 'trial-results.json').write_text(json.dumps(trials, indent=2).replace(str(root), '<isolated>'))
    return 0 if all(trial['verdict'] == 'PASS' for trial in trials) else 1


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--socket', required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    args = parser.parse_args()
    if not args.root.is_absolute() or not args.data.is_relative_to(args.root) or not args.socket.startswith('deck-smoke-'):
        raise SystemExit('Private absolute test root/data and deck-smoke socket required')
    raise SystemExit(observe(args.root, args.data, args.socket, args.evidence))
