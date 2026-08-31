// shortcuts.js — customizable, layout-independent app shortcut routing.
import { isComposingKeyEvent } from './pure.js';
import { DEFAULT_SHORTCUTS, SHORTCUT_ACTIONS, normalizeShortcutBinding } from './settings-model.js';

const handlers = new Map();
const FONT_ACTIONS = new Set(['fontIncrease', 'fontDecrease', 'fontReset']);
const MODIFIERS = ['Meta', 'Control', 'Alt', 'Shift'];

function eventCode(event) {
  const key = String(event?.key || '');
  if (key === '+' || key === '=') return 'Equal';
  if (key === '-') return 'Minus';
  if (/^[a-z]$/i.test(key)) return `Key${key.toUpperCase()}`;
  if (/^[0-9]$/.test(key)) return `Digit${key}`;
  const names = {
    ' ': 'Space', ArrowUp: 'ArrowUp', ArrowDown: 'ArrowDown', ArrowLeft: 'ArrowLeft', ArrowRight: 'ArrowRight',
    Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown', Enter: 'Enter', Tab: 'Tab',
    Backspace: 'Backspace', Delete: 'Delete', '[': 'BracketLeft', ']': 'BracketRight', '\\': 'Backslash',
    ';': 'Semicolon', "'": 'Quote', ',': 'Comma', '.': 'Period', '/': 'Slash', '`': 'Backquote',
  };
  return names[key] || String(event?.code || '');
}

export function shortcutFromEvent(event) {
  if (!event || event.type !== 'keydown' || isComposingKeyEvent(event)) return null;
  const code = eventCode(event);
  if (!code || /^(?:Meta|Control|Alt|Shift)(?:Left|Right)?$/.test(code)) return null;
  const parts = [];
  if (event.metaKey) parts.push('Meta');
  if (event.ctrlKey) parts.push('Control');
  if (event.altKey) parts.push('Alt');
  // Equal intentionally treats ⌘= and ⌘+ as one conventional zoom shortcut.
  if (event.shiftKey && code !== 'Equal') parts.push('Shift');
  parts.push(code);
  return normalizeShortcutBinding(parts.join('+'), null);
}

export function isSafeShortcut(binding) {
  if (!binding) return false;
  const parts = binding.split('+');
  return parts.length > 1 || /^F(?:[1-9]|1[0-2])$/.test(parts[0]);
}

export function shortcutMatches(event, binding) {
  return !!binding && shortcutFromEvent(event) === binding;
}

export function shortcutActionForEvent(event, bindings = DEFAULT_SHORTCUTS) {
  for (const action of SHORTCUT_ACTIONS) {
    if (shortcutMatches(event, bindings?.[action.id])) return action.id;
  }
  return null;
}

export function shortcutConflict(bindings, actionId, binding) {
  if (!binding) return null;
  return SHORTCUT_ACTIONS.find(action => action.id !== actionId && bindings?.[action.id] === binding)?.id || null;
}

export function formatShortcut(binding) {
  if (!binding) return '—';
  const labels = {
    Meta: '⌘', Control: '⌃', Alt: '⌥', Shift: '⇧', Equal: '+', Minus: '−',
    ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→',
    Space: 'Space', Backspace: '⌫', Delete: '⌦', Enter: '↩', Tab: '⇥',
  };
  return binding.split('+').map(part => labels[part] || part.replace(/^Key/, '').replace(/^Digit/, '')).join('');
}

export function registerShortcutAction(actionId, handler) {
  handlers.set(actionId, handler);
  return () => { if (handlers.get(actionId) === handler) handlers.delete(actionId); };
}

function modalOpen() {
  return ['cfm', 'ppd', 'settings-modal', 'tmux-lifecycle-modal']
    .some(id => document.getElementById(id)?.style.display === 'flex');
}

if (typeof document !== 'undefined') {
  document.addEventListener('keydown', event => {
    if (event.target?.closest?.('.shortcut-capture') || isComposingKeyEvent(event)) return;
    const action = shortcutActionForEvent(event, globalThis.settings?.shortcuts);
    const handler = action && handlers.get(action);
    if (!handler) return;
    const editable = event.target?.closest?.('input, textarea, select, [contenteditable="true"]');
    if ((modalOpen() || (editable && !event.target?.closest?.('#terminal'))) && !FONT_ACTIONS.has(action)) return;
    event.preventDefault();
    event.stopPropagation();
    Promise.resolve(handler(event)).catch(() => {});
  }, true);
}
