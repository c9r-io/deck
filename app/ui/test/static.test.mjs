import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = path => readFileSync(resolve(root, path), 'utf8');

test('removed long-output panel cannot return through DOM, routes, or backend registration', () => {
  const production = [
    'app/ui/index.html',
    'app/ui/js/layout.js',
    'app/ui/js/terminal.js',
    'app/ui/js/pure.js',
    'app/src-tauri/src/main.rs',
    'app/src-tauri/src/commands.rs',
  ].map(read).join('\n');
  for (const forbidden of [
    'copybox', 'cb-body', 'Copy output', 'Copy all', 'openCopyPanel',
    'closeCopyPanel', 'copyPanelOpen', 'capture_scrollback', 'cbtn', '⌘⇧C',
  ]) {
    assert.equal(production.includes(forbidden), false, `removed feature token remains: ${forbidden}`);
  }
});

test('production terminal path wires the tmux-owned selection coordinator', () => {
  const layout = read('app/ui/js/layout.js');
  const selection = read('app/ui/js/selection.js');
  const backend = read('app/src-tauri/src/commands.rs');
  const backendSelection = read('app/src-tauri/src/terminal_selection.rs');
  assert.match(layout, /wireTerminalSelection\(pane/);
  assert.match(selection, /terminal_selection_start/);
  assert.match(selection, /terminal_selection_update/);
  assert.match(selection, /pointerDown[\s\S]*?preventDefault\(\)[\s\S]*?stopImmediatePropagation\(\)/);
  assert.match(selection, /\['mousedown', 'mousemove', 'mouseup', 'click', 'dblclick'\]/);
  assert.match(selection, /compatibilityBlocked/);
  assert.match(backend, /copy-mode/);
  assert.match(backend, /selection_start_y/);
  assert.match(backend, /snapshot_selection/);
  assert.match(backendSelection, /copy-selection-no-clear/);
  assert.match(backendSelection, /show-buffer/);
  assert.match(backendSelection, /delete-buffer/);
  assert.doesNotMatch(backend, /extract_terminal_selection/);
  assert.doesNotMatch(selection, /\._core/);
});

test('WK clipboard expected value is generated independently of production copy', () => {
  const smoke = read('app/ui/test/wk-smoke.mjs');
  assert.match(smoke, /fixtureClipboardLine/);
  assert.match(smoke, /expectedHash = fnv1a64\(expected\)/);
  assert.doesNotMatch(smoke, /keySelection\s*=\s*await copyTerminalSelection/);
});
