// Local Translation WKWebView smoke carriers, run unattended by
// scripts/translation-lens-verify.py inside an isolated debug bundle.
//
// Input is native wherever the case is about input: clicks, drags, wheel,
// keys and focus go through AppKit into Deck's own window
// (`smoke_native_input`, native/SmokeBridge.swift), so the DOM receives
// trusted events on the production listeners. DOM-driven steps are named
// `dom` in their check. Evidence never contains translated or copied text:
// checks carry only closed names and numbers.
//
// `translation` (L2): real WKWebView, xterm, tmux, Lens UI and IPC with a
//   CONTROLLED provider and a controlled clipboard gate (fault/late-response
//   injection). Settings, pack install and delete are the product paths.
// `translation-native` (L3): the production translation path end to end —
//   real Bergamot model from the isolated pack, real AppKit pasteboard gate
//   and focus checks, real terminal content from the /copy CLI fixture.
//   Boundary/race cases use a test-owned NAMED pasteboard through the same
//   native gate; the general pasteboard is used under a guard that keeps the
//   user's items in memory and restores them only while the change is ours.
import { $, ctx, inv, state } from '../js/state.js';
import { panes, provider, render, openBuffer } from '../js/board.js';
import { addSplit, backToBoard, focusPane, goLive, openSession } from '../js/layout.js';
import { t, setLocale } from '../js/i18n.js';
import { activateTheme } from '../js/theme.js';
import { applyFontScale } from '../js/font-scale.js';
import { serializeSettings } from '../js/settings-model.js';
import { installTranslationSmokeBackend, packStatus, capability } from '../js/local-intelligence.js';
import { closeTranslationLens, translationLensMetrics, translationLensView, visibleViewport } from '../js/translation-lens.js';
import { withGuard, GuardRefused } from './translation-guard.mjs';

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
let failed = false;
async function waitFor(test, timeout = 8000, step = 25) {
  const end = performance.now() + timeout;
  while (performance.now() < end) { if (await test()) return true; await pause(step); }
  return !!(await test());
}
const report = (name, ok, a = 1, b = 0) => {
  if (!ok) failed = true;
  return inv('ui_event', { code: 'smoke-check', detail: name,
    a: ok ? Math.max(1, Math.trunc(a || 1)) : -Math.max(1, Math.abs(Math.trunc(a || 1))), b: Math.trunc(b || 0) });
};
const metric = (name, a = 0, b = 0) => inv('ui_event', { code: 'smoke-check', detail: name,
  a: Math.max(0, Math.trunc(a)), b: Math.trunc(b) });

/* ----- native input into Deck's own window ----- */
const pb = (action, extra = {}) => inv('smoke_pasteboard', { action, ...extra });
const native = (kind, args = {}) => inv('smoke_native_input', { input: { kind, viewport: window.innerHeight, ...args } });
const center = el => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
async function click(el, point = center(el)) {
  await inputReady();
  await native('down', point); await pause(30); await native('up', point); await pause(60);
}
async function drag(from, to, steps = 6) {
  await native('down', from);
  for (let i = 1; i <= steps; i++) {
    await native('drag', { x: from.x + (to.x - from.x) * i / steps, y: from.y + (to.y - from.y) * i / steps });
    await pause(20);
  }
  await native('up', to); await pause(80);
}
// Focus a terminal through its pane header's empty spacer (mousedown ->
// focusPane -> term.focus), never by clicking terminal text: Deck opens
// terminal links and paths on click, and the fixture's text contains a URL.
// The header click's default action moves DOM focus to the header, so the
// keyboard target is then given to that pane's terminal with xterm's public
// focus() (a DOM-level step; the keystrokes that follow stay native).
async function clickTerm(el) {
  const spacer = el.closest?.('.spane')?.querySelector('.spane-head .hspace')
    || el.querySelector?.('.spane-head .hspace');
  await click(spacer || el);
  panes.get(ctx.attachedName)?.term.focus(); await pause(30);
}
const wheelAt = (el, dy) => native('scroll', { ...center(el), dy });
const KEY_CODES = { a: 0, s: 1, d: 2, f: 3, h: 4, g: 5, z: 6, x: 7, c: 8, v: 9, b: 11, q: 12, w: 13, e: 14, r: 15,
  y: 16, t: 17, 1: 18, 2: 19, 3: 20, 4: 21, 6: 22, 5: 23, '=': 24, 9: 25, 7: 26, '-': 27, 8: 28, 0: 29, ']': 30,
  o: 31, u: 32, '[': 33, i: 34, p: 35, l: 37, j: 38, "'": 39, k: 40, ';': 41, '\\': 42, ',': 43, '/': 44, n: 45,
  m: 46, '.': 47, ' ': 49, '`': 50 };
const SHIFTED = { _: '-', '!': '1', '@': '2', '#': '3', $: '4', '%': '5', '^': '6', '&': '7', '*': '8', '(': '9',
  ')': '0', '+': '=', '{': '[', '}': ']', '|': '\\', ':': ';', '"': "'", '<': ',', '>': '.', '?': '/', '~': '`' };
const SPECIAL = { Enter: [36, '\r'], Tab: [48, '\t'], Escape: [53, '\x1b'], ArrowLeft: [123, ''],
  ArrowRight: [124, ''], ArrowDown: [125, ''], ArrowUp: [126, ''], Home: [115, ''],
  End: [119, ''], PageDown: [121, ''] };
async function key(name, modifiers = []) {
  if (SPECIAL[name]) { const [keyCode, text] = SPECIAL[name]; await native('key', { keyCode, text, modifiers }); }
  else {
    const base = SHIFTED[name];
    const lower = base || name.toLowerCase();
    if (!(lower in KEY_CODES)) throw new Error('unsupported key');
    if (base) { await native('key', { keyCode: KEY_CODES[base], text: name, modifiers: [...modifiers, 'shift'] }); await pause(8); return; }
    const shifted = name !== lower || modifiers.includes('shift');
    await native('key', { keyCode: KEY_CODES[lower], text: shifted ? name.toUpperCase() : name,
      modifiers: shifted && !modifiers.includes('shift') ? [...modifiers, 'shift'] : modifiers });
  }
  await pause(8);
}
async function type(text) { await inputReady(); for (const ch of text) await key(ch); }
const chord = () => key('t', ['command', 'shift']);
const snapshot = name => inv('smoke_native_snapshot', { name }).catch(() => -9);
// Native app state: bit 1 active, bit 2 key window (document.hasFocus() is
// not the app-in-front fact; see translation-lens.js).
const front = async () => ((await native('state')) & 3) === 3;
const away = async () => ((await native('state')) & 1) === 0;
// A deliberate hide (A06, C04, F06) suspends the automatic regain until return.
const hideDeck = async () => { deliberateAway = true; return native('hide'); };
const backFront = async timeout => { const ok = await waitFor(front, timeout, 100); deliberateAway = false; return ok; };
// Another app can take focus mid-run (the Mac is shared with its user).
// Input is delivered only while this window is in front; every regain is
// counted and reported (`tl-front-regains`), never hidden.
let frontRegains = 0, deliberateAway = false;
async function ensureFront() {
  await native('roman'); // this window only: keystrokes are never IME-composed
  if (await front()) return true;
  frontRegains++;
  await native('activate');
  return waitFor(front, 20000, 100);
}
const inputReady = async () => { if (!deliberateAway && !(await front())) await ensureFront(); };
// Trusted/untrusted census of the listener-relevant events.
const census = {};
for (const type of ['wheel', 'pointerdown', 'pointerup', 'mousedown', 'mouseup', 'click', 'keydown', 'copy']) {
  window.addEventListener(type, event => {
    const slot = census[type] ||= { trusted: 0, untrusted: 0 };
    slot[event.isTrusted ? 'trusted' : 'untrusted']++;
  }, true);
}
let blurCount = 0;
window.addEventListener('blur', () => { blurCount++; });
const trusted = type => census[type]?.trusted || 0;
const untrusted = type => census[type]?.untrusted || 0;
const counters = { changed: 0, scroll: 0 };
window.addEventListener('deck-terminal-changed', () => counters.changed++);
window.addEventListener('deck-terminal-scroll', () => counters.scroll++);

const status = () => $('translation-status').textContent;
const STATUS_KEYS = ['translation.ready', 'translation.updating', 'translation.stale', 'translation.held',
  'translation.translating', 'translation.preparing', 'translation.empty', 'translation.waiting',
  'translation.background', 'translation.backgroundEmpty', 'translation.selected'];
