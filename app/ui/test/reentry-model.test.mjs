import test from 'node:test';
import assert from 'node:assert/strict';
import {
  POLICY_BYPASS, POLICY_ORDINARY, afterManagedClose, awaitReentryResult, displayable, planCounts, planView,
  progressView, projectResult, reentryPlan, reentryRequest, reentryResult, responseLost, restartOutcome,
  shellProgressLabel, withAllSelected, withPolicy, withSelection,
} from '../js/reentry-model.js';
import { dictionaries, setLocale, t } from '../js/i18n.js';

const candidate = (id, launchProfile, observed = true) => ({ cardId: id, session: `deck-${id}`, kind: 'candidate', reason: null, launchProfile, source: 'claude-code', observed });
const mixed = (token = 'T1') => ({
  token, lifecycleObservation: true,
  entries: [candidate('A', 'bypass'), candidate('B', 'ordinary'), candidate('C', 'ordinary')],
});
const say = line => t(line.key, line.params);
const selectAll = plan => withAllSelected(plan, true);

test('a new plan selects nothing, uses the ordinary policy and asks for a shell-only restart', () => {
  const plan = reentryPlan(mixed());
  assert.deepEqual(plan.selected, []);
  assert.equal(plan.policy, POLICY_ORDINARY);
  assert.equal(reentryRequest(plan), null);
  assert.equal(planView(plan).confirm, null, 'the confirm button keeps its shell-only label');
  // a policy alone authorizes nothing
  assert.equal(reentryRequest(withPolicy(plan, POLICY_BYPASS)), null);
  assert.equal(withPolicy(plan, POLICY_BYPASS).policy, POLICY_ORDINARY);
});

test('opening the review again never remembers policy B or the selection', () => {
  const used = withPolicy(selectAll(reentryPlan(mixed())), POLICY_BYPASS);
  assert.equal(used.policy, POLICY_BYPASS);
  assert.deepEqual(reentryRequest(used), { token: 'T1', cards: ['A', 'B', 'C'], policy: POLICY_BYPASS });
  const again = reentryPlan(mixed('T2'));
  assert.equal(again.policy, POLICY_ORDINARY);
  assert.deepEqual(again.selected, []);
  assert.equal(reentryRequest(again), null);
  // the module holds no state of its own between plans
  assert.deepEqual(reentryPlan(mixed('T2')), again);
});

test('policy B counts only the selected entries launched with bypass', () => {
  const all = selectAll(reentryPlan(mixed()));
  assert.deepEqual(planCounts(all), { candidates: 3, candidateBypass: 1, selected: 3, bypass: 1, unknown: 0 });
  const view = planView(all);
  assert.equal(view.bypass.enabled, true);
  assert.deepEqual(view.bypass.label, { key: 'reentry.policy.bypass', params: { count: 1 } });
  assert.deepEqual(view.confirm, { key: 'reentry.confirm', params: { count: 3 } });
  assert.deepEqual(planView(all, { managed: true }).confirm, { key: 'reentry.confirmManaged', params: { count: 3 } });
  setLocale('en');
  assert.deepEqual(view.bypass.notes.map(say), [
    'If you lowered permissions while the session ran, reapplying bypass now will enable bypass again.',
    'Applies only to selected, verified sessions in this restart. This choice is not saved.',
  ]);
  setLocale('zh-Hans');
  assert.equal(say(view.bypass.notes[0]), '如果运行期间降低过权限，此次重新施加 bypass 会再次启用 bypass。');
  setLocale('en');
  // the count follows the selection, not the list
  const withoutA = withSelection(all, 'A', false);
  assert.equal(planCounts(withoutA).bypass, 0);
  assert.equal(planView(withoutA).bypass.enabled, false);
});

test('policy B is unavailable without a selected bypass entry, and unknown is counted on its own', () => {
  const ordinaryOnly = withSelection(withSelection(reentryPlan(mixed()), 'B', true), 'C', true);
  assert.equal(withPolicy(ordinaryOnly, POLICY_BYPASS).policy, POLICY_ORDINARY);
  const view = planView(ordinaryOnly);
  assert.equal(view.bypass.enabled, false);
  assert.deepEqual(view.bypass.notes, [{ key: 'reentry.policy.bypassUnavailable' }]);
  assert.equal(view.bypass.unknown, null);

  const unknownOnly = selectAll(reentryPlan({
    token: 'T', lifecycleObservation: true, entries: [candidate('U', 'unknown'), candidate('V', 'something-new')],
  }));
  assert.deepEqual(planCounts(unknownOnly), { candidates: 2, candidateBypass: 0, selected: 2, bypass: 0, unknown: 2 });
  assert.equal(withPolicy(unknownOnly, POLICY_BYPASS).policy, POLICY_ORDINARY);
  assert.equal(planView(unknownOnly).bypass.enabled, false);
  assert.deepEqual(planView(unknownOnly).bypass.unknown, { key: 'reentry.policy.unknownCount', params: { count: 2 } });
  assert.deepEqual(reentryRequest(unknownOnly), { token: 'T', cards: ['U', 'V'], policy: POLICY_ORDINARY });
});

test('deselecting the last bypass entry drops policy B instead of keeping a choice that can never apply', () => {
  const chosen = withPolicy(selectAll(reentryPlan(mixed())), POLICY_BYPASS);
  const dropped = withSelection(chosen, 'A', false);
  assert.equal(dropped.policy, POLICY_ORDINARY);
  assert.equal(withSelection(dropped, 'A', true).policy, POLICY_ORDINARY, 'coming back is a new choice');
});

test('the profile comes from the preview alone: nothing saved on a card can enable policy B', () => {
  const preview = { token: 'T', lifecycleObservation: true, entries: [
    { ...candidate('A', 'ordinary'), cmd: 'claude --dangerously-skip-permissions', launched: true, profile: 'bypass' },
  ] };
  const plan = selectAll(reentryPlan(preview));
  assert.equal(planCounts(plan).bypass, 0);
  assert.equal(withPolicy(plan, POLICY_BYPASS).policy, POLICY_ORDINARY);
  assert.deepEqual(Object.keys(plan.entries[0]).sort(), ['cardId', 'kind', 'observed', 'profile', 'reason', 'session', 'source']);
});

test('the request holds the selected candidate ids only', () => {
  const preview = { token: 'T', lifecycleObservation: true, entries: [
    candidate('A', 'bypass'),
    { cardId: 'S', session: 'deck-S', kind: 'shell', reason: null, launchProfile: null },
    { cardId: 'X', session: 'deck-X', kind: 'unsupported', reason: 'multiple-panes', launchProfile: null },
    candidate('B', 'ordinary'),
  ] };
  let plan = reentryPlan(preview);
  for (const id of ['S', 'X', 'nope', 'B']) plan = withSelection(plan, id, true);
  assert.deepEqual(reentryRequest(plan), { token: 'T', cards: ['B'], policy: POLICY_ORDINARY });
  assert.deepEqual(reentryRequest(selectAll(plan)).cards, ['A', 'B']);
  const rows = planView(plan).rows;
  assert.deepEqual(rows.map(row => [row.cardId, row.selectable, row.note.key]), [
    ['A', true, 'reentry.profile.bypass'],
    ['S', false, 'reentry.kind.shell'],
    ['X', false, 'reentry.reason.multiple-panes'],
    ['B', true, 'reentry.profile.ordinary'],
  ]);
});

