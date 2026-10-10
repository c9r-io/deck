// The layout items of the `reentry` carrier (reentry-smoke.mjs): what its
// `reentry-layout` checkpoint is made of, written as numbers.
//
// `reentry-layout` reports one sum: how many of the two languages kept the
// note, the policy and the confirming action reachable. When it fails, that
// sum says neither which condition failed nor by how much. The carrier
// therefore also writes, for the English and for the Chinese modal,
//
//     [ui] smoke-check reentry-layout-en a=<item> b=<value>
//     [ui] smoke-check reentry-layout-zh a=<item> b=<value>
//
// one line per item. An item is a number from the closed tables below; a
// value is an integer: a length in CSS pixels × 100, a count, or a set of
// bits. No text of the page and no card, session, path or id is ever a
// value, and those two names are the only identifiers. In the manifest both
// are `metric`: their lines are never compared with anything, so they can
// neither fail a run nor pass one. The assertion stays `reentry-layout`.
//
// Each language is sampled twice: in the tick `reachable()` is asserted (the
// items as numbered) and once more after the page had time to settle (the
// same items + LAYOUT_SETTLED), so a layout that was still moving shows.
//
// This file names no global of the page: `measureLayout` reads boxes, styles
// and flags through the view the carrier hands it, the rest is arithmetic.
// reentry-layout.test.mjs runs all of it in Node, the measuring against a
// stand-in page whose text properties throw when read.

/** Added to every item of the second, settled sample. */
export const LAYOUT_SETTLED = 1000;
/** The k-th checked node's items start at LAYOUT_NODE × k (k from 1). */
export const LAYOUT_NODE = 100;
/** At most this many checked nodes are itemised; the counts cover them all. */
export const LAYOUT_NODES = 8;

/* [item, name, unit]. Units: 'px' a length in CSS pixels × 100, rounded;
   'n' a count or a plain number; 'bits' a set of bits, lowest first. */
export const LAYOUT_TABLE = Object.freeze([
  [1, 'taken', 'n'],             // 1 the sample was taken; 0 the sampling threw and nothing else follows
  [2, 'asserted', 'n'],          // what reachable() returned in this tick: 1 or 0; -1 it could not be asked
  [3, 'conditions', 'bits'],     // the terms of reachable(), one bit each, in LAYOUT_CONDITIONS order
  [4, 'agrees', 'n'],            // 1 "every term holds" says the same as `asserted`, 0 it does not; -1 `asserted` is not known
  [5, 'frames', 'n'],            // settled sample: 1 two animation frames ran within 250 ms before it, 0 they did not; -1 otherwise
  [10, 'innerWidth', 'n'],
  [11, 'innerHeight', 'n'],
  [12, 'clientWidth', 'n'],      // of the document element
  [13, 'clientHeight', 'n'],
  [14, 'pixelRatio', 'px'],      // devicePixelRatio × 100
  [15, 'rootFont', 'px'],        // the root font size
  [16, 'media', 'bits'],         // 1 (max-width: 800px) matches, 2 (max-height: 540px) matches: style.css's small-viewport block
  [17, 'page', 'bits'],          // 1 the document is visible, 2 it has the focus
  [20, 'frameLeft', 'px'],       // #tmux-lifecycle-box
  [21, 'frameTop', 'px'],
  [22, 'frameRight', 'px'],
  [23, 'frameBottom', 'px'],
  [24, 'frameOverflow', 'n'],    // its scrollHeight − clientHeight
  [25, 'frameState', 'bits'],    // 1 it has the class `reentry`, 2 its computed overflow-y is not `visible`
  [26, 'frameMaxHeight', 'px'],  // computed; -1 none
  [27, 'frameMarginTop', 'px'],
  [30, 'bodyLeft', 'px'],        // #tmux-lifecycle-body, the one part meant to scroll
  [31, 'bodyTop', 'px'],
  [32, 'bodyRight', 'px'],
  [33, 'bodyBottom', 'px'],
  [34, 'bodyScrollHeight', 'n'],
  [35, 'bodyClientHeight', 'n'],
  [36, 'bodyScrollTop', 'n'],
  [37, 'bodyScrollWidth', 'n'],
  [38, 'bodyClientWidth', 'n'],
  [40, 'actionLeft', 'px'],      // #tmux-restart, the confirming action
  [41, 'actionTop', 'px'],
  [42, 'actionRight', 'px'],
  [43, 'actionBottom', 'px'],
  [44, 'shown', 'bits'],         // buttons with a box: 1 view sessions, 2 later, 4 restart, 8 done
  [45, 'disabled', 'bits'],      // the same buttons, disabled
  [46, 'rowLeft', 'px'],         // the row of buttons
  [47, 'rowTop', 'px'],
  [48, 'rowRight', 'px'],
  [49, 'rowBottom', 'px'],
  [50, 'sectionScrollWidth', 'n'], // #tmux-reentry
  [51, 'sectionClientWidth', 'n'],
  [52, 'sectionOverflow', 'n'],  // scrollWidth − clientWidth: the horizontal overflow
  [53, 'sectionLeft', 'px'],
  [54, 'sectionTop', 'px'],
  [55, 'sectionRight', 'px'],
  [56, 'sectionBottom', 'px'],
  [57, 'sectionState', 'bits'],  // 1 the hidden attribute, 2 computed display none
  [60, 'nodes', 'n'],            // nodes reachable() checks: the policy, every scope note, the details
  [61, 'scopeNotes', 'n'],
  [62, 'nodesOutside', 'n'],     // of them, not inside
  // what the real tmux_server_status said before the script was installed: written once, with the first English sample
  [70, 'statusFlags', 'bits'],   // 1 pendingRestart, 2 shouldPrompt, 4 canRestart, 8 a server pid, 16 a notice, 32 restart blockers
  [71, 'statusState', 'n'],      // 1..6 in STATUS_STATES order; 0 none of them
  [72, 'statusSessions', 'n'],
  [73, 'statusPanes', 'n'],
]);
export const LAYOUT_ITEMS = Object.freeze(Object.fromEntries(LAYOUT_TABLE.map(([item, name]) => [name, item])));

