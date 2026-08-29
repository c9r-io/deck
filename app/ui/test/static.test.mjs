import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { en } from '../js/i18n/en.js';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = path => readFileSync(resolve(root, path), 'utf8');

test('i18n owns visible copy and translation parameters never enter innerHTML', () => {
  const html = read('app/ui/index.html');
  for (const line of html.split('\n')) {
    if (!/>[^<{]*[A-Za-z][^<{]*</.test(line)) continue;
    if (/<(?:title|script|style|svg|path|circle|b)\b/.test(line)) continue;
    if (/class="wordmark"/.test(line)) continue;
    assert.match(line, /data-i18n(?:-title|-placeholder)?=/, `unkeyed visible HTML: ${line.trim()}`);
  }
  const production = ['app/ui/js/app.js', 'app/ui/js/board.js', 'app/ui/js/dialogs.js',
    'app/ui/js/layout.js', 'app/ui/js/scheduler.js', 'app/ui/js/selection.js',
    'app/ui/js/terminal.js'].map(read).join('\n');
  assert.doesNotMatch(production, /innerHTML\s*=\s*t\s*\(/);
  assert.doesNotMatch(production, /(?:toast|confirmDialog|promptDialog)\(\s*['"`][A-Za-z]/,
    'visible dynamic prose must use a stable translation key');
});

test('the canonical dictionary has no unused keys outside documented dynamic families', () => {
  const source = ['app/ui/index.html', 'app/ui/js/app.js', 'app/ui/js/board.js',
    'app/ui/js/dialogs.js', 'app/ui/js/i18n.js', 'app/ui/js/layout.js',
    'app/ui/js/pure.js', 'app/ui/js/scheduler.js', 'app/ui/js/selection.js', 'app/ui/js/state.js',
    'app/ui/js/terminal.js'].map(read).join('\n');
  const dynamic = /^(?:board\.default|board\.hint|session\.status|notice)\./;
  const unused = Object.keys(en).filter(key => !dynamic.test(key) && !source.includes(key));
  assert.deepEqual(unused, []);
});

test('minimum-window layout keeps long localized panels bounded and scrollable', () => {
  const html = read('app/ui/index.html');
  assert.match(html, /@media \(max-width: 800px\), \(max-height: 540px\)/);
  assert.match(html, /#settings-box \{[^}]*max-height: 92vh;[^}]*overflow-y: auto;/);
  assert.match(html, /#cfm-box, #ppd-box \{[^}]*max-height: 84vh;[^}]*overflow-y: auto;/);
  assert.match(html, /#queue-panel \{[^}]*max-height: 55vh;/);
  assert.match(html, /\.qg-row \.row-meta \{[^}]*white-space: normal;/);
});

test('high-risk scheduler confirmation cannot be accepted by ordinary Enter', () => {
  const dialogs = read('app/ui/js/dialogs.js');
  assert.match(dialogs, /confirmDangerDialog/);
  assert.match(dialogs, /if \(!confirmPointerOnly\) cfmDone\(true\)/);
  const scheduler = read('app/ui/js/scheduler.js');
  assert.match(scheduler, /confirmDangerDialog\(message\)/);
  assert.match(scheduler, /acceptProcessMismatch: mismatch/);
  assert.doesNotMatch(scheduler, /queue_set_policy|safetyPolicy|acceptRisk/);
  assert.doesNotMatch(read('app/ui/index.html'), /id="q-policy"/);
});

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
  const backendScroll = read('app/src-tauri/src/terminal_scroll.rs');
  assert.match(layout, /wireTerminalSelection\(pane/);
  assert.match(selection, /terminal_selection_start/);
  assert.match(selection, /terminal_selection_update/);
  assert.match(selection, /pointerDown[\s\S]*?preventDefault\(\)[\s\S]*?stopImmediatePropagation\(\)/);
  assert.match(selection, /\['mousedown', 'mousemove', 'mouseup', 'click', 'dblclick'\]/);
  assert.match(selection, /compatibilityBlocked/);
  assert.match(selection, /setMode\(selected \|\| !!status\.selection_present\)/);
  assert.match(layout, /createTerminalWheelAccumulator/);
  assert.match(layout, /requestAnimationFrame[\s\S]*?wheelInFlight/);
  assert.doesNotMatch(layout, /wheelTimer[\s\S]*?50/);
  assert.match(backend, /copy-mode/);
  assert.match(backend, /terminal_scroll::args/);
  assert.match(backendScroll, /if-shell[\s\S]*?display-message/);
  assert.match(read('app/ui/test/wk-smoke.mjs'), /selection-repeat[\s\S]*?scroll-frame/);
  assert.match(backend, /selection_start_y/);
  assert.match(backend, /dims\.selection_present[\s\S]*?clear-selection/);
  assert.match(backend, /if !before\.active \{/);
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
