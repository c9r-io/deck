// Exercise the actual Settings handlers: the blur/click ordering is the defect.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };
const { initSettings, renderInboundSettings } = await import('../js/settings.js');
const { ctx } = await import('../js/state.js');
const { normalizeSettings } = await import('../js/settings-model.js');
initSettings();
const el = id => fakeDocument.getElementById(id);
const tick = () => new Promise(resolve => setTimeout(resolve, 0));
const defer = () => { let resolve; const promise = new Promise(r => { resolve = r; }); return { promise, resolve }; };

test('real blur then finish waits for the saved token; status renders keep other drafts', async () => {
  ctx.settings = normalizeSettings({ inbound: { sources: { slack: { enabled: true } } } });
  const wait = defer(), calls = []; let bot = false;
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    calls.push(cmd);
    if (cmd === 'inbound_set_secret') { await wait.promise; bot = true; return; }
    if (cmd === 'inbound_status') return { sources: [] };
    if (cmd === 'channel_status') return {};
    if (cmd === 'slack_connection_status') return { botPresent: bot, botValid: bot, appPresent: true, appValid: true, userPresent: true, userValid: true, workspaceMatch: true };
    if (cmd === 'save_settings') return args.data;
  } } };
  el('set-channel-bot').value = 'xoxb-fixture';
  el('set-channel-bot').fire('change');
  const finishing = el('set-channel-finish').onclick();
  await tick();
  assert.equal(calls.filter(c => c === 'inbound_set_secret').length, 1);
  assert.equal(calls.includes('save_settings'), false);
  el('set-inbound-slack-user').value = 'draft-survives-status';
  await renderInboundSettings();
  assert.equal(el('set-inbound-slack-user').value, 'draft-survives-status');
  wait.resolve(); assert.equal(await finishing, true);
  assert.equal(calls.filter(c => c === 'save_settings').length, 1);
  assert.equal(ctx.settings.inbound.channelConnection.enabled, true);
  assert.equal(ctx.settings.inbound.sources.slack.enabled, true);
  assert.equal(el('set-channel-bot').value, '');
  assert.match(el('set-channel-setup-feedback').textContent, /enabled/);
  el('set-inbound-slack-user').value = '';
  await tick();
});

test('failed candidate remains editable and cannot enable with the previous token', async () => {
  ctx.settings = normalizeSettings({}); let saves = 0;
  window.__TAURI__ = { core: { invoke: async cmd => {
    if (cmd === 'inbound_set_secret') throw 'scope';
    if (cmd === 'save_settings') saves++;
    if (cmd === 'inbound_status') return { sources: [] };
    if (cmd === 'channel_status') return {};
    if (cmd === 'slack_connection_status') return { botPresent: true, botValid: true, appPresent: true, workspaceMatch: true };
  } } };
  el('set-channel-bot').value = 'xoxb-insufficient';
  assert.equal(await el('set-channel-finish').onclick(), false);
  await tick();
  assert.equal(saves, 0);
  assert.equal(el('set-channel-bot').value, 'xoxb-insufficient');
  assert.equal(el('set-channel-bot').disabled, false);
  assert.match(el('set-channel-setup-feedback').textContent, /scope|permission/i);
  el('set-channel-bot').value = '';
});

test('canceling the actual guide while status is pending prevents a late enable', async () => {
  ctx.settings = normalizeSettings({}); const wait = defer(); let saves = 0;
  window.__TAURI__ = { core: { invoke: async cmd => {
    if (cmd === 'slack_connection_status') return wait.promise;
    if (cmd === 'save_settings') saves++;
  } } };
  const running = el('set-channel-finish').onclick();
  await tick(); el('set-channel-cancel').onclick();
  wait.resolve({ botPresent: true, botValid: true, appPresent: true, workspaceMatch: true });
  assert.equal(await running, false); assert.equal(saves, 0);
  assert.equal(el('set-channel-upgrade-steps').hidden, true);
});

test('enabled guide refreshes live connection without another save and closing stops scheduled reads', async () => {
  ctx.settings = normalizeSettings({}); let connected = false, reads = 0, saves = 0;
  const timers = [], originalTimeout = globalThis.setTimeout;
  globalThis.setTimeout = (fn, delay) => { timers.push({ fn, delay }); return timers.length; };
  window.__TAURI__ = { core: { invoke: async (cmd, args) => {
    if (cmd === 'inbound_status') return { sources: [] };
    if (cmd === 'channel_status') return {};
    if (cmd === 'slack_connection_status') {
      reads++;
      return { botPresent: true, botValid: true, appPresent: true, workspaceMatch: true, connected };
    }
    if (cmd === 'save_settings') { saves++; return args.data; }
  } } };
  try {
    assert.equal(await el('set-channel-finish').onclick(), true);
    const first = timers.find(timer => timer.delay === 1000);
    assert.ok(first);
    await first.fn();
    const next = timers.find(timer => timer.delay === 3000);
    assert.ok(next);
    connected = true;
    await next.fn();
    assert.match(el('set-slack-transport').textContent, /connected/i);
    assert.equal(saves, 1);
    const before = reads;
    el('set-channel-cancel').onclick();
    await next.fn();
    assert.equal(reads, before);
  } finally { globalThis.setTimeout = originalTimeout; }
});
