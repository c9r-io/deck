// theme.js — the single palette authority for app CSS, xterm and native chrome.
// Themes and accents are closed, reviewed presets; no user-provided colors enter
// this module. `system` resolves to the same light/dark palettes and only adds
// live prefers-color-scheme tracking.

export const THEME_IDS = Object.freeze(['deck-dark', 'light', 'system', 'high-contrast']);
export const ACCENT_IDS = Object.freeze(['teal', 'blue', 'purple', 'orange']);

export const REQUIRED_CSS_TOKENS = Object.freeze([
  'bg', 'panel', 'raised', 'hover', 'border', 'border-soft', 'text', 'muted',
  'faint', 'accent', 'accent-dim', 'accent-contrast', 'focus', 'run', 'wait',
  'stop', 'danger', 'danger-contrast', 'sel', 'run-glow', 'drop-target',
  'card-hover-border', 'waiting-border', 'waiting-border-hover', 'waiting-soft',
  'waiting-soft-border', 'pane-focus', 'scrollchip', 'scrollchip-hover',
  'file-drop', 'terminal-selection-overlay', 'input-separator', 'row-divider',
  'modal-overlay', 'shadow-menu', 'shadow-modal', 'shadow-toast', 'quickbar-bg',
]);

export const REQUIRED_TERMINAL_TOKENS = Object.freeze([
  'background', 'foreground', 'cursor', 'selectionBackground',
  'black', 'brightBlack', 'red', 'brightRed', 'green', 'brightGreen',
  'yellow', 'brightYellow', 'blue', 'brightBlue', 'magenta', 'brightMagenta',
  'cyan', 'brightCyan', 'white', 'brightWhite',
]);

const BASE_PALETTES = {
  'deck-dark': {
    colorScheme: 'dark',
    windowBackground: '#101318',
    css: {
      bg: '#101318', panel: '#171b22', raised: '#1e242e', hover: '#232b37',
      border: '#2a3140', 'border-soft': '#222834', text: '#dce3ec',
      muted: '#7e8a9a', faint: '#566072', run: '#41d392', wait: '#e8b45a',
      stop: '#566072', danger: '#e06c75', 'danger-contrast': '#1a0e10',
      'run-glow': 'rgba(65, 211, 146, 0.55)', 'drop-target': '#182028',
      'card-hover-border': '#3a4356', 'waiting-border': 'rgba(232, 180, 90, 0.45)',
      'waiting-border-hover': 'rgba(232, 180, 90, 0.70)',
      'waiting-soft': 'rgba(232, 180, 90, 0.12)',
      'waiting-soft-border': 'rgba(232, 180, 90, 0.35)', 'pane-focus': '#1a2530',
      'input-separator': 'rgba(126, 138, 153, 0.25)',
      'row-divider': 'rgba(34, 40, 52, 0.55)',
      'modal-overlay': 'rgba(8, 10, 13, 0.55)',
      'shadow-menu': 'rgba(0, 0, 0, 0.50)', 'shadow-modal': 'rgba(0, 0, 0, 0.55)',
      'shadow-toast': 'rgba(0, 0, 0, 0.45)', 'quickbar-bg': 'rgba(23, 27, 34, 0.96)',
    },
    terminal: {
      background: '#101318', foreground: '#dce3ec',
      black: '#171b22', brightBlack: '#566072',
      red: '#e06c75', brightRed: '#ff7b86', green: '#41d392', brightGreen: '#5ee6a8',
      yellow: '#e8b45a', brightYellow: '#ffd078', blue: '#6ca8ff', brightBlue: '#8dbbff',
      magenta: '#c678dd', brightMagenta: '#df91f3', cyan: '#4fd6be', brightCyan: '#70ead4',
      white: '#dce3ec', brightWhite: '#ffffff',
    },
  },
  light: {
    colorScheme: 'light',
    windowBackground: '#f7f8fa',
    css: {
      bg: '#f7f8fa', panel: '#ffffff', raised: '#eef1f5', hover: '#e4e9ef',
      border: '#c4cbd4', 'border-soft': '#d9dee5', text: '#18202a',
      muted: '#475467', faint: '#667085', run: '#067647', wait: '#8a5700',
      stop: '#667085', danger: '#b42318', 'danger-contrast': '#ffffff',
      'run-glow': 'rgba(6, 118, 71, 0.26)', 'drop-target': '#e4f3f0',
      'card-hover-border': '#98a2b3', 'waiting-border': 'rgba(138, 87, 0, 0.48)',
      'waiting-border-hover': 'rgba(138, 87, 0, 0.76)',
      'waiting-soft': 'rgba(138, 87, 0, 0.10)',
      'waiting-soft-border': 'rgba(138, 87, 0, 0.42)', 'pane-focus': '#e8f2f6',
      'input-separator': 'rgba(71, 84, 103, 0.30)',
      'row-divider': 'rgba(196, 203, 212, 0.72)',
      'modal-overlay': 'rgba(24, 32, 42, 0.34)',
      'shadow-menu': 'rgba(24, 32, 42, 0.20)', 'shadow-modal': 'rgba(24, 32, 42, 0.24)',
      'shadow-toast': 'rgba(24, 32, 42, 0.20)', 'quickbar-bg': 'rgba(255, 255, 255, 0.97)',
    },
    terminal: {
      background: '#f7f8fa', foreground: '#18202a',
      black: '#20252b', brightBlack: '#586372',
      red: '#b42318', brightRed: '#d92d20', green: '#067647', brightGreen: '#087f4d',
      yellow: '#8a5700', brightYellow: '#9c6500', blue: '#175cd3', brightBlue: '#175cd3',
      magenta: '#6938ef', brightMagenta: '#6938ef', cyan: '#087f70', brightCyan: '#087f70',
      white: '#d0d5dd', brightWhite: '#ffffff',
    },
  },
  'high-contrast': {
    colorScheme: 'dark',
    windowBackground: '#000000',
    css: {
      bg: '#000000', panel: '#080808', raised: '#121212', hover: '#252525',
      border: '#ffffff', 'border-soft': '#aeb8c4', text: '#ffffff',
      muted: '#e3e8ef', faint: '#c4ccd6', run: '#5cff9d', wait: '#ffd75f',
      stop: '#d7dde5', danger: '#ff7b86', 'danger-contrast': '#000000',
      'run-glow': 'rgba(92, 255, 157, 0.72)', 'drop-target': '#002d2b',
      'card-hover-border': '#ffffff', 'waiting-border': '#ffd75f',
      'waiting-border-hover': '#ffffff', 'waiting-soft': 'rgba(255, 215, 95, 0.18)',
      'waiting-soft-border': '#ffd75f', 'pane-focus': '#10252a',
      'input-separator': 'rgba(255, 255, 255, 0.62)', 'row-divider': '#aeb8c4',
      'modal-overlay': 'rgba(0, 0, 0, 0.78)', 'shadow-menu': 'rgba(255, 255, 255, 0.18)',
      'shadow-modal': 'rgba(255, 255, 255, 0.20)', 'shadow-toast': 'rgba(255, 255, 255, 0.18)',
      'quickbar-bg': '#080808',
    },
    terminal: {
      background: '#000000', foreground: '#ffffff',
      black: '#000000', brightBlack: '#c4ccd6',
      red: '#ff7b86', brightRed: '#ff9aa3', green: '#5cff9d', brightGreen: '#89ffb8',
      yellow: '#ffd75f', brightYellow: '#ffe58f', blue: '#80bfff', brightBlue: '#add6ff',
      magenta: '#e0aaff', brightMagenta: '#efccff', cyan: '#62f5e0', brightCyan: '#9affee',
      white: '#ffffff', brightWhite: '#ffffff',
    },
  },
};

