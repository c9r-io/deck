// Draft title (title-draft.js): one backend call per click, the draft goes
// to the rename field and nowhere else, one draft per card at a time, a slow
// draft says it is under way, and every backend word has its sentence.
import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };
const { createTitleDrafter, titleDraftError } = await import('../js/title-draft.js');
const { t } = await import('../js/i18n.js');

function harness(answer) {
  const calls = []; const toasts = []; const timers = [];
  const inv = async (name, args) => { calls.push({ name, args }); return typeof answer === 'function' ? answer(args) : answer; };
  const draft = createTitleDrafter({
    inv, t, toast: message => toasts.push(message),
    setTimer: (fn, ms) => { const timer = { fn, ms, cleared: false }; timers.push(timer); return timer; },
    clearTimer: timer => { timer.cleared = true; },
  });
  return { draft, calls, toasts, timers };
}

const card = { id: 'c1', session: 'deck-a1', title: 'old' };

test('a draft opens the rename field with the backend title, trimmed, and renames nothing itself', async () => {
  const { draft, calls, toasts, timers } = harness({ title: '  修复登录流程 ', source: 'claude-code' });
  const opened = [];
  assert.equal(await draft(card, title => opened.push(title)), true);
  assert.deepEqual(calls, [{ name: 'card_title_draft', args: { session: 'deck-a1' } }]);
  assert.deepEqual(opened, ['修复登录流程']);
  assert.deepEqual(toasts, [], 'a quick draft says nothing');
  assert.equal(timers[0].ms, 600);
  assert.equal(timers[0].cleared, true);
});

test('one draft per card at a time, and a slow one says it is under way', async () => {
  let release;
  const { draft, calls, toasts, timers } = harness(() => new Promise(resolve => { release = resolve; }));
  const opened = [];
  const first = draft(card, title => opened.push(title));
  assert.equal(await draft(card, title => opened.push(title)), false, 'a second click while one runs');
  timers[0].fn();
  assert.deepEqual(toasts, [t('title.drafting')]);
  release({ title: 'Fix login' });
  assert.equal(await first, true);
  assert.equal(calls.length, 1);
  assert.deepEqual(opened, ['Fix login']);
  // done: the card may be drafted again
  release = null;
  const again = draft(card, () => {});
  release({ title: 'x' });
  assert.equal(await again, true);
});

test('a refusal says why in the user’s words and opens nothing', async () => {
  for (const [code, key] of [
    ['session', 'error.titleSession'], ['empty', 'error.titleEmpty'], ['no-source', 'error.titleNoSource'],
    ['refused', 'error.titleRefused'], ['model', 'error.titleModel'], ['auth', 'error.modelApiAuth'],
    ['busy', 'error.modelApiBusy'], ['network', 'error.modelApiNetwork'], ['worker', 'error.titleFailed'],
  ]) {
    const { draft, toasts, timers } = harness(() => Promise.reject(code));
    const opened = [];
    assert.equal(await draft(card, title => opened.push(title)), false, code);
    assert.deepEqual(opened, [], code);
    assert.deepEqual(toasts, [t(key)], code);
    assert.equal(timers[0].cleared, true, code);
    assert.notEqual(t(key), key, `${key} has a sentence`);
  }
  // an answer without a title is no draft
  const { draft, toasts } = harness({ title: '   ' });
  assert.equal(await draft(card, () => assert.fail('opened')), false);
  assert.deepEqual(toasts, [t('error.modelApiResponse')]);
  assert.equal(titleDraftError('anything else'), 'error.titleFailed');
});

test('no card, or a card without a session, asks nothing', async () => {
  const { draft, calls } = harness({ title: 'x' });
  assert.equal(await draft(null, () => {}), false);
  assert.equal(await draft({ id: 'c2' }, () => {}), false);
  assert.equal(calls.length, 0);
});
