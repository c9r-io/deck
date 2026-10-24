// DOM-contract tests for the crash-recovery dialog: production
// agent-recovery.js rendered into the shared fake document, with a scripted
// IPC.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

const calls = [];
let answers = {};
const listeners = new Map();
globalThis.document = fakeDocument;
globalThis.window = {
  __DECK_DEBUG: false,
  __TAURI__: {
    core: {
      invoke: async (command, args) => {
        calls.push([command, args]);
        const answer = answers[command];
        if (answer instanceof Error) throw answer;
        return typeof answer === 'function' ? answer(args) : answer;
      },
    },
    event: {
      listen: async (name, handler) => { listeners.set(name, handler); return () => listeners.delete(name); },
    },
  },
};

const { setLocale } = await import('../js/i18n.js');
const { ctx } = await import('../js/state.js');
const { initRecovery, offerRecovery, recoveryOpen, refreshRecovery } = await import('../js/agent-recovery.js');

const titles = new Map([['A', 'project A'], ['B', 'project B']]);
const seen = { opened: [], changed: 0 };
initRecovery({
  titleOf: id => titles.get(id) ?? null,
  openCard: id => seen.opened.push(id),
  restoreShells: () => true,
  changed: () => { seen.changed++; },
});
setLocale('en');

const box = id => fakeDocument.getElementById(id);
const walk = node => [node, ...node.children.flatMap(walk)];
const texts = node => walk(node).map(item => item.textContent).filter(Boolean);
const inputs = node => walk(node).filter(item => item.tagName === 'INPUT');
const buttons = node => walk(node).filter(item => item.tagName === 'BUTTON');
const offer = {
  token: 'T1', boardKnown: true,
  entries: [
    { cardId: 'A', session: 'deck-A', source: 'claude-code', blocked: null, recordedAt: 1_800_000_000 },
    { cardId: 'B', session: 'deck-B', source: 'claude-code', blocked: 'shell-open', recordedAt: 1_800_000_000 },
  ],
};
const commands = () => calls.map(([command]) => command).filter(command => command.startsWith('agent_'));
const reset = () => { calls.length = 0; answers = {}; box('recovery-modal').style.display = 'none'; ctx.recoveryHold = null; };

test('nothing interrupted: no dialog, nothing held', async () => {
  reset();
  answers.agent_recovery_offer = { token: null, boardKnown: true, entries: [] };
  await offerRecovery();
  assert.equal(recoveryOpen(), false);
  assert.equal(ctx.recoveryHold, null);
  answers.agent_recovery_offer = new Error('recovery-records-unreadable');
  await offerRecovery();
  assert.equal(recoveryOpen(), false, 'a failed offer shows nothing');
  assert.deepEqual(commands(), ['agent_recovery_offer', 'agent_recovery_offer']);
});

test('an offer opens its own dialog, holds the tickable shells and ticks them', async () => {
  reset();
  answers.agent_recovery_offer = offer;
  await offerRecovery();
  assert.equal(recoveryOpen(), true);
  assert.deepEqual([...ctx.recoveryHold], ['deck-A']);
  const list = box('recovery-list');
  const checks = inputs(list);
  assert.equal(checks.length, 2, '"select all" and card A; the blocked card has no control');
  assert.ok(checks.every(input => input.checked));
  assert.ok(texts(list).includes('project B'));
  assert.ok(texts(list).includes('The card’s shell is already open'));
  assert.equal(box('recovery-confirm').textContent, 'Recover 1');
  assert.equal(box('recovery-title').textContent, 'Recover interrupted Agent sessions');
  // unticking the last card disables the confirmation
  checks[1].checked = false;
  checks[1].fire('change');
  assert.equal(box('recovery-confirm').disabled, true);
  const again = inputs(list)[1];
  again.checked = true;
  again.fire('change');
  assert.equal(box('recovery-confirm').disabled, false);
  setLocale('zh-Hans');
  refreshRecovery();
  assert.equal(box('recovery-confirm').textContent, '恢复 1 个');
  setLocale('en');
  refreshRecovery();
});

test('declining deletes the offer and releases the hold; nothing starts', async () => {
  reset();
  answers.agent_recovery_offer = offer;
  await offerRecovery();
  answers.agent_recovery_decline = null;
  box('recovery-decline').onclick();
  await new Promise(resolve => setTimeout(resolve, 0));
  assert.equal(recoveryOpen(), false);
  assert.equal(ctx.recoveryHold, null);
  assert.deepEqual(calls.filter(([c]) => c === 'agent_recovery_decline'), [['agent_recovery_decline', { token: 'T1' }]]);
  assert.ok(!commands().includes('agent_recovery_run'));
});

test('confirming runs once, follows progress and shows the result with "Open card"', async () => {
  reset();
  answers.agent_recovery_offer = offer;
  await offerRecovery();
  let requestId = null;
  answers.agent_recovery_run = args => {
    requestId = args.requestId;
    listeners.get('agent-recovery-progress')?.({ payload: { requestId, phase: 'reopening', completed: 0, total: 1 } });
    assert.equal(box('recovery-progress').textContent, 'Recovering… 0/1');
    assert.equal(box('recovery-confirm').disabled, true, 'locked while it runs');
    return {
      requestId, state: 'complete', serverReplaced: false,
      entries: [{ cardId: 'A', session: 'deck-A', source: 'claude-code', outcome: 'confirmed', reason: null, warnings: [], shell: true, clientObserved: true, settled: true }],
    };
  };
  await box('recovery-confirm').onclick();
  await new Promise(resolve => setTimeout(resolve, 0));
  const run = calls.find(([c]) => c === 'agent_recovery_run');
  assert.deepEqual(run[1].approval, { token: 'T1', cards: ['A'] });
  assert.equal(run[1].restoreShells, true);
  assert.equal(ctx.recoveryHold, null, 'released');
  assert.equal(box('recovery-title').textContent, 'Interrupted sessions');
  const result = box('recovery-result');
  assert.equal(result.hidden, false);
  assert.ok(texts(result).includes('project A — Claude session re-entry confirmed. No bypass flag was added.'));
  assert.equal(box('recovery-done').style.display, '');
  assert.equal(box('recovery-confirm').style.display, 'none');
  assert.equal(seen.changed, 1);
  buttons(result).find(button => button.textContent === 'Open card').onclick();
  assert.deepEqual(seen.opened, ['A']);
  assert.equal(recoveryOpen(), false);
  // a second click on a closed dialog sends nothing
  await box('recovery-confirm').onclick();
  assert.equal(calls.filter(([c]) => c === 'agent_recovery_run').length, 1);
});

test('a lost answer is read from the operation, never asked for again', async () => {
  reset();
  answers.agent_recovery_offer = offer;
  await offerRecovery();
  answers.agent_recovery_run = new Error('recovery-worker-failed');
  answers.agent_reentry_operation = ({ requestId }) => ({
    known: true,
    result: { requestId, state: 'complete', serverReplaced: false, entries: [{ cardId: 'A', session: 'deck-A', source: 'claude-code', outcome: 'unconfirmed', reason: 'launch-unconfirmed', warnings: [], shell: true, clientObserved: false, settled: true }] },
  });
  await box('recovery-confirm').onclick();
  assert.deepEqual(commands().filter(c => c !== 'agent_recovery_offer'), ['agent_recovery_run', 'agent_reentry_operation']);
  assert.ok(texts(box('recovery-result')).includes('project A — Re-entry was attempted, but the outcome is unknown'));
  box('recovery-done').onclick();
  assert.equal(recoveryOpen(), false);
});
