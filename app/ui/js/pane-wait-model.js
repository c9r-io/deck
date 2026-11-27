// pane-wait-model.js — what a pane says while its terminal is not there yet,
// and when it may offer to try again. Pure: layout.js paints it.
//
// The waits are one list with the backend's (test/fixtures/limits.json
// `terminal_waits`; pty.rs ATTACH_BUDGET / FIRST_READ_WAIT, commands.rs
// START_BUDGET): the attach answers within its budget whatever process
// creation does (2026-11-27 freeze), so the pane can say what Deck observed.
// Text says only what Deck saw — "did not start within N s", "shows nothing
// yet" — never a guessed cause (no "the disk is busy").
//
// A retry is a NEW attach request: input typed meanwhile stays bound to the
// request it was typed under (terminal-input.js), and a request that timed
// out names no generation, so that input is cancelled, never moved to the
// retry. The "A detached pane stays detached" rule of layout.js still
// holds: a retry is offered only for a wait Deck itself gave up on, never
// for a pane whose stream ended (`exited`), a card whose session is stopped,
// a card being closed, or while the shell service restarts.
export const TERMINAL_WAITS = Object.freeze({
  attachBudgetMs: 5000,
  connectingNoticeMs: 1000,
  firstFrameWaitMs: 3000,
  startBudgetMs: 10000,
});

/* the backend's answer, by its wire message (error.rs: the message) */
export function attachFailure(error) {
  const message = String(error ?? '');
  if (message === 'terminal-timeout') return 'timeout';
  if (message === 'terminal-busy') return 'busy';
  if (message === 'attach-superseded') return 'superseded';
  return null;
}

/* a start's answer: `start-unknown` = the launch command may have been
   typed (the caller marks the card launched, never sends it again);
   `terminal-timeout` = no answer in time, no command was typed */
export function startFailure(error) {
  const message = String(error ?? '');
  if (message === 'start-unknown') return 'unknown';
  if (message === 'terminal-timeout') return 'timeout';
  if (message === 'terminal-busy') return 'busy';
  return null;
}

const NOTICES = Object.freeze({
  connecting: { key: 'pane.connecting', action: null },
  starting: { key: 'pane.starting', action: null },
  timeout: { key: 'pane.attachTimeout', action: 'retry' },
  busy: { key: 'pane.attachBusy', action: 'retry' },
  startTimeout: { key: 'pane.startTimeout', action: 'retry' },
  noFrame: { key: 'pane.noFrame', action: 'reconnect' },
});

/* the notice for one wait: an i18n key, its parameters and the one action
   it may offer (null: none) */
export function paneNotice(kind) {
  const notice = NOTICES[kind];
  if (!notice) return null;
  return {
    key: notice.key,
    params: { seconds: Math.round(TERMINAL_WAITS.attachBudgetMs / 1000) },
    action: notice.action,
  };
}

/* whether the pane may offer its retry/reconnect now. An attach retry
   (timeout, busy, noFrame) attaches only, so it needs a session that is not
   stopped; a start retry runs the same start the open did (attachPane
   re-checks the MCP-origin and recovery holds), so it is for a card that is
   still stopped. */
export function retryAllowed({ kind, card, current, restarting, closing, exited }) {
  if (!card || !current || restarting || closing || exited) return false;
  if (kind === 'startTimeout') return card.status === 'stopped';
  return card.status !== 'stopped';
}

/* the toast for a NEW card's start that did not answer (createStarted: no
   card is persisted, and a session the start makes later is cleaned up by
   the backend), or null for every other failure */
export function startFailureToastKey(error) {
  const failure = startFailure(error);
  if (failure === 'timeout' || failure === 'unknown') return 'session.startTimeout';
  if (failure === 'busy') return 'session.startBusy';
  return null;
}
