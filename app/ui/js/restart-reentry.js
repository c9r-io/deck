// restart-reentry.js — the "Reopen Agent session" part of the shell service
// restart modal: the candidate list and the per-restart policy choice, the
// three progress stages, and the one result list. It renders what
// reentry-model.js plans; app.js owns the restart transaction, the modal's
// buttons and every IPC call. No new manager: the three containers live
// inside the existing modal (`tmux-reentry`, `tmux-progress`, `tmux-result`).
//
// The plan is held here in memory for the modal that is open and is replaced
// by every preview (`showReentryPreview`); it is never stored. Candidates
// start unchecked and the ordinary policy is the default; no control here is
// focused by default and none confirms the restart. A preview without
// lifecycle observation lists its candidates as unavailable with ONE setup
// hint that opens the Settings toggle: nothing is installed from here.
// While something can be selected, the scope notes (the new shell's startup
// configuration among them) stand once under the policy choice, and one
// closed `<details>` holds what is said on request; neither is repeated per
// card, asked as a question or kept as a setting.
// A card is named by its Board title; a card the Board no longer has is named
// by its session only when that passes the model's display guard. The result
// offers "Open card" per entry through the caller's existing navigation: no
// retry, no replay of a command. An entry that was not reopened or started
// also offers "Insert resume command": the caller opens the card and puts
// the resume command the pane's own exit hint names into the shell's input
// line, without submitting it (`terminal.insertResumeHint`); nothing is
// typed where no hint is shown.
// Progress has two forms and `showRestartProgress` picks by what THIS restart
// asked for: a restart with no re-entry request keeps the per-phase text on
// the restart button, exactly as before re-entry existed, and never shows the
// stage list; the three stages are for a restart that requested re-entry.
// The modal box carries the class `reentry` only while this module shows
// something in it, so a plain restart review keeps its earlier size.
// What it needs from the app arrives through `initReentry(deps)`.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $ } from './state.js';
import { t } from './i18n.js';
import {
  POLICY_BYPASS, POLICY_ORDINARY, displayable, planCounts, planView, progressView, projectResult, reentryPlan,
  shellProgressLabel, withAllSelected, withPolicy, withSelection,
} from './reentry-model.js';

let titleOf = () => null;
let openCard = () => {};
let insertResume = () => {};
let openSetup = () => {};
let changed = () => {};
let managed = () => false;

let plan = reentryPlan(null);
let locked = false;
let shownResult = null;

const say = line => t(line.key, line.params);
function el(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content != null) node.textContent = content;
  return node;
}
const nameOf = row => titleOf(row.cardId) || displayable(row.session) || t('reentry.unknownCard');

/** The plan the user sees right now (the confirmation reads it once). */
export const currentReentryPlan = () => plan;
export const reentryConfirmLabel = () => planView(plan, { managed: managed() }).confirm;
export const reentryResultShown = () => !!shownResult;

function choice(type, checked, enabled, label, onchange) {
  const row = el('label', 'reentry-choice');
  const input = el('input');
  input.type = type;
  if (type === 'radio') input.name = 'reentry-policy';
  input.checked = checked;
  input.disabled = locked || !enabled;
  input.onchange = () => onchange(input.checked);
  row.append(input, el('span', 'reentry-choice-text', label));
  return row;
}

// The section is rebuilt on every change; the control that was used keeps
// the keyboard focus.
const controls = () => [...$('tmux-reentry').querySelectorAll('input')];
function update(next) {
  const at = controls().indexOf(document.activeElement);
  plan = next;
  renderReentryPreview();
  if (at >= 0) controls()[at]?.focus();
  changed();
}

export function renderReentryPreview() {
  const box = $('tmux-reentry');
  const view = planView(plan, { managed: managed() });
  box.replaceChildren();
  box.hidden = !view.visible || !!shownResult;
  $('tmux-lifecycle-box').classList.toggle('reentry', !box.hidden || !!shownResult);
  if (box.hidden) return;
  box.append(el('div', 'reentry-heading', t('reentry.heading')), el('div', 'reentry-summary', say(view.summary)));
  if (view.setup) {
    const setup = el('div', 'reentry-setup');
    const open = el('button', 'btn', t('reentry.setupOpen'));
    open.type = 'button';
    open.disabled = locked;
    open.onclick = () => openSetup(view.setupSource);
    setup.append(el('span', null, t('reentry.setupHint')), open);
    box.append(setup);
  }
  if (view.canSelect) {
    box.append(choice('checkbox', view.allSelected, true, t('reentry.selectAll'), on => update(withAllSelected(plan, on))));
  }
  const list = el('div', 'reentry-list');
  for (const row of view.rows) {
    const line = row.selectable
      ? choice('checkbox', row.checked, true, nameOf(row), on => update(withSelection(plan, row.cardId, on)))
      : el('div', 'reentry-choice reentry-off');
    if (!row.selectable) line.append(el('span', 'reentry-choice-text', nameOf(row)));
    line.append(el('span', 'reentry-note', say(row.note)));
    list.append(line);
  }
  box.append(list);
  if (!view.canSelect) return;
  const policy = el('fieldset', 'reentry-policy');
  policy.append(el('legend', null, t('reentry.policy.title')));
  policy.append(
    choice('radio', view.policy === POLICY_ORDINARY, true, t('reentry.policy.ordinary'), () => update(withPolicy(plan, POLICY_ORDINARY))),
    el('div', 'reentry-hint', t('reentry.policy.ordinaryHint')),
    choice('radio', view.policy === POLICY_BYPASS, view.bypass.enabled, say(view.bypass.label), () => update(withPolicy(plan, POLICY_BYPASS))),
    ...view.bypass.notes.map(note => el('div', view.bypass.enabled ? 'reentry-hint reentry-warning' : 'reentry-hint', say(note))),
  );
  if (view.bypass.unknown) policy.append(el('div', 'reentry-hint', say(view.bypass.unknown)));
  box.append(policy);
  // the scope notes are said once for the whole confirmation, never per card
  const notes = el('div', 'reentry-notes');
  for (const note of view.notes) notes.append(el('div', 'reentry-scope-note', say(note)));
  // what remains is said once, on request
  const more = el('details', 'reentry-more');
  more.append(el('summary', null, t('settings.details')), ...view.help.map(line => el('div', 'reentry-help', say(line))));
  notes.append(more);
  box.append(notes);
}

