// scheduler.js — scheduled prompts: queue groups, recurring rules, templates
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// A queued prompt may be MANY LINES (`normalizeTemplateStep`/`normalize_prompt`
// keep them; only a CR is folded). The panel therefore stays ONE ROW PER
// PROMPT: a row shows the prompt's first line plus a `⏎N` badge, and only its
// chevron opens the rest — a chain of long prompts must never push the group
// head and its "next fire" out of a 40vh panel. Which rows are open lives in
// `expandedRows` because the panel is rebuilt from scratch on every poll; it
// is pruned against the live queue on each render. Editing edits the WHOLE
// prompt, so a collapsed row opens first and the same click continues into
// the editor, where Enter types a newline and ⌘↵ commits.
//
// The schedule form is a mode picker (after previous / at a time / repeat)
// over ONE parameter row: every control carries the `q-p-<facet>` classes of
// the states it belongs to and `syncForm` shows exactly the controls of the
// active facets, so the row never holds two modes at once. What the form
// sends is the backend's `QueueAddArgs`: a chain item carries its own
// `quietSecs`; a timed prompt is a full local date+time (never rolled to
// "tomorrow" — a past instant is refused); a rule is a minute interval with
// an optional daily window, an optional start (`notBefore`) and a stop. A
// template inserted "after previous" gives every step the chosen quiet
// time; inserted at a time, its follow-up steps keep the default. Calendar
// cadences (daily / weekly / monthly) are deliberately not a card schedule —
// see the Board-level automation note in scheduler/mod.rs.
import { $, ctx, inv, listen, state, uev } from './state.js';
import { blockedBy, chainQuietHint, contextStatusKey, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, isoDate, isoTime, itemDead, localEpoch, MAX_QUIET_SECS, MIN_QUIET_SECS, minToHM, nextFire, promptSummary, promptTooltip, quietSecsOf, winHas } from './pure.js';
export { blockedBy, chainQuietHint, contextStatusKey, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, itemDead, minToHM, nextFire, promptSummary, promptTooltip, winHas };
import { autoGrowField, confirmDangerDialog, confirmDialog, inlineRename, toast, promptDialog } from './dialogs.js';
import { pollNow, provider } from './board.js';
import { strToB64 } from './layout.js';
import { formatDateTime, formatInterval, formatNumber, onLocaleChange, t } from './i18n.js';

/* ---------- scheduled prompts ---------- */
export async function refreshQueue() {
  try { ctx.queueCache = await inv('queue_list'); } catch (e) { return; }
  renderQueueUI();
}

export const sessionQueue = session => ctx.queueCache.items.filter(i => i.session === session);

/* Which multi-line prompts are opened to their full text. The panel is
   rebuilt from scratch on every poll, so this cannot live in the DOM; it is
   pruned against the live queue on each render so removed prompts do not
   leak keys. */
const expandedRows = new Set();

export function setQueueChip(chip, card) {
  if (!chip) return;
  const q = sessionQueue(card.session);
  chip.textContent = q.length ? '⏰' + q.length : '';
  chip.title = q.length
    ? t('queue.next', { when: fmtWhen(q[0]), prompt: promptTooltip(q[0].text) })
    : '';
}

