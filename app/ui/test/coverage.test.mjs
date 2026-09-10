import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { missingCoverage } from '../../../scripts/check-ui-coverage.mjs';

test('coverage inventory detects unloaded new modules, including nested files, without enlarging exclusions', () => {
  const root = mkdtempSync(join(tmpdir(), 'deck-coverage-test-'));
  try {
    mkdirSync(join(root, 'scripts')); mkdirSync(join(root, 'app/ui/js/nested'), { recursive: true });
    writeFileSync(join(root, 'scripts/ui-tests'), "--test-coverage-exclude='app/ui/js/{app}.js'");
    for (const file of ['app.js', 'model.js', 'new.js', 'nested/logic.js']) writeFileSync(join(root, 'app/ui/js', file), 'export const sample = 1;');
    assert.deepEqual(missingCoverage(root, 'SF:app/ui/js/model.js\n'), ['app/ui/js/nested/logic.js', 'app/ui/js/new.js']);
    const report = ['model.js', 'new.js', 'nested/logic.js'].map(file => `SF:${join(root, 'app/ui/js', file)}\n`).join('');
    assert.deepEqual(missingCoverage(root, report), []);
    writeFileSync(join(root, 'scripts/ui-tests'), 'missing configuration');
    assert.throws(() => missingCoverage(root, report), /exclusion list/);
  } finally { rmSync(root, { recursive: true, force: true }); }
});
