import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, readdirSync } from 'node:fs';
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

test('update channels are backend-owned, isolated and never trigger downgrade', () => {
  const app = read('app/ui/js/app.js');
  const dialogs = read('app/ui/js/dialogs.js');
  const backend = read('app/src-tauri/src/commands.rs');
  assert.doesNotMatch(app, /__TAURI__\.updater|\.updater\.check/);
  assert.doesNotMatch(app + dialogs, /https?:\/\//, 'the webview owns no updater endpoint');
  assert.match(app, /inv\('check_for_update', \{ channel: settings\.updateChannel \}\)/);
  assert.match(backend, /STABLE_UPDATE_ENDPOINT[\s\S]*releases\/latest\/download\/latest\.json/);
  assert.match(backend, /NIGHTLY_UPDATE_ENDPOINT[\s\S]*releases\/download\/nightly-feed\/latest\.json/);
  assert.match(backend, /NIGHTLY_UPDATE_PUBKEY[\s\S]*\.pubkey\(pubkey\)/,
    'Nightly overrides the Stable updater key with its own trust root');
  assert.match(read('app/src-tauri/updater/nightly.pub.b64'), /^[A-Za-z0-9+/]+=*\n$/);
  assert.match(backend, /\.endpoints\(vec!\[endpoint\]\)/, 'each check has exactly one endpoint');
  assert.doesNotMatch(dialogs, /install_update|allow_downgrade|allowDowngrade/);
  assert.match(backend, /download_and_install/, 'Tauri retains download, verification and install ownership');
  assert.match(app, /inv\('relaunch_after_update'\)/,
    'verified installs cross a backend-owned clean relaunch boundary');
  assert.doesNotMatch(app, /process\.relaunch|__TAURI__\.process/,
    'the updater must not inherit the replaced app process group');
  const relaunch = read('app/src-tauri/src/relaunch.rs');
  assert.match(relaunch, /libc::setsid\(\)[\s\S]*HELPER_FLAG|HELPER_FLAG[\s\S]*libc::setsid\(\)/,
    'the relaunch waiter is a setsid-detached child of the exiting app');
  assert.match(relaunch, /\/usr\/bin\/open[\s\S]*"-n"/);
  for (const file of readdirSync(resolve(root, 'app/src-tauri/src')).filter((f) => f.endsWith('.rs'))) {
    const source = read(`app/src-tauri/src/${file}`);
    assert.doesNotMatch(source, /launchctl|LaunchAgents|LaunchDaemons|SMAppService/,
      `${file}: deck never registers anything with launchd (corporate EDR flags it)`);
    assert.doesNotMatch(source, /Command::new\("(?:ps|date|osascript|sh|bash|zsh)"\)/,
      `${file}: process facts come from libproc/sysctl and no AppleScript or shell is spawned (EDR noise)`);
  }
  const hooks = read('app/src-tauri/src/agent_status.rs');
  assert.match(hooks, /const HELPER_MARKER: &str = "deck\.app\/Contents\/MacOS\/deck-status-helper"/,
    'hook commands run the helper inside the signed bundle');
  assert.doesNotMatch(hooks, /fn install_helper_binary|atomic_write\(&target/,
    'deck never drops an executable into the home directory');
  assert.match(hooks, /stable_installed_bundle\(bundle\)/,
    'only a release-location install may register hooks');
  assert.match(hooks, /"command": helper, "args": \[source, state\]/,
    'Claude Code entries use exec form so no sh -c runs per hook event');
  assert.doesNotMatch(read('app/src-tauri/capabilities/default.json'), /updater:/,
    'the webview has no direct updater command permission');
  assert.doesNotMatch(read('app/src-tauri/capabilities/default.json'), /process:/,
    'the webview cannot invoke the generic Tauri restart path');
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
  assert.match(lifecycle, /snapshot\.impact_token != expected_impact_token/,
    'restart executes only against the exact reviewed session/pane identity set');
  assert.match(app, /expectedImpactToken: status\.impactToken/);
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
  const html = read('app/ui/index.html');
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
  const html = read('app/ui/index.html');
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

test('card preview depth stays synchronized across capture and rendering', () => {
  const commands = read('app/src-tauri/src/commands.rs');
  const pure = read('app/ui/js/pure.js');
  const board = read('app/ui/js/board.js');
  assert.match(commands, /const CARD_PREVIEW_LINES: usize = 6;/);
  assert.match(commands, /capture_tails\(&want_tails, CARD_PREVIEW_LINES\)/);
  assert.match(pure, /export const CARD_PREVIEW_ROWS = 6;/);
  assert.match(board, /cardPreviewRows\(s\.tail\)/);
});

test('important-card marks are durable and visible on cards and in the sidebar', () => {
  const persistence = read('app/ui/js/persistence.js');
  const board = read('app/ui/js/board.js');
  const app = read('app/ui/js/app.js');
  const html = read('app/ui/index.html');
  assert.match(persistence, /pinned: c\.pinned === true/);
  assert.match(app, /pinned: c\.pinned === true/);
  assert.match(board, /async togglePinned\(sid\)/);
  assert.match(board, /className = 'side-pin'/);
  assert.match(board, /class="card-pin"/);
  assert.match(html, /\.card-pin\.active \{ color: var\(--wait\); \}/);
});

test('an inbound card keeps its origin across Board writes, and only identifiers', () => {
  const persistence = read('app/ui/js/persistence.js');
  const board = read('app/ui/js/board.js');
  assert.match(persistence, /'pinned', 'origin',\n\]\)/);
  assert.match(persistence, /\.\.\.\(cardOrigin\(c\) \? \{ origin: cardOrigin\(c\) \} : \{\}\)/);
  assert.match(persistence, /const \{ source, key, badge \} = o;/);
  assert.match(board, /\.\.\.\(origin \? \{ origin \} : \{\}\)/);
});

test('cards can change Boards only from the Board view', () => {
  const html = read('app/ui/index.html');
  const layout = read('app/ui/js/layout.js');
  const terminal = read('app/ui/js/terminal.js');
  const board = read('app/ui/js/board.js');
  assert.doesNotMatch(html + layout + terminal, /sess-col|session\.moveBoard/,
    'the card session header has no Board selector or move handler');
  assert.match(board, /provider\.move\(sid, c\.id\)/,
    'Board drag-and-drop remains the supported placement control');
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
  assert.match(commands, /tmux_with_stdin/);
  assert.match(commands, /"start-server"[\s\S]*"load-buffer"[\s\S]*"new-session"/,
    'the first restored card must keep an empty tmux server alive through buffer loading');
  assert.match(recovery, /RESTORE_TTY_FORMAT: &str = "#\{pane_tty\}"/);
  assert.match(commands, /"new-session"[\s\S]*"save-buffer"[\s\S]*RESTORE_TTY_FORMAT[\s\S]*"delete-buffer"/,
    'the tmux server writes the private buffer to the new pane tty and discards it');
  assert.doesNotMatch(commands + recovery, /shell_restore|RESTORE_SCRIPT|"\/bin\/sh"|"-sh"|login_shell/,
    'no shell script and no shell argv on the restore path (EDR inline-script signature)');
  assert.doesNotMatch(commands + main + recovery,
    /BOOTSTRAP_ARG|--deck-shell-bootstrap|maybe_run_bootstrap|bootstrap\.executable|bootstrap\.payload/,
    'the signed deck executable must never bootstrap a restored pane');
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
  assert.match(selection, /onModeChange\(true, lastStatus,/,
    'a frozen-selection scroll must publish live-cursor visibility');
  assert.match(selection, /terminalSelectionOverlayRows/);
  assert.match(selection, /if \(!selected \|\| !lastStatus\) return;/,
    'the stable overlay must paint while dragging as well as after pointerup');
  assert.match(selection, /lastStatus = status;[\s\S]{0,280}renderOverlay\(\);/,
    'each settled drag reply must publish its overlay geometry');
  assert.match(read('app/src-tauri/src/tmux.rs'),
    /mode-style 'none'[\s\S]*copy-mode-selection-style 'none'/,
    'tmux intermediate selection frames must stay visually empty');
  assert.match(backend,
    /copy-mode-selection-style", "none"[\s\S]*copy-mode-position-style/,
    'existing servers receive the non-flashing selection style too');
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
  assert.match(layout, /if \(copyRoute === 'native'\) \{\s*e\.preventDefault\(\)/,
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
    /selection-drag-overlay[\s\S]*?selection-scroll-stable[\s\S]*?selection-scroll-cursor[\s\S]*?selection-overlay[\s\S]*?selection-repeat[\s\S]*?selection-resize[\s\S]*?scroll-frame/,
    'real WKWebView smoke must verify drag/frozen selection paint, cursor visibility and DOM cleanup');
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

test('clipboard diagnostics cover every copy and paste handoff without content', () => {
  const layout = read('app/ui/js/layout.js');
  const terminal = read('app/ui/js/terminal.js');
  const backend = read('app/src-tauri/src/commands.rs');
  for (const stage of [
    'pasteTrace.keyCapture()', 'pasteTrace.keyHandler()', 'pasteTrace.event(',
    'pasteTrace.onData(', 'pasteTrace.write(',
  ]) assert.ok(layout.includes(stage), `missing paste diagnostic handoff: ${stage}`);
  for (const stage of ['key-capture', 'keydown-deck', 'keydown-native', 'keydown-none',
    'selection-vanished']) assert.ok(layout.includes(stage), `missing copy diagnostic: ${stage}`);
  for (const stage of ['pbcopy-success', 'pbcopy-failed', 'web-success', 'web-failed',
    'web-unavailable']) assert.ok(terminal.includes(stage), `missing clipboard writer diagnostic: ${stage}`);
  assert.match(backend, /"terminal-paste",[\s\S]*?"ondata-missing"[\s\S]*?"pty-failed"/);
  assert.match(backend, /"clipboard-write",[\s\S]*?"pbcopy-success"[\s\S]*?"web-unavailable"/);
});

test('every way a terminal selection dies is attributable in the log', () => {
  const selection = read('app/ui/js/selection.js');
  const layout = read('app/ui/js/layout.js');
  const backend = read('app/src-tauri/src/commands.rs');
  // ⌘C can only report what it FOUND. Without a lifecycle record, a
  // `terminal-copy keydown-none` is indistinguishable from a drag that never
  // promoted, a start tmux refused, and a live selection something revoked.
  for (const stage of ['promote', 'start-ok', 'start-failed', 'finish-ok', 'finish-failed',
    'update-failed', 'dimensions-changed', 'freeze-ok', 'freeze-failed'])
    assert.ok(selection.includes(`sev('${stage}'`), `selection stage never logged: ${stage}`);
  // The two paths that used to cancel behind nothing but a toast.
  assert.match(selection, /sev\('start-failed'\);\s*cancel\(false, null\);/,
    'a refused selection start must be logged, not only toasted');
  assert.match(selection, /sev\('update-failed'\);\s*await cancel\(false, null\);/,
    'a failed drag update must be logged, not only toasted');
  // No cancel may reach the log as an anonymous one: either it names the
  // revoke, or the caller already logged a more specific failure (null).
  assert.doesNotMatch(selection, /(?:^|[^.\w])cancel\(\s*(?:true|false)?\s*\)/,
    'every selection cancel must carry a reason label or an explicit null');
  assert.match(selection, /if \(hadSelection && reason\) sev\(`cancel-\$\{reason\}`/,
    'a cancel with nothing to destroy must stay silent so ordinary clicks do not flood the log');
  // The age reset belongs to the synchronous teardown. Left after the awaited
  // backend cancel it lands on whatever selection promoted meanwhile, which
  // reported a live drag's entire lifetime as "never promoted" (b=-1).
  assert.match(selection, /frozen = false;[\s\S]{0,320}?promotedAt = 0;[\s\S]{0,200}?const cancelGeneration/,
    'promotedAt must reset before cancel awaits anything');
  const reasons = new Set();
  for (const source of [selection, layout])
    for (const [, r] of source.matchAll(/cancel(?:TerminalSelection|AllTerminalSelections)?\(\s*(?:pane|previous|p|true|false)?\s*,?\s*'([a-z-]+)'\s*\)/g))
      reasons.add(r);
  for (const r of ['pointer', 'pointer-cancel', 'blur', 'hidden', 'input', 'escape',
    'focus', 'live', 'exit', 'leave', 'dispose'])
    assert.ok(reasons.has(r), `revoke reason never wired: ${r}`);
  // Every label the frontend can emit must exist in the backend's closed set,
  // or the diagnostic silently degrades to `<redacted>`.
  const vocabulary = /const SELECTION_EVENTS: &\[&str\] = &\[([\s\S]*?)\];/.exec(backend);
  assert.ok(vocabulary, 'backend must own a closed selection vocabulary');
  const allowed = new Set([...vocabulary[1].matchAll(/"([a-z-]+)"/g)].map(m => m[1]));
  for (const r of reasons)
    assert.ok(allowed.has(`cancel-${r}`), `backend would redact cancel-${r}`);
  assert.ok(allowed.has('cancel-other'), 'the default reason must be loggable too');
  // Revoker forensics: the pointerdown that destroys a live selection is
  // classified by provenance (trusted pointerType, or synthetic when
  // isTrusted is false) so a failed ⌘C can be attributed to a replayed
  // event, a trackpad lift-off tap, or a real re-click.
  for (const label of ['revoker-mouse', 'revoker-touch', 'revoker-pen',
    'revoker-unknown', 'revoker-synthetic']) {
    assert.ok(selection.includes(`'${label}'`), `revoker class never wired: ${label}`);
    assert.ok(allowed.has(label), `backend would redact ${label}`);
  }
  assert.match(selection, /'revoker-unknown'[\s\S]{0,400}?cancel\(false, 'pointer'\)/,
    'revoker attribution must run before the pointer cancel destroys the evidence');
  assert.ok(selection.includes("sev('native-cleared'"),
    'a late compatibility-mouse xterm selection must leave replay evidence');
  assert.ok(allowed.has('native-cleared'), 'backend would redact native-cleared');
  // The empty-handed ⌘C split: a live Deck selection in another pane (focus
  // never followed the drag) must be told apart from nothing anywhere.
  assert.ok(layout.includes("'keydown-elsewhere'"), '⌘C wrong-pane attribution never wired');
  assert.match(backend, /"keydown-elsewhere"/);
  assert.match(backend, /"terminal-selection",\s*DetailPolicy::Closed\(SELECTION_EVENTS\)/);
});