export const fmtClock = ts => {
  const d = new Date(ts * 1000);
  const today = new Date().toDateString() === d.toDateString();
  return formatDateTime(d, today
    ? { hour: '2-digit', minute: '2-digit' }
    : { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' });
};
export function fmtWhen(i) {
  if (i.mode === 'chain') return t('queue.afterPrevious');
  if (i.mode === 'every') return t('queue.every', { interval: formatInterval(i.every) });
  return fmtClock(i.at);
}

export function localizedChainQuietHint(idleSecs, alive, total = quietSecsOf()) {
  if (!alive) return t('queue.quiet.stopped');
  if (idleSecs == null) return '';
  const seconds = Math.min(Math.floor(idleSecs), total);
  return seconds >= total ? t('queue.quiet.done') : t('queue.quiet.progress', { seconds, total });
}

export const contextLabel = item => {
  const check = item?.last_context || item;
  if (!check) return '';
  if (check.status === 'foreground-different') {
    return t('queue.context.differentProcess', { process: item.expected_process || '?' });
  }
  return t(contextStatusKey(check.status));
};

async function refreshItemProbe(item) {
  const result = await inv('queue_probe_context', { id: item.id });
  await refreshQueue();
  return result;
}

async function manualSendNow(item) {
  let probe;
  try { probe = await refreshItemProbe(item); }
  catch (e) { toast(t('queue.context.probeFailed')); return; }
  const mismatch = probe.status === 'foreground-different';
  if (probe.status !== 'ready' && !mismatch) {
    toast(t(contextStatusKey(probe.status)));
    return;
  }
  const target = probe.current_process || t('queue.context.noProcess');
  const message = mismatch
    ? t('queue.manualMismatchConfirm', { expected: probe.expected_process || '?', current: target })
    : t('queue.manualReadyConfirm', { current: target });
  const accepted = mismatch ? await confirmDangerDialog(message) : await confirmDialog(message);
  if (!accepted) return;
  inv('queue_send_now', { id: item.id, acceptProcessMismatch: mismatch })
    .catch(() => toast(t('error.operation', { operation: t('queue.manualNow') })));
}

const chainWhenSuffix = (i, card) =>
  i.mode === 'chain' && card ? localizedChainQuietHint(card.idle, card.status !== 'stopped', quietSecsOf(i)) : '';

/* refresh the quiet counters in place on every poll tick — text-only, so an
   open panel never gets its DOM (hover/click targets, inline edits) rebuilt.
   The panel is per-session, so every chain head shares one hint. */
export function updateQuietHints() {
  if (!ctx.queueOpen || state.view !== 'session') return;
  const card = provider.get(state.sessionId);
  if (!card) return;
  const alive = card.status !== 'stopped';
  document.querySelectorAll('#queue-list .qg-when[data-quiet]').forEach(el => {
    el.textContent = t('queue.afterPrevious') + localizedChainQuietHint(card.idle, alive, Number(el.dataset.quiet));
  });
}

export function qMeta(i) {
  const parts = [];
  if (i.state === 'ambiguous') parts.push(t('queue.meta.ambiguous'));
  else if (i.state === 'firing') parts.push(t('queue.meta.sending'));
  if (i.tpl) parts.push(`tpl·${i.tpl} ${i.tpl_idx}/${i.tpl_total}`);
  if (i.mode === 'every') {
    if (hasWindow(i)) parts.push(minToHM(i.win_from) + '–' + minToHM(i.win_to));
    if (i.paused) {
      parts.push(t('queue.meta.paused'));
    } else {
      const nm = new Date();
      const sleeping = hasWindow(i) && !winHas(nm.getHours() * 60 + nm.getMinutes(), i.win_from, i.win_to);
      const notYet = i.not_before && i.not_before * 1000 > nm.getTime();
      parts.push(t(notYet ? 'queue.meta.from' : sleeping ? 'queue.meta.sleeping' : 'queue.meta.next', { time: fmtClock(nextFire(i)) }));
    }
    if (i.fired) parts.push(formatNumber(i.fired) + '×' + (i.until_n ? '/' + formatNumber(i.until_n) : ''));
    if (i.state === 'failed') parts.push(t('queue.meta.failed', { attempts: formatNumber(i.attempts) }));
    else if (i.until_n) parts.push(t('queue.meta.stops', { count: formatNumber(i.until_n) }));
    if (i.until_at) parts.push(t('queue.meta.until', { time: fmtClock(i.until_at) }));
  }
  return parts.join(' · ');
}

export async function saveGroupAsTemplate(g) {
  const card = provider.get(state.sessionId);
  if (!card) return;
  const steps = groupSteps(g);
  const name = await promptDialog(
    t('queue.templateSavePrompt', { count: formatNumber(steps.length) }),
    g.head.tpl || '');
  if (!name) return;
  await provider.saveTemplate(card.projectId, name, steps);
  toast(t('queue.templateSaved', { name }));
}

export function groupEl(g, card) {
  const rule = g.head.mode === 'every';
  const el = document.createElement('div');
  el.className = 'q-group' + (rule ? ' rule' : '') + (g.head.paused ? ' paused' : '');

  const head = document.createElement('div');
  head.className = 'qg-head';
  head.innerHTML = '<span class="qg-when"></span><span class="qg-meta"></span><span class="qg-act">'
    + (rule ? '<button class="qg-pause"></button>' : '')
    + (rule && itemDead(g.head) ? '<button class="qg-retry">↻</button>' : '')
    + '<button class="qg-save">☆</button>'
    + '<button class="qg-del">✕</button></span>';
  const whenEl = head.querySelector('.qg-when');
  whenEl.textContent = fmtWhen(g.head) + chainWhenSuffix(g.head, card);
  /* chain heads get their quiet counter refreshed on every poll tick */
  if (g.head.mode === 'chain') whenEl.dataset.quiet = String(quietSecsOf(g.head));
  const n = groupSteps(g).length;
  head.querySelector('.qg-meta').textContent =
    [qMeta(g.head), n > 1 ? t('queue.followups', { count: formatNumber(n - 1) }) : '']
      .filter(Boolean).join(' · ');
  const pb = head.querySelector('.qg-pause');
  if (pb) {
    pb.textContent = g.head.paused ? '▶' : '⏸';
    pb.title = t(g.head.paused ? 'queue.resumeRule' : 'queue.pauseRule');
    pb.onclick = () => inv('queue_pause', { id: g.head.id, paused: !g.head.paused }).catch(() => toast(t('error.operation', { operation: t('queue.meta.paused') })));
  }
  const hr = head.querySelector('.qg-retry');
  if (hr) {
    hr.title = t('queue.retryRule');
    hr.onclick = () => inv('queue_retry', { id: g.head.id }).catch(() => toast(t('error.operation', { operation: t('queue.riskRetry') })));
  }
  head.querySelector('.qg-save').title = t('queue.saveGroup');
  head.querySelector('.qg-del').title = t('queue.removeGroup');
  head.querySelector('.qg-save').onclick = () => saveGroupAsTemplate(g);
  head.querySelector('.qg-del').onclick = () => {
    for (const i of g.rows) inv('queue_remove', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('common.delete') })));
  };
  el.appendChild(head);

  /* rows: real queue items, plus a rule's embedded steps (read-only). Each
     row carries the key its expanded/collapsed state is remembered under —
     the panel is rebuilt on every poll, so the state cannot live in the DOM. */
  const rows = [];
  for (const i of g.rows) {
    rows.push({ text: i.text, item: i, key: i.id });
    if (i.steps) i.steps.forEach((step, n) => rows.push({ text: step, item: null, key: `${i.id}#${n}` }));
  }
  rows.forEach((r, k) => {
    const { first, extra } = promptSummary(r.text);
    const expanded = extra > 0 && expandedRows.has(r.key);
    const row = document.createElement('div');
    row.className = 'qg-row' + (r.item ? '' : ' ro') + (expanded ? ' open' : '');
    row.dataset.qkey = r.key;
    const dead = r.item && itemDead(r.item);
    const ambiguous = r.item && r.item.state === 'ambiguous';
    const contextBlocked = r.item && r.item.last_context && r.item.last_context.status !== 'ready'
      && !ambiguous && r.item.state !== 'firing';
    const manualAllowed = r.item && !ambiguous && r.item.state !== 'firing' && !dead;
    row.innerHTML = '<span class="tree"></span><button class="q-chev"></button>'
      + '<span class="q-text"></span><span class="q-nl"></span><span class="row-meta"></span>'
      + (contextBlocked ? '<button class="q-wait"></button>' : '')
      + (manualAllowed ? '<button class="q-now"></button>' : '')
      + (ambiguous ? '<button class="q-ack"></button><button class="q-risk-retry"></button>' : '')
      + (dead ? '<button class="q-retry">↻</button><button class="q-skip">⏭</button>' : '')
      + (r.item ? '<button class="q-del">✕</button>' : '');
    row.querySelector('.tree').textContent =
      rows.length === 1 || k === 0 ? '' : (k === rows.length - 1 ? '└' : '├');
    /* a multi-line prompt shows its first line and says how many more; the
       chevron is the only thing that opens the rest, so the list stays one
       row per prompt no matter how long the prompts are */
    const chev = row.querySelector('.q-chev');
    const nl = row.querySelector('.q-nl');
    if (extra) {
      chev.textContent = expanded ? '▾' : '▸';
      chev.title = t(expanded ? 'queue.collapsePrompt' : 'queue.expandPrompt');
      chev.onclick = event => {
        event.stopPropagation();
        if (expanded) expandedRows.delete(r.key); else expandedRows.add(r.key);
        renderQueueUI();
      };
      nl.textContent = '⏎' + formatNumber(extra);
      nl.title = t('queue.moreLines', { count: formatNumber(extra) });
    } else {
      /* the chevron column stays, empty: every row's first line keeps the
         same left edge whether or not the prompt has more of them */
      nl.hidden = true;
    }
    const txt = row.querySelector('.q-text');
    txt.textContent = expanded ? r.text : first;
    if (r.item) {
      const i = r.item;
      txt.title = t('common.edit');
      txt.onclick = () => {
        /* edit the WHOLE prompt, so open the row first: a collapsed row
           shows one line and the editor is about to show all of them. The
           re-render replaces this node, so the click continues on the new
           one — one gesture, whatever the row's state was. */
        if (extra && !expanded) {
          expandedRows.add(r.key);
          renderQueueUI();
          const fresh = document.querySelector(`#queue-list .qg-row[data-qkey="${r.key}"] .q-text`);
          if (fresh) fresh.click();
          return;
        }
        /* the row must stop clipping while it holds a growing editor —
           a single-line prompt is edited in an unopened row */
        row.classList.add('editing');
        inlineRename(txt, i.text, v => {
          row.classList.remove('editing');
          if (v && v !== i.text) {
            inv('queue_update', { id: i.id, text: v }).catch(() => toast(t('error.operation', { operation: t('common.edit') })));
          } else {
            setTimeout(renderQueueUI, 0);   // after blur, so the guard won't skip
          }
        }, { multiline: true });
      };
      const del = row.querySelector('.q-del');
      del.title = t('queue.removePrompt');
      del.onclick = () => inv('queue_remove', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('common.delete') })));
      const wait = row.querySelector('.q-wait');
      if (wait) {
        wait.textContent = t('queue.keepWaiting');
        wait.onclick = () => refreshItemProbe(i)
          .then(() => toast(t('queue.waitingContinues')))
          .catch(() => toast(t('queue.context.probeFailed')));
      }
      const now = row.querySelector('.q-now');
      if (now) {
        now.textContent = t('queue.manualNow');
        now.onclick = () => manualSendNow(i);
      }
      const rb = row.querySelector('.q-retry');
      if (rb) { rb.title = t('queue.retryStep'); rb.onclick = () => inv('queue_retry', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('queue.riskRetry') }))); }
      const ack = row.querySelector('.q-ack');
      if (ack) {
        ack.textContent = t('queue.ack'); ack.title = t('queue.ackTitle');
        ack.onclick = () => inv('queue_acknowledge', { id: i.id })
          .catch(() => toast(t('error.operation', { operation: t('queue.ack') })));
      }
      const riskRetry = row.querySelector('.q-risk-retry');
      if (riskRetry) riskRetry.onclick = async () => {
        if (!(await confirmDialog(t('queue.retryAmbiguousConfirm')))) return;
        inv('queue_retry', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('queue.riskRetry') })));
      };
      if (riskRetry) { riskRetry.textContent = t('queue.riskRetry'); riskRetry.title = t('queue.riskRetryTitle'); }
      const sb = row.querySelector('.q-skip');
      if (sb) { sb.title = t('queue.skipStep'); sb.onclick = () => inv('queue_skip', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('queue.skipStep') }))); }
      const bits = [];
      if (i.tpl && i.mode !== 'every') bits.push(`tpl·${i.tpl} ${i.tpl_idx}/${i.tpl_total}`);
      if (i.state === 'ambiguous') {
        bits.push(t('queue.ambiguousDetail'));
      } else if (i.state === 'failed' && i.mode !== 'every') {
        bits.push('⚠ ' + t(itemDead(i) ? 'queue.gaveUp' : 'queue.failedRetrying'));
      } else if (i.last_context && i.last_context.status !== 'ready') {
        bits.push('⏸ ' + contextLabel(i));
      } else if (blockedBy(i, g.rows)) {
        bits.push(t('queue.blocked'));
      }
      row.querySelector('.row-meta').textContent = bits.join(' · ');
    } else {
      txt.title = t('queue.templateStep');
    }
    el.appendChild(row);
  });
  return el;
}

