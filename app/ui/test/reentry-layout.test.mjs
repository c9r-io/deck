// The layout items of the `reentry` carrier (reentry-layout.mjs), run in
// Node: the item tables are closed and collision-free, the terms are the
// carrier's `reachable()`, every value is an integer, the lines read back —
// and, against the real manifest and the real verdict, the items can neither
// fail a run nor pass one. The carrier itself is read as text: its assertion
// and its report line are the ones the manifest judges, the items are read in
// the tick of the assertion, and nothing the items find can reach `laidOut`.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { ALL_CONDITIONS, LAYOUT_BUTTONS, LAYOUT_CONDITIONS, LAYOUT_ITEMS, LAYOUT_NODE, LAYOUT_NODES, LAYOUT_SETTLED, LAYOUT_TABLE, NODE_TABLE,
  STATUS_STATES, describeLayout, insideBits, layoutConditions, layoutItems, measureLayout, readLayout, statusItems } from './reentry-layout.mjs';
import { judge, loadManifest } from '../../../scripts/smoke-verdict.mjs';

const carrier = readFileSync(new URL('./reentry-smoke.mjs', import.meta.url), 'utf8');
const helper = readFileSync(new URL('./reentry-layout.mjs', import.meta.url), 'utf8');
const manifest = loadManifest();

const rect = (left, top, right, bottom) => ({ left, top, right, bottom, width: right - left, height: bottom - top });
/* a 1280 × 772 window in which everything `reachable()` asks for holds */
const good = () => ({
  innerWidth: 1280, innerHeight: 772, clientWidth: 1280, clientHeight: 772, pixelRatio: 2, rootFont: 13, media: 0, page: 3,
  frame: rect(360, 61.76, 920, 700.5), frameOverflow: 0, frameState: 1, frameMaxHeight: 648.48, frameMarginTop: 61.76,
  body: rect(379, 110, 901, 630), bodyScrollHeight: 640, bodyClientHeight: 520, bodyScrollTop: 0, bodyScrollWidth: 522, bodyClientWidth: 522,
  action: rect(700, 650, 900, 680), row: rect(379, 646, 901, 682), shown: 7, disabled: 0,
  section: { scrollWidth: 512, clientWidth: 512, rect: rect(379, 200, 891, 760), state: 0 },
  nodes: [
    { kind: 1, rect: rect(379, 400, 891, 600), state: 0 },
    { kind: 2, rect: rect(379, 610, 891, 630), state: 0 },
    { kind: 2, rect: rect(379, 632, 891, 652), state: 0 },
    { kind: 2, rect: rect(379, 654, 891, 674), state: 0 },
    { kind: 2, rect: rect(379, 676, 891, 696), state: 0 },
    { kind: 3, rect: rect(379, 700, 891, 720), state: 0 },
  ],
});
/* the carrier's expression, written out once more over plain numbers */
const reference = g => {
  const { frame, body, action } = g;
  const inside = r => r.width > 0 && r.height > 0 && r.left >= frame.left - 1 && r.right <= frame.right + 1;
  return frame.top >= 0 && frame.bottom <= g.innerHeight + 1 && frame.left >= 0 && frame.right <= g.innerWidth + 1
    && inside(action) && action.bottom <= frame.bottom + 1 && action.top >= body.bottom - 1
    && g.section.scrollWidth <= g.section.clientWidth + 1
    && g.nodes.every(node => inside(node.rect));
};
const value = (pairs, item) => pairs.find(([at]) => at === item)?.[1];