/* A checked node's items, added to LAYOUT_NODE × k. */
export const NODE_TABLE = Object.freeze([
  [0, 'kind', 'n'],              // 1 the policy, 2 a scope note, 3 the details (closed), 13 the details (open); 0 no such node
  [1, 'left', 'px'],
  [2, 'top', 'px'],
  [3, 'right', 'px'],
  [4, 'bottom', 'px'],
  [5, 'inside', 'bits'],         // 1 width > 0, 2 height > 0, 4 left ≥ frame.left − 1, 8 right ≤ frame.right + 1; 15 is inside
  [6, 'state', 'bits'],          // 1 display none, 2 visibility not visible, 4 the hidden attribute, 8 no box at all
]);

/** The terms of the carrier's `reachable()`, in the order it writes them. */
export const LAYOUT_CONDITIONS = Object.freeze([
  'frame.top >= 0',
  'frame.bottom <= innerHeight + 1',
  'frame.left >= 0',
  'frame.right <= innerWidth + 1',
  'inside(action)',
  'action.bottom <= frame.bottom + 1',
  'action.top >= body.bottom - 1',
  'section.scrollWidth <= section.clientWidth + 1',
  'every checked node is inside',
]);
export const ALL_CONDITIONS = (1 << LAYOUT_CONDITIONS.length) - 1;

/** `tmux_lifecycle::CompatibilityState`, as the status serializes it. */
export const STATUS_STATES = Object.freeze(['CompatibleCurrentBuild', 'CompatibleDifferentBuild', 'RestartRequired',
  'LegacyUnknown', 'CorruptOrUnreachable', 'SourceUnstable']);

const bits = flags => flags.reduce((sum, on, at) => sum | (on ? 1 << at : 0), 0);
const px = length => Math.round(length * 100);
const whole = value => (Number.isFinite(value) ? Math.round(value) : -1);

/** The four parts of the carrier's `inside(rect)`, one bit each. */
export function insideBits(rect, frame) {
  return bits([rect.width > 0, rect.height > 0, rect.left >= frame.left - 1, rect.right <= frame.right + 1]);
}

/** The terms of `reachable()` over one measurement, one bit each. */
export function layoutConditions(g) {
  const inside = rect => insideBits(rect, g.frame) === 15;
  return bits([
    g.frame.top >= 0,
    g.frame.bottom <= g.innerHeight + 1,
    g.frame.left >= 0,
    g.frame.right <= g.innerWidth + 1,
    inside(g.action),
    g.action.bottom <= g.frame.bottom + 1,
    g.action.top >= g.body.bottom - 1,
    g.section.scrollWidth <= g.section.clientWidth + 1,
    g.nodes.every(node => inside(node.rect)),
  ]);
}

/** The buttons of the modal's row, in the bit order of `shown` and `disabled`. */
export const LAYOUT_BUTTONS = Object.freeze(['tmux-view-sessions', 'tmux-later', 'tmux-restart', 'tmux-done']);

/**
 * One measurement of the review modal. `byId` finds an element by its id,
 * `part` is the re-entry section (#tmux-reentry) and `win` the window. Only
 * boxes, scroll sizes, computed styles, flags and media queries are read:
 * never a node's text, value, name or attribute. The nodes are those of the
 * carrier's `reachable()`, in its order — the policy, every scope note, the
 * details — and a node that is not there is a kind-0 node without a box.
 */
