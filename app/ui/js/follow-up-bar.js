// follow-up-bar.js — one card's follow-up buttons, painted into a bar its
// caller owns: the card's pane (layout.js) and its Needs attention row
// (attention.js). DOM only: what a card offers, what is already queued and
// what the model picked are follow-up-model.js; a click is the caller's
// `pick(index)`. A bar keeps one line whatever it shows, so a pane's
// terminal never changes size when a suggestion or a queued mark arrives.
import { ctx } from './state.js';
import { promptSummary, promptTooltip } from './pure.js';
import { cardFollowUps, followUpQueued, followUpSuggestion } from './follow-up-model.js';
import { t } from './i18n.js';

/* clicks whose queueing has not answered yet, by card and index */
const inflight = new Set();

function part(bar, className, tag = 'span') {
  let el = bar.querySelector(`.${className}`);
  if (!el) {
    el = document.createElement(tag);
    el.className = className;
    bar.append(el);
  }
  return el;
}

/* Paint `card`'s follow-ups into `bar`; hidden when it has none. */
export function paintFollowUps(bar, card, pick) {
  const texts = cardFollowUps(card);
  bar.hidden = !texts.length;
  if (!texts.length) { bar.replaceChildren(); return; }
  bar.setAttribute('role', 'group');
  bar.setAttribute('aria-label', t('followUps.group', { name: card.title }));
  part(bar, 'follow-label').textContent = t('followUps.label');
  const list = part(bar, 'follow-list');
  const reason = part(bar, 'follow-reason');
  while (list.children.length > texts.length) list.lastElementChild.remove();
  while (list.children.length < texts.length) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'btn follow-btn';
    list.append(button);
  }
  const suggestion = followUpSuggestion(ctx.attention?.get(card), texts.length);
  const stopped = card.status === 'stopped';
  texts.forEach((text, index) => {
    const button = list.children[index];
    const key = `${card.id}/${index}`;
    const queued = followUpQueued(ctx.queueCache?.items, card.session, text);
    const busy = inflight.has(key);
    const suggested = suggestion?.index === index;
    const { first, extra } = promptSummary(text);
    button.textContent = (queued ? '✓ ' : '') + first + (extra ? ` ⏎${extra}` : '');
    button.classList.toggle('suggested', suggested);
    button.classList.toggle('queued', queued);
    button.setAttribute('aria-disabled', String(stopped || queued || busy));
    button.setAttribute('aria-label', t(queued ? 'followUps.queuedNamed' : 'followUps.sendNamed', { text: first }));
    button.title = [
      promptTooltip(text),
      suggested ? t('followUps.suggested') : '',
      stopped ? t('followUps.stopped') : queued ? t('followUps.queuedTitle') : t('followUps.sendTitle'),
    ].filter(Boolean).join('\n');
    button.onclick = async () => {
      if (button.getAttribute('aria-disabled') === 'true') return;
      inflight.add(key);
      button.setAttribute('aria-disabled', 'true');
      try { await pick(index); } finally {
        inflight.delete(key);
        paintFollowUps(bar, card, pick);
      }
    };
  });
  reason.hidden = !suggestion;
  reason.textContent = suggestion ? t(suggestion.reason ? 'followUps.reason' : 'followUps.suggested', { reason: suggestion.reason }) : '';
  reason.title = reason.textContent;
}
