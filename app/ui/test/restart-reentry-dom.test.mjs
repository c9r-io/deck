// DOM-contract tests for the re-entry part of the shell service restart
// modal: production restart-reentry.js rendered into the shared fake document.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const { setLocale } = await import('../js/i18n.js');
const { reentryRequest } = await import('../js/reentry-model.js');
const {
  currentReentryPlan, initReentry, lockReentry, reentryConfirmLabel, reentryResultShown, refreshReentry,
  showReentryPreview, showReentryProgress, showReentryResult, showRestartProgress,
} = await import('../js/restart-reentry.js');

const titles = new Map([['A', 'project A'], ['B', 'project B'], ['C', 'a card with a very long name '.repeat(8).trim()]]);
const seen = { opened: [], setup: 0, changed: 0, managed: false };
initReentry({
  titleOf: id => titles.get(id) ?? null,
  openCard: id => seen.opened.push(id),
  openSetup: () => { seen.setup++; },
  changed: () => { seen.changed++; },
  managed: () => seen.managed,
});
setLocale('en');

const box = id => fakeDocument.getElementById(id);
const walk = node => [node, ...node.children.flatMap(walk)];
const texts = node => walk(node).map(item => item.textContent).filter(Boolean);
const inputs = (node, type) => walk(node).filter(item => item.tagName === 'INPUT' && (!type || item.type === type));
const buttons = node => walk(node).filter(item => item.tagName === 'BUTTON');
const click = input => { input.checked = input.type === 'radio' ? true : !input.checked; input.fire('change'); };

const candidate = (id, launchProfile) => ({ cardId: id, session: `deck-${id}`, kind: 'candidate', reason: null, launchProfile });
const mixed = (token = 'T1') => ({
  token, lifecycleObservation: true,
  entries: [candidate('A', 'bypass'), candidate('B', 'ordinary'), candidate('C', 'ordinary'),
    { cardId: 'S', session: 'deck-S', kind: 'shell', reason: null, launchProfile: null },
    { cardId: 'X', session: 'deck-X', kind: 'unsupported', reason: 'multiple-panes', launchProfile: null }],
});

test('candidates start unchecked with policy A, nothing is focused, and policy B needs a selected bypass entry', () => {
  fakeDocument.activeElement = null;
  showReentryPreview(mixed());
  const section = box('tmux-reentry');
  assert.equal(section.hidden, false);
  const checks = inputs(section, 'checkbox');
  assert.equal(checks.length, 4, '"select all" and the three candidates; the shell and the unsupported row have no control');
  assert.ok(checks.every(input => !input.checked && !input.disabled));
  const [ordinary, bypass] = inputs(section, 'radio');
  assert.deepEqual([ordinary.checked, bypass.checked, bypass.disabled], [true, false, true]);
  assert.equal(fakeDocument.activeElement, null, 'no control takes the focus');
  assert.equal(reentryConfirmLabel(), null);
  assert.equal(reentryRequest(currentReentryPlan()), null);
  const said = texts(section);
  for (const line of ['Reopen Agent sessions', 'Claude · Launch profile: bypass', 'Claude · Launch profile: ordinary',
    'Shell session; nothing to reopen', 'Sessions with multiple windows or panes need manual re-entry.',
    'Reopen all without reapplying bypass', 'No selected session has a verified launch profile that bypass can be applied to.',
    'Re-entry without an override does not force lower permissions. Claude handles native restoration.',
    'Only the session each client was in when it exited is reopened. Tasks and temporary settings are not guaranteed to come back.',
    'Additional working directories may need to be restored manually. Queues stay paused.']) {
    assert.ok(said.includes(line), line);
  }
  assert.ok(!said.some(line => /current permission|final permission/i.test(line)));

  // an ordinary card alone: B stays unavailable
  const before = seen.changed;
  click(inputs(section, 'checkbox')[2]);
  assert.equal(seen.changed, before + 1, 'the confirm label is told');
  assert.deepEqual(reentryConfirmLabel(), { key: 'reentry.confirm', params: { count: 1 } });
  assert.equal(inputs(section, 'radio')[1].disabled, true);
  // the bypass card: B can be chosen, with the mandatory sentence and its scope
  click(inputs(section, 'checkbox')[1]);
  assert.equal(inputs(section, 'radio')[1].disabled, false);
  assert.ok(texts(section).includes('Reapply bypass to 1 session(s) launched with bypass'));
  assert.ok(texts(section).includes('If you reduced permissions during the session, this option will enable bypass again.'));
  assert.ok(texts(section).includes('Applies only to selected, verified sessions in this restart. This choice is not saved.'));
  click(inputs(section, 'radio')[1]);
  assert.deepEqual(reentryRequest(currentReentryPlan()), { token: 'T1', cards: ['A', 'B'], policy: 'reapply-bypass' });
  // select all, then none
  click(inputs(section, 'checkbox')[0]);
  assert.deepEqual(reentryRequest(currentReentryPlan()).cards, ['A', 'B', 'C']);
  seen.managed = true;
  assert.deepEqual(reentryConfirmLabel(), { key: 'reentry.confirmManaged', params: { count: 3 } });
  seen.managed = false;
  click(inputs(section, 'checkbox')[0]);
  assert.equal(reentryRequest(currentReentryPlan()), null);
  assert.equal(inputs(section, 'radio')[0].checked, true, 'policy B went with its last bypass entry');
});

