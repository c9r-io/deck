// Source-level lints for the frontend. Everything here is a tripwire on
// shape (unkeyed copy, a retired feature returning, a forbidden pattern),
// never a proxy for behaviour: behaviour lives in the DOM/pure tests, the
// Rust unit and contract tests, and the real-WKWebView smoke.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { en } from '../js/i18n/en.js';
import { COPY_NO_SELECTION_REASONS, PROMOTION_SOURCES } from '../js/selection-forensics.js';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const read = path => readFileSync(resolve(root, path), 'utf8');
const production = readdirSync(resolve(root, 'app/ui/js')).filter(name => name.endsWith('.js'))
  .map(name => read(`app/ui/js/${name}`)).join('\n');

// The terminal is the unchanged published @xterm/xterm 5.5.0 artifact; any
// edit, upgrade or extra vendored file must show up here first.
test('vendored xterm is byte-identical to the published 5.5.0 build', () => {
  assert.deepEqual(readdirSync(resolve(root, 'app/ui/vendor')).sort(),
    ['addon-fit.js', 'xterm.css', 'xterm.js']);
  const digest = createHash('sha256').update(readFileSync(resolve(root, 'app/ui/vendor/xterm.js'))).digest('hex');
  assert.equal(digest, '1f991ac3b4b283ebf96e60ae23a00a52765dd3a2e46fa6fdda9f1aab032f7495');
});

