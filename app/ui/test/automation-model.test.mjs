// The automations drawer's DOM-free half: what a rule and its runs read as,
// how the editor's fields become a rule and how a saved rule joins the list.
// The drawer itself (automation.js) is verified by the WKWebView smoke.
import test from 'node:test';
import assert from 'node:assert/strict';
import { approvalText, approveRule, composeRule, firstSendNeedsConfirm, firstSendSupported, firstSendText, withFirstSend, graceOptions, graceText, grantDigest, grantManifest, grantState, liveRules, mergeRules, parseMonthDays, recentRuns, ruleFacts, ruleLabel, runSummary, scheduleFromFields, schedulePreview, scheduleText, stepClass, templateCarriesMessage, triggerText } from '../js/automation-model.js';
import { readFileSync } from 'node:fs';
import { GRACE_CHOICES } from '../js/settings-model.js';
import { setLocale } from '../js/i18n.js';

setLocale('en');
const now = new Date(2026, 2, 15, 12, 0, 0);
const nowSecs = Math.floor(now.getTime() / 1000);
const clockRule = (over = {}) => ({
  id: 'a1', source: 'clock', badge: 'a1', name: 'Nightly', projectId: 'P', columnId: 'C', template: 'tpl', cmd: 'claude', dir: '/w',
  enabled: true, finish: 'close', graceMin: 15, since: 0, schedule: { unit: 'week', days: [1, 3], minute: 540 }, ...over,
});
const slackRule = (over = {}) => ({
  id: 'r1', source: 'slack', badge: 'deck', name: '', projectId: 'P', columnId: 'C', template: 'tpl', cmd: '', dir: '', enabled: true, finish: 'keep', ...over,
});
let ids = 0;
const genId = prefix => `${prefix}${++ids}`;

test('grace, schedule and trigger read in the user\'s language', () => {
  assert.equal(graceText(0), 'never — skip it');
  assert.equal(graceText(15), 'still start within 15 min');
  assert.equal(graceText(120), 'still start within 2 h');
  assert.equal(graceText(1440), 'still start the same day');
  assert.deepEqual(graceOptions(GRACE_CHOICES[0]), [...GRACE_CHOICES]);
  assert.deepEqual(graceOptions(45), [...GRACE_CHOICES, 45].sort((a, b) => a - b), 'a saved value outside the choices stays offered');
  assert.equal(scheduleText({ unit: 'week', days: [1, 3], minute: 540 }), 'every Mon, Wed at 09:00');
  assert.equal(scheduleText({ unit: 'month', days: [1, 15], minute: 1350 }), 'monthly on day 1, 15 at 22:30');
  assert.equal(scheduleText({ unit: 'day', days: [], minute: 0 }), 'every day at 00:00');
  assert.equal(triggerText(clockRule()), 'every Mon, Wed at 09:00');
  assert.equal(triggerText(slackRule()), 'on :deck:');
  assert.equal(ruleLabel(clockRule()), 'Nightly');
  assert.equal(ruleLabel(clockRule({ name: '' })), 'a1');
  assert.equal(ruleLabel(slackRule()), ':deck:');
  assert.equal(ruleLabel({ source: 'channel', id: 'channel-1', name: 'Reviews' }), 'Reviews');
  assert.equal(ruleLabel({ source: 'channel', id: 'channel-1', name: '' }), 'channel-1');
});

test('a run line states its outcome; only a running run with a card can be opened', () => {
  const started = nowSecs - 600;
  const running = runSummary({ outcome: 'running', started, card: 'c1' }, now);
  assert.equal(running.outcome, 'running');
  assert.match(running.text, /^● .* · running$/);
  assert.equal(running.openable, true);
  assert.equal(runSummary({ outcome: 'running', started }, now).openable, false);
  const closed = runSummary({ outcome: 'closed', started, ended: started + 290 }, now);
  assert.deepEqual([closed.outcome, closed.mark], ['closed', '✓ ']);
  assert.match(closed.text, / · 5 min · closed$/);
  assert.match(runSummary({ outcome: 'closed', started }, now).text, /^[^·]+ · closed$/, 'no duration without an end');
  assert.match(runSummary({ outcome: 'skipped', started, reason: 'missed' }, now).text, /^– .* · skipped \(deck was not running\)$/);
  assert.match(runSummary({ outcome: 'skipped', started }, now).text, /· skipped$/);
});

