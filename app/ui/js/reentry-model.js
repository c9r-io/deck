// reentry-model.js — the DOM-free half of "Reopen Agent session" inside the
// shell service restart modal (restart-reentry.js renders it, app.js owns the
// restart transaction). The backend owns every fact and every authority: this
// module only plans what the user picked for ONE restart and words what the
// backend answered.
//
// # Plan (one preview, one confirmation)
// `reentryPlan(preview)` starts from nothing selected and the ordinary
// policy, every time. A plan is never read from or written to storage and
// nothing of an earlier plan enters a new one: a new preview is a new plan.
// Only a `candidate` entry of a preview whose lifecycle observation is
// installed can be selected. Policy B ("reapply-bypass") can be chosen only
// while at least one SELECTED entry has the launch profile `bypass`, and its
// count is selected ∩ bypass; `unknown` is counted on its own and is never
// taken for ordinary. The profile is the backend's classification of the live
// client: nothing here reads a card's command, history or terminal text.
// Choosing a policy selects no card, and a selection that loses its last
// bypass entry falls back to the ordinary policy.
// Whenever something can be selected, the view carries the standing scope
// notes once (`notes`) and what is said on request (`help`). One of the notes
// is the new shell's startup configuration: re-entry types into a shell whose
// startup files may have changed since the old client was started, and a
// difference is found only after the start. Nothing here offers to check,
// retry or undo that; with nothing selectable there is nothing to say.
// `reentryRequest(plan)` is the `reentry` argument of `restart_tmux_server`:
// null when nothing is selected (a shell-only restart, as before), otherwise
// the preview's token, the selected candidate ids and the policy.
// `afterManagedClose` is the one place a selection crosses to another
// preview: after the listed MCP cards were closed, the fresh preview must be
// exactly the reviewed list without those cards; anything else is a changed
// review, never a silent remap.
//
// # Result
// The title reports the shell service first (`serverReplaced`); an entry
// that could not be confirmed never turns a completed replacement into a
// failed restart. Outcomes, reasons and warnings are closed codes mapped to
// dictionary keys; an unknown code gets the generic sentence, never its raw
// text. `launch-profile-unconfirmed` and `bypass-not-reapplied` on one entry
// are one line. "Claude started" is said only for `unconfirmed` with a client
// the backend observed, or with one of the two startup-context reasons
// (`context-changed`, `context-unverified`), which the backend gives only for
// a client it found started. Those two reasons carry their own advice (open
// the card and check), so the "before trying again" line is not added to
// them. Nothing here says resumed, ready or restored, and no row offers a retry.
// Three things about a bypass request are never merged. A CONFIRMED entry the
// backend marks `bypass-reauthorized` says that the session was re-entered and
// that the bypass flag was sent as authorized — the backend gives that mark
// only when the started client's own arguments matched the request — and its
// one detail line says that Deck did not verify the permission in effect.
// Nothing says bypass was restored or is on. An entry that is not confirmed
// says only that re-entry was attempted: it never says a flag was delivered.
// `check-incomplete` is a check that did not finish; it is worded as that and
// never as a program that changed. No row names a release, a digest or a
// certification: the page has no such field to show.
// Every entry carries `settled`: its outcome is final. An entry is settled
// when the backend says so or when the whole operation is `complete`; a
// missing flag in a `running` operation is NOT settled. An unsettled entry is
// in progress and is worded as that ("Reopening…"), never as a result and
// never with the advice that belongs to a final `unconfirmed`. When the
// bounded poll ended and an entry is still unsettled (`pollEnded`), the row
// says the result is still pending and can be looked at later: that is not
// "could not be confirmed" either.
//
// # Shell-only progress
// A restart that asked for no re-entry shows its progress exactly as it did
// before re-entry existed: `shellProgressLabel` is the per-phase text of the
// restart button. The three stages (`progressView`) belong to a restart that
// requested re-entry and to no other.
//
// # Lost response
// `awaitReentryResult` only reads (`agent_reentry_operation`), on a bounded
// one-second poll while the backend worker is still running. It never invokes
// the restart again: a timeout is not a failure and a missing answer is not
// permission to repeat. Each read is handed to `onRead`, so the list that is
// already showing follows the entries as they settle.
//
// # Display guard
// Only the whitelisted fields of a backend object are read, so a field this
// contract does not name cannot reach the page. A backend string used as a
// label of last resort (a session name) passes `displayable`, which refuses
// anything that is not one of deck's own session names, and any such name
// carrying the shape of a native session id, a digest or a path.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const POLICY_ORDINARY = 'ordinary';
export const POLICY_BYPASS = 'reapply-bypass';

