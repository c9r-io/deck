// attention.js — the needs-attention view: one derived list across projects
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - Derived, never a Board. Rows come from `attentionRows` (attention-model.js)
//   over the live projects and cards plus `ctx.attention` (the runtime
//   tracker); the view moves no card and persists nothing but its filter.
// - Freshness is explicit: the tracker's `freshness` decides whether counts
//   show ("fresh"), show with an "old" mark ("stale") or show as "—"
//   ("unknown"), and a retry button re-polls. `attentionStatusText` and the
//   status title also feed the Board card's status line, so both surfaces
//   read the same words.
// - Keyed rows preserve focus and scroll: `updateRows` reconciles children by
//   key instead of rebuilding, restores the focused button after the
//   insertBefore blur, and a pointer held inside the list freezes
//   reconciliation until the click has dispatched (`pointerHeld`, released on
//   pointerup, pointercancel and window blur).
// - Every action closes over `card.id`, never an index. Opening a live card
//   re-polls first and checks a navigation token, so a session that exited
//   during the round trip is located on its Board instead; opening never
//   starts a session (`allowStart: false`). Unknown, stale and stopped
//   entries locate the original card.
// - Returning (`showAttention(true)`) restores project, filter, scroll and
//   the focused row from the `ctx.attentionReturn` that `openSession` kept.
import { $, ctx, QUIET_SECS, state, store } from './state.js';
import { ATTENTION_FILTERS, attentionRows } from './attention-model.js';
import { formatDateTime, formatNumber, t } from './i18n.js';
import { pollNow, provider, render, switchProject } from './board.js';
import { leaveSessionView, openSession } from './layout.js';

let pointerHeld = false;
let deferredRender = false;
let navigation = 0;

const node = (tag, className, text) => {
  const el = document.createElement(tag);
  if (className) el.className = className;
  if (text != null) el.textContent = text;
  return el;
};

export function attentionStatusText(card) {
  const snapshot = ctx.attention.get(card);
  if (!snapshot) return t('attention.unknown');
  let text;
  if (!snapshot.alive) text = t('attention.stopped');
  else if (snapshot.agent === 'needs-input') text = t('attention.input');
  else if (snapshot.agent === 'turn-done') text = t(snapshot.seen ? 'attention.doneRead' : 'attention.doneUnread');
  else if (snapshot.agent === 'working') text = t('attention.working');
  else text = t(snapshot.idle == null ? 'attention.noSignal'
    : snapshot.idle >= QUIET_SECS ? 'attention.quiet' : 'attention.recent');
  return snapshot.stale ? `${text} · ${t('attention.old')}` : text;
}

function sourceText(card) {
  const snapshot = ctx.attention.get(card);
  if (!snapshot) return t('attention.unknown');
  if (!snapshot.alive) return t('attention.noSession');
  if (!snapshot.agent) return t('attention.noSignalHint');
  return t(snapshot.seen ? 'attention.viewed' : 'attention.notViewed');
}

function fillTools(container, cards) {
  if (!container.firstElementChild) {
    const summary = node('div', 'attention-summary');
    summary.append(node('span', 'attention-totals'));
    const notice = node('div', 'attention-notice');
    notice.setAttribute('role', 'status');
    notice.append(node('span', 'attention-freshness'));
    const retry = node('button', 'btn');
    retry.type = 'button'; retry.onclick = () => pollNow();
    notice.append(retry);
    const filters = node('div', 'attention-filters');
    filters.setAttribute('role', 'group');
    container.append(summary, notice, filters);
  }
  const tracker = ctx.attention;
  const counts = tracker.counts(cards);
  const freshness = tracker.freshness(cards);
  container.querySelector('.attention-totals').textContent = t('attention.summaryGlobal', {
    input: freshness.kind === 'unknown' ? '—' : formatNumber(counts.input),
    done: freshness.kind === 'unknown' ? '—' : formatNumber(counts.done), unavailable: formatNumber(counts.unavailable),
  }) + (freshness.kind === 'fresh' ? '' : ` · ${t(freshness.kind === 'unknown' ? 'attention.unknown' : 'attention.old')}`);
  const notice = container.querySelector('.attention-notice');
  notice.hidden = freshness.kind === 'fresh';
  notice.querySelector('.attention-freshness').textContent = freshness.kind === 'unknown' ? t('attention.unknownHint')
    : t('attention.staleHint', { time: freshness.lastSuccess == null ? '—' : formatDateTime(freshness.lastSuccess, { hour: '2-digit', minute: '2-digit', second: '2-digit' }) });
  notice.querySelector('button').textContent = t('attention.retry');
  const filters = container.querySelector('.attention-filters');
  filters.setAttribute('aria-label', t('attention.globalFilters'));
  const selected = ctx.attentionFilter;
  for (const filter of ATTENTION_FILTERS) {
    let button = [...filters.children].find(el => el.dataset.filter === filter);
    if (!button) {
      button = node('button', 'btn'); button.type = 'button'; button.dataset.filter = filter;
      button.onclick = () => {
        ctx.attentionFilter = filter; $('attention-list').scrollTop = 0;
        refreshAttention();
      };
      filters.append(button);
    }
    const count = freshness.kind === 'unknown' && !['all', 'unavailable'].includes(filter) ? '—' : formatNumber(counts[filter]);
    button.textContent = `${t(`attention.filter.${filter}`)} ${count}`;
    button.setAttribute('aria-pressed', String(selected === filter));
    button.classList.toggle('active', selected === filter);
  }
}

