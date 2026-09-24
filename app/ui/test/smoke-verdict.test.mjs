// scripts/smoke-verdict against a real isolated `settings` run's app.log
// (test/fixtures/smoke-log-settings.txt: timestamps, closed names and
// integers only) and deliberately broken copies of it.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { currentRun, judge, loadManifest, parseChecks } from '../../../scripts/smoke-verdict.mjs';

const manifest = loadManifest();
const settingsLog = readFileSync(new URL('./fixtures/smoke-log-settings.txt', import.meta.url), 'utf8');
const script = fileURLToPath(new URL('../../../scripts/smoke-verdict.mjs', import.meta.url));
const line = (name, a, b = 0) => `1790265995 [ui] smoke-check ${name} a=${a} b=${b}`;

test('the fixture log holds only timestamped closed smoke-check lines', () => {
  const lines = settingsLog.trimEnd().split('\n');
  assert.ok(lines.every(text => /^\d+ \[ui\] smoke-check [a-z0-9-]+ a=-?\d+ b=-?\d+$/.test(text)), 'no path, content or free text');
});

test('the recorded settings run passes with nothing unexpected', () => {
  const result = judge(settingsLog, manifest, 'settings');
  assert.deepEqual(result.failures, []);
  assert.equal(result.ok, true);
  assert.equal(result.total, 7);
});

test('a broken copy fails for each kind of problem', () => {
  const cases = [
    [settingsLog.replace('settings-viewport a=32', 'settings-viewport a=31'), 'settings-viewport: a=31 (expected 32)'],
    [settingsLog.replace('button-force-touch a=128', 'button-force-touch a=-128'), 'button-force-touch: a=-128'],
    [settingsLog.split('\n').filter(text => !text.includes('settings-logs')).join('\n'), 'settings-logs: missing'],
    [settingsLog.replace('done a=1', 'done a=-1'), 'done: a=-1 b=0'],
    [`${settingsLog.trimEnd()}\n${line('settings-logs', 1)}\n`, 'done: not the last checkpoint (last is settings-logs)'],
  ];
  for (const [log, failure] of cases) {
    const result = judge(log, manifest, 'settings');
    assert.equal(result.ok, false, failure);
    assert.ok(result.failures.includes(failure), `${failure} in ${JSON.stringify(result.failures)}`);
  }
});

test('a name outside the mode\'s manifest entry fails the run', () => {
  for (const name of ['dropdown', '<redacted>']) {
    const log = settingsLog.replace(line('done', 1), `${line(name, 1)}\n${line('done', 1)}`);
    const result = judge(log, manifest, 'settings');
    assert.equal(result.ok, false, name);
    assert.deepEqual(result.failures, [`${name}: not in the manifest for settings`]);
  }
});

test('exact lists, metrics, failure-only names and open modes', () => {
  const run = manifest.modes.run.checks;
  assert.deepEqual(run['split-picker-pty'], { a: [1, 2] });
  const lines = name => judge([line(name, 1), line(name, 2), line('done', 1)].join('\n'),
    { modes: { m: { terminal: 'done', checks: { [name]: run[name] } } } }, 'm');
  assert.equal(lines('split-picker-pty').ok, true);
  assert.deepEqual(judge([line('split-picker-pty', 1), line('done', 1)].join('\n'),
    { modes: { m: { terminal: 'done', checks: { 'split-picker-pty': { a: [1, 2] } } } } }, 'm').failures,
  ['split-picker-pty: a=1 (expected 1,2)']);
  const metric = { modes: { m: { terminal: 'done', checks: { gap: { metric: true } } } } };
  assert.equal(judge([line('gap', -40), line('done', 1)].join('\n'), metric, 'm').ok, true);
  const guarded = { modes: { m: { terminal: 'done', checks: { stage: { failureOnly: true } } } } };
  assert.equal(judge(line('done', 1), guarded, 'm').ok, true);
  assert.deepEqual(judge([line('stage', -1, 3), line('done', 1)].join('\n'), guarded, 'm').failures,
    ["stage: reported (its carrier's exception path ran)"]);
  assert.equal(judge(line('connector-transport-ready', 1), manifest, 'connector-transport').ok, true);
  assert.throws(() => judge(settingsLog, manifest, 'no-such-mode'), /unknown smoke mode/);
});

test('a relaunched root is judged by its latest run only', () => {
  const restart = `${settingsLog.trimEnd()}\n${line('rename-restart', 1, 1)}\n${line('done', 1)}\n`;
  assert.deepEqual(currentRun(parseChecks(restart), manifest).map(check => check.name), ['rename-restart', 'done']);
  assert.equal(judge(restart, manifest, 'restart').ok, true);
  assert.equal(judge(settingsLog, manifest, 'restart').ok, false, 'a root with only the first run');
});

test('the command line exits 0 pass, 1 fail, 2 usage', () => {
  const dir = mkdtempSync(join(tmpdir(), 'deck-smoke-verdict-'));
  try {
    const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: 'utf8' });
    writeFileSync(join(dir, 'app.log'), settingsLog);
    const pass = run(dir, 'settings');
    assert.equal(pass.status, 0, pass.stdout + pass.stderr);
    assert.match(pass.stdout, /^smoke-verdict settings: PASS \(7\/7 expected checkpoints, 7 lines\)/);
    writeFileSync(join(dir, 'app.log'), settingsLog.replace('done a=1', 'done a=-1'));
    const fail = run(dir, 'settings');
    assert.equal(fail.status, 1);
    assert.match(fail.stdout, /FAIL done: a=-1/);
    assert.equal(run(dir).status, 2);
    assert.equal(run(join(dir, 'missing'), 'settings').status, 2);
    assert.equal(run(dir, 'no-such-mode').status, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
