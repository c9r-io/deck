// 自动响应 (inbound): the pure planner, template filling, settings shape and
// the durability of a card's origin. No DOM, no Tauri.
import test from 'node:test';
import assert from 'node:assert/strict';
import { expandHome, fillInboundTemplate, inboundTitle, planInbound, runDateLabel } from '../js/pure.js';
import { DEFAULT_GRACE_MIN, normalizeGrace, normalizeInbound, normalizeSchedule, normalizeSettings } from '../js/settings-model.js';

const msg = { text: 'line one\n\n  line two', from: 'alice', where: '#frontend', link: 'https://x.slack.com/p1' };

test('a step keeps its own lines and the message pasted into it does not', () => {
  assert.equal(fillInboundTemplate('Triage: {{msg.text}} — by {{ msg.from }} in {{msg.where}}', msg),
    'Triage: line one line two — by alice in #frontend',
    'third-party text is flattened: it must not reshape the prompt around it');
  /* the step's own newlines are the template author's, and survive */
  assert.equal(fillInboundTemplate('/bug-fix\n{{msg.text}}\n\nsee {{msg.link}}', msg),
    '/bug-fix\nline one line two\n\nsee https://x.slack.com/p1');
  assert.equal(fillInboundTemplate('keep {{msg.nope}} and {{other}}', msg), 'keep {{msg.nope}} and {{other}}');
  assert.equal(fillInboundTemplate('   ', msg), '');
  assert.ok(!fillInboundTemplate('a {{msg.text}}', { text: 'x\ry' }).includes('\r'),
    'and no carriage return reaches the queue by way of a message');
});

test('titles take the first non-empty line, bounded by characters not bytes', () => {
  assert.equal(inboundTitle('\n\n  fix login  \nmore'), 'fix login');
  assert.equal(inboundTitle('登录页在 Safari 上闪烁'.repeat(5), 10), '登录页在 Safa…');
  assert.equal(inboundTitle(''), '');
});

test('home expansion is explicit and never touches absolute paths', () => {
  assert.equal(expandHome('', '/Users/me'), '/Users/me');
  assert.equal(expandHome('~', '/Users/me/'), '/Users/me/');
  assert.equal(expandHome('~/work/web', '/Users/me/'), '/Users/me/work/web');
  assert.equal(expandHome('/srv/app', '/Users/me'), '/srv/app');
});

const projects = [{
  id: 'P1', name: 'web',
  columns: [{ id: 'C1', name: 'Slack' }, { id: 'C2', name: 'Working' }],
  templates: [{ name: 'triage', steps: ['/triage {{msg.text}}', 'summarize for {{msg.from}}'] }, { name: 'empty', steps: [] }],
}];
const rule = { id: 'R1', source: 'slack', badge: 'deck', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 'triage', dir: '~/work/web' };
const event = { source: 'slack', key: 'C9/1.2', badge: 'deck', text: 'login flickers\ndetails', from: 'alice', where: '#frontend', link: 'https://l' };
const item = { id: 7, event, rule };

test('the planner creates one card with origin, prompts and rule-derived fields', () => {
  const plan = planInbound(item, { cards: [], projects, home: '/Users/me' });
  assert.equal(plan.outcome, 'create');
  assert.deepEqual(plan.card, {
    projectId: 'P1', columnId: 'C1', title: 'login flickers', cmd: 'claude', dir: '/Users/me/work/web',
    desc: ':deck: · #frontend · alice',
    origin: { source: 'slack', key: 'C9/1.2', badge: 'deck' },
  });
  assert.deepEqual(plan.steps, ['/triage login flickers details', 'summarize for alice']);
  assert.equal(plan.template, 'triage');
});

