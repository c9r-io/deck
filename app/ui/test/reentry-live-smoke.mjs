// The shell service restart with "Reopen Agent sessions", through the real
// dialog, with REAL Claude Code clients in the cards. Debug carrier only, on
// an isolated data root and tmux socket, at <home>/Applications/deck.app (the
// lifecycle hooks are installed only from a stable location) under a home the
// operator gives it (`open --env HOME=…`), never the home of a person.
//
// The page does what a person does: it starts the cards, types into the pane
// a card shows, sets the two Claude hook boxes of Settings and confirms the
// question each asks, opens the review from Settings → Terminal → "Restart
// service", ticks, picks the policy and confirms. No backend answer is
// scripted and no command is called in the dialog's place: each restart is
// the ONE request the dialog itself sends.
//
// `reentry-live`, one launch:
//   E  `claude`, started BEFORE observation is on: the review can only name
//      the setting (`relive-hint`); then both hook families are switched on;
//   A  `claude`: a turn, `/clear`, another turn, so the session to re-enter
//      is not the one it started in. One prompt is queued on A that its own
//      "not before" holds for a day;
//   B  `claude --dangerously-skip-permissions`;
//   C  the same, lowered to plan mode with Shift+Tab while it runs;
//   D  `cat`: a foreground program that is no Agent, so its row must say it
//      has to be reopened by hand;
//   T  `top`: a foreground program of another user (setuid root), which Deck
//      cannot read. Its row must offer nothing; WHICH sentence it carries is
//      recorded (`relive-other`), not judged;
//   then transaction 1, everything ticked and bypass reapplied (Policy B),
//   and the next review, which must start from nothing.
// `reentry-live-relaunch`, the same data root and tmux server after the app
// was quit and started again, the clients still running: the review starts
// from nothing, then transaction 2 with the policy left alone (Policy A).
//
// What it needs from the operator (a person, or a runner following app.log):
// - `<root>` = `/private/tmp/deck-relive-<12 hex>` with the carrier's home at
//   `<root>/home`, and the launch's scenario word
//   (`translation-fixture/scenario`) `relive-<12 hex>`; any other word is
//   refused.
// - Claude signed in under that home, `<root>/home/work/{e,a,b,c,d}` already
//   trusted by it and the bypass-mode warning already accepted, so that no
//   first-run question appears. A screen that asks something where a client
//   should wait for input is never typed into: the checkpoint those steps
//   belong to fails with a = -99 and the run ends there.
// - Neither hook family of Deck in that home's `.claude/settings.json`.
// - A window that stays visible: the timers and the terminals are the page's.
// - The process table at every `relive-hold` (a = 1 before the first restart,
//   2 after it, 3 after the relaunch's), each followed by a pause. Which
//   client runs with which arguments, and in which session, is the operator's
//   reading: the page sees no process and no session id.
//
// A checkpoint with several terms reports them as `a`, one bit per term in
// the order the code lists them, so a failed one says which did not hold.
//
// What a pass does NOT prove:
// - a debug carrier under a relocated home is not the release build in
//   /Applications, a person's home or their global hook file;
// - "confirmed" is the dialog's word for "this client entered that session".
//   It does not say that work continues or that the model is ready, and Deck
//   verified no permission in effect: the page reads Claude's mode line
//   before the restart only;
// - nothing about E, whose client started before the hooks existed.
//   `relive-early` only records what its row said: 1 not attempted (the
//   session it exited in unconfirmed), 2 confirmed, 3 anything else.