const statusCode = () => STATUS_KEYS.findIndex(key => status() === t(key)) + 1 || (status().length ? 99 : 0);
const resultText = () => $('translation-result').textContent;
const isStatus = key => status() === t(key);
const lensMetrics = () => translationLensMetrics();
const lensSourceText = () => translationLensView().source;
// First occurrence of a Lens stamp at or after `since` (performance.now()).
const firstAfter = (name, since) => (lensMetrics().history[name] || []).find(at => at >= since) ?? NaN;
const tab = mode => $(`translation-tab-${mode}`);
async function quietTerminal(ms = 600, timeout = 8000) {
  const end = performance.now() + timeout;
  let seen = counters.changed, since = performance.now();
  while (performance.now() < end) {
    await pause(50);
    if (counters.changed !== seen) { seen = counters.changed; since = performance.now(); }
    else if (performance.now() - since >= ms) return true;
  }
  return false;
}
const viewportHas = (pane, text) => visibleViewport(pane).includes(text);
async function saveTranslationSettings(patch) {
  ctx.settings.localIntelligence.translation = { ...ctx.settings.localIntelligence.translation, ...patch };
  await inv('save_settings', { data: serializeSettings(ctx.settings) });
}
async function newCard(title, cmd) {
  const project = provider.projects()[0];
  const { card } = await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
    title, cmd, dir: '/tmp' });
  render(); return card;
}
// Focus a pane and leave tmux copy-mode so typing reaches its program.
// card.scrolled can lag the real copy-mode state until the next poll, so the
// product's own back-to-live action (what the scrollback chip runs) is called
// unconditionally — a DOM-level call, idempotent at the live view.
async function toLive(pane, card) {
  await clickTerm(pane.term.element);
  await goLive(card.session); await pause(150);
}
// Leave tmux copy-mode only when the focused card is actually scrolled. Typing
// in a scrolled pane returns it to live AND forwards the key, so the key is
// Enter: an empty line is harmless to the fixture and to a shell.
async function leaveHistory() {
  const card = provider.get(state.sessionId);
  if (!card?.scrolled) return;
  await key('Enter'); await waitFor(() => !provider.get(state.sessionId)?.scrolled, 3000);
}
const shellQuote = text => `'${text.replaceAll("'", "'\\''")}'`;
const lagProbe = () => {
  let max = 0, last = performance.now(), stop = false;
  (function loop() { if (stop) return; const now = performance.now(); max = Math.max(max, now - last - 20);
    last = now; setTimeout(loop, 20); })();
  return { done() { stop = true; return Math.round(max); } };
};

/* ===== L2: controlled provider ===== */
function controlledBackend() {
  const fake = { requests: [], outstanding: 0, maxOutstanding: 0, cancelled: 0, auto: null, autoDelay: 120,
    failNext: [], clipboard: '', version: 0, baseline: 0, armed: false, arms: 0, reads: 0, pollDelay: 0,
    installed: false, copied: '', unloads: 0 };
  const pack = () => ({ installed: fake.installed, corrupt: false, downloadBytes: 36745493, installedBytes: 49913927,
    liveBytes: 4096, selectionBytes: 16384, documentChoices: [8192, 16384], targetLanguage: 'zh-Hans' });
  const settle = (item, text, code) => {
    if (item.done) return; item.done = true; fake.outstanding--;
    if (text) item.resolve({ requestId: item.id, text, sourceLanguage: 'en', targetLanguage: 'zh-Hans' });
    else item.reject(new Error(code));
  };
  fake.settle = settle;
  fake.pending = mode => fake.requests.find(item => !item.done && (!mode || item.strategy === mode));
  installTranslationSmokeBackend({
    capability: async () => ({ available: fake.installed && ctx.settings.localIntelligence.translation.enabled,
      enabled: ctx.settings.localIntelligence.translation.enabled, installed: fake.installed, loaded: false }),
    packStatus: async () => pack(),
    packInstall: async () => { fake.installed = true; return pack(); },
    packDelete: async () => { fake.installed = false; },
    unload: async () => { fake.unloads++; },
    translate: (id, text, target, strategy) => new Promise((resolve, reject) => {
      const item = { id, text, strategy, resolve, reject, done: false, at: performance.now() };
      fake.requests.push(item); fake.outstanding++;
      fake.maxOutstanding = Math.max(fake.maxOutstanding, fake.outstanding);
      if (fake.failNext.length) { const code = fake.failNext.shift(); setTimeout(() => settle(item, null, code), 60); }
      else if (fake.auto) setTimeout(() => settle(item, fake.auto(text)), fake.autoDelay);
    }),
    cancel: async () => { fake.cancelled++; }, // like native: authority only, the call still settles
    arm: async () => {
      fake.armed = true; fake.baseline = fake.version; fake.arms++; },
    disarm: async () => { fake.armed = false; },
    // Same contract as the native gate: only the receipted Lens write is excluded.
    poll: async () => {
      if (!fake.armed || fake.version <= fake.baseline) return null;
      fake.baseline = fake.version;
      const own = fake.own === fake.version; fake.own = null;
      if (own) return null;
      fake.reads++;
      const text = fake.clipboard;
      if (fake.pollDelay) await pause(fake.pollDelay);
      return text;
    },
    copy: async text => { fake.copied = text; fake.clipboard = text; fake.version++;
      if (fake.armed) fake.own = fake.version; return fake.version; },
  });
  fake.copyExternally = text => { fake.clipboard = text; fake.version++; };
  return fake;
}
const lastLine = text => text.trim().split('\n').at(-1).slice(0, 40);
const fakeTranslation = text => `译文〔${lastLine(text)}〕\n第二行\n第三行`;