test('the planner refuses to double-create and reports dangling rules honestly', () => {
  const existing = { id: 'S1', origin: { source: 'slack', key: 'C9/1.2', badge: 'deck' } };
  assert.equal(planInbound(item, { cards: [existing], projects, home: '/' }).outcome, 'duplicate');
  const otherBadge = { ...existing, origin: { ...existing.origin, badge: 'bug' } };
  assert.equal(planInbound(item, { cards: [otherBadge], projects, home: '/' }).outcome, 'create');
  assert.equal(planInbound({ ...item, rule: { ...rule, columnId: 'nope' } }, { cards: [], projects, home: '/' }).outcome, 'no-rule-target');
  assert.equal(planInbound({ ...item, rule: { ...rule, projectId: 'nope' } }, { cards: [], projects, home: '/' }).outcome, 'no-rule-target');
  const noTpl = planInbound({ ...item, rule: { ...rule, template: 'missing' } }, { cards: [], projects, home: '/' });
  assert.deepEqual(noTpl, { outcome: 'no-template', template: 'missing' });
  assert.equal(planInbound({ ...item, rule: { ...rule, template: 'empty' } }, { cards: [], projects, home: '/' }).outcome, 'no-template');
  const untitled = planInbound({ ...item, event: { ...event, text: '\n' } }, { cards: [], projects, home: '/' });
  assert.equal(untitled.card.title, ':deck:');
});

test('inbound settings normalize to a closed shape and drop rules the backend would refuse', () => {
  assert.deepEqual(normalizeInbound(undefined), { sources: { slack: { enabled: false } }, rules: [] });
  assert.deepEqual(normalizeInbound({ sources: { slack: { enabled: 'yes' }, notion: { enabled: true } } }).sources,
    { slack: { enabled: false } });
  const good = { id: 'R1', source: 'slack', badge: 'deck', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 't', dir: '' };
  const out = normalizeInbound({ sources: { slack: { enabled: true } }, rules: [
    good,
    { ...good, id: 'R2', badge: 'Deck' },
    { ...good, id: 'R3', badge: 'deck' },
    { ...good, id: 'R1', badge: 'bug' },
    { ...good, id: 'R4', source: 'notion', badge: 'x' },
    { ...good, id: 'R5', badge: 'y', cmd: 'a\nb' },
    { ...good, id: 'R6', badge: 'z', template: '' },
    { ...good, id: 'R7', badge: 'ok', dir: '~/w' },
    'garbage',
  ] });
  assert.equal(out.sources.slack.enabled, true);
  assert.deepEqual(out.rules.map(r => r.id), ['R1', 'R7']);
  assert.equal(out.rules[1].dir, '~/w');
  const settings = normalizeSettings({ inbound: { rules: [good] }, future: { kept: 1 } });
  assert.equal(settings.inbound.rules.length, 1);
  assert.deepEqual(settings.future, { kept: 1 });
  assert.deepEqual(normalizeSettings({}).inbound, { sources: { slack: { enabled: false } }, rules: [] });
});

/* ---------- 自动化: clock rules ---------- */
const clockRule = (over = {}) => ({
  id: 'a1', source: 'clock', badge: 'a1', projectId: 'P1', columnId: 'C1', cmd: 'claude',
  template: 'morning', dir: '', name: 'Morning tests', enabled: true,
  schedule: { unit: 'week', days: [1, 3, 5], minute: 540 }, finish: 'close', since: 0, ...over,
});
const board = {
  projects: [{ id: 'P1', name: 'deck', columns: [{ id: 'C1', name: 'Working' }],
    templates: [{ name: 'morning', steps: ['run the tests', 'fix the first failure'] }] }],
  home: '/Users/me',
};
const slot = 1_788_800_000;   // some epoch second