test('a preview without lifecycle observation lists candidates as unavailable with one setup hint', () => {
  const plan = selectAll(reentryPlan({ ...mixed(), lifecycleObservation: false }));
  const view = planView(plan);
  assert.equal(view.setup, true);
  assert.equal(view.canSelect, false);
  assert.ok(view.rows.every(row => !row.selectable && !row.checked));
  assert.equal(reentryRequest(plan), null);
  assert.equal(view.summary.key, 'reentry.summary.unavailable');
});

test('an unusable preview is the plain shell-only restart', () => {
  for (const bad of [null, undefined, 'x', {}, { token: '', entries: [] }, { token: 'T' }, { token: 'T', entries: 'no' }]) {
    const plan = reentryPlan(bad);
    assert.equal(planView(plan).visible, false);
    assert.equal(reentryRequest(selectAll(plan)), null);
  }
  // one card listed twice names no single client
  const twice = selectAll(reentryPlan({ token: 'T', lifecycleObservation: true, entries: [candidate('A', 'bypass'), candidate('A', 'bypass')] }));
  assert.equal(reentryRequest(twice), null);
  assert.ok(planView(twice).rows.every(row => row.note.key === 'reentry.reason.identity-unavailable'));
});

test('a new preview with a different candidate set carries no approval over', () => {
  const approved = withPolicy(selectAll(reentryPlan(mixed())), POLICY_BYPASS);
  const managedGone = { cardId: 'M', session: 'deck-M', kind: 'unsupported', reason: 'unsupported-agent', launchProfile: null };
  const reviewed = withPolicy(selectAll(reentryPlan({ ...mixed(), entries: [...mixed().entries, managedGone] })), POLICY_BYPASS);

  // exactly the reviewed list without the closed card: one confirmation covers it, on the FRESH token
  assert.deepEqual(afterManagedClose(reviewed, mixed('FRESH'), ['M']),
    { request: { token: 'FRESH', cards: ['A', 'B', 'C'], policy: POLICY_BYPASS } });
  // anything else is a changed review
  const cases = {
    'another candidate appeared': { ...mixed('F'), entries: [...mixed().entries, candidate('D', 'bypass')] },
    'a candidate is gone': { ...mixed('F'), entries: mixed().entries.slice(1) },
    'ordinary became bypass': { ...mixed('F'), entries: [candidate('A', 'bypass'), candidate('B', 'bypass'), candidate('C', 'ordinary')] },
    'bypass became unknown': { ...mixed('F'), entries: [candidate('A', 'unknown'), candidate('B', 'ordinary'), candidate('C', 'ordinary')] },
    'the session behind a card changed': { ...mixed('F'), entries: [{ ...candidate('A', 'bypass'), session: 'deck-A-2' }, ...mixed().entries.slice(1)] },
    'observation went away': { ...mixed('F'), lifecycleObservation: false },
    'the closed card is still listed': { ...mixed('F'), entries: [...mixed().entries, managedGone] },
    'no preview': null,
  };
  for (const [name, fresh] of Object.entries(cases)) {
    assert.deepEqual(afterManagedClose(reviewed, fresh, ['M']), { changed: true }, name);
  }
  // nothing selected needs no preview at all
  assert.deepEqual(afterManagedClose(reentryPlan(mixed()), null, ['M']), { request: null });
  // and a plan built from the different preview starts empty, whatever was approved before
  assert.equal(reentryRequest(reentryPlan(cases['another candidate appeared'])), null);
  assert.equal(approved.policy, POLICY_BYPASS, 'the earlier plan is untouched');
});

test('backend phases map to the three user stages', () => {
  const stage = phase => progressView({ phase, completed: 1, total: 3 });
  assert.equal(stage('exiting').stage, 'preparing');
  assert.equal(stage('saving').stage, 'preparing');
  assert.equal(stage('replacing').stage, 'restarting');
  assert.equal(stage('reopening').stage, 'reopening');
  assert.equal(stage('something-else'), null);
  assert.equal(progressView(null), null);
  assert.equal(stage('exiting').counts, '1/3');
  assert.equal(stage('replacing').counts, '', 'the replacement has no count');
  assert.equal(progressView({ phase: 'reopening', completed: 5, total: 3 }).counts, '');
  assert.equal(progressView({ phase: 'reopening', completed: '1', total: 3 }).counts, '');
  setLocale('en');
  assert.deepEqual(stage('reopening').stages.map(item => [say(item.label), item.current]), [
    ['Preparing sessions', false], ['Restarting shell service', false], ['Reopening Agent sessions', true],
  ]);
  assert.deepEqual(progressView({ phase: 'replacing' }, { reopening: false }).stages.map(item => item.stage), ['preparing', 'restarting']);
});

const entry = (cardId, outcome, extra = {}) => ({
  cardId, session: `deck-${cardId}`, source: 'claude-code', outcome, reason: null, warnings: [], shell: true, clientObserved: false, ...extra,
});
const complete = (entries, serverReplaced = true) => ({ requestId: 'R1', state: 'complete', serverReplaced, entries });

test('two confirmed and one unconfirmed after a replacement is "restarted", never a failed restart', () => {
  const answer = {
    server: { serverPid: 2, serverStartedAt: 9 }, replaced: true,
    reentry: complete([
      entry('A', 'confirmed', { warnings: ['bypass-reauthorized'] }),
      entry('B', 'confirmed'),
      entry('C', 'unconfirmed', { reason: 'launch-unconfirmed' }),
    ]),
  };
  const outcome = restartOutcome(answer);
  assert.equal(outcome.replaced, true);
  const view = projectResult({ replaced: outcome.replaced, result: outcome.result, requested: ['A', 'B', 'C'] });
  setLocale('en');
  assert.equal(say(view.title), 'Shell service restarted');
  assert.equal(view.toast, 'tmux.restartComplete');
  assert.notEqual(view.toast, 'tmux.restartFailed');
  assert.deepEqual(view.rows.map(row => [row.cardId, row.tone, say(row.line)]), [
    ['A', 'ok', 'Claude session re-entry confirmed. The bypass flag was sent as authorized.'],
    ['B', 'ok', 'Claude session re-entry confirmed. No bypass flag was added.'],
    ['C', 'warn', 'Re-entry was attempted, but the outcome is unknown'],
  ]);
  // the one thing added for B: what Deck did NOT establish
  assert.deepEqual(view.rows[0].details.map(say), ['Deck did not independently verify Claude’s current effective permission.']);
  assert.deepEqual(view.rows[1].details, []);
  assert.deepEqual(view.rows[2].details.map(say), ['Open this card to check the current client before trying again.']);
  assert.deepEqual(view.summary, { key: 'reentry.result.summary', params: { confirmed: 2, other: 1 } });
  assert.ok(view.footer.map(say).includes('Queues remain paused.'));
  // what is said about a session never claims more than a confirmed re-entry
  // and a flag that was sent: nothing is resumed, ready, restored or verified
  const said = [say(view.title), ...view.rows.flatMap(row => [say(row.line), ...row.details.map(say)])].join(' ');
  assert.doesNotMatch(said, /did not complete|resumed|ready|restored|continued|permission (?:was |is )?verified|bypass is (?:on|active)/i);
  setLocale('zh-Hans');
  assert.deepEqual(view.rows.map(row => say(row.line)), [
    '已确认重新进入 Claude 会话；已按本次授权发送 bypass 参数。',
    '已确认重新进入 Claude 会话；未追加 bypass 参数。',
    '已尝试发送重入请求，结果未确认',
  ]);
  assert.deepEqual(view.rows[0].details.map(say), ['Deck 未独立验证 Claude 当前有效权限。']);
  const zhSaid = view.rows.flatMap(row => [say(row.line), ...row.details.map(say)]).join(' ');
  assert.doesNotMatch(zhSaid, /已恢复|权限已验证|已重新施加/);
  setLocale('en');
});

