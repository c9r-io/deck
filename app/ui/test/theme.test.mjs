import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import {
  ACCENT_IDS, REQUIRED_CSS_TOKENS, REQUIRED_TERMINAL_TOKENS, THEME_IDS,
  activateTheme, onThemeChange, resolveTheme,
} from '../js/theme.js';
import { parseSettings, serializeSettings } from '../js/settings-model.js';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = path => readFileSync(resolve(root, path), 'utf8');

function rgb(hex) {
  const match = /^#([0-9a-f]{6})$/i.exec(hex);
  assert.ok(match, `expected opaque hex color, got ${hex}`);
  const n = Number.parseInt(match[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
function luminance(hex) {
  return rgb(hex).map(channel => {
    const c = channel / 255;
    return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  }).reduce((sum, value, i) => sum + value * [0.2126, 0.7152, 0.0722][i], 0);
}
function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

test('old settings gain safe theme defaults and unknown fields round-trip', () => {
  const old = parseSettings('{"editor":"Zed","future":{"kept":1}}');
  assert.equal(old.theme, 'deck-dark');
  assert.equal(old.accent, 'teal');
  assert.deepEqual(JSON.parse(serializeSettings(old)).future, { kept: 1 });
  assert.equal(parseSettings('{"theme":"unknown","accent":"red"}').theme, 'deck-dark');
  assert.equal(parseSettings('{"theme":"unknown","accent":"red"}').accent, 'teal');
});

test('every theme and accent resolves complete CSS, xterm and native palettes', () => {
  for (const theme of THEME_IDS) {
    for (const accent of ACCENT_IDS) {
      for (const prefersDark of [false, true]) {
        const palette = resolveTheme({ theme, accent }, prefersDark);
        for (const token of REQUIRED_CSS_TOKENS) assert.ok(palette.css[token], `${theme}/${accent} CSS ${token}`);
        for (const token of REQUIRED_TERMINAL_TOKENS) assert.ok(palette.terminal[token], `${theme}/${accent} terminal ${token}`);
        assert.match(palette.windowBackground, /^#[0-9a-f]{6}$/i);
        assert.ok(['light', 'dark'].includes(palette.colorScheme));
      }
    }
  }
});

test('reviewed palette combinations meet AA and high-contrast targets', () => {
  const semantic = ['text', 'muted', 'accent', 'run', 'wait', 'danger'];
  for (const theme of ['deck-dark', 'light', 'high-contrast']) {
    for (const accent of ACCENT_IDS) {
      const palette = resolveTheme({ theme, accent });
      const minimum = theme === 'high-contrast' ? 7 : 4.5;
      for (const token of semantic) {
        assert.ok(contrast(palette.css[token], palette.css.bg) >= minimum,
          `${theme}/${accent} ${token} on bg is below ${minimum}:1`);
      }
      assert.ok(contrast(palette.css.text, palette.css.panel) >= minimum, `${theme}/${accent} text on panel`);
      assert.ok(contrast(palette.css.accent, palette.css['accent-contrast']) >= 4.5,
        `${theme}/${accent} primary button contrast`);
      assert.ok(contrast(palette.terminal.foreground, palette.terminal.background) >= minimum,
        `${theme}/${accent} terminal foreground`);
      for (const ansi of [
        'red', 'brightRed', 'green', 'brightGreen', 'yellow', 'brightYellow',
        'blue', 'brightBlue', 'magenta', 'brightMagenta', 'cyan', 'brightCyan',
      ]) {
        assert.ok(contrast(palette.terminal[ansi], palette.terminal.background) >= 4.5,
          `${theme}/${accent} terminal ${ansi}`);
      }
    }
  }
});

test('system listens for appearance changes while fixed themes do not', () => {
  let handler = null;
  let added = 0;
  let removed = 0;
  const media = {
    matches: false,
    addEventListener(type, fn) { assert.equal(type, 'change'); handler = fn; added++; },
    removeEventListener(type, fn) { assert.equal(type, 'change'); assert.equal(fn, handler); removed++; },
  };
  const windowRef = { matchMedia: () => media };
  const values = new Map();
  const documentRef = { documentElement: {
    style: { setProperty: (key, value) => values.set(key, value) }, dataset: {},
  } };
  const seen = [];
  const off = onThemeChange(palette => seen.push(palette.effectiveTheme));
  assert.equal(activateTheme({ theme: 'system', accent: 'teal' }, { windowRef, documentRef }).effectiveTheme, 'light');
  assert.equal(added, 1);
  media.matches = true;
  handler({ matches: true });
  assert.equal(seen.at(-1), 'deck-dark');
  assert.equal(values.get('--bg'), '#101318');
  activateTheme({ theme: 'light', accent: 'blue' }, { windowRef, documentRef });
  assert.equal(removed, 1);
  const count = seen.length;
  media.matches = false;
  handler?.({ matches: false });
  assert.equal(seen.length, count, 'detached fixed theme ignores later media changes');
  off();
});

test('production CSS and all pane lifecycle paths consume the registry', () => {
  const html = read('app/ui/index.html');
  const layout = read('app/ui/js/layout.js');
  const board = read('app/ui/js/board.js');
  const app = read('app/ui/js/app.js');
  const frontendWithoutRegistry = [
    'app/ui/js/app.js', 'app/ui/js/board.js', 'app/ui/js/dialogs.js',
    'app/ui/js/layout.js', 'app/ui/js/scheduler.js', 'app/ui/js/selection.js',
    'app/ui/js/state.js', 'app/ui/js/terminal.js',
  ].map(read).join('\n');
  assert.doesNotMatch(html, /#[0-9a-f]{3,8}\b|rgba?\(/i, 'component CSS must not own palette literals');
  assert.doesNotMatch(frontendWithoutRegistry, /#[0-9a-f]{3,8}\b|rgba?\(/i,
    'dynamic component styles must not own palette literals');
  assert.doesNotMatch(board, /TERM_THEME/);
  assert.match(layout, /theme: getTerminalTheme\(\)/, 'new panes inherit the current palette');
  assert.match(layout, /onThemeChange\(\(\{ terminal \}\) => panes\.forEach/, 'open panes update in place');
  assert.match(layout, /pane\.term\.options\.theme = terminal/);
  assert.ok(app.indexOf('await loadSettings()') < app.indexOf('await revealThemedWindow()'),
    'typed settings and theme apply before the hidden window is revealed');
  assert.doesNotMatch(frontendWithoutRegistry, /localStorage|sessionStorage/,
    'theme boot does not copy settings or user data to a weaker store');
  const tauri = JSON.parse(read('app/src-tauri/tauri.conf.json'));
  assert.equal(tauri.app.windows[0].backgroundColor, resolveTheme({ theme: 'deck-dark' }).windowBackground,
    'native fallback matches the default registry palette');
});
