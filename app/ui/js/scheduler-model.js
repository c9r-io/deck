// scheduler-model.js — the ⏱ panel's DOM-free half: what a queue item READS
// as (its time, its meta line, its context label, a row's quiet hint) and
// which backend calls start a list. scheduler.js owns the panel's DOM and
// imports these; node tests exercise them directly (../test/scheduler-model.test.mjs).
// Keep this module free of document/window access and Tauri APIs; time comes
// in as an argument so a test can pin "today".
import { contextStatusKey, hasWindow, minToHM, nextFire, quietSecsOf, winHas } from './pure.js';
import { formatDateTime, formatInterval, formatNumber, t } from './i18n.js';

/* "09:30" today, "3/14 09:30" on another day */
export function fmtClock(ts, now = new Date()) {
  const d = new Date(ts * 1000);
  const today = now.toDateString() === d.toDateString();
  return formatDateTime(d, today
    ? { hour: '2-digit', minute: '2-digit' }
    : { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });
}

/* when an item fires, in the user's words: after the previous row, every N,
   or at its instant */
export function fmtWhen(i, now = new Date()) {
  if (i.mode === 'chain') return t('queue.afterPrevious');
  if (i.mode === 'every') return t('queue.every', { interval: formatInterval(i.every) });
  return fmtClock(i.at, now);
}

/* the quiet countdown behind a chain row: "quiet 12/30 s", "quiet reached",
   or the stopped notice when the session is gone */
export function localizedChainQuietHint(idleSecs, alive, total = quietSecsOf()) {
  if (!alive) return t('queue.quiet.stopped');
  if (idleSecs == null) return '';
  const seconds = Math.min(Math.floor(idleSecs), total);
  return seconds >= total ? t('queue.quiet.done') : t('queue.quiet.progress', { seconds, total });
}

/* the closed context status of an item (or its last probe) as a label; a
   foreground mismatch names the expected process */
export const contextLabel = item => {
  const check = item?.last_context || item;
  if (!check) return '';
  if (check.status === 'foreground-different') {
    return t('queue.context.differentProcess', { process: item.expected_process || '?' });
  }
  return t(contextStatusKey(check.status));
};

/* what follows "after the previous row" on a chain head: its quiet hint
   against the card's live idle time */
export const chainWhenSuffix = (i, card) =>
  (i.mode === 'chain' && card ? localizedChainQuietHint(card.idle, card.status !== 'stopped', quietSecsOf(i)) : '');

/* the grey meta line of a list head: delivery state, template position, and
   for a repeating list its window, next fire, count and stop */
export function qMeta(i, now = new Date()) {
  const parts = [];
  if (i.state === 'ambiguous') parts.push(t('queue.meta.ambiguous'));
  else if (i.state === 'firing') parts.push(t('queue.meta.sending'));
  if (i.tpl) parts.push(`tpl·${i.tpl} ${i.tpl_idx}/${i.tpl_total}`);
  if (i.mode === 'every') {
    if (hasWindow(i)) parts.push(minToHM(i.win_from) + '–' + minToHM(i.win_to));
    if (i.paused) {
      parts.push(t('queue.meta.paused'));
    } else {
      const sleeping = hasWindow(i) && !winHas(now.getHours() * 60 + now.getMinutes(), i.win_from, i.win_to);
      const notYet = i.not_before && i.not_before * 1000 > now.getTime();
      const nowSecs = Math.floor(now.getTime() / 1000);
      parts.push(t(notYet ? 'queue.meta.from' : sleeping ? 'queue.meta.sleeping' : 'queue.meta.next', { time: fmtClock(nextFire(i, nowSecs), now) }));
    }
    if (i.fired) parts.push(formatNumber(i.fired) + '×' + (i.until_n ? '/' + formatNumber(i.until_n) : ''));
    if (i.state === 'failed') parts.push(t('queue.meta.failed', { attempts: formatNumber(i.attempts) }));
    else if (i.until_n) parts.push(t('queue.meta.stops', { count: formatNumber(i.until_n) }));
    if (i.until_at) parts.push(t('queue.meta.until', { time: fmtClock(i.until_at, now) }));
  }
  return parts.join(' · ');
}

/* the backend calls that start a list from `steps`, in order: a repeating
   list is ONE rule holding the whole template (steps 2..N are its embedded
   steps); a reviewed one-shot list enters as one transaction; a plain
   one-shot list is an "at" head plus chain rows that follow it. `base` is the
   card's identity (session, cardId, dir, cmd) plus `reviewEach`; `tpl` tags
   every row with the template's name and position. */
export function listStartCalls(base, sched, steps, tpl = null) {
  const tag = k => (tpl ? { tpl: tpl.name, tplIdx: k + 1, tplTotal: steps.length } : {});
  if (sched.mode === 'every') {
    return [['queue_add', { args: { ...base, ...sched, text: steps[0], steps: steps.slice(1), ...tag(0) } }]];
  }
  if (base.reviewEach) {
    return [['queue_add_reviewed_list', { args: { ...base, ...sched, text: steps[0], ...tag(0) }, texts: steps }]];
  }
  return steps.map((text, k) => {
    const follow = { mode: 'chain', quietSecs: null };
    return ['queue_add', { args: { ...base, ...(k === 0 ? sched : follow), text, ...tag(k) } }];
  });
}
