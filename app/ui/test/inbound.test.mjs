// 自动响应 (inbound): the pure planner, template filling, settings shape and
// the durability of a card's origin. No DOM, no Tauri.
import test from 'node:test';
import assert from 'node:assert/strict';
import { expandHome, fillInboundTemplate, inboundTitle, planInbound } from '../js/pure.js';
import { normalizeInbound, normalizeSettings } from '../js/settings-model.js';

const msg = { text: 'line one\n\n  line two', from: 'alice', where: '#frontend', link: 'https://x.slack.com/p1' };

test('placeholders fill from the message and prompts collapse to one line', () => {
  assert.equal(fillInboundTemplate('Triage: {{msg.text}} — by {{ msg.from }} in {{msg.where}}', msg),
    'Triage: line one line two — by alice in #frontend');
  assert.equal(fillInboundTemplate('/bug-fix\n{{msg.text}}\n\nsee {{msg.link}}', msg),
    '/bug-fix line one line two see https://x.slack.com/p1');
  assert.equal(fillInboundTemplate('keep {{msg.nope}} and {{other}}', msg), 'keep {{msg.nope}} and {{other}}');
  assert.equal(fillInboundTemplate('   ', msg), '');
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