// ============================================================================
// WHAT CLAUDE'S OWN SCREEN SAYS. Everything this file believes about the
// client's screen is in this block and nowhere else. Read from a real Claude
// Code 2.1.296 on 2026-10-11 (its screens are the vendor's and may change:
// calibrate again before trusting a failed run on another release). BUSY is
// the one line no client was read for: the turns observed ended too quickly.
// A waiting client shows its mode line under the input box. A bare `claude`
// starts in "auto mode"; Shift+Tab from bypass goes auto, manual, accept
// edits, plan, bypass, and "manual mode on" is the one line without the words
// "shift+tab to cycle".
const READY = /(auto mode|bypass permissions|accept edits|plan mode|manual mode) on/;
const BUSY = /esc to interrupt/;              // a line of a screen that is still working
const REPLY_MARK = '⏺';                       // what an assistant line starts with
const PLAN_LINE = /plan mode on/;             // the mode line in plan mode
const BYPASS_LINE = /bypass permissions on/;  // the mode line under bypass
// Screens that ask something: nothing is ever typed into one. A new folder is
// asked about with "No, exit" selected, and so is the bypass warning.
const DIALOG = [/Yes, I trust this folder/, /Yes, I accept/, /Select login method/, /Press Enter to continue/,
  /Enter to confirm · Esc to cancel/, /Try the new fullscreen renderer/];
// ============================================================================

// What the page types into a client, and what the cards run.
const PROMPT = 'Reply with exactly this one word and nothing else: ';
const CLEAR = '/clear';
const NEXT_MODE = '\x1b[Z';                   // Shift+Tab: the next permission mode
const AGENT = 'claude';
const AGENT_BYPASS = 'claude --dangerously-skip-permissions';
const QUEUED = `${PROMPT}deckqueued`;
const HELD_SECS = 86400;                      // the queued prompt's "not before": a day from now

// A DIALOG screen where a client should wait for input. The reads and the one
// place that types throw it; the mode's catch fails the checkpoint and ends.
const ASKED = new Error('dialog');

