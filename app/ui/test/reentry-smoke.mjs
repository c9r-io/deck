// Isolated real-WKWebView carrier for "Reopen Agent session" inside the shell
// service restart modal (app.js `showTmuxLifecycle` / `restartTmuxServer`,
// restart-reentry.js, reentry-model.js). The production modules run in the
// bundled webview and paint the real modal; what the BACKEND would answer is
// scripted in this page only, at the one place every invoke leaves it
// (`window.__TAURI__.core.invoke`):
//   - `tmux_server_status`      the real status with a session count, so the
//                               review asks for a preview;
//   - `agent_reentry_preview`   a preview without a candidate, then one with
//                               three candidates (bypass, ordinary, unknown),
//                               a shell and a card nobody can name;
//   - `restart_tmux_server`     NEVER reaches the backend: the request is
//                               recorded and a scripted answer is returned;
//   - `agent_reentry_operation` the scripted operation as it settles.
// No shell service is restarted, no session is created, killed or typed
// into and no agent is started: while the script is installed every command
// that could do one of those is refused here and counted (`reentry-ipc`), and
// the carrier stops before it opens the review if the script cannot be
// installed. When the carrier ends, the page keeps a guard that refuses
// `restart_tmux_server` for the rest of its life, so a restart that was
// still on its way when a check failed cannot reach the backend afterwards.
// Controls are activated with DOM `click()`, which runs WebKit's
// default action and the production `change` handlers; this is not a hand on
// a mouse, and the window does not have to be frontmost.
// The cards of the scripted preview are not on the Board (creating one needs
// a session), so every row is named by the model's last-resort label: a
// deck session name, or "Unknown card". "Open card" is therefore not offered
// here; restart-reentry-dom.test.mjs covers it.
// Every scripted reply also carries fields shaped like a native session id,
// a pid, a digest and paths; none of them may be painted (`reentry-no-native`).
//
// `reentry-layout` is one sum. What it is made of is written beside it as
// numbers, for each language (`reentry-layout-en`, `reentry-layout-zh`): the
// window, the modal's frame, its scrolling body and the confirming action,
// the section's horizontal overflow, and for the policy, every scope note
// and the details their box and the parts of the `inside` test. The items
// and their units are in reentry-layout.mjs. They are measurements: they are
// read in the tick of the assertion and again after the page settled, they
// are never compared with anything and never touch `failed`, and no text of
// the page is among them.
import { LAYOUT_SETTLED, LAYOUT_ITEMS, layoutItems, measureLayout, statusItems } from './reentry-layout.mjs';