test('the fact rows name the target, the inspection mode and the trigger\'s own facts', () => {
  const clock = ruleFacts(clockRule(), { columnName: 'Working', home: '/home', nowSecs });
  assert.deepEqual(clock.map(([key]) => key), ['automation.kv.target', 'automation.kv.cmd', 'automation.kv.template', 'queue.plan', 'automation.kv.finish', 'automation.kv.grace', 'automation.kv.next', 'automation.kv.firstSend']);
  assert.equal(clock[0][1], 'Working · /w');
  assert.equal(clock[3][1], 'Time and quiet delivery');
  assert.equal(clock[4][1], 'close the card');
  assert.equal(clock[5][1], 'still start within 15 min');
  assert.notEqual(clock[6][1], '—', 'a weekly schedule always has a next slot');
  const paused = ruleFacts(clockRule({ enabled: false, reviewEach: true, cmd: '', dir: '' }), { home: '/home', nowSecs });
  assert.equal(paused[0][1], '(column missing) · /home');
  assert.equal(paused[1][1], 'shell only');
  assert.equal(paused[3][1], 'Inspect after each step');
  assert.equal(paused[6][1], 'paused');
  const slack = ruleFacts(slackRule(), { columnName: 'Working', slackConnected: true });
  assert.deepEqual(slack.slice(-3)[0], ['automation.kv.connection', 'connected']);
  assert.match(ruleFacts(slackRule(), {}).slice(-3)[0][1], /^not connected/);
  assert.deepEqual(slack.slice(-2)[0], ['automation.kv.autoSend', 'off — each follow-up waits for Send now']);
  assert.deepEqual(slack.slice(-1)[0], ['automation.kv.firstSend', 'waits for your first interaction or Send now']);
});

test('recent runs belong to the rule by id (clock) or badge (Slack), newest first, at most four', () => {
  const runs = [1, 2, 3, 4, 5].map(n => ({ rule: 'a1', started: n })).concat([{ rule: 'deck', started: 9 }, { rule: 'other', started: 8 }]);
  assert.deepEqual(recentRuns(runs, clockRule()).map(r => r.started), [5, 4, 3, 2]);
  assert.deepEqual(recentRuns(runs, slackRule()).map(r => r.started), [9]);
});

test('composing a rule refuses what is missing and names the control to focus', () => {
  const base = { trigger: 'clock', name: 'Nightly', columnId: 'C', template: 'tpl', cmd: 'claude', dir: '/w', finish: 'close', reviewEach: false, unit: 'week', days: [1, 3], monthDays: '1', time: '09:00', graceMin: '15' };
  const opts = { projectId: 'P', genId, nowSecs };
  assert.deepEqual(composeRule({ ...base, columnId: '' }, opts), { error: 'automation.needsColumn', focus: 'auto-column', params: {} });
  assert.deepEqual(composeRule({ ...base, template: '' }, opts), { error: 'automation.needsTemplate', focus: 'auto-template', params: {} });
  assert.equal(composeRule({ ...base, name: 'x'.repeat(121) }, opts).error, 'automation.longName');
  assert.equal(composeRule({ ...base, name: '' }, opts).error, 'automation.needsName');
  assert.deepEqual(composeRule({ ...base, days: [] }, opts), { error: 'automation.needsDays', focus: null, params: {} });
  assert.equal(composeRule({ ...base, time: '' }, opts).error, 'automation.needsTime');
  assert.equal(composeRule({ ...base, trigger: 'slack', badge: 'Deck Badge' }, opts).error, 'automation.invalidBadge');
  const taken = composeRule({ ...base, trigger: 'slack', badge: ':deck:' }, { ...opts, rules: [slackRule()] });
  assert.deepEqual(taken, { error: 'automation.badgeTaken', focus: 'auto-badge', params: { badge: 'deck' } });
});

