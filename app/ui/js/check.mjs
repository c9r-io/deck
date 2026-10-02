#!/usr/bin/env node
// Static UNRESOLVED-IDENTIFIER scan for deck's no-build ES modules — NOT a
// syntax check (CI runs `node --check` for that) and not a behavior test
// (CI runs `node --test` on ui/test/): every identifier a module uses must
// be declared in it, imported, a vendor global, or a browser built-in
// (shared runtime slots are members of state.js's exported `ctx`, so a bare
// slot name is exactly the kind of leftover this catches). Catches the "forgot an
// import during refactor" class before the webview does. Also forbids
// xterm private API (`._core`) in deck's own code, and confines static
// import CYCLES to the view core: board.js, layout.js, terminal.js may import
// each other (they share one document); every other
// module must be a leaf or a strict dependency — what a leaf needs from the
// core arrives through its `init*(deps)` from app.js (attention, templates,
// automation and scheduler set the pattern). A cycle through any other module fails.
// The cycle check sees every STATIC edge — `import … from`, a bare side-effect
// `import './x.js'` and a re-export `export … from` — between every module
// under this directory, subdirectories included, and reports strongly
// connected components, so a cycle is found whichever edge closes it.
// Runs in CI (gate.yml) and exits non-zero on violations. `node check.mjs
// <directory>` checks another module directory (ui/test/static.test.mjs
// feeds it small fixtures).

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, posix, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = process.argv[2] ? resolve(process.argv[2]) : dirname(fileURLToPath(import.meta.url));
// every module, as a path relative to `dir` with `/` (subdirectories included)
function modulesUnder(prefix) {
  return readdirSync(join(dir, prefix), { withFileTypes: true }).flatMap(entry =>
    entry.isDirectory() ? modulesUnder(`${prefix}${entry.name}/`)
      : entry.name.endsWith('.js') ? [`${prefix}${entry.name}`] : []);
}
const files = modulesUnder('').sort();

const BROWSER = new Set([
  'window', 'document', 'globalThis', 'navigator', 'location', 'console',
  'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'requestAnimationFrame',
  'JSON', 'Math', 'Date', 'Intl', 'URL', 'Promise', 'Object', 'Array', 'String', 'Number', 'Boolean',
  'Map', 'Set', 'WeakMap', 'RegExp', 'Error', 'Uint8Array', 'TextEncoder', 'TextDecoder',
  'atob', 'btoa', 'isNaN', 'parseInt', 'parseFloat', 'undefined', 'null', 'true', 'false',
  'NaN', 'Infinity', 'ResizeObserver', 'MutationObserver', 'CustomEvent', 'Event',
  'KeyboardEvent', 'MouseEvent', 'localStorage', 'structuredClone', 'queueMicrotask',
  'alert', 'prompt', 'confirm', 'getComputedStyle', 'encodeURIComponent', 'decodeURIComponent', 'fetch',
  'innerWidth', 'innerHeight', 'devicePixelRatio', 'performance', 'crypto', 'history',
  'FileReader', 'Element', 'HTMLSelectElement',
  // vendored xterm.js globals (classic scripts)
  'Terminal', 'FitAddon', 'WebLinksAddon',
]);
const KEYWORDS = new Set(('break case catch class const continue debugger default delete do else export extends '
  + 'finally for function if import in instanceof let new of return static super switch this throw try typeof '
  + 'var void while with yield async await get set from as').split(' '));

