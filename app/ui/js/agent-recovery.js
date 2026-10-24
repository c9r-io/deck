// agent-recovery.js — the "Recover interrupted Agent sessions" dialog: asked
// for once after the Board loaded (`offerRecovery`), shown only when the
// backend has cards whose Claude session was interrupted with the shell
// service. Its own modal, separate from the restart review: nothing here is
// a restart and nothing here reads the restart's state. recovery-model.js
// plans and words; this module owns the DOM and the three IPC calls
// (`agent_recovery_offer`, `agent_recovery_decline`, `agent_recovery_run`).
//
// While the dialog is open the offered cards' shells are held back
// (`ctx.recoveryHold`, read by layout.js before a stopped card's shell is
// started): a shell opened meanwhile would make that card's recovery
// impossible. Declining or finishing releases the hold. Declining deletes
// the offer's records; a dialog left unanswered (Deck quit) is offered again
// at the next launch. The result offers "Open card" per entry and no retry.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, ctx, genId, inv, listen, uev } from './state.js';
import { t } from './i18n.js';
import {
  displayable, heldSessions, planView, projectRecovery, recoveryOffer, recoveryPlan, recoveryRequest,
  withAllSelected, withSelection,
} from './recovery-model.js';

let titleOf = () => null;
let openCard = () => {};
let restoreShells = () => false;
let changed = () => {};

let plan = null;
let running = false;
let progress = null;
let result = null;

const say = line => t(line.key, line.params);
function el(tag, className, content) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (content != null) node.textContent = content;
  return node;
}
const nameOf = row => titleOf(row.cardId) || displayable(row.session) || t('reentry.unknownCard');

function choice(checked, enabled, label, onchange) {
  const row = el('label', 'reentry-choice');
  const input = el('input');
  input.type = 'checkbox';
  input.checked = checked;
  input.disabled = running || !enabled;
  input.onchange = () => onchange(input.checked);
  row.append(input, el('span', 'reentry-choice-text', label));
  return row;
}

const controls = () => [...$('recovery-list').querySelectorAll('input')];
function update(next) {
  const at = controls().indexOf(document.activeElement);
  plan = next;
  render();
  if (at >= 0) controls()[at]?.focus();
}

function renderSelection() {
  const list = $('recovery-list');
  list.replaceChildren();
  list.hidden = !!result;
  $('recovery-message').hidden = !!result;
  $('recovery-notes').hidden = !!result;
  if (result) return;
  const view = planView(plan);
  if (view.canSelect && plan.entries.length > 1) {
    list.append(choice(view.allSelected, true, t('reentry.selectAll'), on => update(withAllSelected(plan, on))));
  }
  const rows = el('div', 'reentry-list recovery-rows');
  for (const row of view.rows) {
    const line = row.selectable
      ? choice(row.checked, true, nameOf(row), on => update(withSelection(plan, row.cardId, on)))
      : el('div', 'reentry-choice reentry-off');
    if (!row.selectable) line.append(el('span', 'reentry-choice-text', nameOf(row)));
    line.append(el('span', 'reentry-note', say(row.note)));
    rows.append(line);
  }
  list.append(rows);
  const confirm = $('recovery-confirm');
  confirm.textContent = say(view.confirm);
  confirm.disabled = running || !view.confirmEnabled;
  confirm.style.display = '';
  $('recovery-decline').disabled = running;
  $('recovery-decline').style.display = '';
  $('recovery-done').style.display = 'none';
}

function renderProgress() {
  const box = $('recovery-progress');
  box.hidden = !running;
  box.textContent = running
    ? t('recovery.progress', { done: progress?.completed ?? 0, total: progress?.total ?? plan.selected.length })
    : '';
}

function renderResult() {
  const box = $('recovery-result');
  box.replaceChildren();
  box.hidden = !result;
  if (!result) return;
  $('recovery-title').textContent = say(result.title);
  if (result.summary) box.append(el('div', 'reentry-summary', say(result.summary)));
  const list = el('div', 'reentry-list reentry-result-list');
  for (const row of result.rows) {
    const item = el('div', `reentry-result-row ${row.tone}`);
    const head = el('div', 'reentry-result-head');
    head.append(el('span', 'reentry-mark', { ok: '✓', warn: '⚠', info: '–', pending: '…' }[row.tone]),
      el('span', 'reentry-choice-text', t('reentry.result.line', { card: nameOf(row), result: say(row.line) })));
    if (titleOf(row.cardId) != null) {
      const open = el('button', 'btn reentry-open', t('reentry.result.open'));
      open.type = 'button';
      open.onclick = () => { close(); openCard(row.cardId); };
      head.append(open);
    }
    item.append(head);
    for (const detail of row.details) item.append(el('div', 'reentry-hint', say(detail)));
    list.append(item);
  }
  if (result.rows.length) box.append(list);
  const foot = el('div', 'reentry-notes');
  for (const line of result.footer) foot.append(el('div', null, say(line)));
  box.append(foot);
  $('recovery-confirm').style.display = 'none';
  $('recovery-decline').style.display = 'none';
  $('recovery-done').style.display = '';
}

function render() {
  if (!plan) return;
  if (!result) $('recovery-title').textContent = t('recovery.title');
  renderSelection();
  renderProgress();
  renderResult();
}

function open() {
  $('recovery-modal').style.display = 'flex';
}

function close() {
  $('recovery-modal').style.display = 'none';
  plan = null;
  result = null;
  progress = null;
}

export const recoveryOpen = () => $('recovery-modal')?.style.display === 'flex';

/** Ask the backend once whether anything was interrupted; show the dialog
 *  only when it was. Called after the Board loaded. */
export async function offerRecovery() {
  let raw = null;
  try { raw = await inv('agent_recovery_offer'); } catch (_) { uev('recovery-offer-fail'); return; }
  const offer = recoveryOffer(raw);
  if (!offer) return;
  ctx.recoveryHold = heldSessions(offer);
  plan = recoveryPlan(offer);
  result = null;
  running = false;
  open();
  render();
}

async function decline() {
  if (running || !plan) return;
  const token = plan.token;
  close();
  ctx.recoveryHold = null;
  try { await inv('agent_recovery_decline', { token }); } catch (_) { uev('recovery-decline-fail'); }
}

async function confirm() {
  const approval = recoveryRequest(plan);
  if (!approval || running) return;
  running = true;
  progress = null;
  render();
  const requestId = genId('V');
  let unlisten = null;
  try {
    unlisten = await listen('agent-recovery-progress', event => {
      if (event.payload?.requestId !== requestId) return;
      progress = event.payload;
      renderProgress();
    });
  } catch (_) { uev('listen-fail', 'agent-recovery-progress'); }
  let answer = null;
  try {
    answer = await inv('agent_recovery_run', { requestId, approval, restoreShells: restoreShells() });
  } catch (_) {
    // a lost answer is read, never asked for again
    try {
      const operation = await inv('agent_reentry_operation', { requestId });
      answer = operation?.known === true ? operation.result : null;
    } catch (_) { answer = null; }
    uev('recovery-run-fail');
  } finally {
    unlisten?.();
    running = false;
    ctx.recoveryHold = null;
  }
  if (!plan) return;
  result = projectRecovery(answer, approval.cards);
  render();
  $('recovery-done').focus();
  changed();
}

export function refreshRecovery() {
  if (recoveryOpen()) render();
}

export function initRecovery(deps) {
  ({ titleOf, openCard, restoreShells, changed } = deps);
  $('recovery-decline').onclick = () => decline();
  $('recovery-confirm').onclick = () => confirm();
  $('recovery-done').onclick = () => close();
}
