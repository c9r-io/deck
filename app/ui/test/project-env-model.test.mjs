import test from 'node:test';
import assert from 'node:assert/strict';
import {
  ENV_MAX_TOTAL_BYTES, ENV_MAX_VALUE_BYTES, ENV_MAX_VARS, envChanged, envEntries, envNameProblem, envStartFailureKey, envWarnings,
} from '../js/project-env-model.js';

const row = (name, value = '', stored = '') => ({ name, value, stored });

test('names follow the backend shape and DECK_/TMUX names are reserved', () => {
  assert.equal(envNameProblem('OPENAI_API_KEY'), null);
  assert.equal(envNameProblem('_a9'), null);
  assert.equal(envNameProblem('9a'), 'name');
  assert.equal(envNameProblem('A.B'), 'name');
  assert.equal(envNameProblem('A'.repeat(129)), 'name');
  assert.equal(envNameProblem('Deck_x'), 'reserved');
  assert.equal(envNameProblem('tmux_tmpdir'), 'reserved');
});

test('a typed value is sent, an untouched stored row keeps its value by name', () => {
  assert.deepEqual(envEntries([row(' A ', 'one'), row('B', '', 'B'), row('', '')]), {
    entries: [{ name: 'A', value: 'one' }, { name: 'B', value: null }],
  });
  assert.deepEqual(envEntries([]), { entries: [] });
});

test('a new or renamed row needs a value typed now', () => {
  assert.deepEqual(envEntries([row('NEW')]), { problem: 'value', index: 0 });
  assert.deepEqual(envEntries([row('X', 'ok'), row('RENAMED', '', 'OLD')]), { problem: 'value', index: 1 });
  assert.deepEqual(envEntries([row('RENAMED', 'v', 'OLD')]), { entries: [{ name: 'RENAMED', value: 'v' }] });
});

test('every refusal names the row it is about', () => {
  assert.deepEqual(envEntries([row('A', '1'), row('1A', '2')]), { problem: 'name', index: 1 });
  assert.deepEqual(envEntries([row('TMUX', '1')]), { problem: 'reserved', index: 0 });
  assert.deepEqual(envEntries([row('A', '1'), row('A', '2')]), { problem: 'duplicate', index: 1 });
  assert.deepEqual(envEntries([row('A', 'a\nb')]), { problem: 'control', index: 0 });
  assert.deepEqual(envEntries([row('A', 'a\tb')]), { problem: 'control', index: 0 });
  assert.deepEqual(envEntries([row('A', '\u0085')]), { problem: 'control', index: 0 });
  assert.deepEqual(envEntries([row('A', 'x'.repeat(ENV_MAX_VALUE_BYTES + 1))]), { problem: 'control', index: 0 });
  assert.deepEqual(envEntries([row('', 'value without a name')]), { problem: 'name', index: 0 });
  const many = Array.from({ length: ENV_MAX_VARS + 1 }, (_, i) => row(`V${i}`, '1'));
  assert.deepEqual(envEntries(many), { problem: 'count', index: ENV_MAX_VARS });
  const big = Array.from({ length: 5 }, (_, i) => row(`V${i}`, 'x'.repeat(ENV_MAX_VALUE_BYTES)));
  assert.ok(5 * ENV_MAX_VALUE_BYTES > ENV_MAX_TOTAL_BYTES);
  assert.deepEqual(envEntries(big), { problem: 'size', index: 4 });
  // a multi-byte value counts in bytes, as the backend does
  assert.deepEqual(envEntries([row('A', '密'.repeat(3000))]), { problem: 'control', index: 0 });
});

test('the stored list is touched only when the rows say something new', () => {
  assert.equal(envChanged([row('A', '', 'A'), row('', '')], ['A']), false);
  assert.equal(envChanged([row('A', 'new', 'A')], ['A']), true);
  assert.equal(envChanged([], ['A']), true);
  assert.equal(envChanged([row('B', '', 'A')], ['A']), true);
  assert.equal(envChanged([row('B', '', 'B'), row('A', '', 'A')], ['A', 'B']), true, 'order is part of the list');
  assert.equal(envChanged([], []), false);
});

test('names that move an Agent off Deck’s hooks are pointed out', () => {
  assert.deepEqual(envWarnings([row('CLAUDE_CONFIG_DIR'), row('X'), row(' CODEX_HOME ')]), ['CLAUDE_CONFIG_DIR', 'CODEX_HOME']);
  assert.deepEqual(envWarnings([row('X')]), []);
});

test('a card that did not start because of its project list says so', () => {
  assert.equal(envStartFailureKey('project environment unavailable'), 'projectEnv.startUnavailable');
  assert.equal(envStartFailureKey(new Error('project environment is invalid')), 'projectEnv.startInvalid');
  assert.equal(envStartFailureKey('tmux could not create the session'), null);
  assert.equal(envStartFailureKey(undefined), null);
});
