// Crash recovery of Claude sessions with REAL Claude Code clients, in the
// real window: the shell service is lost the way a power loss loses it, and
// the next launch offers "Recover interrupted Agent sessions". Debug carrier
// only, on an isolated data root and tmux socket, at
// <home>/Applications/deck.app under a home the operator gives it, never the
// home of a person. The harness (how a card is started, read and typed into,
// and what Claude's screen says) is reentry-live-smoke.mjs's.
//
// `recovery-live`, the launch before the loss:
//   the lifecycle hooks are switched on through their box (observation);
//   A  `claude`: a turn, `/clear`, another turn, so the session to recover is
//      not the one it started in;
//   B  `claude --dangerously-skip-permissions`: a turn under bypass;
//   D  `cat`: a foreground program that is no Agent, so nothing is recorded;
//   then `done`, and the page waits. The OPERATOR now reads the record file
//   and the process table, and kills the app and the tmux server with
//   SIGKILL (nothing is told to quit: that is the loss).
// `recovery-live-relaunch`, the same data root, socket name and home after
// the loss; the carrier starts a NEW server at boot:
//   the dialog opens by itself with A and B ticked and D not listed; one
//   confirmation; both rows say the session was entered with no bypass flag;
//   A's pane shows the turn after `/clear` and not the one before it; B's mode
//   line is not bypass; A answers one more turn; asked again, nothing is
//   offered. A `rclive-hold` (a = 1) gives the operator the process table.
//
// What it needs from the operator: `<root>` = `/private/tmp/deck-rclive-<12 hex>`
// with the carrier's home at `<root>/home`, the scenario word
// (`translation-fixture/scenario`) `rclive-<12 hex>`, Claude signed in under
// that home with `<root>/home/work/{a,b}` trusted and the bypass warning
// accepted, no hook of Deck's in that home's `.claude/settings.json`, and a
// window that stays visible. A screen that asks something is never typed
// into: the checkpoint fails with a = -99 and the run ends there.
//
// What a pass does NOT prove: a debug carrier under a relocated home is not
// the release build; a SIGKILL of the app and the server is not a powered-off
// Mac (the kernel, the disk cache and launchd survive it; the socket file
// stays behind); which session each new client resumed is the operator's
// reading of its arguments against the record file and Claude's transcripts.
import { harness, REPLY_MARK, BYPASS_LINE, CLEAR, AGENT, AGENT_BYPASS } from './reentry-live-smoke.mjs';

// A DIALOG screen where a client should wait for input (harness `ASKED`).
const isAsked = error => error instanceof Error && error.message === 'dialog';

async function recoveryHarness() {
  const h = await harness('rclive');
  const { panes } = await import('../js/board.js');
  const { ctx } = await import('../js/state.js');
  const { recoveryOpen } = await import('../js/agent-recovery.js');
  /* every line a pane's terminal holds, its scrollback included */
  const buffer = session => {
    const term = panes.get(session)?.term;
    const active = term?.buffer.active;
    return active ? Array.from({ length: active.length }, (_, row) => active.getLine(row)?.translateToString(true) ?? '') : [];
  };
  /* an assistant line that carries `token`: the prompt's own echo carries it too, but not after the mark */
  const answered = (session, token) => buffer(session).some(line => line.trimStart().startsWith(REPLY_MARK) && line.includes(token));
  const card = (letter, work) => h.store.cards.find(item => item.dir === `${work}/${letter}`);
  const rows = () => h.all(h.$('recovery-list'), '.recovery-rows > *');
  const rowOf = item => rows().find(row => h.text(row, '.reentry-choice-text') === (h.provider.get(item.id)?.title ?? ''));
  const ticked = row => !!row && row.matches('label.reentry-choice') && row.querySelector('input')?.checked === true
    && row.querySelector('input')?.disabled === false;
  const resultRows = () => h.all(h.$('recovery-result'), '.reentry-result-row');
  const says = (item, key) => resultRows().find(row => h.text(row, '.reentry-result-head .reentry-choice-text')
    === h.t('reentry.result.line', { card: h.provider.get(item.id)?.title ?? '', result: h.t(key, { agent: 'Claude' }) }));
  return { ...h, ctx, recoveryOpen, buffer, answered, card, rows, rowOf, ticked, resultRows, says };
}