test('a composed clock rule uses its id as its badge and starts its schedule at now', () => {
  const fields = { trigger: 'clock', name: ' Nightly ', columnId: 'C', template: 'tpl', cmd: ' claude ', dir: '/w', finish: 'keep', reviewEach: true, unit: 'week', days: [1, 3], monthDays: '1', time: '09:00', graceMin: '1440' };
  ids = 0;
  const { rule } = composeRule(fields, { projectId: 'P', genId, nowSecs });
  assert.deepEqual(rule, {
    id: 'a1', source: 'clock', badge: 'a1', projectId: 'P', columnId: 'C', cmd: 'claude', template: 'tpl', dir: '/w', name: 'Nightly',
    enabled: true, finish: 'keep', reviewEach: true, schedule: { unit: 'week', days: [1, 3], minute: 540 }, graceMin: 1440, since: nowSecs,
  });
  const monthly = composeRule({ ...fields, unit: 'month', monthDays: '15' }, { projectId: 'P', genId, nowSecs }).rule;
  assert.deepEqual(monthly.schedule, { unit: 'month', days: [15], minute: 540 });
  assert.equal('reviewEach' in composeRule({ ...fields, reviewEach: false }, { projectId: 'P', genId, nowSecs }).rule, false);
});

test('an interval schedule reads with its count and date, and is read from the controls only with a real anchor', () => {
  assert.equal(scheduleText({ unit: 'week', days: [1, 5], minute: 540, every: 2, anchor: '2026-10-19' }), 'every 2 weeks on Mon, Fri at 09:00, from 2026-10-19');
  assert.equal(scheduleText({ unit: 'month', days: [1], minute: 0, every: 3, anchor: '2026-10-01' }), 'every 3 months on day 1 at 00:00, from 2026-10-01');
  assert.equal(scheduleText({ unit: 'day', days: [], minute: 60, every: 3, anchor: '2026-10-30' }), 'every 3 days at 01:00, from 2026-10-30');
  assert.equal(scheduleText({ unit: 'day', days: [], minute: 60, every: 1 }), 'every day at 01:00', 'an interval of 1 reads as before');
  const fields = { unit: 'week', days: [1, 5], monthDays: '1', time: '09:00', every: '2', anchor: '2026-10-19' };
  assert.deepEqual(scheduleFromFields(fields), { schedule: { unit: 'week', days: [1, 5], minute: 540, every: 2, anchor: '2026-10-19' } });
  assert.deepEqual(scheduleFromFields({ ...fields, every: '1' }), { schedule: { unit: 'week', days: [1, 5], minute: 540 } }, 'an interval of 1 stores no date');
  assert.deepEqual(scheduleFromFields({ ...fields, every: '' }), { schedule: { unit: 'week', days: [1, 5], minute: 540 } }, 'an empty box is 1');
  for (const every of ['13', '0', '2.5', 'x']) {
    assert.deepEqual(scheduleFromFields({ ...fields, every }), { error: 'automation.badEvery', focus: 'auto-every' }, every);
  }
  for (const anchor of ['', '2026-02-30', '19.10.2026']) {
    assert.deepEqual(scheduleFromFields({ ...fields, anchor }), { error: 'automation.needsAnchor', focus: 'auto-anchor' }, anchor);
  }
  assert.deepEqual(scheduleFromFields({ ...fields, days: [] }), { error: 'automation.needsDays', focus: null });
  assert.deepEqual(scheduleFromFields({ ...fields, time: '' }), { error: 'automation.needsTime', focus: 'auto-time' });
  assert.deepEqual(scheduleFromFields({ ...fields, unit: 'month', monthDays: '15, 1, 15' }),
    { schedule: { unit: 'month', days: [1, 15], minute: 540, every: 2, anchor: '2026-10-19' } }, 'sorted and unique');
  for (const monthDays of ['0, 40', '', '1.5', '1, x', '32']) {
    assert.deepEqual(scheduleFromFields({ ...fields, unit: 'month', monthDays }), { error: 'automation.badMonthDays', focus: 'auto-dom' }, monthDays);
  }
  assert.deepEqual(scheduleFromFields({ ...fields, unit: 'day' }), { schedule: { unit: 'day', days: [], minute: 540, every: 2, anchor: '2026-10-19' } });
  assert.deepEqual(parseMonthDays('31、1，15 2'), [1, 2, 15, 31], 'every separator a keyboard offers');
  assert.equal(parseMonthDays('1.5'), null);
  assert.equal(parseMonthDays(null), null);
});

