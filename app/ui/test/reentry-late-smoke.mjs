// The late exit (2026-11-28) through the real restart dialog, with REAL
// Claude Code clients. The root, home, sign-in and directories of
// `reentry-live` (reentry-live-smoke.mjs: `relive-<12 hex>`, prep.py
// `init relive`); the same harness, the same rules for what is typed.
//
// `reentry-late`, one launch:
//   A  `claude` with a background shell still running: asked at Ctrl+D, it
//      answers with its own "Background work is running" question and does
//      not leave (measured on 2.1.332, air2, 2026-11-28);
//   B  `claude`, idle after one turn: the ordinary exit;
//   C  `claude` with a FOREGROUND tool still running: a turn in progress
//      (no background work: on 2.1.332 it leaves at Ctrl+D, measured in
//      this mode's first run, 2026-11-28 r1);
//   E  `claude` with a background shell AND a foreground tool running: the
//      production case of 2026-11-28 11:22 (a turn behind the question);
//   then ONE restart, the four ticked, the ordinary policy (A).
// Judged:
//   - the review offers the four and states the late-exit note once;
//   - A is confirmed re-entered WITH the late-exit line, B without it, C and
//     E confirmed (which route they took is recorded, `late-routes`: bit 1
//     C, bit 2 E), and the result is the list (never the toast);
//   - after the restart, C's and E's panes show their tool call interrupted
//     and wait for input — nothing continued the turn — and A's pane shows
//     that its background shell did not finish before the session ended.
// What the page cannot see, the runner reads: the process table at each
// `relive-hold` (1 before, 2 after) and app.log (`late-exit`, each entry's
// route).
//
// What a pass does NOT prove: other background work (a Monitor, a
// subagent, a teammate) — only a background shell was held here; a
// release build, a person's own home or their global hook file; the
// client's model being ready. "Confirmed" is the dialog's word for "this
// client entered that session".

import { harness, REPLY_MARK } from './reentry-live-smoke.mjs';

// What Claude's own screen says (2.1.332, air2, 2026-11-28).
const STILL = /still running|1 shell/;                       // the footer while a background shell runs
const BUSY = /esc to interrupt/;                              // a turn in progress
const INTERRUPTED = /Interrupted/;                            // a resumed tool call of an ended turn
const UNFINISHED = /didn.t finish before the previous session ended/;  // a resumed background shell

const BG = 'Use the Bash tool with run_in_background set to true to run exactly this command: '
  + 'python3 -c "import time; time.sleep(900)" deckbg . Do not wait for it. '
  + 'Then reply with exactly this one word and nothing else: deckbgok';
const FG = 'Use the Bash tool in the foreground (run_in_background false, timeout 300000) to run exactly this command: '
  + 'python3 -c "import time; time.sleep(240)" deckfg . Wait for it to finish, '
  + 'then reply with exactly this one word and nothing else: deckfgdone';