export async function runRecoveryLiveSmoke() {
  const h = await recoveryHarness();
  const { inv, store, provider, pause, waitFor, report, bits, workDir, shows, start, waitReady, say, ask, hooks,
    openAgents, enable, $ } = h;
  let stage = 0;
  try {
    // 1. a root of the expected shape, an empty Board, neither hook family of Deck
    stage = 1;
    await waitFor(() => provider.projects().length > 0);
    const project = provider.projects()[0].id;
    const work = await workDir();
    const first = await hooks();
    const clean = [!!work, store.cards.length === 0, first.claude === false, first.claude_reentry === false];
    if (!await report('rclive-clean', clean.every(Boolean), bits(clean), store.cards.length)) throw new Error('root');

    // 2. observation: the lifecycle hooks, through their box and its own question
    stage = 2;
    await openAgents();
    const life = await enable('set-claude-reentry', 'settings.claudeReentryEnableConfirm', now => now.claude_reentry === true);
    const observed = [life, (await hooks()).claude_reentry === true, $('set-claude-reentry').checked];
    $('set-close').click();
    if (!await report('rclive-observe', observed.every(Boolean), bits(observed))) throw new Error('hooks');

    // 3. the cards: A leaves its first session with /clear, B runs under bypass, D is no Agent
    stage = 3;
    const A = await start(project, `${work}/a`, AGENT);
    let cleared = await waitReady(A.session) && await ask(A.session, 'deckra1');
    if (cleared) {
      await say(A.session, CLEAR);
      await pause(1500);   // /clear answers nothing: the client gets the time to leave its idle screen
      cleared = await waitReady(A.session) && await ask(A.session, 'deckra2');
    }
    const B = await start(project, `${work}/b`, AGENT_BYPASS);
    const bypassed = await waitReady(B.session) && await ask(B.session, 'deckrb1') && shows(B.session, BYPASS_LINE);
    await start(project, `${work}/d`, 'cat');
    const reached = [cleared, bypassed];
    // a = the Claude cards in their final state; b = which, one bit each (A 1, B 2)
    if (!await report('rclive-cards', reached.every(Boolean) && store.cards.length === 3,
      reached.filter(Boolean).length, bits(reached))) throw new Error('cards');

    // 4. the operator takes over: record file, process table, then the loss
    stage = 4;
    await report('done', !h.failed(), 1, 0);
    for (;;) await pause(60000);
  } catch (error) {
    if (isAsked(error)) await report(stage < 3 ? 'rclive-observe' : 'rclive-cards', false, 99);
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 22, b: stage });
    await report('done', false, 0, 22);
  }
}

export async function runRecoveryLiveRelaunchSmoke() {
  const h = await recoveryHarness();
  const { $, inv, t, pause, waitFor, report, metric, bits, workDir, shows, show, waitReady, ask, ctx, recoveryOpen,
    answered, card, rows, rowOf, ticked, resultRows, says } = h;
  let stage = 0;
  try {
    // 1. the dialog opened by itself: A and B ticked, D not offered, the button names two sessions
    stage = 1;
    const work = await workDir();
    const loaded = !!work && await waitFor(() => ['a', 'b', 'd'].every(letter => card(letter, work)), 20000);
    const [A, B, D] = ['a', 'b', 'd'].map(letter => card(letter, work));
    const shown = loaded && await waitFor(() => recoveryOpen() && rows().length > 0, 30000);
    const offered = [loaded, shown, $('recovery-title').textContent === t('recovery.title'),
      shown && ticked(rowOf(A)) && ticked(rowOf(B)), shown && rows().length === 2 && !rowOf(D),
      $('recovery-confirm').textContent === t('recovery.confirm', { count: 2 }) && !$('recovery-confirm').disabled,
      shown && ctx.recoveryHold?.has(A.session) === true && ctx.recoveryHold?.has(B.session) === true];
    if (!await report('rclive-offer', offered.every(Boolean), bits(offered), rows().length)) throw new Error('offer');

    // 2. one confirmation: both entered, neither with a bypass flag
    stage = 2;
    $('recovery-confirm').click();
    const ended = await waitFor(() => $('recovery-done').style.display !== 'none' && resultRows().length > 0
      && !resultRows().some(row => row.classList.contains('pending')), 120000);
    const done = [ended, $('recovery-title').textContent === t('recovery.result.title'),
      !!says(A, 'reentry.result.confirmedOrdinary'), !!says(B, 'reentry.result.confirmedOrdinary'),
      ctx.recoveryHold === null];
    // a = the bits; b = rows marked ok
    await report('rclive-result', done.every(Boolean), bits(done), resultRows().filter(row => row.classList.contains('ok')).length);
    $('recovery-done').click();

    // 3. what each pane shows: A the turn after /clear and not the one before it; B no longer under bypass
    stage = 3;
    // a resumed client draws its input box before it has drawn the conversation again
    const history = (item, token) => waitFor(() => answered(item.session, token), 20000);
    const seenA = await show(A) && await waitReady(A.session, 90000);
    const toldA = seenA && await history(A, 'deckra2');
    const seenB = await show(B) && await waitReady(B.session, 90000);
    const toldB = seenB && await history(B, 'deckrb1');
    const resumed = [seenA, toldA, seenA && !answered(A.session, 'deckra1'),
      seenB, toldB, seenB && !shows(B.session, BYPASS_LINE)];
    await report('rclive-resumed', resumed.every(Boolean), bits(resumed));

    // 4. the recovered client works: one more turn in A
    stage = 4;
    const turned = await show(A) && await ask(A.session, 'deckra3');
    await report('rclive-turn', turned);

    // 5. asked again, nothing is offered: the records were consumed and the service is alive
    stage = 5;
    const again = await inv('agent_recovery_offer');
    const none = [!again?.token, (again?.entries || []).length === 0, !recoveryOpen()];
    await report('rclive-again', none.every(Boolean), bits(none), (again?.entries || []).length);
    await metric('rclive-hold', 1);
    await pause(12000);
    await report('done', !h.failed(), 1, 0);
  } catch (error) {
    if (isAsked(error)) await report('rclive-resumed', false, 99);
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 22, b: stage });
    await report('done', false, 0, 22);
  }
}