const KINDS = ['candidate', 'shell', 'unsupported'];
// which agent a row is, when the backend knows (its adapter's source word):
// a label, never a permission
const SOURCES = ['claude-code', 'codex', 'pi'];
// the display name each source word gets in a sentence; an unknown source
// is "the Agent"
const AGENT_NAMES = { 'claude-code': 'Claude', codex: 'Codex', pi: 'Pi' };
const agentOf = source => (own(AGENT_NAMES, source) ? AGENT_NAMES[source] : 'Agent');
const PROFILES = ['bypass', 'ordinary', 'unknown'];
const OUTCOMES = ['not-attempted', 'refused-before-launch', 'confirmed', 'started', 'unconfirmed', 'known-no-launch-failure'];

const PROFILE_KEYS = {
  bypass: 'reentry.profile.bypass',
  ordinary: 'reentry.profile.ordinary',
  unknown: 'reentry.profile.unknown',
};
const REASON_KEYS = {
  'unsupported-agent': 'reentry.reason.unsupported-agent',
  'observation-off': 'reentry.reason.observation-off',
  'client-identity-unverifiable': 'reentry.reason.client-identity-unverifiable',
  'multiple-panes': 'reentry.reason.multiple-panes',
  'identity-unavailable': 'reentry.reason.identity-unavailable',
  'executable-unavailable': 'reentry.reason.executable-unavailable',
  'final-session-unconfirmed': 'reentry.reason.final-session-unconfirmed',
  'check-incomplete': 'reentry.reason.check-incomplete',
  'exit-timeout': 'reentry.reason.exit-timeout',
  'impact-changed': 'reentry.reason.impact-changed',
  'launch-refused': 'reentry.reason.launch-refused',
  'launch-unconfirmed': 'reentry.reason.launch-unconfirmed',
  'context-changed': 'reentry.reason.context-changed',
  'context-unverified': 'reentry.reason.context-unverified',
  'native-session-not-found': 'reentry.reason.native-session-not-found',
  'user-action-required': 'reentry.reason.user-action-required',
  'bypass-not-reapplied': 'reentry.reason.bypass-not-reapplied',
};
// reasons of an `unconfirmed` entry whose new client was found started
const CONTEXT_REASONS = ['context-changed', 'context-unverified'];
const WARNING_KEYS = {
  'bypass-not-reapplied': 'reentry.reason.bypass-not-reapplied',
  'launch-profile-unconfirmed': 'reentry.warning.launch-profile-unconfirmed',
  'bypass-reauthorized': 'reentry.warning.bypass-reauthorized',
  'additional-dirs-not-restored': 'reentry.warning.additional-dirs-not-restored',
  'ordinary-profile-requested': 'reentry.warning.ordinary-profile-requested',
};
const STAGES = ['preparing', 'restarting', 'reopening'];
const STAGE_KEYS = {
  preparing: 'reentry.stage.preparing',
  restarting: 'reentry.stage.restarting',
  reopening: 'reentry.stage.reopening',
};
const PHASE_STAGE = { exiting: 'preparing', saving: 'preparing', replacing: 'restarting', reopening: 'reopening' };

const own = (map, code) => (typeof code === 'string' && Object.hasOwn(map, code) ? map[code] : null);
const text = (key, params) => (params ? { key, params } : { key });
const idOf = value => (typeof value === 'string' && value ? value : null);