test('an interval rule composes with its anchor; a new interval or date starts fresh, a rename keeps both', () => {
  const previous = clockRule({ since: 1000, schedule: { unit: 'week', days: [1], minute: 540, every: 2, anchor: '2026-10-19' } });
  const fields = { trigger: 'clock', name: 'Sprint', columnId: 'C', template: 'tpl', cmd: 'claude', dir: '/w', finish: 'close', reviewEach: false,
    unit: 'week', days: [1], monthDays: '1', time: '09:00', every: '2', anchor: '2026-10-19', graceMin: '15' };
  const opts = { previous, projectId: 'P', genId, nowSecs };
  const same = composeRule(fields, opts).rule;
  assert.deepEqual([same.since, same.schedule], [1000, { unit: 'week', days: [1], minute: 540, every: 2, anchor: '2026-10-19' }], 'a rename keeps the phase and since');
  const widened = composeRule({ ...fields, every: '3' }, opts).rule;
  assert.deepEqual([widened.since, widened.schedule.every, widened.schedule.anchor], [nowSecs, 3, '2026-10-19'], 'a new interval keeps the date the user set');
  const moved = composeRule({ ...fields, anchor: '2026-10-26' }, opts).rule;
  assert.deepEqual([moved.since, moved.schedule.anchor], [nowSecs, '2026-10-26']);
  const plain = composeRule({ ...fields, every: '1' }, opts).rule;
  assert.deepEqual([plain.since, plain.schedule], [nowSecs, { unit: 'week', days: [1], minute: 540 }], 'back to every week: no date stored');
  assert.deepEqual(composeRule({ ...fields, anchor: '' }, opts), { error: 'automation.needsAnchor', focus: 'auto-anchor', params: {} });
  assert.deepEqual(composeRule({ ...fields, every: '99' }, opts), { error: 'automation.badEvery', focus: 'auto-every', params: {} });
  assert.equal(triggerText(same), 'every 2 weeks on Mon at 09:00, from 2026-10-19');
});

test('the preview names the next three slots with their weekdays, the year only when it is not this one', () => {
  const now = Math.floor(new Date(2026, 9, 19, 8, 0).getTime() / 1000);     // Monday 2026-10-19, 08:00
  assert.equal(schedulePreview({ unit: 'week', days: [1], minute: 540, every: 2, anchor: '2026-10-19' }, now), 'Mon 10-19 · Mon 11-02 · Mon 11-16');
  assert.equal(schedulePreview({ unit: 'week', days: [1], minute: 420, every: 2, anchor: '2026-10-19' }, now), 'Mon 11-02 · Mon 11-16 · Mon 11-30', 'a slot earlier today is not next');
  assert.equal(schedulePreview({ unit: 'month', days: [1], minute: 540, every: 12, anchor: '2026-10-01' }, now), 'Fri 2027-10-01 · Sun 2028-10-01 · Mon 2029-10-01');
  assert.equal(schedulePreview({ unit: 'week', days: [1], minute: 540, every: 2, anchor: '2027-03-01' }, now), 'Mon 2027-03-01 · Mon 2027-03-15 · Mon 2027-03-29');
  assert.equal(schedulePreview({ unit: 'day', days: [], minute: 540 }, now), 'Mon 10-19 · Tue 10-20 · Wed 10-21', 'a plain rule previews too');
});

