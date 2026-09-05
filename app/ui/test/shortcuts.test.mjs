import test from 'node:test';
import assert from 'node:assert/strict';
import {
  CUSTOMIZABLE_SHORTCUT_ACTIONS, DEFAULT_SHORTCUTS,
  normalizeFontScale, normalizeSettings, normalizeShortcutBinding,
} from '../js/settings-model.js';
import {
  formatShortcut, isSafeShortcut, shortcutActionForEvent, shortcutConflict, shortcutFromEvent,
} from '../js/shortcuts.js';
import { APP_BASE_FONT_SIZE, applyFontScale } from '../js/font-scale.js';

const key = (keyValue, extra = {}) => ({
  type: 'keydown', key: keyValue, code: '', metaKey: false, ctrlKey: false,
  altKey: false, shiftKey: false, isComposing: false, keyCode: 0, ...extra,
});

test('fixed macOS font shortcuts normalize and route on US and Japanese JIS keyboards', () => {
  const plus = key('+', { code: 'Equal', metaKey: true, shiftKey: true });
  const equal = key('=', { code: 'Equal', metaKey: true });
  const jisPlus = key(';', { code: 'Semicolon', metaKey: true, shiftKey: true });
  const minus = key('-', { code: 'Minus', metaKey: true });
  assert.equal(shortcutFromEvent(plus), 'Meta+Equal');
  assert.equal(shortcutFromEvent(equal), 'Meta+Equal');
  assert.equal(shortcutFromEvent(jisPlus), 'Meta+Equal');
  assert.equal(shortcutFromEvent(key('＋', { metaKey: true, shiftKey: true })), 'Meta+Equal');
  assert.equal(shortcutActionForEvent(plus, DEFAULT_SHORTCUTS), 'fontIncrease');
  assert.equal(shortcutActionForEvent(jisPlus, DEFAULT_SHORTCUTS), 'fontIncrease');
  assert.equal(shortcutActionForEvent(minus, DEFAULT_SHORTCUTS), 'fontDecrease');
  assert.equal(formatShortcut(DEFAULT_SHORTCUTS.fontIncrease), '⌘+');
});

test('Command++ still routes when a macOS IME masks the logical key', () => {
  const processPlus = key('Process', {
    code: 'Equal', keyCode: 229, metaKey: true, shiftKey: true, isComposing: true,
  });
  const unidentifiedPlus = key('Unidentified', {
    code: 'Equal', keyCode: 229, metaKey: true, shiftKey: true,
  });
  const legacyPlus = key('Process', {
    code: '', keyCode: 187, metaKey: true, shiftKey: true,
  });
  for (const event of [processPlus, unidentifiedPlus, legacyPlus]) {
    assert.equal(shortcutFromEvent(event), 'Meta+Equal');
    assert.equal(shortcutActionForEvent(event, DEFAULT_SHORTCUTS), 'fontIncrease');
  }
  assert.equal(shortcutFromEvent(key('Process', {
    code: 'Equal', keyCode: 229, shiftKey: true, isComposing: true,
  })), null, 'the same IME event without a command modifier remains text input');
});

test('a composing keydown reaches only the zoom actions, never sidebar or session chords', () => {
  // The WKWebView smoke (`ime-routing` bit 2) dispatches exactly this event
  // and expects the sidebar untouched.
  const composingB = key('b', { keyCode: 229, metaKey: true, isComposing: true });
  assert.equal(shortcutFromEvent(composingB), 'Meta+KeyB');
  assert.equal(shortcutActionForEvent(composingB, DEFAULT_SHORTCUTS), null);
  for (const [k, code] of [['n', 'KeyN'], ['d', 'KeyD']]) {
    assert.equal(shortcutActionForEvent(key(k, { code, keyCode: 229, metaKey: true }), DEFAULT_SHORTCUTS), null);
  }
  assert.equal(shortcutActionForEvent(key('=', { code: 'Equal', keyCode: 229, metaKey: true, isComposing: true }),
    DEFAULT_SHORTCUTS), 'fontIncrease');
  assert.equal(shortcutActionForEvent(key('b', { code: 'KeyB', metaKey: true }), DEFAULT_SHORTCUTS), 'toggleSidebar',
    'the ordinary chord is unaffected');
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
  assert.equal(shortcutConflict(bindings, 'newSession', 'Meta+Equal'), 'fontIncrease',
    'custom actions cannot shadow a fixed font shortcut');
  assert.equal(shortcutActionForEvent(key('K', { metaKey: true, shiftKey: true }), bindings), 'newSession');
  assert.deepEqual(CUSTOMIZABLE_SHORTCUT_ACTIONS.map(action => action.id), [
    'newSession', 'toggleSidebar', 'splitRight', 'splitDown',
  ]);
});

test('old settings gain bounded font and shortcut defaults while extensions round-trip', () => {
  const old = normalizeSettings({ future: { kept: 1 } });
  assert.equal(old.fontScale, 1);
  assert.deepEqual(old.shortcuts, DEFAULT_SHORTCUTS);
  const custom = normalizeSettings({
    fontScale: 1.34,
    shortcuts: {
      newSession: 'Meta+Shift+KeyK', fontIncrease: 'Alt+F8', futureAction: 'Alt+F8',
    },
  });
  assert.equal(custom.fontScale, 1.3);
  assert.equal(custom.shortcuts.newSession, 'Meta+Shift+KeyK');
  assert.equal(custom.shortcuts.fontIncrease, DEFAULT_SHORTCUTS.fontIncrease,
    'legacy font customization migrates back to the fixed cross-layout binding');
  assert.equal(custom.shortcuts.futureAction, 'Alt+F8');
  assert.equal(normalizeFontScale(99), 1);
  assert.equal(normalizeSettings({ fontScale: 0.5 }).fontScale, 0.5);
  assert.equal(normalizeSettings({ fontScale: 1.6 }).fontScale, 1.6);
});

test('font scale updates the rem root at the same bounded value used by xterm', () => {
  const style = { value: '', set fontSize(value) { this.value = value; }, get fontSize() { return this.value; } };
  const classes = new Set();
  const classList = { toggle(name, enabled) { enabled ? classes.add(name) : classes.delete(name); } };
  const documentRef = { documentElement: { style, classList } };
  assert.equal(applyFontScale(1.4, documentRef), 1.4);
  assert.equal(style.fontSize, `${APP_BASE_FONT_SIZE * 1.4}px`);
  assert.equal(classes.has('font-scale-large'), true);
  assert.equal(applyFontScale(1.3, documentRef), 1.3);
  assert.equal(classes.has('font-scale-large'), false);
  assert.equal(applyFontScale(9, documentRef), 1);
});