const NATIVE_SHAPES = [
  /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/i,
  /[0-9a-f]{32,}/i,
  /[/\\]/,
];
// deck's own session names (pure.js `sessionName`): `deck-<slug>-<id tail>`
const SESSION_NAME = /^deck-[a-z0-9-]{1,40}$/i;
/** A backend string that may be shown as a label of last resort, or null:
 *  only one of deck's session names, and none that carries a native shape. */
export function displayable(value) {
  if (typeof value !== 'string' || !SESSION_NAME.test(value)) return null;
  return NATIVE_SHAPES.some(shape => shape.test(value)) ? null : value;
}

/* ---------- plan ---------- */

const EMPTY_PLAN = Object.freeze({
  token: '', observation: false, unknown: false, entries: Object.freeze([]), selected: Object.freeze([]), policy: POLICY_ORDINARY,
});
/* What Deck could not find out is said as such, never as "nothing to
   reopen": a preview that could not be read (`{ unknown: true }`, app.js)
   or one read without a current Board (`boardKnown: false`). */
const UNKNOWN_PLAN = Object.freeze({ ...EMPTY_PLAN, unknown: true });

function previewEntry(raw, repeated) {
  const cardId = idOf(raw?.cardId);
  if (!cardId) return null;
  const session = idOf(raw.session) || '';
  // one card listed twice names no single client: neither row is a candidate
  if (repeated.has(cardId)) return { cardId, session, kind: 'unsupported', reason: 'identity-unavailable', profile: null };
  const kind = KINDS.includes(raw.kind) ? raw.kind : 'unsupported';
  return {
    cardId, session, kind,
    reason: kind === 'candidate' ? null : (own(REASON_KEYS, raw.reason) ? raw.reason : null),
    source: SOURCES.includes(raw.source) ? raw.source : null,
    profile: kind !== 'candidate' ? null : PROFILES.includes(raw.launchProfile) ? raw.launchProfile : 'unknown',
    // a candidate whose adapter's lifecycle observation is on: the only
    // rows that can be ticked (the backend's own flag, per row)
    observed: kind === 'candidate' && raw.observed === true,
  };
}

/** A fresh plan for this preview: nothing selected, ordinary policy. A
 *  preview that is missing or unusable gives the empty plan (shell-only). */
export function reentryPlan(preview) {
  if (preview?.unknown === true) return UNKNOWN_PLAN;
  if (!preview || typeof preview !== 'object' || !idOf(preview.token) || !Array.isArray(preview.entries)) return EMPTY_PLAN;
  if (preview.boardKnown === false) return UNKNOWN_PLAN;
  const seen = new Set(), repeated = new Set();
  for (const raw of preview.entries) {
    const id = idOf(raw?.cardId);
    if (id && seen.has(id)) repeated.add(id);
    if (id) seen.add(id);
  }
  return {
    token: preview.token,
    observation: preview.lifecycleObservation === true,
    unknown: false,
    entries: preview.entries.map(raw => previewEntry(raw, repeated)).filter(Boolean),
    selected: [],
    policy: POLICY_ORDINARY,
  };
}

const selectable = (plan, entry) => plan.observation && entry.kind === 'candidate' && entry.observed;
const selectedEntries = plan => plan.entries.filter(entry => selectable(plan, entry) && plan.selected.includes(entry.cardId));

export function planCounts(plan) {
  const candidates = plan.entries.filter(entry => selectable(plan, entry));
  const chosen = selectedEntries(plan);
  return {
    candidates: candidates.length,
    candidateBypass: candidates.filter(entry => entry.profile === 'bypass').length,
    selected: chosen.length,
    bypass: chosen.filter(entry => entry.profile === 'bypass').length,
    unknown: chosen.filter(entry => entry.profile === 'unknown').length,
  };
}

