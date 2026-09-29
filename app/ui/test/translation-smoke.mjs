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
import { addSplit, backToBoard, focusPane, openSession } from '../js/layout.js';
import { t, setLocale } from '../js/i18n.js';
import { activateTheme } from '../js/theme.js';
import { applyFontScale } from '../js/font-scale.js';
import { serializeSettings } from '../js/settings-model.js';
import { installTranslationSmokeBackend, packStatus, capability } from '../js/local-intelligence.js';
import { closeTranslationLens, translationLensMetrics, translationLensView, visibleViewport } from '../js/translation-lens.js';

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
const native = (kind, args = {}) => inv('smoke_native_input', { kind, viewport: window.innerHeight, ...args });
const center = el => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
async function click(el, point = center(el)) {
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
async function type(text) { for (const ch of text) await key(ch); }
const chord = () => key('t', ['command', 'shift']);
const snapshot = name => inv('smoke_native_snapshot', { name }).catch(() => -9);
// Native app state: bit 1 active, bit 2 key window (document.hasFocus() is
// not the app-in-front fact; see translation-lens.js).
const front = async () => ((await native('state')) & 3) === 3;
const away = async () => ((await native('state')) & 1) === 0;
async function ensureFront() {
  if (await front()) return true;
  await native('activate');
  return waitFor(front, 20000, 100);
}
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
async function toLive(pane, card) {
  await click(pane.term.element);
  if (provider.get(card.id)?.scrolled) {
    await key('Enter'); await waitFor(() => !provider.get(card.id)?.scrolled, 3000);
  }
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
    poll: async () => {
      if (!fake.armed || fake.version <= fake.baseline) return null;
      fake.baseline = fake.version; fake.reads++;
      const text = fake.clipboard;
      if (fake.pollDelay) await pause(fake.pollDelay);
      return text;
    },
    copy: async text => { fake.copied = text; fake.clipboard = text; fake.version++; },
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
    if (first) fake.settle(first, '静态译文');
    await report('tl-a01-display', await waitFor(() => resultText() === '静态译文' && isStatus('translation.ready')),
      1, statusCode() * 100 + fake.requests.filter(r => !r.done).length * 10 + fake.requests.length);
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
    if (second) fake.settle(second, '快捷键译文');
    await report('tl-a02-display', await waitFor(() => resultText() === '快捷键译文' || resultText().startsWith('译文〔')),
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
    await click($('terminal-host')); await type('/next'); await key('Enter');
    await report('tl-b05-click-no-pause', await waitFor(() => resultText().includes('explains how to restart')
      || fake.requests.some(r => r.done && r.text.includes('restart the server')), 5000)
      && await waitFor(() => isStatus('translation.ready'), 5000));
    const guard = await inv('smoke_pasteboard', { action: 'guard-begin' });
    await report('tl-e04-l2-guard', guard >= 0, guard + 1, guard);
    await click($('terminal-host')); await type('/stream 25'); await key('Enter');
    await waitFor(() => fake.requests.some(r => r.text.includes('Stream line')), 4000);
    const text = $('translation-result').getBoundingClientRect();
    const start = { x: text.left + 16, y: text.top + 20 }, end = { x: text.left + 200, y: text.top + 60 };
    await drag(start, end);
    const shownAtSelect = resultText();
    const selected = String(document.getSelection());
    await waitFor(() => lensMetrics().status === 'translation.held', 5000);
    await report('tl-b05-select-hold', resultText() === shownAtSelect && selected.includes('\n')
      && document.activeElement === $('translation-result'), selected.length, isStatus('translation.held') ? 1 : 0);
    await key('c', ['command']);
    const claim = guard >= 0 ? await inv('smoke_pasteboard', { action: 'claim', text: selected }) : -1;
    await report('tl-b05-cmd-c', claim === 0, trusted('copy'), claim);
    await key('Tab');
    await report('tl-b05-release', await waitFor(() => resultText() !== shownAtSelect, 5000), 1, lensMetrics().domWrites);
    await click($('translation-result')); await key('PageDown'); await key('ArrowDown');
    await quietTerminal(800);
    await report('tl-b05-keyboard', await waitFor(() => isStatus('translation.ready') || isStatus('translation.updating'), 4000)
      && !isStatus('translation.held'));
    const restored = guard >= 0 ? await inv('smoke_pasteboard', { action: 'guard-end' }) : -1;
    await report('tl-e04-l2-restored', restored === 0, 1, restored);

    /* B01: 5 s continuous output */
    stage = 5;
    await click($('terminal-host'));
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
    await click($('terminal-host')); await type('/stream 40'); await key('Enter');
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
    await click(paneB.term.element); await type("echo 'Beta pane text is visible here.'"); await key('Enter');
    await waitFor(() => viewportHas(paneB, 'Beta pane text is visible here.'), 6000);
    await click(paneA.term.element); await quietTerminal(600);
    for (let i = 0; i < 20; i++) { await wheelAt(paneA.term.element, 30); await pause(12); }
    await click(paneB.term.element);
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
    await click(paneB.term.element); await pause(300);
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
    await click(paneB.term.element);
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
    await click(paneA.term.element); await quietTerminal(600);

    /* A06: hide Deck (real resign-active), come back, Live continues unasked */
    stage = 8;
    await waitFor(() => isStatus('translation.ready'), 4000);
    await type('/next'); await key('Enter'); await pause(50);
    const blurs = blurCount;
    const hidden = await native('hide');
    await report('tl-a06-away', await waitFor(away, 5000, 50) && await waitFor(() => lensMetrics().status.startsWith('translation.background'), 3000),
      1, hidden * 10 + (blurCount > blurs ? 1 : 0));
    await report('tl-a06-await-return', true);
    await native('activate');
    const back = await waitFor(front, 30000, 100);
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
    const c7 = resultText(), c7Req = fake.requests.length;
    await click(paneA.term.element); await type('/next'); await key('Enter');
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
    await click(paneA.term.element); await type('/next'); await key('Enter'); await pause(900);
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
    await click(paneA.term.element);
    for (let t0 = performance.now(), round = 0; performance.now() - t0 < 30000; round++) {
      await type('/stream 12'); await key('Enter');
      for (let i = 0; i < 40; i++) { await wheelAt(paneA.term.element, i < 20 ? 30 : -30); await pause(12); }
      await leaveHistory();
      await click(tab('clipboard')); await pause(120); await click(tab('live'));
      await click(paneA.term.element); await pause(300);
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
    await click(paneC.term.element);
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
  let stage = 0, guard = -1, named = false;
  const own = async text => (guard >= 0 ? inv('smoke_pasteboard', { action: 'claim', text }) : -1);
  // The general pasteboard holds exactly the source of the shown translation.
  const ownShown = async () => (await own(lensSourceText())) === 0;
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
    await click($('terminal-host')); await type('/next'); await quietTerminal(700);
    const warmStart = lensMetrics().submitted, enterAt = performance.now(); await key('Enter');
    await report('tl-n-live-update', await shown(() => lensMetrics().submitted > warmStart
      && isStatus('translation.ready') && lensSourceText().includes(ANSWER_TWO) && han(resultText()) > 10, 30000));
    // The request whose accepted result is now shown (the answer's own).
    const shownRequest = lensMetrics().accepted.at(-1);
    await metric('tl-t-warm', shownRequest.settle - shownRequest.submit);

    /* A06 real: hide and return */
    stage = 3;
    await native('hide');
    await report('tl-n-a06-away', await waitFor(away, 5000, 50) && await waitFor(() => lensMetrics().status.startsWith('translation.background'), 3000));
    await report('tl-n-a06-await-return', true);
    await native('activate');
    const back = await waitFor(front, 30000, 100);
    await click($('terminal-host')); await type('/history 12'); await key('Enter');
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

    /* shared general pasteboard: guarded section */
    stage = 5;
    guard = await inv('smoke_pasteboard', { action: 'guard-begin' });
    await report('tl-n-guard', guard >= 0, guard + 1, guard);
    if (guard < 0) throw new Error('general pasteboard not preservable');
    await inv('smoke_pasteboard', { action: 'write', text: 'Deck harmless earlier copy before the tab.' });
    const c1 = lensMetrics().submitted, tabAt = performance.now();
    await click(tab('clipboard'));
    await report('tl-n-c01-ready', await waitFor(() => isStatus('translation.waiting'), 5000));
    await metric('tl-t-armed', firstAfter('armed', tabAt) - tabAt);
    await pause(1200);
    await report('tl-n-c01-old-unread', newRequests(c1) === 0 && !resultText());

    /* C02 real: controlled /copy right after the baseline */
    await click($('terminal-host')); await type('/copy');
    const copyAt = performance.now(); await key('Enter');
    const c2ok = await shown(() => han(resultText()) > 10 && isStatus('translation.ready'), 30000);
    const copiedAt = firstAfter('copied', copyAt), shownAt = firstAfter('display', copiedAt);
    // The fixture shows answer two since the Live update's /next.
    const c2own = c2ok && await ownShown(); // claim only a copy the Lens observed
    await report('tl-n-c02-copy', c2ok && lensSourceText().startsWith(ANSWER_TWO) && c2own,
      Math.round(shownAt - copyAt), (c2ok ? 1 : 0) + (lensSourceText().startsWith(ANSWER_TWO) ? 2 : 0)
        + (lensSourceText().startsWith(ANSWER_ONE) ? 4 : 0) + (c2own ? 8 : 0) + statusCode() * 100
        + (status().length > 0 && statusCode() === 99 ? 0 : 0));
    if (!c2ok) await snapshot('l3-debug-c02');
    await metric('tl-t-copy-accept', copiedAt - copyAt);
    await metric('tl-t-copy-display', shownAt - copiedAt);
    await snapshot('l3-copied-real');
    const c2Result = resultText();

    /* C02 near arm: re-enter the tab and /copy the next answer as soon as it is armed */
    await type('/next'); await key('Enter'); await pause(300);
    await click(tab('live')); await click(tab('clipboard'));
    await waitFor(() => isStatus('translation.waiting'), 5000, 5);
    const armedAt = lensMetrics().last.armed;
    await click($('terminal-host')); await type('/copy'); await key('Enter');
    const nearGap = performance.now() - armedAt;
    await report('tl-n-c02-near-arm', await shown(() => han(resultText()) > 5 && isStatus('translation.ready')
      && resultText() !== c2Result, 30000) && lensSourceText().startsWith(ANSWER_ONE) && await ownShown(), Math.round(nearGap));

    /* C07 real: the copied snapshot ignores terminal output and scroll */
    const c7 = resultText(), c7r = lensMetrics().submitted;
    await type('/history 30'); await key('Enter');
    for (let i = 0; i < 30; i++) { await wheelAt($('terminal-host'), 30); await pause(12); }
    await pause(1200); await leaveHistory();
    await report('tl-n-c07-static', resultText() === c7 && newRequests(c7r) === 0);

    /* C05 real: Deck's own copies never feed back; the next real copy works */
    const c5 = lensMetrics().submitted;
    await click($('translation-copy')); await pause(1200);
    const copyTranslationOwned = await own(c7);
    await click($('translation-copy-source')); await pause(1200);
    const copySourceOwned = await own(lensSourceText());
    await report('tl-n-c05-no-feedback', newRequests(c5) === 0 && copyTranslationOwned === 0 && copySourceOwned === 0,
      1, copyTranslationOwned * 10 + copySourceOwned);
    await click($('terminal-host')); await type('/next'); await key('Enter'); await pause(300);
    await type('/copy'); await key('Enter');
    const c5ok = await shown(() => resultText() !== c7 && isStatus('translation.ready'), 30000);
    const c5own = c5ok && await ownShown();
    await report('tl-n-c05-next-copy', c5ok && c5own, 1, (c5ok ? 1 : 0) + (c5own ? 2 : 0) + statusCode() * 100);

    /* C04 real: copy while Deck is away (driver's pbcopy) is never read */
    stage = 6;
    const c4 = lensMetrics().submitted;
    await native('hide');
    await waitFor(away, 5000, 50);
    await report('tl-n-c04-await-away', true); // driver: pbcopy AWAY_TEXT, then reopen the bundle
    const returned = await waitFor(front, 45000, 100);
    await waitFor(() => isStatus('translation.ready') || isStatus('translation.waiting'), 5000);
    const awayOwned = await own('Deck harmless text copied while Deck was away.');
    await pause(1500);
    await report('tl-n-c04-away-unread', returned && newRequests(c4) === 0 && awayOwned === 0, 1, awayOwned);
    await click($('terminal-host')); await type('/next'); await key('Enter'); await pause(300);
    await type('/copy'); await key('Enter');
    await report('tl-n-c04-after-return', await shown(() => newRequests(c4) === 1 && isStatus('translation.ready'), 30000)
      && await ownShown());
    await click($('translation-close')); await click($('translation-btn'));
    const reopened = lensMetrics().submitted;
    await click(tab('clipboard')); await waitFor(() => isStatus('translation.waiting'), 5000); await pause(1200);
    await report('tl-n-c04-reopen-unread', newRequests(reopened) <= 1 && !resultText(), newRequests(reopened));
    const restored = await inv('smoke_pasteboard', { action: 'guard-end' }); guard = -1;
    await report('tl-n-guard-restored', restored === 0, 1, restored);

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
      await click($('terminal-host')); await type('/next'); await key('Enter');
      await report(`tl-n-d07-${step}`, await waitFor(() => isStatus(`translation.error.${expected}`), 15000));
    }
    await report('tl-n-d07-await-restore', true);
    await waitFor(async () => (await packStatus()).installed, 30000, 200);
    await click($('terminal-host')); await type('/next'); await key('Enter');
    await report('tl-n-d07-restored', await shown(() => isStatus('translation.ready'), 60000));

    /* C07 real selection snapshot: native drag in the terminal, shortcut */
    stage = 11;
    await quietTerminal(600);
    const rows = $('terminal-host').querySelector('.xterm-screen').getBoundingClientRect();
    await drag({ x: rows.left + 4, y: rows.top + 6 }, { x: rows.left + rows.width * 0.8, y: rows.top + rows.height * 0.3 }, 8);
    await chord();
    await report('tl-n-c07-selection', await shown(() => isStatus('translation.selected'), 30000)
      && lensSourceText().length > 0, trusted('pointerdown'));
    closeTranslationLens(); backToBoard();
    await saveTranslationSettings({ enabled: false });
    window.dispatchEvent(new Event('deck-translation-disabled'));
    await metric('tl-l3-native-events', trusted('pointerdown') + trusted('wheel') + trusted('keydown'),
      untrusted('pointerdown') + untrusted('wheel'));
    await report('done', !failed);
  } catch (error) {
    await metric('tl-exception', stage, String(error?.message || '').length);
    if (named) await inv('smoke_pasteboard', { action: 'named-off' }).catch(() => {});
    if (guard >= 0) {
      const restored = await inv('smoke_pasteboard', { action: 'guard-end' }).catch(() => -9);
      await report('tl-n-guard-restored', restored === 0, 1, restored);
    }
    await report('done', false, 1, stage);
  }
}