test('a new preview never inherits the selection or policy B', () => {
  showReentryPreview(mixed());
  const section = box('tmux-reentry');
  click(inputs(section, 'checkbox')[0]);
  click(inputs(section, 'radio')[1]);
  assert.equal(currentReentryPlan().policy, 'reapply-bypass');
  showReentryPreview(mixed('T2'));
  assert.ok(inputs(section, 'checkbox').every(input => !input.checked));
  assert.deepEqual(inputs(section, 'radio').map(input => input.checked), [true, false]);
  assert.equal(reentryRequest(currentReentryPlan()), null);
  // no preview at all: the section is gone and the restart is shell-only
  showReentryPreview(null);
  assert.equal(section.hidden, true);
  assert.equal(reentryConfirmLabel(), null);
});

test('a running restart locks every control', () => {
  showReentryPreview(mixed());
  const section = box('tmux-reentry');
  click(inputs(section, 'checkbox')[1]);
  lockReentry(true);
  assert.ok(inputs(section).every(input => input.disabled));
  assert.deepEqual(reentryRequest(currentReentryPlan()).cards, ['A'], 'the plan read at the click is unchanged');
  lockReentry(false);
  assert.ok(inputs(section, 'checkbox').every(input => !input.disabled));
});

test('without lifecycle observation candidates are unavailable and one hint opens Settings', () => {
  showReentryPreview({ ...mixed(), lifecycleObservation: false });
  const section = box('tmux-reentry');
  assert.equal(inputs(section).length, 0, 'nothing can be selected and no policy is offered');
  const [open, ...others] = buttons(section);
  assert.equal(others.length, 0, 'one hint, not one per card');
  assert.equal(open.textContent, 'Open Settings');
  const setups = seen.setup;
  open.onclick();
  assert.equal(seen.setup, setups + 1);
  assert.equal(reentryRequest(currentReentryPlan()), null);
});

