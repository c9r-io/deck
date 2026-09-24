// The WKWebView smoke carriers against test/fixtures/smoke-manifest.json:
// every checkpoint name a carrier file can emit, read as single-line
// literals, equals the names the manifest expects from the modes that file
// carries, and app/run.sh accepts exactly the manifest's modes.
// (diagnostics.rs holds SMOKE_CHECKS and main.rs SMOKE_ENTRIES to the same
// manifest, and scripts/smoke-verdict judges a run's app.log against it.)
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const manifest = JSON.parse(readFileSync(new URL('./fixtures/smoke-manifest.json', import.meta.url), 'utf8'));

/* Which modes each carrier file serves: main.rs dispatches every
   --smoke-wkwebview mode into wk-smoke.mjs, whose verify* entries hand
   five of them to their own files. */
const CARRIERS = {
  'wk-smoke.mjs': ['run', 'settings', 'ambiguous', 'restart', 'buffer', 'channel', 'channel-fault', 'connector', 'connector-transport'],
  'attention-smoke.mjs': ['attention'],
  'resume-smoke.mjs': ['resume'],
  'review-smoke.mjs': ['review', 'review-restart'],
  'voice-smoke.mjs': ['voice'],
};

/* Names built at runtime, which a single-line literal scan cannot read:
   (file, the exact source text that builds them, the names it yields). */
const DYNAMIC = [
  ['wk-smoke.mjs', 'await report(kind, ', ['channel-network', 'channel-scope']],
  ['resume-smoke.mjs', "report('resume-capture-' + i, ", ['resume-capture-0', 'resume-capture-1']],
  ['review-smoke.mjs', "report(seq === 2 ? 'review-second' : 'review-last', ", ['review-second', 'review-last']],
  ['voice-smoke.mjs', 'report(`voice-theme-${theme}`, ', ['voice-theme-light', 'voice-theme-high-contrast', 'voice-theme-deck-dark']],
  ['voice-smoke.mjs', 'report(`voice-exception-${stage}`, ', ['voice-exception-0', 'voice-exception-1', 'voice-exception-2', 'voice-exception-3', 'voice-exception-4']],
];

/* Literal checkpoint names on one line: report('x', / metric('x', and a
   direct ui_event whose detail is a literal on a smoke-check line. */
export function literalChecks(source) {
  const names = new Set();
  for (const line of source.split('\n')) {
    for (const match of line.matchAll(/\b(?:report|metric)\(\s*'([a-z0-9-]+)'\s*[,)]/g)) names.add(match[1]);
    if (line.includes("'smoke-check'")) {
      for (const match of line.matchAll(/detail:\s*'([a-z0-9-]+)'/g)) names.add(match[1]);
    }
  }
  return names;
}

const expectedFor = modes => {
  const names = new Set();
  for (const mode of modes) {
    const spec = manifest.modes[mode];
    assert.ok(spec, `manifest has mode ${mode}`);
    if (spec.terminal) names.add(spec.terminal);
    for (const name of Object.keys(spec.checks)) names.add(name);
  }
  return names;
};

test('every smoke mode is carried by exactly one file', () => {
  const carried = Object.values(CARRIERS).flat();
  assert.deepEqual([...carried].sort(), Object.keys(manifest.modes).sort());
  assert.equal(new Set(carried).size, carried.length);
});

test('app/run.sh launches exactly the manifest modes', () => {
  // The one `case` pattern line that lists the accepted modes (anything
  // else falls back to run there, as in main.rs SMOKE_ENTRIES).
  const run = readFileSync(new URL('../../run.sh', import.meta.url), 'utf8');
  const lines = run.split('\n').map(line => line.trim()).filter(line => /^[a-z-]+(?:\|[a-z-]+)+\) ;;$/.test(line));
  assert.equal(lines.length, 1, 'one mode list in run.sh');
  assert.deepEqual(lines[0].slice(0, -') ;;'.length).split('|').sort(), Object.keys(manifest.modes).sort());
});

test('each carrier emits exactly the checkpoints the manifest expects from its modes', () => {
  for (const [file, modes] of Object.entries(CARRIERS)) {
    const source = readFileSync(new URL(`./${file}`, import.meta.url), 'utf8');
    const emitted = literalChecks(source);
    for (const [dynamicFile, text, names] of DYNAMIC) {
      if (dynamicFile !== file) continue;
      assert.ok(source.includes(text), `${file} no longer builds names with ${text}`);
      for (const name of names) emitted.add(name);
    }
    assert.deepEqual([...emitted].sort(), [...expectedFor(modes)].sort(), file);
  }
});

test('the literal scan reads one line and nothing else', () => {
  assert.deepEqual([...literalChecks("await report('a-b', ok);\nmetric('c', 1)")], ['a-b', 'c']);
  assert.deepEqual([...literalChecks("inv('ui_event', { code: 'smoke-check', detail: 'done', a: 1 })")], ['done']);
  assert.deepEqual([...literalChecks("inv('ui_event', { code: 'js-reject', detail: 'x' })")], []);
  assert.deepEqual([...literalChecks('report(name, ok)')], []);
  assert.deepEqual([...literalChecks("report('prefix-' + i, ok)")], [], 'a concatenated name is a DYNAMIC entry');
});