export async function runTranslationSmoke() {
  const fake = controlledBackend();
  let stage = 0;
  try {
    const fixture = await inv('smoke_native_fixture');
    await waitFor(() => provider.projects().length > 0);
    stage = 1;
    const cardA = await newCard('translation smoke', `python3 ${shellQuote(fixture)}`);
    await openSession(cardA.id);
    const paneA = panes.get(cardA.session);
    await ensureFront();
    await waitFor(() => viewportHas(paneA, 'The build completed successfully'), 15000);
    await report('translation-default-off', $('translation-btn').hidden && $('translation-panel').hidden);
    await chord(); await pause(150);
    await report('translation-disabled-shortcut', $('translation-panel').hidden, 1, trusted('keydown'));
    stage = 2;
    const { openSettings } = await import('../js/settings.js');
    await openSettings({ section: 'terminal' });
    await waitFor(() => $('set-translation-pack-status').textContent.length > 0);
    $('set-local-translation').click();
    await report('translation-enable-confirm', await waitFor(() => $('cfm').style.display === 'flex')
      && !fake.installed && !ctx.settings.localIntelligence.translation.enabled);
    $('cfm-no').click();
    await report('translation-enable-cancel', await waitFor(() => !$('set-local-translation').checked)
      && !fake.installed && !ctx.settings.localIntelligence.translation.enabled);
    $('set-local-translation').click();
    await waitFor(() => $('cfm').style.display === 'flex');
    $('cfm-yes').click();
    await report('translation-download-enable', await waitFor(() => fake.installed && ctx.settings.localIntelligence.translation.enabled));
    const persisted = await inv('load_settings').catch(() => null);
    let saved = null; try { saved = JSON.parse(persisted?.data || 'null'); } catch { saved = null; }
    const savedEnabled = saved?.localIntelligence?.translation?.enabled ?? saved?.data?.localIntelligence?.translation?.enabled;
    await report('tl-e02-persisted', savedEnabled === true);
    $('set-close').click();
    await report('translation-enabled', await waitFor(() => !$('translation-btn').hidden));
    await report('tl-e03-removed', !document.getElementById('translation-resume')
      && !document.getElementById('translation-use-current') && !document.getElementById('translation-mode')
      && !!tab('live') && !!tab('clipboard'));
    await ensureFront();
    await chord(); await report('translation-shortcut-live', await waitFor(() => !$('translation-panel').hidden));
    await chord(); await report('translation-shortcut-close', await waitFor(() => $('translation-panel').hidden));

    /* A01: overlay layout, static output, button, nothing written afterwards */
    stage = 3;
    $('session-workspace').style.maxWidth = '750px';
    await quietTerminal(800);
    const changedBefore = counters.changed, scrollBefore = counters.scroll;
    const clickAt = performance.now();
    await click($('translation-btn'));
    await report('translation-open', await waitFor(() => !$('translation-panel').hidden));
    await report('translation-overlay', $('session-workspace').classList.contains('translation-overlay'));
    const first = await waitFor(() => fake.pending('live'), 3000) && fake.pending('live');
    await report('tl-a01-static-open', !!first && first.text.includes('The build completed successfully'),
      first ? first.at - clickAt : 1, trusted('pointerdown'));
    await report('tl-a01-no-output', counters.changed === changedBefore && counters.scroll === scrollBefore,
      1, counters.changed - changedBefore);
    // A context reset (e.g. the pane re-created during early attach) may
    // supersede a request; every live request the Lens issues is answered.
    const answerLive = text => waitFor(() => {
      for (const item of fake.requests.filter(r => !r.done && r.strategy === 'live')) fake.settle(item, text);
      return resultText() === text && isStatus('translation.ready');
    }, 8000, 50);
    await report('tl-a01-display', await answerLive('静态译文'),
      fake.requests.filter(r => r.at >= clickAt).length, statusCode() * 100 + fake.requests.filter(r => !r.done).length * 10);
    await snapshot('l2-a01-overlay-live');
    await click($('translation-close'));
    await report('translation-close', await waitFor(() => $('translation-panel').hidden) && !resultText());
    $('session-workspace').style.maxWidth = '';
    await quietTerminal(800);

    /* A02: shortcut, then a blank-area click and Tab before the first frame */
    await chord();
    const blank = $('translation-result'); await pause(5);
    await click(blank); await key('Tab');
    const second = await waitFor(() => fake.pending('live'), 3000) && fake.pending('live');
    await report('tl-a02-interact', !!second && !status().includes('暂停') && !/paused/i.test(status()),
      1, trusted('pointerdown'));
    const dock = $('translation-panel').getBoundingClientRect(), terminal = $('terminal-host').getBoundingClientRect();
    await report('translation-dock', !$('session-workspace').classList.contains('translation-overlay')
      && terminal.right <= dock.left + 1);
    await report('tl-a02-display', await answerLive('快捷键译文'),
      lensMetrics().submitted, statusCode() * 100 + fake.requests.filter(r => !r.done).length);
    fake.auto = fakeTranslation;
    for (const item of fake.requests.filter(r => !r.done)) fake.settle(item, fakeTranslation(item.text));

    /* B06: identical text → no DOM rewrite (dom-driven events + real layout changes) */
    await quietTerminal(800); await pause(700);
    const writes = lensMetrics().domWrites, submittedB6 = lensMetrics().submitted;
    for (let i = 0; i < 20; i++) window.dispatchEvent(new CustomEvent('deck-terminal-changed', { detail: cardA.session }));
    await pause(900);
    await report('tl-b06-no-rewrite', lensMetrics().domWrites === writes && lensMetrics().submitted === submittedB6,
      1, lensMetrics().domWrites - writes);

    /* B05: reading never pauses; selection holds display; ⌘C copies the selection */
    stage = 4;
    const scrollSignals = counters.scroll, beforeWheel = lensMetrics().submitted;
    for (let i = 0; i < 10; i++) { await wheelAt($('translation-result'), -20); await pause(16); }
    await pause(500);
    await report('tl-b05-result-wheel', counters.scroll === scrollSignals && lensMetrics().submitted === beforeWheel,
      trusted('wheel'), counters.scroll - scrollSignals);
    await click($('translation-result'));
    await clickTerm($('terminal-host')); await type('/next'); await key('Enter');
    await report('tl-b05-click-no-pause', await waitFor(() => resultText().includes('explains how to restart')
      || fake.requests.some(r => r.done && r.text.includes('restart the server')), 5000)
      && await waitFor(() => isStatus('translation.ready'), 5000));
    // L2 never writes the general pasteboard: Cmd+C in the result is a
    // Lens-owned write, which this mode's controlled backend receives.
    const generalBefore = await pb('count', { board: 0 });
    await clickTerm($('terminal-host')); await type('/stream 25'); await key('Enter');
    await waitFor(() => fake.requests.some(r => r.text.includes('Stream line')), 4000);
    const text = $('translation-result').getBoundingClientRect();
    const start = { x: text.left + 16, y: text.top + 20 }, end = { x: text.left + 200, y: text.top + 60 };
    await drag(start, end);
    const shownAtSelect = resultText();
    const selected = String(document.getSelection());
    await waitFor(() => lensMetrics().status === 'translation.held', 5000);
    await report('tl-b05-select-hold', resultText() === shownAtSelect && selected.includes('\n')
      && document.activeElement === $('translation-result'), selected.length, isStatus('translation.held') ? 1 : 0);
    const selfCopies = fake.requests.length;
    await key('c', ['command']);
    await report('tl-b05-cmd-c', await waitFor(() => fake.copied === selected, 2000)
      && document.getSelection().toString() === selected, trusted('copy'), fake.requests.length - selfCopies);
    await key('Tab');
    await report('tl-b05-release', await waitFor(() => resultText() !== shownAtSelect, 5000), 1, lensMetrics().domWrites);
    await click($('translation-result')); await key('PageDown'); await key('ArrowDown');
    await quietTerminal(800);
    await report('tl-b05-keyboard', await waitFor(() => isStatus('translation.ready') || isStatus('translation.updating'), 4000)
      && !isStatus('translation.held'));
    await report('tl-e04-l2-no-general', await pb('count', { board: 0 }) === generalBefore);
    /* G05 (L2): a real copy of exactly the bytes the Lens just copied is new input */
    await click(tab('clipboard')); await waitFor(() => isStatus('translation.waiting'), 3000);
    fake.copyExternally(selected);
    await report('tl-g05-same-bytes', await waitFor(() => fake.requests.some(r => r.strategy === 'clipboard' && r.text === selected), 3000));
    await click(tab('live')); await quietTerminal(600);

    /* B01: 5 s continuous output */
    stage = 5;
    await clickTerm($('terminal-host'));
    const b1 = lensMetrics().submitted, b1Writes = lensMetrics().domWrites, streamAt = performance.now();
    await type('/stream 50'); await key('Enter');
    await waitFor(() => viewportHas(paneA, 'Stream line 050'), 12000, 50);
    const streamMs = performance.now() - streamAt;
    const b1Submitted = lensMetrics().submitted - b1;
    await report('tl-b01-bounded', b1Submitted >= 3 && b1Submitted <= Math.ceil(streamMs / 650) + 3, b1Submitted, Math.round(streamMs));
    await report('tl-b01-readable', lensMetrics().domWrites - b1Writes >= 2, lensMetrics().domWrites - b1Writes);
    await report('tl-b01-tail', await waitFor(() => lensSourceText().includes('Stream line 050') && isStatus('translation.ready'), 4000));

    /* B02/B04 tmux route: 5 s of native wheel over the terminal */
    const b2Start = lensMetrics().submitted, b2Wheel = trusted('wheel'), b2Signals = counters.scroll;
    const inflightAtStart = lensMetrics().inflight;
    let lastWheel = 0;
    for (let i = 0, t0 = performance.now(); performance.now() - t0 < 5000; i++) {
      await wheelAt($('terminal-host'), i % 120 < 60 ? 40 : -40); lastWheel = performance.now(); await pause(12);
    }
    const duringBurst = lensMetrics().submitted - b2Start;
    await report('tl-b02-no-intermediate', duringBurst <= inflightAtStart, trusted('wheel') - b2Wheel, duringBurst);
    await report('tl-b04-tmux-route', counters.scroll - b2Signals > 50, counters.scroll - b2Signals);
    await waitFor(() => lensMetrics().submitted > b2Start + duringBurst, 3000, 10);
    const settleMs = lensMetrics().last.submit - lastWheel;
    const finalView = visibleViewport(paneA);
    const finalReq = fake.requests.at(-1);
    await report('tl-b02-final', !!finalReq && finalReq.text === finalView && lensMetrics().submitted - b2Start - duringBurst === 1,
      Math.round(settleMs), lensMetrics().submitted - b2Start);
    await metric('tl-t-l2-scroll-submit', settleMs);
    await leaveHistory(); await quietTerminal(600);

    /* B03: output + scroll overlap; scroll then mode switch; scroll then pane switch */
    stage = 6;
    await clickTerm($('terminal-host')); await type('/stream 40'); await key('Enter');
    await pause(700);
    const b3 = lensMetrics().submitted; let b3Last = 0;
    for (let t0 = performance.now(); performance.now() - t0 < 1500;) {
      await wheelAt($('terminal-host'), 30); b3Last = performance.now(); await pause(14);
    }
    const b3During = lensMetrics().submitted - b3;
    await waitFor(() => lensMetrics().submitted > b3 + b3During, 2000, 10);
    await report('tl-b03-overlap', b3During <= 1 && lensMetrics().last.submit - b3Last >= 200, b3During,
      Math.round(lensMetrics().last.submit - b3Last));
    await leaveHistory();
    await report('tl-b03-tail', await waitFor(() => lensSourceText().includes('Stream line 040') && isStatus('translation.ready'), 8000));
    for (let i = 0; i < 20; i++) { await wheelAt($('terminal-host'), 30); await pause(12); }
    await click(tab('clipboard'));
    const afterSwitch = fake.requests.length;
    await pause(900);
    await report('tl-b03-scroll-mode', !fake.requests.slice(afterSwitch).some(r => r.strategy === 'live')
      && tab('clipboard').getAttribute('aria-selected') === 'true');
    await click(tab('live')); await leaveHistory(); await quietTerminal(600);

    const cardB = await newCard('translation pane b', '');
    await addSplit(cardA.id, 'row', false, cardB.id);
    const paneB = panes.get(cardB.session);
    await waitFor(() => !!paneB && paneB.term.buffer.active.length > 0, 8000);
    await clickTerm(paneB.term.element); await type("echo 'Beta pane text is visible here.'"); await key('Enter');
    await waitFor(() => viewportHas(paneB, 'Beta pane text is visible here.'), 6000);
    await clickTerm(paneA.term.element); await quietTerminal(600);
    for (let i = 0; i < 20; i++) { await wheelAt(paneA.term.element, 30); await pause(12); }
    await clickTerm(paneB.term.element);
    await waitFor(() => fake.requests.at(-1)?.text.includes('Beta pane'), 3000);
    await pause(500);
    const afterPane = fake.requests.filter(r => r.at > lastWheel);
    await report('tl-b03-scroll-pane', afterPane.length > 0 && afterPane.at(-1).text.includes('Beta pane')
      && await waitFor(() => lensSourceText().includes('Beta pane'), 3000));

    /* A05: pane A late result never displays for pane B; reopen rejects late */
    stage = 7;
    await toLive(paneA, cardA); await quietTerminal(600);
    fake.auto = null;
    await type('/next'); await key('Enter');
    const lateA = await waitFor(() => fake.pending('live'), 3000) && fake.pending('live');
    await clickTerm(paneB.term.element); await pause(300);
    // Pane A's request is still outstanding natively: B waits behind it (one outstanding).
    const heldBehind = !fake.requests.some(r => !r.done && r !== lateA);
    if (lateA) fake.settle(lateA, '甲窗格迟到译文');
    const bReq = await waitFor(() => fake.requests.some(r => !r.done && r.text.includes('Beta')), 3000);
    await pause(200);
    await report('tl-a05-pane-switch', !!lateA && heldBehind && bReq && !resultText().includes('甲窗格'),
      1, (heldBehind ? 1 : 0) + (bReq ? 2 : 0));
    for (const item of fake.requests.filter(r => !r.done)) fake.settle(item, '乙窗格译文');
    await report('tl-a05-pane-b', await waitFor(() => resultText() === '乙窗格译文'));
    await type("echo 'A later line in pane B.'"); await key('Enter');
    const lateClose = await waitFor(() => fake.pending('live'), 3000) && fake.pending('live');
    await click($('translation-close')); await click($('translation-btn'));
    if (lateClose) fake.settle(lateClose, '关闭前的迟到译文');
    await pause(300);
    await report('tl-a05-reopen', !!lateClose && resultText() !== '关闭前的迟到译文', 1, fake.unloads);
    fake.auto = fakeTranslation;
    for (const item of fake.requests.filter(r => !r.done)) fake.settle(item, fakeTranslation(item.text));

    /* B04 Agent route: an alternate-screen TUI scrolls its own history */
    await clickTerm(paneB.term.element);
    await type(`python3 ${shellQuote(fixture)} --mouse`); await key('Enter');
    await waitFor(() => viewportHas(paneB, 'History line 200'), 6000);
    await quietTerminal(600);
    const b4 = lensMetrics().submitted; let b4Last = 0;
    for (let t0 = performance.now(); performance.now() - t0 < 2000;) {
      await wheelAt(paneB.term.element, 40); b4Last = performance.now(); await pause(14);
    }
    const b4During = lensMetrics().submitted - b4;
    await waitFor(() => lensMetrics().submitted > b4 + b4During, 3000, 10);
    const agentView = visibleViewport(paneB);
    await report('tl-b04-agent-route', b4During <= 1 && !agentView.includes('History line 200')
      && fake.requests.at(-1).text === agentView, b4During, Math.round(lensMetrics().last.submit - b4Last));
    await key('q'); await quietTerminal(600); // q quits the mouse-mode TUI fixture
    await clickTerm(paneA.term.element); await quietTerminal(600);

    /* A06: hide Deck (real resign-active), come back, Live continues unasked */
    stage = 8;
    await waitFor(() => isStatus('translation.ready'), 4000);
    await type('/next'); await key('Enter'); await pause(50);
    const blurs = blurCount;
    const hidden = await hideDeck();
    await report('tl-a06-away', await waitFor(away, 5000, 50) && await waitFor(() => lensMetrics().status.startsWith('translation.background'), 3000),
      1, hidden * 10 + (blurCount > blurs ? 1 : 0));
    await report('tl-a06-await-return', true);
    await native('activate');
    const back = await backFront(30000);
    await report('tl-a06-resume', back && await waitFor(() => isStatus('translation.ready')
      && resultText().includes('〔'), 6000), 1, back ? 1 : 0);

    /* D01 / D02 / D03 failure and late-response injection */
    stage = 9;
    await quietTerminal(800);
    await click(tab('clipboard')); await pause(200);
    const d1 = fake.requests.length;
    fake.failNext = ['translation-failed'];
    await click(tab('live')); // intent on an unchanged view; the first attempt fails
    const sawError = await waitFor(() => isStatus('translation.error.translation-failed'), 4000, 10);
    const recovered = await waitFor(() => isStatus('translation.ready'), 5000);
    const attempts = fake.requests.slice(d1);
    await report('tl-d01-retry', sawError && recovered && attempts.length === 2 && attempts[0].text === attempts[1].text,
      attempts.length, lensMetrics().retries);
    // Type, let the echo settle, arm the fault, then Enter: one atomic burst,
    // so exactly one new capture follows.
    const burst = async command => {
      await toLive(paneA, cardA); await type(command); await quietTerminal(700);
      await waitFor(() => isStatus('translation.ready'), 4000);
    };
    await burst('/history 20');
    const kept = resultText(), d2 = fake.requests.length;
    fake.failNext = ['protected-restoration-failed'];
    await key('Enter');
    const d2Error = await waitFor(() => isStatus('translation.error.protected-restoration-failed'), 5000);
    await pause(1600); // longer than every retry delay: a definite failure never retries
    await report('tl-d02-update-failure', d2Error && resultText() === kept && lensMetrics().inflight === 0
      && isStatus('translation.error.protected-restoration-failed') && fake.requests.length === d2 + 1,
      fake.requests.length - d2, statusCode() * 100 + Math.min(9, fake.requests.length - d2) * 10 + (d2Error ? 1 : 0));
    await snapshot('l2-d02-state');
    fake.auto = null;
    const retriesBefore = lensMetrics().retries;
    await burst('/next'); await key('Enter');
    const late = await waitFor(() => fake.pending('live'), 3000) && fake.pending('live');
    await click(tab('clipboard'));
    if (late) fake.settle(late, null, 'translation-failed');
    await pause(1800);
    await report('tl-d03-late-failure', !!late && lensMetrics().retries === retriesBefore
      && !isStatus('translation.error.translation-failed'), lensMetrics().retries - retriesBefore + 1, statusCode() * 10 + (late ? 1 : 0));
    fake.pollDelay = 600; fake.copyExternally('A copy whose read is still in flight.');
    await pause(400); await click(tab('live'));
    await pause(900);
    await report('tl-d03-late-read', !fake.requests.some(r => r.text === 'A copy whose read is still in flight.'));
    fake.pollDelay = 0; fake.auto = fakeTranslation;
    for (const item of fake.requests.filter(r => !r.done)) fake.settle(item, fakeTranslation(item.text));

    /* C: copied content with the controlled gate (UI contract; L3 is native) */
    stage = 10;
    fake.copyExternally('An unrelated copy made before the tab.');
    const armsBefore = fake.arms, readsBefore = fake.reads;
    await click(tab('clipboard'));
    await report('tl-c01-no-old-read', await waitFor(() => isStatus('translation.waiting'), 3000)
      && !fake.requests.some(r => r.text.startsWith('An unrelated copy')) && fake.reads === readsBefore, fake.arms - armsBefore);
    await click(tab('clipboard'));
    await pause(300);
    await report('tl-c08-reselect', fake.arms - armsBefore === 1, fake.arms - armsBefore);
    fake.copyExternally('First copied answer.'); await pause(40); fake.copyExternally('Second copied answer.');
    await report('tl-c03-latest', await waitFor(() => resultText().includes('Second copied answer'), 4000)
      && !fake.requests.some(r => r.text === 'First copied answer.' && r.done && resultText().includes('First')));
    const c3 = fake.requests.length;
    fake.copyExternally('Second copied answer.'); await pause(900);
    await report('tl-c03-same-text', fake.requests.length === c3);
    await click($('translation-copy')); await pause(900);
    await click($('translation-copy-source')); await pause(900);
    await report('translation-copy', fake.copied === 'Second copied answer.');
    await report('tl-c05-no-feedback', fake.requests.length === c3);
    fake.copyExternally('A genuinely new answer.');
    await report('tl-c05-next-copy', await waitFor(() => resultText().includes('A genuinely new answer'), 4000));
    /* G01-G03, G08 (L2): content similarity never decides provenance */
    const clipTranslate = async (text, out) => {
      const n = fake.requests.length; fake.copyExternally(text);
      const ok = await waitFor(() => fake.requests.slice(n).some(r => r.text === text), 3000);
      const req = fake.requests.slice(n).find(r => r.text === text && !r.done);
      if (req) fake.settle(req, out);
      return ok && await waitFor(() => lensSourceText() === text && isStatus('translation.ready'), 3000);
    };
    fake.auto = null;
    for (const item of fake.requests.filter(r => !r.done)) fake.settle(item, fakeTranslation(item.text));
    await clipTranslate('Check the log: `The deployment completed successfully.` Then continue.',
      '检查日志：`The deployment completed successfully.` 然后继续。');
    await report('tl-g01-substring', await clipTranslate('The deployment completed successfully.', '部署已成功完成。')
      && resultText() === '部署已成功完成。');
    await report('tl-g02-whole', await clipTranslate('部署已成功完成。', '部署已成功完成。'));
    await clipTranslate('Answer A.', '回答甲。'); await clipTranslate('Answer B.', '回答乙。');
    await report('tl-g03-a-b-a', await clipTranslate('Answer A.', '回答甲。') && resultText() === '回答甲。');
    fake.failNext = ['translation-model-corrupt'];
    fake.copyExternally('Retry after a failure.');
    const failedFirst = await waitFor(() => isStatus('translation.error.translation-model-corrupt'), 3000);
    await report('tl-g08-recopy', failedFirst && await clipTranslate('Retry after a failure.', '失败后重试。'));
    fake.maxOutstanding = fake.outstanding; fake.auto = fakeTranslation;
    for (let i = 0; i < 8; i++) { fake.copyExternally(`Rapid copy ${i}.`); await pause(60); }
    await report('tl-g08-rapid-bounded', await waitFor(() => lensSourceText() === 'Rapid copy 7.' && isStatus('translation.ready'), 5000)
      && fake.maxOutstanding <= 1 && lensMetrics().pending <= 1, fake.maxOutstanding);
    const c7 = resultText(), c7Req = fake.requests.length;
    await clickTerm(paneA.term.element); await type('/next'); await key('Enter');
    for (let i = 0; i < 20; i++) { await wheelAt(paneA.term.element, 30); await pause(12); }
    await pause(900); await leaveHistory();
    await report('tl-c07-static', resultText() === c7 && fake.requests.length === c7Req);

    /* selection snapshot + Copy Source pairing */
    await quietTerminal(600);
    paneA.term.selectAll(); await chord();
    await report('translation-selection', await waitFor(() => fake.requests.at(-1)?.strategy === 'selection', 3000)
      && tab('live').getAttribute('aria-selected') === 'false' && tab('clipboard').getAttribute('aria-selected') === 'false');
    await report('tl-c07-selection-status', await waitFor(() => isStatus('translation.selected'), 3000));
    const selectionCount = fake.requests.length;
    await clickTerm(paneA.term.element); await type('/next'); await key('Enter'); await pause(900);
    await report('translation-selection-static', fake.requests.length === selectionCount);
    await click($('translation-copy-source'));
    await report('translation-copy-source', await waitFor(() => fake.copied === fake.requests[selectionCount - 1].text, 2000));

    /* E01: tabs by keyboard and mouse, ARIA, narrow/overlay, font scale, themes, locales */
    stage = 11;
    await click(tab('live'));
    await report('tl-e01-mouse', tab('live').getAttribute('aria-selected') === 'true', trusted('pointerdown'));
    await key('ArrowRight');
    const right = tab('clipboard').getAttribute('aria-selected') === 'true' && document.activeElement === tab('clipboard');
    await key('ArrowLeft');
    const left = tab('live').getAttribute('aria-selected') === 'true' && document.activeElement === tab('live');
    await key('End'); const endKey = tab('clipboard').getAttribute('aria-selected') === 'true';
    await key('Home'); const homeKey = tab('live').getAttribute('aria-selected') === 'true';
    await report('tl-e01-keyboard', right && left && endKey && homeKey, 1, trusted('keydown'));
    const list = $('translation-tabs');
    await report('tl-e01-aria', list.getAttribute('role') === 'tablist' && !!list.getAttribute('aria-label')
      && tab('live').getAttribute('role') === 'tab' && tab('live').getAttribute('aria-controls') === 'translation-result'
      && tab('live').tabIndex === 0 && tab('clipboard').tabIndex === -1
      && $('translation-result').getAttribute('role') === 'tabpanel'
      && $('translation-result').getAttribute('aria-labelledby') === 'translation-tab-live');
    const fits = () => [tab('live'), tab('clipboard')].every(el => el.scrollWidth <= el.clientWidth + 1
      && el.getBoundingClientRect().right <= $('translation-panel').getBoundingClientRect().right + 1);
    $('session-workspace').style.maxWidth = '560px'; await pause(400);
    await report('tl-e01-narrow', $('session-workspace').classList.contains('translation-overlay') && fits());
    await snapshot('l2-e01-narrow');
    $('session-workspace').style.maxWidth = '';
    applyFontScale(1.3); await pause(300);
    const fontFits = fits(); await snapshot('l2-e01-font-130');
    applyFontScale(1); await pause(200);
    await report('tl-e01-font', fontFits);
    const themes = [];
    for (const theme of ['light', 'high-contrast', 'deck-dark']) {
      activateTheme({ ...ctx.settings, theme }); await pause(250);
      const selectedTab = getComputedStyle(tab('live')), other = getComputedStyle(tab('clipboard'));
      themes.push(selectedTab.backgroundColor !== other.backgroundColor);
      await snapshot(`l2-e01-theme-${theme}`);
    }
    activateTheme(ctx.settings);
    await report('tl-e01-themes', themes.every(Boolean), themes.filter(Boolean).length);
    const locales = [];
    for (const locale of ['en', 'zh-Hans']) {
      setLocale(locale); await pause(200);
      locales.push(tab('clipboard').textContent === t('translation.clipboard') && status().length > 0);
      await snapshot(`l2-e01-locale-${locale}`);
    }
    setLocale(ctx.settings.locale || 'system');
    await report('tl-e01-locales', locales.every(Boolean) && t('translation.clipboard') !== 'Clipboard');

    /* B07: 30 s mixed output, scroll and tab switching */
    stage = 12;
    const lag = lagProbe();
    const b7 = { ...lensMetrics() }, b7Req = fake.requests.length, b7Cancel = fake.cancelled;
    fake.maxOutstanding = fake.outstanding;
    let maxPending = 0;
    const sampler = setInterval(() => { maxPending = Math.max(maxPending, lensMetrics().pending); }, 50);
    await clickTerm(paneA.term.element);
    for (let t0 = performance.now(), round = 0; performance.now() - t0 < 30000; round++) {
      await type('/stream 12'); await key('Enter');
      for (let i = 0; i < 40; i++) { await wheelAt(paneA.term.element, i < 20 ? 30 : -30); await pause(12); }
      await leaveHistory();
      await click(tab('clipboard')); await pause(120); await click(tab('live'));
      await clickTerm(paneA.term.element); await pause(300);
    }
    clearInterval(sampler);
    await quietTerminal(800, 15000);
    const b7m = lensMetrics();
    await metric('tl-b07-requests', b7m.submitted - b7.submitted, fake.cancelled - b7Cancel);
    await metric('tl-b07-dom', b7m.domWrites - b7.domWrites, b7m.captures - b7.captures);
    await report('tl-b07-bounded', fake.maxOutstanding <= 1 && maxPending <= 1 && b7m.inflight <= 1,
      fake.maxOutstanding, maxPending);
    const maxLag = lag.done();
    await report('tl-b07-responsive', maxLag < 400, maxLag, fake.requests.length - b7Req);
    await report('tl-b07-caught-up', await waitFor(() => isStatus('translation.ready'), 8000));

    /* A04: rapid tab round trips on a static view */
    stage = 13;
    fake.auto = null; fake.maxOutstanding = fake.outstanding;
    await quietTerminal(600);
    for (let i = 0; i < 10; i++) { await click(tab('clipboard')); await click(tab('live')); }
    await pause(300);
    let drained = 0;
    while (fake.pending() && drained < 30) { fake.settle(fake.pending(), '往返后的译文'); drained++; await pause(80); }
    await report('tl-a04-roundtrip', await waitFor(() => resultText() === '往返后的译文' && isStatus('translation.ready'), 3000),
      drained, fake.maxOutstanding);
    await report('tl-a04-bounded', fake.maxOutstanding <= 1, fake.maxOutstanding);
    fake.auto = fakeTranslation;

    /* A03: a truly blank viewport, then late first output */
    stage = 14;
    $('session-workspace').style.maxWidth = '750px'; // overlay: opening the Lens never resizes the terminal
    const cardC = await newCard('translation blank',
      "printf '\\033[2J\\033[3J\\033[H'; sleep 9; echo 'Late output arrives after the Lens is open.'; sleep 600");
    await openSession(cardC.id);
    const paneC = panes.get(cardC.session);
    await quietTerminal(600);
    const blankFirst = await waitFor(() => !visibleViewport(paneC).trim(), 8000);
    await metric('tl-a03-blank-first', blankFirst ? 1 : 0);
    const c0 = fake.requests.length;
    if (!$('translation-panel').hidden) await click($('translation-close'));
    await clickTerm(paneC.term.element);
    await click($('translation-btn'));
    await report('tl-a03-empty-honest', await waitFor(() => isStatus('translation.empty'), 3000)
      && fake.requests.length === c0, 1, statusCode() * 10000 + Math.min(999, lensSourceText().length) * 10
        + (ctx.attachedName === cardC.session ? 1 : 0));
    await report('tl-a03-late-output', await waitFor(() => fake.requests.slice(c0).some(r => r.text.includes('Late output')), 15000)
      && await waitFor(() => resultText().includes('Late output'), 3000));

    $('session-workspace').style.maxWidth = '';
    /* lifecycle boundaries: buffer, disable, delete, leave */
    stage = 15;
    await openBuffer(cardC.id);
    await report('translation-buffer-mutual', $('translation-panel').hidden && !$('buffer-panel').hidden);
    await click($('translation-btn'));
    await report('translation-buffer-close', $('buffer-panel').hidden && !$('translation-panel').hidden);
    await openSettings({ section: 'terminal' });
    $('set-local-translation').click();
    await report('translation-disable', await waitFor(() => $('translation-btn').hidden)
      && $('translation-panel').hidden && fake.unloads > 0 && fake.installed);
    $('set-translation-delete').click();
    await waitFor(() => $('cfm').style.display === 'flex');
    $('cfm-yes').click();
    await report('translation-delete', await waitFor(() => !fake.installed));
    $('set-close').click();
    backToBoard();
    await report('translation-leave', $('translation-panel').hidden && state.view === 'board');
    await metric('tl-front-regains', frontRegains);
    await metric('tl-l2-native-events', trusted('pointerdown') + trusted('wheel') + trusted('keydown'),
      untrusted('pointerdown') + untrusted('wheel'));
    await report('done', !failed);
  } catch (error) {
    await metric('tl-exception', stage, String(error?.message || '').length);
    await report('done', false, 1, stage);
  }
}