test('progress shows the three stages with the current one and its counts', () => {
  showReentryPreview(mixed());
  click(inputs(box('tmux-reentry'), 'checkbox')[0]);
  const progress = box('tmux-progress');
  showReentryProgress({ requestId: 'R', phase: 'reopening', completed: 2, total: 3, elapsedMs: 900 });
  assert.equal(progress.hidden, false);
  assert.deepEqual(texts(progress), ['Preparing sessions', 'Restarting shell service', 'Reopening Agent sessions… 2/3']);
  showReentryProgress({ requestId: 'R', phase: 'replacing', completed: 0, total: 0 });
  assert.deepEqual(texts(progress), ['Preparing sessions', 'Restarting shell service…', 'Reopening Agent sessions']);
  showReentryProgress({ phase: 'mystery' });
  assert.equal(progress.hidden, true);
  // a shell-only restart has no third stage
  showReentryPreview(mixed());
  showReentryProgress({ phase: 'saving', completed: 1, total: 4 });
  assert.deepEqual(texts(progress), ['Preparing sessions… 1/4', 'Restarting shell service']);
  showReentryProgress(null);
  assert.equal(progress.hidden, true);
});

const entry = (cardId, outcome, extra = {}) => ({
  cardId, session: `deck-${cardId}`, outcome, reason: null, warnings: [], shell: true, clientObserved: false, ...extra,
});

test('the result list replaces the review, names the server outcome first and offers only "Open card"', () => {
  showReentryPreview(mixed());
  const view = showReentryResult({
    replaced: true,
    requested: ['A', 'B', 'C'],
    result: { state: 'complete', serverReplaced: true, entries: [
      entry('A', 'confirmed', { warnings: ['bypass-reauthorized'] }), entry('B', 'confirmed'),
      entry('C', 'unconfirmed', { reason: 'launch-unconfirmed' }),
    ] },
  });
  assert.equal(reentryResultShown(), true);
  assert.equal(view.toast, 'tmux.restartComplete');
  assert.equal(box('tmux-lifecycle-title').textContent, 'Shell service restarted');
  assert.equal(box('tmux-reentry').hidden, true);
  const result = box('tmux-result');
  assert.equal(result.hidden, false);
  const said = texts(result);
  assert.ok(said.includes('project A — Claude session reopened'));
  assert.ok(said.includes('project B — Claude session reopened without a bypass override'));
  assert.ok(said.includes(`${titles.get('C')} — Re-entry was requested, but could not be confirmed`));
  assert.ok(said.includes('Queues remain paused.'));
  const actions = buttons(result);
  assert.deepEqual(actions.map(button => button.textContent), ['Open card', 'Open card', 'Open card']);
  assert.ok(!said.some(line => /retry|try again now|restart failed|did not complete/i.test(line)));
  actions[2].onclick();
  assert.deepEqual(seen.opened, ['C']);
  // a locale change repaints the same result
  setLocale('zh-Hans');
  refreshReentry();
  assert.equal(box('tmux-lifecycle-title').textContent, 'Shell 服务已重启');
  setLocale('en');
  // the next review starts clean
  showReentryPreview(mixed('T3'));
  assert.equal(reentryResultShown(), false);
  assert.equal(result.hidden, true);
  assert.equal(box('tmux-reentry').hidden, false);
});