test('a replacement that did not happen is titled so, with the exited agents said separately', () => {
  const outcome = restartOutcome({ server: { serverPid: 1 }, replaced: false, reentry: complete([entry('A', 'not-attempted', { reason: 'exit-timeout', shell: false })], false) });
  assert.equal(outcome.replaced, false);
  const view = projectResult({ replaced: false, result: outcome.result, requested: ['A'] });
  setLocale('en');
  assert.equal(say(view.title), 'Shell service was not restarted');
  assert.equal(view.toast, null);
  assert.deepEqual(view.notes.map(say), ['Some Agents may already have exited. Open their cards to check.']);
  assert.equal(say(view.rows[0].line), 'Re-entry was not attempted');
  assert.ok(!view.footer.map(say).includes('Queues remain paused.'));
  // the operation's own server conclusion counts when the call's flag is missing
  assert.equal(restartOutcome({ server: {}, reentry: complete([]) }).replaced, true);
  assert.equal(restartOutcome({ server: {}, replaced: 'yes', reentry: null }).replaced, false);
  assert.equal(restartOutcome(null).server, null);
});

test('an unverified launch profile and the bypass that was not reapplied are one line', () => {
  const both = projectResult({ replaced: true, result: reentryResult(complete([
    entry('A', 'confirmed', { warnings: ['launch-profile-unconfirmed', 'bypass-not-reapplied', 'launch-profile-unconfirmed', 'additional-dirs-not-restored', 'made-up'] }),
    entry('B', 'confirmed', { warnings: ['bypass-not-reapplied'] }),
  ])) });
  setLocale('en');
  assert.deepEqual(both.rows[0].details.map(say), [
    'The launch profile could not be verified. Bypass was not reapplied.',
    'Additional working directories may need to be restored.',
  ]);
  assert.deepEqual(both.rows[1].details.map(say), ['Bypass was not reapplied.']);
});

test('"Claude started" is said only for an unconfirmed entry whose client was observed', () => {
  const view = projectResult({ replaced: true, result: reentryResult(complete([
    entry('A', 'unconfirmed', { clientObserved: true }),
    entry('B', 'unconfirmed', { clientObserved: false }),
    entry('C', 'refused-before-launch', { clientObserved: true, reason: 'launch-refused' }),
    entry('D', 'known-no-launch-failure', { reason: 'launch-refused' }),
    entry('E', 'not-attempted', { reason: 'final-session-unconfirmed', clientObserved: true }),
    entry('F', 'something-new', { clientObserved: true }),
  ])) });
  setLocale('en');
  const lines = view.rows.map(row => say(row.line));
  assert.deepEqual(lines, [
    'Claude started, but re-entry could not be confirmed',
    'Re-entry was attempted, but the outcome is unknown',
    'Claude was not started',
    'Claude was not started',
    'Re-entry was not attempted',
    'The re-entry result is not known',
  ]);
  assert.equal(lines.filter(line => /Claude started/.test(line)).length, 1);
  assert.deepEqual(view.rows[4].details.map(say), ['The session Claude exited in could not be confirmed. Claude was not automatically started.']);
});

test('every closed reason and warning has its sentence in both languages; an unknown code shows no raw text', () => {
  const reasons = ['unsupported-agent', 'client-identity-unverifiable', 'multiple-panes', 'identity-unavailable', 'executable-unavailable', 'check-incomplete',
    'final-session-unconfirmed', 'exit-timeout', 'impact-changed', 'launch-refused', 'launch-unconfirmed', 'context-changed', 'context-unverified',
    'native-session-not-found', 'user-action-required', 'bypass-not-reapplied'];
  for (const reason of reasons) {
    const [row] = projectResult({ replaced: true, result: reentryResult(complete([entry('A', 'not-attempted', { reason })])) }).rows;
    assert.equal(row.details[0].key, `reentry.reason.${reason}`);
    for (const dictionary of Object.values(dictionaries)) assert.ok(dictionary[row.details[0].key], reason);
  }
  const odd = reentryResult(complete([entry('A', 'not-attempted', { reason: 'toString', warnings: ['constructor', 42, null] })]));
  assert.equal(odd.entries[0].reason, null);
  assert.deepEqual(odd.entries[0].warnings, []);
  assert.equal(reentryResult({ entries: 'no' }), null);
  assert.equal(reentryResult(null), null);
});

test('a start with a changed or unverified startup configuration stays unconfirmed, says why once and offers no retry', () => {
  const result = reentryResult(complete([
    entry('A', 'unconfirmed', { reason: 'context-changed', clientObserved: true }),
    entry('B', 'unconfirmed', { reason: 'context-unverified', clientObserved: true, warnings: ['bypass-not-reapplied'] }),
    // the reason is given only for a client that was found started
    entry('C', 'unconfirmed', { reason: 'context-changed' }),
    entry('D', 'unconfirmed', { reason: 'launch-unconfirmed' }),
  ]));
  assert.deepEqual(result.entries.map(row => row.reason), ['context-changed', 'context-unverified', 'context-changed', 'launch-unconfirmed']);
  const view = projectResult({ replaced: true, result });
  assert.deepEqual(view.rows.map(row => row.tone), ['warn', 'warn', 'warn', 'warn'], 'the unconfirmed presentation');
  assert.deepEqual(view.summary, { key: 'reentry.result.summary', params: { confirmed: 0, other: 4 } });
  assert.deepEqual(view.title, { key: 'reentry.result.restarted' }, 'never a failed restart');
  assert.deepEqual(view.rows.map(row => row.line.key), ['reentry.result.startedUnconfirmed', 'reentry.result.startedUnconfirmed',
    'reentry.result.startedUnconfirmed', 'reentry.result.unconfirmed']);
  assert.deepEqual(view.rows.map(row => row.details.map(line => line.key)), [
    ['reentry.reason.context-changed'],
    ['reentry.reason.context-unverified', 'reentry.reason.bypass-not-reapplied'],
    ['reentry.reason.context-changed'],
    ['reentry.result.check'],
  ]);
  setLocale('en');
  assert.deepEqual(view.rows.slice(0, 2).map(row => say(row.details[0])), [
    'Claude started, but with a different startup configuration than expected; open the card to check.',
    'Claude started, but its startup configuration could not be verified; open the card to check.',
  ]);
  setLocale('zh-Hans');
  assert.deepEqual(view.rows.slice(0, 2).map(row => say(row.details[0])), [
    'Claude 已启动，但启动配置与预期不同；请打开卡片检查。',
    'Claude 已启动，但无法核实其启动配置；请打开卡片检查。',
  ]);
  for (const locale of ['en', 'zh-Hans']) {
    setLocale(locale);
    const said = view.rows.slice(0, 3).flatMap(row => [say(row.line), ...row.details.map(say)]).join('\n');
    assert.doesNotMatch(said, /retry|try(ing)? again|undo|重试|再次尝试|撤销/i, locale);
  }
  setLocale('en');
  // a reason of an entry that is still running is not a result yet
  const running = projectResult({ replaced: true, result: reentryResult({ state: 'running', serverReplaced: true,
    entries: [entry('A', 'unconfirmed', { reason: 'context-changed', clientObserved: true })] }) });
  assert.deepEqual([running.rows[0].tone, running.rows[0].line.key, running.rows[0].details], ['pending', 'reentry.result.reopening', []]);
});