export function measureLayout({ byId, part, win }) {
  const styleOf = node => win.getComputedStyle(node);
  const boxOf = node => {
    const r = node.getBoundingClientRect();
    return { left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height };
  };
  const stateOf = node => {
    const style = styleOf(node);
    return bits([style.display === 'none', style.visibility !== 'visible', node.hidden === true, node.getClientRects().length === 0]);
  };
  const frame = byId('tmux-lifecycle-box');
  const body = byId('tmux-lifecycle-body');
  const action = byId('tmux-restart');
  const frameStyle = styleOf(frame);
  const root = win.document.documentElement;
  const checked = [[1, part.querySelector('.reentry-policy')], ...[...part.querySelectorAll('.reentry-scope-note')].map(node => [2, node]),
    [3, part.querySelector('details')]];
  return {
    innerWidth: win.innerWidth, innerHeight: win.innerHeight, clientWidth: root.clientWidth, clientHeight: root.clientHeight,
    pixelRatio: win.devicePixelRatio, rootFont: parseFloat(styleOf(root).fontSize),
    media: bits([win.matchMedia('(max-width: 800px)').matches, win.matchMedia('(max-height: 540px)').matches]),
    page: bits([win.document.visibilityState === 'visible', win.document.hasFocus()]),
    frame: boxOf(frame), frameOverflow: frame.scrollHeight - frame.clientHeight,
    frameState: bits([frame.classList.contains('reentry'), frameStyle.overflowY !== 'visible']),
    frameMaxHeight: frameStyle.maxHeight === 'none' ? -1 : parseFloat(frameStyle.maxHeight), frameMarginTop: parseFloat(frameStyle.marginTop),
    body: boxOf(body), bodyScrollHeight: body.scrollHeight, bodyClientHeight: body.clientHeight, bodyScrollTop: body.scrollTop,
    bodyScrollWidth: body.scrollWidth, bodyClientWidth: body.clientWidth,
    action: boxOf(action), row: boxOf(action.parentElement),
    shown: bits(LAYOUT_BUTTONS.map(id => byId(id).getClientRects().length > 0)), disabled: bits(LAYOUT_BUTTONS.map(id => byId(id).disabled === true)),
    section: { scrollWidth: part.scrollWidth, clientWidth: part.clientWidth, rect: boxOf(part),
      state: bits([part.hidden === true, styleOf(part).display === 'none']) },
    nodes: checked.map(([kind, node]) => (node
      ? { kind: kind === 3 && node.open === true ? 13 : kind, rect: boxOf(node), state: stateOf(node) }
      : { kind: 0, rect: { left: 0, top: 0, right: 0, bottom: 0, width: 0, height: 0 }, state: 8 })),
  };
}

const box = (first, rect) => [[first, px(rect.left)], [first + 1, px(rect.top)], [first + 2, px(rect.right)], [first + 3, px(rect.bottom)]];

/**
 * One measurement as [item, value] pairs of integers. `asserted` is what
 * `reachable()` returned in the same tick (1, 0, or -1 when it could not be
 * asked) and `frames` whether the settled sample saw its animation frames.
 */
export function layoutItems(g, asserted, frames = -1) {
  const I = LAYOUT_ITEMS;
  const conditions = layoutConditions(g);
  const pairs = [
    [I.taken, 1], [I.asserted, asserted], [I.conditions, conditions],
    [I.agrees, asserted < 0 ? -1 : Number((conditions === ALL_CONDITIONS) === (asserted === 1))], [I.frames, frames],
    [I.innerWidth, g.innerWidth], [I.innerHeight, g.innerHeight], [I.clientWidth, g.clientWidth], [I.clientHeight, g.clientHeight],
    [I.pixelRatio, px(g.pixelRatio)], [I.rootFont, px(g.rootFont)], [I.media, g.media], [I.page, g.page],
    ...box(I.frameLeft, g.frame), [I.frameOverflow, g.frameOverflow], [I.frameState, g.frameState],
    [I.frameMaxHeight, g.frameMaxHeight < 0 ? -1 : px(g.frameMaxHeight)], [I.frameMarginTop, px(g.frameMarginTop)],
    ...box(I.bodyLeft, g.body), [I.bodyScrollHeight, g.bodyScrollHeight], [I.bodyClientHeight, g.bodyClientHeight],
    [I.bodyScrollTop, g.bodyScrollTop], [I.bodyScrollWidth, g.bodyScrollWidth], [I.bodyClientWidth, g.bodyClientWidth],
    ...box(I.actionLeft, g.action), [I.shown, g.shown], [I.disabled, g.disabled], ...box(I.rowLeft, g.row),
    [I.sectionScrollWidth, g.section.scrollWidth], [I.sectionClientWidth, g.section.clientWidth],
    [I.sectionOverflow, g.section.scrollWidth - g.section.clientWidth], ...box(I.sectionLeft, g.section.rect), [I.sectionState, g.section.state],
    [I.nodes, g.nodes.length], [I.scopeNotes, g.nodes.filter(node => node.kind === 2).length],
    [I.nodesOutside, g.nodes.filter(node => insideBits(node.rect, g.frame) !== 15).length],
  ];
  g.nodes.slice(0, LAYOUT_NODES).forEach((node, at) => {
    const first = LAYOUT_NODE * (at + 1);
    pairs.push([first, node.kind], ...box(first + 1, node.rect), [first + 5, insideBits(node.rect, g.frame)], [first + 6, node.state]);
  });
  return pairs.map(([item, value]) => [item, whole(value)]);
}