export function renderQueueUI() {
  /* don't clobber an in-progress inline edit of a queued prompt */
  if (document.activeElement && ['INPUT', 'TEXTAREA'].includes(document.activeElement.tagName)
      && document.activeElement.closest('#queue-list')) return;
  const card = state.view === 'session' && provider.get(state.sessionId);
  if (card) {
    const q = sessionQueue(card.session);
    $('queue-cnt').textContent = q.length || '';
    const live = new Set();
    for (const i of q) {
      live.add(i.id);
      (i.steps || []).forEach((_, n) => live.add(`${i.id}#${n}`));
    }
    for (const key of expandedRows) if (!live.has(key)) expandedRows.delete(key);
    if (ctx.queueOpen) {
      const list = $('queue-list');
      list.innerHTML = '';
      for (const g of groupQueue(q)) list.appendChild(groupEl(g, card));
      if (!q.length) {
        const empty = document.createElement('div');
        empty.className = 'q-hint'; empty.textContent = t('queue.empty'); list.appendChild(empty);
      }
    }
  }
  document.querySelectorAll('.card[data-sid]').forEach(el => {
    const c = provider.get(el.dataset.sid);
    setQueueChip(el.querySelector('.q-chip'), c || { session: '' });
  });
}

export function toggleQueuePanel(open) {
  ctx.queueOpen = open !== undefined ? open : !ctx.queueOpen;
  $('queue-panel').style.display = ctx.queueOpen ? 'flex' : 'none';
  if (ctx.queueOpen) {
    const card = provider.get(state.sessionId);
    /* a first prompt is typically "when my quota resets in a few hours";
       anything after it naturally follows the previous one */
    if (card && sessionQueue(card.session).length) {
      segSet('q-mode', 'chain');
    } else {
      segSet('q-mode', 'at');
      applyQuickOffset(300);
    }
    syncForm();
    renderQueueUI();
    $('q-text').focus();
  } else if (ctx.term) {
    ctx.term.focus();
  }
}