test('editing keeps the rule\'s id and pause state; only a changed schedule or trigger starts fresh', () => {
  const previous = clockRule({ enabled: false, since: 1000 });
  const fields = { trigger: 'clock', name: 'Nightly', columnId: 'C', template: 'tpl', cmd: 'claude', dir: '/w', finish: 'close', reviewEach: false, unit: 'week', days: [1, 3], monthDays: '1', time: '09:00', graceMin: '15' };
  const same = composeRule(fields, { previous, projectId: 'P', genId, nowSecs }).rule;
  assert.deepEqual([same.id, same.enabled, same.since], ['a1', false, 1000]);
  const moved = composeRule({ ...fields, time: '10:00' }, { previous, projectId: 'P', genId, nowSecs }).rule;
  assert.deepEqual([moved.id, moved.since], ['a1', nowSecs]);
  ids = 0;
  const retriggered = composeRule({ ...fields, trigger: 'slack', badge: 'deck' }, { previous, projectId: 'P', genId, nowSecs }).rule;
  assert.deepEqual([retriggered.id, retriggered.source, retriggered.badge, retriggered.enabled], ['R1', 'slack', 'deck', true], 'a Slack rule has no pause');
  const kept = composeRule({ ...fields, trigger: 'slack', badge: 'deck' }, { previous: slackRule(), projectId: 'P', genId, nowSecs }).rule;
  assert.equal(kept.id, 'r1');
});

test('a saved rule replaces its entry in place, also under the id a trigger change retired', () => {
  const rules = [clockRule(), slackRule()];
  const edited = clockRule({ name: 'Renamed' });
  assert.deepEqual(mergeRules(rules, edited).map(r => r.name), ['Renamed', '']);
  const fresh = slackRule({ id: 'r2', badge: 'ship' });
  assert.deepEqual(mergeRules(rules, fresh).map(r => r.id), ['a1', 'r1', 'r2']);
  const retriggered = slackRule({ id: 'R9', badge: 'nightly' });
  assert.deepEqual(mergeRules(rules, retriggered, 'a1').map(r => r.id), ['R9', 'r1']);
  assert.deepEqual(rules.map(r => r.id), ['a1', 'r1'], 'the input array is never mutated');
});

test('rules of a deleted project are dropped; nothing changes when every project is live', () => {
  const rules = [clockRule(), slackRule({ projectId: 'gone' })];
  assert.equal(liveRules(rules, [{ id: 'P' }, { id: 'gone' }]), null);
  assert.deepEqual(liveRules(rules, [{ id: 'P' }]).map(r => r.id), ['a1']);
});

/* ---------- delivery approval (twin of scheduler/authority.rs) ---------- */

const vector = JSON.parse(readFileSync(new URL('./fixtures/automation-grant.json', import.meta.url), 'utf8'));
const template = { name: vector.rule.template, steps: vector.templateSteps };

test('the approval digest is byte-identical to the backend vector', async () => {
  const grant = vector.rule.autoSend;
  assert.equal(grantManifest(vector.rule, grant.steps, grant.classes, grant.external), vector.manifest);
  assert.equal(await grantDigest(vector.rule, grant), grant.digest);
  const { autoSend, ...plain } = vector.rule;
  const approved = await approveRule(plain, template, { external: true });
  assert.deepEqual(approved.autoSend, autoSend, 'approving the fixture rule reproduces its stored approval');
  assert.equal(await grantState(vector.rule, template), 'valid');
});

test('steps are fixed owner text or bounded by known message placeholders', async () => {
  assert.equal(stepClass('Review and run the tests.'), 'fixed');
  assert.equal(stepClass('Look at {{msg.text}}'), 'bounded');
  assert.equal(stepClass('Look at {{ msg.link }}'), 'bounded');
  assert.equal(stepClass('Literal {{msg.unknown}} stays text'), 'fixed');
  assert.equal(templateCarriesMessage(template), true);
  assert.equal(templateCarriesMessage({ steps: ['a', 'b'] }), false);
  // the external box is meaningless without a bounded step and is stored false
  const { autoSend, ...plain } = vector.rule;
  const fixedOnly = await approveRule(plain, { steps: ['one', 'two'] }, { external: true });
  assert.equal(fixedOnly.autoSend.external, false);
  assert.deepEqual(fixedOnly.autoSend.classes, ['fixed', 'fixed']);
});