/* A stand-in page laid out as a measurement says. Every property that holds a text of a real page throws when read. */
const TEXT = ['textContent', 'innerText', 'innerHTML', 'outerHTML', 'value', 'title', 'name', 'id', 'className', 'dataset', 'nodeValue'];
const element = (box, style = {}, extra = {}) => {
  const node = {
    getBoundingClientRect: () => ({ ...box, x: box.left, y: box.top }),
    getClientRects: () => (box.width > 0 || box.height > 0 ? [box] : []),
    hidden: false, disabled: false,
    style: { display: 'block', visibility: 'visible', overflowY: 'visible', maxHeight: 'none', marginTop: '0px', fontSize: '13px', ...style },
    ...extra,
  };
  for (const key of TEXT) Object.defineProperty(node, key, { get() { throw new Error(`a text of the page was read: ${key}`); } });
  return node;
};
const page = (g, change = {}) => {
  const frame = element(g.frame, { display: 'flex', maxHeight: g.frameMaxHeight < 0 ? 'none' : `${g.frameMaxHeight}px`, marginTop: `${g.frameMarginTop}px` },
    { scrollHeight: 640 + g.frameOverflow, clientHeight: 640, classList: { contains: name => name === 'reentry' && (g.frameState & 1) === 1 } });
  const body = element(g.body, { overflowY: 'auto' }, { scrollHeight: g.bodyScrollHeight, clientHeight: g.bodyClientHeight,
    scrollTop: g.bodyScrollTop, scrollWidth: g.bodyScrollWidth, clientWidth: g.bodyClientWidth });
  const buttons = {
    'tmux-view-sessions': element(rect(379, 650, 540, 680)),
    'tmux-later': element(rect(620, 650, 690, 680)),
    'tmux-restart': element(g.action, {}, { parentElement: element(g.row) }),
    'tmux-done': element(rect(0, 0, 0, 0), { display: 'none' }),
  };
  const nodes = g.nodes.map(node => element(node.rect, {}, node.kind === 13 ? { open: true } : {}));
  const part = element(g.section.rect, {}, {
    scrollWidth: g.section.scrollWidth, clientWidth: g.section.clientWidth,
    querySelector: selector => (selector === '.reentry-policy' ? nodes[0] : selector === 'details' ? nodes[nodes.length - 1] : null),
    querySelectorAll: selector => (selector === '.reentry-scope-note' ? nodes.slice(1, -1) : []),
  });
  const view = {
    byId: id => ({ 'tmux-lifecycle-box': frame, 'tmux-lifecycle-body': body, ...buttons })[id],
    part,
    win: {
      innerWidth: g.innerWidth, innerHeight: g.innerHeight, devicePixelRatio: g.pixelRatio,
      getComputedStyle: node => node.style,
      matchMedia: query => ({ matches: query === '(max-height: 540px)' && g.innerHeight <= 540 }),
      document: { documentElement: element(rect(0, 0, g.innerWidth, g.innerHeight), { fontSize: `${g.rootFont}px` },
        { clientWidth: g.clientWidth, clientHeight: g.clientHeight }), visibilityState: 'visible', hasFocus: () => true },
    },
  };
  change.page?.({ frame, body, buttons, nodes, part, view });
  return view;
};
const line = (name, a, b = 0) => `1791584890 [ui] smoke-check ${name} a=${a} b=${b}`;

test('the item tables are closed: every item once, fixed items below the nodes, nodes below the settled offset', () => {
  const items = LAYOUT_TABLE.map(([item]) => item);
  assert.equal(new Set(items).size, items.length);
  assert.equal(new Set(LAYOUT_TABLE.map(([, name]) => name)).size, items.length);
  assert.ok(items.every(item => Number.isInteger(item) && item >= 1 && item < LAYOUT_NODE));
  assert.ok(LAYOUT_TABLE.every(([, , unit]) => ['px', 'n', 'bits'].includes(unit)));
  assert.deepEqual(LAYOUT_ITEMS, Object.fromEntries(LAYOUT_TABLE.map(([item, name]) => [name, item])));
  const offsets = NODE_TABLE.map(([at]) => at);
  assert.deepEqual(offsets, [0, 1, 2, 3, 4, 5, 6]);
  assert.ok(LAYOUT_NODE * LAYOUT_NODES + Math.max(...offsets) < LAYOUT_SETTLED, 'a node item never looks like a settled one');
  assert.equal(LAYOUT_CONDITIONS.length, 9);
  assert.equal(ALL_CONDITIONS, 511);
});