/* ---------- the schedule form ---------- */
export const segGet = id => $(id).querySelector('button[aria-pressed="true"]')?.dataset.v || '';
export function segSet(id, v) {
  $(id).querySelectorAll('button').forEach(b => b.setAttribute('aria-pressed', b.dataset.v === v ? 'true' : 'false'));
}
/* which controls the row shows: a control is visible when EVERY facet it
   is tagged with is active, so a sub-control (custom quiet, custom window,
   start/stop dates) needs both its mode and its own switch */
export function activeFacets() {
  const mode = segGet('q-mode');
  const on = new Set([mode]);
  if (mode === 'chain' && $('q-quiet').value === 'custom') on.add('quietc');
  if (mode === 'every') {
    if ($('q-win').value === 'custom') on.add('winc');
    if ($('q-start').value === 'date') on.add('startc');
    if ($('q-until').value === 'date') on.add('untilc');
  }
  return on;
}

export function syncForm() {
  const on = activeFacets();
  document.querySelectorAll('#queue-panel .q-p').forEach(el => {
    const facets = [...el.classList].filter(c => c.startsWith('q-p-')).map(c => c.slice(4));
    el.hidden = !facets.every(f => on.has(f));
  });
}

/* fill date+time with now + `minutes`, rounded up to the next 5 minutes */
export function applyQuickOffset(minutes) {
  const d = new Date(Date.now() + minutes * 60000);
  d.setMinutes(Math.ceil(d.getMinutes() / 5) * 5, 0, 0);
  $('q-date').value = isoDate(d);
  $('q-time').value = isoTime(d);
}