test('any meaning-bearing edit leaves a stale approval; presentation edits do not', async () => {
  const rule = vector.rule;
  for (const [what, edited, tpl] of [
    ['template text', rule, { ...template, steps: ['changed', ...template.steps.slice(1)] }],
    ['step order', rule, { ...template, steps: [...template.steps].reverse() }],
    ['step removed', rule, { ...template, steps: template.steps.slice(1) }],
    ['template missing', rule, null],
    ['Claude ↔ Codex', { ...rule, cmd: 'claude' }, template],
    ['command args', { ...rule, cmd: 'codex' }, template],
    ['directory', { ...rule, dir: '~/other' }, template],
    ['project', { ...rule, projectId: 'P2' }, template],
    ['template identity', { ...rule, template: 'other' }, template],
    ['badge', { ...rule, badge: 'rocket' }, template],
    ['trigger', { ...rule, source: 'clock' }, template],
    ['review', { ...rule, reviewEach: false }, template],
    ['finish', { ...rule, finish: 'keep' }, template],
    ['external policy', { ...rule, autoSend: { ...rule.autoSend, external: false } }, template],
  ]) assert.equal(await grantState(edited, tpl), 'stale', what);
  for (const [what, edited] of [
    ['name', { ...rule, name: 'Renamed' }], ['column', { ...rule, columnId: 'C9' }], ['enabled', { ...rule, enabled: false }],
  ]) assert.equal(await grantState(edited, template), 'valid', what);
  assert.equal(await grantState({ ...rule, autoSend: undefined }, template), 'none');
  assert.equal(approvalText(rule, 'valid'), 'approved follow-up steps continue, including Slack message content');
  assert.equal(approvalText({ ...rule, autoSend: { ...rule.autoSend, external: false } }, 'valid'), 'approved follow-up steps continue (no Slack message content)');
  assert.match(approvalText(rule, 'stale'), /^off — the rule or its template changed/);
  assert.match(approvalText(rule, 'none'), /^off — each follow-up waits/);
});

test('composing a rule never carries an old approval forward', () => {
  const { rule } = composeRule({ trigger: 'slack', badge: 'deck', columnId: 'C', template: 'tpl', cmd: 'claude', finish: 'keep' },
    { previous: { ...slackRule(), autoSend: vector.rule.autoSend }, rules: [], projectId: 'P', genId });
  assert.equal('autoSend' in rule, false);
});

test('the bounded-step expansion vectors are the webview\'s own fill (the backend twin checks the same vectors)', async () => {
  const { fillInboundTemplate } = await import('../js/pure.js');
  assert.ok(vector.expansions.length >= 5);
  for (const v of vector.expansions) assert.equal(fillInboundTemplate(v.skeleton, v.msg), v.expected, v.skeleton);
});