/* ===== L3: production path ===== */
const ANSWER_ONE = 'The build completed successfully and all tests passed.';
const ANSWER_TWO = 'The second answer explains how to restart the server safely.';
const han = text => (text.match(/[一-鿿]/g) || []).length;
const PROTECTED = ['src/intelligence/translation.rs', 'https://example.com/reports/build-42',
  'cargo test --workspace', 'translation_translate'];
function paragraph(i) {
  return `Paragraph ${i}: the worker saved the file to /var/tmp/deck_case_${String(i).padStart(4, '0')}.log `
    + `and reported the identifier item_${String(i).padStart(4, '0')} to https://example.com/items/${i}. `
    + 'The team reviewed the result and approved the change.\n\n';
}
function sizedDocument(bytes) {
  let text = '', i = 1;
  while (new TextEncoder().encode(text + paragraph(i)).length <= bytes) text += paragraph(i++);
  const pad = bytes - new TextEncoder().encode(text).length;
  return { text: text + 'x'.repeat(Math.max(0, pad)), anchors: i - 1 };
}
const CORPUS = [
  { id: 'prose', text: 'Please review the pull request before the meeting tomorrow. The team will discuss the release plan and the test results.' },
  { id: 'mixed', text: 'Open src/lib/parser.rs and update the function parse_config_value. See https://docs.example.com/guide#setup for details. Run `npm run build` after that.',
    exact: ['src/lib/parser.rs', 'parse_config_value', 'https://docs.example.com/guide#setup', 'npm run build'] },
  { id: 'fence', text: 'Use this command to build the project:\n\n```\ncargo build --release --locked\n```\n\nThen restart the application.',
    exact: ['cargo build --release --locked'] },
  { id: 'chinese', text: '构建已经成功完成，所有测试均已通过。', identity: true },
  { id: 'unsupported', text: 'Bonjour, je voudrais réserver une table pour deux personnes ce soir, s’il vous plaît.',
    error: 'source-language-unsupported' },
];