test('the startup configuration note stands once whenever something can be selected, under either policy', () => {
  const noteKeys = view => view.notes.map(line => line.key);
  const fresh = reentryPlan(mixed());
  const chosen = withPolicy(selectAll(fresh), POLICY_BYPASS);
  assert.equal(planView(chosen).policy, POLICY_BYPASS);
  for (const plan of [fresh, selectAll(fresh), chosen, withSelection(fresh, 'B', true)]) {
    const view = planView(plan);
    assert.deepEqual(noteKeys(view), ['reentry.note.ordinary', 'reentry.note.scope', 'reentry.note.context', 'reentry.note.dirs']);
    assert.deepEqual(view.help.map(line => line.key), ['reentry.note.otherClients', 'reentry.help.context']);
    assert.ok([...view.notes, ...view.help].every(line => !line.params), 'standing copy: no count, no card');
  }
  // nothing selectable: no observation, no candidate, no preview
  const off = reentryPlan({ ...mixed(), lifecycleObservation: false });
  const none = reentryPlan({ token: 'T2', lifecycleObservation: true, entries: [{ cardId: 'S', session: 'deck-S', kind: 'shell' }] });
  for (const plan of [off, none, reentryPlan(null)]) {
    assert.deepEqual([planView(plan).notes, planView(plan).help], [[], []]);
  }
  assert.equal(dictionaries.en['reentry.note.context'], 'Re-entry uses the new shell\'s startup configuration; if it has changed, you may need to check manually.');
  assert.equal(dictionaries['zh-Hans']['reentry.note.context'], '重新进入会使用新 shell 的启动配置；若配置已变化，可能需要手动检查。');
  assert.equal(dictionaries.en['reentry.help.context'], 'Some differences can only be found after the start. When one is found, Deck does not retry and does not undo the start that already happened.');
  assert.equal(dictionaries['zh-Hans']['reentry.help.context'], '部分差异只有在启动后才能发现。发现差异时 Deck 不会自动重试，也不会撤销已经发生的启动。');
  // no proof vocabulary in what the user reads
  for (const dictionary of Object.values(dictionaries)) {
    for (const key of ['reentry.note.context', 'reentry.help.context', 'reentry.reason.context-changed', 'reentry.reason.context-unverified']) {
      assert.doesNotMatch(dictionary[key], /digest|hash|fence|proof|receipt|baseline|generation|argv|env\b|摘要|哈希|收据|基线|证明/i, key);
    }
  }
});

test('a lost response is looked up read-only and the restart is never invoked again', async () => {
  assert.equal(responseLost('tmux-restart-timeout', true), true);
  assert.equal(responseLost('some transport error', true), true);
  assert.equal(responseLost('tmux-restart-timeout', false), false, 'a shell-only restart has no operation to read');
  for (const refusal of ['tmux-server-impact-changed', 'reentry-preview-expired', 'tmux-restart-agent-timeout', 'tmux-restart-busy', 'managed-close-ambiguous']) {
    assert.equal(responseLost(refusal, true), false, refusal);
  }

  const calls = [];
  const waits = [];
  const answers = [
    { known: true, result: { requestId: 'R1', state: 'running', serverReplaced: true, entries: [entry('A', 'not-attempted')] } },
    { known: true, result: { requestId: 'R1', state: 'running', serverReplaced: true, entries: [entry('A', 'not-attempted')] } },
    { known: true, result: complete([entry('A', 'confirmed')]) },
  ];
  const invoke = async (command, args) => { calls.push([command, args]); return answers.shift(); };
  const known = await awaitReentryResult({ requestId: 'R1', invoke, wait: async ms => { waits.push(ms); } });
  assert.equal(known.known, true);
  assert.equal(known.result.state, 'complete');
  assert.deepEqual(calls, Array(3).fill(['agent_reentry_operation', { requestId: 'R1' }]));
  assert.ok(calls.every(([command]) => command !== 'restart_tmux_server'));
  assert.deepEqual(waits, [1000, 1000]);

  // a worker that never finishes is read a bounded number of times, then shown as it stands
  let reads = 0;
  const stuck = await awaitReentryResult({
    requestId: 'R2', wait: async () => {},
    invoke: async command => { assert.equal(command, 'agent_reentry_operation'); reads++; return answers.length ? null : { known: true, result: { state: 'running', serverReplaced: true, entries: [entry('A', 'not-attempted')] } }; },
  });
  assert.equal(reads, 21);
  assert.equal(stuck.result.state, 'running');
  // the poll is over and the worker still runs: pending, to look at later; not a result
  const pending = projectResult({ replaced: true, result: stuck.result, requested: ['A', 'B'], pollEnded: true });
  setLocale('en');
  assert.equal(say(pending.title), 'Shell service restarted');
  assert.deepEqual(pending.rows.map(row => say(row.line)), ['The result is still pending', 'The result is still pending']);
  assert.deepEqual(pending.notes.map(say), ['Some results were still pending when Deck stopped waiting. Open those cards later to see them.']);

  // an operation the backend no longer knows, or a query that fails, is unknown: still no second restart
  for (const answer of [{ known: false, result: null }, null]) {
    const seen = [];
    const unknown = await awaitReentryResult({ requestId: 'R3', wait: async () => {}, invoke: async command => { seen.push(command); if (!answer) throw new Error('ipc'); return answer; } });
    assert.deepEqual(unknown, { known: false, result: null });
    assert.deepEqual(seen, ['agent_reentry_operation']);
  }
  const lost = projectResult({ replaced: true, result: null, requested: ['A'] });
  assert.equal(say(lost.title), 'Shell service restarted');
  assert.equal(say(lost.rows[0].line), 'The re-entry result is not known');
});