// Every change ends here: the selection holds selectable ids only, in entry
// order, and policy B survives only while a selected bypass entry backs it.
function settled(plan, selected, policy) {
  const next = { ...plan, selected, policy };
  next.selected = selectedEntries(next).map(entry => entry.cardId);
  next.policy = policy === POLICY_BYPASS && planCounts(next).bypass > 0 ? POLICY_BYPASS : POLICY_ORDINARY;
  return next;
}

export function withSelection(plan, cardId, on) {
  const rest = plan.selected.filter(id => id !== cardId);
  return settled(plan, on ? [...rest, cardId] : rest, plan.policy);
}

export function withAllSelected(plan, on) {
  return settled(plan, on ? plan.entries.map(entry => entry.cardId) : [], plan.policy);
}

/** Choosing a policy selects nothing; B without a selected bypass entry stays A. */
export function withPolicy(plan, policy) {
  return settled(plan, plan.selected, policy);
}

/** The `reentry` argument of `restart_tmux_server`. */
export function reentryRequest(plan) {
  const cards = selectedEntries(plan).map(entry => entry.cardId);
  if (!plan.token || !cards.length) return null;
  return { token: plan.token, cards, policy: settled(plan, plan.selected, plan.policy).policy };
}

const signature = entry => [entry.cardId, entry.session, entry.kind, entry.reason, entry.profile, entry.observed].join('\0');

/** After the listed managed cards were closed: the request against the FRESH
 *  preview, or `{ changed: true }` when that preview is not the reviewed list
 *  without exactly those cards. */
export function afterManagedClose(plan, freshPreview, closedCardIds) {
  if (!reentryRequest(plan)) return { request: null };
  const closed = new Set(closedCardIds || []);
  const fresh = reentryPlan(freshPreview);
  const reviewed = plan.entries.filter(entry => !closed.has(entry.cardId)).map(signature).sort();
  const offered = fresh.entries.map(signature).sort();
  if (!fresh.token || fresh.observation !== plan.observation
      || reviewed.length !== offered.length || reviewed.some((value, i) => value !== offered[i])) {
    return { changed: true };
  }
  const carried = settled(fresh, plan.selected, plan.policy);
  if (carried.selected.length !== plan.selected.length || carried.policy !== plan.policy) return { changed: true };
  return { request: reentryRequest(carried) };
}

/** Everything the preview section shows, as dictionary keys and counts. */
export function planView(plan, { managed = false } = {}) {
  const counts = planCounts(plan);
  const candidates = plan.entries.filter(entry => entry.kind === 'candidate');
  const listed = candidates.length;
  // candidates whose adapter's observation is off: the one setup hint
  // names the first one's agent
  const unobserved = candidates.filter(entry => !selectable(plan, entry));
  const rows = plan.entries.map(entry => ({
    cardId: entry.cardId,
    session: entry.session,
    source: entry.source ?? null,
    selectable: selectable(plan, entry),
    checked: plan.selected.includes(entry.cardId),
    note: entry.kind === 'candidate' ? text(PROFILE_KEYS[entry.profile], { agent: agentOf(entry.source) })
      : entry.kind === 'shell' ? text('reentry.kind.shell')
        : text(own(REASON_KEYS, entry.reason) || 'reentry.reason.identity-unavailable', { agent: agentOf(entry.source) }),
  }));
  const bypassOn = counts.bypass > 0;
  // Codex reports entering a session only at its first turn, and may go on
  // with an active goal by itself: said once when a Codex row can be chosen
  const codex = candidates.some(entry => selectable(plan, entry) && entry.source === 'codex');
  return {
    visible: plan.entries.length > 0 || plan.unknown === true,
    summary: plan.unknown === true ? text('reentry.summary.unknown')
      : !listed ? text('reentry.summary.none')
      : !counts.candidates ? text('reentry.summary.unavailable', { count: listed })
        : text('reentry.summary', { count: counts.candidates, bypass: counts.candidateBypass }),
    setup: unobserved.length > 0,
    setupSource: unobserved[0]?.source ?? null,
    rows,
    canSelect: counts.candidates > 0,
    // said once for the whole confirmation, under either policy
    notes: counts.candidates > 0
      ? ['reentry.note.ordinary', 'reentry.note.scope', 'reentry.note.context', 'reentry.note.dirs', ...(codex ? ['reentry.note.codex'] : [])]
        .map(key => text(key)) : [],
    help: counts.candidates > 0 ? [text('reentry.note.otherClients'), text('reentry.help.context')] : [],
    allSelected: counts.candidates > 0 && counts.selected === counts.candidates,
    policy: plan.policy,
    bypass: {
      enabled: bypassOn,
      label: bypassOn ? text('reentry.policy.bypass', { count: counts.bypass }) : text('reentry.policy.bypassNone'),
      // the mandatory sentence and its scope while B can be chosen; why it cannot otherwise
      notes: bypassOn ? [text('reentry.policy.bypassWarning'), text('reentry.policy.scope')]
        : [text('reentry.policy.bypassUnavailable')],
      unknown: counts.unknown > 0 ? text('reentry.policy.unknownCount', { count: counts.unknown }) : null,
    },
    confirm: !counts.selected ? null
      : text(managed ? 'reentry.confirmManaged' : 'reentry.confirm', { count: counts.selected }),
  };
}

