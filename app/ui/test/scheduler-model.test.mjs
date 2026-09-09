// The ⏱ panel's DOM-free half: what a queue item reads as and which backend
// calls start a list. The panel itself (scheduler.js) is verified by the
// WKWebView smoke; these pin the words and payloads it places.
import test from 'node:test';
import assert from 'node:assert/strict';
import { chainWhenSuffix, contextLabel, fmtClock, fmtWhen, listStartCalls, localizedChainQuietHint, qMeta } from '../js/scheduler-model.js';
import { formatInterval, setLocale } from '../js/i18n.js';

setLocale('en');
const now = new Date(2026, 2, 15, 12, 0, 0);            // 2026-03-15 12:00 local
const secs = d => Math.floor(d.getTime() / 1000);
const at = (h, m, dayOffset = 0) => secs(new Date(2026, 2, 15 + dayOffset, h, m, 0));

test('an instant reads as a clock today and gains its date on another day', () => {
  assert.doesNotMatch(fmtClock(at(9, 30), now), /3\/1[46]/);
  assert.match(fmtClock(at(9, 30, -1), now), /3\/14/);
  assert.equal(fmtWhen({ mode: 'chain' }, now), '↳ once quiet');
  assert.equal(fmtWhen({ mode: 'every', every: 900 }, now), `↻ every ${formatInterval(900)}`);
  assert.equal(fmtWhen({ mode: 'at', at: at(9, 30) }, now), fmtClock(at(9, 30), now));
});

test('the quiet hint counts up to the row\'s own quiet time and names a stopped session', () => {
  assert.equal(localizedChainQuietHint(5, true, 30), ' · quiet 5s/30s');
  assert.equal(localizedChainQuietHint(45.9, true, 30), ' · quiet ✓');
  assert.equal(localizedChainQuietHint(null, true, 30), '');
  assert.equal(localizedChainQuietHint(5, false, 30), ' · session stopped');
  const card = { idle: 12, status: 'running' };
  assert.equal(chainWhenSuffix({ mode: 'chain', quiet_secs: 60 }, card), ' · quiet 12s/60s');
  assert.equal(chainWhenSuffix({ mode: 'at' }, card), '');
  assert.equal(chainWhenSuffix({ mode: 'chain' }, null), '');
});

test('a context label names the expected process on a foreground mismatch and is empty without a probe', () => {
  assert.equal(contextLabel({ last_context: { status: 'foreground-different' }, expected_process: 'claude' }),
    'waiting for claude to return to the foreground');
  assert.equal(contextLabel({ status: 'foreground-different' }), 'waiting for ? to return to the foreground');
  assert.equal(contextLabel(null), '');
  assert.notEqual(contextLabel({ status: 'ready' }), '');
});

test('the meta line states delivery state, template position, window, count and stop', () => {
  assert.equal(qMeta({ mode: 'at', state: 'pending' }, now), '');
  assert.equal(qMeta({ mode: 'at', state: 'firing' }, now), 'sending…');
  assert.match(qMeta({ mode: 'at', state: 'ambiguous', tpl: 'deploy', tpl_idx: 2, tpl_total: 3 }, now), /^Delivery uncertain.* · tpl·deploy 2\/3$/);
  const paused = qMeta({ mode: 'every', every: 600, paused: true, win_from: 540, win_to: 1020, fired: 3, until_n: 5 }, now);
  assert.equal(paused, '09:00–17:00 · paused · 3×/5 · stops after 5×');
  const failed = qMeta({ mode: 'every', every: 600, paused: true, state: 'failed', attempts: 2, until_at: at(18, 0) }, now);
  assert.equal(failed, `paused · ⚠ send failed ×2 · until ${fmtClock(at(18, 0), now)}`);
  const notYet = qMeta({ mode: 'every', every: 600, not_before: at(9, 0, 1), last_fired: null }, now);
  assert.match(notYet, /^starts /);
  const sleeping = qMeta({ mode: 'every', every: 600, win_from: 1080, win_to: 1200 }, now);
  assert.match(sleeping, /^18:00–20:00 · sleeps — resumes /);
});

test('starting a list is one rule, one reviewed transaction, or a head plus chain rows', () => {
  const base = { session: 's', cardId: 'c', dir: '/w', cmd: 'claude', reviewEach: false };
  const steps = ['one', 'two', 'three'];
  const every = listStartCalls(base, { mode: 'every', every: 600 }, steps, { name: 'tpl' });
  assert.deepEqual(every, [['queue_add', { args: { ...base, mode: 'every', every: 600, text: 'one', steps: ['two', 'three'], tpl: 'tpl', tplIdx: 1, tplTotal: 3 } }]]);
  const reviewed = listStartCalls({ ...base, reviewEach: true }, { mode: 'at', at: 1 }, steps);
  assert.deepEqual(reviewed, [['queue_add_reviewed_list', { args: { ...base, reviewEach: true, mode: 'at', at: 1, text: 'one' }, texts: steps }]]);
  const plain = listStartCalls(base, { mode: 'at', at: 1 }, steps, { name: 'tpl' });
  assert.equal(plain.length, 3);
  assert.deepEqual(plain.map(([command]) => command), ['queue_add', 'queue_add', 'queue_add']);
  assert.deepEqual(plain[0][1].args, { ...base, mode: 'at', at: 1, text: 'one', tpl: 'tpl', tplIdx: 1, tplTotal: 3 });
  assert.deepEqual(plain[2][1].args, { ...base, mode: 'chain', quietSecs: null, text: 'three', tpl: 'tpl', tplIdx: 3, tplTotal: 3 });
  assert.equal('tpl' in listStartCalls(base, { mode: 'at', at: 1 }, ['x'])[0][1].args, false, 'no template tag without a template');
});
