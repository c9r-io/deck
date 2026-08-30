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

test('update channels are backend-owned, isolated and never trigger downgrade', () => {
  const app = read('app/ui/js/app.js');
  const dialogs = read('app/ui/js/dialogs.js');
  const backend = read('app/src-tauri/src/commands.rs');
  assert.doesNotMatch(app, /__TAURI__\.updater|\.updater\.check/);
  assert.doesNotMatch(app + dialogs, /https?:\/\//, 'the webview owns no updater endpoint');
  assert.match(app, /inv\('check_for_update', \{ channel: settings\.updateChannel \}\)/);
  assert.match(backend, /STABLE_UPDATE_ENDPOINT[\s\S]*releases\/latest\/download\/latest\.json/);
  assert.match(backend, /NIGHTLY_UPDATE_ENDPOINT[\s\S]*releases\/download\/nightly-feed\/latest\.json/);
  assert.match(backend, /\.endpoints\(vec!\[endpoint\]\)/, 'each check has exactly one endpoint');
  assert.doesNotMatch(dialogs, /install_update|allow_downgrade|allowDowngrade/);
  assert.match(backend, /download_and_install/, 'Tauri retains download, verification and install ownership');
  assert.doesNotMatch(read('app/src-tauri/capabilities/default.json'), /updater:/,
    'the webview has no direct updater command permission');
});

test('tmux upgrades preserve occupied servers until explicit pointer confirmation', () => {
  const lifecycle = read('app/src-tauri/src/tmux_lifecycle.rs');
  const commands = read('app/src-tauri/src/commands.rs');
  const app = read('app/ui/js/app.js');
  const html = read('app/ui/index.html');
  const run = read('app/run.sh');
  const plist = read('app/src-tauri/Info.plist');

  for (const field of ['schema_version', 'protocol_version', 'channel', 'bundle_identifier',
    'app_version', 'build_identifier', 'helper_version', 'created_at', 'source']) {
    assert.match(lifecycle, new RegExp(`\\b${field}\\b`), `server metadata includes ${field}`);
  }
  assert.match(lifecycle, /should_auto_replace\(state, snapshot\.sessions\.len\(\)\)/);
  assert.match(lifecycle, /snapshot\.pid != expected_pid[\s\S]*snapshot\.started_at != expected_started_at[\s\S]*snapshot\.sessions\.len\(\) as u32 != expected_session_count[\s\S]*snapshot\.pane_count\(\) != expected_pane_count/);
  assert.match(lifecycle, /pty_state\.detach_all\(\)[\s\S]*complete_restart/);
  assert.match(lifecycle, /fresh\.pid == old\.pid[\s\S]*CompatibleCurrentBuild/);
  assert.match(lifecycle, /APP_UPDATE_INSTALLING\.load\(Ordering::Acquire\)/);
  assert.match(lifecycle, /fn begin_app_update_install\(\)[\s\S]*?try_operation\(\)\?[\s\S]*?APP_UPDATE_INSTALLING\.store\(true/,
    'updater installation serializes with restart and session creation');
  assert.match(lifecycle, /fn restart_tmux_server\([\s\S]*?APP_UPDATE_INSTALLING\.load\(Ordering::Acquire\)/,
    'manual replacement cannot start from an updater-relocated process');
  assert.ok(commands.indexOf('begin_app_update_install') < commands.indexOf('.download_and_install('),
    'old updater process is embargoed before Tauri relocates the app');

  assert.match(html, /id="tmux-later"[\s\S]*id="tmux-restart"/);
  assert.match(app, /\$\('tmux-later'\)\.focus\(\)/);
  assert.match(app, /if \(event\.key === 'Enter'\) \{ event\.preventDefault\(\); event\.stopPropagation\(\); \}/);
  assert.match(app, /defer_tmux_restart/);
  assert.match(app, /markSessionsStoppedForServerRestart\(\)[\s\S]*restart_tmux_server/);

  assert.match(run, /BUNDLE_ID=io\.c9r\.deck\.dev/);
  assert.match(run, /deck-smoke\*/);
  assert.match(plist, /NSLocalNetworkUsageDescription/);
  assert.match(plist, /Terminal tools running in deck may connect to local services and devices you choose\./);
  assert.doesNotMatch(plist, /scan/i);
});

test('the canonical dictionary has no unused keys outside documented dynamic families', () => {
  const source = ['app/ui/index.html', 'app/ui/js/app.js', 'app/ui/js/board.js',
    'app/ui/js/dialogs.js', 'app/ui/js/i18n.js', 'app/ui/js/layout.js',
    'app/ui/js/pure.js', 'app/ui/js/scheduler.js', 'app/ui/js/selection.js', 'app/ui/js/state.js',
    'app/ui/js/terminal.js'].map(read).join('\n');
  const dynamic = /^(?:board\.default|board\.hint|session\.status|notice|tmux\.notice)\./;
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

test('shell restart recovery is real tmux history, never a blocking webview layer', () => {
  const layout = read('app/ui/js/layout.js');
  const html = read('app/ui/index.html');
  const main = read('app/src-tauri/src/main.rs');
  const commands = read('app/src-tauri/src/commands.rs');
  const recovery = read('app/src-tauri/src/shell_state.rs');
  const production = layout + html + main;

  assert.doesNotMatch(production, /shell-recovery|recoverychip|load_shell_snapshot/);
  assert.match(layout, /outcome\.restored = !!started\.restored/);
  assert.match(layout, /if \(created && !restored\)[^\n]*clear_history/,
    'restored tmux history must survive the fresh-shell cleanup');
  assert.match(commands, /prepare_bootstrap/);
  assert.match(commands, /BOOTSTRAP_ARG/);
  assert.match(recovery, /out\.write_all\(transcript\.as_bytes\(\)\)/);
  assert.match(recovery, /Command::new\(shell\)\.arg0\(login_name\)\.exec\(\)/);
  assert.match(recovery, /custom_flags\(libc::O_NOFOLLOW\)/);
  assert.doesNotMatch(recovery, /recovered_prefixes|merge_transcripts/,
    'a transcript already in tmux must not be appended out of band again');
});

test('production terminal path wires the token-bound frozen selection coordinator', () => {
  const layout = read('app/ui/js/layout.js');
  const selection = read('app/ui/js/selection.js');
  const backend = read('app/src-tauri/src/commands.rs');
  const backendSelection = read('app/src-tauri/src/terminal_selection.rs');
  const backendScroll = read('app/src-tauri/src/terminal_scroll.rs');
  assert.match(layout, /wireTerminalSelection\(pane/);
  assert.match(selection, /terminal_selection_start/);
  assert.match(selection, /terminal_selection_update/);
  assert.doesNotMatch(selection, /replayClick|new MouseEvent\(['"]mousedown/);
  assert.match(selection, /trustedClick/);
  assert.match(selection, /compatibilityBlocked/);
  assert.match(selection, /terminal_selection_finish/);
  assert.match(selection, /terminal_selection_scroll/);
  assert.match(selection, /onModeChange\(true, lastStatus\)/,
    'a frozen-selection scroll must publish live-cursor visibility');
  assert.match(selection, /terminalSelectionOverlayRows/);
  assert.match(selection, /status reply and its PTY repaint have no fixed ordering/,
    'selection scrolling must handle either tmux-status/xterm-frame ordering');
  assert.match(selection, /pane\.term\.onSelectionChange/,
    'promoted pointer drags must clear late native xterm selections');
  assert.match(selection, /getSelectionPosition/,
    'native xterm word/line selections need public coordinates for wheel adoption');
  assert.doesNotMatch(selection, /distance\s*<\s*4/,
    'terminal drag ownership must not depend on an arbitrary CSS-pixel threshold');
  assert.match(selection, /if \(!ended\.promoted\) promote\(\)/,
    'pointerup must recover a cell transition from a coalesced final pointermove');
  assert.match(selection, /updateAt\(currentToken, finalPoint, false\)/,
    'pointerup must position exactly at the pointer without another edge scroll');
  assert.match(selection, /grid: \{ cols: pane\.term\.cols, rows: pane\.term\.rows \}/);
  assert.match(layout, /term\.hasSelection\(\)[\s\S]{0,80}e\.preventDefault\(\)/,
    'xterm-native Command-C must suppress WebKit default copy');
  assert.match(layout, /createTerminalResizeCoordinator/);
  assert.match(layout, /pane\.syncSize = \(\) => resize\.sync/);
  assert.match(layout, /pane\.selection\?\.writeParsed\(\)/,
    'selection frame barrier must observe every parsed xterm write');
  assert.match(layout, /pane\.selection\.freezeNative\(\)[\s\S]{0,160}pane\.selection\.scroll\(lines\)/,
    'the first native-selection wheel frame must join the frozen tmux scroll path');
  assert.match(layout, /createTerminalWheelAccumulator/);
  assert.match(layout, /terminalAgentHistoryUpRoute/);
  assert.match(layout, /term\.input\(AGENT_HISTORY_VERTICAL_UP\)/,
    'agent history workaround must re-enter the ordinary xterm input path');
  assert.match(layout, /createTerminalWheelFrameScheduler/);
  assert.match(read('app/ui/js/pure.js'), /if \(inFlight\) \{[\s\S]{0,80}schedule\(\)/,
    'a busy wheel frame must remain armed instead of waiting for Promise completion');
  assert.doesNotMatch(layout, /wheelTimer[\s\S]*?50/);
  assert.match(backend, /copy-mode/);
  assert.match(backend,
    /terminal_selection_scroll[\s\S]*?terminal_scroll::cursor_following_args/,
    'frozen-selection scrolling must re-anchor the copy cursor to live input');
  assert.match(backendScroll, /if-shell[\s\S]*?display-message/);
  assert.match(read('app/ui/test/wk-smoke.mjs'),
    /selection-scroll-stable[\s\S]*?selection-scroll-cursor[\s\S]*?selection-overlay[\s\S]*?selection-repeat[\s\S]*?selection-resize[\s\S]*?scroll-frame/,
    'real WKWebView smoke must verify frozen-selection cursor position, visibility and DOM cleanup');
  assert.match(backend, /selection_start_y/);
  assert.match(backend, /dims\.selection_present[\s\S]*?clear-selection/);
  assert.match(backend, /if !before\.active \{/);
  assert.match(backend, /snapshot_selection/);
  assert.match(backend, /TerminalSelectionLease::Frozen/);
  assert.match(backend, /selection_token_matches/);
  assert.match(backend, /selection-dimensions-changed/);
  assert.match(backendSelection, /copy-selection-no-clear/);
  assert.match(backendSelection, /show-buffer/);
  assert.match(backendSelection, /delete-buffer/);
  assert.doesNotMatch(backend, /extract_terminal_selection/);
  assert.doesNotMatch(selection, /\._core/);
  assert.doesNotMatch(selection, /options\.disableStdin\s*=\s*true/);
  assert.match(layout, /isComposingKeyEvent\(e\)/);
  assert.match(layout, /shouldRouteImeKeydownThroughInput\(event\)/);
  assert.match(layout, /isPlainShiftKeydown\(event\)/);
  assert.match(layout, /event\.stopPropagation\(\)/);
  assert.match(layout, /macOptionIsMeta: false/);
});

test('WK clipboard expected value is generated independently of production copy', () => {
  const smoke = read('app/ui/test/wk-smoke.mjs');
  assert.match(smoke, /fixtureClipboardLine/);
  assert.match(smoke, /expectedHash = fnv1a64\(expected\)/);
  assert.doesNotMatch(smoke, /keySelection\s*=\s*await copyTerminalSelection/);
});