// Character-level scanner: blanks out comments, strings, template literals
// (nested ${`…`} included) and regex literals so only real code identifiers
// remain. Everything blanked becomes spaces (line structure preserved).
function stripped(src) {
  const out = src.split('');
  const blank = (a, b) => { for (let i = a; i < b; i++) if (out[i] !== '\n') out[i] = ' '; };
  let i = 0;
  let lastSig = '';           // last significant char, to tell regex from division
  const tplDepth = [];        // template-literal ${ } nesting
  while (i < src.length) {
    const c = src[i], d = src[i + 1];
    if (c === '/' && d === '/') { const j = src.indexOf('\n', i); const e = j < 0 ? src.length : j; blank(i, e); i = e; continue; }
    if (c === '/' && d === '*') { const j = src.indexOf('*/', i + 2); const e = j < 0 ? src.length : j + 2; blank(i, e); i = e; continue; }
    if (c === "'" || c === '"') {
      let j = i + 1;
      while (j < src.length && src[j] !== c) j += src[j] === '\\' ? 2 : 1;
      blank(i + 1, j); i = j + 1; lastSig = c; continue;
    }
    if (c === '`') {
      // scan template; ${ … } interiors stay (they are code)
      let j = i + 1;
      while (j < src.length) {
        if (src[j] === '\\') { j += 2; continue; }
        if (src[j] === '`') break;
        if (src[j] === '$' && src[j + 1] === '{') { blank(i + 1, j); tplDepth.push(0); i = j + 2; lastSig = '{'; break; }
        j++;
      }
      if (src[j] === '`') { blank(i + 1, j); i = j + 1; lastSig = '`'; }
      if (tplDepth.length && src[i - 2] === '$') continue;   // entered ${ …
      continue;
    }
    if (c === '{' && tplDepth.length) tplDepth[tplDepth.length - 1]++;
    if (c === '}' && tplDepth.length) {
      if (tplDepth[tplDepth.length - 1] === 0) {
        // closing a ${ } — resume scanning the enclosing template literal
        tplDepth.pop();
        let j = i + 1;
        while (j < src.length) {
          if (src[j] === '\\') { j += 2; continue; }
          if (src[j] === '`') break;
          if (src[j] === '$' && src[j + 1] === '{') { blank(i + 1, j); tplDepth.push(0); j += 1; break; }
          j++;
        }
        if (src[j] === '`') { blank(i + 1, j); i = j + 1; lastSig = '`'; continue; }
        i = j + 1; lastSig = '{'; continue;
      }
      tplDepth[tplDepth.length - 1]--;
    }
    if (c === '/' && !'\n'.includes(d)) {
      // regex literal iff a value cannot precede it
      const before = out.slice(Math.max(0, i - 12), i).join('');
      const kwBefore = /(?:^|[^\w$])(?:return|typeof|case|in|of|do|else)\s*$/.test(before);
      if (!/[\w$)\]]/.test(lastSig) || kwBefore) {
        let j = i + 1, inClass = false;
        while (j < src.length && (inClass || src[j] !== '/')) {
          if (src[j] === '\\') j++;
          else if (src[j] === '[') inClass = true;
          else if (src[j] === ']') inClass = false;
          else if (src[j] === '\n') break;
          j++;
        }
        if (src[j] === '/') {
          while (/[a-z]/.test(src[j + 1] || '')) j++;
          blank(i, j + 1); i = j + 1; lastSig = ')';
          continue;
        }
      }
    }
    if (!/\s/.test(c)) lastSig = c;
    i++;
  }
  return out.join('');
}


