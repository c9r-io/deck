import test from 'node:test';
import assert from 'node:assert/strict';
import { channelAgentCommand, channelDigestId, channelRunExpired, channelSource, channelTemplatePlan, collectingCard, nextCollectedAt, normalizeChannelConfig, unfinishedChannelPlans } from '../js/channel-model.js';

const rule = { id: 'R1', enabled: true, channelIds: ['C1'], senderUserIds: ['U1'], senderBotIds: [],
  match: { kind: 'regex', value: 'INC-(?<incident>[0-9]+)', groupCapture: 'incident' }, includeThreads: true,
  projectId: 'P1', columnId: 'C1', dir: '/tmp', cmd: 'claude', template: 'triage', idleMinutes: 30 };

test('channel settings normalize closed rules while preserving ordinary inbound settings', () => {
  const got = normalizeChannelConfig({ channelConnection: { enabled: true }, channelRules: [rule, rule] });
  assert.deepEqual(got.channelConnection, { enabled: true, connectionId: 'default' });
  assert.equal(got.channelRules.length, 1);
  assert.equal(got.channelRules[0].match.groupCapture, 'incident');
  assert.equal(got.channelRules[0].idleMinutes, 30);
});

test('channel targets require an explicit Codex or Claude process', () => {
  assert.equal(channelAgentCommand('codex --full-auto'), 'codex');
  assert.equal(channelAgentCommand('env -i FOO=1 /opt/bin/claude --x'), 'claude');
  for (const cmd of ['', '/bin/zsh', '/bin/zsh -lc claude', 'while true', 'python bot.py', 'Claude']) {
    assert.equal(channelAgentCommand(cmd), null, cmd);
    assert.equal(normalizeChannelConfig({ channelRules: [{ ...rule, cmd }] }).channelRules.length, 0);
  }
  const unsafe = { target: { cmd: '', template: 'triage' }, body: 'incident; id' };
  assert.equal(channelTemplatePlan(unsafe, { templates: [{ name: 'triage', steps: ['{{msg.text}}'] }] }, 1).error, 'target');
});

test('group routing uses explicit group keys and idle windows only', () => {
  const item = { groupKey: 'default/T1/C1/R1/INC-4', target: { projectId: 'P1' } };
  const card = { projectId: 'P1', channelRun: { collecting: true, groupKey: item.groupKey, lastCollectedAt: 100, idleMinutes: 30 } };
  assert.equal(collectingCard([card], item), card);
  assert.equal(channelRunExpired(card.channelRun, 1900), false);
  assert.equal(channelRunExpired(card.channelRun, 1901), true);
});

test('idle collection uses deck collection time and never moves backward for old Slack backlog', () => {
  const run = { collecting: true, lastCollectedAt: 10_000, idleMinutes: 30 };
  assert.equal(nextCollectedAt(run, 9_000), 10_000, 'out-of-order arrivals do not shorten the window');
  assert.equal(nextCollectedAt(run, 10_050), 10_050);
  assert.equal(channelRunExpired({ ...run, lastCollectedAt: 10_050 }, 11_850), false);
  assert.equal(channelRunExpired({ ...run, lastCollectedAt: 10_050 }, 11_851), true);
});

test('template plan freezes expanded text and source retains reference ids without inventing links', () => {
  const item = { body: 'INC-4 failed', channelId: 'C1', eventId: 'Ev1', connectionId: 'default', workspaceId: 'T1',
    ruleId: 'R1', occurredAt: 123, messageTs: '123.4', senderUserId: 'U1', target: { cmd: 'claude', template: 'triage' } };
  const plan = channelTemplatePlan(item, { templates: [{ name: 'triage', steps: ['Handle {{msg.text}} in {{msg.where}}'] }] }, 500);
  assert.deepEqual(plan.texts, ['Handle INC-4 failed in C1']);
  assert.deepEqual(channelSource(item).links, []);
  assert.equal(channelSource(item).messageTs, '123.4');
});

test('first-event identities and unfinished queue journals are deterministic across crash recovery', async () => {
  const one = await channelDigestId('S', 'channel:default/T1/E1/R1');
  const two = await channelDigestId('S', 'channel:default/T1/E1/R1');
  assert.equal(one, two);
  assert.match(one, /^S[0-9a-f]{32}$/);
  const pending = { id: one, channelRun: { initialQueued: false, initialSteps: [{ operationId: 'B1', text: 'frozen' }] } };
  assert.deepEqual(unfinishedChannelPlans([pending, { channelRun: { initialQueued: true } }]), [pending]);
});