/* ---------- progress ---------- */

const SHELL_PROGRESS_KEYS = {
  exiting: 'tmux.progress.exiting',
  saving: 'tmux.progress.saving',
  replacing: 'tmux.progress.replacing',
};

/** The restart button's text for a progress event of a restart that asked
 *  for NO re-entry (the behaviour before re-entry existed), or null for a
 *  phase that has none. */
export function shellProgressLabel(progress) {
  const key = own(SHELL_PROGRESS_KEYS, progress?.phase);
  return key ? { key, params: { completed: progress.completed, total: progress.total } } : null;
}

const count = value => (Number.isInteger(value) && value >= 0 && value <= 9999 ? value : null);

/** The user stage of a backend progress event: which of the three, and the
 *  counts when the stage has any. Unknown phases show nothing. A restart
 *  that reopens nothing has no third stage. */
export function progressView(progress, { reopening = true } = {}) {
  const stage = own(PHASE_STAGE, progress?.phase);
  if (!stage) return null;
  const completed = count(progress.completed), total = count(progress.total);
  const counted = stage !== 'restarting' && completed != null && total != null && total > 0 && completed <= total;
  return {
    stage,
    stages: STAGES.filter(name => reopening || name !== 'reopening' || stage === 'reopening')
      .map(name => ({ stage: name, label: text(STAGE_KEYS[name]), current: name === stage })),
    counts: counted ? `${completed}/${total}` : '',
  };
}

/* ---------- result ---------- */

/** A backend ReentryResult reduced to the fields this contract names. */
export function reentryResult(raw) {
  if (!raw || typeof raw !== 'object' || !Array.isArray(raw.entries)) return null;
  const state = raw.state === 'complete' ? 'complete' : 'running';
  return {
    state,
    serverReplaced: raw.serverReplaced === true,
    entries: raw.entries.map(entry => {
      const cardId = idOf(entry?.cardId);
      return cardId && {
        cardId,
        // final once the backend says so, or once the whole operation is
        settled: state === 'complete' || entry.settled === true,
        session: idOf(entry.session) || '',
        source: SOURCES.includes(entry.source) ? entry.source : null,
        outcome: OUTCOMES.includes(entry.outcome) ? entry.outcome : null,
        reason: own(REASON_KEYS, entry.reason) ? entry.reason : null,
        warnings: (Array.isArray(entry.warnings) ? entry.warnings : []).filter(code => own(WARNING_KEYS, code)),
        shell: entry.shell === true,
        clientObserved: entry.clientObserved === true,
      };
    }).filter(Boolean),
  };
}

/** What `restart_tmux_server` answered. `replaced` is the server conclusion
 *  and comes first: the call's own flag, or the operation's. */