const running = (entries, serverReplaced = true) => ({ requestId: 'R1', state: 'running', serverReplaced, entries });
const wording = view => [say(view.title), say(view.summary), ...view.notes.map(say),
  ...view.rows.flatMap(row => [say(row.line), ...row.details.map(say)])].join('\n');
const FINAL_WORDING = /not known|could not be confirmed|outcome is unknown|未知|无法确认|结果未确认|before trying again|再次尝试|not attempted|未尝试|not started|未启动/i;

test('an unsettled entry of a running operation is in progress, never a result', () => {
  // the backend's own "not yet" shapes: explicit false, a missing flag, and an outcome it already wrote down
  const raw = running([
    entry('A', 'not-attempted', { settled: false }),
    entry('B', null),
    entry('C', 'unconfirmed', { settled: false, reason: 'launch-unconfirmed', clientObserved: true }),
    entry('D', 'confirmed', { settled: true }),
    entry('E', 'unconfirmed', { settled: true, reason: 'launch-unconfirmed' }),
    entry('F', 'confirmed', { settled: 'yes' }),
  ]);
  const result = reentryResult(raw);
  assert.deepEqual(result.entries.map(item => item.settled), [false, false, false, true, true, false], 'only the boolean true settles an entry of a running operation');
  for (const locale of ['en', 'zh-Hans']) {
    setLocale(locale);
    const view = projectResult({ replaced: true, result, requested: ['A', 'B', 'C', 'D', 'E', 'F', 'G'] });
    assert.equal(view.pending, true);
    assert.deepEqual(view.rows.map(row => [row.cardId, row.tone]), [
      ['A', 'pending'], ['B', 'pending'], ['C', 'pending'], ['D', 'ok'], ['E', 'warn'], ['F', 'pending'], ['G', 'pending'],
    ], 'a requested card the running operation has not listed yet is in progress too');
    const waiting = view.rows.filter(row => row.tone === 'pending');
    for (const row of waiting) {
      assert.deepEqual(row.line, { key: 'reentry.result.reopening' });
      assert.deepEqual(row.details, [], 'no advice that belongs to a final result');
      assert.doesNotMatch(say(row.line), FINAL_WORDING);
    }
    assert.deepEqual(view.summary, { key: 'reentry.result.summaryPending', params: { confirmed: 1, pending: 5, other: 1 } });
    assert.deepEqual(view.notes, [{ key: 'reentry.result.pending' }]);
    // the one settled unconfirmed entry keeps its final wording and advice
    assert.equal(view.rows[4].line.key, 'reentry.result.unconfirmed');
    assert.deepEqual(view.rows[4].details, [{ key: 'reentry.result.check' }]);
  }
  setLocale('en');
  assert.equal(say({ key: 'reentry.result.reopening' }), 'Reopening…');
  assert.equal(dictionaries['zh-Hans']['reentry.result.reopening'], '正在重新进入…');
  // all in progress: nothing in the whole list reads as an outcome
  const all = projectResult({ replaced: true, result: reentryResult(running([entry('A', 'not-attempted'), entry('B', 'unconfirmed', { settled: false })])), requested: ['A', 'B'] });
  for (const locale of ['en', 'zh-Hans']) { setLocale(locale); assert.doesNotMatch(wording(all), FINAL_WORDING, locale); }
  setLocale('en');
});

test('a complete operation settles every entry, with or without the flag', () => {
  const result = reentryResult(complete([entry('A', 'unconfirmed'), entry('B', 'not-attempted', { settled: false }), entry('C', 'confirmed', { settled: true }), entry('D', null)]));
  assert.deepEqual(result.entries.map(item => item.settled), [true, true, true, true]);
  const view = projectResult({ replaced: true, result, requested: ['A', 'B', 'C', 'D', 'E'], pollEnded: true });
  setLocale('en');
  assert.equal(view.pending, false);
  assert.deepEqual(view.rows.map(row => say(row.line)), [
    'Re-entry was attempted, but the outcome is unknown', 'Re-entry was not attempted', 'Claude session re-entry confirmed. No bypass flag was added.',
    'The re-entry result is not known', 'The re-entry result is not known',
  ]);
  assert.ok(view.rows.every(row => row.tone !== 'pending'));
  assert.deepEqual(view.summary, { key: 'reentry.result.summary', params: { confirmed: 1, other: 4 } });
  assert.deepEqual(view.notes, []);
});

test('a poll that ended while the operation still ran says "still pending", distinct from a settled unconfirmed', () => {
  const result = reentryResult(running([entry('A', 'not-attempted'), entry('B', 'unconfirmed', { settled: true, reason: 'launch-unconfirmed' })]));
  const live = projectResult({ replaced: true, result, requested: ['A', 'B'] });
  const ended = projectResult({ replaced: true, result, requested: ['A', 'B'], pollEnded: true });
  assert.deepEqual(live.rows[0].line, { key: 'reentry.result.reopening' });
  assert.deepEqual(ended.rows[0].line, { key: 'reentry.result.stillPending' });
  assert.deepEqual(ended.rows[0].details, [{ key: 'reentry.result.checkLater' }]);
  assert.deepEqual(ended.notes, [{ key: 'reentry.result.pendingEnded' }]);
  assert.deepEqual(live.notes, [{ key: 'reentry.result.pending' }]);
  // the settled unconfirmed row is the same in both, and shares no sentence with the pending one
  assert.deepEqual(ended.rows[1], live.rows[1]);
  assert.equal(ended.rows[1].line.key, 'reentry.result.unconfirmed');
  for (const locale of ['en', 'zh-Hans']) {
    setLocale(locale);
    const pendingSaid = [say(ended.rows[0].line), ...ended.rows[0].details.map(say), ...ended.notes.map(say)];
    const settledSaid = [say(ended.rows[1].line), ...ended.rows[1].details.map(say)];
    assert.ok(pendingSaid.every(line => !settledSaid.includes(line)), locale);
    assert.doesNotMatch(pendingSaid.join('\n'), FINAL_WORDING, locale);
    assert.match(pendingSaid.join('\n'), locale === 'en' ? /pending/ : /仍未确定/);
    assert.match(pendingSaid.join('\n'), locale === 'en' ? /later/ : /稍后/, 'it can be looked at later');
  }
  setLocale('en');
});

