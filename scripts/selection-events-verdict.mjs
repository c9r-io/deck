// Judge debug-only, real physical input after the selection-events smoke is
// ready. Reads closed event names, bounded integers and numeric IDs only.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const linePattern = /\[ui\] (terminal-selection|terminal-copy) ([a-z-]+)(?: a=(-?\d+))?(?: b=(-?\d+))? run=(\d+) pane=(\d+) selection=(\d+) gesture=(\d+) attempt=(\d+)/;
export function judgeSelectionEvents(log) {
  const ready = log.lastIndexOf('[ui] smoke-check selection-events-ready a=1');
  if (ready < 0) return { ok: false, missing: ['ready'] };
  const events = log.slice(ready).split('\n').flatMap(line => {
    const match = linePattern.exec(line);
    return match ? [{ code: match[1], detail: match[2], a: Number(match[3] || 0),
      b: Number(match[4] || 0), run: Number(match[5]), pane: Number(match[6]),
      selection: Number(match[7]), gesture: Number(match[8]), attempt: Number(match[9]) }] : [];
  });
  const groups = new Map();
  for (const e of events) {
    if (!e.gesture) continue;
    const key = `${e.run}/${e.pane}/${e.gesture}`;
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(e);
  }
  const gestures = [...groups.values()];
  const has = (group, detail, check = () => true) => group.some(e => e.detail === detail && check(e));
  const short = group => has(group, 'event-end', e => e.a === 1)
    && group.some(e => ['event-pointer', 'event-compat', 'event-up'].includes(e.detail)
      && e.a === 0 && Math.abs(e.b) >= 1 && Math.abs(e.b) <= 3);
  const long = group => has(group, 'event-end', e => e.a === 1)
    && group.some(e => ['event-pointer', 'event-compat', 'event-up'].includes(e.detail)
      && Math.abs(e.a) >= 3);
  const copied = group => has(group, 'event-end', e => e.a === 1)
    && has(group, 'keydown-deck') && has(group, 'success');
  const native = group => has(group, 'event-mousedown', e => e.a === 2 && e.b === 1)
    && has(group, 'keydown-native') && has(group, 'success');
  const checks = {
    click: gestures.some(group => has(group, 'event-end', e => e.a === 0)
      && !has(group, 'event-promote-pointer') && !has(group, 'event-promote-compat')
      && !has(group, 'event-promote-up')),
    short: gestures.some(short), long: gestures.some(long),
    dragCopy: gestures.some(copied), doubleCopy: gestures.some(native),
  };
  const missing = Object.entries(checks).filter(([, yes]) => !yes).map(([name]) => name);
  return { ok: missing.length === 0, missing };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const path = process.argv[2];
  if (!path) { console.error('usage: scripts/selection-events-verdict.mjs <isolated-data-dir>'); process.exit(2); }
  try {
    const result = judgeSelectionEvents(readFileSync(resolve(path, 'app.log'), 'utf8'));
    console.log(`selection-events: ${result.ok ? 'PASS' : 'FAIL'}${result.missing.length ? ` missing=${result.missing.join(',')}` : ''}`);
    process.exit(result.ok ? 0 : 1);
  } catch (error) { console.error(`selection-events: unreadable log (${error.code || 'error'})`); process.exit(2); }
}