test('the first-send readiness override is a separate, clock-or-Slack-badge, confirmed choice', async () => {
  // off unless explicitly on; only a Slack badge rule keeps it
  assert.equal('firstSendWithoutReadiness' in withFirstSend(slackRule(), false), false);
  assert.equal(withFirstSend(slackRule(), true).firstSendWithoutReadiness, true);
  assert.equal('firstSendWithoutReadiness' in withFirstSend(slackRule({ firstSendWithoutReadiness: true }), false), false,
    'turning it off removes it');
  assert.equal(withFirstSend(clockRule(), true).firstSendWithoutReadiness, true);
  assert.equal('firstSendWithoutReadiness' in withFirstSend({ source: 'channel', id: 'c1' }, true), false);
  assert.equal(withFirstSend(slackRule(), 'yes').firstSendWithoutReadiness, undefined, 'only a real true');
  // turning it on asks; turning it off (or keeping it) never does
  assert.equal(firstSendNeedsConfirm(false, true), true);
  assert.equal(firstSendNeedsConfirm(true, false), false);
  assert.equal(firstSendNeedsConfirm(false, false), false);
  // Claude, or Codex kept out of its shared daemon (twin of first_send.rs)
  for (const [cmd, ok] of [['claude', true], ['claude --model opus', true], ['codex --no-daemon', true],
    ['codex --yolo --no-daemon', true], ['codex', false], ['codex --yolo', false], ['codex --no-daemon=false', false],
    ['', false], ['bash', false]]) assert.equal(firstSendSupported(cmd), ok, cmd);
  // independent of the follow-up approval: not part of the grant digest
  const template = { name: 'tpl', steps: ['Review.', 'Test.'] };
  const plain = await approveRule(slackRule({ cmd: 'claude' }), template);
  const withIt = await approveRule(withFirstSend(slackRule({ cmd: 'claude' }), true), template);
  assert.equal(withIt.autoSend.digest, plain.autoSend.digest);
  assert.equal(await grantState(withFirstSend(plain, true), template), 'valid');
  // what the rule's facts say, per state
  assert.equal(firstSendText(slackRule()), 'waits for your first interaction or Send now');
  assert.equal(firstSendText(slackRule({ cmd: 'claude', firstSendWithoutReadiness: true })),
    'sent without waiting for readiness (startup-dialog risk accepted)');
  assert.equal(firstSendText(slackRule({ cmd: 'codex', firstSendWithoutReadiness: true })),
    'on, but not used: the command must be Claude, or Codex with --no-daemon');
  const facts = rule => Object.fromEntries(ruleFacts(rule));
  assert.equal(facts(slackRule())['automation.kv.firstSend'], 'waits for your first interaction or Send now');
  assert.equal(facts(clockRule())['automation.kv.firstSend'], 'waits for your first interaction or Send now');
  assert.equal('automation.kv.firstSend' in facts({ ...slackRule(), source: 'channel', channelIds: [], senderUserIds: [], senderBotIds: [], idleMinutes: 30 }), false);
});

test('the first-send option and its warning are in both languages and say the right thing', async () => {
  const en = (await import('../js/i18n/en.js')).default;
  const zh = (await import('../js/i18n/zh-Hans.js')).default;
  for (const key of ['automation.firstSend.option', 'automation.firstSend.hint', 'automation.firstSend.confirm',
    'automation.firstSend.on', 'automation.firstSend.off', 'automation.firstSend.unsupported',
    'automation.kv.firstSend', 'queue.stage.firstSendOverride']) {
    assert.ok(en[key] && zh[key], key);
  }
  assert.equal(en['automation.autoSend.option'], 'Automatically continue approved follow-up steps');
  assert.equal(zh['automation.autoSend.option'], '自动继续已批准的后续步骤');
  assert.equal(zh['automation.firstSend.option'], '新启动的 Agent 也直接发送第一步');
  for (const word of ['Trust', 'Update', 'permission', 'Enter', 'risk']) {
    assert.ok(en['automation.firstSend.confirm'].includes(word), word);
  }
  for (const word of ['Trust', 'Update', 'Permission', '回车', '风险']) {
    assert.ok(zh['automation.firstSend.confirm'].includes(word), word);
  }
  assert.ok(/--no-daemon/.test(en['automation.firstSend.hint']) && /--no-daemon/.test(zh['automation.firstSend.hint']));
  // the user-facing concept is never "bypass", and the ordinary first-send
  // stage asks for readiness, not approval
  for (const dict of [en, zh]) {
    for (const key of Object.keys(dict).filter(k => k.startsWith('automation.firstSend.'))) {
      assert.equal(/bypass|绕过/i.test(dict[key]), false, key);
    }
  }
  assert.equal(/approv/i.test(en['queue.stage.firstSend']), false);
  assert.ok(/ready/.test(en['queue.stage.firstSend']));
});

