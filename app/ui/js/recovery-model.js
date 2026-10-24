// recovery-model.js — the DOM-free half of "Recover interrupted Agent
// sessions" (agent-recovery.js renders it, app.js asks for the offer once
// the Board loaded). It is not the restart's re-entry: that one reopens
// sessions the user's own restart ended; this one is offered after the
// shell service was LOST (a frozen Mac, a power cut), from the backend's
// record of each card's current Claude session (`agent_reentry/recovery.rs`).
// The backend owns every fact and every authority: this module plans what
// the user ticked for ONE offer and words what the backend answered.
//
// # Offer and plan
// `recoveryOffer(raw)` reads only the fields this contract names. An entry
// with a `blocked` reason is listed and cannot be ticked. `recoveryPlan`
// starts with every tickable entry ticked: the dialog exists only because
// these sessions were interrupted, and nothing launches until the user
// presses the confirm button. There is no policy choice: a recovery always
// asks for the plain resume, never the bypass flag. `recoveryRequest(plan)`
// is the `approval` argument of `agent_recovery_run`, null when nothing is
// ticked. `heldSessions(offer)` are the sessions whose shell must not be
// started while the offer is open: a shell opened meanwhile would make that
// card's recovery impossible.
//
// # Result
// Rows are the restart's own (`entryRows`); the title and summary are this
// operation's, and nothing says a shell service was restarted.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { displayable, entryRows, reentryResult } from './reentry-model.js';

const BLOCKED_KEYS = {
  'observation-off': 'recovery.blocked.observation-off',
  'shell-open': 'recovery.blocked.shell-open',
  'directory-missing': 'recovery.blocked.directory-missing',
  'program-missing': 'recovery.blocked.program-missing',
  unknown: 'recovery.blocked.unknown',
};
const text = (key, params) => (params ? { key, params } : { key });
const idOf = value => (typeof value === 'string' && value ? value : null);

/** What `agent_recovery_offer` answered, reduced; null when there is
 *  nothing to ask. */
export function recoveryOffer(raw) {
  if (!raw || typeof raw !== 'object' || !idOf(raw.token) || !Array.isArray(raw.entries)) return null;
  const entries = raw.entries.map(entry => {
    const cardId = idOf(entry?.cardId);
    if (!cardId) return null;
    const blocked = entry.blocked == null ? null
      : (typeof entry.blocked === 'string' && Object.hasOwn(BLOCKED_KEYS, entry.blocked) ? entry.blocked : 'unknown');
    const at = Number(entry.recordedAt);
    return {
      cardId,
      session: idOf(entry.session) || '',
      blocked,
      recordedAt: Number.isFinite(at) && at > 0 ? at : null,
    };
  }).filter(Boolean);
  return entries.length ? { token: raw.token, entries } : null;
}

export function recoveryPlan(offer) {
  const entries = offer?.entries || [];
  return {
    token: offer?.token || null,
    entries,
    selected: entries.filter(entry => !entry.blocked).map(entry => entry.cardId),
  };
}

export function withSelection(plan, cardId, on) {
  const tickable = plan.entries.some(entry => entry.cardId === cardId && !entry.blocked);
  if (!tickable) return plan;
  const rest = plan.selected.filter(id => id !== cardId);
  return { ...plan, selected: on ? [...rest, cardId] : rest };
}

export function withAllSelected(plan, on) {
  return { ...plan, selected: on ? plan.entries.filter(entry => !entry.blocked).map(entry => entry.cardId) : [] };
}

export function recoveryRequest(plan) {
  if (!plan?.token || !plan.selected.length) return null;
  return { token: plan.token, cards: [...plan.selected] };
}

export function heldSessions(offer) {
  return new Set((offer?.entries || []).filter(entry => !entry.blocked && entry.session).map(entry => entry.session));
}

const pad = n => String(n).padStart(2, '0');
/** "14:32", or "10-23 14:32" for another day, in local time. */
export function recordedLabel(seconds, now = Date.now()) {
  if (!seconds) return null;
  const at = new Date(seconds * 1000);
  const today = new Date(now);
  const time = `${pad(at.getHours())}:${pad(at.getMinutes())}`;
  return at.toDateString() === today.toDateString() ? time : `${pad(at.getMonth() + 1)}-${pad(at.getDate())} ${time}`;
}

/** The selection list as the dialog shows it. */
export function planView(plan, now = Date.now()) {
  const tickable = plan.entries.filter(entry => !entry.blocked);
  return {
    rows: plan.entries.map(entry => ({
      cardId: entry.cardId,
      session: entry.session,
      selectable: !entry.blocked,
      checked: plan.selected.includes(entry.cardId),
      note: entry.blocked
        ? text(BLOCKED_KEYS[entry.blocked])
        : text('recovery.recordedAt', { time: recordedLabel(entry.recordedAt, now) || '—' }),
    })),
    canSelect: tickable.length > 0,
    allSelected: tickable.length > 0 && tickable.every(entry => plan.selected.includes(entry.cardId)),
    confirm: text('recovery.confirm', { count: plan.selected.length }),
    confirmEnabled: plan.selected.length > 0,
  };
}

/** The result of one recovery: `raw` is what `agent_recovery_run` answered
 *  (a ReentryResult) or null when it is not known. */
export function projectRecovery(raw, requested = []) {
  const result = reentryResult(raw);
  const rows = entryRows(result);
  const answered = new Set(rows.map(row => row.cardId));
  for (const cardId of requested) {
    if (!idOf(cardId) || answered.has(cardId)) continue;
    rows.push({ cardId, session: '', shell: false, tone: 'warn', line: text('reentry.result.unknown'), details: [text('reentry.result.check')] });
  }
  const recovered = rows.filter(row => row.tone === 'ok').length;
  return {
    title: text('recovery.result.title'),
    summary: rows.length ? text('recovery.result.summary', { recovered, other: rows.length - recovered }) : null,
    rows,
    footer: [text('recovery.result.footer')],
  };
}

export { displayable };
