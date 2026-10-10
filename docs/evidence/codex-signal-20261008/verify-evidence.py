"""Offline verification only; does not start Codex, tmux, hooks, or listeners."""
import json
import hashlib
import re
from pathlib import Path

BASE = Path(__file__).parent
rows = [json.loads(line) for line in (BASE / 'events.jsonl').read_text().splitlines()]
analysis = json.loads((BASE / 'analysis.json').read_text())
publication = json.loads((BASE / 'publication.json').read_text())
assert publication['new_experiments'] == 0
for name, digest in publication['derived_sha256'].items():
    path = BASE / Path(name).name
    assert hashlib.sha256(path.read_bytes()).hexdigest() == digest
for record in rows + analysis['checks'] + analysis['turns']:
    assert re.fullmatch(r'session-[0-9]{2}', record['session_id'])
    if record.get('turn_id'):
        assert re.fullmatch(r'turn-[0-9]{2}', record['turn_id'])

PROCESS_KEYS = {'pid', 'ppid', 'pgid', 'tty', 'tpgid', 'sec', 'usec'}
EVENT_KEYS = {'hook_event_name', 'session_id', 'turn_id', 'pane_marker', 'socket_marker',
              'time', 'phase', 'peer_pid', 'ancestry', 'panes'}

def process(row):
    assert set(row) == PROCESS_KEYS
    assert all(isinstance(v, int) for v in row.values())

def continuity(chain, foreground, tty):
    if not tty or foreground not in [p['pid'] for p in chain]:
        return False
    segment = chain[:next(i for i, p in enumerate(chain) if p['pid'] == foreground) + 1]
    detached = next((i for i, p in enumerate(segment) if p['tty'] == tty), len(segment))
    hook, above = segment[:detached], segment[detached:]
    group = segment[0]['pgid']
    return (all(p['tty'] == 0 and p['pgid'] == group for p in hook)
            and (not hook or hook[-1]['pid'] == group)
            and bool(above) and all(p['tty'] == tty for p in above))

assert len(rows) == len(analysis['checks']) == 23
checks = {r['peer_pid']: r for r in analysis['checks']}
for row in rows:
    assert set(row) <= EVENT_KEYS
    assert row['hook_event_name'] in {'SessionStart', 'UserPromptSubmit', 'PermissionRequest', 'Stop', 'Interrupt'}
    assert row['socket_marker'] in {'A', 'E'}
    assert row['pane_marker'] == ('%0' if row['socket_marker'] == 'A' else '%2')
    assert row['peer_pid'] == row['ancestry'][0]['pid']
    for p in row['ancestry']:
        process(p)
    for child, parent in zip(row['ancestry'], row['ancestry'][1:]):
        assert child['ppid'] == parent['pid']
    check = checks[row['peer_pid']]
    for label, pane_id in [('A', '%0'), ('B', '%1'), ('E', '%2')]:
        pane = row['panes'].get(pane_id)
        if pane is None:
            assert check[label] is None
            continue
        assert set(pane) == {'pane_pid', 'process', 'foreground'}
        process(pane['process'])
        process(pane['foreground'])
        assert pane['process']['tpgid'] == pane['foreground']['pid']
        pids = [p['pid'] for p in row['ancestry']]
        expected = {
            'pane_ancestry': pane['pane_pid'] in pids,
            'foreground_ancestry': pane['foreground']['pid'] in pids,
            'terminal_continuity': continuity(row['ancestry'], pane['foreground']['pid'], pane['process']['tty']),
        }
        assert check[label] == expected
        if expected['foreground_ancestry']:
            peer_fg = next(p for p in row['ancestry'] if p['pid'] == pane['foreground']['pid'])
            assert (peer_fg['sec'], peer_fg['usec']) == (pane['foreground']['sec'], pane['foreground']['usec'])
shared = [r for r in analysis['checks'] if r['socket_marker'] == 'A']
embedded = [r for r in analysis['checks'] if r['socket_marker'] == 'E']
assert len(shared) == 18 and len(embedded) == 5
assert all(not r['A']['terminal_continuity'] and not r['B']['foreground_ancestry'] for r in shared)
assert all(all(r['E'].values()) for r in embedded)
assert len(analysis['turns']) == 10
assert len({r['session_id'] for r in rows}) == 3
print('PASS: allowlisted metadata, process generations, 18 shared failures, 5 embedded topology passes, 10 turns')