test('each term of reachable() has its own bit, and breaking one term clears that bit alone', () => {
  assert.equal(layoutConditions(good()), ALL_CONDITIONS);
  const breaks = [
    g => { g.frame = rect(360, -2, 920, 700.5); },                              // frame.top >= 0
    g => { g.innerHeight = 690; },                                               // frame.bottom <= innerHeight + 1
    g => { g.frame = rect(-5, 61.76, 920, 700.5); },                            // frame.left >= 0
    g => { g.innerWidth = 900; },                                                // frame.right <= innerWidth + 1
    g => { g.action = rect(700, 650, 925, 680); },                               // inside(action)
    g => { g.action = rect(700, 650, 900, 702); },                               // action.bottom <= frame.bottom + 1
    g => { g.action = rect(700, 628, 900, 680); },                               // action.top >= body.bottom - 1
    g => { g.section.scrollWidth = 514; },                                       // no horizontal overflow of the section
    g => { g.nodes[3].rect = rect(379, 654, 891, 654); },                        // every checked node is inside (a note without height)
  ];
  breaks.forEach((spoil, at) => {
    const g = good();
    spoil(g);
    assert.equal(layoutConditions(g), ALL_CONDITIONS & ~(1 << at), LAYOUT_CONDITIONS[at]);
    assert.equal(reference(g), false, LAYOUT_CONDITIONS[at]);
  });
  // the one-pixel allowances are the carrier's
  const edge = good();
  edge.action = rect(359, 629, 921, 701.5);
  edge.section.scrollWidth = 513;
  assert.equal(layoutConditions(edge), ALL_CONDITIONS);
  assert.equal(insideBits(rect(359, 0, 921, 10), edge.frame), 15);
  assert.equal(insideBits(rect(358.9, 0, 921.1, 10), edge.frame), 3);
  assert.equal(insideBits(rect(400, 5, 400, 5), edge.frame), 12);
});

test('"every term holds" is exactly what the carrier asserts, over many measurements', () => {
  let seed = 20261010;
  const random = () => { seed = (seed * 1664525 + 1013904223) % 4294967296; return seed / 4294967296; };
  const nudge = (r, by) => rect(r.left + by(), r.top + by(), r.right + by(), r.bottom + by());
  const seen = { true: 0, false: 0 };
  for (let round = 0; round < 4000; round += 1) {
    const g = good();
    const wide = round % 4 === 0 ? 60 : 2.5;            // mostly near the allowances, sometimes far outside
    const by = () => (random() < 0.7 ? 0 : (random() - 0.5) * wide);
    g.frame = nudge(g.frame, by); g.body = nudge(g.body, by); g.action = nudge(g.action, by);
    g.innerWidth += Math.round(by()); g.innerHeight += Math.round(by() * 30);
    g.section.scrollWidth += Math.round(by());
    g.nodes = g.nodes.map(node => ({ ...node, rect: random() < 0.03 ? rect(node.rect.left, node.rect.top, node.rect.left, node.rect.top) : nudge(node.rect, by) }));
    const held = reference(g);
    assert.equal(layoutConditions(g) === ALL_CONDITIONS, held);
    seen[held] += 1;
    const pairs = layoutItems(g, held ? 1 : 0);
    assert.equal(value(pairs, LAYOUT_ITEMS.agrees), 1);
  }
  assert.ok(seen.true > 200 && seen.false > 200, `both outcomes were tried: ${JSON.stringify(seen)}`);
});

test('one sample is pairs of integers: lengths in pixels × 100, counts, bits', () => {
  const pairs = layoutItems(good(), 1);
  assert.ok(pairs.every(pair => pair.length === 2 && pair.every(Number.isInteger)));
  assert.equal(new Set(pairs.map(([item]) => item)).size, pairs.length, 'an item is written once per sample');
  const I = LAYOUT_ITEMS;
  assert.deepEqual([I.taken, I.asserted, I.conditions, I.agrees, I.frames].map(item => value(pairs, item)), [1, 1, 511, 1, -1]);
  assert.deepEqual([I.innerWidth, I.innerHeight, I.pixelRatio, I.rootFont].map(item => value(pairs, item)), [1280, 772, 200, 1300]);
  assert.deepEqual([I.frameLeft, I.frameTop, I.frameRight, I.frameBottom].map(item => value(pairs, item)), [36000, 6176, 92000, 70050]);
  assert.deepEqual([I.frameMaxHeight, I.frameMarginTop].map(item => value(pairs, item)), [64848, 6176]);
  assert.deepEqual([I.sectionScrollWidth, I.sectionClientWidth, I.sectionOverflow].map(item => value(pairs, item)), [512, 512, 0]);
  assert.deepEqual([I.nodes, I.scopeNotes, I.nodesOutside].map(item => value(pairs, item)), [6, 4, 0]);
  // node 1 is the policy, node 6 the details; each with its box and the four parts of `inside`
  assert.deepEqual([100, 101, 102, 103, 104, 105, 106].map(item => value(pairs, item)), [1, 37900, 40000, 89100, 60000, 15, 0]);
  assert.equal(value(pairs, 600), 3);
  assert.equal(value(pairs, 700), undefined);
  // the settled sample says whether its frames ran; an assertion that could not be asked is -1, and so is the agreement
  assert.equal(value(layoutItems(good(), 1, 0), I.frames), 0);
  assert.deepEqual([I.asserted, I.agrees].map(item => value(layoutItems(good(), -1, 1), item)), [-1, -1]);
  assert.equal(value(layoutItems(good(), 0), I.agrees), 0, 'the items say it holds, the assertion said it did not');
  // no maximum height, and a number that is not one
  const odd = good();
  odd.frameMaxHeight = -1;
  odd.rootFont = Number.NaN;
  assert.deepEqual([I.frameMaxHeight, I.rootFont].map(item => value(layoutItems(odd, 1), item)), [-1, -1]);
});