let bad = 0;
for (const f of files) {
  const raw = readFileSync(join(dir, f), 'utf8');
  // deck code must stay on xterm's public API (vendored addons are exempt —
  // they live in ui/vendor/, outside this scan)
  if (raw.includes('._core')) {
    bad++;
    console.error(`${f}: uses xterm private API (._core) — public API or DOM measurement only`);
  }
  const src = stripped(raw);
  const declared = new Set();
  for (const m of src.matchAll(/\b(?:const|let|var|function|class)\s+([A-Za-z_$][\w$]*)/g)) declared.add(m[1]);
  // object-literal method shorthand: name(args) { — name and params
  for (const m of src.matchAll(/^\s*(?:async\s+)?([A-Za-z_$][\w$]*)\s*\(([^)]*)\)\s*\{/gm)) {
    declared.add(m[1]);
    for (const n of m[2].split(',')) {
      const id = n.trim().replace(/=.*/, '').replace(/[{}[\]\s.]/g, '');
      if (id) declared.add(id);
    }
  }
  for (const m of src.matchAll(/import\s*\{([^}]*)\}\s*from/g)) {
    for (const n of m[1].split(',')) declared.add(n.trim().split(/\s+as\s+/).pop());
  }
  // function params & catch params & arrow params (approximate)
  for (const m of src.matchAll(/(?:function[^(]*|catch)\s*\(([^)]*)\)/g)) {
    for (const n of m[1].split(',')) {
      const id = n.trim().replace(/=.*/, '').replace(/[{}[\]\s.]/g, '');
      if (id) declared.add(id);
    }
  }
  for (const m of src.matchAll(/\(([^()]*)\)\s*=>/g)) {
    for (const n of m[1].split(',')) {
      const id = n.trim().replace(/=.*/, '').replace(/[{}[\]\s.]/g, '');
      if (id) declared.add(id);
    }
  }
  for (const m of src.matchAll(/\b([A-Za-z_$][\w$]*)\s*=>/g)) declared.add(m[1]);
  for (const m of src.matchAll(/(?:for)\s*\(\s*(?:const|let|var)?\s*\[?([A-Za-z_$][\w$]*)/g)) declared.add(m[1]);
  for (const m of src.matchAll(/(?:const|let|var)\s*\{([^}]*)\}/g)) {
    for (const n of m[1].split(',')) {
      const id = n.trim().split(':').pop().trim().replace(/=.*/, '').trim();
      if (/^[A-Za-z_$][\w$]*$/.test(id)) declared.add(id);
    }
  }
  for (const m of src.matchAll(/(?:const|let|var)\s*\[([^\]]*)\]/g)) {
    for (const n of m[1].split(',')) {
      const id = n.trim();
      if (/^[A-Za-z_$][\w$]*$/.test(id)) declared.add(id);
    }
  }
  // multi-declarator: let a = 1, b = 2
  for (const m of src.matchAll(/\b(?:const|let|var)\s+[^;]*/g)) {
    for (const n of m[0].replace(/^(const|let|var)\s+/, '').split(',')) {
      const id = n.trim().split(/[=\s]/)[0];
      if (/^[A-Za-z_$][\w$]*$/.test(id)) declared.add(id);
    }
  }

  const unknown = new Map();
  for (const m of src.matchAll(/(?<![.\w$])([A-Za-z_$][\w$]*)\b(?!\s*:)/g)) {
    const id = m[1];
    if (KEYWORDS.has(id) || BROWSER.has(id) || declared.has(id)) continue;
    unknown.set(id, (unknown.get(id) || 0) + 1);
  }
  if (unknown.size) {
    bad += unknown.size;
    console.error(`${f}: unresolved identifiers: ${[...unknown.keys()].join(', ')}`);
  }
}
// ---- import cycles outside the view core ----
const CORE = new Set(['board.js', 'layout.js', 'terminal.js']);
// Static specifiers only (a dynamic import() is a deliberate late edge), in
// each form a module can depend on another: `import … from`, the bare
// `import './x.js'`, and `export * from` / `export { … } from`.
const STATIC_EDGE = /^(?:import\s+(?:[^;'"]*?\bfrom\s+)?|export\s+(?:\*[^;'"]*?|\{[^;'"]*?\})\s*from\s+)['"](\.{1,2}\/[^'"]+\.js)['"]/gm;
const edges = new Map(files.map(f => [f,
  [...readFileSync(join(dir, f), 'utf8').matchAll(STATIC_EDGE)]
    .map(m => posix.normalize(posix.join(posix.dirname(f), m[1])))
    .filter(target => files.includes(target))]));
// A cycle is a strongly connected component of more than one module (or a
// module that imports itself). Components rather than a depth-first walk
// that returns at visited modules: such a walk never reports a cycle that
// closes through a module it has already finished.
function components() {
  const index = new Map(), low = new Map(), onStack = new Set(), stack = [], found = [];
  const visit = node => {
    index.set(node, index.size); low.set(node, index.get(node));
    stack.push(node); onStack.add(node);
    for (const next of edges.get(node)) {
      if (!index.has(next)) { visit(next); low.set(node, Math.min(low.get(node), low.get(next))); }
      else if (onStack.has(next)) low.set(node, Math.min(low.get(node), index.get(next)));
    }
    if (low.get(node) !== index.get(node)) return;
    const component = [];
    let member;
    do { member = stack.pop(); onStack.delete(member); component.push(member); } while (member !== node);
    found.push(component.sort());
  };
  for (const f of files) if (!index.has(f)) visit(f);
  return found;
}
for (const component of components()) {
  if (component.length === 1 && !edges.get(component[0]).includes(component[0])) continue;
  const outside = component.filter(name => !CORE.has(name));
  if (outside.length) {
    bad++;
    console.error(`import cycle leaves the view core (${outside.join(', ')}): ${component.join(' <-> ')}`);
  }
}

if (bad) {
  console.error(`\n${bad} violation(s)`);
  process.exit(1);
}
console.log(`ok: ${files.length} modules, all identifiers resolve, import cycles stay inside the view core`);