// Accent groups are reviewed per effective palette. They own every accent-
// derived state together, including focus, selection and the xterm cursor.
export const ACCENT_PRESETS = Object.freeze({
  'deck-dark': {
    teal:   { accent: '#4fd6be', dim: '#2a6e62', contrast: '#0b201c', focus: '#7af0d8', sel: 'rgba(79, 214, 190, 0.10)', scrollchip: 'rgba(79, 214, 190, 0.12)', scrollchipHover: 'rgba(79, 214, 190, 0.22)', fileDrop: 'rgba(79, 214, 190, 0.13)', terminalSelection: 'rgba(79, 214, 190, 0.34)' },
    blue:   { accent: '#78a9ff', dim: '#365d91', contrast: '#0b1830', focus: '#a9c8ff', sel: 'rgba(120, 169, 255, 0.10)', scrollchip: 'rgba(120, 169, 255, 0.12)', scrollchipHover: 'rgba(120, 169, 255, 0.22)', fileDrop: 'rgba(120, 169, 255, 0.13)', terminalSelection: 'rgba(120, 169, 255, 0.34)' },
    purple: { accent: '#c49aff', dim: '#664c91', contrast: '#211033', focus: '#dfc4ff', sel: 'rgba(196, 154, 255, 0.10)', scrollchip: 'rgba(196, 154, 255, 0.12)', scrollchipHover: 'rgba(196, 154, 255, 0.22)', fileDrop: 'rgba(196, 154, 255, 0.13)', terminalSelection: 'rgba(196, 154, 255, 0.34)' },
    orange: { accent: '#ffb86b', dim: '#8a5b2e', contrast: '#281406', focus: '#ffd09c', sel: 'rgba(255, 184, 107, 0.10)', scrollchip: 'rgba(255, 184, 107, 0.12)', scrollchipHover: 'rgba(255, 184, 107, 0.22)', fileDrop: 'rgba(255, 184, 107, 0.13)', terminalSelection: 'rgba(255, 184, 107, 0.34)' },
  },
  light: {
    teal:   { accent: '#087f70', dim: '#63b7aa', contrast: '#ffffff', focus: '#006b5f', sel: 'rgba(8, 127, 112, 0.10)', scrollchip: 'rgba(8, 127, 112, 0.10)', scrollchipHover: 'rgba(8, 127, 112, 0.18)', fileDrop: 'rgba(8, 127, 112, 0.12)', terminalSelection: 'rgba(8, 127, 112, 0.28)' },
    blue:   { accent: '#175cd3', dim: '#84aef0', contrast: '#ffffff', focus: '#004eae', sel: 'rgba(23, 92, 211, 0.10)', scrollchip: 'rgba(23, 92, 211, 0.10)', scrollchipHover: 'rgba(23, 92, 211, 0.18)', fileDrop: 'rgba(23, 92, 211, 0.12)', terminalSelection: 'rgba(23, 92, 211, 0.28)' },
    purple: { accent: '#6938ef', dim: '#aa96ee', contrast: '#ffffff', focus: '#5425c9', sel: 'rgba(105, 56, 239, 0.10)', scrollchip: 'rgba(105, 56, 239, 0.10)', scrollchipHover: 'rgba(105, 56, 239, 0.18)', fileDrop: 'rgba(105, 56, 239, 0.12)', terminalSelection: 'rgba(105, 56, 239, 0.28)' },
    orange: { accent: '#9a4700', dim: '#d49a68', contrast: '#ffffff', focus: '#7a3700', sel: 'rgba(154, 71, 0, 0.10)', scrollchip: 'rgba(154, 71, 0, 0.10)', scrollchipHover: 'rgba(154, 71, 0, 0.18)', fileDrop: 'rgba(154, 71, 0, 0.12)', terminalSelection: 'rgba(154, 71, 0, 0.28)' },
  },
  'high-contrast': {
    teal:   { accent: '#62f5e0', dim: '#62f5e0', contrast: '#000000', focus: '#ffffff', sel: 'rgba(98, 245, 224, 0.20)', scrollchip: 'rgba(98, 245, 224, 0.18)', scrollchipHover: 'rgba(98, 245, 224, 0.30)', fileDrop: 'rgba(98, 245, 224, 0.22)', terminalSelection: 'rgba(98, 245, 224, 0.46)' },
    blue:   { accent: '#80bfff', dim: '#80bfff', contrast: '#000000', focus: '#ffffff', sel: 'rgba(128, 191, 255, 0.20)', scrollchip: 'rgba(128, 191, 255, 0.18)', scrollchipHover: 'rgba(128, 191, 255, 0.30)', fileDrop: 'rgba(128, 191, 255, 0.22)', terminalSelection: 'rgba(128, 191, 255, 0.46)' },
    purple: { accent: '#e0aaff', dim: '#e0aaff', contrast: '#000000', focus: '#ffffff', sel: 'rgba(224, 170, 255, 0.20)', scrollchip: 'rgba(224, 170, 255, 0.18)', scrollchipHover: 'rgba(224, 170, 255, 0.30)', fileDrop: 'rgba(224, 170, 255, 0.22)', terminalSelection: 'rgba(224, 170, 255, 0.46)' },
    orange: { accent: '#ffb86b', dim: '#ffb86b', contrast: '#000000', focus: '#ffffff', sel: 'rgba(255, 184, 107, 0.20)', scrollchip: 'rgba(255, 184, 107, 0.18)', scrollchipHover: 'rgba(255, 184, 107, 0.30)', fileDrop: 'rgba(255, 184, 107, 0.22)', terminalSelection: 'rgba(255, 184, 107, 0.46)' },
  },
});

