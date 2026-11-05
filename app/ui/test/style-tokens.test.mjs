// The spacing and type contract of style.css (its header): every margin,
// padding, gap and font-size is a token of the two :root scales, calc() over
// tokens, 0, 1px, auto, a percentage or a viewport unit. The scales are rem
// against the 13px root, so they follow the font-size setting.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';

const read = path => readFileSync(new URL(path, import.meta.url), 'utf8');
const css = read('../style.css');
const body = css.replace(/\/\*[\s\S]*?\*\//g, match => match.replace(/[^\n]/g, ' '));
const SPACING = /(?<![-\w])((?:margin|padding)(?:-[a-z-]+)?|gap|row-gap|column-gap|font-size)\s*:\s*([^;{}"]+)/g;
// a length that is not a token: any number with a length unit but a hairline
const rawLengths = value => [...value.matchAll(/(?<![\w.-])-?\d*\.?\d+(px|rem|em|ch|pt)\b/g)]
  .map(found => found[0]).filter(length => length !== '1px' && length !== '-1px');
const lineOf = index => body.slice(0, index).split('\n').length;

test('every spacing and font size in style.css comes from the scales', () => {
  const offenders = [];
  for (const found of body.matchAll(SPACING)) {
    const raw = rawLengths(found[2]);
    if (raw.length) offenders.push(`line ${lineOf(found.index)}: ${found[1]}: ${found[2].trim()}`);
  }
  // the root's own size is what the scales are measured against
  assert.deepEqual(offenders, [`line ${lineOf(body.indexOf('html { font-size: 13px; }'))}: font-size: 13px`]);
  // spacing held in custom properties (a textarea's padding, a form's label column)
  const custom = [...body.matchAll(/(--[a-z-]*(?:pad|gap|space)[a-z-]*)\s*:\s*([^;{}]+)/g)]
    .filter(found => rawLengths(found[2]).length).map(found => `${found[1]}: ${found[2].trim()}`);
  assert.deepEqual(custom, []);
});

test('the scales are rem against the root the font-size setting scales', () => {
  const base = Number(read('../js/font-scale.js').match(/APP_BASE_FONT_SIZE = (\d+);/)[1]);
  assert.match(css, new RegExp(`^html \\{ font-size: ${base}px; \\}$`, 'm'));
  assert.match(css, new RegExp(`--u: calc\\(1rem / ${base}\\);`));
  const defined = new Map([...css.matchAll(/(--(?:sp|fs)-(\d+)): calc\((\d+) \* var\(--u\)\);/g)]
    .map(found => [found[1], [Number(found[2]), Number(found[3])]]));
  for (const [name, [step, factor]] of defined) assert.equal(step, factor, `${name} is ${step}px at 100 %`);
  assert.deepEqual([...defined.keys()].filter(name => name.startsWith('--sp-')).map(name => Number(name.slice(5))),
    [2, 4, 6, 8, 10, 12, 16, 20, 24, 32]);
  assert.deepEqual([...defined.keys()].filter(name => name.startsWith('--fs-')).map(name => Number(name.slice(5))),
    [10, 11, 12, 13, 14, 15, 18]);
  // nothing names a step the scales do not have
  const used = new Set([...css.matchAll(/var\((--(?:sp|fs)-\d+)\)/g)].map(found => found[1]));
  assert.deepEqual([...used].filter(name => !defined.has(name)), []);
});

test('a text block aligned with labelled controls is indented by its form\'s label column', () => {
  // the offset follows the gap of the row it lines up with, at every font size
  const offsets = [...body.matchAll(/margin(?:-left)?:[^;{}]*calc\(var\(--label-w\) \+ var\(--sp-10\)\)/g)];
  assert.ok(offsets.length >= 6, 'set-hint, shortcut-list, pdf-chips, the preset error, the template hint, the rule hint');
  assert.match(css, /^\.set-row \{[^}]*gap: var\(--sp-10\);/m);
  assert.match(css, /^\.set-row label \{[^}]*width: var\(--label-w\);/m);
  for (const form of ['#pdf-box', '#tpl-form', '#mcp-auth-box', '#auto-editor']) {
    assert.match(css, new RegExp(`^${form} \\{ --label-w: `, 'm'), form);
  }
  // at 140 % and up labels sit above their controls, so each indented block
  // starts at the left edge there too; only the rule editor keeps its label
  // column at every size
  const clean = selector => selector.trim().replace(/\s+/g, ' ');
  const indented = [...body.matchAll(/([^{}]+)\{[^{}]*margin(?:-left)?:[^;{}]*calc\(var\(--label-w\) \+ var\(--sp-10\)\)/g)]
    .map(found => clean(found[1]));
  const flush = [...body.matchAll(/([^{}]+)\{ margin-left: 0; \}/g)]
    .flatMap(found => found[1].split(',').map(clean))
    .filter(selector => selector.startsWith('html.font-scale-large '))
    .map(selector => selector.slice('html.font-scale-large '.length));
  assert.deepEqual(indented.filter(selector => !selector.startsWith('#auto-editor ') && !flush.includes(selector)), []);
  assert.match(css, /^#auto-editor \.set-row label \{ width: var\(--label-w\);/m);
});

test('inline styles in index.html and the modules use the scales too', () => {
  const html = read('../index.html');
  const inline = [...html.matchAll(/style="([^"]*)"/g)].map(found => found[1])
    .filter(style => [...style.matchAll(SPACING)].some(found => rawLengths(found[2]).length));
  assert.deepEqual(inline, []);
  const dir = new URL('../js/', import.meta.url);
  const offenders = [];
  for (const file of readdirSync(dir).filter(name => name.endsWith('.js'))) {
    const source = readFileSync(new URL(file, dir), 'utf8');
    for (const found of source.matchAll(/(?:style="|cssText\s*=\s*['`])([^"'`]*)/g)) {
      if ([...found[1].matchAll(SPACING)].some(spacing => rawLengths(spacing[2]).length)) offenders.push(file);
    }
    if (/style\.(?:margin|padding|gap|fontSize)\w*\s*=/.test(source) && file !== 'font-scale.js') offenders.push(file);
  }
  assert.deepEqual(offenders, []);
});

test('every width that holds text follows the font size', () => {
  // N px at 100 % is calc(N * var(--u)); px stays for an offset from the
  // window or the box, a size under 40px, and the pairing QR code
  const offenders = [];
  for (const rule of body.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selector = rule[1].trim().replace(/\s+/g, ' ');
    if (selector === '#set-connector-qr') continue;
    for (const found of rule[2].matchAll(/(?<![-\w])((?:min-|max-)?width|flex(?:-basis)?|grid-template-columns|grid-auto-columns|--[a-z-]*(?:width|-w))\s*:\s*([^;]+)/g)) {
      const raw = [...found[2].matchAll(/(^|[\s(,])(\d+)px\b/g)].filter(px => Number(px[2]) >= 40
        && !/(?:vw|vh|%)\s*-\s*$/.test(found[2].slice(0, px.index + px[1].length)));
      if (raw.length) offenders.push(`${selector}: ${found[1]}: ${found[2].trim()}`);
    }
  }
  assert.deepEqual(offenders, []);
  // the tool drawer's base is the number layout.js multiplies by the font scale
  assert.match(css, /--session-tool-base: 440; --session-tool-width: calc\(var\(--session-tool-base\) \* var\(--u\)\);/);
  assert.match(read('../js/layout.js'), /getPropertyValue\('--session-tool-base'\)\) \|\| 440;\n  const drawer = base \* getFontScale\(\);/);
  // no fixed width that only the large font size used to need
  assert.doesNotMatch(css, /^html\.font-scale-large (?:\.set-sidebar|#settings-box|#tpl-box) \{/m);
});

test('the rule drawer keeps its scrollbar at all times', () => {
  // WebKit can leave stretched rows laid out under a bar that appears later;
  // the next click re-wrapped the rule editor's trigger row under the pointer
  // (approval-press-box, 2026-11-05). scrollbar-gutter: stable did not stop it.
  assert.match(css, /^#auto-drawer \{[^}]*overflow-y: scroll;/m);
});