test('a restart without a re-entry request shows its phase on the button and never the stage list', () => {
  showReentryPreview(mixed());   // candidates are listed, none selected: a shell-only restart
  const button = box('tmux-restart'), progress = box('tmux-progress');
  button.textContent = 'Restarting…';
  for (const [event, expected] of [
    [{ requestId: 'R', phase: 'exiting', completed: 1, total: 3 }, 'Exiting agents… 1/3'],
    [{ requestId: 'R', phase: 'saving', completed: 2, total: 3 }, 'Saving output… 2/3'],
    [{ requestId: 'R', phase: 'replacing', completed: 0, total: 0 }, 'Restarting service…'],
  ]) {
    showRestartProgress(event, false);
    assert.equal(button.textContent, expected);
    assert.equal(progress.hidden, true, 'no stage list');
    assert.deepEqual(texts(progress), []);
  }
  // a phase without shell-only text leaves the button as it is
  showRestartProgress({ phase: 'reopening', completed: 1, total: 1 }, false);
  showRestartProgress({ phase: 'mystery' }, false);
  assert.equal(button.textContent, 'Restarting service…');
  assert.equal(progress.hidden, true);
  setLocale('zh-Hans');
  showRestartProgress({ phase: 'exiting', completed: 1, total: 3 }, false);
  assert.equal(button.textContent, '正在退出 agent… 1/3');
  setLocale('en');

  // the same events of a restart that requested re-entry: the stage list, and the button is not this module's
  button.textContent = 'Restarting…';
  click(inputs(box('tmux-reentry'), 'checkbox')[0]);
  showRestartProgress({ requestId: 'R', phase: 'exiting', completed: 1, total: 3 }, true);
  assert.equal(progress.hidden, false);
  assert.deepEqual(texts(progress), ['Preparing sessions… 1/3', 'Restarting shell service', 'Reopening Agent sessions']);
  assert.equal(button.textContent, 'Restarting…');
  // and a stage list that was showing goes away for a shell-only event
  showRestartProgress({ phase: 'saving', completed: 1, total: 2 }, false);
  assert.equal(progress.hidden, true);
  showReentryPreview(null);
});