test('extracted terminal adapters and queue view cannot import the view core', () => {
  for (const name of ['terminal-links', 'terminal-links-model', 'terminal-clipboard', 'terminal-bytes', 'voice-target', 'scheduler']) {
    assert.doesNotMatch(read(`app/ui/js/${name}.js`), /from\s+['"]\.\/(?:board|layout|terminal)\.js['"]/,
      `${name} receives view actions from its owner`);
  }
});

test('i18n owns visible copy and translation parameters never enter innerHTML', () => {
  const html = read('app/ui/index.html');
  for (const line of html.split('\n')) {
    if (!/>[^<{]*[A-Za-z][^<{]*</.test(line)) continue;
    if (/<(?:title|script|style|svg|path|circle|b)\b/.test(line)) continue;
    if (/class="wordmark"/.test(line)) continue;
    assert.match(line, /data-i18n(?:-title|-placeholder)?=/, `unkeyed visible HTML: ${line.trim()}`);
  }

  assert.doesNotMatch(production, /innerHTML\s*=\s*t\s*\(/);
  assert.doesNotMatch(production, /(?:toast|confirmDialog|promptDialog)\(\s*['"`][A-Za-z]/,
    'visible dynamic prose must use a stable translation key');
});

test('the updater, relaunch and server restart stay backend-owned', () => {
  const app = read('app/ui/js/app.js');
  const dialogs = read('app/ui/js/dialogs.js') + read('app/ui/js/settings.js');
  const html = read('app/ui/index.html');
  const capabilities = read('app/src-tauri/capabilities/default.json');
  // Endpoint selection, trust roots and downgrade refusal are unit-tested in
  // updater.rs; the webview only asks for a check on the chosen channel.
  assert.doesNotMatch(app, /__TAURI__\.updater|\.updater\.check/);
  assert.doesNotMatch(app + dialogs, /https?:\/\//, 'the webview owns no updater endpoint');
  assert.match(app, /inv\('check_for_update', \{ channel: ctx\.settings\.updateChannel \}\)/);
  assert.doesNotMatch(dialogs, /install_update|allow_downgrade|allowDowngrade/);
  assert.match(app, /inv\('relaunch_after_update'\)/,
    'verified installs cross a backend-owned clean relaunch boundary');
  assert.doesNotMatch(app, /process\.relaunch|__TAURI__\.process/,
    'the updater must not inherit the replaced app process group');
  assert.doesNotMatch(capabilities, /updater:/, 'the webview has no direct updater command permission');
  assert.doesNotMatch(capabilities, /process:/, 'the webview cannot invoke the generic Tauri restart path');
  assert.match(read('app/src-tauri/updater/nightly.pub.b64'), /^[A-Za-z0-9+/]+=*\n$/);
  // The one ordering the compiler cannot see: the creation embargo is set
  // before Tauri renames the running app, and a manual restart refuses to
  // start from an updater-relocated process.
  const lifecycle = read('app/src-tauri/src/tmux_lifecycle.rs');
  const updater = read('app/src-tauri/src/updater.rs');
  assert.match(lifecycle, /fn begin_app_update_install\(\)[\s\S]*?try_operation\(\)\?[\s\S]*?APP_UPDATE_INSTALLING\.store\(true/,
    'updater installation serializes with restart and session creation');
  assert.match(lifecycle, /fn restart_tmux_server\([\s\S]*?APP_UPDATE_INSTALLING\.load\(Ordering::Acquire\)/,
    'manual replacement cannot start from an updater-relocated process');
  const install = updater.indexOf('fn install_update(');
  assert.ok(install > 0);
  assert.ok(updater.indexOf('writable_bundle()?', install) < updater.indexOf('begin_app_update_install()?', install)
    && updater.indexOf('begin_app_update_install()?', install) < updater.indexOf('.download(', install),
    'writability is refused first, then the old process is embargoed, then the archive is fetched and deck relocates itself');
  assert.doesNotMatch(updater.replace(/^\s*\/\/.*$/gm, ''), /download_and_install|\.install\(/,
    'the plugin installer (admin AppleScript, PATH touch) is never called');
  // The restart confirmation defaults to "later" and Enter cannot accept it.
  assert.match(html, /id="tmux-later"[\s\S]*id="tmux-restart"/);
  assert.match(app, /\$\('tmux-later'\)\.focus\(\)/);
  assert.match(app, /if \(event\.key === 'Enter'\) \{ event\.preventDefault\(\); event\.stopPropagation\(\); \}/);
  assert.match(app, /expectedImpactToken: status\.impactToken/,
    'restart executes only against the reviewed session/pane identity set');
  assert.match(app, /restart_tmux_server[\s\S]*markSessionsStoppedForServerRestart\(\)/,
    'a backend blocker refusal must not present ordinary cards as stopped');
  assert.match(app, /closeManagedForRestart\(review[\s\S]*restart_tmux_server/,
    'known managed blockers use the local close transaction before restart');
  assert.match(app, /managedBlockers\(status\)[\s\S]*tmux\.managedExplanation/,
    'restart review explains and lists managed blockers');
  assert.match(app, /row\.textContent = t\('tmux\.managedCard'/,
    'the review lists each blocking card from backend status');
  assert.match(app, /list\.style\.display = managedBlockers\(status\)\.length \? 'block'/,
    'blocking cards are visible before a destructive click');
  assert.match(app, /if \(!review \|\| ctx\.tmuxRestarting\) return;/,
    'repeat clicks cannot start another restart transaction');
  const run = read('app/run.sh');
  assert.match(run, /BUNDLE_ID=io\.c9r\.deck\.dev/);
  assert.match(run, /deck-smoke\*/);
  const plist = read('app/src-tauri/Info.plist');
  assert.match(plist, /NSLocalNetworkUsageDescription/);
  assert.match(plist, /Shell commands, CLIs, and agents launched or restored by deck can access devices and services on your local network\./);
  assert.doesNotMatch(plist, /scan/i);
});

test('the canonical dictionary has no unused keys outside documented dynamic families', () => {
  const source = read('app/ui/index.html') + production;
  const dynamic = /^(?:attention\.column|attention\.filter|automation\.run|automation\.wd|board\.default|buffer\.state|mcp\.tunnelState|session\.status|settings\.shortcut|settings\.notifyStatus|notice|tmux\.notice|voice\.phase|voice\.error|voice\.notice)\./;
  const unused = Object.keys(en).filter(key => !dynamic.test(key) && !source.includes(key));
  assert.deepEqual(unused, []);
});

test('the coverage exclusion list only shrinks: new logic lands in a measured *-model.js', () => {
  const gate = read('scripts/ui-tests');
  const [, list] = /--test-coverage-exclude='app\/ui\/js\/\{([^}]+)\}\.js'/.exec(gate) || [];
  assert.ok(list, 'the WKWebView-bound exclusion list is the one brace group in scripts/ui-tests');
  const frozen = ['app', 'attention', 'automation', 'board', 'dropdown', 'layout', 'scheduler', 'queue-review', 'selection', 'terminal'];
  for (const name of list.split(',')) {
    assert.ok(frozen.includes(name), `${name}.js joined the coverage exclusion list; split its DOM-free half into ${name}-model.js instead`);
  }
  for (const model of ['attention-model', 'automation-model', 'scheduler-model']) {
    assert.doesNotMatch(read(`app/ui/js/${model}.js`), /\b(?:document|window|navigator)\.|__TAURI__/, `${model}.js stays DOM-free`);
  }
});

test('minimum-window layout keeps long localized panels bounded and scrollable', () => {
  const html = read('app/ui/style.css');
  assert.match(html, /@media \(max-width: 800px\), \(max-height: 540px\)/);
  assert.match(html, /#settings-modal, #tpl-modal \{[^}]*align-items: center;[^}]*padding: 20px;/);
  assert.match(html, /#settings-box, #tpl-box \{[^}]*width: 940px;[^}]*max-height: 100%;[^}]*overflow: hidden;/,
    'settings frame stays inside the viewport');
  assert.match(html, /#tpl-box \{ width: \d+px; height: \d+px; \}/,
    'the template manager shares that frame and only resizes it');
  assert.match(html, /#set-content, #tpl-content \{[^}]*min-width: 0;[^}]*overflow-y: auto;/,
    'only modal content scrolls; navigation and footer remain reachable');
  assert.match(html, /#cfm-box, #ppd-box \{[^}]*max-height: 84vh;[^}]*overflow-y: auto;/);
  assert.match(html, /#queue-panel \{[^}]*max-height: 55vh;/);
  assert.match(html, /\.qg-row \.row-meta \{[^}]*white-space: normal;/);
});

test('large font scaling reflows dense rows instead of clipping scaled line boxes', () => {
  const html = read('app/ui/style.css');
  const fontScale = read('app/ui/js/font-scale.js');
  const settings = read('app/ui/js/settings.js');
  assert.match(fontScale, /classList\?\.toggle\('font-scale-large', current >= 1\.4\)/);
  assert.match(html, /html\.font-scale-large \.set-row \{[^}]*flex-wrap: wrap;/);
  assert.match(html, /html\.font-scale-large \.shortcut-row \{[^}]*grid-template-columns: minmax\(0, 1fr\);/);
  assert.match(html, /html\.font-scale-large \.sess-head \{[^}]*flex-wrap: wrap;/);
  assert.match(html, /html\.font-scale-large \.q-add,[\s\S]*?flex-wrap: wrap;/);
  assert.match(html, /\.card-meta \{[\s\S]*?min-height: 1\.53846rem;/);
  assert.doesNotMatch(html, /(?:\.card-meta|\.sess-head \.btn)[^{]*\{[^}]*(?:height: 20px|height: 47px|height: 28px)/);
  assert.match(settings, /for \(const action of CUSTOMIZABLE_SHORTCUT_ACTIONS\)/,
    'fixed US/JIS font gestures stay out of the shortcut editor');
});

test('retired features do not return', () => {
  const html = read('app/ui/index.html');
  const layout = read('app/ui/js/layout.js');
  const terminal = read('app/ui/js/terminal.js');
  const scheduler = read('app/ui/js/scheduler.js');
  const backend = ['app/src-tauri/src/main.rs', 'app/src-tauri/src/commands.rs',
    'app/src-tauri/src/terminal.rs'].map(read).join('\n');
  const production = [html, layout, terminal, read('app/ui/js/pure.js'), backend].join('\n');
  // the long-output copy panel (tmux selection replaced it)
  for (const token of ['copybox', 'cb-body', 'Copy output', 'Copy all', 'openCopyPanel',
    'closeCopyPanel', 'copyPanelOpen', 'capture_scrollback', 'cbtn', '⌘⇧C']) {
    assert.equal(production.includes(token), false, `removed feature token remains: ${token}`);
  }
  // moving a card between Boards from the session header (Board DnD only)
  assert.doesNotMatch(html + layout + terminal, /sess-col|session\.moveBoard/);
  assert.match(read('app/ui/js/board.js'), /provider\.move\(sid, c\.id\)/);
  // a persisted scheduler safety policy (context protection is automatic)
  assert.doesNotMatch(scheduler, /queue_set_policy|safetyPolicy|acceptRisk/);
  assert.doesNotMatch(html, /id="q-policy"/);
  // a one-shot foreground-mismatch bypass (external text could reach a shell)
  assert.doesNotMatch(scheduler + read('app/src-tauri/src/scheduler/ops.rs'),
    /acceptProcessMismatch|accept_process_mismatch|manualMismatchConfirm/);
  // a webview-side shell recovery layer (restore is tmux history)
  assert.doesNotMatch(layout + html + read('app/src-tauri/src/main.rs'),
    /shell-recovery|recoverychip|load_shell_snapshot/);
  assert.match(layout, /outcome\.restored = !!started\.restored/);
  // the six-row terminal preview on Board cards (02 B v02: output is read in the terminal)
  const board = read('app/ui/js/board.js');
  assert.doesNotMatch(board + html + read('app/ui/style.css') + read('app/ui/js/pure.js'),
    /card-tail|cardPreviewRows|CARD_PREVIEW_ROWS|emit\('output'/);
  assert.match(board, /const tailFor = \[\];/, 'the Board poll never requests pane captures');
  // the description line is reserved so a card keeps its height with or without one
  assert.match(board, /<div class="card-desc"><\/div>`;/);
  assert.match(read('app/ui/style.css'), /\.card-desc \{[\s\S]*?min-height: 1\.23846rem;/);
  assert.match(layout, /if \(created && !restored\)[^\n]*clear_history/,
    'restored tmux history must survive the fresh-shell cleanup');
});

test('terminal input and selection ownership tripwires', () => {
  const layout = read('app/ui/js/layout.js');
  const selection = read('app/ui/js/selection.js');
  // The behaviour (drag promotion, frozen lease, overlay, wheel routing) is
  // exercised by the WKWebView smoke and the tmux contract tests; these are
  // the patterns that once broke it.
  assert.doesNotMatch(selection, /replayClick|new MouseEvent\(['"]mousedown/,
    'no synthetic compatibility click is replayed');
  assert.doesNotMatch(selection, /distance\s*<\s*4/,
    'terminal drag ownership must not depend on an arbitrary CSS-pixel threshold');
  assert.doesNotMatch(selection, /options\.disableStdin\s*=\s*true/);
  assert.doesNotMatch(layout, /wheelTimer[\s\S]*?50/);
  assert.match(layout, /macOptionIsMeta: false/, 'Option stays owned by macOS text input');
  assert.match(selection, /grid: \{ cols: pane\.term\.cols, rows: pane\.term\.rows \}/,
    'every selection call carries the confirmed frontend grid');
});

test('WK clipboard expected value is generated independently of production copy', () => {
  const smoke = read('app/ui/test/wk-smoke.mjs');
  assert.match(smoke, /fixtureClipboardLine/);
  assert.match(smoke, /expectedHash = fnv1a64\(expected\)/);
  assert.doesNotMatch(smoke, /keySelection\s*=\s*await copyTerminalSelection/);
});

test('clipboard and selection diagnostics are wired at every handoff', () => {
  // Whether each label survives the backend formatter is proven in
  // diagnostics.rs (`every_frontend_event_label_survives_the_formatter`);
  // this only checks that no handoff lost its call.
  const layout = read('app/ui/js/layout.js');
  const clipboard = read('app/ui/js/terminal-clipboard.js');
  const selection = read('app/ui/js/selection.js');
  for (const stage of ['keydown-deck', 'keydown-native', 'keydown-none',
    'keydown-elsewhere', 'source-elsewhere'])
    assert.ok(clipboard.includes(stage), `missing copy diagnostic: ${stage}`);
  for (const stage of ['pbcopy-failed', 'web-failed', 'web-unavailable'])
    assert.ok(clipboard.includes(stage), `missing clipboard writer diagnostic: ${stage}`);
  for (const stage of ['promote', 'start-ok', 'start-failed', 'finish-ok', 'finish-failed',
    'update-failed', 'dimensions-changed', 'freeze-ok', 'freeze-failed', 'native-cleared'])
    assert.ok(selection.includes(`sev('${stage}'`), `selection stage never logged: ${stage}`);
  for (const label of ['revoker-mouse', 'revoker-touch', 'revoker-pen', 'revoker-unknown', 'revoker-synthetic'])
    assert.ok(selection.includes(`'${label}'`), `revoker class never wired: ${label}`);
  // No cancel may reach the log anonymously: it names the revoke, or the
  // caller already logged a more specific failure (explicit null).
  assert.doesNotMatch(selection, /(?:^|[^.\w])cancel\(\s*(?:true|false)?\s*\)/,
    'every selection cancel must carry a reason label or an explicit null');
  const reasons = new Set();
  for (const source of [selection, layout])
    for (const [, r] of source.matchAll(/cancel(?:TerminalSelection|AllTerminalSelections)?\(\s*(?:pane|previous|p|true|false)?\s*,?\s*'([a-z-]+)'\s*\)/g))
      reasons.add(r);
  for (const r of ['pointer', 'pointer-cancel', 'blur', 'hidden', 'input', 'escape',
    'focus', 'live', 'exit', 'leave', 'dispose'])
    assert.ok(reasons.has(r), `revoke reason never wired: ${r}`);
});

test('forensic reasons and movement sources are closed at the Rust log boundary', () => {
  const rust = read('app/src-tauri/src/diagnostics.rs');
  const selection = read('app/ui/js/selection.js');
  for (const reason of COPY_NO_SELECTION_REASONS)
    assert.ok(rust.includes(`"copy-no-selection-${reason}"`), reason);
  for (const source of PROMOTION_SOURCES) {
    assert.ok(rust.includes(`"copy-promotion-${source}"`), source);
    assert.ok(rust.includes(`"copy-gesture-${source}"`) || source === 'up', source);
  }
  assert.match(selection, /forensics\.reason\(\)/);
  assert.doesNotMatch(read('app/ui/js/selection-forensics.js'), /getSelection\(|terminal_selection_copy|clipboard|session/);
});