test('a clock slot becomes a dated card with the template, unless the rule already has a run', () => {
  const item = { id: 7, event: { source: 'clock', key: String(slot), badge: 'a1', text: 'Morning tests', from: '', where: '', link: '' }, rule: clockRule() };
  const plan = planInbound(item, { ...board, cards: [] });
  assert.equal(plan.outcome, 'create');
  assert.equal(plan.card.title, `Morning tests · ${runDateLabel(slot)}`);
  assert.equal(plan.card.desc, 'morning', 'the description names the template, not a badge');
  assert.deepEqual(plan.card.origin, { source: 'clock', key: String(slot), badge: 'a1' });
  assert.deepEqual(plan.steps, ['run the tests', 'fix the first failure']);
  assert.equal(plan.card.dir, '/Users/me');
  const running = { id: 'S1', origin: { source: 'clock', key: String(slot - 86400), badge: 'a1' } };
  assert.equal(planInbound(item, { ...board, cards: [running] }).outcome, 'busy', 'one run at a time');
  const other = { id: 'S2', origin: { source: 'clock', key: 'x', badge: 'a2' } };
  assert.equal(planInbound(item, { ...board, cards: [other] }).outcome, 'create', 'another rule\'s run does not block');
  assert.equal(planInbound(item, { ...board, cards: [{ id: 'S3', origin: item.event }] }).outcome, 'duplicate');
  const nameless = { ...item, rule: clockRule({ name: '' }) };
  assert.equal(planInbound(nameless, { ...board, cards: [] }).card.title, `a1 · ${runDateLabel(slot)}`);
});

test('clock rules normalize their schedule and keep their id as badge; slack rules carry none', () => {
  assert.deepEqual(normalizeSchedule({ unit: 'week', days: [5, 1, 5, '3', 9, 0], minute: 540 }),
    { unit: 'week', days: [1, 3, 5], minute: 540 }, 'sorted, unique, in range');
  assert.deepEqual(normalizeSchedule({ unit: 'day', days: [2], minute: 0 }), { unit: 'day', days: [], minute: 0 });
  assert.deepEqual(normalizeSchedule({ unit: 'month', days: [31], minute: 1439 }), { unit: 'month', days: [31], minute: 1439 });
  assert.equal(normalizeSchedule({ unit: 'week', days: [], minute: 0 }), null);
  assert.equal(normalizeSchedule({ unit: 'day', minute: 1440 }), null);
  assert.equal(normalizeSchedule({ unit: 'year', days: [1], minute: 0 }), null);
  assert.equal(normalizeSchedule(null), null);
  const { rules } = normalizeInbound({ rules: [
    clockRule({ badge: 'whatever', finish: 'archive', since: -5, enabled: undefined }),
    clockRule({ id: 'a2', badge: 'a2', schedule: { unit: 'week', days: [] } }),
    clockRule({ id: 'a3', badge: 'a3', name: 'two\nlines' }),
    clockRule({ id: 'A4', badge: 'A4' }),
    { id: 'R1', source: 'slack', badge: 'deck', projectId: 'P1', columnId: 'C1', cmd: 'claude', template: 't' },
  ] });
  assert.equal(rules.length, 2, 'an uppercase id cannot be a badge, so A4 is dropped too');
  assert.equal(rules[0].badge, 'a1', 'a clock rule\'s badge is its id');
  assert.equal(rules[0].finish, 'keep', 'unknown finish falls back to keep');
  assert.equal(rules[0].since, 0);
  assert.equal(rules[0].enabled, true);
  assert.equal(rules[0].graceMin, DEFAULT_GRACE_MIN, 'a rule without a grace gets the default');
  assert.equal(normalizeInbound({ rules: [clockRule({ graceMin: 0 })] }).rules[0].graceMin, 0, 'zero is a choice, not "unset"');
  assert.equal(normalizeInbound({ rules: [clockRule({ graceMin: 1440 })] }).rules[0].graceMin, 1440);
  assert.equal(normalizeInbound({ rules: [clockRule({ graceMin: 1441 })] }).rules[0].graceMin, DEFAULT_GRACE_MIN);
  assert.equal(normalizeGrace('15'), DEFAULT_GRACE_MIN, 'a string is not a minute count');
  assert.equal(normalizeGrace(-1), DEFAULT_GRACE_MIN);
  assert.equal(rules[1].graceMin, undefined, 'a slack rule has no grace');
  assert.deepEqual(rules[0].schedule, { unit: 'week', days: [1, 3, 5], minute: 540 });
  assert.equal(rules[1].id, 'R1');
  assert.equal(rules[1].enabled, true);
  assert.equal('schedule' in rules[1], false, 'slack rules carry no schedule');
  const settings = normalizeSettings({ inbound: { rules: [clockRule()] } });
  assert.equal(settings.inbound.rules[0].source, 'clock');
});