export async function runTranslationNativeSmoke() {
  let stage = 0, named = false;
  const newRequests = since => lensMetrics().submitted - since;
  const shown = async (pred, timeout = 30000) => waitFor(pred, timeout, 25);
  try {
    const fixture = await inv('smoke_native_fixture');
    await waitFor(() => provider.projects().length > 0);
    stage = 1;
    await saveTranslationSettings({ enabled: true, documentLimitBytes: 16384 });
    window.dispatchEvent(new Event('deck-translation-enabled-changed'));
    const cap = await capability();
    await report('tl-n-capability', cap.available && cap.installed && !cap.loaded, 1, cap.loaded ? 1 : 0);
    const card = await newCard('translation native', `python3 ${shellQuote(fixture)}`);
    await openSession(card.id);
    const pane = panes.get(card.session);
    await ensureFront();
    await waitFor(() => viewportHas(pane, ANSWER_ONE), 15000);
    await waitFor(() => !$('translation-btn').hidden, 5000);
    await quietTerminal(800);

    /* A01 real: open on static output, real cold model */
    stage = 2;
    const clickAt = performance.now();
    await click($('translation-btn'));
    const firstOk = await shown(() => han(resultText()) > 10 && isStatus('translation.ready'), 60000);
    const capAt = firstAfter('capture', clickAt), subAt = firstAfter('submit', capAt);
    const setAt = firstAfter('settle', subAt), dispAt = firstAfter('display', setAt);
    const result = resultText();
    await report('tl-n-a01-static-open', firstOk, Math.round(dispAt - clickAt), trusted('pointerdown'));
    await report('tl-n-d06-protected-live', PROTECTED.every(item => result.includes(item)),
      PROTECTED.filter(item => result.includes(item)).length, PROTECTED.length);
    await metric('tl-t-open-capture', capAt - clickAt);
    await metric('tl-t-capture-submit', subAt - capAt);
    await metric('tl-t-cold', lensMetrics().accepted[0].settle - lensMetrics().accepted[0].submit);
    await metric('tl-t-settle-display', dispAt - setAt);
    await report('tl-n-d07-loaded', (await capability()).loaded === true);
    await snapshot('l3-live-real');

    /* warm Live update */
    await clickTerm($('terminal-host')); await type('/next'); await quietTerminal(700);
    const warmStart = lensMetrics().submitted, enterAt = performance.now(); await key('Enter');
    await report('tl-n-live-update', await shown(() => lensMetrics().submitted > warmStart
      && isStatus('translation.ready') && lensSourceText().includes(ANSWER_TWO) && han(resultText()) > 10, 30000));
    // The request whose accepted result is now shown (the answer's own).
    const shownRequest = lensMetrics().accepted.at(-1);
    await metric('tl-t-warm', shownRequest.settle - shownRequest.submit);

    /* A06 real: hide and return */
    stage = 3;
    await hideDeck();
    await report('tl-n-a06-away', await waitFor(away, 5000, 50) && await waitFor(() => lensMetrics().status.startsWith('translation.background'), 3000));
    await report('tl-n-a06-await-return', true);
    await native('activate');
    const back = await backFront(30000);
    await clickTerm($('terminal-host')); await type('/history 12'); await key('Enter');
    const a6 = lensMetrics().submitted;
    await report('tl-n-a06-resume', back && await shown(() => lensMetrics().submitted > a6 && isStatus('translation.ready')
      && lensSourceText().includes('History line'), 30000) || false);

    /* B02 real: scroll burst, final viewport only */
    stage = 4;
    await type('/history 120'); await key('Enter'); await quietTerminal(800);
    await shown(() => isStatus('translation.ready'), 30000);
    const b2 = lensMetrics().submitted; let lastWheel = 0;
    for (let t0 = performance.now(); performance.now() - t0 < 3000;) {
      await wheelAt($('terminal-host'), 40); lastWheel = performance.now(); await pause(12);
    }
    const during = newRequests(b2);
    await waitFor(() => newRequests(b2) > during, 3000, 10);
    const submitAt = lensMetrics().last.submit;
    const finalView = visibleViewport(pane);
    await report('tl-n-b02-final', during <= 1 && await shown(() => isStatus('translation.ready')
      && lensMetrics().last.display > submitAt, 30000), during, Math.round(submitAt - lastWheel));
    await metric('tl-t-scroll-submit', firstAfter('submit', lastWheel) - lastWheel);
    await metric('tl-t-scroll-display', firstAfter('display', firstAfter('settle', lastWheel)) - lastWheel);
    await report('tl-n-b02-final-view', finalView.includes('History line') && !finalView.includes('History line 120'));
    await leaveHistory(); await quietTerminal(600);

    /* shared general pasteboard: four short guarded sections (F08). Every
       write is gated before it happens: the guard's own write, or a permit
       followed by the writer's receipt (fixture compare-and-write, the Lens
       writer). A refusal throws; settling disarms Copied-text observation
       first, so a restored original is never read. */
    stage = 5;
    const term = () => $('terminal-host');
    const fixtureReceipt = async () => {
      let receipt = -1;
      await waitFor(async () => (receipt = await inv('smoke_native_fixture_receipt')) > 0, 5000, 50);
      return receipt;
    };
    const copyFromFixture = async g => {
      const version = await g.permit();
      await clickTerm(term()); await type(`/copy ${version}`);
      const at = performance.now(); await key('Enter');
      await g.adopt(await fixtureReceipt());
      return at;
    };
    const lensCopy = async (g, button) => {
      await g.permit();
      const before = lensMetrics().lastWriteReceipt;
      await click(button);
      await waitFor(() => lensMetrics().lastWriteReceipt !== before, 3000);
      await g.adopt(lensMetrics().lastWriteReceipt);
    };
    const section = body => withGuard(pb, 0, body, async info => {
      await report('tl-f08-guard', info.result === 10 || info.result === 11, info.result, info.stage === 'begin' ? info.code : info.id);
    });
    const reArm = async () => { await click(tab('live')); await click(tab('clipboard'));
      return waitFor(() => isStatus('translation.waiting'), 5000, 5); };

    let copyAt = 0;
    await section(async g => {
      await g.write('Deck harmless earlier copy before the tab.');
      const c1 = lensMetrics().submitted, tabAt = performance.now();
      await click(tab('clipboard'));
      await report('tl-n-c01-ready', await waitFor(() => isStatus('translation.waiting'), 5000));
      await metric('tl-t-armed', firstAfter('armed', tabAt) - tabAt);
      await pause(1200);
      await report('tl-n-c01-old-unread', newRequests(c1) === 0 && !resultText());
      /* C02 real: controlled /copy right after the baseline */
      copyAt = await copyFromFixture(g);
      await waitFor(() => firstAfter('copied', copyAt) > 0, 5000);
    });
    const c2ok = await shown(() => han(resultText()) > 10 && isStatus('translation.ready'), 30000);
    const copiedAt = firstAfter('copied', copyAt), shownAt = firstAfter('display', copiedAt);
    // The fixture shows answer two since the Live update's /next.
    await report('tl-n-c02-copy', c2ok && lensSourceText().startsWith(ANSWER_TWO), Math.round(shownAt - copyAt));
    await metric('tl-t-copy-accept', copiedAt - copyAt);
    await metric('tl-t-copy-display', shownAt - copiedAt);
    await snapshot('l3-copied-real');
    const c2Result = resultText();

    /* C02 near arm: /copy the next answer as soon as the new baseline exists */
    await clickTerm(term()); await type('/next'); await key('Enter'); await pause(300);
    let nearGap = 0;
    await section(async g => {
      await reArm();
      const armedAt = lensMetrics().last.armed;
      const at = await copyFromFixture(g);
      nearGap = at - armedAt;
      await waitFor(() => firstAfter('copied', at) > 0, 5000);
    });
    await report('tl-n-c02-near-arm', await shown(() => han(resultText()) > 5 && isStatus('translation.ready')
      && resultText() !== c2Result, 30000) && lensSourceText().startsWith(ANSWER_ONE), Math.round(nearGap));

    /* C07 real: the copied snapshot ignores terminal output and scroll (no pasteboard writes) */
    const c7 = resultText(), c7r = lensMetrics().submitted;
    await clickTerm(term()); await type('/history 30'); await key('Enter');
    for (let i = 0; i < 30; i++) { await wheelAt(term(), 30); await pause(12); }
    await pause(1200); await leaveHistory();
    await report('tl-n-c07-static', resultText() === c7 && newRequests(c7r) === 0);

    /* C05 real: Deck's own copies never feed back; the next real copy works */
    await clickTerm(term()); await type('/next'); await key('Enter'); await pause(300);
    let c5 = 0, c5At = 0, c5Shown = '';
    await section(async g => {
      await reArm(); // re-entering the tab resets the result: copy an answer in first
      await copyFromFixture(g);
      await shown(() => lensSourceText().startsWith(ANSWER_TWO) && isStatus('translation.ready'), 30000);
      c5Shown = resultText(); c5 = lensMetrics().submitted;
      await lensCopy(g, $('translation-copy')); await pause(1000);
      await lensCopy(g, $('translation-copy-source')); await pause(1000);
      await report('tl-n-c05-no-feedback', newRequests(c5) === 0 && resultText() === c5Shown, 1,
        lensMetrics().lastWriteReceipt > 0 ? 1 : 0);
      await clickTerm(term()); await type('/next'); await key('Enter'); await pause(300);
      c5At = await copyFromFixture(g);
      await waitFor(() => firstAfter('copied', c5At) > 0, 5000);
    });
    await report('tl-n-c05-next-copy', await shown(() => resultText() !== c5Shown && lensSourceText().startsWith(ANSWER_ONE)
      && isStatus('translation.ready'), 30000));

    /* C04 real: a copy made while Deck is away (the driver's compare-and-write) is never read */
    stage = 6;
    await clickTerm(term()); await type('/next'); await key('Enter'); await pause(300);
    let c4 = 0, returned = false;
    await section(async g => {
      await reArm();
      c4 = lensMetrics().submitted;
      const version = await g.permit();
      await hideDeck();
      await waitFor(away, 5000, 50);
      await report('tl-n-c04-await-away', true, 1, version); // driver: compare-and-write, receipt, reopen
      returned = await backFront(45000);
      await g.adopt(await fixtureReceipt());
      await waitFor(() => isStatus('translation.ready') || isStatus('translation.waiting'), 5000);
      await pause(1500);
      await report('tl-n-c04-away-unread', returned && newRequests(c4) === 0);
      const at = await copyFromFixture(g);
      await waitFor(() => firstAfter('copied', at) > 0, 5000);
    });
    await report('tl-n-c04-after-return', await shown(() => newRequests(c4) === 1 && isStatus('translation.ready')
      && lensSourceText().startsWith(ANSWER_TWO), 30000));
    await click($('translation-close')); await click($('translation-btn'));
    const reopened = lensMetrics().submitted;
    await click(tab('clipboard')); await waitFor(() => isStatus('translation.waiting'), 5000); await pause(1200);
    await report('tl-n-c04-reopen-unread', newRequests(reopened) <= 1 && !resultText(), newRequests(reopened));

    /* test-owned named pasteboard through the same native gate */
    stage = 7;
    await inv('smoke_pasteboard', { action: 'named-on' }); named = true;
    await click(tab('live')); await click(tab('clipboard'));
    await waitFor(() => isStatus('translation.waiting'), 5000);
    const nextCopy = async (text, kind = 'named-text') => {
      const before = lensMetrics().submitted;
      await inv('smoke_pasteboard', { action: kind, text });
      return before;
    };
    await nextCopy('', 'named-data');
    await report('tl-n-c06-not-text', await waitFor(() => isStatus('translation.error.clipboard-not-text'), 3000));
    await nextCopy('', 'named-empty');
    await report('tl-n-c06-empty', await waitFor(() => isStatus('translation.error.text-empty'), 3000));
    let before = await nextCopy('The first copied line.');
    await pause(30); await inv('smoke_pasteboard', { action: 'named-text', text: 'The second copied line replaces the first one.' });
    await report('tl-n-c03-latest', await shown(() => isStatus('translation.ready') && han(resultText()) > 3, 30000)
      && lensSourceText() === 'The second copied line replaces the first one.', newRequests(before));
    before = await nextCopy('The second copied line replaces the first one.');
    await pause(1200);
    await report('tl-n-c03-same', newRequests(before) === 0);

    /* G01-G07 real (named board, same native gate): provenance comes from the
       Lens writer's receipt, never from text resembling an old result */
    stage = 12;
    const copied = async (text, timeout = 30000) => { await nextCopy(text);
      return shown(() => lensSourceText() === text && isStatus('translation.ready'), timeout); };
    const kept = 'The deployment completed successfully.';
    await copied(`Please check the log: \`${kept}\` Then continue.`);
    const keptInTranslation = resultText().includes(kept);
    await report('tl-n-g01-substring', keptInTranslation && await copied(kept) && han(resultText()) > 3, 1, keptInTranslation ? 1 : 0);
    const whole = resultText();
    await report('tl-n-g02-whole', await copied(whole) && resultText() === whole);
    await copied('The first answer is ready.'); await copied('The second answer is ready.');
    await report('tl-n-g03-a-b-a', await copied('The first answer is ready.'));
    const g4 = lensMetrics().submitted, shownNow = resultText();
    await click($('translation-copy')); await pause(1000);
    await click($('translation-copy-source')); await pause(1000);
    const g4Quiet = newRequests(g4) === 0;
    await report('tl-n-g04-buttons', g4Quiet && await copied(shownNow), newRequests(g4), g4Quiet ? 1 : 0);
    const box = $('translation-result').getBoundingClientRect();
    await drag({ x: box.left + 16, y: box.top + 14 }, { x: box.left + 140, y: box.top + 16 });
    const piece = String(document.getSelection()), g5 = lensMetrics().submitted, g5Receipt = lensMetrics().lastWriteReceipt;
    await key('c', ['command']);
    await waitFor(() => lensMetrics().lastWriteReceipt !== g5Receipt, 3000); await pause(900);
    const g5Quiet = newRequests(g5) === 0 && lensMetrics().lastWriteReceipt !== g5Receipt;
    await key('Tab');
    await report('tl-n-g05-cmd-c', piece.length > 0 && g5Quiet && await copied(piece), piece.length, g5Quiet ? 1 : 0);
    const g6 = lensMetrics().submitted;
    await click($('translation-copy'));
    await inv('smoke_pasteboard', { action: 'named-text', text: 'A new copy right after the Lens copy.' });
    await report('tl-n-g06-interleave', await shown(() => lensSourceText() === 'A new copy right after the Lens copy.'
      && isStatus('translation.ready'), 30000), newRequests(g6));
    await click($('translation-copy')); await pause(300);
    await click(tab('live')); await click(tab('clipboard')); await waitFor(() => isStatus('translation.waiting'), 5000);
    const g7Mode = await copied('Copied in the next observation cycle.');
    await click($('translation-copy')); await pause(300);
    await click($('translation-close')); await click($('translation-btn'));
    await click(tab('clipboard')); await waitFor(() => isStatus('translation.waiting'), 5000);
    await report('tl-n-g07-cycles', g7Mode && await copied('Copied after the Lens reopened.'));

    /* D05/D06 real corpus */
    stage = 8;
    for (const item of CORPUS) {
      before = await nextCopy(item.text);
      if (item.error) {
        await report(`tl-n-d05-${item.id}`, await waitFor(() => isStatus(`translation.error.${item.error}`), 15000));
        continue;
      }
      const ok = await shown(() => lensSourceText() === item.text && isStatus('translation.ready'), 30000);
      const out = resultText();
      const exact = (item.exact || []).filter(anchor => out.includes(anchor)).length;
      const pass = ok && (item.identity ? out === item.text : han(out) >= 6) && exact === (item.exact || []).length;
      await report(`tl-n-d05-${item.id}`, pass, han(out) + 1, exact);
    }

    /* C06/D04 real boundaries through Settings' document limit */
    stage = 9;
    for (const limit of [8192, 16384]) {
      await saveTranslationSettings({ documentLimitBytes: limit });
      const doc = sizedDocument(limit);
      before = await nextCopy(doc.text);
      const at = performance.now();
      const ok = await shown(() => lensSourceText() === doc.text && isStatus('translation.ready'), 60000);
      const out = resultText();
      const anchors = Array.from({ length: doc.anchors }, (_, i) => `item_${String(i + 1).padStart(4, '0')}`)
        .filter(anchor => out.includes(anchor)).length;
      await report(`tl-n-c06-${limit / 1024}k`, ok, Math.round(performance.now() - at), limit);
      await report(`tl-n-d06-${limit / 1024}k-anchors`, ok && anchors === doc.anchors, anchors, doc.anchors);
      await metric(`tl-t-doc-${limit / 1024}k`, lensMetrics().last.settle - lensMetrics().last.submit);
      await nextCopy(doc.text + 'y');
      await report(`tl-n-c06-${limit / 1024}k-over`, await waitFor(() => isStatus('translation.error.text-too-large'), 5000)
        && resultText() === out);
    }

    /* D07 real: unload on close, cold reopen, damaged/missing pack, restore */
    stage = 10;
    await inv('smoke_pasteboard', { action: 'named-off' }); named = false;
    await click($('translation-close'));
    await report('tl-n-d07-unloaded', await waitFor(async () => (await capability()).loaded === false, 5000));
    await click($('translation-btn'));
    const reopenAt = performance.now();
    await report('tl-n-d07-cold-reopen', await shown(() => isStatus('translation.ready') && han(resultText()) > 3, 60000)
      && (await capability()).loaded === true, Math.round(performance.now() - reopenAt));
    await metric('tl-t-cold-reopen', lensMetrics().last.settle - lensMetrics().last.submit);
    for (const [step, expected] of [['corrupt', 'translation-model-corrupt'], ['missing', 'translation-model-missing']]) {
      await report(`tl-n-d07-await-${step}`, true); // driver damages / removes the isolated pack copy
      await waitFor(async () => { const pack = await packStatus(); return step === 'corrupt' ? pack.corrupt : !pack.installed && !pack.corrupt; }, 30000, 200);
      await clickTerm($('terminal-host')); await goLive(card.session); await pause(150);
      await type('/next'); await key('Enter');
      const d7 = await waitFor(() => isStatus(`translation.error.${expected}`), 15000);
      await report(`tl-n-d07-${step}`, d7, 1, statusCode() * 100 + lensMetrics().activity);
      if (!d7) await snapshot(`l3-debug-d07-${step}`);
    }
    await report('tl-n-d07-await-restore', true);
    await waitFor(async () => (await packStatus()).installed, 30000, 200);
    await clickTerm($('terminal-host')); await goLive(card.session); await pause(150);
    await type('/next'); await key('Enter');
    await report('tl-n-d07-restored', await shown(() => isStatus('translation.ready'), 60000));

    /* C07 real selection snapshot: native drag in the terminal, shortcut */
    stage = 11;
    await goLive(card.session); await quietTerminal(600);
    const rows = $('terminal-host').querySelector('.xterm-screen').getBoundingClientRect();
    await drag({ x: rows.left + 4, y: rows.top + 6 }, { x: rows.left + rows.width * 0.8, y: rows.top + rows.height * 0.3 }, 8);
    await chord();
    await report('tl-n-c07-selection', await shown(() => isStatus('translation.selected'), 30000)
      && lensSourceText().length > 0, trusted('pointerdown'));
    closeTranslationLens(); backToBoard();
    await saveTranslationSettings({ enabled: false });
    window.dispatchEvent(new Event('deck-translation-disabled'));
    await metric('tl-front-regains', frontRegains);
    await metric('tl-l3-native-events', trusted('pointerdown') + trusted('wheel') + trusted('keydown'),
      untrusted('pointerdown') + untrusted('wheel'));
    await report('done', !failed);
  } catch (error) {
    await metric('tl-exception', stage, String(error?.message || '').length);
    // withGuard already settled any open section before this point.
    if (error instanceof GuardRefused) await report('tl-f08-refused', false, 1, error.code);
    if (named) await inv('smoke_pasteboard', { action: 'named-off' }).catch(() => {});
    await report('done', false, 1, stage);
  }
}

