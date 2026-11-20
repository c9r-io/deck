import test from 'node:test';
import assert from 'node:assert/strict';
import { tunnelActionKey, tunnelCode, tunnelFailureKey, tunnelSetupKey } from '../js/tunnel-model.js';

test('a rejected or unreachable Tunnel names the way out, not a generic failure', () => {
  assert.equal(tunnelFailureKey('tunnel_client_readiness_timeout'), 'mcp.tunnelFailure.connect');
  assert.equal(tunnelFailureKey('tunnel_not_ready'), 'mcp.tunnelFailure.connect');
  assert.equal(tunnelFailureKey('key_invalid'), 'mcp.tunnelFailure.key');
  assert.equal(tunnelFailureKey('keychain_unavailable'), 'mcp.tunnelFailure.key');
  assert.equal(tunnelFailureKey('tunnel_client_replaced'), 'mcp.tunnelFailure.client');
  assert.equal(tunnelFailureKey('helper_timeout'), 'mcp.tunnelFailure.other');
  assert.equal(tunnelFailureKey('anything else'), 'mcp.tunnelFailure.other');
});

test('every configured state offers setup again, so a dead key can be replaced', () => {
  assert.equal(tunnelSetupKey('not_configured'), 'mcp.tunnelSetup');
  assert.equal(tunnelSetupKey('key_missing'), 'mcp.tunnelSetup');
  for (const state of ['stopped', 'starting', 'ready', 'unhealthy', 'stale', 'error']) {
    assert.equal(tunnelSetupKey(state), 'mcp.tunnelResetup', state);
  }
  assert.equal(tunnelSetupKey('tunnel_client_missing'), null);
  assert.equal(tunnelSetupKey(undefined), null);
});

test('only a plain closed word is shown as a code', () => {
  assert.equal(tunnelCode('tunnel_client_readiness_timeout'), 'tunnel_client_readiness_timeout');
  assert.equal(tunnelCode('/Users/x/secret'), 'helper_action_failed');
  assert.equal(tunnelCode('Error: boom'), 'helper_action_failed');
  assert.equal(tunnelCode(undefined), 'helper_action_failed');
  assert.equal(tunnelActionKey('stop'), 'mcp.tunnelAction.stop');
  assert.equal(tunnelActionKey('remove'), 'mcp.tunnelAction.remove');
  assert.equal(tunnelActionKey(undefined), 'mcp.tunnelAction.start');
});