async function harness() {
  const { $, ctx, inv, state, store } = await import('../js/state.js');
  const { panes, provider } = await import('../js/board.js');
  const { openSession } = await import('../js/layout.js');
  const { newSession } = await import('../js/terminal.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const { openSettings, selectSettingsSection } = await import('../js/settings.js');
  const { listScheduleArgs } = await import('../js/pure.js');
  const { listStartCalls } = await import('../js/scheduler-model.js');
  const { t } = await import('../js/i18n.js');
  let failed = false;
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  /* ASKED passes through every wait: a screen that asks something ends the run */
  const waitFor = async (test, timeout = 8000) => {
    const deadline = Date.now() + timeout;
    for (;;) {
      try { if (await test()) return true; } catch (error) { if (error === ASKED) throw error; /* else not yet */ }
      if (Date.now() >= deadline) return false;
      await pause(250);
    }
  };
  const report = async (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(a, 1), b });
    return ok;
  };
  /* written, never judged: `failed` is not its to touch */
  const metric = (name, a, b = 0) => inv('ui_event', { code: 'smoke-check', detail: name, a, b });
  /* the terms of one checkpoint as a sum, one bit each in the order given */
  const bits = terms => terms.reduce((sum, held, i) => sum + (held ? 1 << i : 0), 0);
  /* the operator's time at the process table: the mark first, then the pause */
  const hold = async (n, ms) => { await metric('relive-hold', n); await pause(ms); };
  /* the operator's directories: only a word of the expected shape names a root */
  const workDir = async () => {
    const word = await inv('smoke_native_scenario');
    return /^relive-[0-9a-f]{12}$/.test(word) ? `/private/tmp/deck-${word}/home/work` : '';
  };

  /* ---- a card's pane: read, and typed into, only while it is the one shown ---- */
  /* the screen a pane shows now: the bottom page of its terminal, a string per row */
  const screen = session => {
    const term = panes.get(session)?.term;
    const buffer = term?.buffer.active;
    return buffer ? Array.from({ length: term.rows }, (_, row) => buffer.getLine(buffer.baseY + row)?.translateToString(true) ?? '') : [];
  };
  const shows = (session, shape) => screen(session).some(line => shape.test(line));
  const asks = lines => DIALOG.some(shape => lines.some(line => shape.test(line)));
  /* The Board's own click on a card (board.js → layout.js `openSession`):
     that card's pane becomes the one shown. It never starts a shell. */
  const show = async card => {
    await openSession(card.id, { allowStart: false });
    return waitFor(() => state.view === 'session' && state.sessionId === card.id
      && panes.get(card.session)?.attached === true, 15000);
  };
  /* a card of its own in `dir`, started the way ＋ starts one, and shown */
  const start = async (projectId, dir, cmd) => {
    const card = await newSession(dir, { projectId, cmd, rethrow: true });
    if (!await show(card)) throw new Error('pane');
    return card;
  };
  /* the client waits for input: READY on its screen and BUSY nowhere on it */
  const ready = session => {
    const lines = screen(session);
    if (asks(lines)) throw ASKED;
    return lines.some(line => READY.test(line)) && !lines.some(line => BUSY.test(line));
  };
  /* on two reads in a row: one frame of a client that is still drawing proves nothing */
  const waitReady = (session, timeout = 60000) => {
    let before = false;
    return waitFor(() => { const held = before; before = ready(session); return held && before; }, timeout);
  };
  /* The one place this page types into a pane: only into the pane it reads,
     and never into a screen that asks something. */
  const type = async (session, text) => {
    if (panes.get(session)?.attached !== true) throw new Error('pane');
    if (asks(screen(session))) throw ASKED;
    await inv('pty_write', { name: session, gen: ctx.ptyGens.get(session), dataB64: strToB64(text) });
  };
  /* one line of input: the text, then Enter as a key of its own, the way the
     scheduler sends a prompt (a CR inside the burst is a pasted newline) */
  const say = async (session, text) => { await type(session, text); await pause(600); await type(session, '\r'); };
  /* one turn: asked for one word, the client answered it and waits again
     (the prompt's own echo carries the word too, but not after the mark) */
  const ask = async (session, token) => {
    const deadline = Date.now() + 90000;
    await say(session, `${PROMPT}${token}`);
    const answered = () => screen(session).some(line => line.trimStart().startsWith(REPLY_MARK) && line.includes(token));
    return await waitFor(() => answered() && ready(session), 90000)
      && await waitReady(session, Math.max(deadline - Date.now(), 1000)) && answered();
  };

  /* ---- Settings: the two Claude hook boxes ---- */
  const hooks = () => inv('agent_hooks_status');
  const confirming = () => $('cfm').style.display === 'flex';
  /* Settings → Agents, with both boxes showing what the file says */
  const openAgents = async () => {
    await openSettings(); selectSettingsSection('agents');
    return waitFor(async () => {
      const now = await hooks();
      return $('set-agent-hooks').checked === (now.claude === true)
        && $('set-claude-reentry').checked === (now.claude_reentry === true);
    });
  };
  /* Set a box on the way a click does and confirm the question it asks; any
     other question is declined. True once the backend says `applied`. */
  const enable = async (id, question, applied) => {
    const box = $(id);
    box.checked = true;
    box.dispatchEvent(new Event('change'));
    const expected = await waitFor(confirming, 3000) && $('cfm-msg').textContent === t(question);
    if (confirming()) $(expected ? 'cfm-yes' : 'cfm-no').click();
    return expected && waitFor(async () => !box.disabled && box.checked && applied(await hooks()), 15000);
  };

  /* ---- the restart review (restart-reentry.js paints it) ---- */
  const section = () => $('tmux-reentry');
  const all = (root, selector) => [...root.querySelectorAll(selector)];
  const text = (node, selector) => node?.querySelector(selector)?.textContent ?? '';
  const reviewing = () => $('tmux-lifecycle-modal').style.display === 'flex';
  const rows = () => all(section(), '.reentry-list > *');
  const boxes = () => all(section(), 'input[type="checkbox"]');
  const radios = () => all(section(), '.reentry-policy input[type="radio"]');
  /* a row is named by its card's Board title; titles of one project differ */
  const titleOf = card => provider.get(card.id)?.title ?? '';
  const rowOf = card => rows().find(row => text(row, '.reentry-choice-text') === titleOf(card));
  const selectable = row => !!row && row.matches('label.reentry-choice')
    && all(row, 'input[type="checkbox"]').some(box => !box.disabled);
  const isOff = row => !!row && row.matches('div.reentry-off') && all(row, 'input').length === 0;
  const ticked = () => rows().filter(row => row.querySelector('input[type="checkbox"]')?.checked);
  const bypassRows = list => list.filter(row => text(row, '.reentry-note') === t('reentry.profile.bypass'));
  /* "Select all candidates": the one box above the list */
  const selectAll = () => [...section().children].find(node => node.matches('label.reentry-choice')
    && text(node, '.reentry-choice-text') === t('reentry.selectAll'))?.querySelector('input');
  /* what every new preview starts from: the ordinary policy, bypass not chosen, nothing ticked */
  const untouched = () => [radios()[0]?.checked === true, radios()[1]?.checked === false,
    boxes().length > 0 && boxes().every(box => !box.checked)];
  /* Settings → Terminal → "Restart service": a restart of the user's own.
     True once the review shows what the backend previewed. */
  const openReview = async () => {
    await openSettings(); selectSettingsSection('terminal');
    if (!await waitFor(() => !$('set-tmux-restart').disabled, 10000)) return false;
    $('set-tmux-restart').click();
    return waitFor(() => reviewing() && !section().hidden, 20000);
  };
  /* leave the review by one of its own buttons, and Settings behind it */
  const leaveReview = async id => {
    $(id).click();
    await waitFor(() => !reviewing(), 5000);
    $('set-close').click();
  };

  /* ---- the result list ---- */
  const resultRows = () => all($('tmux-result'), '.reentry-result-row');
  const resultShown = () => $('tmux-done').style.display !== 'none';
  const settledRows = () => resultRows().length > 0 && !resultRows().some(row => row.classList.contains('pending'));
  /* The restart just confirmed has ended and no entry of its result list is
     still in progress. False at once when it ended without a result list. */
  const conclude = async () => await waitFor(() => (resultShown() && settledRows())
    || (!ctx.tmuxRestarting && !resultShown()), 90000) && resultShown() && settledRows();
  const head = row => text(row, '.reentry-result-head .reentry-choice-text');
  const details = row => (row ? all(row, '.reentry-hint').map(node => node.textContent) : []);
  /* the row that says `key` about `card`, in the dialog's own sentence */
  const says = (card, key) => resultRows()
    .find(row => head(row) === t('reentry.result.line', { card: titleOf(card), result: t(key) }));
  const confirmed = () => resultRows().filter(row => row.classList.contains('ok'));
  /* rows that speak of the bypass flag: the Policy B sentence or its detail line */
  const flagged = () => resultRows().filter(row => head(row).includes(t('reentry.result.confirmed'))
    || details(row).includes(t('reentry.warning.bypass-reauthorized')));
  const footer = () => all($('tmux-result'), '.reentry-notes > div').map(node => node.textContent);

  /* ---- the queue ---- */
  /* One list of one prompt on `card`, started the way the ⏱ panel starts a
     list that has a "not before" (scheduler.js `startList`): an `at` head,
     which the scheduler selects by that instant alone. */
  const queueLater = async card => {
    const now = Math.floor(Date.now() / 1000);
    const { session, id, dir, cmd } = provider.get(card.id);
    const base = { session, cardId: id, dir, cmd, reviewEach: false };
    const sched = { ...listScheduleArgs({ notBefore: now + HELD_SECS }, now).args, reviewEach: false };
    for (const [command, payload] of listStartCalls(base, sched, [QUEUED])) await inv(command, payload);
  };
  /* the queue as the backend holds it: that prompt as the one row of `card`, and what was delivered */
  const queued = async card => {
    const queue = await inv('queue_list');
    const items = queue.items || [];
    return {
      item: items.length === 1 && items[0].session === card.session && items[0].text === QUEUED ? items[0] : null,
      delivered: (queue.deliveries || []).length,
    };
  };

  return { $, inv, store, provider, t, pause, waitFor, report, metric, bits, hold, workDir, shows, start, waitReady, type, say,
    ask, hooks, openAgents, enable, section, all, text, rows, radios, rowOf, selectable, isOff, ticked, bypassRows,
    selectAll, untouched, openReview, leaveReview, conclude, resultRows, details, says, confirmed, flagged, footer,
    queueLater, queued, failed: () => failed };
}

