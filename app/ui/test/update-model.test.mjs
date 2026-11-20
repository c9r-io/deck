import test from 'node:test';
import assert from 'node:assert/strict';
import { updateCheckFailureKey } from '../js/update-model.js';

test('a failed update check names its reason, and only the network asks about the connection', () => {
  assert.equal(updateCheckFailureKey('update check failed: network'), 'update.checkFailedNetwork');
  assert.equal(updateCheckFailureKey('update check failed: timeout'), 'update.checkFailedNetwork');
  assert.equal(updateCheckFailureKey('update check failed: feed-unavailable'), 'update.checkFailedFeed');
  assert.equal(updateCheckFailureKey('update check failed: feed-invalid'), 'update.checkFailedInvalid');
  assert.equal(updateCheckFailureKey('update check failed: other'), 'update.checkFailed');
  assert.equal(updateCheckFailureKey('update check failed: constructor'), 'update.checkFailed');
  assert.equal(updateCheckFailureKey(new Error('boom')), 'update.checkFailed');
  assert.equal(updateCheckFailureKey(undefined), 'update.checkFailed');
});
