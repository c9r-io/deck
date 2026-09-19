import test from 'node:test';
import assert from 'node:assert/strict';
import { createResumeCache, resumeCommands, resumeTarget } from '../js/resume-model.js';

const A = '01a0b76c-b577-7493-86ef-a1f6209b823e';
const B = '0f3ab19c-4d2e-4a71-9b8c-1d2e3f4a5b6c';
const hints = [{ agent: 'codex', id: A }, { agent: 'claude', id: B }];

test('resume completion preserves flags, whitespace, quoted values and partial UUID casing', () => {
  for (const line of [
    'codex resume', 'codex --yolo resume ', 'codex resume --yolo',
    'codex  --model gpt-5 --profile work resume  ',
    'codex -c model="some model" resume', "codex --cd '/tmp/my repo' resume",
  ]) {
    const result = resumeCommands(line, hints);
    assert.equal(result.length, 1, line);
    assert.ok(result[0].startsWith(line));
    assert.ok(result[0].endsWith(A));
  }
  assert.deepEqual(resumeCommands('codex --yolo resume 01A0', hints), ['codex --yolo resume 01A0' + A.slice(4)]);
  for (const line of ['claude --resume', 'claude -r ', 'claude --dangerously-skip-permissions --resume']) {
    assert.deepEqual(resumeCommands(line, hints), [line + (line.endsWith(' ') ? '' : ' ') + B]);
  }
  assert.deepEqual(resumeCommands('codex resume fff', hints), []);
});

test('ambiguous grammar, existing IDs and unrelated commands never trigger capture', () => {
  for (const line of [
    null, '', 'codex', 'claude', 'echo codex resume', ' codex resume', '"codex" resume',
    'codex exec resume', 'codex resume --last', 'claude --continue',
    'codex --unknown resume', 'codex --model', 'codex --model --yolo resume',
    'codex --model resume', 'codex resume resume', 'claude -r --resume',
    'codex resume ' + A, 'codex resume named-session', 'codex resume 01a0 ',
    'codex resume "01a0"', 'codex resume; echo hi', 'codex resume | cat',
    'codex resume\necho hi', 'codex resume\t', 'codex --cd $(pwd) resume',
    'codex --cd "$HOME" resume', 'codex --cd "`pwd`" resume',
    'codex --cd "unfinished resume', 'codex --cd /tmp/my\\ repo resume',
    'codex ' + 'x'.repeat(4097),
  ]) assert.equal(resumeTarget(line), null, String(line));
});

test('only valid IDs of the requested tool are returned, deduplicated in source order', () => {
  assert.deepEqual(resumeCommands('codex resume', [
    ...hints, hints[0], { agent: 'codex', id: 'bad; echo hi' }, { agent: 'codex', id: B },
  ]), ['codex resume ' + A, 'codex resume ' + B]);
  assert.deepEqual(resumeCommands('echo', hints), []);
});

const flush = () => new Promise(resolve => setImmediate(resolve));
test('pane cache coalesces requests and revokes stale results across focus/submit/reset', async () => {
  const pending = [], changed = [];
  const cache = createResumeCache(owner => new Promise((resolve, reject) => pending.push({ owner, resolve, reject })), owner => changed.push(owner));
  const a = {}, b = {};
  assert.deepEqual(cache.get(a), []);
  cache.get(a);
  await flush();
  assert.equal(pending.length, 1);
  cache.reset(); // A → B
  cache.get(b);
  await flush();
  cache.reset(); // B → A, old A must still be discarded
  cache.get(a);
  await flush();
  pending[0].resolve(hints);
  pending[1].resolve(hints);
  await flush();
  assert.deepEqual(changed, []);
  assert.deepEqual(cache.get(a), []);
  pending[2].resolve([hints[0]]);
  await flush();
  assert.deepEqual(changed, [a]);
  assert.deepEqual(cache.get(a), [hints[0]]);
  cache.get(b);
  await flush();
  pending[3].reject(new Error('pane ended'));
  await flush();
  assert.deepEqual(cache.get(b), []);
  assert.equal(pending.length, 4, 'failed reads do not loop on each render');
});
