// Source-level lints for the frontend. Everything here is a tripwire on
// shape (unkeyed copy, a retired feature returning, a forbidden pattern),
// never a proxy for behaviour: behaviour lives in the DOM/pure tests, the
// Rust unit and contract tests, and the real-WKWebView smoke.
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { dirname, join, resolve } from 'node:path';
import { en } from '../js/i18n/en.js';
import { zhHans } from '../js/i18n/zh-Hans.js';
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
    if (/<(?:title|script|style|svg|path|circle)\b/.test(line)) continue;
    if (/class="wordmark"/.test(line)) continue;
    assert.match(line, /data-i18n(?:-title|-placeholder)?=/, `unkeyed visible HTML: ${line.trim()}`);
  }

  assert.doesNotMatch(production, /innerHTML\s*=\s*t\s*\(/);
  assert.doesNotMatch(production, /(?:toast|confirmDialog|promptDialog)\(\s*['"`][A-Za-z]/,
    'visible dynamic prose must use a stable translation key');
});

// The content policy is what keeps injected markup from running script
// (`script-src 'self'`, nothing inline) or reaching the network, and the
// security section is where a webview gains reach (an asset protocol, a
// relaxed pattern). Both are a reviewed edit here, not a config tweak.
test('the webview content policy and security options are the reviewed ones', () => {
  const { security } = JSON.parse(read('app/src-tauri/tauri.conf.json')).app;
  assert.deepEqual(Object.keys(security).sort(), ['csp', 'dangerousDisableAssetCspModification'],
    'a new security option widens what the page may reach');
  assert.equal(security.csp, "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; "
    + "font-src 'self' data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; "
    + "frame-src 'none'");
  assert.deepEqual(security.dangerousDisableAssetCspModification, ['style-src'],
    'only the style directive is left as written; scripts keep the policy the loader enforces');
});

/* The expression that starts at `from`, reduced to its shape: every string
   literal becomes `S`, a template literal `S` followed by its `${…}`
   expressions, and whitespace is dropped. It ends at the `;` or at the
   unbalanced closing bracket that ends the expression. An unterminated
   literal just ends the scan at the end of the file; such a module already
   fails the syntax gate. */
function shapeAt(source, from) {
  let shape = '';
  let depth = 0;
  for (let i = from; i < source.length; i++) {
    const c = source[i];
    if (c === "'" || c === '"') {
      for (i++; i < source.length && source[i] !== c; i++) if (source[i] === '\\') i++;
      shape += 'S';
    } else if (c === '`') {
      shape += 'S';
      for (i++; i < source.length && source[i] !== '`'; i++) {
        if (source[i] === '\\') { i++; continue; }
        if (source[i] !== '$' || source[i + 1] !== '{') continue;
        let braces = 1, expression = '';
        for (i += 2; i < source.length; i++) {
          if (source[i] === '{') braces++;
          else if (source[i] === '}' && --braces === 0) break;
          expression += source[i];
        }
        shape += `{${expression.replace(/\s+/g, '')}}`;
      }
    } else if ('([{'.includes(c)) { depth++; shape += c; }
    else if (')]}'.includes(c)) { if (depth-- === 0) break; shape += c; }
    else if (c === ';' && depth === 0) break;
    else if (!/\s/.test(c)) shape += c;
  }
  return shape;
}
const shapesAfter = (source, pattern) => [...source.matchAll(pattern)].map(m => shapeAt(source, m.index + m[0].length));
// literals joined by `+`, where `(condition ? 'a' : 'b')` still chooses between literals
const literalOnly = shape => /^S(\+S)*$/.test(shape.replace(/\((?:[^()]|\([^()]*\))*\?S:S\)/g, 'S'));

test('innerHTML takes literals; interpolated sites are a reviewed list and none carries translated text', () => {
  assert.equal(literalOnly(shapeAt("'<a>' + (open ? '<b></b>' : '') + '</a>';", 0)), true);
  assert.equal(literalOnly(shapeAt('`<i></i>`;', 0)), true);
  assert.equal(literalOnly(shapeAt('`<i>${name}</i>`;', 0)), false);
  assert.equal(literalOnly(shapeAt("'<b>' + t('key');", 0)), false);
  assert.equal(literalOnly(shapeAt('html;', 0)), false);
  assert.equal(literalOnly(shapeAt("'<b>unterminated + name", 0)), true, 'an unterminated literal ends the scan');
  assert.equal(shapeAt('`<b>${never closed', 0), 'S{neverclosed}');
  // What each interpolated site puts into markup, in source order. Text the
  // user or the outside world wrote goes through textContent, never here.
  const reviewed = {
    'board.js': [
      'S{s.status}',       // the sidebar dot: a closed status word as a class
      'S{cards.length}',   // a column's card count: a number
      'S{s.status}',       // the card dot: the same closed status word
    ],
    'layout.js': ['S{card.status}'],   // the pane head dot: the same closed status word
    'scheduler.js': ['html'],          // the template menu's local `add(cls, html)`: literals only, checked below
    // the link menu: one button per entry of a closed action list
    'terminal.js': ['S+linkMenuItems(kind).map(item=>S{item.action}).join(S)'],
  };
  let sites = 0;
  for (const name of readdirSync(resolve(root, 'app/ui/js')).filter(file => file.endsWith('.js')).sort()) {
    const shapes = shapesAfter(read(`app/ui/js/${name}`), /\.innerHTML\s*=(?!=)/g);
    sites += shapes.length;
    const interpolated = shapes.filter(shape => !literalOnly(shape));
    assert.deepEqual(interpolated, reviewed[name] ?? [],
      `${name}: innerHTML takes a literal and values go in through textContent; a closed value may be interpolated once its shape is reviewed here`);
    for (const shape of interpolated) {
      assert.doesNotMatch(shape, /(?<![\w$.])t\(/, `${name}: translated text never enters innerHTML`);
    }
  }
  assert.ok(sites >= 20, `the scan reads the assignments (${sites})`);
  const menuRows = shapesAfter(read('app/ui/js/scheduler.js'), /(?<![.\w$])add\(/g);
  assert.ok(menuRows.length >= 4, 'the template menu helper is still called `add`');
  for (const shape of menuRows) assert.match(shape, /^S(,S)?$/, 'the template menu builds its rows from literals');
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

// The main window is the app: nothing in it may load another page in the
// webview. The page has no anchor at all, and the one script that builds an
// anchor cancels the click and hands the address to the validated native
// opener. The tmux banner used to carry the only bare link, next to advice
// (install tmux with Homebrew) that could never help: deck runs only the
// tmux inside its own bundle (`tmux::tmux_bin`).
test('no link in the main window loads another page, and the tmux banner names a step that can help', () => {
  const html = read('app/ui/index.html');
  assert.doesNotMatch(html, /<a[\s>]/i, 'index.html has no anchor');
  assert.doesNotMatch(production, /<a\s+(?:href|class|id|target)\b/i, 'no anchor built from markup');
  const built = [...production.matchAll(/createElement\(\s*['"]a['"]\s*\)/g)];
  assert.equal(built.length, 1, 'a new script-built anchor needs the same handling and a review here');
  const site = production.slice(built[0].index, built[0].index + 300);
  assert.match(site, /a\.onclick = event => \{ event\.preventDefault\(\); openExternalLink\(href\); \};/);
  assert.match(site, /a\.onauxclick = event => event\.preventDefault\(\);/);
  assert.doesNotMatch(production, /window\.open\(|\blocation\.(?:href|assign|replace)\b|\blocation\s*=[^=]/);

  // one keyed sentence, shown only when `tmux_available` is false
  assert.match(html, /<div id="banner" data-i18n="app\.tmuxMissing">[^<]+<\/div>/);
  assert.match(read('app/ui/js/app.js'), /if \(!ok\) \$\('banner'\)\.style\.display = 'block';/);
  for (const [name, dictionary] of [['en', en], ['zh-Hans', zhHans]]) {
    assert.match(dictionary['app.tmuxMissing'] || '', /tmux/, `${name} names what cannot run`);
    assert.doesNotMatch(Object.values(dictionary).join('\n'), /brew/i, `${name} sends the user to a package manager`);
  }
  assert.match(en['app.tmuxMissing'], /sessions cannot start.*reinstall/i);
  assert.match(zhHans['app.tmuxMissing'], /session 无法启动.*重新下载并安装/);
});

test('the canonical dictionary has no unused keys outside documented dynamic families', () => {
  const source = read('app/ui/index.html') + production;
  const dynamic = /^(?:attention\.column|attention\.filter|attention\.waiting|automation\.run|automation\.wd|board\.default|buffer\.state|mcp\.tunnelState|session\.status|settings\.shortcut|settings\.notifyStatus|settings\.translationPackState|notice|tmux\.notice|voice\.phase|voice\.error|voice\.notice|translation\.error)\./;
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

test('the minimum supported window is 1280×800 points, enforced by the window config alone', () => {
  const [window, ...others] = JSON.parse(read('app/src-tauri/tauri.conf.json')).app.windows;
  assert.equal(others.length, 0, 'one window');
  assert.equal(window.minWidth, 1280);
  assert.equal(window.minHeight, 800);
  assert.ok(window.width >= window.minWidth && window.height >= window.minHeight,
    'the default window is inside the supported envelope');
  const smoke = JSON.parse(read('app/ui/test/fixtures/smoke-manifest.json'));
  assert.equal(smoke.modes['buffer-narrow'].checks['window-min-clamp'].a, window.minWidth,
    'the real-window smoke judges the same minimum');
});

test('long localized panels stay bounded and scrollable in the window', () => {
  const html = read('app/ui/style.css');
  assert.match(html, /#settings-modal, #tpl-modal \{[^}]*align-items: center;[^}]*padding: 20px;/);
  assert.match(html, /#settings-box, #tpl-box \{[^}]*width: 940px;[^}]*max-height: 100%;[^}]*overflow: hidden;/,
    'settings frame stays inside the viewport');
  assert.match(html, /#tpl-box \{ width: \d+px; height: \d+px; \}/,
    'the template manager shares that frame and only resizes it');
  assert.match(html, /#set-content, #tpl-content \{[^}]*min-width: 0;[^}]*overflow-y: auto;/,
    'only modal content scrolls; navigation and footer remain reachable');
  assert.match(html, /#cfm-box, #ppd-box \{[^}]*max-height: 84vh;[^}]*overflow-y: auto;/);
  assert.match(html, /#queue-body \{[^}]*min-height: 0;[^}]*overflow-y: auto;[^}]*overflow-x: hidden;/,
    'scheduled prompts scroll inside their drawer, never sideways');
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
    for (const [, r] of source.matchAll(/cancel(?:TerminalSelection|AllTerminalSelections)?\(\s*(?:pane|previous|p|true|false)?\s*,?\s*'([a-z-]+)'\s*(?:,\s*diagnostic)?\)/g))
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
  for (const label of ['input-ondata-unknown', 'input-compositionstart', 'input-unknown']) {
    assert.ok(selection.includes(`'${label}'`), `Input entry must be wired: ${label}`);
    assert.ok(rust.includes(`"${label}"`), `Input entry must be whitelisted: ${label}`);
  }
  assert.match(selection, /forensics\.reason\(\)/);
  assert.doesNotMatch(read('app/ui/js/selection-forensics.js'), /getSelection\(|terminal_selection_copy|clipboard|session/);
});

test('the first-send option is a separate, unticked box for Slack badge and clock rules, shown with its warning', () => {
  const html = read('app/ui/index.html');
  // every trigger-bound control of the editor carries a `q-p-<trigger>` class
  const triggers = classes => classes.split(' ').filter(name => name.startsWith('q-p-')).sort();
  const box = /<label class="([^"]*)"><input type="checkbox" id="auto-first-send"([^>]*)>/.exec(html);
  assert.ok(box, 'the box exists');
  // the override exists for a Slack badge run and a clock run (scheduler/first_send.rs), nothing else
  assert.deepEqual(triggers(box[1]), ['q-p-clock', 'q-p-slack'], 'shown for exactly the Slack badge and clock triggers');
  assert.equal(/checked/.test(box[2]), false, 'unticked by default');
  const hint = /<p\b[^>]*\bid="auto-first-send-hint"[^>]*>/.exec(html);
  assert.ok(hint, 'the warning exists');
  assert.deepEqual(triggers(/\bclass="([^"]*)"/.exec(hint[0])[1]), triggers(box[1]),
    'the warning is shown for every trigger that shows the box');
  assert.ok(html.indexOf('id="auto-first-send"') > html.indexOf('id="auto-send"'), 'after, and apart from, the approval box');
  const js = read('app/ui/js/automation.js');
  assert.ok(/firstSendNeedsConfirm\(false, box\.checked\)/.test(js) && /confirmDialog\(t\('automation\.firstSend\.confirm'\)\)/.test(js),
    'turning it on asks for confirmation');
});

test('every view change leaves the session view first, and the tool slot closes before panes go', () => {
  const dir = resolve(root, 'app/ui/js');
  const sites = [];
  for (const file of readdirSync(dir).filter(name => name.endsWith('.js'))) {
    const lines = read(`app/ui/js/${file}`).split('\n');
    lines.forEach((line, n) => {
      if (!/\bstate\.view\s*=[^=]/.test(line)) return;
      sites.push(file);
      /* the same function, a few lines up, detaches panes and closes the
         session tools; a bare view write would leave both behind */
      assert.match(lines.slice(Math.max(0, n - 6), n).join('\n'), /leaveSessionView\(/, `${file}:${n + 1}`);
    });
  }
  assert.deepEqual(sites.sort(), ['app.js', 'attention.js', 'board.js', 'layout.js', 'layout.js']);
  const layout = read('app/ui/js/layout.js');
  assert.match(layout, /export function leaveSessionView\([^)]*\) \{\n  closeSessionTools\('leave'\);/);
  for (const file of ['board.js', 'translation-lens.js', 'layout.js', 'app.js'])
    assert.doesNotMatch(read(`app/ui/js/${file}`), /queue-panel|ctx\.queueOpen\s*=/, `${file} reaches into the queue drawer`);
  const html = read('app/ui/index.html');
  const workspace = html.slice(html.indexOf('<div id="session-workspace">'), html.indexOf('<div id="ctx">'));
  for (const id of ['queue-panel', 'buffer-panel', 'translation-panel'])
    assert.match(workspace, new RegExp(`<section id="${id}" class="session-tool"`), `${id} is a right-hand session tool`);
});

test('every reason a channel event stays pending goes through the once-per-run notice', () => {
  const inbound = read('app/ui/js/inbound.js');
  const handle = inbound.slice(inbound.indexOf('async function handleChannel(item)'), inbound.indexOf('export async function drainChannel()'));
  assert.ok(handle.length > 1000 && handle.length < 6000, 'handleChannel is where a channel event is placed or left pending');
  assert.equal((handle.match(/\btoast\(/g) || []).length, 0, 'a direct toast here repeats at every drain');
  // one notice per unacknowledged way out, each with its own sentence
  assert.deepEqual((handle.match(/pendingNotice\(item, /g) || []).length, 6);
  for (const key of ['expirySaveFailed', 'eventConflict', 'bufferFull', 'noTemplate', 'blockedCommand', 'blockedTemplate', 'noTarget', 'orphan', 'createFailed']) {
    assert.ok(handle.includes(`'channel.${key}'`), key);
  }
});

test('reminders poll the backend only while the Board has one', () => {
  const app = read('app/ui/js/app.js');
  const start = app.slice(app.indexOf('function startReminders()'), app.indexOf('const offerBoardExit'));
  assert.ok(start.length > 0 && start.length < 1200, 'startReminders is where the reminder triggers are set up');
  // the one timer goes through the model's check (reminder-model.js `reminderTick`)
  assert.equal((start.match(/setInterval\(/g) || []).length, 1);
  assert.match(start, /setInterval\(reminderTick\(\(\) => store\.cards, reconcileReminders\), 2000\)/);
  // boot, focus and visibility stay unconditional: they pick up a response to
  // a notification whose reminder is no longer on the Board
  assert.match(start, /\{\s*reconcileReminders\(\);/);
  assert.match(start, /addEventListener\("focus", reconcileReminders\)/);
  assert.match(start, /addEventListener\("visibilitychange", reconcileReminders\)/);
  const code = app.replace(/\/\*[\s\S]*?\*\//g, '');
  assert.equal((code.match(/reconcileReminders/g) || []).length, 6, 'the import, these four and the response-only launch: nothing else in app.js drives reminders');
});

/* 22-COLD-SNOOZE: the system launches deck to deliver "remind in 1 hour"
   when it was not running. That launch transacts the answer and asks the
   backend whether the process ends; nothing of the ordinary boot (polling,
   the reminder tick, inbound triggers, the Connector drain, the update
   check) starts before that answer. */
test('a launch made for a notification answer transacts it before anything else starts', () => {
  const app = read('app/ui/js/app.js').replace(/\/\*[\s\S]*?\*\//g, '');
  const boot = app.slice(app.indexOf('export async function boot()'));
  const asked = boot.indexOf('inv("reminder_launch_visible")');
  const block = boot.indexOf('if (!visible) {');
  const finish = boot.indexOf("inv('reminder_response_finish')");
  assert.ok(asked > 0 && block > asked && finish > block, 'the question, then the response-only block with its finish');
  assert.match(boot, /const visible = await inv\("reminder_launch_visible"\)\.catch\(\(\) => true\);\n  if \(visible\) await revealThemedWindow\(\);/);
  // the answer is transacted only against a Board that loaded, and a failed
  // call keeps deck (true): the process never ends on an error
  assert.match(boot, /if \(!visible\) \{\n    if \(!loadErr\) await reconcileReminders\(\);\n    if \(!\(await inv\('reminder_response_finish'\)\.catch\(\(\) => true\)\)\) return;\n    await revealThemedWindow\(\);\n  \}/);
  for (const start of ['startPolling();', 'startReminders();', 'startInbound();', 'drainConnector();', 'setTimeout(checkForUpdate', 'refreshQueue();']) {
    assert.ok(boot.indexOf(start) > finish, `${start} waits for the answer`);
  }
});

/* 08-C4: a reaction approves a run for a message, not the bytes that are
   sent. Its event carries no text; Deck reads the message when it handles the
   reaction (at once on the live path, later through the search catch-up), so
   the hint may not say the reaction approved what is sent. */
test('the automatic-send hint says when the Slack text is read, in both languages and in the guide', () => {
  const said = {
    en: 'Your reaction approves the run and its first step for that message. The text is read when Deck handles the reaction, so an edit made after you reacted may be what gets sent.',
    zh: '你的表情回应批准的是针对那条消息的这次运行及其第一步。文本在 Deck 处理该回应时读取，所以你回应之后的编辑可能会被发送。',
  };
  const en = read('app/ui/js/i18n/en.js'), zh = read('app/ui/js/i18n/zh-Hans.js');
  assert.ok(en.includes(`'automation.autoSend.hint': '${said.en} With this on, `), 'English');
  assert.ok(zh.includes(`'automation.autoSend.hint': '${said.zh}开启后，`), 'Simplified Chinese');
  // the page's default text is the English entry, word for word
  const entry = en.match(/'automation\.autoSend\.hint': '([^']*)'/)[1];
  assert.ok(read('app/ui/index.html').includes(`data-i18n="automation.autoSend.hint">${entry}</p>`), 'index.html');
  assert.ok(read('docs/auto-respond.md').replace(/\s+/g, ' ').includes(said.en), 'docs/auto-respond.md');
  // nothing says the reaction approved the text that is sent
  for (const file of ['app/ui/js/i18n/en.js', 'app/ui/js/i18n/zh-Hans.js', 'app/ui/index.html', 'docs/auto-respond.md']) {
    assert.doesNotMatch(read(file), /already approves the run and its first step|已批准这次运行及其第一步/, file);
  }
});

const code = file => read(file).replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');

test("inbound triggers are pulled only once the webview holds the user's Board", () => {
  const app = code('app/ui/js/app.js');
  // two starts and no other pull: boot after a Board that loaded, and the lost
  // Board's way out, the one other moment the placeholder is replaced
  assert.equal((app.match(/\bstartInbound\b/g) || []).length, 3, 'the import and the two starts');
  assert.match(app, /\n  if \(!loadErr\) startInbound\(\);\n/);
  const exit = app.slice(app.indexOf('const offerBoardExit'), app.indexOf('export async function boot()'));
  assert.match(exit, /exited: \(\) => \{[^}]*\bstartReminders\(\);\s*startInbound\(\);[^}]*\}/);
  assert.doesNotMatch(app, /drainInbound|drainChannel|initInbound/);
  // the module wires nothing on its own: both listeners and the one timer
  // belong to startInbound, which also runs the first drains
  const inbound = code('app/ui/js/inbound.js');
  const start = inbound.slice(inbound.indexOf('export function startInbound()'));
  assert.ok(start.length > 100 && start.length < 600, 'startInbound closes the module');
  for (const call of [/\blisten\(/g, /\bsetInterval\(/g]) {
    assert.equal((inbound.match(call) || []).length, (start.match(call) || []).length, String(call));
  }
  assert.deepEqual([(start.match(/\blisten\(/g) || []).length, (start.match(/\bsetInterval\(/g) || []).length], [2, 1]);
  assert.match(start, /return Promise\.all\(\[drainInbound\(\), drainChannel\(\)\]\);/);
});

test("orphaned rules are dropped only once the webview holds the user's Board", () => {
  const app = code('app/ui/js/app.js');
  // two starts: boot after a Board that loaded, and the lost Board's way out
  assert.equal((app.match(/\bstartOrphanPruning\b/g) || []).length, 3, 'the import and the two starts');
  assert.match(app, /\n  if \(!loadErr\) startOrphanPruning\(\);\n/);
  const exit = app.slice(app.indexOf('const offerBoardExit'), app.indexOf('export async function boot()'));
  assert.match(exit, /exited: \(\) => \{[^}]*\bstartOrphanPruning\(\);[^}]*\}/);
  // the boot start comes before a first run creates its project, so that
  // project event is judged as it always was
  const boot = app.slice(app.indexOf('export async function boot()'));
  assert.ok(boot.indexOf('startOrphanPruning()') > boot.indexOf("inv('load_board')"));
  assert.ok(boot.indexOf('startOrphanPruning()') < boot.indexOf("provider.createProject('main')"));
  // in the drawer the pruning has one caller, and the subscription made at
  // init, before any Board is held, only renders
  const automation = code('app/ui/js/automation.js');
  assert.equal((automation.match(/\bpruneOrphans\(\)/g) || []).length, 2, 'its definition and the one call');
  const init = automation.slice(automation.indexOf('export function initAutomation(deps)'), automation.indexOf('export function startOrphanPruning()'));
  assert.ok(init.length > 1000, 'startOrphanPruning follows initAutomation');
  assert.doesNotMatch(init, /pruneOrphans/);
  const start = automation.slice(automation.indexOf('export function startOrphanPruning()'), automation.indexOf('export const stopAutomation'));
  assert.match(start, /provider\.subscribe\(ev => \{ if \(ev === 'projects'\) pruneOrphans\(\); \}\);/);
});

/* 09-C6: the sidebar's reminder label is a button inside the row. Its click
   edits the reminder; the row's own handler skips it, so the label never also
   opens (and, for a stopped card, starts) the session. The real click is in
   the reminder smoke (`reminder-ui-save`). */
test("a click on the sidebar's reminder label does not open the session", () => {
  const board = code('app/ui/js/board.js');
  const sidebar = board.slice(board.indexOf('export function renderSidebar()'), board.indexOf('export function updateSidebarSelection()'));
  assert.ok(sidebar.length > 500, 'renderSidebar is where the rows are built');
  // the row opens the session for every click except one on the label
  assert.match(sidebar, /el\.onclick = e => \{ if \(!e\.target\.closest\('\.card-reminder'\)\) openSession\(s\.id\); \};/);
  assert.equal((sidebar.match(/openSession\(/g) || []).length, 1);
  // the label is the one a card carries (`reminderChip`): it keeps its one
  // job and lets the click travel on, so the menus that close on any click
  // still close. It comes after the name, which the row must go on showing:
  // the row asks for the date alone on another day (the card adds the time).
  assert.match(sidebar, /if \(s\.reminder\) el\.append\(reminderChip\(s, reminderDay\)\);/);
  assert.doesNotMatch(sidebar, /stopPropagation|reminderLabel\(|card-reminder"/);
  const css = read('app/ui/style.css');
  // the full-date button that took the row's width, and its rule, are gone
  assert.doesNotMatch(css, /^\.card-reminder \{/m);
  // a collapsed sidebar shows neither the name nor the label
  assert.match(css, /body\.side-collapsed \.side-item \.name, body\.side-collapsed \.side-item \.card-reminder,/);
});

/* 09b: a Board card shows its reminder too (docs/card-reminders.md): a short
   label at the right end of the status row, which keeps the card's height.
   The real click and the height are in the reminder smoke. */
test('a Board card shows its reminder as a label at the end of the status row', () => {
  const board = code('app/ui/js/board.js');
  const card = board.slice(board.indexOf('export function cardEl(s)'), board.indexOf('export async function closeSession('));
  assert.ok(card.length > 1500, 'cardEl is where a card is built');
  // only a card with a reminder gets one, after the row was painted
  assert.match(card, /paintCardSignalStatus\(el\.querySelector\('\.card-status'\), s\);\s*if \(s\.reminder\) el\.querySelector\('\.card-status'\)\.append\(reminderChip\(s\)\);/);
  // the card's own click handler skips it, so the label edits the reminder only
  assert.match(card, /closest\('\.card-x, \.card-pin, \.card-signal-help, \.card-reminder'\)/);
  const label = board.slice(board.indexOf('function reminderChip(s, spell = reminderShort)'), board.indexOf('export function cardEl(s)'));
  assert.ok(label.length > 200 && label.length < 900, 'reminderChip sits right above cardEl');
  assert.match(label, /chip\.className = 'card-reminder' \+ \(due \? ' due' : ''\);/);
  assert.match(label, /chip\.textContent = '🔔 ' \+ \(due \? t\('reminder\.dueShort'\) : spell\(s\.reminder\)\);/);
  assert.match(label, /chip\.title = reminderLabel\(s\);/);
  assert.match(label, /chip\.onclick = \(\) => editReminder\(s\.id\);/);
  assert.doesNotMatch(label, /openSession|stopPropagation/);
  // one line at the row's right end, no taller than the row's own text
  const rule = read('app/ui/style.css').match(/\.card-status \.card-reminder, \.side-item \.card-reminder \{[^}]*\}/)?.[0] || '';
  for (const part of ['margin: 0 0 0 auto', 'flex: none', 'white-space: nowrap', 'line-height: 1.2']) assert.ok(rule.includes(part), part);
  for (const file of ['en.js', 'zh-Hans.js']) assert.match(read(`app/ui/js/i18n/${file}`), /"reminder\.dueShort": "(Due|到期)",/);
});

// check.mjs is the gate that keeps import cycles inside the view core. It is
// run here on small module directories: a cycle outside the core must be
// found whichever static edge closes it, wherever the module lives, and
// whatever the walk has already finished.
test('the import-cycle gate sees every static edge and every cycle', () => {
  const checker = resolve(root, 'app/ui/js/check.mjs');
  const env = { ...process.env };
  delete env.NODE_V8_COVERAGE;   // a gate run in a child process, not a covered module
  const check = modules => {
    const dir = mkdtempSync(join(tmpdir(), 'deck-check-'));
    try {
      for (const [name, source] of Object.entries(modules)) {
        mkdirSync(dirname(join(dir, name)), { recursive: true });
        writeFileSync(join(dir, name), source);
      }
      const run = spawnSync(process.execPath, [checker, dir], { encoding: 'utf8', env });
      return { status: run.status, output: run.stdout + run.stderr };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  };
  const core = {
    'board.js': "import { layout } from './layout.js';\nexport const board = layout;\n",
    'layout.js': "import { terminal } from './terminal.js';\nexport const layout = terminal;\n",
    'terminal.js': "import { board } from './board.js';\nexport const terminal = board;\n",
  };
  const b = "import { a } from './a.js';\nexport const b = a;\n";

  // the three core modules may import each other, and a leaf may be used
  const allowed = check({
    ...core,
    'leaf.js': 'export const leaf = 1;\n',
    'user.js': "import { leaf } from './leaf.js';\nexport const user = leaf;\n",
  });
  assert.equal(allowed.status, 0, allowed.output);
  assert.match(allowed.output, /^ok: 5 modules/);

  // one cycle between two leaves, closed by each kind of static edge
  const closedBy = {
    'import … from': { 'a.js': "import { b } from './b.js';\nexport const a = b;\n", 'b.js': b },
    'a multi-line import': { 'a.js': "import {\n  b,\n} from './b.js';\nexport const a = b;\n", 'b.js': b },
    'a side-effect import': { 'a.js': "import './b.js';\nexport const a = 1;\n", 'b.js': b },
    'export * from': { 'a.js': "export * from './b.js';\nexport const a = 1;\n", 'b.js': b },
    'export { … } from': { 'a.js': "export { b } from './b.js';\nexport const a = 1;\n", 'b.js': b },
    'a module in a subdirectory': {
      'a.js': "import { b } from './sub/b.js';\nexport const a = b;\n",
      'sub/b.js': "import { a } from '../a.js';\nexport const b = a;\n",
    },
  };
  for (const [edge, modules] of Object.entries(closedBy)) {
    const found = check(modules);
    assert.equal(found.status, 1, `${edge}: ${found.output}`);
    assert.match(found.output, /import cycle leaves the view core \(a\.js, (?:sub\/)?b\.js\): a\.js <-> (?:sub\/)?b\.js/, edge);
  }

  // a leaf in a cycle THROUGH the core, closed by a module the walk has
  // already finished: board ⇄ layout is seen first, then board → leaf → layout
  const through = check({
    'board.js': "import { layout } from './layout.js';\nimport { leaf } from './leaf.js';\nexport const board = [layout, leaf];\n",
    'layout.js': "import { board } from './board.js';\nexport const layout = board;\n",
    'leaf.js': "import { layout } from './layout.js';\nexport const leaf = layout;\n",
  });
  assert.equal(through.status, 1, through.output);
  assert.match(through.output, /import cycle leaves the view core \(leaf\.js\): board\.js <-> layout\.js <-> leaf\.js/);

  // a dynamic import() is a deliberate late edge, not a cycle
  const late = check({ 'a.js': "export const a = () => import('./b.js');\n", 'b.js': b });
  assert.equal(late.status, 0, late.output);
});

test('the project defaults dialog scrolls inside itself and its confirmation opens above it', () => {
  const css = readFileSync(new URL('../style.css', import.meta.url), 'utf8');
  const z = selector => Number(css.match(new RegExp(`${selector} \\{[^}]*z-index: (\\d+)`))?.[1]);
  assert.match(css, /#pdf-box \{ max-height: calc\(84vh - 20px\); overflow-y: auto; \}/);
  assert.ok(z('#cfm\\.cfm-over-dialog') > z('#chd, #pdf, #mcp-auth'), 'the confirmation is not hidden behind the dialog');
});