export async function runReentryLiveSmoke() {
  const h = await harness();
  const { $, inv, store, provider, t, pause, waitFor, report, metric, bits, hold, workDir, shows, start, waitReady, type, say,
    ask, hooks, openAgents, enable, section, all, text, rows, radios, rowOf, selectable, isOff, ticked, bypassRows,
    selectAll, untouched, openReview, leaveReview, conclude, resultRows, details, says, confirmed, flagged, footer,
    queueLater, queued } = h;
  let stage = 0;
  try {
    // 1. a root of the expected shape, an empty Board, neither hook family of Deck
    stage = 1;
    await waitFor(() => provider.projects().length > 0);
    const project = provider.projects()[0].id;
    const work = await workDir();
    const first = await hooks();
    const clean = [!!work, store.cards.length === 0, first.claude === false, first.claude_reentry === false];
    if (!await report('relive-clean', clean.every(Boolean), bits(clean), store.cards.length)) throw new Error('root');

    // 2. E, started before observation is on: the review names the setting and offers nothing to tick
    stage = 2;
    const E = await start(project, `${work}/e`, AGENT);
    const entered = await waitReady(E.session) && await ask(E.session, 'deckoke1');
    const opened = await openReview();
    const hint = [opened, text(section(), '.reentry-setup').includes(t('reentry.setupHint')),
      text(section(), '.reentry-summary') === t('reentry.summary.unavailable', { count: 1 }),
      rows().length === 1 && isOff(rowOf(E)) && all(section(), 'input').length === 0];
    await report('relive-hint', hint.every(Boolean), bits(hint), rows().length);
    await leaveReview('tmux-later');

    // 3. observation and the status hooks, each through its box and its own question
    stage = 3;
    await openAgents();
    const life = await enable('set-claude-reentry', 'settings.claudeReentryEnableConfirm', now => now.claude_reentry === true);
    const signal = life && await enable('set-agent-hooks', 'settings.agentHooksEnableConfirm', now => now.claude === true);
    const both = await hooks();
    const observed = [life, signal, both.claude === true && both.claude_reentry === true,
      $('set-agent-hooks').checked && $('set-claude-reentry').checked];
    $('set-close').click();
    if (!await report('relive-observe', observed.every(Boolean), bits(observed))) throw new Error('hooks');

    // 4. the cards of the transaction: their clients start with observation on
    stage = 4;
    // A leaves the session it started in: the one to re-enter is the one after /clear
    const A = await start(project, `${work}/a`, AGENT);
    let cleared = await waitReady(A.session) && await ask(A.session, 'deckoka1');
    if (cleared) {
      await say(A.session, CLEAR);
      await pause(1500);   // /clear answers nothing: the client gets the time to leave its idle screen
      cleared = await waitReady(A.session) && await ask(A.session, 'deckoka2');
    }
    const B = await start(project, `${work}/b`, AGENT_BYPASS);
    const bypassed = await waitReady(B.session) && await ask(B.session, 'deckokb1') && shows(B.session, BYPASS_LINE);
    // C starts with bypass and is lowered to plan mode while it runs
    const C = await start(project, `${work}/c`, AGENT_BYPASS);
    let planned = await waitReady(C.session) && await ask(C.session, 'deckokc1') && shows(C.session, BYPASS_LINE);
    for (let press = 0; planned && press < 6 && !shows(C.session, PLAN_LINE); press++) {
      await type(C.session, NEXT_MODE);
      await pause(700);
    }
    planned = planned && shows(C.session, PLAN_LINE);
    const D = await start(project, `${work}/d`, 'cat');
    const T = await start(project, `${work}/t`, 'top');
    const reached = [entered, cleared, bypassed, planned];
    // a = the Claude cards in their final state; b = which, one bit each (E 1, A 2, B 4, C 8)
    if (!await report('relive-cards', reached.every(Boolean) && store.cards.length === 6,
      reached.filter(Boolean).length, bits(reached))) throw new Error('cards');

    // 5. one prompt queued on A, which its own "not before" holds for a day; b = its quiet time, none
    stage = 5;
    await queueLater(A);
    const waiting = await queued(A);
    const held = [!!waiting.item, waiting.item?.paused === false,
      waiting.item?.mode === 'at' && waiting.item.at - Date.now() / 1000 > 600, waiting.delivered === 0];
    await report('relive-queue', held.every(Boolean), bits(held), waiting.item?.quiet_secs || 0);

    // 6. the operator reads the process table: every client as it was started
    stage = 6;
    await hold(1, 10000);

    // 7. the review of transaction 1: four clients to tick, D and T said as they are, nothing chosen yet
    stage = 7;
    const reviewed = await openReview();
    const offered = [reviewed, rows().filter(selectable).length === 4 && [E, A, B, C].every(card => selectable(rowOf(card))),
      isOff(rowOf(D)) && text(rowOf(D), '.reentry-note') === t('reentry.reason.unsupported-agent') && isOff(rowOf(T)),
      ...untouched(), !section().querySelector('.reentry-setup')];
    await report('relive-preview', offered.every(Boolean), bits(offered), bypassRows(rows()).length);
    // which sentence the unreadable program's row carries: 1 by hand, 2 not verified, 3 checks
    // not completed, 4 program changed, 5 a shell, 9 another
    const said = text(rowOf(T), '.reentry-note');
    const known = ['reentry.reason.unsupported-agent', 'reentry.reason.identity-unavailable',
      'reentry.reason.check-incomplete', 'reentry.reason.executable-unavailable', 'reentry.kind.shell'];
    await metric('relive-other', known.findIndex(key => said === t(key)) + 1 || 9, rows().length);
    // everything ticked and bypass reapplied: Policy B is this restart's explicit choice
    selectAll()?.click();
    await pause(100);
    const reapply = radios()[1];
    const reapplies = !!reapply && !reapply.disabled
      && text(reapply.closest('label'), '.reentry-choice-text') === t('reentry.policy.bypass', { count: 2 });
    if (reapplies) reapply.click();
    await pause(100);
    const asked = reapplies && ticked().length === 4 && bypassRows(ticked()).length === 2
      && radios()[1].checked && !radios()[0].checked && $('tmux-restart').textContent === t('reentry.confirm', { count: 4 });
    // the click is made for exactly this request and for no other
    if (!await report('relive-request', asked, ticked().length, bypassRows(ticked()).length)) throw new Error('request');
    $('tmux-restart').click();

    // 8. its result: the service restarted, A entered without a flag, B and C with the one this restart authorized
    stage = 8;
    const ended = await conclude();
    const settled = ended && $('tmux-lifecycle-title').textContent === t('reentry.result.restarted')
      && !!says(A, 'reentry.result.confirmedOrdinary')
      && [B, C].every(card => details(says(card, 'reentry.result.confirmed')).includes(t('reentry.warning.bypass-reauthorized')))
      && footer().includes(t('reentry.result.queuesPaused'));
    // a = entries confirmed; b = rows that speak of the bypass flag
    await report('relive-b-result', settled, confirmed().length, flagged().length);
    // E's client started before the hooks existed; what its row says is recorded, never judged
    const skipped = details(says(E, 'reentry.result.notAttempted')).includes(t('reentry.reason.final-session-unconfirmed'));
    const reentered = !!(says(E, 'reentry.result.confirmedOrdinary') || says(E, 'reentry.result.confirmed'));
    await metric('relive-early', skipped ? 1 : reentered ? 2 : 3, resultRows().length);

    // 9. the prompt on A: still the one row, paused by the restart, undelivered
    stage = 9;
    const kept = await queued(A);
    const paused = [!!kept.item, kept.item?.paused === true, kept.delivered === 0];
    await report('relive-b-queue', paused.every(Boolean), bits(paused), kept.delivered);
    await leaveReview('tmux-done');
    await hold(2, 12000);

    // 10. the next review inherits nothing: the ordinary policy again, and no tick
    stage = 10;
    const fresh = [await openReview(), ...untouched()];
    await report('relive-next-preview', fresh.every(Boolean), bits(fresh), rows().filter(selectable).length);
    await leaveReview('tmux-later');
    await report('done', !h.failed(), 1, 0);
  } catch (error) {
    // Claude asked something where it should wait for input (E in step 2, A, B or C
    // in step 4): nothing was typed into it, and the checkpoint of those steps fails
    if (error === ASKED) await (stage < 4 ? report('relive-hint', false, 99) : report('relive-cards', false, 99));
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 21, b: stage });
    await report('done', false, 0, 21);
  }
}