/* option lists that carry numbers go through the locale's formatter, so
   they are rebuilt on a language change instead of being static HTML */
export function fillFormOptions() {
  const fill = (sel, entries, initial) => {
    const keep = sel.value || initial;
    sel.innerHTML = '';
    for (const [value, label] of entries) {
      const o = document.createElement('option');
      o.value = value; o.textContent = label;
      sel.appendChild(o);
    }
    sel.value = keep;
    if (sel.selectedIndex < 0) sel.selectedIndex = 0;
  };
  fill($('q-quiet'), [
    ...[30, 60, 180, 600, 1800].map(secs => [String(secs), formatInterval(secs)]),
    ['custom', t('queue.quiet.custom')],
  ], '180');
  fill($('q-every'), [5, 15, 30, 60, 120].map(min => [String(min), formatInterval(min * 60)]), '30');
}

/* the form → QueueAddArgs schedule fields; null plus a toast when a field
   the chosen mode needs is missing or contradictory */
export function readSchedule() {
  const now = Math.floor(Date.now() / 1000);
  const mode = segGet('q-mode');
  const out = { mode, at: null, quietSecs: null, every: null, notBefore: null,
    winFrom: null, winTo: null, untilN: null, untilAt: null };
  const fail = (key, focus) => { toast(t(key)); if (focus) $(focus).focus(); return null; };
  if (mode === 'chain') {
    const q = $('q-quiet').value;
    out.quietSecs = q === 'custom'
      ? Math.round(Number($('q-quiet-n').value) * Number($('q-quiet-u').value))
      : Number(q);
    if (!(out.quietSecs >= MIN_QUIET_SECS && out.quietSecs <= MAX_QUIET_SECS)) return fail('queue.badQuiet', 'q-quiet-n');
    return out;
  }
  if (mode === 'at') {
    out.at = localEpoch($('q-date').value, $('q-time').value);
    if (out.at == null) return fail('queue.setTime', $('q-date').value ? 'q-time' : 'q-date');
    if (out.at <= now) return fail('queue.pastTime', 'q-time');
    return out;
  }
  out.every = Number($('q-every').value) * 60;
  const wv = $('q-win').value;
  if (wv === 'custom') {
    const a = $('q-win-a').value, b = $('q-win-b').value;
    if (!a || !b) return fail('queue.setWindow', a ? 'q-win-b' : 'q-win-a');
    out.winFrom = hmToMin(a);
    out.winTo = hmToMin(b);
  } else if (wv) {
    [out.winFrom, out.winTo] = wv.split('-').map(Number);
  }
  if ($('q-start').value === 'date') {
    out.notBefore = localEpoch($('q-start-d').value, $('q-start-t').value);
    if (out.notBefore == null) return fail('queue.setStart', $('q-start-d').value ? 'q-start-t' : 'q-start-d');
  }
  const uv = $('q-until').value;
  if (uv.startsWith('n')) {
    out.untilN = Number(uv.slice(1));
  } else if (uv === 'date') {
    out.untilAt = localEpoch($('q-until-d').value, $('q-until-t').value);
    if (out.untilAt == null) return fail('queue.setStop', $('q-until-d').value ? 'q-until-t' : 'q-until-d');
    if (out.untilAt <= Math.max(now, out.notBefore || 0)) return fail('queue.stopBeforeStart', 'q-until-t');
  }
  return out;
}

