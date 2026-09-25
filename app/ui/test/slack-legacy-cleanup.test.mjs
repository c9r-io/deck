import test from 'node:test';
import assert from 'node:assert/strict';
import { removeLegacySlackCredentials } from '../js/slack-legacy-cleanup.js';

test('cancel leaves legacy credentials and view untouched', async () => {
  const calls = [];
  await removeLegacySlackCredentials({
    confirm: async () => false,
    invoke: async () => calls.push('delete'),
    refresh: async () => calls.push('refresh'),
    notice: () => calls.push('notice'),
  });
  assert.deepEqual(calls, []);
});

test('confirmed cleanup invokes the narrow command then refreshes', async () => {
  const calls = [];
  await removeLegacySlackCredentials({
    confirm: async () => true,
    invoke: async command => { calls.push(command); },
    refresh: async () => calls.push('refresh'),
    notice: outcome => calls.push(outcome),
  });
  assert.deepEqual(calls, ['slack_legacy_credentials_clear', 'cleared', 'refresh']);
});

test('partial failure shows error, refreshes remaining presence, and permits retry', async () => {
  const calls = [];
  let attempt = 0;
  const action = {
    confirm: async () => true,
    invoke: async command => {
      calls.push(command);
      if (++attempt === 1) throw Error('keychain');
    },
    refresh: async () => calls.push('refresh'),
    notice: outcome => calls.push(outcome),
  };
  await removeLegacySlackCredentials(action);
  await removeLegacySlackCredentials(action);
  assert.deepEqual(calls, [
    'slack_legacy_credentials_clear', 'error', 'refresh',
    'slack_legacy_credentials_clear', 'cleared', 'refresh',
  ]);
});
