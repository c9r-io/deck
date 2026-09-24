// Judge one isolated WKWebView smoke run from its app.log.
//
//   scripts/smoke-verdict <data-dir> <mode>
//
// Reads only `[ui] smoke-check <name> a=<n> b=<m>` lines (closed names and
// integers; tests/log_privacy.rs keeps anything else out) and compares them
// with the mode's entry in app/ui/test/fixtures/smoke-manifest.json:
// - every listed check must appear; each occurrence needs a positive `a`,
//   or exactly the listed `a`, or (a list) exactly those values once each;
// - `metric` checks only need to appear (their `a` is a measurement);
// - `failureOnly` checks are emitted only on a carrier's exception path, so
//   their presence is a failure;
// - the last checkpoint must be the mode's terminal check with a=1 (a mode
//   with `terminal: null` stays open for an external step and has none);
// - a name outside the mode's manifest entry fails the run (including a
//   `<redacted>` name, which the logger writes for one SMOKE_CHECKS lacks):
//   a legitimate new checkpoint is added to the manifest, never tolerated.
// A relaunched mode (restart, ambiguous, review-restart) appends to the
// app.log of the run before it. Every completed run ends with exactly one
// terminal checkpoint, so only the checkpoints after the previous run's
// terminal are judged (`currentRun`).
// Exit 0 = pass, 1 = fail, 2 = usage or unreadable input.
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const LINE = /\[ui\] smoke-check (\S+) a=(-?\d+)(?: b=(-?\d+))?/;

export function parseChecks(logText) {
  const checks = [];
  for (const line of logText.split('\n')) {
    const match = LINE.exec(line);
    if (match) checks.push({ name: match[1], a: Number(match[2]), b: match[3] === undefined ? 0 : Number(match[3]) });
  }
  return checks;
}

/* The checkpoints of the latest run in a data root that earlier runs also
   wrote to: everything after the second-to-last terminal checkpoint. A run
   that never reached its terminal is judged together with what precedes it
   and fails on its missing terminal. */
export function currentRun(checks, manifest) {
  const terminals = new Set(Object.values(manifest.modes).map(spec => spec.terminal).filter(Boolean));
  const ends = checks.flatMap((check, index) => (terminals.has(check.name) ? [index] : []));
  return ends.length >= 2 ? checks.slice(ends[ends.length - 2] + 1) : checks;
}

export function judge(logText, manifest, mode) {
  const spec = manifest.modes?.[mode];
  if (!spec) throw new Error(`unknown smoke mode: ${mode}`);
  const checks = currentRun(parseChecks(logText), manifest);
  const failures = [];
  const expected = spec.checks || {};
  for (const [name, rule] of Object.entries(expected)) {
    const seen = checks.filter(check => check.name === name);
    if (rule.failureOnly) {
      if (seen.length) failures.push(`${name}: reported (its carrier's exception path ran)`);
      continue;
    }
    if (!seen.length) { failures.push(`${name}: missing`); continue; }
    if (rule.metric) continue;
    if (Array.isArray(rule.a)) {
      const got = seen.map(check => check.a).sort((x, y) => x - y);
      const want = [...rule.a].sort((x, y) => x - y);
      if (got.join(',') !== want.join(',')) failures.push(`${name}: a=${got.join(',')} (expected ${want.join(',')})`);
    } else {
      for (const check of seen) {
        if (typeof rule.a === 'number' ? check.a !== rule.a : check.a <= 0) {
          failures.push(`${name}: a=${check.a}${typeof rule.a === 'number' ? ` (expected ${rule.a})` : ''}`);
        }
      }
    }
  }
  if (spec.terminal) {
    const last = checks[checks.length - 1];
    if (!last) failures.push(`${spec.terminal}: missing (no smoke-check lines)`);
    else if (last.name !== spec.terminal) failures.push(`${spec.terminal}: not the last checkpoint (last is ${last.name})`);
    else if (last.a !== 1) failures.push(`${spec.terminal}: a=${last.a} b=${last.b}`);
  }
  const known = new Set([...Object.keys(expected), spec.terminal].filter(Boolean));
  for (const name of new Set(checks.map(check => check.name))) {
    if (!known.has(name)) failures.push(`${name}: not in the manifest for ${mode}`);
  }
  const total = Object.values(expected).filter(rule => !rule.failureOnly).length + (spec.terminal ? 1 : 0);
  return { ok: failures.length === 0, failures, total, lines: checks.length };
}

export function loadManifest(root = resolve(import.meta.dirname, '..')) {
  return JSON.parse(readFileSync(resolve(root, 'app/ui/test/fixtures/smoke-manifest.json'), 'utf8'));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [dir, mode] = process.argv.slice(2);
  if (!dir || !mode) {
    console.error('usage: scripts/smoke-verdict <data-dir> <mode>');
    process.exit(2);
  }
  let result;
  try {
    const log = readFileSync(resolve(dir, 'app.log'), 'utf8');
    result = judge(log, loadManifest(), mode);
  } catch (error) {
    console.error(`smoke-verdict: ${error.message}`);
    process.exit(2);
  }
  const passed = result.total - result.failures.length;
  console.log(`smoke-verdict ${mode}: ${result.ok ? 'PASS' : 'FAIL'} (${Math.max(0, passed)}/${result.total} expected checkpoints, ${result.lines} lines)`);
  for (const failure of result.failures) console.log(`  FAIL ${failure}`);
  process.exit(result.ok ? 0 : 1);
}