/** The real status as pairs: flags, a closed state index and two counts. */
export function statusItems(status) {
  const I = LAYOUT_ITEMS;
  const s = status && typeof status === 'object' ? status : {};
  return [
    [I.statusFlags, bits([s.pendingRestart === true, s.shouldPrompt === true, s.canRestart === true, Number.isInteger(s.serverPid),
      s.notice != null, Array.isArray(s.restartBlockers) && s.restartBlockers.length > 0])],
    [I.statusState, STATUS_STATES.indexOf(s.status) + 1],
    [I.statusSessions, whole(s.sessionCount)],
    [I.statusPanes, whole(s.paneCount)],
  ];
}

/* ---------- reading the lines back ---------- */

const LINE = /\[ui\] smoke-check reentry-layout-(en|zh) a=(-?\d+) b=(-?\d+)/;

/**
 * The layout lines of an app.log: language → 'asserted' | 'settled' → item →
 * value. A later line for the same item replaces an earlier one, so a log
 * that holds several runs reads as its last.
 */
export function readLayout(logText) {
  const read = { en: { asserted: new Map(), settled: new Map() }, zh: { asserted: new Map(), settled: new Map() } };
  for (const line of logText.split('\n')) {
    const match = LINE.exec(line);
    if (!match) continue;
    const item = Number(match[2]);
    const settled = item >= LAYOUT_SETTLED;
    read[match[1]][settled ? 'settled' : 'asserted'].set(settled ? item - LAYOUT_SETTLED : item, Number(match[3]));
  }
  return read;
}

const shown = (unit, value) => (unit === 'px' && value !== -1 ? (value / 100).toFixed(2) : String(value));
const named = (table, item) => table.find(([at]) => at === item);

/** One sample as lines a person reads: every item by name, and which terms of `reachable()` failed. */
export function describeSample(items) {
  if (!items.size) return ['  (no line)'];
  if (items.get(LAYOUT_ITEMS.taken) !== 1) return ['  the sampling threw: nothing was measured'];
  const out = [];
  const conditions = items.get(LAYOUT_ITEMS.conditions);
  const failed = LAYOUT_CONDITIONS.filter((_, at) => !(conditions & (1 << at)));
  out.push(`  reachable() = ${items.get(LAYOUT_ITEMS.asserted)}; terms that do not hold: ${failed.length ? failed.join(' | ') : 'none'}`
    + `${items.get(LAYOUT_ITEMS.agrees) === 0 ? '  [the items and the assertion DISAGREE]' : ''}`);
  for (const [item, value] of [...items].sort((a, b) => a[0] - b[0])) {
    if (item < LAYOUT_NODE) {
      const row = named(LAYOUT_TABLE, item);
      out.push(`  ${String(item).padStart(4)} ${row ? row[1] : '?'} = ${row ? shown(row[2], value) : value}`);
    } else {
      const row = named(NODE_TABLE, item % LAYOUT_NODE);
      out.push(`  ${String(item).padStart(4)} node ${Math.floor(item / LAYOUT_NODE)} ${row ? row[1] : '?'} = ${row ? shown(row[2], value) : value}`);
    }
  }
  return out;
}

/** Everything `readLayout` found, for both languages and both samples. */
export function describeLayout(logText) {
  const read = readLayout(logText);
  const out = [];
  for (const [language, label] of [['en', 'English'], ['zh', 'Chinese']]) {
    for (const [sample, when] of [['asserted', 'in the tick of the assertion'], ['settled', 'after the page settled']]) {
      out.push(`${label}, ${when}:`, ...describeSample(read[language][sample]));
    }
  }
  return out.join('\n');
}