// F07: a clock rule that closes its card says what that depends on. The
// sentence is shown for that combination only, and names no agent's command.
test('the close-the-card hint is for a clock rule that closes its card', async () => {
  const { finishHintShown } = await import('../js/automation-model.js');
  const { dictionaries } = await import('../js/i18n.js');
  const { readFileSync } = await import('node:fs');
  assert.equal(finishHintShown('clock', 'close'), true);
  assert.equal(finishHintShown('clock', 'keep'), false);
  assert.equal(finishHintShown('slack', 'close'), false, 'a badge run has no slot to skip');
  assert.equal(finishHintShown('channel', 'close'), false);
  assert.equal(finishHintShown(undefined, undefined), false);
  for (const [locale, dictionary] of Object.entries(dictionaries)) {
    const hint = dictionary['automation.finish.closeHint'];
    assert.equal(typeof hint, 'string', locale);
    assert.doesNotMatch(hint, /\/exit|\/quit|claude|codex/i, `${locale}: no vendor command in the interface`);
  }
  const read = name => readFileSync(new URL(`../${name}`, import.meta.url), 'utf8');
  assert.match(read('index.html'), /<p class="q-hint" id="auto-finish-hint" data-i18n="automation\.finish\.closeHint" hidden>/,
    'hidden until the editor decides');
  assert.match(read('js/automation.js'), /\$\('auto-finish-hint'\)\.hidden = !finishHintShown\(segGet\('auto-trigger'\), segGet\('auto-finish'\)\);/);
});

// F16: why an approval grants nothing, and which approvals one template save voids.
test('a stale approval says whether the template or the rule changed', async () => {
  const { grantDetail, approvalsVoidedBy } = await import('../js/automation-model.js');
  const rule = vector.rule;
  assert.equal(await grantDetail(rule, template), 'valid');
  assert.equal(await grantDetail({ ...rule, autoSend: undefined }, template), 'none');
  for (const tpl of [{ ...template, steps: ['changed', ...template.steps.slice(1)] },
    { ...template, steps: [...template.steps].reverse() }, { ...template, steps: template.steps.slice(1) }, null]) {
    assert.equal(await grantDetail(rule, tpl), 'stale-template');
    assert.equal(await grantState(rule, tpl), 'stale', 'the three-state reading is unchanged');
  }
  for (const edited of [{ ...rule, cmd: 'claude' }, { ...rule, dir: '~/other' }, { ...rule, finish: 'keep' }, { ...rule, source: 'clock' }]) {
    assert.equal(await grantDetail(edited, template), 'stale-rule');
    assert.equal(await grantState(edited, template), 'stale');
  }
  assert.notEqual(approvalText(rule, 'stale-template'), approvalText(rule, 'stale-rule'));
  assert.notEqual(approvalText(rule, 'stale-template'), approvalText(rule, 'stale'));
  assert.match(approvalText(rule, 'stale-template'), /template/i);
  assert.match(approvalText(rule, 'stale-rule'), /rule/i);
  // one save of this template: only a rule that WAS approved against the old
  // steps and is not against the new ones
  const changed = ['changed', ...template.steps.slice(1)];
  const rules = [rule, { ...rule, id: 'never', autoSend: undefined }, { ...rule, id: 'elsewhere', projectId: 'P2' },
    { ...rule, id: 'other-template', template: 'other' }, { ...rule, id: 'already-stale', cmd: 'claude' },
    { ...rule, id: 'clock', source: 'clock' }];
  const voided = await approvalsVoidedBy(rules, rule.projectId, rule.template, template.steps, changed);
  assert.deepEqual(voided.map(value => value.id), [rule.id]);
  assert.deepEqual(await approvalsVoidedBy(rules, rule.projectId, rule.template, template.steps, [...template.steps]), [],
    'a save that changes no step voids nothing');
  assert.deepEqual(await approvalsVoidedBy(rules, rule.projectId, rule.template, changed, template.steps), [],
    'an approval that was not valid before is not "voided" by this save');
  assert.deepEqual(await approvalsVoidedBy(null, rule.projectId, rule.template, template.steps, changed), []);
});