/* templates live on the project object → persisted inside the board file */
export function projTemplates(card) {
  const p = card && provider.project(card.projectId);
  if (!p) return [];
  return p.templates || [];
}

export function setQSrc(tpl) {
  ctx.qTpl = tpl;
  const btn = $('q-src'), inp = $('q-text');
  if (tpl) {
    btn.textContent = '📋';
    btn.classList.add('tpl');
    inp.value = `${tpl.name}  (${t('queue.steps', { count: formatNumber(tpl.steps.length) })})`;
    inp.readOnly = true;
    $('q-add-btn').textContent = t('common.addCount', { count: formatNumber(tpl.steps.length) });
  } else {
    btn.textContent = '✎';
    btn.classList.remove('tpl');
    if (inp.readOnly) inp.value = '';
    inp.readOnly = false;
    $('q-add-btn').textContent = t('common.add');
  }
  autoGrowField(inp);
}

export function hideTplPop() { $('tpl-pop').style.display = 'none'; }

export function showTplPop() {
  const card = provider.get(state.sessionId);
  if (!card) return;
  const pop = $('tpl-pop');
  const tpls = projTemplates(card);
  pop.innerHTML = '';
  const add = (cls, html) => {
    const el = document.createElement('div');
    el.className = cls;
    if (html !== undefined) el.innerHTML = html;
    pop.appendChild(el);
    return el;
  };
  const proj = provider.project(card.projectId);
  add('t-head').textContent = t('queue.templatesProject', { project: (proj && proj.name) || '' });
  if (ctx.qTpl) {
    const r = add('t-row', '<span class="t-name"></span>');
    r.querySelector('.t-name').textContent = t('queue.typePrompt');
    r.onclick = () => { setQSrc(null); hideTplPop(); $('q-text').focus(); };
  }
  if (!tpls.length) {
    const r = add('t-row', '<span class="t-name" style="color:var(--faint)"></span>');
    r.querySelector('.t-name').textContent = t('queue.noTemplates');
  }
  for (const template of tpls) {
    const r = add('t-row', '<span class="t-name"></span><span class="t-n"></span><button class="t-act">✎</button><button class="t-act t-del">✕</button>');
    r.querySelector('.t-name').textContent = template.name;
    r.querySelector('.t-name').title = promptTooltip(template.steps.join('\n'));
    r.querySelector('.t-n').textContent = t('queue.steps', { count: formatNumber(template.steps.length) });
    r.querySelector('.t-act').title = t('common.rename');
    r.querySelector('.t-del').title = t('common.delete');
    r.querySelector('.t-name').onclick = () => { setQSrc(template); hideTplPop(); };
    r.querySelector('.t-act').onclick = async e => {
      e.stopPropagation();
      const name = await promptDialog(t('queue.renameTemplate'), template.name);
      if (name && name !== template.name) {
        const oldName = template.name;
        await provider.renameTemplate(card.projectId, oldName, name);
        if (ctx.qTpl === template) setQSrc({ ...template, name });
      }
    };
    r.querySelector('.t-del').onclick = async e => {
      e.stopPropagation();
      if (!(await confirmDialog(t('queue.deleteTemplate', { name: template.name })))) return;
      await provider.deleteTemplate(card.projectId, template.name);
      if (ctx.qTpl === template) setQSrc(null);
      hideTplPop();
    };
  }
  add('t-sep');
  const hint = add('t-row', '<span class="t-name" style="color:var(--faint)"></span>');
  hint.querySelector('.t-name').textContent = t('queue.saveTemplateHint');
  const a = $('q-src');
  pop.style.left = a.offsetLeft + 'px';
  pop.style.top = (a.offsetTop + a.offsetHeight + 6) + 'px';
  pop.style.display = 'block';
}

