import test from 'node:test';
import assert from 'node:assert/strict';
import { createInputSourceState } from '../js/input-source.js';

test('a late snapshot cannot replace a newer native source event', () => {
  const painted = [];
  const state = createInputSourceState(name => painted.push(name));
  assert.equal(state.apply({ sequence: 2, name: 'Installed input method' }), true);
  assert.equal(state.apply({ sequence: 1, name: 'Old layout' }), false);
  assert.equal(state.apply({ sequence: 2, name: 'Duplicate' }), false);
  assert.deepEqual(painted, [{ name: 'Installed input method', icon: null }]);
});

test('missing names hide the ambient indicator and later focus snapshots restore it', () => {
  const painted = [];
  const state = createInputSourceState(name => painted.push(name));
  state.apply({ sequence: 3, name: null });
  state.apply({ sequence: 4, name: 'ABC' });
  state.repaint();
  assert.deepEqual(painted, [
    { name: null, icon: null },
    { name: 'ABC', icon: null },
    { name: 'ABC', icon: null },
  ]);
  assert.equal(state.apply({ sequence: '5', name: 'Invalid' }), false);
});

test('system icon replaces the compact label without changing the full name', () => {
  const painted = [];
  const state = createInputSourceState(value => painted.push(value));
  state.apply({ sequence: 1, name: 'Pinyin – Simplified', icon: 'data:image/png;base64,cG5n' });
  state.apply({ sequence: 2, name: 'ABC', icon: null });
  assert.deepEqual(painted, [
    { name: 'Pinyin – Simplified', icon: 'data:image/png;base64,cG5n' },
    { name: 'ABC', icon: null },
  ]);
});
