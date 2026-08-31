import test from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_SHORTCUTS, normalizeFontScale, normalizeSettings, normalizeShortcutBinding,
} from '../js/settings-model.js';
import {
  formatShortcut, isSafeShortcut, shortcutActionForEvent, shortcutConflict, shortcutFromEvent,
} from '../js/shortcuts.js';
import { APP_BASE_FONT_SIZE, applyFontScale } from '../js/font-scale.js';

const key = (keyValue, extra = {}) => ({
  type: 'keydown', key: keyValue, code: '', metaKey: false, ctrlKey: false,
  altKey: false, shiftKey: false, isComposing: false, keyCode: 0, ...extra,
});

test('standard macOS font shortcuts normalize and route by customizable bindings', () => {
  const plus = key('+', { code: 'Equal', metaKey: true, shiftKey: true });
  const equal = key('=', { code: 'Equal', metaKey: true });
  const minus = key('-', { code: 'Minus', metaKey: true });
  assert.equal(shortcutFromEvent(plus), 'Meta+Equal');
  assert.equal(shortcutFromEvent(equal), 'Meta+Equal');
  assert.equal(shortcutActionForEvent(plus, DEFAULT_SHORTCUTS), 'fontIncrease');
  assert.equal(shortcutActionForEvent(minus, DEFAULT_SHORTCUTS), 'fontDecrease');
  assert.equal(formatShortcut(DEFAULT_SHORTCUTS.fontIncrease), '⌘+');
});

test('custom shortcut normalization is closed, safe and conflict-aware', () => {
  assert.equal(normalizeShortcutBinding('Meta+Shift+KeyK'), 'Meta+Shift+KeyK');
  assert.equal(normalizeShortcutBinding('Shift+Meta+KeyK', 'fallback'), 'fallback');
  assert.equal(normalizeShortcutBinding('Meta+Shift+Equal', 'fallback'), 'fallback');
  assert.equal(normalizeShortcutBinding('Meta+LaunchMissiles', 'fallback'), 'fallback');
  assert.equal(isSafeShortcut('KeyK'), false);
  assert.equal(isSafeShortcut('Meta+KeyK'), true);
  assert.equal(isSafeShortcut('F8'), true);
  const bindings = { ...DEFAULT_SHORTCUTS, newSession: 'Meta+Shift+KeyK' };
  assert.equal(shortcutConflict(bindings, 'splitRight', 'Meta+Shift+KeyK'), 'newSession');
  assert.equal(shortcutActionForEvent(key('K', { metaKey: true, shiftKey: true }), bindings), 'newSession');
});

test('old settings gain bounded font and shortcut defaults while extensions round-trip', () => {
  const old = normalizeSettings({ future: { kept: 1 } });
  assert.equal(old.fontScale, 1);
  assert.deepEqual(old.shortcuts, DEFAULT_SHORTCUTS);
  const custom = normalizeSettings({
    fontScale: 1.34,
    shortcuts: { newSession: 'Meta+Shift+KeyK', futureAction: 'Alt+F8' },
  });
  assert.equal(custom.fontScale, 1.3);
  assert.equal(custom.shortcuts.newSession, 'Meta+Shift+KeyK');
  assert.equal(custom.shortcuts.futureAction, 'Alt+F8');
  assert.equal(normalizeFontScale(99), 1);
  assert.equal(normalizeSettings({ fontScale: 0.8 }).fontScale, 0.8);
  assert.equal(normalizeSettings({ fontScale: 1.6 }).fontScale, 1.6);
});

test('font scale updates the rem root at the same bounded value used by xterm', () => {
  const style = { value: '', set fontSize(value) { this.value = value; }, get fontSize() { return this.value; } };
  const documentRef = { documentElement: { style } };
  assert.equal(applyFontScale(1.4, documentRef), 1.4);
  assert.equal(style.fontSize, `${APP_BASE_FONT_SIZE * 1.4}px`);
  assert.equal(applyFontScale(9, documentRef), 1);
});