test('the read-only poll hands every answer on, keeps reading while entries settle and stops when nobody looks', async () => {
  const answers = [
    { known: true, result: running([entry('A', 'not-attempted'), entry('B', 'not-attempted')]) },
    { known: true, result: running([entry('A', 'confirmed', { settled: true }), entry('B', 'not-attempted', { settled: false })]) },
    { known: true, result: complete([entry('A', 'confirmed'), entry('B', 'unconfirmed')]) },
  ];
  const commands = [], seen = [];
  const known = await awaitReentryResult({
    requestId: 'R7', wait: async () => {},
    invoke: async (command, args) => { commands.push([command, args]); return answers.shift(); },
    onRead: result => seen.push(projectResult({ replaced: true, result, requested: ['A', 'B'] }).rows.map(row => row.tone)),
  });
  assert.deepEqual(seen, [['pending', 'pending'], ['ok', 'pending'], ['ok', 'warn']]);
  assert.equal(known.result.state, 'complete');
  assert.deepEqual(commands, Array(3).fill(['agent_reentry_operation', { requestId: 'R7' }]), 'reads only');

  // a list the user closed is not read for again, and gets no late answer
  let reads = 0, looking = true;
  const late = [];
  const left = await awaitReentryResult({
    requestId: 'R8', wait: async () => { looking = false; }, active: () => looking, onRead: result => late.push(result),
    invoke: async () => { reads++; return { known: true, result: running([entry('A', 'not-attempted')]) }; },
  });
  assert.equal(reads, 1);
  assert.equal(late.length, 1);
  assert.equal(left.result.state, 'running');
  looking = false;
  await awaitReentryResult({ requestId: 'R9', wait: async () => {}, active: () => looking, onRead: result => late.push(result),
    invoke: async () => ({ known: true, result: complete([entry('A', 'confirmed')]) }) });
  assert.equal(late.length, 1, 'an answer that arrives after the list was left is not handed on');
});

test('a restart without a re-entry request keeps its per-phase button text, key for key', () => {
  assert.deepEqual(shellProgressLabel({ requestId: 'R', phase: 'exiting', completed: 1, total: 3, elapsedMs: 900 }),
    { key: 'tmux.progress.exiting', params: { completed: 1, total: 3 } });
  assert.equal(shellProgressLabel({ phase: 'saving', completed: 2, total: 4 }).key, 'tmux.progress.saving');
  assert.equal(shellProgressLabel({ phase: 'replacing', completed: 0, total: 0 }).key, 'tmux.progress.replacing');
  // the re-entry phase, an unknown phase and inherited names have no shell-only text
  for (const phase of ['reopening', 'mystery', 'toString', 'constructor', '', null, undefined]) assert.equal(shellProgressLabel({ phase }), null, String(phase));
  assert.equal(shellProgressLabel(null), null);
  // the three strings are the ones the restart had before re-entry existed
  assert.deepEqual(['exiting', 'saving', 'replacing'].map(phase => dictionaries.en[`tmux.progress.${phase}`]),
    ['Exiting agents… {completed}/{total}', 'Saving output… {completed}/{total}', 'Restarting service…']);
  assert.deepEqual(['exiting', 'saving', 'replacing'].map(phase => dictionaries['zh-Hans'][`tmux.progress.${phase}`]),
    ['正在退出 agent… {completed}/{total}', '正在保存输出… {completed}/{total}', '正在重启服务…']);
  setLocale('en');
  assert.equal(say(shellProgressLabel({ phase: 'exiting', completed: 1, total: 3 })), 'Exiting agents… 1/3');
  assert.equal(say(shellProgressLabel({ phase: 'replacing' })), 'Restarting service…');
});

test('a preview whose rows are all unsupported or shells offers nothing to reopen', () => {
  const known = { cardId: 'U1', session: 'deck-U1', kind: 'unsupported', reason: 'identity-unavailable', launchProfile: null };
  const preview = { token: 'T9', lifecycleObservation: true, entries: [
    known,
    // a row the backend ruled out keeps that ruling whatever else it carries
    { cardId: 'U2', session: 'deck-U2', kind: 'unsupported', reason: 'identity-unavailable', launchProfile: 'bypass' },
    { cardId: 'U3', session: 'deck-U3', kind: 'unsupported', reason: 'executable-unavailable', launchProfile: null },
    { cardId: 'S', session: 'deck-S', kind: 'shell', reason: null, launchProfile: null },
  ] };
  let plan = reentryPlan(preview);
  // nothing the user does can select a row or reach policy B
  plan = withPolicy(withAllSelected(withSelection(plan, 'U2', true), true), POLICY_BYPASS);
  assert.deepEqual(plan.selected, []);
  assert.equal(plan.policy, POLICY_ORDINARY);
  assert.equal(reentryRequest(plan), null, 'the restart stays shell-only');
  assert.deepEqual(planCounts(plan), { candidates: 0, candidateBypass: 0, selected: 0, bypass: 0, unknown: 0 });
  const view = planView(plan);
  assert.equal(view.visible, true, 'the rows and their reasons are listed');
  assert.deepEqual(view.summary, { key: 'reentry.summary.none' });
  assert.equal(view.canSelect, false);
  assert.equal(view.setup, false);
  assert.equal(view.bypass.enabled, false);
  assert.equal(view.confirm, null, 'the confirm button keeps its shell-only label');
  assert.equal(planView(plan, { managed: true }).confirm, null);
  assert.deepEqual(view.rows.map(row => [row.cardId, row.selectable, row.checked, row.note.key]), [
    ['U1', false, false, 'reentry.reason.identity-unavailable'],
    ['U2', false, false, 'reentry.reason.identity-unavailable'],
    ['U3', false, false, 'reentry.reason.executable-unavailable'],
    ['S', false, false, 'reentry.kind.shell'],
  ]);
  setLocale('en');
  assert.equal(say(view.rows[0].note), 'The session’s re-entry context could not be verified.');
  setLocale('zh-Hans');
  assert.equal(say(view.rows[0].note), '无法确认此 session 的重入条件。');
  setLocale('en');
  // one such row beside a real candidate takes nothing away from the candidate
  const mixedPlan = withAllSelected(reentryPlan({ ...preview, entries: [known, candidate('A', 'bypass')] }), true);
  assert.deepEqual(reentryRequest(mixedPlan), { token: 'T9', cards: ['A'], policy: POLICY_ORDINARY });
  assert.equal(planView(mixedPlan).rows[0].note.key, 'reentry.reason.identity-unavailable');
});

test('a release no list names shows the same plan as a known one, and nothing about a release reaches the page', () => {
  // what a backend could say about two releases of the product: the same
  // facts. Whatever else rides along — a digest, a version, a "certified"
  // flag — is not a field this contract names, so it changes nothing.
  const facts = token => ({
    token, lifecycleObservation: true,
    entries: [candidate('A', 'bypass'), candidate('B', 'ordinary'), { cardId: 'C', session: 'deck-C', kind: 'unsupported', reason: 'check-incomplete', launchProfile: null }],
  });
  const known = facts('T-known');
  const fresh = facts('T-new');
  fresh.version = '9.9.999';
  fresh.certified = false;
  fresh.entries = fresh.entries.map(entry => ({
    ...entry, version: '9.9.999', certified: false, digest: 'c9b5341637becbd423ddffc5b254afb645682a3868cb708bbc6cc0e7bb419937',
    reason: entry.reason, note: 'not certified for this version',
  }));
  const views = [known, fresh].map(preview => {
    const plan = withPolicy(selectAll(reentryPlan(preview)), POLICY_BYPASS);
    return { counts: planCounts(plan), view: planView(plan), request: { ...reentryRequest(plan), token: null } };
  });
  assert.deepEqual(views[0], views[1], 'the same availability, the same A/B counts, the same request');
  assert.deepEqual(views[1].counts, { candidates: 2, candidateBypass: 1, selected: 2, bypass: 1, unknown: 0 });
  assert.equal(views[1].view.policy, POLICY_BYPASS, 'B needs a selected bypass launch and nothing about a release');
  for (const locale of ['en', 'zh-Hans']) {
    setLocale(locale);
    const { view } = views[1];
    const shown = [say(view.summary), say(view.bypass.label), ...view.bypass.notes.map(say), ...view.notes.map(say), ...view.help.map(say),
      ...view.rows.map(row => say(row.note)), say(view.confirm)].join('\n');
    assert.doesNotMatch(shown, /9\.9\.999|c9b53416|certif|认证|digest|摘要|sha|版本|version/i, locale);
    // the one sentence a user must have read before choosing B, once
    assert.equal(view.bypass.notes.filter(note => note.key === 'reentry.policy.bypassWarning').length, 1);
  }
  setLocale('en');
});