/** A new preview is a new plan: nothing selected, ordinary policy. */
export function showReentryPreview(preview) {
  plan = reentryPlan(preview);
  shownResult = null;
  locked = false;
  $('tmux-result').hidden = true;
  $('tmux-result').replaceChildren();
  showReentryProgress(null);
  renderReentryPreview();
  changed();
}

/** While the restart runs nothing in the preview can be changed. */
export function lockReentry(on) {
  locked = !!on;
  renderReentryPreview();
}

/** The three stages with the current one marked; null hides them. */
export function showReentryProgress(progress) {
  const box = $('tmux-progress');
  const view = progress ? progressView(progress, { reopening: planCounts(plan).selected > 0 }) : null;
  box.replaceChildren();
  box.hidden = !view;
  if (!view) return;
  for (const stage of view.stages) {
    const row = el('div', stage.current ? 'reentry-stage current' : 'reentry-stage',
      say(stage.label) + (stage.current ? `… ${view.counts}`.trimEnd() : ''));
    if (stage.current) row.setAttribute('aria-current', 'step');
    box.append(row);
  }
}

/** One progress event of the restart that is running. Without a re-entry
 *  request the restart button says the phase and no stage list appears. */
export function showRestartProgress(progress, reentryRequested) {
  if (reentryRequested) { showReentryProgress(progress); return; }
  showReentryProgress(null);
  const label = shellProgressLabel(progress);
  if (label) $('tmux-restart').textContent = say(label);
}

function renderResult() {
  const box = $('tmux-result');
  const view = projectResult(shownResult);
  box.replaceChildren();
  box.hidden = false;
  $('tmux-lifecycle-title').textContent = say(view.title);
  if (view.summary) box.append(el('div', 'reentry-summary', say(view.summary)));
  for (const note of view.notes) box.append(el('div', 'reentry-hint', say(note)));
  const list = el('div', 'reentry-list reentry-result-list');
  for (const row of view.rows) {
    const item = el('div', `reentry-result-row ${row.tone}`);
    const head = el('div', 'reentry-result-head');
    head.append(el('span', 'reentry-mark', { ok: '✓', warn: '⚠', info: '–', pending: '…' }[row.tone]),
      el('span', 'reentry-choice-text', t('reentry.result.line', { card: nameOf(row), result: say(row.line) })));
    if (titleOf(row.cardId) != null) {
      const open = el('button', 'btn reentry-open', t('reentry.result.open'));
      open.type = 'button';
      open.onclick = () => openCard(row.cardId);
      head.append(open);
      // a card whose client was neither reopened nor started: the pane's
      // own exit hint can be put on the input line, never submitted
      if (row.tone === 'warn' || row.tone === 'info') {
        const insert = el('button', 'btn reentry-open', t('reentry.result.insertResume'));
        insert.type = 'button';
        insert.onclick = () => insertResume(row.cardId);
        head.append(insert);
      }
    }
    item.append(head);
    for (const detail of row.details) item.append(el('div', 'reentry-hint', say(detail)));
    list.append(item);
  }
  if (view.rows.length) box.append(list);
  const foot = el('div', 'reentry-notes');
  for (const line of view.footer) foot.append(el('div', null, say(line)));
  box.append(foot);
  return view;
}

/** Switch the modal to the one result list; returns the projection (its
 *  `toast` is the single toast the caller may show). */
export function showReentryResult(outcome) {
  shownResult = outcome;
  showReentryProgress(null);
  renderReentryPreview();
  return renderResult();
}

/** A locale change repaints whatever is showing. */
export function refreshReentry() {
  if (shownResult) renderResult();
  else renderReentryPreview();
}

export function initReentry(deps) {
  ({ titleOf, openCard, openSetup, changed, managed } = deps);
  insertResume = deps.insertResume || (() => {});
}
