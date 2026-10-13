// board-card.js — Board card presentation and gesture routing.
// Owns the fixed card DOM, labels/chips and event isolation. No persistence,
// polling, session lifecycle or placement decision belongs here. Actions are
// supplied by Board and retain its existing confirmation/transaction paths.
// Native ES module; imports point only to existing painters/models, never back
// to the view core. User text enters through textContent, never markup.
import { dotTitle, setMemChip } from './state.js';
import { paintCardSignalStatus, paintCardAttentionBadge } from './attention.js';
import { setQueueChip } from './scheduler.js';
import { ruleOf } from './automation.js';
import { ruleLabel } from './automation-model.js';
import { t } from './i18n.js';

/**
 * @typedef {Object} BoardCardActions
 * @property {(id: string) => unknown} open
 * @property {(id: string) => unknown} close
 * @property {(id: string) => Promise<unknown>} togglePinned
 * @property {(event: MouseEvent, id: string) => unknown} context
 * @property {(card: Object) => HTMLElement} reminderChip
 */

/**
 * Render one current card snapshot; handlers delegate by its stable id.
 * @param {{id: string, session: string, title: string, status: string,
 *   cmd?: string, dir: string, desc?: string, pinned?: boolean,
 *   mem?: number, origin?: Object, reminder?: Object}} s
 * @param {BoardCardActions} actions
 * @returns {HTMLElement}
 */
export function createBoardCard(s, { open, close, togglePinned, context, reminderChip }) {
  const el = document.createElement('div');
  el.className = 'card' + (s.status === 'waiting' ? ' waiting' : s.status === 'attention' ? ' attention' : '');
  el.dataset.sid = s.id;
  el.draggable = true;

  /* fixed shape: no hover-only rows, the description line is reserved even
     while empty, and a poll only changes text and classes — cards never
     change size under the pointer or when a description is added. The
     attention badge is always present and only hidden (attention.js). */
  el.innerHTML = `
    <div class="card-top"><span class="dot ${s.status}"></span><span class="card-title"></span><span class="card-attention-badge" hidden></span><button class="card-pin" type="button"></button><button class="card-x" type="button">✕</button></div>
    <div class="card-meta"><span class="cmd"></span><span class="dir"></span><span class="auto-chip"></span><span class="q-chip"></span><span class="mem-chip"></span></div>
    <div class="card-status"><span class="card-status-text"></span><button class="card-signal-help" type="button" hidden></button></div>
    <div class="card-desc"></div>`;
  el.querySelector('.card-title').textContent = s.title;
  paintCardSignalStatus(el.querySelector('.card-status'), s);
  if (s.reminder) el.querySelector('.card-status').append(reminderChip(s));
  paintCardAttentionBadge(el.querySelector('.card-attention-badge'), s);
  el.querySelector('.dot').title = dotTitle(s.status);
  const pin = el.querySelector('.card-pin');
  const pinLabel = t(s.pinned === true ? 'card.unmarkImportant' : 'card.markImportant');
  pin.textContent = s.pinned === true ? '★' : '☆';
  pin.classList.toggle('active', s.pinned === true);
  pin.title = pinLabel;
  pin.setAttribute('aria-label', pinLabel);
  pin.setAttribute('aria-pressed', s.pinned === true ? 'true' : 'false');
  el.querySelector('.card-x').title = t('session.closeTitle');
  el.querySelector('.mem-chip').title = t('session.memory');
  setMemChip(el.querySelector('.mem-chip'), s);
  setQueueChip(el.querySelector('.q-chip'), s);   // self-fill: survives card rebuilds
  const autoChip = el.querySelector('.auto-chip');
  const rule = ruleOf(s.origin);
  autoChip.textContent = rule ? '↻ ' + ruleLabel(rule) : '';
  autoChip.hidden = !rule;
  el.querySelector('.cmd').textContent = s.cmd ? '$ ' + s.cmd : '';
  el.querySelector('.dir').textContent = s.dir;
  const desc = el.querySelector('.card-desc');
  desc.textContent = s.desc || '';
  if (s.desc) desc.title = s.desc;
  el.addEventListener('dragstart', e => {
    e.dataTransfer.setData('text/deck-session', s.id);
    el.classList.add('dragging');
  });
  el.addEventListener('dragend', () => el.classList.remove('dragging'));
  el.querySelector('.card-x').onclick = e => {
    e.stopPropagation();
    close(s.id);
  };
  /* busy is `aria-disabled`, never `disabled`, so a focused star keeps its
     focus while the save runs (attention.js has the reason) */
  pin.onclick = async e => {
    e.stopPropagation();
    if (pin.getAttribute('aria-disabled') === 'true') return;
    pin.setAttribute('aria-disabled', 'true');
    try { await togglePinned(s.id); }
    finally { pin.removeAttribute('aria-disabled'); }
  };
  el.addEventListener('click', e => {
    e.stopPropagation();   // don't toggle the board selection underneath
    if (e.target.closest('.card-x, .card-pin, .card-signal-help, .card-reminder')) return;
    open(s.id);
  });
  el.oncontextmenu = e => context(e, s.id);
  return el;
}