export async function runReentryLateSmoke() {
  const h = await harness();
  const { $, store, provider, t, pause, waitFor, report, metric, bits, hold, workDir, shows, show, start, waitReady,
    say, ask, hooks, openAgents, enable, section, rows, rowOf, selectable, ticked, radios, selectAll, openReview,
    leaveReview, conclude, resultRows, details, says } = h;
  const { ctx } = await import('../js/state.js');
  const answered = (session, token) => shows(session, new RegExp(`^\\s*${REPLY_MARK}.*${token}`));
  let stage = 0;
  try {
    // 1. a root of the expected shape, an empty Board, neither hook family of Deck
    stage = 1;
    await waitFor(() => provider.projects().length > 0);
    const project = provider.projects()[0].id;
    const work = await workDir();
    const first = await hooks();
    const clean = [!!work, store.cards.length === 0, first.claude === false, first.claude_reentry === false];
    if (!await report('late-clean', clean.every(Boolean), bits(clean), store.cards.length)) throw new Error('root');

    // 2. observation and the status hooks, each through its box and its own question
    stage = 2;
    await openAgents();
    const life = await enable('set-claude-reentry', 'settings.claudeReentryEnableConfirm', now => now.claude_reentry === true);
    const signal = life && await enable('set-agent-hooks', 'settings.agentHooksEnableConfirm', now => now.claude === true);
    $('set-close').click();
    if (!await report('late-observe', life && signal, bits([life, signal]))) throw new Error('hooks');

    // 3. the three cards, started with observation on, each brought to its state
    stage = 3;
    const B = await start(project, `${work}/b`, 'claude');
    const idle = await waitReady(B.session) && await ask(B.session, 'deckokb1');
    const A = await start(project, `${work}/a`, 'claude');
    let background = await waitReady(A.session);
    if (background) {
      await say(A.session, BG);
      background = await waitFor(() => answered(A.session, 'deckbgok'), 120000)
        && await waitReady(A.session) && shows(A.session, STILL);
    }
    const C = await start(project, `${work}/c`, 'claude');
    let running = await waitReady(C.session);
    if (running) {
      await say(C.session, FG);
      // the tool call is on its screen and the turn is still going
      running = await waitFor(() => shows(C.session, /deckfg/) && shows(C.session, BUSY), 120000);
      if (running) await pause(3000);
      running = running && shows(C.session, BUSY) && !answered(C.session, 'deckfgdone');
    }
    const Ecard = await start(project, `${work}/e`, 'claude');
    let both = await waitReady(Ecard.session);
    if (both) {
      await say(Ecard.session, BG);
      both = await waitFor(() => answered(Ecard.session, 'deckbgok'), 120000) && await waitReady(Ecard.session);
    }
    if (both) {
      await say(Ecard.session, FG);
      both = await waitFor(() => shows(Ecard.session, /deckfg/) && shows(Ecard.session, BUSY), 120000);
      if (both) await pause(3000);
      both = both && shows(Ecard.session, BUSY) && !answered(Ecard.session, 'deckfgdone');
    }
    const states = [idle, background, running, both];
    if (!await report('late-cards', states.every(Boolean) && store.cards.length === 4,
      states.filter(Boolean).length, bits(states))) throw new Error('cards');

    // 4. the operator reads the process table before the restart
    stage = 4;
    await hold(1, 8000);

    // 5. the review: three to tick, the late-exit note said once; all ticked, policy A
    stage = 5;
    const reviewed = await openReview();
    const notes = [...section().querySelectorAll('.reentry-notes > .reentry-scope-note')].map(node => node.textContent);
    const lateNote = notes.filter(line => line === t('reentry.note.lateExit')).length;
    const offered = [reviewed, rows().filter(selectable).length === 4, [A, B, C, Ecard].every(card => selectable(rowOf(card)))];
    await report('late-preview', offered.every(Boolean), bits(offered), lateNote);
    selectAll()?.click();
    await pause(100);
    const asked = ticked().length === 4 && radios()[0]?.checked === true && radios()[1]?.checked !== true
      && $('tmux-restart').textContent === t('reentry.confirm', { count: 4 });
    if (!await report('late-request', asked, ticked().length)) throw new Error('request');
    const toasts = [];
    const watch = new MutationObserver(records => records.forEach(record => record.addedNodes
      .forEach(node => toasts.push(node.textContent))));
    watch.observe($('toasts'), { childList: true });
    $('tmux-restart').click();

    // 6. the result list: A and C re-entered by the late exit, B by its own exit
    stage = 6;
    const ended = await conclude();
    watch.disconnect();
    const lateLine = t('reentry.result.lateExit', { agent: 'Claude' });
    const rowA = says(A, 'reentry.result.confirmedOrdinary');
    const rowB = says(B, 'reentry.result.confirmedOrdinary');
    const rowC = says(C, 'reentry.result.confirmedOrdinary');
    const rowE = says(Ecard, 'reentry.result.confirmedOrdinary');
    const terms = [ended, $('tmux-lifecycle-title').textContent === t('reentry.result.restarted'),
      !!rowA && details(rowA).includes(lateLine), !!rowB && !details(rowB).includes(lateLine),
      !!rowC && !!rowE, !toasts.some(text => text === t('reentry.result.quiet', { count: 4 }))];
    // which of C and E took the late exit: recorded, not judged
    await metric('late-routes', (rowC && details(rowC).includes(lateLine) ? 1 : 0)
      + (rowE && details(rowE).includes(lateLine) ? 2 : 0), resultRows().length);
    await metric('late-terms', bits(terms), resultRows().length);
    await report('late-result', terms.every(Boolean), bits(terms), resultRows().length);
    await leaveReview('tmux-done');
    await waitFor(() => !ctx.tmuxRestarting, 20000);

    // 7. what the re-entered clients show: C's turn interrupted and waiting
    //    for input, A's background shell unfinished. Each read while shown.
    stage = 7;
    const [nowA, nowC, nowE] = ['a', 'c', 'e'].map(letter => store.cards.find(card => card.dir === `${work}/${letter}`));
    // interrupted and waiting, and still waiting a while later: nothing continued the turn
    const waiting = async card => !!card && await show(card) && await waitFor(() => shows(card.session, INTERRUPTED), 60000)
      && await waitReady(card.session, 30000) && !shows(card.session, BUSY)
      && (await pause(5000), !shows(card.session, BUSY) && !answered(card.session, 'deckfgdone'));
    const seenC = await waiting(nowC);
    const seenE = await waiting(nowE);
    const seenA = !!nowA && await show(nowA) && await waitFor(() => shows(nowA.session, UNFINISHED), 60000)
      && await waitReady(nowA.session, 30000);
    const after = [seenC, seenE, seenA];
    await report('late-screens', after.every(Boolean), bits(after));
    await hold(2, 10000);
    await report('done', !h.failed(), 1, 0);
  } catch (error) {
    await metric('late-stage', stage, 0);
    await (stage < 4 ? report('late-cards', false, 99) : report('late-result', false, 99));
    await h.inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 21, b: stage });
    await report('done', false, 0, 21);
  }
}
