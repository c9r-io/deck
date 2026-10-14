import test from 'node:test';
import assert from 'node:assert/strict';
import { createSlackChannelSetup, mergeSlackChannelManifest, slackAppSettingsUrl, slackChannelSetupIssue } from '../js/slack-setup-model.js';

const ready = { botPresent: true, botValid: true, appPresent: true, workspaceMatch: true };
const deferred = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };
const required = { features: { bot_user: { display_name: 'deck monitor', always_online: false } },
  oauth_config: { scopes: { bot: ['channels:history', 'groups:history'] } },
  settings: { event_subscriptions: { bot_events: ['message.channels', 'message.groups'] } } };

test('upgrade merges necessary channel configuration and preserves customized settings idempotently', () => {
  const original = { display_information: { name: 'My Deck', background_color: '#123456' },
    features: { bot_user: { display_name: 'My bot' }, app_home: { home_tab_enabled: true } },
    oauth_config: { redirect_urls: ['https://example.org/callback'], scopes: { user: ['search:read'], bot: ['chat:write'] } },
    settings: { token_rotation_enabled: true, event_subscriptions: { user_events: ['reaction_added'], bot_events: ['app_mention'] } } };
  const merged = mergeSlackChannelManifest(JSON.stringify(original), required);
  const doc = JSON.parse(merged);
  assert.deepEqual(doc.oauth_config.scopes.bot, ['chat:write', 'channels:history', 'groups:history']);
  assert.deepEqual(doc.oauth_config.scopes.user, original.oauth_config.scopes.user);
  assert.deepEqual(doc.features, original.features);
  assert.equal(doc.settings.token_rotation_enabled, true);
  assert.equal(doc.settings.socket_mode_enabled, true);
  assert.deepEqual(doc.settings.event_subscriptions.user_events, ['reaction_added']);
  assert.deepEqual(doc.settings.event_subscriptions.bot_events, ['app_mention', 'message.channels', 'message.groups']);
  assert.equal(mergeSlackChannelManifest(merged, required), merged);
  assert.deepEqual(JSON.parse(mergeSlackChannelManifest('{"display_information":{"name":"deck"}}', required)).features.bot_user, required.features.bot_user);
  for (const text of ['', '[]', '{}', 'x'.repeat(131073), '{"display_information":{"name":"d"},"settings":[]}', '{"display_information":{"name":"d"},"oauth_config":{"scopes":{"bot":"bad"}}}']) {
    assert.throws(() => mergeSlackChannelManifest(text, required));
  }
});

test('navigation confines App IDs and never substitutes a create-new URL', () => {
  assert.equal(slackAppSettingsUrl('A123', 'app-manifest', 'T123'), 'https://app.slack.com/app-settings/T123/A123/app-manifest');
  assert.equal(slackAppSettingsUrl('A123', 'oauth', 'T123'), 'https://app.slack.com/app-settings/T123/A123/oauth');
  assert.equal(slackAppSettingsUrl('A123'), 'https://api.slack.com/apps');
  for (const id of [null, '', '../x', 'A123?token=x', 'A123/evil', 'xapp-secret']) assert.equal(slackAppSettingsUrl(id, 'oauth', 'T123'), 'https://api.slack.com/apps');
  for (const team of [null, '', '../x', 'T123?token=x', 'T123/evil']) assert.equal(slackAppSettingsUrl('A123', 'oauth', team), 'https://api.slack.com/apps');
  assert.throws(() => slackAppSettingsUrl('A123', 'evil'));
});

test('setup distinguishes missing credentials, scopes, workspace, network and unavailable state', () => {
  assert.equal(slackChannelSetupIssue(null), 'unavailable');
  assert.equal(slackChannelSetupIssue({}), 'unavailable');
  assert.equal(slackChannelSetupIssue({ ...ready, botPresent: false }), 'needs-bot');
  assert.equal(slackChannelSetupIssue({ ...ready, appPresent: false }), 'needs-app');
  assert.equal(slackChannelSetupIssue({ ...ready, botValid: false, botError: 'scope' }), 'needs-scopes');
  assert.equal(slackChannelSetupIssue({ ...ready, botValid: false, botError: 'timeout' }), 'network');
  assert.equal(slackChannelSetupIssue({ ...ready, botValid: false, botError: 'auth' }), 'invalid-bot');
  assert.equal(slackChannelSetupIssue({ ...ready, workspaceMatch: false }), 'workspace-mismatch');
  assert.equal(slackChannelSetupIssue({ ...ready, userPresent: true, userValid: false }), 'user-error');
  assert.equal(slackChannelSetupIssue({ ...ready, appValid: false, connected: false }), null);
});

test('immediate enable waits for token save and duplicate clicks share one operation', async () => {
  const saved = deferred(), states = []; let reads = 0, writes = 0;
  const flow = createSlackChannelSetup({ saveTokens: () => saved.promise, readStatus: async () => { reads++; return ready; },
    enable: async proceed => { assert.equal(proceed(), true); writes++; return true; }, changed: s => states.push(s) });
  const first = flow.run(); assert.equal(flow.run(), first); assert.equal(reads, 0);
  saved.resolve(true); assert.equal(await first, true); assert.equal(reads, 1); assert.equal(writes, 1);
  assert.deepEqual(states, ['checking', 'enabled']);
});

test('cancel while saving or checking cannot later enable; a fresh attempt can retry', async () => {
  for (const phase of ['save', 'status']) {
    const wait = deferred(), states = []; let writes = 0;
    const flow = createSlackChannelSetup({ saveTokens: () => phase === 'save' ? wait.promise : true,
      readStatus: () => phase === 'status' ? wait.promise : ready,
      enable: async proceed => { writes++; return proceed(); }, changed: s => states.push(s) });
    const running = flow.run(); await Promise.resolve(); flow.cancel(); wait.resolve(phase === 'save' ? true : ready);
    assert.equal(await running, false); assert.equal(writes, 0); assert.deepEqual(states, ['checking']);
    assert.equal(await flow.run(), true);
  }
});

test('failed save, invalid capability, unavailable read and settings failure stay retryable', async () => {
  for (const [save, status, enable, expected] of [[false, ready, true, 'credential-error'], [true, null, true, 'unavailable'],
    [true, { ...ready, botPresent: false }, true, 'needs-bot'], [true, ready, false, 'save-error']]) {
    const states = [];
    const flow = createSlackChannelSetup({ saveTokens: async () => save, readStatus: async () => status, enable: async () => enable, changed: s => states.push(s) });
    assert.equal(await flow.run(), false); assert.equal(states.at(-1), expected);
  }
  const states = []; let failed = true;
  const flow = createSlackChannelSetup({ saveTokens: async () => true, readStatus: async () => { if (failed) throw Error('offline'); return ready; }, enable: async () => true, changed: s => states.push(s) });
  assert.equal(await flow.run(), false); assert.equal(states.at(-1), 'unavailable');
  failed = false; assert.equal(await flow.run(), true);
});