export async function runReentrySmoke() {
  const { $, ctx, inv } = await import('../js/state.js');
  const { getLocale, setLocale, t } = await import('../js/i18n.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 8000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) throw new Error('bounded re-entry condition timeout');
      await pause(50);
    }
  };
  const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

  const nativeTauri = window.__TAURI__;
  const nativeInvoke = nativeTauri.core.invoke;
  let failed = false;
  /* reports go straight to the backend, whatever the script does to other commands */
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return nativeInvoke('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -a, b });
  };
  /* a measurement takes the same way out and decides nothing: `failed` is not its to touch */
  const metric = (name, a = 0, b = 0) => nativeInvoke('ui_event', { code: 'smoke-check', detail: name, a: Math.trunc(a), b: Math.trunc(b) });
  const layoutLine = {
    en: (item, value) => metric('reentry-layout-en', item, value),
    zh: (item, value) => metric('reentry-layout-zh', item, value),
  };

  /* planted values no surface may show */
  const UUID = '3f2b8c1e-9a4d-4c7b-8e21-0d5f6a7b8c9d';
  const DIGEST = '9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08';
  const planted = {
    nativeSessionId: UUID, sessionId: UUID, pid: 48211, foregroundPid: 48211, birth: 1760000048211,
    binaryHash: DIGEST, executable: '/Users/someone/.local/share/claude/versions/2.1.294',
    transcriptPath: `/Users/someone/.claude/projects/x/${UUID}.jsonl`, hostLease: DIGEST, cwd: '/Users/someone/work',
  };
  const NATIVE = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{32,}|48211|\/Users\/|\.jsonl|tok-reentry/i;

  const TOKEN = 'tok-reentry-2';
  const candidate = (cardId, session, launchProfile) => ({ cardId, session, kind: 'candidate', reason: null, launchProfile, ...planted });
  const previews = [
    { token: 'tok-reentry-1', lifecycleObservation: true, ...planted, entries: [
      { cardId: 'RS1', session: 'deck-shell-s1', kind: 'shell', reason: null, launchProfile: null, ...planted },
      { cardId: 'RU1', session: 'deck-other-u1', kind: 'unsupported', reason: 'unsupported-agent', launchProfile: null, ...planted },
    ] },
    { token: TOKEN, lifecycleObservation: true, ...planted, entries: [
      candidate('RA1', 'deck-atlas-a1', 'bypass'),
      candidate('RB1', 'deck-borealis-b1', 'ordinary'),
      // a card nobody can name: its session carries a native shape
      candidate('RC1', `deck-${UUID}`, 'unknown'),
      { cardId: 'RS1', session: 'deck-shell-s1', kind: 'shell', reason: null, launchProfile: null, ...planted },
    ] },
  ];
  const resultEntry = (cardId, session, outcome, extra = {}) => ({
    cardId, session, outcome, reason: null, warnings: [], shell: true, clientObserved: false, settled: true, ...planted, ...extra,
  });
  const operation = (state, last) => ({
    requestId: UUID, state, serverReplaced: true, ...planted, entries: [
      resultEntry('RA1', 'deck-atlas-a1', 'confirmed', { clientObserved: true, warnings: ['bypass-reauthorized'] }),
      resultEntry('RB1', 'deck-borealis-b1', 'unconfirmed', { reason: 'context-changed', clientObserved: true }),
      last,
    ],
  });
  const RUNNING = operation('running', resultEntry('RC1', `deck-${UUID}`, 'unconfirmed', { reason: 'launch-unconfirmed', settled: false }));
  const COMPLETE = operation('complete', resultEntry('RC1', `deck-${UUID}`, 'unconfirmed', { reason: 'context-unverified', clientObserved: true }));

  /* the script: what each command answers while it is installed */
  const REFUSED = new Set(['start_session', 'kill_session', 'pty_write', 'attach_session', 'detach_session', 'mcp_start_session',
    'voice_deliver', 'defer_tmux_restart']);
  let status = null;                 // the scripted tmux_server_status
  let previewAt = 0;                 // which preview the next read gets
  let previewReads = 0;
  let restarts = [];                 // every restart request the page sent
  let restartGate = null;            // the scripted restart answers when this lets go
  let operationReads = 0;
  let operationGate = null;          // the operation completes when this lets go
  let refused = 0;                   // commands that would have touched a session
  const scripted = async (command, args) => {
    if (command === 'tmux_server_status') return { ...status };
    if (command === 'agent_reentry_preview') { previewReads += 1; return structuredClone(previews[previewAt]); }
    if (command === 'acknowledge_tmux_lifecycle_notice') return null;
    if (command === 'restart_tmux_server') {
      restarts.push(structuredClone(args));
      if (restartGate) await restartGate.promise;
      return { replaced: true, server: { ...status, serverPid: status.serverPid + 1, serverStartedAt: status.serverStartedAt + 1 },
        reentry: structuredClone(RUNNING), ...planted };
    }
    if (command === 'agent_reentry_operation') {
      operationReads += 1;
      if (operationReads < 2) return { known: true, result: structuredClone(RUNNING) };
      if (operationGate) await operationGate.promise;
      return { known: true, result: structuredClone(COMPLETE) };
    }
    if (REFUSED.has(command)) { refused += 1; throw new Error('refused by the re-entry smoke'); }
    return nativeInvoke(command, args);
  };

  /* the three containers, the title and the buttons: everything this feature paints */
  const SURFACES = ['tmux-lifecycle-title', 'tmux-reentry', 'tmux-progress', 'tmux-result', 'tmux-restart'];
  const painted = [];
  const capture = () => { for (const id of SURFACES) painted.push($(id).textContent); };
  const section = () => $('tmux-reentry');
  const all = (root, selector) => [...root.querySelectorAll(selector)];
  const saidOnce = (root, line) => all(root, 'div, span, legend, summary').filter(node => node.children.length === 0 && node.textContent === line).length;
  const radios = () => all(section(), 'input[type="radio"]');
  const boxes = () => all(section(), 'input[type="checkbox"]');
  const modalOpen = () => $('tmux-lifecycle-modal').style.display === 'flex';
  const visible = node => !!node && node.getClientRects().length > 0 && getComputedStyle(node).visibility !== 'hidden';
  const openReview = async () => {
    const reads = previewReads;
    // Settings' own "Restart service" when it can be pressed, else the sidebar entry: both open the same review
    ($('set-tmux-restart').disabled ? $('tmux-restart-btn') : $('set-tmux-restart')).click();
    await until(modalOpen);
    await until(() => previewReads > reads && !section().hidden);
    await pause(100);
  };
  const closeReview = async () => { $('tmux-done').click(); await until(() => !modalOpen()); };

  /* ---- the layout items (reentry-layout.mjs): boxes, styles and flags of this page as numbers; never a text ---- */
  /* one sample as pairs; a sampling that throws is one pair that says so, never an exception of the carrier */
  const sampleLayout = (asserted, frames = -1) => {
    try { return layoutItems(measureLayout({ byId: $, part: section(), win: window }), asserted, frames); } catch (_) { return [[LAYOUT_ITEMS.taken, 0]]; }
  };
  /* two animation frames, or 250 ms when the page paints none; then 300 ms more: 1 the frames ran, 0 they did not */
  const settle = async () => {
    const frames = await Promise.race([
      new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve(1)))),
      pause(250).then(() => 0),
    ]);
    await pause(300);
    return frames;
  };
  const tellLayout = async samples => {
    for (const [language, first, pairs] of samples) {
      for (const [item, value] of pairs) {
        try { await layoutLine[language](first + item, value); } catch (_) { /* a measurement that cannot be written is not a failure */ }
      }
    }
  };

  const locale = getLocale();
  const realStatus = ctx.tmuxServerStatus;
  let stage = 0;
  let installed = false;
  try {
    stage = 1;
    const observed = await nativeInvoke('tmux_server_status');
    /* what the carrier's own server really reported, as flags and counts (a layout item, written with the first English sample) */
    const statusPairs = statusItems(observed);
    status = { ...observed, sessionCount: 4, paneCount: 4, attachedSessionCount: 0, foregroundSessionCount: 3, sessions: [],
      restartBlockers: [], pendingRestart: false, shouldPrompt: false, canRestart: true, notice: null,
      serverPid: Number(observed.serverPid) || 1, serverStartedAt: Number(observed.serverStartedAt) || 1, impactToken: 'reentry-smoke' };
    window.__TAURI__ = { ...nativeTauri, core: { ...nativeTauri.core, invoke: scripted } };
    installed = window.__TAURI__.core.invoke === scripted;
    /* the page's own invoke must go through the script before anything is pressed */
    const probe = installed ? await inv('tmux_server_status') : null;
    if (!installed || probe?.impactToken !== 'reentry-smoke') throw new Error('the script is not installed');
    setLocale('en');
    await pause(200);

    // 1. a preview without a candidate: the rows and their reasons, no policy, no note
    stage = 2;
    previewAt = 0;
    await openReview();
    capture();
    const zero = saidOnce(section(), t('reentry.summary.none')) === 1
      && all(section(), 'input').length === 0 && all(section(), 'fieldset, details, button').length === 0
      && saidOnce(section(), t('reentry.note.context')) === 0
      && saidOnce(section(), t('reentry.kind.shell')) === 1 && saidOnce(section(), t('reentry.reason.unsupported-agent')) === 1
      && $('tmux-restart').textContent === t('tmux.restart');
    await report('reentry-zero', zero, 1, all(section(), '.reentry-list > *').length);
    await closeReview();

    // 2. three candidates with their launch profile, a shell, and a card nobody can name
    stage = 3;
    previewAt = 1;
    await openReview();
    capture();
    const rows = all(section(), '.reentry-list > *');
    const notes = rows.map(row => row.querySelector('.reentry-note')?.textContent);
    const names = rows.map(row => row.querySelector('.reentry-choice-text')?.textContent);
    const listed = rows.length === 4
      && notes.join('|') === ['reentry.profile.bypass', 'reentry.profile.ordinary', 'reentry.profile.unknown', 'reentry.kind.shell'].map(key => t(key)).join('|')
      && names.join('|') === ['deck-atlas-a1', 'deck-borealis-b1', t('reentry.unknownCard'), 'deck-shell-s1'].join('|')
      && saidOnce(section(), t('reentry.summary', { count: 3, bypass: 1 })) === 1;
    await report('reentry-preview', listed, rows.length, boxes().length);

    // 3. nothing is selected, policy A is the default, policy B cannot be chosen yet, nothing risky has the focus
    stage = 4;
    const fresh = boxes().length === 4 && boxes().every(input => !input.checked && !input.disabled)
      && radios().length === 2 && radios()[0].checked && !radios()[1].checked && radios()[1].disabled
      && document.activeElement !== $('tmux-restart') && !section().contains(document.activeElement)
      && $('tmux-restart').textContent === t('tmux.restart');
    boxes()[0].click();                                // "select all"
    await pause(100);
    const selected = boxes().every(input => input.checked) && radios()[0].checked && !radios()[1].checked && !radios()[1].disabled
      && $('tmux-restart').textContent === t('reentry.confirm', { count: 3 });
    await report('reentry-policy-default', fresh && selected, fresh ? 1 : 2, selected ? 1 : 0);

    // 4. the startup configuration note under policy A: once, with its help behind one closed disclosure
    stage = 5;
    const noteOnce = () => saidOnce(section(), t('reentry.note.context')) === 1 && saidOnce(section(), t('reentry.help.context')) === 1
      && all(section(), 'details').length === 1 && !section().querySelector('details').open
      && saidOnce(section().querySelector('details'), t('reentry.help.context')) === 1
      && saidOnce(section().querySelector('details'), t('reentry.note.context')) === 0
      && visible(all(section(), '.reentry-scope-note').find(node => node.textContent === t('reentry.note.context')));
    let noted = noteOnce() ? 1 : 0;

    // 5. policy B: the count and the mandatory sentence, in both languages
    stage = 6;
    radios()[1].click();
    await pause(100);
    capture();
    const warned = () => radios()[1].checked && !radios()[0].checked
      && saidOnce(section(), t('reentry.policy.bypass', { count: 1 })) === 1
      && all(section(), '.reentry-warning').filter(node => node.textContent === t('reentry.policy.bypassWarning')).length === 1
      && visible(section().querySelector('.reentry-warning'))
      && saidOnce(section(), t('reentry.policy.unknownCount', { count: 1 })) === 1;
    const english = warned() && t('reentry.policy.bypassWarning') === 'If you reduced permissions during the session, this option will enable bypass again.';
    noted += noteOnce() ? 2 : 0;
    /* the note, the policy and the action that confirms can all be reached at this window's size */
    const reachable = () => {
      const body = $('tmux-lifecycle-body').getBoundingClientRect();
      const frame = $('tmux-lifecycle-box').getBoundingClientRect();
      const action = $('tmux-restart').getBoundingClientRect();
      const inside = rect => rect.width > 0 && rect.height > 0 && rect.left >= frame.left - 1 && rect.right <= frame.right + 1;
      return frame.top >= 0 && frame.bottom <= innerHeight + 1 && frame.left >= 0 && frame.right <= innerWidth + 1
        && inside(action) && action.bottom <= frame.bottom + 1 && action.top >= body.bottom - 1
        && section().scrollWidth <= section().clientWidth + 1
        && [section().querySelector('.reentry-policy'), ...all(section(), '.reentry-scope-note'), section().querySelector('details')]
          .every(node => inside(node.getBoundingClientRect()));
    };
    let laidOut = reachable() ? 1 : 0;
    /* the English items, in the tick of the assertion: nothing is awaited between the two reads */
    const layoutEn = [...sampleLayout(laidOut & 1), ...statusPairs];
    setLocale('zh-Hans');
    await pause(200);
    capture();
    const chinese = warned() && t('reentry.policy.bypassWarning') === '如果你在会话运行过程中降低过权限，这个选项会再次启用 bypass。'
      && $('tmux-restart').textContent === t('reentry.confirm', { count: 3 });
    noted += noteOnce() && t('reentry.note.context') === '重新进入会使用新 shell 的启动配置；若配置已变化，可能需要手动检查。' ? 4 : 0;
    laidOut += reachable() ? 2 : 0;
    /* the Chinese items in that tick too. From here on `laidOut` is final: the settled samples below wait and read
       again, and what they find is written as items only — it cannot reach the assertion. */
    const layoutZh = sampleLayout(laidOut >> 1);
    const settled = async () => {
      const frames = await settle();
      let again = -1;
      try { again = reachable() ? 1 : 0; } catch (_) { /* the sample says it could not be asked */ }
      return sampleLayout(again, frames);
    };
    const layoutZhSettled = await settled();
    setLocale('en');
    await pause(200);
    const layoutEnSettled = await settled();
    // a = 1 when both languages held; b = the number of sessions bypass would be reapplied to
    await report('reentry-policy-bypass', english && chinese, english ? 1 : 2, 1);
    // a = 8 + one bit per place the note stood exactly once (policy A, policy B, Chinese)
    await report('reentry-context-note', noted === 7, 8 + noted, saidOnce(section(), t('reentry.note.context')));
    // a = 4 + one bit per language whose modal kept everything reachable
    await report('reentry-layout', laidOut === 3, 4 + laidOut, innerWidth);
    // what that sum is made of, as items (reentry-layout.mjs): measurements, never judged
    await tellLayout([['en', 0, layoutEn], ['zh', 0, layoutZh], ['zh', LAYOUT_SETTLED, layoutZhSettled], ['en', LAYOUT_SETTLED, layoutEnSettled]]);

    // 6. the click sends ONE request with what was reviewed; nothing can be changed while it runs
    stage = 7;
    restartGate = deferred();
    operationGate = deferred();
    $('tmux-restart').click();
    await until(() => restarts.length > 0);
    await pause(100);
    const sent = restarts[0]?.reentry;
    const asked = restarts.length === 1 && !!sent && sent.token === TOKEN && sent.policy === 'reapply-bypass'
      && JSON.stringify(sent.cards) === JSON.stringify(['RA1', 'RB1', 'RC1']) && Object.keys(sent).sort().join(',') === 'cards,policy,token';
    const lockedNow = all(section(), 'input').length === 6 && all(section(), 'input').every(input => input.disabled)
      && $('tmux-restart').disabled && $('tmux-later').disabled;
    await report('reentry-request', asked && lockedNow, asked ? 1 : 2, sent?.cards?.length || 0);

    // 7. the result list: reopened, started with a different startup configuration, still in progress
    stage = 8;
    restartGate.resolve();
    const result = () => $('tmux-result');
    const resultRows = () => all(result(), '.reentry-result-row');
    await until(() => !result().hidden && resultRows().length === 3);
    capture();
    const tones = () => resultRows().map(row => row.className.replace('reentry-result-row', '').trim()).join(',');
    const head = index => resultRows()[index].querySelector('.reentry-result-head .reentry-choice-text').textContent;
    const hints = index => all(resultRows()[index], '.reentry-hint').map(node => node.textContent);
    const line = (card, key) => t('reentry.result.line', { card, result: t(key) });
    const three = tones() === 'ok,warn,pending'
      && $('tmux-lifecycle-title').textContent === t('reentry.result.restarted')
      && head(0) === line('deck-atlas-a1', 'reentry.result.confirmed')
      && hints(0).join('|') === t('reentry.warning.bypass-reauthorized')
      && head(1) === line('deck-borealis-b1', 'reentry.result.startedUnconfirmed')
      && hints(1).join('|') === t('reentry.reason.context-changed')
      && head(2) === line(t('reentry.unknownCard'), 'reentry.result.reopening') && hints(2).length === 0
      && new Set(resultRows().map(row => row.querySelector('.reentry-mark').textContent)).size === 3
      && section().hidden;
    await report('reentry-result', three, three ? 1 : 2, resultRows().length);

    // 8. nothing in the result repeats the restart: no retry control, only "Done" is left
    stage = 9;
    const controls = () => all(result(), 'button, input, a, select');
    const retryWords = /retry|try again|again|undo|重试|再试|撤销/i;
    const closedOnly = () => controls().length === 0
      && visible($('tmux-done')) && !visible($('tmux-restart')) && !visible($('tmux-later')) && !visible($('tmux-view-sessions'))
      && !retryWords.test(result().textContent) && !retryWords.test($('tmux-done').textContent);
    let quiet = closedOnly() ? 1 : 0;

    // 9. the operation settles: the row in progress becomes its result, read, never asked for again
    stage = 10;
    await until(() => operationReads >= 2);
    operationGate.resolve();
    await until(() => tones() === 'ok,warn,warn');
    await pause(1300);                                 // longer than one poll interval: nothing reads or asks again
    capture();
    const settledRows = tones() === 'ok,warn,warn'
      && head(2) === line(t('reentry.unknownCard'), 'reentry.result.startedUnconfirmed')
      && hints(2).join('|') === t('reentry.reason.context-unverified')
      && hints(1).join('|') === t('reentry.reason.context-changed')
      && all(result(), '.reentry-hint').every(node => node.textContent !== t('reentry.result.check'))
      && saidOnce(result(), t('reentry.note.context')) === 0;
    quiet += closedOnly() ? 2 : 0;
    setLocale('zh-Hans');
    await pause(200);
    capture();
    const settledChinese = hints(1).join('|') === 'Claude 已启动，但启动配置与预期不同；请打开卡片检查。'
      && hints(2).join('|') === 'Claude 已启动，但无法核实其启动配置；请打开卡片检查。';
    quiet += closedOnly() ? 4 : 0;
    setLocale('en');
    await pause(200);
    // a = 8 + one bit per look at the result that found no retry control (in progress, settled, Chinese)
    await report('reentry-no-retry', quiet === 7, 8 + quiet, controls().length);
    // b = how often the operation was read; the restart itself was asked for once
    await report('reentry-settled', settledRows && settledChinese && operationReads === 2 && restarts.length === 1,
      settledRows ? (settledChinese ? 1 : 3) : 2, operationReads);
    await closeReview();

    // 10. nothing shaped like a native id, a pid, a digest, a path or the preview token was painted
    stage = 11;
    const leaked = painted.filter(text => NATIVE.test(text)).length;
    await report('reentry-no-native', painted.length >= 30 && leaked === 0, painted.length, leaked);

    // 11. the backend was never asked to restart and no session command left the page
    stage = 12;
    await report('reentry-ipc', installed && restarts.length === 1 && refused === 0, 1, refused);

    await report('done', !failed, 1, 0);
  } catch (_) {
    await report('reentry-exception', false, stage || 1, restarts.length);
    await report('done', false, 1, stage);
  } finally {
    restartGate?.resolve();
    operationGate?.resolve();
    /* give a restart that is still unwinding its scripted answers before the real invoke returns */
    await pause(300);
    try { if (modalOpen()) $('tmux-done').click(); } catch (_) { /* the modal stays as it is */ }
    /* the script goes, the refusal of a real restart stays (this page only ever scripted it) */
    const guarded = (command, args) => (command === 'restart_tmux_server'
      ? Promise.reject(new Error('refused by the re-entry smoke')) : nativeInvoke(command, args));
    window.__TAURI__ = { ...nativeTauri, core: { ...nativeTauri.core, invoke: guarded } };
    ctx.tmuxServerStatus = realStatus;
    setLocale(locale);
  }
}