export const THEME_REGISTRY = Object.freeze({
  'deck-dark': BASE_PALETTES['deck-dark'],
  light: BASE_PALETTES.light,
  system: Object.freeze({ colorScheme: 'light dark', variants: Object.freeze({ dark: 'deck-dark', light: 'light' }) }),
  'high-contrast': BASE_PALETTES['high-contrast'],
});

function safeTheme(value) { return THEME_IDS.includes(value) ? value : 'deck-dark'; }
function safeAccent(value) { return ACCENT_IDS.includes(value) ? value : 'teal'; }

export function resolveTheme(settings = {}, prefersDark = true) {
  const requestedTheme = safeTheme(settings.theme);
  const accentId = safeAccent(settings.accent);
  const effectiveTheme = requestedTheme === 'system' ? (prefersDark ? 'deck-dark' : 'light') : requestedTheme;
  const base = BASE_PALETTES[effectiveTheme];
  const accent = ACCENT_PRESETS[effectiveTheme][accentId];
  const css = {
    ...base.css,
    accent: accent.accent, 'accent-dim': accent.dim, 'accent-contrast': accent.contrast,
    focus: accent.focus, sel: accent.sel, scrollchip: accent.scrollchip,
    'scrollchip-hover': accent.scrollchipHover, 'file-drop': accent.fileDrop,
    'terminal-selection-overlay': accent.terminalSelection,
  };
  const terminal = {
    ...base.terminal,
    cursor: accent.accent,
    selectionBackground: accent.terminalSelection,
  };
  return Object.freeze({
    requestedTheme, effectiveTheme, accent: accentId, colorScheme: base.colorScheme,
    windowBackground: base.windowBackground, css: Object.freeze(css), terminal: Object.freeze(terminal),
  });
}

