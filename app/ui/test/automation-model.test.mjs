// The automations drawer's DOM-free half: what a rule and its runs read as,
// how the editor's fields become a rule and how a saved rule joins the list.
// The drawer itself (automation.js) is verified by the WKWebView smoke.
import test from 'node:test';
import assert from 'node:assert/strict';
import { composeRule, graceOptions, graceText, liveRules, mergeRules, recentRuns, ruleFacts, ruleLabel, runSummary, scheduleText, triggerText } from '../js/automation-model.js';
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
  assert.deepEqual(clock.map(([key]) => key), ['automation.kv.target', 'automation.kv.cmd', 'automation.kv.template', 'queue.plan', 'automation.kv.finish', 'automation.kv.grace', 'automation.kv.next']);
  assert.equal(clock[0][1], 'Working · /w');
  assert.equal(clock[3][1], 'Time and quiet delivery');
  assert.equal(clock[4][1], 'close the card');
  assert.equal(clock[5][1], 'still start within 15 min');
  assert.notEqual(clock[6][1], '—', 'a weekly schedule always has a next slot');
  const paused = ruleFacts(clockRule({ enabled: false, reviewEach: true, cmd: '', dir: '' }), { home: '/home', nowSecs });
  assert.equal(paused[0][1], '(column missing) · /home');
  assert.equal(paused[1][1], 'shell only');
  assert.equal(paused[3][1], 'Inspect after each row');
  assert.equal(paused[6][1], 'paused');
  const slack = ruleFacts(slackRule(), { columnName: 'Working', slackConnected: true });
  assert.deepEqual(slack.slice(-1)[0], ['automation.kv.connection', 'connected']);
  assert.match(ruleFacts(slackRule(), {}).slice(-1)[0][1], /^not connected/);
});

test('recent runs belong to the rule by id (clock) or badge (Slack), newest first, at most four', () => {
  const runs = [1, 2, 3, 4, 5].map(n => ({ rule: 'a1', started: n })).concat([{ rule: 'deck', started: 9 }, { rule: 'other', started: 8 }]);
  assert.deepEqual(recentRuns(runs, clockRule()).map(r => r.started), [5, 4, 3, 2]);
  assert.deepEqual(recentRuns(runs, slackRule()).map(r => r.started), [9]);
});

test('composing a rule refuses what is missing and names the control to focus', () => {
  const base = { trigger: 'clock', name: 'Nightly', columnId: 'C', template: 'tpl', cmd: 'claude', dir: '/w', finish: 'close', reviewEach: false, unit: 'week', days: [1, 3], dayOfMonth: '1', time: '09:00', graceMin: '15' };
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
  const fields = { trigger: 'clock', name: ' Nightly ', columnId: 'C', template: 'tpl', cmd: ' claude ', dir: '/w', finish: 'keep', reviewEach: true, unit: 'week', days: [1, 3], dayOfMonth: '1', time: '09:00', graceMin: '1440' };
  ids = 0;
  const { rule } = composeRule(fields, { projectId: 'P', genId, nowSecs });
  assert.deepEqual(rule, {
    id: 'a1', source: 'clock', badge: 'a1', projectId: 'P', columnId: 'C', cmd: 'claude', template: 'tpl', dir: '/w', name: 'Nightly',
    enabled: true, finish: 'keep', reviewEach: true, schedule: { unit: 'week', days: [1, 3], minute: 540 }, graceMin: 1440, since: nowSecs,
  });
  const monthly = composeRule({ ...fields, unit: 'month', dayOfMonth: '15' }, { projectId: 'P', genId, nowSecs }).rule;
  assert.deepEqual(monthly.schedule, { unit: 'month', days: [15], minute: 540 });
  assert.equal('reviewEach' in composeRule({ ...fields, reviewEach: false }, { projectId: 'P', genId, nowSecs }).rule, false);
});

test('editing keeps the rule\'s id and pause state; only a changed schedule or trigger starts fresh', () => {
  const previous = clockRule({ enabled: false, since: 1000 });
  const fields = { trigger: 'clock', name: 'Nightly', columnId: 'C', template: 'tpl', cmd: 'claude', dir: '/w', finish: 'close', reviewEach: false, unit: 'week', days: [1, 3], dayOfMonth: '1', time: '09:00', graceMin: '15' };
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