export function restartOutcome(answer) {
  const server = answer && typeof answer === 'object' && answer.server && typeof answer.server === 'object' ? answer.server : null;
  const result = reentryResult(answer?.reentry);
  return { server, result, replaced: answer?.replaced === true || result?.serverReplaced === true };
}

// one line per entry: launch-profile-unconfirmed already says that bypass was
// not reapplied, so the pair is shown once
function warningLines(warnings, agent) {
  const codes = [...new Set(warnings)];
  const merged = codes.includes('launch-profile-unconfirmed') ? codes.filter(code => code !== 'bypass-not-reapplied') : codes;
  return merged.map(code => text(WARNING_KEYS[code], { agent }));
}

// An entry whose outcome is not final yet: in progress while the list is
// still being followed, "still pending" once the bounded poll has ended.
function pendingRow(base, pollEnded) {
  return pollEnded
    ? { ...base, tone: 'pending', line: text('reentry.result.stillPending'), details: [text('reentry.result.checkLater')] }
    : { ...base, tone: 'pending', line: text('reentry.result.reopening'), details: [] };
}

// `running`: the operation this entry belongs to has not completed. Only then
// can an entry be unsettled, and only the boolean true settles it.
function resultRow(entry, running, pollEnded) {
  const base = { cardId: entry.cardId, session: entry.session, shell: entry.shell, started: false };
  if (running && entry.settled !== true) return pendingRow(base, pollEnded);
  const agent = agentOf(entry.source);
  const A = { agent };
  // the bypass mark belongs to a confirmed or started entry alone: nothing
  // about a flag is said for an entry whose request may never have arrived
  const verified = entry.outcome === 'confirmed' || entry.outcome === 'started';
  const warnings = warningLines(verified ? entry.warnings
    : entry.warnings.filter(code => code !== 'bypass-reauthorized'), agent);
  const reason = own(REASON_KEYS, entry.reason);
  if (entry.outcome === 'confirmed') {
    return { ...base, tone: 'ok', details: warnings,
      line: text(entry.warnings.includes('bypass-reauthorized') ? 'reentry.result.confirmed' : 'reentry.result.confirmedOrdinary', A) };
  }
  // started by exactly the request, in the expected context, by an agent
  // that reports entering only at its first turn: never "re-entered"
  if (entry.outcome === 'started') {
    return { ...base, tone: 'ok', started: true, details: [text('reentry.result.startedDetail', A), ...warnings],
      line: text('reentry.result.started', A) };
  }
  if (entry.outcome === 'unconfirmed') {
    const context = CONTEXT_REASONS.includes(entry.reason);
    const more = reason && entry.reason !== 'launch-unconfirmed' ? [text(reason, A)] : [];
    return { ...base, tone: 'warn', details: [...more, ...(context ? [] : [text('reentry.result.check')]), ...warnings],
      line: text(entry.clientObserved || context ? 'reentry.result.startedUnconfirmed' : 'reentry.result.unconfirmed', A) };
  }
  if (!entry.outcome) {
    return { ...base, tone: 'warn', line: text('reentry.result.unknown'), details: [text('reentry.result.check')] };
  }
  return { ...base, tone: 'info', details: [...(reason ? [text(reason, A)] : []), ...warnings],
    line: text(entry.outcome === 'not-attempted' ? 'reentry.result.notAttempted' : 'reentry.result.notStarted', A) };
}

/** One result row per entry of a reduced ReentryResult, worded exactly as
 *  the restart's list words them; a crash recovery (`recovery-model.js`)
 *  shares them and says its own title. */
export function entryRows(result, { pollEnded = false } = {}) {
  const pending = !!result && result.state !== 'complete';
  return (result?.entries || []).map(entry => resultRow(entry, pending, pending && pollEnded === true));
}

/** The result list. `replaced` is the shell service conclusion (the call's
 *  flag, the operation's, or the observed server); `result` a reduced
 *  ReentryResult or null when the operation is not known; `requested` the
 *  card ids that were sent; `pollEnded` says that nobody is reading the
 *  operation any more although it was still running. */