/* native menu: Terminal → Clear (⌘K) */

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initScheduler() {
  $('queue-btn').onclick = () => toggleQueuePanel();

  fillFormOptions();
  onLocaleChange(fillFormOptions);
  $('q-mode').querySelectorAll('button').forEach(b => {
    b.onclick = () => { segSet('q-mode', b.dataset.v); syncForm(); };
  });
  for (const id of ['q-quiet', 'q-win', 'q-start', 'q-until']) $(id).addEventListener('change', syncForm);
  /* the quick list is a verb, not a value: it fills the date and time and
     shows its placeholder again */
  $('q-quick').addEventListener('change', () => {
    if ($('q-quick').value) applyQuickOffset(Number($('q-quick').value));
    $('q-quick').value = '';
  });

  $('q-src').onclick = e => {
    e.stopPropagation();
    if ($('tpl-pop').style.display === 'block') hideTplPop(); else showTplPop();
  };

  document.addEventListener('click', e => {
    if (!e.target.closest('#tpl-pop') && !e.target.closest('#q-src')) hideTplPop();
  });

  $('q-add-btn').onclick = async () => {
    const card = provider.get(state.sessionId);
    if (!card) return;
    const sched = readSchedule();
    if (!sched) return;
    const base = {
      session: card.session, cardId: card.id, dir: card.dir, cmd: card.cmd,
    };
    try {
      if (ctx.qTpl) {
        const steps = ctx.qTpl.steps.slice();
        if (sched.mode === 'every') {
          /* one standing rule holds the whole template; steps 2..N re-enqueue
             as chain items on every fire */
          await inv('queue_add', { args: { ...base, ...sched, text: steps[0],
            steps: steps.slice(1), tpl: ctx.qTpl.name, tplIdx: 1, tplTotal: steps.length } });
        } else {
          for (let k = 0; k < steps.length; k++) {
            const follow = { mode: 'chain', quietSecs: sched.mode === 'chain' ? sched.quietSecs : null };
            await inv('queue_add', { args: { ...base, ...(k === 0 ? sched : follow), text: steps[k],
              tpl: ctx.qTpl.name, tplIdx: k + 1, tplTotal: steps.length } });
          }
        }
        setQSrc(null);
      } else {
        const text = $('q-text').value.trim();
        if (!text) { $('q-text').focus(); return; }
        await inv('queue_add', { args: { ...base, ...sched, text } });
        $('q-text').value = '';
        autoGrowField($('q-text'));
      }
      segSet('q-mode', 'chain');   // natural default for the next one
      syncForm();
    } catch (e) {
      toast(t('error.operation', { operation: t('common.add') }));
    }
  };

  $('q-text').addEventListener('input', () => autoGrowField($('q-text')));

  /* a prompt can be many lines, so Enter types one; ⌘↵ is what queues it */
  $('q-text').addEventListener('keydown', e => {
    if (e.key !== 'Enter') return;
    if (e.isComposing || e.keyCode === 229) return;   // IME commit, not submit
    if (!(e.metaKey || e.ctrlKey)) return;
    e.preventDefault();
    $('q-add-btn').click();
  });

  listen('menu-clear', () => {
    if (state.view === 'session' && ctx.term) {
      ctx.term.clear();
      if (ctx.attachedName) inv('pty_write', { name: ctx.attachedName, dataB64: strToB64('\x0c') }).catch(() => {});
    }
  }).catch(() => uev('listen-fail', 'menu-clear'));

  listen('queue-changed', refreshQueue).catch(() => uev('listen-fail', 'queue-changed'));

  listen('queue-fired', ev => {
    toast(t('queue.sent', { session: ev.payload.session }));
    pollNow();
  }).catch(() => uev('listen-fail', 'queue-fired'));
}
