// font-scale.js — one persistent scale for app text and every xterm pane.
import { normalizeFontScale } from './settings-model.js';

export const APP_BASE_FONT_SIZE = 13;
export const TERMINAL_BASE_FONT_SIZE = 12.5;

let current = 1;
const listeners = new Set();

export function applyFontScale(value, documentRef = globalThis.document) {
  current = normalizeFontScale(value);
  const root = documentRef?.documentElement;
  if (root?.style) {
    root.style.fontSize = `${APP_BASE_FONT_SIZE * current}px`;
    root.classList?.toggle('font-scale-large', current >= 1.4);
  }
  listeners.forEach(listener => listener(current));
  return current;
}

export function getFontScale() { return current; }
export function onFontScaleChange(listener) { listeners.add(listener); return () => listeners.delete(listener); }