test('the restart progress is routed by the request, after the 300ms no-flash rule', () => {
  const app = readFileSync(new URL('../js/app.js', import.meta.url), 'utf8');
  const listener = app.slice(app.indexOf("listen('tmux-restart-progress'"), app.indexOf('await prepareCardsForServerRestart'));
  assert.match(listener, /if \(!ctx\.tmuxRestarting \|\| progress\.requestId !== requestId \|\| performance\.now\(\) - began < 300\) return;\s*(?:\/\/[^\n]*\n\s*)*showRestartProgress\(progress, !!reentry\);/);
  assert.doesNotMatch(listener, /showReentryProgress\(progress/, 'the stage list is never shown unconditionally');
  assert.equal((app.match(/showRestartProgress\(/g) || []).length, 1);
  assert.equal((app.match(/showReentryProgress\((?!null\))/g) || []).length, 0, 'app.js itself only hides the stage list');
});

test('the startup configuration note is in the confirmation once, under either policy, with its help behind one disclosure', () => {
  const note = 'Re-entry uses the new shell\'s startup configuration; if it has changed, you may need to check manually.';
  const help = 'Some differences can only be found after the start. When one is found, Deck does not retry and does not undo the start that already happened.';
  const count = (section, line) => texts(section).filter(text => text === line).length;
  showReentryPreview(mixed());
  const section = box('tmux-reentry');
  // nothing selected yet: candidates can be selected, so the note already stands
  assert.deepEqual([count(section, note), count(section, help)], [1, 1]);
  click(inputs(section, 'checkbox')[0]);
  assert.equal(inputs(section, 'radio')[0].checked, true, 'policy A');
  assert.deepEqual([count(section, note), count(section, help)], [1, 1], 'once for three selected cards, not per card');
  click(inputs(section, 'radio')[1]);
  assert.equal(inputs(section, 'radio')[1].checked, true, 'policy B');
  assert.deepEqual([count(section, note), count(section, help)], [1, 1]);
  assert.equal(count(section, 'If you reduced permissions during the session, this option will enable bypass again.'), 1, 'the mandatory sentence stays');
  // the help is inside the one disclosure, which starts closed and is no control
  const disclosures = walk(section).filter(item => item.tagName === 'DETAILS');
  assert.equal(disclosures.length, 1);
  assert.ok(!disclosures[0].open, 'closed until asked for');
  assert.deepEqual(disclosures[0].children.map(item => item.tagName), ['SUMMARY', 'DIV', 'DIV']);
  assert.equal(disclosures[0].children[2].textContent, help);
  assert.ok(!walk(disclosures[0]).some(item => item.textContent === note), 'the note itself is not hidden behind it');
  assert.equal(inputs(section).length, 6, 'no extra box or toggle: select all, three cards, two policies');
  assert.equal(buttons(section).length, 0, 'and no button');
  setLocale('zh-Hans');
  refreshReentry();
  assert.deepEqual([count(section, '重新进入会使用新 shell 的启动配置；若配置已变化，可能需要手动检查。'),
    count(section, '部分差异只有在启动后才能发现。发现差异时 Deck 不会自动重试，也不会撤销已经发生的启动。'),
    count(section, '如果你在会话运行过程中降低过权限，这个选项会再次启用 bypass。')], [1, 1, 1]);
  setLocale('en');
  refreshReentry();
  // nothing selectable: neither the note nor the disclosure
  showReentryPreview({ ...mixed('T8'), lifecycleObservation: false });
  assert.deepEqual([count(section, note), count(section, help)], [0, 0]);
  assert.equal(walk(section).filter(item => item.tagName === 'DETAILS').length, 0);
  // the result list does not repeat it
  showReentryPreview(mixed());
  click(inputs(section, 'checkbox')[0]);
  showReentryResult({ replaced: true, requested: ['A'], result: { state: 'complete', serverReplaced: true, entries: [entry('A', 'confirmed')] } });
  assert.deepEqual([count(box('tmux-result'), note), count(box('tmux-result'), help)], [0, 0]);
  showReentryPreview(null);
});

test('a start with a different or unverified startup configuration is listed as unconfirmed with its reason and only "Open card"', () => {
  showReentryPreview(mixed());
  showReentryResult({
    replaced: true,
    requested: ['A', 'B', 'C'],
    result: { state: 'complete', serverReplaced: true, entries: [
      entry('A', 'confirmed'),
      entry('B', 'unconfirmed', { reason: 'context-changed', clientObserved: true }),
      entry('C', 'unconfirmed', { reason: 'context-unverified', clientObserved: true }),
    ] },
  });
  const result = box('tmux-result');
  const rows = walk(result).filter(item => /reentry-result-row/.test(item.className));
  assert.deepEqual(rows.map(row => row.className), ['reentry-result-row ok', 'reentry-result-row warn', 'reentry-result-row warn']);
  assert.ok(texts(rows[1]).includes('project B — Claude started, but re-entry could not be confirmed'));
  assert.ok(texts(rows[1]).includes('Claude started, but with a different startup configuration than expected; open the card to check.'));
  assert.ok(texts(rows[2]).includes('Claude started, but its startup configuration could not be verified; open the card to check.'));
  assert.ok(!texts(result).includes('Open this card to check the current client before trying again.'), 'no advice about trying again');
  assert.deepEqual(buttons(result).map(button => button.textContent), ['Open card', 'Open card', 'Open card']);
  assert.ok(!texts(result).some(line => /retry|try again|undo/i.test(line)));
  assert.equal(box('tmux-lifecycle-title').textContent, 'Shell service restarted');
  setLocale('zh-Hans');
  refreshReentry();
  assert.ok(texts(result).includes('Claude 已启动，但启动配置与预期不同；请打开卡片检查。'));
  assert.ok(texts(result).includes('Claude 已启动，但无法核实其启动配置；请打开卡片检查。'));
  setLocale('en');
  showReentryPreview(null);
});

test('rows the backend ruled out before the restart show their reason and no policy is offered', () => {
  const ruledOut = id => ({ cardId: id, session: `deck-${id}`, kind: 'unsupported', reason: 'identity-unavailable', launchProfile: 'bypass' });
  showReentryPreview({ token: 'T7', lifecycleObservation: true, entries: [ruledOut('A'), ruledOut('B'),
    { cardId: 'S', session: 'deck-S', kind: 'shell', reason: null, launchProfile: null }] });
  const section = box('tmux-reentry');
  assert.equal(section.hidden, false);
  const said = texts(section);
  assert.ok(said.includes('No session here can be reopened automatically.'));
  assert.ok(said.includes('project A') && said.includes('project B'));
  assert.equal(said.filter(line => line === 'The session’s re-entry context could not be verified.').length, 2, 'each row says why');
  assert.equal(inputs(section).length, 0, 'no checkbox, no "select all", no policy radio');
  assert.equal(buttons(section).length, 0, 'and no setup hint: observation is on');
  assert.ok(!said.some(line => /Permission policy|Reapply bypass|Reopen all|Launch profile/.test(line)), 'nothing reads as if a session could be reopened');
  assert.equal(reentryConfirmLabel(), null, 'the confirm button keeps its shell-only label');
  seen.managed = true;
  assert.equal(reentryConfirmLabel(), null);
  seen.managed = false;
  assert.equal(reentryRequest(currentReentryPlan()), null);
  setLocale('zh-Hans');
  refreshReentry();
  assert.ok(texts(section).includes('无法确认此 session 的重入条件。'));
  setLocale('en');
  showReentryPreview(null);
});

test('a running result shows unsettled entries as in progress and follows them to their final wording', () => {
  showReentryPreview(mixed());
  const result = box('tmux-result');
  const shown = { replaced: true, requested: ['A', 'B', 'C'] };
  const live = showReentryResult({ ...shown, result: { state: 'running', serverReplaced: true, entries: [
    entry('A', 'confirmed', { settled: true }), entry('B', 'not-attempted', { settled: false }), entry('C', 'unconfirmed'),
  ] } });
  assert.equal(live.pending, true);
  assert.equal(box('tmux-lifecycle-title').textContent, 'Shell service restarted');
  let said = texts(result);
  assert.ok(said.includes('project A — Claude session reopened without a bypass override'));
  assert.ok(said.includes('project B — Reopening…'));
  assert.ok(said.includes(`${titles.get('C')} — Reopening…`), 'a missing flag in a running operation is not settled');
  assert.ok(said.includes('1 reopened · 2 in progress · 0 to check'));
  assert.ok(said.includes('Deck is still reopening some sessions. This list updates as their results arrive.'));
  assert.ok(!said.some(line => /not known|could not be confirmed|before trying again|not attempted/i.test(line)), 'nothing reads as a final result');
  assert.ok(said.includes('…'), 'the in-progress mark');

  // the bounded poll ended and the worker still runs
  showReentryResult({ ...shown, pollEnded: true, result: { state: 'running', serverReplaced: true, entries: [
    entry('A', 'confirmed', { settled: true }), entry('B', 'unconfirmed', { settled: true }), entry('C', 'unconfirmed', { settled: false }),
  ] } });
  said = texts(result);
  assert.ok(said.includes('project B — Re-entry was requested, but could not be confirmed'));
  assert.ok(said.includes('Open this card to check the current client before trying again.'));
  assert.ok(said.includes(`${titles.get('C')} — The result is still pending`));
  assert.ok(said.includes('Open this card later to see how it ended.'));
  assert.ok(said.includes('Some results were still pending when Deck stopped waiting. Open those cards later to see them.'));
  assert.equal(said.filter(line => line === 'Open this card to check the current client before trying again.').length, 1, 'only the settled entry gets the final advice');

  // the operation completed: final wording everywhere
  const done = showReentryResult({ ...shown, result: { state: 'complete', serverReplaced: true, entries: [
    entry('A', 'confirmed'), entry('B', 'unconfirmed'), entry('C', 'confirmed', { settled: false }),
  ] } });
  assert.equal(done.pending, false);
  said = texts(result);
  assert.ok(!said.some(line => /Reopening…|still pending|in progress/.test(line)));
  assert.ok(said.includes('2 reopened · 1 to check'));
  showReentryPreview(null);
});

test('the modal box is marked only while re-entry content is in it', () => {
  const marks = [];
  box('tmux-lifecycle-box').classList = { toggle: (name, on) => marks.push([name, on]) };
  showReentryPreview(null);
  assert.deepEqual(marks.at(-1), ['reentry', false], 'a plain restart review keeps its size');
  showReentryPreview(mixed());
  assert.deepEqual(marks.at(-1), ['reentry', true]);
  showReentryResult({ replaced: true, requested: ['A'], result: { state: 'complete', serverReplaced: true, entries: [entry('A', 'confirmed')] } });
  assert.deepEqual(marks.at(-1), ['reentry', true]);
  showReentryPreview(null);
  assert.deepEqual(marks.at(-1), ['reentry', false]);
});

test('nothing shaped like a native id, a pid, a digest or a path is rendered, whatever the backend adds', () => {
  const uuid = '3f2b8c1e-9a4d-4c7b-8e21-0d5f6a7b8c9d';
  const digest = '9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08';
  const sentinel = {
    nativeSessionId: uuid, sessionId: uuid, pid: 48211, foregroundPid: 48211, birth: 1760000048211,
    binaryHash: digest, executable: '/Users/someone/.local/share/claude/versions/2.1.294',
    transcriptPath: `/Users/someone/.claude/projects/x/${uuid}.jsonl`, hostLease: digest, cwd: '/Users/someone/work',
  };
  const native = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{32,}|48211|\/Users\/|\.jsonl/i;
  const hostile = [uuid, digest, sentinel.executable, `deck-${uuid}`, 'pid 48211'];
  let rendered = [];
  const capture = () => { rendered = rendered.concat(...['tmux-reentry', 'tmux-progress', 'tmux-result', 'tmux-lifecycle-title'].map(id => texts(box(id)))); };

  for (const [round, value] of hostile.entries()) {
    // cards the Board no longer has are named by their session only when it is displayable
    const preview = {
      token: `tok-${round}`, lifecycleObservation: round % 2 === 0, ...sentinel,
      entries: [
        { ...candidate('A', 'bypass'), ...sentinel },
        { ...candidate('gone-1', value), session: value, ...sentinel },
        { cardId: 'gone-2', session: `deck-${uuid}`, kind: value, reason: value, launchProfile: value, ...sentinel },
        { cardId: 'gone-3', session: 'deck-gone-3', kind: 'unsupported', reason: value, launchProfile: null, ...sentinel },
      ],
    };
    showReentryPreview(preview);
    for (const input of inputs(box('tmux-reentry'), 'checkbox').slice(0, 1)) click(input);
    capture();
    showReentryProgress({ phase: 'reopening', completed: 1, total: 2, requestId: uuid, ...sentinel });
    capture();
    showReentryProgress({ phase: value, completed: value, total: value });
    capture();
    showReentryResult({
      replaced: round % 2 === 1,
      requested: ['A', 'gone-1', 'gone-9'],
      result: { requestId: uuid, state: round % 2 ? 'complete' : 'running', serverReplaced: true, ...sentinel, entries: [
        { ...entry('A', 'confirmed', { warnings: [value, 'bypass-reauthorized'] }), ...sentinel },
        { ...entry('gone-1', 'unconfirmed', { reason: value, clientObserved: true }), session: value, ...sentinel },
        { ...entry('gone-2', value, { reason: value, warnings: [value] }), session: `deck-${uuid}`, ...sentinel },
        { ...entry('gone-3', 'not-attempted', { reason: 'final-session-unconfirmed' }), ...sentinel },
      ] },
    });
    capture();
  }
  assert.ok(rendered.length > 100, 'the fuzz rendered every surface');
  assert.ok(rendered.includes('Unknown card — Claude started, but re-entry could not be confirmed'), 'a card without a displayable name is still listed');
  assert.ok(rendered.some(line => line.startsWith('deck-gone-3')), 'an ordinary session name is an acceptable last resort');
  for (const line of rendered) assert.doesNotMatch(line, native);
  showReentryPreview(null);
});
