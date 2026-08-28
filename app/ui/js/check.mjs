#!/usr/bin/env node
// Static UNRESOLVED-IDENTIFIER scan for deck's no-build ES modules — NOT a
// syntax check (CI runs `node --check` for that) and not a behavior test
// (CI runs `node --test` on ui/test/): every identifier a module uses must
// be declared in it, imported, a shared globalThis slot (declared in
// state.js), a vendor global, or a browser built-in. Catches the "forgot an
// import during refactor" class before the webview does. Also forbids
// xterm private API (`._core`) in deck's own code.
// Runs in CI (test.yml) and exits non-zero on violations.

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = dirname(fileURLToPath(import.meta.url));
const files = readdirSync(dir).filter(f => f.endsWith('.js'));

const BROWSER = new Set([
  'window', 'document', 'globalThis', 'navigator', 'location', 'console',
  'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval', 'requestAnimationFrame',
  'JSON', 'Math', 'Date', 'Promise', 'Object', 'Array', 'String', 'Number', 'Boolean',
  'Map', 'Set', 'WeakMap', 'RegExp', 'Error', 'Uint8Array', 'TextEncoder', 'TextDecoder',
  'atob', 'btoa', 'isNaN', 'parseInt', 'parseFloat', 'undefined', 'null', 'true', 'false',
  'NaN', 'Infinity', 'ResizeObserver', 'MutationObserver', 'CustomEvent', 'Event',
  'KeyboardEvent', 'MouseEvent', 'localStorage', 'structuredClone', 'queueMicrotask',
  'alert', 'prompt', 'confirm', 'getComputedStyle', 'encodeURIComponent', 'decodeURIComponent', 'fetch',
  'innerWidth', 'innerHeight', 'devicePixelRatio', 'performance', 'crypto', 'history',
  // vendored xterm.js globals (classic scripts)
  'Terminal', 'FitAddon', 'WebLinksAddon', 'ClipboardAddon',
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

// shared slots come from state.js's Object.assign(globalThis, {...})
const stateSrc = readFileSync(join(dir, 'state.js'), 'utf8');
const slotBlock = stateSrc.match(/Object\.assign\(globalThis, \{([\s\S]*?)\}\);/);
const SLOTS = new Set([...(slotBlock ? slotBlock[1].matchAll(/^\s*([A-Za-z_$][\w$]*):/gm) : [])].map(m => m[1]));

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
    if (KEYWORDS.has(id) || BROWSER.has(id) || SLOTS.has(id) || declared.has(id)) continue;
    unknown.set(id, (unknown.get(id) || 0) + 1);
  }
  if (unknown.size) {
    bad += unknown.size;
    console.error(`${f}: unresolved identifiers: ${[...unknown.keys()].join(', ')}`);
  }
}
if (bad) {
  console.error(`\n${bad} unresolved identifier(s)`);
  process.exit(1);
}
console.log(`ok: ${files.length} modules, all identifiers resolve`);
