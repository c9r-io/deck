// quota.js — the sidebar's Quota area, its detail popover and a card's quota
// chip in the session pane head. One account per row (quota.rs dedupes by
// account): the agent and its folders, one thin bar per usage window with
// its percent, the reset as a relative time; stale data dashes its bars and
// says "as of HH:MM"; a window past its reset says so instead of a guess.
// The area is hidden while no account was observed (the Quota switch off).
// Presentation only: no notification, badge, attention row, card colour or
// order follows from it. Polls `quota_snapshot` every 20 s while the window
// is visible, and at once when the switch changes (`deck-quota-changed`).
// Part of deck's no-build frontend: native ES modules, no bundler.

import { $, inv } from './state.js';
import { t } from './i18n.js';
import { accountLabel, chipFor, clock, level, resetIn, tightest, tightestOverall, windowShort } from './quota-model.js';

const POLL_MS = 20000;
let snapshot = { accounts: [] };
let deps = { provider: null, openSession: null };
let timer = null;
let open = null;
const listeners = new Set();

export const quotaSnapshot = () => snapshot;
export const onQuotaChange = fn => listeners.add(fn);

function el(tag, cls, text) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

function bar(w, stale) {
  const outer = el('span', `quota-bar quota-${w.expired ? 'expired' : level(w.usedPercent)}${stale ? ' quota-stale' : ''}`);
  const fill = el('span', 'quota-fill');
  fill.style.width = `${w.expired ? 0 : Math.max(0, Math.min(100, w.usedPercent))}%`;
  outer.append(fill);
  return outer;
}

function windowRow(w, stale) {
  const row = el('span', 'quota-win');
  row.append(el('span', 'quota-win-name', windowShort(w)), bar(w, stale),
    el('span', 'quota-win-pct', w.expired ? '—' : `${Math.round(w.usedPercent)}%`));
  return row;
}

function paintArea() {
  const area = $('quota-area');
  if (!area) return;
  const accounts = snapshot.accounts || [];
  area.hidden = accounts.length === 0 && !snapshot.codex;
  const list = $('quota-list');
  list.replaceChildren();
  for (const account of accounts) {
    const row = el('button', 'quota-row');
    row.type = 'button';
    row.dataset.account = account.id;
    const head = el('span', 'quota-row-head');
    head.append(el('span', 'quota-label', accountLabel(account)));
    if (account.stale) head.append(el('span', 'quota-asof', t('quota.asOf', { time: clock(account.observedAt) })));
    row.append(head);
    const wins = el('span', 'quota-wins');
    for (const w of account.windows) wins.append(windowRow(w, account.stale));
    row.append(wins);
    const w = tightest(account);
    row.title = w ? `${accountLabel(account)} — ${windowShort(w)} ${Math.round(w.usedPercent)}%, ${resetIn(w.resetsAt, Date.now(), t)}` : accountLabel(account);
    row.onclick = event => { event.stopPropagation(); showDetail(account.id, row); };
    list.append(row);
  }
  if (snapshot.codex === 'unreadable') list.append(el('div', 'quota-note', t('quota.codexUnreadable')));
  const ring = $('quota-ring');
  const top = tightestOverall(snapshot);
  ring.className = `quota-ring quota-${top ? level(top.usedPercent) : 'ok'}`;
  ring.textContent = top ? `${Math.round(top.usedPercent)}` : '–';
}

function cardTitle(session) {
  const card = deps.provider?.list?.().find(c => c.session === session);
  return card ? { id: card.id, title: card.title } : null;
}

export function showDetail(accountId, anchor) {
  closeDetail();
  const account = (snapshot.accounts || []).find(a => a.id === accountId);
  if (!account) return;
  const pop = el('div', 'quota-pop');
  pop.id = 'quota-pop';
  pop.setAttribute('role', 'dialog');
  pop.append(el('h3', 'quota-pop-title', accountLabel(account)));
  if (account.plan) pop.append(el('div', 'quota-pop-plan', account.plan));
  for (const w of account.windows) {
    const line = el('div', 'quota-pop-win');
    line.append(windowRow(w, account.stale),
      el('span', 'quota-pop-reset', w.expired ? t('quota.resetWaiting') : resetIn(w.resetsAt, Date.now(), t)));
    pop.append(line);
  }
  pop.append(el('div', 'quota-pop-meta', t(account.source === 'rollout' ? 'quota.sourceRollout' : 'quota.sourceStatusline',
    { time: clock(account.observedAt) })));
  if (!account.byAccount) pop.append(el('div', 'quota-pop-meta', t('quota.byFolder')));
  const folders = el('div', 'quota-pop-meta');
  folders.textContent = (account.folders || []).map(f => f.path).join(' · ');
  pop.append(folders);
  const cards = (account.sessions || []).map(cardTitle).filter(Boolean);
  if (cards.length) {
    pop.append(el('div', 'quota-pop-sub', t('quota.cards')));
    for (const card of cards) {
      const link = el('button', 'quota-pop-card', card.title);
      link.type = 'button';
      link.onclick = () => { closeDetail(); deps.openSession?.(card.id); };
      pop.append(link);
    }
  }
  pop.append(el('div', 'quota-pop-meta', t('quota.presentationOnly')));
  document.body.append(pop);
  const r = anchor.getBoundingClientRect();
  pop.style.left = `${Math.min(r.right + 8, window.innerWidth - pop.offsetWidth - 8)}px`;
  pop.style.top = `${Math.max(8, Math.min(r.top, window.innerHeight - pop.offsetHeight - 8))}px`;
  open = pop;
}

export function closeDetail() {
  open?.remove();
  open = null;
}

/* a card's chip in its pane head, before the close button; none when the
   card's session is linked to no account */
export function paintQuotaChip(head, session) {
  if (!head) return;
  let chip = head.querySelector('.quotachip');
  const view = chipFor(snapshot, session);
  if (!view) { chip?.remove(); return; }
  if (!chip) {
    chip = el('button', 'quotachip');
    chip.type = 'button';
    head.querySelector('.px')?.before(chip);
  }
  chip.className = `quotachip quota-${view.level}${view.stale ? ' quota-stale' : ''}`;
  chip.textContent = t('quota.chip', { text: view.text });
  chip.onclick = event => { event.stopPropagation(); showDetail(view.account, chip); };
}

export async function refreshQuota() {
  try {
    snapshot = await inv('quota_snapshot');
  } catch {
    return;
  }
  paintArea();
  for (const fn of listeners) fn(snapshot);
}

export function initQuota(next) {
  deps = { ...deps, ...next };
  document.addEventListener('click', event => { if (open && !open.contains(event.target)) closeDetail(); });
  document.addEventListener('keydown', event => { if (event.key === 'Escape') closeDetail(); });
  window.addEventListener('deck-quota-changed', () => refreshQuota());
}

export function startQuota() {
  if (timer) return;
  refreshQuota();
  timer = setInterval(() => { if (!document.hidden) refreshQuota(); }, POLL_MS);
}