let activeSettings = Object.freeze({ theme: 'deck-dark', accent: 'teal' });
let current = resolveTheme(activeSettings, true);
let mediaQuery = null;
let mediaHandler = null;
const listeners = new Set();

function prefersDark(windowRef) {
  try { return windowRef?.matchMedia?.('(prefers-color-scheme: dark)').matches !== false; }
  catch (_) { return true; }
}

function syncNativeBackground(windowRef, color) {
  try {
    const nativeWindow = windowRef?.__TAURI__?.window?.getCurrentWindow?.();
    if (nativeWindow?.setBackgroundColor) Promise.resolve(nativeWindow.setBackgroundColor(color)).catch(() => {});
  } catch (_) { /* browser/test environment */ }
}

export function syncThemeIntegrations(windowRef = globalThis.window) {
  syncNativeBackground(windowRef, current.windowBackground);
  try {
    const invoke = windowRef?.__TAURI__?.core?.invoke;
    if (invoke) Promise.resolve(invoke('set_terminal_mode_style', {
      foreground: current.css['accent-contrast'], background: current.css.accent,
    })).catch(() => {});
  } catch (_) { /* no tmux server yet or browser/test environment */ }
}

export function applyTheme(settings = activeSettings, options = {}) {
  const documentRef = options.documentRef ?? globalThis.document;
  const windowRef = options.windowRef ?? globalThis.window;
  current = resolveTheme(settings, options.prefersDark ?? prefersDark(windowRef));
  const root = documentRef?.documentElement;
  if (root?.style) {
    for (const [token, value] of Object.entries(current.css)) root.style.setProperty(`--${token}`, value);
    root.style.colorScheme = current.colorScheme;
  }
  if (root?.dataset) {
    root.dataset.theme = current.requestedTheme;
    root.dataset.effectiveTheme = current.effectiveTheme;
    root.dataset.accent = current.accent;
  }
  syncThemeIntegrations(windowRef);
  listeners.forEach(listener => listener(current));
  return current;
}

function removeSystemListener() {
  if (!mediaQuery || !mediaHandler) return;
  if (mediaQuery.removeEventListener) mediaQuery.removeEventListener('change', mediaHandler);
  else mediaQuery.removeListener?.(mediaHandler);
  mediaQuery = null;
  mediaHandler = null;
}

export function activateTheme(settings = {}, options = {}) {
  const windowRef = options.windowRef ?? globalThis.window;
  activeSettings = Object.freeze({ theme: safeTheme(settings.theme), accent: safeAccent(settings.accent) });
  removeSystemListener();
  const result = applyTheme(activeSettings, { ...options, windowRef });
  if (activeSettings.theme === 'system' && windowRef?.matchMedia) {
    mediaQuery = windowRef.matchMedia('(prefers-color-scheme: dark)');
    const query = mediaQuery;
    mediaHandler = () => {
      // A change event already queued by WebKit may arrive after switching to
      // a fixed theme. Ignore that stale callback as well as detaching it.
      if (activeSettings.theme !== 'system' || mediaQuery !== query) return;
      applyTheme(activeSettings, { ...options, windowRef, prefersDark: query.matches });
    };
    if (mediaQuery.addEventListener) mediaQuery.addEventListener('change', mediaHandler);
    else mediaQuery.addListener?.(mediaHandler);
  }
  return result;
}

export function getTerminalTheme() { return current.terminal; }
export function getResolvedTheme() { return current; }
export function onThemeChange(listener) { listeners.add(listener); return () => listeners.delete(listener); }

export async function revealThemedWindow(windowRef = globalThis.window) {
  try {
    const nativeWindow = windowRef?.__TAURI__?.window?.getCurrentWindow?.();
    if (!nativeWindow) return;
    await nativeWindow.show();
    await nativeWindow.setFocus();
  } catch (_) { /* plain browser or a closing window */ }
}