export function refreshBoardAttention() {
  if (state.view !== 'board') return;
  /* an empty project shows the Board's own starting point instead (06 B v01) */
  const emptyProject = !document.querySelector('#columns .card[data-sid]');
  for (const column of document.querySelectorAll('#columns .column')) {
    for (const el of column.querySelectorAll('.card[data-sid]')) {
      const card = provider.get(el.dataset.sid);
      if (!card) continue;
      const status = el.querySelector('.card-status');
      if (status) { status.textContent = attentionStatusText(card); status.title = sourceText(card); }
    }
    let empty = column.querySelector('.attention-column-empty');
    if (!empty) { empty = node('p', 'attention-column-empty'); column.querySelector('.col-cards').append(empty); }
    empty.hidden = emptyProject || !!column.querySelector('.card[data-sid]');
    empty.textContent = t('attention.emptyColumn');
  }
}

function updateRows() {
  if (pointerHeld) { deferredRender = true; return; }
  const list = $('attention-list');
  const scroll = list.scrollTop;
  const focus = document.activeElement;
  const focusedHere = list.contains(focus);
  const rows = attentionRows(store.projects, store.cards, ctx.attention, ctx.attentionFilter);
  const keys = new Set();
  let cursor = list.firstElementChild;
  const place = (key, create) => {
    keys.add(key);
    let el = [...list.children].find(child => child.dataset.key === key);
    if (!el) { el = create(); el.dataset.key = key; }
    if (el !== cursor) list.insertBefore(el, cursor);
    cursor = el.nextElementSibling;
    return el;
  };
  if (rows.length) {
    const labels = place('labels', () => {
      const el = node('div', 'attention-labels');
      for (const key of ['card', 'origin', 'reason', 'viewing', 'action']) el.append(node('span', '', key));
      return el;
    });
    ['card', 'origin', 'reason', 'viewing', 'action'].forEach((key, i) => {
      labels.children[i].textContent = t(`attention.column.${key}`);
    });
  }
  let previousKind = null;
  for (const entry of rows) {
    const { card, project, column, kind } = entry;
    if (ctx.attentionFilter === 'pending' && kind !== previousKind) {
      const heading = place(`heading-${kind}`, () => node('h2', 'attention-section'));
      heading.textContent = t(`attention.filter.${kind}`);
      previousKind = kind;
    }
    const row = place(`card-${card.id}`, () => {
      const el = node('div', 'attention-row'); el.dataset.sid = card.id;
      el.append(node('span', 'attention-card-name'), node('span', 'attention-origin'), node('span', 'attention-reason'), node('span', 'attention-viewed'));
      const open = node('button', 'btn'); open.type = 'button';
      open.onclick = () => openAttentionCard(card.id);
      el.append(open);
      return el;
    });
    const snapshot = ctx.attention.get(card);
    const locate = !snapshot || snapshot.stale || !snapshot.alive;
    row.querySelector('.attention-card-name').textContent = `${card.title}${card.pinned ? ' ★' : ''}`;
    row.querySelector('.attention-origin').textContent = `${project.name} / ${column.name}`;
    row.querySelector('.attention-reason').textContent = attentionStatusText(card);
    row.querySelector('.attention-reason').dataset.status = snapshot?.status || 'unknown';
    row.querySelector('.attention-viewed').textContent = sourceText(card);
    row.querySelector('button').textContent = t(locate ? 'attention.locate' : 'attention.open');
    row.querySelector('button').setAttribute('aria-label', t(locate ? 'attention.locateNamed' : 'attention.openNamed', { name: card.title, project: project.name }));
  }
  if (!rows.length) {
    const empty = place('empty', () => node('p', 'attention-empty'));
    empty.textContent = t(ctx.attention.freshness(store.cards).kind === 'unknown' ? 'attention.unknownHint'
      : ctx.attention.freshness(store.cards).kind === 'stale' ? 'attention.staleEmpty'
      : ctx.attentionFilter === 'pending' ? 'attention.emptyPending' : 'attention.emptyFilter');
  }
  for (const el of [...list.children]) if (!keys.has(el.dataset.key)) el.remove();
  /* insertBefore of a connected row removes it first, and removal blurs the
     focused button inside it: put focus back where the reader left it */
  if (focusedHere && focus.isConnected) {
    if (document.activeElement !== focus) focus.focus({ preventScroll: true });
  } else if (focusedHere) {
    $('attention-tools').querySelector('[aria-pressed="true"]')?.focus({ preventScroll: true });
  }
  list.scrollTop = scroll;
}