test('more nodes than are itemised are still counted, and a missing node is not inside', () => {
  const g = good();
  g.nodes = [...g.nodes, ...Array.from({ length: 5 }, () => ({ kind: 2, rect: rect(379, 0, 950, 10), state: 0 }))];
  const pairs = layoutItems(g, 0);
  assert.deepEqual([LAYOUT_ITEMS.nodes, LAYOUT_ITEMS.scopeNotes, LAYOUT_ITEMS.nodesOutside].map(item => value(pairs, item)), [11, 9, 5]);
  assert.equal(Math.max(...pairs.map(([item]) => item)), LAYOUT_NODE * LAYOUT_NODES + 6);
  assert.equal(value(pairs, LAYOUT_NODE * 7 + 5), 7, 'the seventh node is too wide on the right');
  const missing = good();
  missing.nodes[5] = { kind: 0, rect: rect(0, 0, 0, 0), state: 8 };
  assert.equal(layoutConditions(missing), ALL_CONDITIONS & ~(1 << 8));
  // kind 0, an empty box at the origin (no size, left of the frame; only "right ≤ frame.right + 1" holds), no box at all
  assert.deepEqual([600, 605, 606].map(item => value(layoutItems(missing, 0), item)), [0, 8, 8]);
});

test('the real status is flags, a closed state and two counts', () => {
  const I = LAYOUT_ITEMS;
  const pairs = statusItems({ status: 'LegacyUnknown', pendingRestart: true, shouldPrompt: true, canRestart: true, serverPid: 4242,
    notice: null, restartBlockers: [], sessionCount: 0, paneCount: 0, sessions: [{ name: 'not-a-value' }] });
  assert.deepEqual(pairs, [[I.statusFlags, 15], [I.statusState, 4], [I.statusSessions, 0], [I.statusPanes, 0]]);
  assert.deepEqual(statusItems({ status: 'CompatibleCurrentBuild', canRestart: false, serverPid: null, notice: 'restartCompleted',
    restartBlockers: [{}], sessionCount: 3, paneCount: 5 }), [[I.statusFlags, 48], [I.statusState, 1], [I.statusSessions, 3], [I.statusPanes, 5]]);
  for (const nothing of [null, undefined, 'text', 7, {}]) {
    assert.deepEqual(statusItems(nothing), [[I.statusFlags, 0], [I.statusState, 0], [I.statusSessions, -1], [I.statusPanes, -1]]);
  }
  assert.equal(STATUS_STATES.length, 6);
  const native = readFileSync(new URL('../../src-tauri/src/tmux_lifecycle.rs', import.meta.url), 'utf8');
  const states = native.slice(native.indexOf('pub(crate) enum CompatibilityState {'));
  assert.deepEqual(states.slice(states.indexOf('{') + 1, states.indexOf('}')).split(',').map(name => name.trim()).filter(Boolean), [...STATUS_STATES]);
});