test('a check that did not finish is said as that, never as a changed program, and a result names no flag it cannot vouch for', () => {
  // the preview row
  const plan = reentryPlan({ token: 'T', lifecycleObservation: true, entries: [
    { cardId: 'U', session: 'deck-U', kind: 'unsupported', reason: 'check-incomplete', launchProfile: null },
    { cardId: 'X', session: 'deck-X', kind: 'unsupported', reason: 'executable-unavailable', launchProfile: null, source: 'claude-code' },
  ] });
  const rows = planView(plan).rows;
  assert.deepEqual(rows.map(row => row.note.key), ['reentry.reason.check-incomplete', 'reentry.reason.executable-unavailable']);
  setLocale('zh-Hans');
  assert.equal(say(rows[0].note), '未能完成重入条件检查，未自动重新进入。');
  assert.equal(say(rows[1].note), '原 Claude 程序已改变或不可用，未自动重新进入。');
  assert.doesNotMatch(say(rows[0].note), /改变|不可用/, 'an unfinished check does not say the program changed');
  setLocale('en');
  assert.equal(say(rows[0].note), 'The re-entry checks could not be completed. The session was not reopened automatically.');
  assert.doesNotMatch(say(rows[0].note), /changed|unavailable/i);
  // the result row of an entry that was not attempted for that reason
  const [late] = projectResult({ replaced: true, result: reentryResult(complete([entry('A', 'not-attempted', { reason: 'check-incomplete', shell: false })])) }).rows;
  assert.deepEqual([say(late.line), ...late.details.map(say)], ['Re-entry was not attempted', 'The re-entry checks could not be completed. The session was not reopened automatically.']);
  // An entry that is not confirmed never says a flag was sent, whatever
  // marks ride on it: only a confirmed entry is given the bypass sentence.
  const marked = projectResult({ replaced: true, result: reentryResult(complete([
    entry('B', 'unconfirmed', { reason: 'launch-unconfirmed', warnings: ['bypass-reauthorized'] }),
    entry('C', 'unconfirmed', { reason: 'context-changed', clientObserved: true, warnings: ['bypass-reauthorized'] }),
    entry('D', 'known-no-launch-failure', { reason: 'launch-refused', warnings: ['bypass-reauthorized'] }),
  ])) });
  for (const locale of ['en', 'zh-Hans']) {
    setLocale(locale);
    for (const row of marked.rows) {
      const said = [say(row.line), ...row.details.map(say)].join(' ');
      assert.doesNotMatch(said, /flag was sent|发送 bypass 参数|sent as authorized|已按本次授权|bypass|permission|权限/i, `${locale} ${row.cardId}`);
    }
  }
  setLocale('en');
  assert.equal(say(marked.rows[0].line), 'Re-entry was attempted, but the outcome is unknown');
});

test('the display guard refuses native-looking strings', () => {
  for (const bad of ['3f2b8c1e-9a4d-4c7b-8e21-0d5f6a7b8c9d', 'deck-3F2B8C1E-9A4D-4C7B-8E21-0D5F6A7B8C9D', 'a'.repeat(32) + '0'.repeat(32),
    'd41d8cd98f00b204e9800998ecf8427e', '/Users/someone/.claude/projects', 'C:\\x', '', null, 7, 'deck-' + 'x'.repeat(41), 'pid 48211', 'claude']) {
    assert.equal(displayable(bad), null, String(bad));
  }
  assert.equal(displayable('deck-Cmf1x2a-0001'), 'deck-Cmf1x2a-0001');
});

/* A Pi card in the review: named, told its own reason, never selectable. It
   takes no part in what is chosen for the Claude cards beside it, under
   either policy, and it is never part of the request. */
test('a Pi card is named with its own reason, can never be chosen, and is never requested', () => {
  const preview = {
    token: 'tok', lifecycleObservation: true,
    entries: [
      { cardId: 'A', session: 'deck-a', kind: 'candidate', launchProfile: 'bypass', source: 'claude-code', observed: true },
      { cardId: 'P', session: 'deck-p', kind: 'unsupported', reason: 'client-identity-unverifiable', source: 'pi' },
      { cardId: 'Q', session: 'deck-q', kind: 'unsupported', reason: 'unsupported-agent' },
    ],
  };
  const plan = reentryPlan(preview);
  const rows = which => planView(which).rows;
  const pi = rows(plan).find(row => row.cardId === 'P');
  assert.equal(pi.source, 'pi');
  assert.equal(pi.selectable, false);
  assert.deepEqual(pi.note, { key: 'reentry.reason.client-identity-unverifiable', params: { agent: 'Pi' } });
  assert.equal(rows(plan).find(row => row.cardId === 'A').source, 'claude-code');
  assert.equal(rows(plan).find(row => row.cardId === 'Q').source, null);
  // choosing everything, and reapplying bypass, choose the Claude card and nothing of Pi
  for (const chosen of [selectAll(plan), withPolicy(selectAll(plan), POLICY_BYPASS)]) {
    assert.deepEqual(reentryRequest(chosen).cards, ['A']);
    const row = rows(chosen).find(each => each.cardId === 'P');
    assert.deepEqual([row.selectable, row.checked], [false, false]);
  }
  // only Pi and an unknown program: nothing can be chosen at all
  const none = reentryPlan({ ...preview, entries: preview.entries.slice(1) });
  assert.equal(planView(none).canSelect, false);
  assert.equal(reentryRequest(selectAll(none)), null);
  // the reason is Pi's own in both languages, and the label is a closed word
  for (const dictionary of Object.values(dictionaries)) {
    assert.match(dictionary['reentry.reason.client-identity-unverifiable'], /^Pi · /);
  }
  assert.match(dictionaries.en['reentry.reason.client-identity-unverifiable'], /cannot verify whose code this Pi runs.*not reopened automatically/);
  assert.equal(rows(reentryPlan({ ...preview, entries: [{ ...preview.entries[1], source: 'node' }] }))[0].source, null);
  // a session Deck observes for that Pi is no part of this payload and changes nothing here
  const observed = reentryPlan({ ...preview, entries: [{ ...preview.entries[1], piSession: 'observed', nativeSessionId: 'x' }] });
  assert.deepEqual(rows(observed)[0], { ...pi });
});

