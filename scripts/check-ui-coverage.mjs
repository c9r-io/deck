// Fail when an eligible production module silently disappears from coverage.
// Node only reports loaded modules: freezing exclusions alone cannot enforce
// the denominator. Read the gate's one exclusion list, never add another one.
import { readFileSync, readdirSync } from 'node:fs';
import { resolve, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

export function missingCoverage(root, report) {
  const gate = readFileSync(resolve(root, 'scripts/ui-tests'), 'utf8');
  const match = /--test-coverage-exclude='app\/ui\/js\/\{([^}]+)\}\.js'/.exec(gate);
  if (!match) throw new Error('Cannot read the UI coverage exclusion list');
  const excluded = new Set(match[1].split(',').map(name => `app/ui/js/${name}.js`));
  const covered = new Set([...report.matchAll(/^SF:(.+)$/gm)].map(([, file]) => relative(root, resolve(root, file.trim()))));
  const walk = dir => readdirSync(resolve(root, dir), { withFileTypes: true }).flatMap(entry =>
    entry.isDirectory() ? walk(`${dir}/${entry.name}`) : [`${dir}/${entry.name}`]);
  return walk('app/ui/js').filter(file => file.endsWith('.js') && !excluded.has(file) && !covered.has(file));
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const missing = missingCoverage(resolve(import.meta.dirname, '..'), readFileSync(process.argv[2], 'utf8'));
  if (missing.length) {
    console.error(`Production modules missing from coverage:\n${missing.join('\n')}`);
    process.exitCode = 1;
  } else console.log('ok: every eligible production UI module appears in coverage');
}