test('the lines read back, per language and sample, and say which term failed', () => {
  const failing = good();
  failing.innerHeight = 600;
  const log = [
    line('reentry-layout', -4, 1280),
    ...layoutItems(failing, 0).map(([item, v]) => line('reentry-layout-en', item, v)),
    ...layoutItems(good(), 1, 1).map(([item, v]) => line('reentry-layout-zh', LAYOUT_SETTLED + item, v)),
    '1791584893 [ui] smoke-check done a=-1 b=0',
  ].join('\n');
  const read = readLayout(log);
  assert.deepEqual([...read.en.asserted], layoutItems(failing, 0));
  assert.deepEqual([...read.zh.settled], layoutItems(good(), 1, 1));
  assert.equal(read.en.settled.size + read.zh.asserted.size, 0);
  const text = describeLayout(log);
  assert.match(text, /English, in the tick of the assertion:\n {2}reachable\(\) = 0; terms that do not hold: frame\.bottom <= innerHeight \+ 1\n/);
  assert.match(text, /Chinese, after the page settled:\n {2}reachable\(\) = 1; terms that do not hold: none\n/);
  assert.match(text, / {4}21 frameTop = 61\.76\n/);
  assert.match(text, / {3}105 node 1 inside = 15\n/);
  assert.match(text, /Chinese, in the tick of the assertion:\n {2}\(no line\)/);
  assert.doesNotMatch(text, /DISAGREE/);
  assert.match(describeLayout(layoutItems(good(), 0).map(([item, v]) => line('reentry-layout-en', item, v)).join('\n')), /DISAGREE/);
  assert.match(describeLayout(line('reentry-layout-zh', 1, 0)), /Chinese, in the tick of the assertion:\n {2}the sampling threw/);
});

test('against the real manifest and verdict the items neither fail a run nor pass one', () => {
  const spec = manifest.modes.reentry;
  assert.deepEqual(spec.checks['reentry-layout'], { a: 7 }, 'the assertion keeps its expectation');
  assert.deepEqual([spec.checks['reentry-layout-en'], spec.checks['reentry-layout-zh']], [{ metric: true }, { metric: true }]);
  const metricNames = ['reentry-layout-en', 'reentry-layout-zh'];
  const judged = Object.entries(spec.checks).filter(([name, rule]) => !rule.failureOnly && !metricNames.includes(name));
  const passing = judged.map(([name, rule]) => line(name, typeof rule.a === 'number' ? rule.a : 1, rule.b ?? 0));
  const items = (g, asserted) => metricNames.flatMap(name => layoutItems(g, asserted).map(([item, v]) => line(name, item, v)));
  const broken = good();
  broken.frame = rect(360, -400, 920, 3000);
  broken.nodes = [];
  const run = (checks, measured, done = 1) => judge([...checks, ...measured, line('done', done)].join('\n'), manifest, 'reentry');

  // a passing run stays passing whatever the items say
  assert.equal(run(passing, items(good(), 1)).ok, true);
  assert.equal(run(passing, items(broken, 0)).ok, true);
  assert.equal(run(passing, metricNames.map(name => line(name, -999999, -999999))).ok, true);
  // a failing run stays failing whatever the items say, with exactly its own failures
  const failingChecks = passing.map(text => (text.includes(' reentry-layout a=') ? line('reentry-layout', -4, 1280) : text));
  for (const measured of [items(good(), 1), items(broken, 0)]) {
    const verdict = run(failingChecks, measured, -1);
    assert.equal(verdict.ok, false);
    assert.deepEqual(verdict.failures, ['reentry-layout: a=-4 (expected 7)', 'done: a=-1 b=0']);
  }
  // items that say "all holds" do not rescue a wrong sum either
  assert.deepEqual(run(failingChecks, items(good(), 1), 1).failures, ['reentry-layout: a=-4 (expected 7)']);
  // a carrier that stopped writing them is caught
  assert.deepEqual(run(passing, []).failures, ['reentry-layout-en: missing', 'reentry-layout-zh: missing']);
  assert.equal(run(passing, items(good(), 1)).total, judged.length + metricNames.length + 1);
});