/* ===== F: harness safety, on the test-owned NAMED pasteboard only =====
   Every injected failure (refused begin, external takeover, equal-text
   impostor, exception, driver cancel, kill) runs on the named board through
   the same guard implementation the general sections use. The general
   pasteboard is never written by this mode. */
export async function runTranslationGuardSmoke() {
  let stage = 0;
  const count = () => pb('count', { board: 1 });
  const results = [];
  const guarded = (body, board = 1) => withGuard(pb, board, body, info => { results.push(info); });
  const outcome = promise => promise.then(() => 'completed', error => (error instanceof GuardRefused ? error.stage : 'error'));
  try {
    const fixture = await inv('smoke_native_fixture');
    await waitFor(() => provider.projects().length > 0);
    const card = await newCard('translation guard', `python3 ${shellQuote(fixture)}`);
    await openSession(card.id);
    await ensureFront();
    await waitFor(() => viewportHas(panes.get(card.session), ANSWER_ONE), 15000);
    await inv('smoke_native_fixture_receipt'); // start without a receipt

    stage = 1; /* F07: empty, multi-item multi-type, non-text: exact restore */
    for (const [kind, name] of [['named-clear', 'empty'], ['named-multi', 'multi'], ['named-data', 'non-text']]) {
      await pb(kind);
      await guarded(async g => { await g.write('synthetic test write'); });
      await report(`tl-f07-${name}`, results.at(-1).result === 11, results.at(-1).result);
    }

    stage = 2; /* F01: lazy/promised data refuses begin; nothing after it runs */
    await pb('named-lazy');
    const f1Count = await count(), keysBefore = trusted('keydown'), effects = [];
    const f1 = await outcome(guarded(async g => {
      effects.push('typed'); await clickTerm($('terminal-host')); await type(`/copy ${await g.permit()}`); await key('Enter');
      effects.push('written'); await g.write('x');
    }));
    await pause(800);
    await report('tl-f01-begin-refused', f1 === 'begin' && results.at(-1).result === 15, 1, results.at(-1).code);
    await report('tl-f01-zero-effects', effects.length === 0 && trusted('keydown') === keysBefore
      && await count() === f1Count && await inv('smoke_native_fixture_receipt') === -1, effects.length);

    stage = 3; /* F02: no guard, ended guard, version changed before the write */
    await pb('named-text', { text: 'original' });
    let before = await count();
    await report('tl-f02-no-guard', await pb('write', { text: 'x' }) < 0 && await pb('permit') < 0 && await count() === before);
    await guarded(async () => {});
    before = await count();
    await report('tl-f02-ended-guard', results.at(-1).result === 10 && await pb('write', { text: 'x' }) < 0 && await count() === before);
    const f2 = await outcome(guarded(async g => {
      await pb('named-text', { text: 'external' }); before = await count();
      await g.write('test');
    }));
    await report('tl-f02-version-changed', f2 === 'write' && results.at(-1).result === 12 && await count() === before);
    // a fixture /copy whose permitted version is stale refuses to write
    const f2b = await outcome(guarded(async g => {
      const version = await g.permit();
      await pb('named-text', { text: 'external again' });
      await g.adopt(-1);
      void version;
    }));
    await report('tl-f02-stale-permit', f2b === 'adopt' && results.at(-1).result === 12);

    stage = 4; /* F03: an external write equal (or normalized-equal) to the test text is not adopted */
    for (const sample of ['The expected test text.', 'The expected test text.\n ']) {
      await pb('named-text', { text: 'original' });
      const f3 = await outcome(guarded(async g => {
        await g.permit();
        await pb('named-text', { text: sample });
        before = await count();
        await g.adopt(-1); // no writer receipt exists for someone else's write
      }));
      await report('tl-f03-equal-text', f3 === 'adopt' && results.at(-1).result === 12 && await count() === before,
        1, results.at(-1).result);
    }

    stage = 5; /* F04: external takeover after a test write: stop, keep the external version */
    await pb('named-text', { text: 'original' });
    const f4 = await outcome(guarded(async g => {
      await g.write('test one');
      await pb('named-text', { text: 'user copy' }); before = await count();
      await g.write('test two');
    }));
    await report('tl-f04-takeover', f4 === 'write' && results.at(-1).result === 12 && await count() === before);

    stage = 6; /* F05: assertion / IPC failure inside a section settles (restores) first */
    for (const fault of ['assertion', 'ipc']) {
      await pb('named-multi');
      const f5 = await outcome(guarded(async g => {
        await g.write('synthetic');
        if (fault === 'ipc') await pb('no-such-action');
        throw new Error('assertion failed');
      }));
      await report(`tl-f05-${fault}`, f5 === 'error' && results.at(-1).result === 11, 1, results.at(-1).result);
    }

    stage = 7; /* F06: driver cancel while the window is unavailable: settle before teardown */
    await pb('named-multi');
    const f6id = await pb('guard-begin', { board: 1 });
    await pb('write', { text: 'written before the cancel' });
    await hideDeck();
    await waitFor(away, 5000, 50);
    await report('tl-f06-await-cancel', f6id > 0, 1, f6id); // driver: settle-request, then reopen
    const settledByDriver = await waitFor(async () => await pb('state') === 0, 30000, 100);
    const audit = String(await inv('smoke_pasteboard_audit')).split(';').map(row => row.split(',').map(Number));
    const f6 = audit.find(row => row[0] === f6id);
    await backFront(30000);
    await report('tl-f06-cancel-settled', settledByDriver && f6?.[5] === 11 && await pb('write', { text: 'x' }) < 0,
      1, f6?.[5] ?? -1);

    stage = 8; /* F08 (harness side): the general board was never written by this mode */
    await report('tl-f08-named-only', audit.every(row => row[1] === 1), audit.length);
    await metric('tl-front-regains', frontRegains);
    await report('done', !failed);
    /* F06 kill: a guard with a written board, then the driver SIGKILLs this
       process. The driver must classify it as unconfirmed, never success. */
    await pb('named-multi');
    const killId = await pb('guard-begin', { board: 1 });
    await pb('write', { text: 'written before the kill' });
    await report('tl-f06-await-kill', killId > 0, 1, killId);
  } catch (error) {
    await metric('tl-exception', stage, String(error?.message || '').length);
    await report('done', false, 1, stage);
  }
}