export function projectResult({ replaced, result, requested = [], pollEnded = false }) {
  const done = replaced === true || result?.serverReplaced === true;
  const pending = !!result && result.state !== 'complete';
  const ended = pending && pollEnded === true;
  const rows = (result?.entries || []).map(entry => resultRow(entry, pending, ended));
  const answered = new Set(rows.map(row => row.cardId));
  for (const cardId of requested) {
    if (!idOf(cardId) || answered.has(cardId)) continue;
    answered.add(cardId);
    // a running operation has not answered for this card yet; an operation
    // nobody knows, or a complete one without it, has no answer to wait for
    rows.push(pending ? pendingRow({ cardId, session: '', shell: false }, ended)
      : { cardId, session: '', shell: false, tone: 'warn', line: text('reentry.result.unknown'), details: [text('reentry.result.check')] });
  }
  const started = rows.filter(row => row.started).length;
  const confirmed = rows.filter(row => row.tone === 'ok').length - started;
  const waiting = rows.filter(row => row.tone === 'pending').length;
  const other = rows.length - confirmed - started - waiting;
  // a started entry is counted as such, never among the re-entered
  const summary = !rows.length ? null
    : started && waiting ? text('reentry.result.summaryPendingStarted', { confirmed, started, pending: waiting, other })
    : started ? text('reentry.result.summaryStarted', { confirmed, started, other })
    : waiting ? text('reentry.result.summaryPending', { confirmed, pending: waiting, other })
      : text('reentry.result.summary', { confirmed, other });
  return {
    replaced: done,
    pending,
    title: text(done ? 'reentry.result.restarted' : 'reentry.result.notRestarted'),
    summary,
    notes: [
      ...(done ? [] : [text('reentry.result.agentsMayHaveExited')]),
      ...(waiting ? [text(ended ? 'reentry.result.pendingEnded' : 'reentry.result.pending')] : []),
    ],
    rows,
    footer: [...(done ? [text('reentry.result.queuesPaused')] : []), text(WARNING_KEYS['additional-dirs-not-restored'])],
    // one toast at most, and only the server conclusion
    toast: done ? 'tmux.restartComplete' : null,
  };
}

/* ---------- lost response ---------- */

// Refusals the backend makes before anything of this request ran, each with
// its own sentence: nothing to look up for them.
const REFUSED_BEFORE_START = new Set([
  'managed-review-changed', 'managed-status-unavailable', 'managed-reminder-protected', 'managed-close-rejected',
  'managed-close-ambiguous', 'tmux-restart-mcp-managed-sessions', 'tmux-server-impact-changed',
  'tmux-restart-agent-timeout', 'tmux-restart-snapshot-failed', 'tmux-restart-busy', 'reentry-preview-expired',
]);

/** Whether a rejected restart that asked for re-entry may have an operation
 *  whose facts can be read: a timeout or any answer that is not a known
 *  refusal. */
export function responseLost(message, requested) {
  return !!requested && !REFUSED_BEFORE_START.has(message);
}

/** Read what the backend knows about this request, waiting (bounded) while
 *  its worker is still finishing. Read-only: the one command called here is
 *  `agent_reentry_operation`. Every readable answer goes to `onRead`; the
 *  poll stops early once `active()` says nobody is looking any more. */
export async function awaitReentryResult({ requestId, invoke, wait, attempts = 20, everyMs = 1000, onRead = null, active = null }) {
  let result = null;
  for (let attempt = 0; ; attempt++) {
    let operation = null;
    try { operation = await invoke('agent_reentry_operation', { requestId }); } catch (_) { operation = null; }
    const read = operation?.known === true ? reentryResult(operation.result) : null;
    if (active && !active()) return { known: !!result, result };
    if (read) { result = read; onRead?.(read); }
    if (!read || read.state === 'complete' || attempt >= attempts) return { known: !!result, result };
    await wait(everyMs);
    if (active && !active()) return { known: !!result, result };
  }
}