test('the carrier keeps its assertion, reads the items in the same tick and lets nothing they find reach it', () => {
  // the assertion and its report, as the manifest judges them
  const assertion = [
    "    const reachable = () => {",
    "      const body = $('tmux-lifecycle-body').getBoundingClientRect();",
    "      const frame = $('tmux-lifecycle-box').getBoundingClientRect();",
    "      const action = $('tmux-restart').getBoundingClientRect();",
    "      const inside = rect => rect.width > 0 && rect.height > 0 && rect.left >= frame.left - 1 && rect.right <= frame.right + 1;",
    "      return frame.top >= 0 && frame.bottom <= innerHeight + 1 && frame.left >= 0 && frame.right <= innerWidth + 1",
    "        && inside(action) && action.bottom <= frame.bottom + 1 && action.top >= body.bottom - 1",
    "        && section().scrollWidth <= section().clientWidth + 1",
    "        && [section().querySelector('.reentry-policy'), ...all(section(), '.reentry-scope-note'), section().querySelector('details')]",
    "          .every(node => inside(node.getBoundingClientRect()));",
    "    };",
  ].join('\n');
  assert.ok(carrier.includes(assertion), 'reachable() is the function it was');
  assert.ok(carrier.includes("    await report('reentry-layout', laidOut === 3, 4 + laidOut, innerWidth);\n"));
  // `laidOut` is written in exactly two places, and both are the assertion's own
  assert.deepEqual(carrier.match(/^.*\blaidOut\s*(?:\+=|=(?!=)).*$/gm).map(text => text.trim()),
    ['let laidOut = reachable() ? 1 : 0;', 'laidOut += reachable() ? 2 : 0;']);
  // each language's items are read right after its assertion, with nothing awaited in between
  const between = (from, to) => { const start = carrier.indexOf(from); const end = carrier.indexOf(to, start); assert.ok(start >= 0 && end > start, to); return carrier.slice(start, end); };
  assert.doesNotMatch(between('let laidOut = reachable() ? 1 : 0;', 'sampleLayout(laidOut & 1)'), /\bawait\b|setLocale|pause\(/);
  assert.doesNotMatch(between('laidOut += reachable() ? 2 : 0;', 'sampleLayout(laidOut >> 1)'), /\bawait\b|setLocale|pause\(/);
  // everything that waits or reads again comes after the last write of `laidOut`
  const last = carrier.indexOf('laidOut += reachable() ? 2 : 0;');
  for (const later of ['await settled()', 'await tellLayout(', 'const settled = async']) assert.ok(carrier.indexOf(later) > last, later);
  assert.ok(carrier.indexOf('await tellLayout(') > carrier.indexOf("await report('reentry-layout', "), 'the items follow the sum');
  // a measurement never touches `failed`: the only writer is report()
  assert.deepEqual(carrier.match(/^.*\bfailed\s*=(?!=).*$/gm).map(text => text.trim()), ['let failed = false;', 'if (!ok) failed = true;']);
  // the two names are the only ones a measurement is written under
  assert.deepEqual(carrier.match(/\bmetric\('[^']*'/g), ["metric('reentry-layout-en'", "metric('reentry-layout-zh'"]);
});

test('the measuring reads a page into exactly the measurement it shows, through the view it is handed', () => {
  assert.deepEqual(measureLayout(page(good())), good());
  assert.deepEqual(layoutItems(measureLayout(page(good())), 1), layoutItems(good(), 1));
  assert.deepEqual(LAYOUT_BUTTONS, ['tmux-view-sessions', 'tmux-later', 'tmux-restart', 'tmux-done']);
  // an open details, a note that is not shown, a policy that is not there, the result's "Done" in place of the review's buttons
  const other = good();
  other.nodes[5].kind = 13;
  const measured = measureLayout(page(other, { page: ({ buttons, nodes, part }) => {
    nodes[2].style.display = 'none';
    nodes[2].getClientRects = () => [];
    nodes[3].hidden = true;
    nodes[4].style.visibility = 'hidden';
    const found = part.querySelector;
    part.querySelector = selector => (selector === '.reentry-policy' ? null : found(selector));
    for (const id of ['tmux-view-sessions', 'tmux-later', 'tmux-restart']) { buttons[id].getClientRects = () => []; buttons[id].disabled = true; }
    buttons['tmux-done'].getClientRects = () => [rect(800, 650, 900, 680)];
  } }));
  assert.deepEqual(measured.nodes.map(node => [node.kind, node.state]), [[0, 8], [2, 0], [2, 9], [2, 4], [2, 2], [13, 0]]);
  assert.deepEqual([measured.shown, measured.disabled], [8, 7]);
  // a short window is the small-viewport block of the stylesheet; a frame that scrolls and has lost its class says so
  const short = good();
  short.innerHeight = 500;
  short.frameState = 0;
  short.frameOverflow = 37;
  const small = measureLayout(page(short, { page: ({ frame, view }) => { frame.style.overflowY = 'auto'; view.win.document.visibilityState = 'hidden'; view.win.document.hasFocus = () => false; } }));
  assert.deepEqual([small.media, small.page, small.frameState, small.frameOverflow], [2, 0, 2, 37]);
  const unbounded = good();
  unbounded.frameMaxHeight = -1;
  assert.equal(measureLayout(page(unbounded)).frameMaxHeight, -1);
});

test('what is measured is boxes, styles and flags: no text of the page is read for an item', () => {
  // every stand-in element above throws when a text property is read; so does the section itself
  assert.throws(() => page(good()).part.textContent, /a text of the page was read/);
  assert.doesNotThrow(() => measureLayout(page(good())));
  const measuring = helper.slice(helper.indexOf('export function measureLayout'), helper.indexOf('const box = '));
  assert.ok(measuring.length > 500);
  assert.doesNotMatch(measuring, /textContent|innerText|innerHTML|outerHTML|\.value\b|getAttribute|\.title\b|\.name\b|dataset|\.id\b|className\b|JSON\.stringify/);
  // the carrier hands it this page and nothing else, and keeps a sampling that throws to itself
  assert.ok(carrier.includes("try { return layoutItems(measureLayout({ byId: $, part: section(), win: window }), asserted, frames); } catch (_) { return [[LAYOUT_ITEMS.taken, 0]]; }"));
  // the helper builds numbers and nothing else out of a measurement
  assert.doesNotMatch(helper.slice(helper.indexOf('export function layoutItems'), helper.indexOf('/* ---------- reading the lines back')), /String\(|toString|`/);
  // and what reaches the backend is truncated to two integers
  assert.ok(carrier.includes("const metric = (name, a = 0, b = 0) => nativeInvoke('ui_event', { code: 'smoke-check', detail: name, a: Math.trunc(a), b: Math.trunc(b) });"));
});

test('a choice in the dialog is a checkbox or a radio: its own rule takes it out of the general input width, and no rule of the section clips', () => {
  const css = readFileSync(new URL('../style.css', import.meta.url), 'utf8');
  const rules = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)]
    .map(found => [found[1].replace(/\/\*[\s\S]*?\*\//g, '').trim().replace(/\s+/g, ' '), found[2].trim().replace(/\s+/g, ' ')]);
  const of = selector => rules.filter(([name]) => name === selector).map(([, declarations]) => declarations);
  // the general rule, as it is for every other field: an input is as wide as its row unless its own rule says otherwise
  assert.deepEqual(of('input'), ['font: inherit; color: var(--text); background: var(--bg); border: 1px solid var(--border); border-radius: 6px; padding: 7px 10px; width: 100%;']);
  // the dialog's choices say otherwise, in one rule of their own
  assert.deepEqual(of('.reentry-choice input'), ['flex: none; width: auto; margin: 0; align-self: center;']);
  // every input the dialog makes is such a choice: the one `input` it creates is the first child of a .reentry-choice label
  const dialog = readFileSync(new URL('../js/restart-reentry.js', import.meta.url), 'utf8');
  assert.equal(dialog.match(/el\('input'/g).length, 1);
  assert.ok(dialog.includes("  const row = el('label', 'reentry-choice');\n  const input = el('input');\n  input.type = type;"));
  assert.ok(dialog.includes("  row.append(input, el('span', 'reentry-choice-text', label));"));
  // the section the carrier measures (scrollWidth against clientWidth) has no overflow of its own, and no rule
  // that names the section or its parts clips or hides anything sideways: content that did not fit would still
  // be measured as not fitting. The two lists scroll downwards inside themselves, as before.
  assert.ok(carrier.includes("const section = () => $('tmux-reentry');"));
  assert.deepEqual(of('#tmux-reentry'), ['margin-top: 12px; padding-top: 12px; border-top: 1px solid var(--border-soft); font-size: 0.923077rem;']);
  const own = rules.filter(([name]) => /reentry/.test(name));
  assert.ok(own.length >= 25, 'the rules of the section were found');
  const clips = own.filter(([, declarations]) => /overflow(?!-wrap)|clip|contain\b|text-overflow/.test(declarations)).map(([name, declarations]) => `${name}: ${declarations.match(/overflow[a-z-]*: *[^;]+/)?.[0]}`);
  assert.deepEqual(clips, ['.reentry-list: overflow-y: auto']);
});
