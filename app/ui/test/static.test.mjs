// Source-level lints for the frontend. Everything here is a tripwire on
// shape (unkeyed copy, a retired feature returning, a forbidden pattern),
// never a proxy for behaviour: behaviour lives in the DOM/pure tests, the
// Rust unit and contract tests, and the real-WKWebView smoke.
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
    'app/ui/js/inbound.js', 'app/ui/js/layout.js', 'app/ui/js/scheduler.js', 'app/ui/js/selection.js',
    'app/ui/js/templates.js', 'app/ui/js/terminal.js'].map(read).join('\n');
  assert.doesNotMatch(production, /innerHTML\s*=\s*t\s*\(/);
  assert.doesNotMatch(production, /(?:toast|confirmDialog|promptDialog)\(\s*['"`][A-Za-z]/,
    'visible dynamic prose must use a stable translation key');
});

test('the updater, relaunch and server restart stay backend-owned', () => {
  const app = read('app/ui/js/app.js');
  const dialogs = read('app/ui/js/dialogs.js');
  const html = read('app/ui/index.html');
  const capabilities = read('app/src-tauri/capabilities/default.json');
  // Endpoint selection, trust roots and downgrade refusal are unit-tested in
  // updater.rs; the webview only asks for a check on the chosen channel.
  assert.doesNotMatch(app, /__TAURI__\.updater|\.updater\.check/);
  assert.doesNotMatch(app + dialogs, /https?:\/\//, 'the webview owns no updater endpoint');
  assert.match(app, /inv\('check_for_update', \{ channel: settings\.updateChannel \}\)/);
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
  assert.ok(updater.indexOf('begin_app_update_install') < updater.indexOf('.download_and_install('),
    'old updater process is embargoed before Tauri relocates the app');
  // The restart confirmation defaults to "later" and Enter cannot accept it.
  assert.match(html, /id="tmux-later"[\s\S]*id="tmux-restart"/);
  assert.match(app, /\$\('tmux-later'\)\.focus\(\)/);
  assert.match(app, /if \(event\.key === 'Enter'\) \{ event\.preventDefault\(\); event\.stopPropagation\(\); \}/);
  assert.match(app, /expectedImpactToken: status\.impactToken/,
    'restart executes only against the reviewed session/pane identity set');
  assert.match(app, /markSessionsStoppedForServerRestart\(\)[\s\S]*restart_tmux_server/);
  const run = read('app/run.sh');
  assert.match(run, /BUNDLE_ID=io\.c9r\.deck\.dev/);
  assert.match(run, /deck-smoke\*/);
  const plist = read('app/src-tauri/Info.plist');
  assert.match(plist, /NSLocalNetworkUsageDescription/);
  assert.match(plist, /Shell commands, CLIs, and agents launched or restored by deck can access devices and services on your local network\./);
  assert.doesNotMatch(plist, /scan/i);
});

test('the canonical dictionary has no unused keys outside documented dynamic families', () => {
  const source = ['app/ui/index.html', 'app/ui/js/app.js', 'app/ui/js/board.js',
    'app/ui/js/dialogs.js', 'app/ui/js/i18n.js', 'app/ui/js/inbound.js', 'app/ui/js/layout.js',
    'app/ui/js/pure.js', 'app/ui/js/scheduler.js', 'app/ui/js/selection.js', 'app/ui/js/state.js',
    'app/ui/js/templates.js', 'app/ui/js/terminal.js'].map(read).join('\n');
  const dynamic = /^(?:board\.default|board\.hint|session\.status|settings\.shortcut|notice|tmux\.notice)\./;
  const unused = Object.keys(en).filter(key => !dynamic.test(key) && !source.includes(key));
  assert.deepEqual(unused, []);
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
  const dialogs = read('app/ui/js/dialogs.js');
  assert.match(fontScale, /classList\?\.toggle\('font-scale-large', current >= 1\.4\)/);
  assert.match(html, /html\.font-scale-large \.set-row \{[^}]*flex-wrap: wrap;/);
  assert.match(html, /html\.font-scale-large \.shortcut-row \{[^}]*grid-template-columns: minmax\(0, 1fr\);/);
  assert.match(html, /html\.font-scale-large \.sess-head \{[^}]*flex-wrap: wrap;/);
  assert.match(html, /html\.font-scale-large \.q-item,[\s\S]*?flex-wrap: wrap;/);
  assert.match(html, /\.card-meta \{[\s\S]*?min-height: 1\.53846rem;/);
  assert.match(html, /\.card-tail \{[\s\S]*?height: calc\(7\.61538rem \+ 14px\);/);
  assert.doesNotMatch(html, /(?:\.card-meta|\.card-tail|\.sess-head \.btn)[^{]*\{[^}]*(?:height: 20px|height: 47px|height: 28px)/);
  assert.match(dialogs, /for \(const action of CUSTOMIZABLE_SHORTCUT_ACTIONS\)/,
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
  assert.match(scheduler, /acceptProcessMismatch: mismatch/);
  // a webview-side shell recovery layer (restore is tmux history)
  assert.doesNotMatch(layout + html + read('app/src-tauri/src/main.rs'),
    /shell-recovery|recoverychip|load_shell_snapshot/);
  assert.match(layout, /outcome\.restored = !!started\.restored/);
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
  const terminal = read('app/ui/js/terminal.js');
  const selection = read('app/ui/js/selection.js');
  for (const stage of ['pasteTrace.keyCapture()', 'pasteTrace.keyHandler()', 'pasteTrace.event(',
    'pasteTrace.onData(', 'pasteTrace.write('])
    assert.ok(layout.includes(stage), `missing paste diagnostic handoff: ${stage}`);
  for (const stage of ['key-capture', 'keydown-deck', 'keydown-native', 'keydown-none',
    'keydown-elsewhere', 'selection-vanished'])
    assert.ok(layout.includes(stage), `missing copy diagnostic: ${stage}`);
  for (const stage of ['pbcopy-success', 'pbcopy-failed', 'web-success', 'web-failed', 'web-unavailable'])
    assert.ok(terminal.includes(stage), `missing clipboard writer diagnostic: ${stage}`);
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