test('what Deck could not find out is said as unknown, never as nothing to reopen', () => {
  for (const preview of [
    { unknown: true },
    { token: 'tok-unknown', lifecycleObservation: true, boardKnown: false, entries: [] },
  ]) {
    const plan = reentryPlan(preview);
    assert.equal(plan.unknown, true);
    assert.equal(reentryRequest(withAllSelected(plan, true)), null, 'nothing can be selected');
    const view = planView(plan);
    assert.equal(view.visible, true, 'the section is shown');
    assert.deepEqual(view.summary, { key: 'reentry.summary.unknown' });
    assert.equal(view.canSelect, false);
    assert.equal(view.setup, false);
    assert.equal(view.confirm, null, 'the confirm button keeps its shell-only label');
  }
  // a known Board with no candidate is still "none"; a missing field is a known Board
  const none = planView(reentryPlan({ token: 'tok-none', lifecycleObservation: true, boardKnown: true, entries: [] }));
  assert.equal(none.visible, false);
  assert.equal(reentryPlan({ token: 'tok-old', lifecycleObservation: true, entries: [] }).unknown, false);
  assert.equal(reentryPlan(null).unknown, false, 'no preview at all stays the plain restart');
  for (const locale of ['en', 'zh-Hans']) {
    setLocale(locale);
    assert.notEqual(t('reentry.summary.unknown'), 'reentry.summary.unknown');
  }
  setLocale('en');
});

test('a Codex row is labelled Codex, says its note once, and is chosen only while its own observation is on', () => {
  const codex = (id, launchProfile, observed = true) => ({ ...candidate(id, launchProfile, observed), source: 'codex' });
  const preview = { token: 'T', lifecycleObservation: true, entries: [candidate('A', 'bypass'), codex('X', 'bypass'), codex('Y', 'ordinary')] };
  const plan = selectAll(reentryPlan(preview));
  const view = planView(plan);
  assert.deepEqual(view.rows.map(row => [row.cardId, row.source, row.selectable, say(row.note)]), [
    ['A', 'claude-code', true, 'Claude · Launch profile: bypass'],
    ['X', 'codex', true, 'Codex · Launch profile: bypass'],
    ['Y', 'codex', true, 'Codex · Launch profile: ordinary'],
  ]);
  // the Codex note stands once among the scope notes, and only with a Codex row to choose
  assert.equal(view.notes.filter(note => note.key === 'reentry.note.codex').length, 1);
  assert.equal(planView(selectAll(reentryPlan({ ...preview, entries: [candidate('A', 'bypass')] }))).notes.some(note => note.key === 'reentry.note.codex'), false);
  assert.deepEqual(planCounts(plan), { candidates: 3, candidateBypass: 2, selected: 3, bypass: 2, unknown: 0 });
  assert.deepEqual(reentryRequest(plan).cards, ['A', 'X', 'Y']);
  // a Codex candidate whose own observation is off cannot be chosen; the one setup hint names its agent
  const off = selectAll(reentryPlan({ ...preview, entries: [candidate('A', 'bypass'), codex('X', 'bypass', false)] }));
  const offView = planView(off);
  assert.deepEqual(offView.rows.map(row => [row.cardId, row.selectable, row.checked]), [['A', true, true], ['X', false, false]]);
  assert.equal(offView.setup, true);
  assert.equal(offView.setupSource, 'codex');
  assert.deepEqual(reentryRequest(off).cards, ['A']);
  assert.deepEqual(offView.summary, { key: 'reentry.summary', params: { count: 1, bypass: 1 } });
  // the backend's own word for that row, worded for its agent
  const ruled = planView(reentryPlan({ ...preview, entries: [
    { cardId: 'X', session: 'deck-X', kind: 'unsupported', reason: 'observation-off', launchProfile: null, source: 'codex' },
  ] }));
  assert.equal(say(ruled.rows[0].note), 'Codex session re-entry observation is not installed for this Agent. Turn it on in Settings.');
  assert.equal(ruled.setup, false, 'a ruled-out row is not a candidate: the setting is named in its own line');
});

test('a started Codex entry is counted as started, never as re-entered, and says what Deck did not observe', () => {
  const started = (id, extra = {}) => entry(id, 'started', { source: 'codex', clientObserved: true, ...extra });
  const view = projectResult({ replaced: true, requested: ['A', 'X', 'Y', 'Z'], result: reentryResult(complete([
    entry('A', 'confirmed'),
    started('X', { warnings: ['ordinary-profile-requested'] }),
    started('Y', { warnings: ['bypass-reauthorized'] }),
    entry('Z', 'unconfirmed', { source: 'codex', reason: 'launch-unconfirmed', clientObserved: true }),
  ])) });
  setLocale('en');
  assert.deepEqual(view.rows.map(row => [row.cardId, row.tone, row.started === true, say(row.line)]), [
    ['A', 'ok', false, 'Claude session re-entry confirmed. No bypass flag was added.'],
    ['X', 'ok', true, 'Codex started with the session it exited in. It reports entering a session only at its first turn.'],
    ['Y', 'ok', true, 'Codex started with the session it exited in. It reports entering a session only at its first turn.'],
    ['Z', 'warn', false, 'Codex started, but re-entry could not be confirmed'],
  ]);
  assert.deepEqual(view.rows[1].details.map(say), [
    'Deck did not observe the session being entered. Open the card to continue.',
    'The ordinary profile was requested: Codex keeps bypass on a bare resume, so the sandbox and approval words that undo it were sent. Codex writes back what it applied.',
  ]);
  assert.deepEqual(view.rows[2].details.map(say), [
    'Deck did not observe the session being entered. Open the card to continue.',
    'Deck did not independently verify Codex’s current effective permission.',
  ]);
  assert.deepEqual(view.summary, { key: 'reentry.result.summaryStarted', params: { confirmed: 1, started: 2, other: 1 } });
  // nothing says a started session was entered, resumed or restored
  const said = view.rows.slice(1, 3).flatMap(row => [say(row.line), ...row.details.map(say)]).join(' ');
  assert.doesNotMatch(said, /re-entered|re-entry confirmed|resumed|restored|entered the session/i);
  setLocale('zh-Hans');
  assert.equal(say(view.rows[1].line), 'Codex 已以退出时的会话启动；它只在第一个回合时报告进入了哪个会话。');
  assert.equal(say(view.summary), '1 个已重新进入 · 2 个已启动 · 1 个需查看');
  setLocale('en');
  // with an entry still in progress the started count keeps its own place
  const pending = projectResult({ replaced: true, requested: ['X', 'P'], result: reentryResult({ requestId: 'R', state: 'running', serverReplaced: true, entries: [started('X', { settled: true }), entry('P', 'unconfirmed', { settled: false })] }) });
  assert.deepEqual(pending.summary, { key: 'reentry.result.summaryPendingStarted', params: { confirmed: 0, started: 1, pending: 1, other: 0 } });
});
