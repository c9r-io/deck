import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { slackConnectionView } from '../js/slack-connection-model.js';
const settings = (reaction = true, channel = false) => ({ inbound: { sources: { slack: { enabled: reaction } }, channelConnection: { enabled: channel } } });
const base = { userPresent: true, userValid: true, appPresent: true, appValid: true, botPresent: false, legacyPresent: false, workspaceMatch: true, connected: true };

test('one Slack section preserves reaction-only readiness and offers channel upgrade', () => {
  const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
  assert.equal((html.match(/data-setting-id="slack-reactions"/g) || []).length, 1);
  assert.equal((html.match(/data-setting-id="slack-channel"/g) || []).length, 0);
  assert.match(html, /id="set-channel-upgrade"/);
  assert.deepEqual(slackConnectionView(base, settings()).reaction, 'ready');
  assert.equal(slackConnectionView(base, settings()).channel, 'not-enabled');
  assert.match(html, /id="set-channel-bot"/);
  assert.doesNotMatch(html, /id="set-channel-app"/);
});
test('upgrade failures and legacy slots leave reaction readiness independent', () => {
  const old = { ...base, legacyPresent: true, channelRules: 2 };
  assert.equal(slackConnectionView(old, settings(true,true)).channel, 'upgrade-required');
  assert.equal(slackConnectionView(old, settings(true,false)).channel, 'upgrade-required');
  assert.equal(slackConnectionView(old, settings(true,true)).reaction, 'ready');
  assert.equal(slackConnectionView(old, settings(true,true)).legacy, true);
  assert.equal(slackConnectionView(old, settings(true,true)).legacyNotice, 'upgrade-required');
  assert.equal(slackConnectionView({ ...old, botPresent:true, botError:'scope' }, settings(true,true)).channel, 'needs-scopes');
  assert.equal(slackConnectionView({ ...old, botPresent:true, botValid:true, workspaceMatch:false }, settings(true,true)).channel, 'workspace-mismatch');
});
test('unified and channel-only status reconstruct from credentials and switches', () => {
  const unified = { ...base, botPresent:true, botValid:true };
  assert.equal(slackConnectionView(unified, settings(true,true)).channel, 'ready');
  assert.equal(slackConnectionView(unified, settings(true,false)).channel, 'off');
  assert.equal(slackConnectionView(unified, settings(false,true)).reaction, 'off');
  assert.equal(slackConnectionView({ ...unified, userPresent:false, userValid:false }, settings(false,true)).channel, 'ready');
  assert.equal(slackConnectionView({ ...unified, appPresent:false }, settings(true,true)).channel, 'needs-app');
  assert.equal(slackConnectionView({ ...unified, appValid:false }, settings(true,true)).channel, 'invalid');
  assert.equal(slackConnectionView({ ...unified, legacyPresent:true }, settings(true,true)).legacyNotice, 'retained');
  assert.equal(slackConnectionView({ ...unified, legacyPresent:true }, settings(true,false)).legacyNotice, 'retained');
});
