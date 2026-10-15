// Generated extension imports use one exported name with distinct local names.
// The source checker must accept the alias without inventing a global binding.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

test('aliased imports resolve only their local bindings and still participate in cycles', () => {
  const directory = mkdtempSync(join(tmpdir(), 'deck-import-alias-'));
  const check = () => spawnSync(process.execPath, [resolve('app/ui/js/check.mjs'), directory], { encoding: 'utf8' });
  try {
    writeFileSync(join(directory, 'entry.js'), 'export const CONTRIBUTION = {};\n');
    const imports = "import { CONTRIBUTION as entry0 } from './entry.js';\n";
    writeFileSync(join(directory, 'catalog.js'), imports + 'export const entries = [entry0];\n');
    assert.equal(check().status, 0);
    writeFileSync(join(directory, 'catalog.js'), imports + 'export const entries = [CONTRIBUTION];\n');
    const unresolved = check();
    assert.equal(unresolved.status, 1);
    assert.match(unresolved.stderr, /unresolved identifiers: CONTRIBUTION/);
    writeFileSync(join(directory, 'catalog.js'), imports + 'export const entries = [entry0];\n');
    writeFileSync(join(directory, 'entry.js'), "import { entries as catalog } from './catalog.js';\nexport const CONTRIBUTION = catalog;\n");
    const cycle = check();
    assert.equal(cycle.status, 1);
    assert.match(cycle.stderr, /import cycle leaves the view core/);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});
