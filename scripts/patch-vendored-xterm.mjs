#!/usr/bin/env node
// The one local change to the vendored xterm build, as data plus a check.
//
//   node scripts/patch-vendored-xterm.mjs --verify
//       the vendored file is the published build with exactly this change
//   node scripts/patch-vendored-xterm.mjs <published lib/xterm.js> <output>
//       rebuild the vendored file from the published build
//
// The change, its upstream commit and both SHA-256 values live in
// app/ui/test/fixtures/xterm-patch.json (docs/vendored-xterm.md explains them).
// Any input that is not the pinned published build, any text that does not
// match exactly once, and any result with another digest is a failure.
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const patch = JSON.parse(readFileSync(resolve(root, 'app/ui/test/fixtures/xterm-patch.json'), 'utf8'));
const sha256 = text => createHash('sha256').update(text).digest('hex');
const count = (text, part) => text.split(part).length - 1;
const fail = message => { console.error(`patch-vendored-xterm: ${message}`); process.exit(1); };

function applyPatch(published) {
  if (sha256(published) !== patch.originalSha256) fail(`input is not ${patch.package} ${patch.version} ${patch.file}`);
  if (count(published, patch.before) !== 1 || count(published, patch.after) !== 0) fail('the patched expression does not match exactly once');
  const patched = published.replace(patch.before, patch.after);
  if (sha256(patched) !== patch.patchedSha256) fail('the patched build has an unexpected digest');
  return patched;
}

function verifyVendored(vendored) {
  if (sha256(vendored) !== patch.patchedSha256) fail('the vendored build is not the pinned patched build');
  if (count(vendored, patch.after) !== 1 || count(vendored, patch.before) !== 0) fail('the vendored build does not carry the change exactly once');
  if (sha256(vendored.replace(patch.after, patch.before)) !== patch.originalSha256) fail('the vendored build differs from the published build by more than the change');
}

const args = process.argv.slice(2);
if (args.length === 1 && args[0] === '--verify') {
  verifyVendored(readFileSync(resolve(root, 'app/ui/vendor/xterm.js'), 'utf8'));
  console.log(`vendored xterm.js = ${patch.package} ${patch.version} + upstream ${patch.upstreamCommit.slice(0, 12)} (${patch.patchedSha256})`);
} else if (args.length === 2) {
  writeFileSync(args[1], applyPatch(readFileSync(args[0], 'utf8')));
  console.log(patch.patchedSha256);
} else {
  fail('usage: --verify | <published lib/xterm.js> <output>');
}