// The same data root and tmux server after the app was quit and started
// again; the clients of the first launch are still running. Nothing of that
// launch's Policy B may be left: the review starts from the ordinary policy
// with nothing ticked, and a restart with the policy left alone sends no flag.
// The cards are found by the directories they were started in.
export async function runReentryLiveRelaunchSmoke() {
  const h = await harness();
  const { $, inv, store, t, pause, waitFor, report, bits, hold, workDir, rows, radios, rowOf, selectable, ticked, selectAll,
    untouched, openReview, leaveReview, conclude, says, confirmed, flagged, queued } = h;
  let stage = 0;
  try {
    // 1. the Board of the launch before, and a review that inherits nothing from it
    stage = 1;
    const work = await workDir();
    const at = letter => store.cards.find(card => card.dir === `${work}/${letter}`);
    const loaded = !!work && await waitFor(() => ['a', 'b', 'c'].every(letter => at(letter)), 20000);
    const [A, B, C] = ['a', 'b', 'c'].map(letter => at(letter));
    const opened = loaded && await openReview();
    const inherited = [loaded, opened, ...untouched(), opened && [A, B, C].every(card => selectable(rowOf(card)))];
    if (!await report('relive-relaunch-preview', inherited.every(Boolean), bits(inherited),
      rows().filter(selectable).length)) throw new Error('preview');

    // 2. everything ticked, the policy left alone: Policy A. The click is made for that request only
    stage = 2;
    selectAll()?.click();
    await pause(100);
    const asked = [A, B, C].every(card => ticked().includes(rowOf(card))) && radios()[0].checked && !radios()[1].checked
      && $('tmux-restart').textContent === t('reentry.confirm', { count: ticked().length });
    if (asked) $('tmux-restart').click();
    const ended = asked && await conclude();
    const settled = ended && $('tmux-lifecycle-title').textContent === t('reentry.result.restarted')
      && [A, B, C].every(card => !!says(card, 'reentry.result.confirmedOrdinary')) && flagged().length === 0;
    // a = entries confirmed; b = rows that speak of the bypass flag: none
    await report('relive-a-result', settled, confirmed().length, flagged().length);

    // 3. the prompt on A: still the one row, still paused, still undelivered
    stage = 3;
    const kept = await queued(A);
    const paused = [!!kept.item, kept.item?.paused === true, kept.delivered === 0];
    await report('relive-a-queue', paused.every(Boolean), bits(paused), kept.delivered);
    await leaveReview('tmux-done');
    await hold(3, 12000);
    await report('done', !h.failed(), 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 21, b: stage });
    await report('done', false, 0, 21);
  }
}