export function refreshAttention() {
  const counts = ctx.attention.counts(store.cards);
  const freshness = ctx.attention.freshness(store.cards);
  $('attention-count').textContent = freshness.kind === 'unknown' ? '—'
    : `${formatNumber(counts.pending)}${freshness.kind === 'stale' ? ` · ${t('attention.old')}` : ''}`;
  $('attention-btn').title = t('attention.globalTitle');
  $('attention-btn').classList.toggle('active', state.view === 'attention');
  $('attention-btn').setAttribute('aria-pressed', String(state.view === 'attention'));
  refreshBoardAttention();
  if (state.view !== 'attention') return;
  $('attention-back').textContent = t('attention.backProject', { name: provider.project(state.projectId)?.name || '' });
  fillTools($('attention-tools'), store.cards);
  updateRows();
}

export function showAttention(returning = false) {
  const saved = returning ? ctx.attentionReturn : null;
  navigation++;
  if (state.view === 'session') leaveSessionView();
  if (saved && provider.project(saved.projectId)) state.projectId = saved.projectId;
  state.view = 'attention'; state.sessionId = null;
  if (saved) ctx.attentionFilter = saved.filter;
  ctx.attentionReturn = null;
  render();
  if (saved) {
    const row = [...$('attention-list').children].find(el => el.dataset.sid === saved.focusId);
    (row?.querySelector('button') || $('attention-tools').querySelector('[aria-pressed="true"]'))?.focus({ preventScroll: true });
    $('attention-list').scrollTop = saved.scroll;
  } else {
    $('attention-tools').querySelector('[aria-pressed="true"]')?.focus({ preventScroll: true });
  }
  pollNow();
}

export function locateAttentionCard(id) {
  const card = provider.get(id);
  if (!card) { refreshAttention(); return; }
  switchProject(card.projectId);
  const el = [...document.querySelectorAll('#columns .card')].find(item => item.dataset.sid === id);
  if (el) { el.tabIndex = -1; el.focus({ preventScroll: true }); el.scrollIntoView({ block: 'nearest', inline: 'nearest' }); }
}

export async function openAttentionCard(id) {
  const token = ++navigation;
  const card = provider.get(id);
  const before = ctx.attention.get(card);
  if (!before || before.stale || !before.alive) { locateAttentionCard(id); return; }
  await pollNow();
  if (token !== navigation || state.view !== 'attention') return;
  const current = provider.get(id);
  const snapshot = ctx.attention.get(current);
  if (!current || !snapshot || snapshot.stale || !snapshot.alive) { locateAttentionCard(id); return; }
  const returnTo = { projectId: state.projectId, filter: ctx.attentionFilter, scroll: $('attention-list').scrollTop, focusId: id };
  await openSession(id, { allowStart: false, attentionReturn: returnTo });
}

export function initAttention() {
  $('attention-btn').onclick = () => showAttention();
  $('attention-back').onclick = () => switchProject(state.projectId);
  document.addEventListener('pointerdown', event => {
    if (event.target.closest('#attention-list')) pointerHeld = true;
  });
  const release = () => {
    setTimeout(() => {
      pointerHeld = false;
      if (deferredRender) { deferredRender = false; refreshAttention(); }
    }, 0);
  };
  document.addEventListener('pointerup', release);
  document.addEventListener('pointercancel', release);
  window.addEventListener('blur', release);
}
