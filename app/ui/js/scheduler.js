// scheduler.js — the ⏱ panel: a card's LISTS of prompts to send later
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// C v01: a list optionally leaves a durable human checkpoint after EVERY
// delivery, including the last. queue-review.js renders backend selection
// observations and separate delivery/inspection records. Never use agent
// state or viewed markers to release a checkpoint; only its revision-bound
// confirmation does. Reviewed templates enter as one backend transaction.
// Plans are read-only snapshots: an open panel re-fetches them on the poll
// tick (refreshQueuePlans) so a waiting list is not shown as "unknown".
// The panel shows one LIST per queue group (an "at" head and the chain rows
// behind it) or per standing "every" rule (its embedded rows). A list's head
// carries the two optional fields the form offers — NOT BEFORE (the head's
// instant / a rule's start; never rolled to "tomorrow", a past instant is
// refused) and REPEAT (a minute cadence with an optional daily window and a
// stop: never / N times / an instant). A ROW carries only "once quiet for
// X": a row of a one-shot list is a chain item that names its list
// (`QueueAddArgs.group`, `listKey` in pure.js); a row of a repeating list
// is one of the rule's embedded steps, edited wholesale through
// `queue_update { steps }` and re-enqueued on every fire with the default
// quiet time. There is no mode picker: `listScheduleArgs` (pure.js) turns
// the form into an "at" head or an "every" rule and nothing else.
//
// A prompt may be MANY LINES (`normalizeTemplateStep`/`normalize_prompt`
// keep them; only a CR is folded), so the panel stays ONE ROW PER PROMPT: a
// row shows its first line plus a `⏎N` badge and only its chevron opens the
// rest. Which rows are open (`expandedRows`) and what is typed into a list's
// footer (`drafts`) live here because the panel is rebuilt from scratch on
// every poll; both are pruned against the live queue on each render. Editing
// edits the WHOLE prompt: a collapsed row opens first and the same click
// continues into the editor, where Enter types a newline and ⌘↵ commits.
//
// What an item READS as (its time, meta line, context label, a row's quiet
// hint) and which backend calls start a list live DOM-free in
// scheduler-model.js, where node tests pin them; this module only places
// those words in the panel.
//
// Templates are the project's saved lists (`templates.js` owns them): the
// panel's 📋 starts a new list from one, a list head's 📋 inserts one into
// that list or saves the list as a new template, and either menu ends with
// "Manage templates…" — the project manager opens from where templates are
// used, not from a standing Board button. Inserting is a copy — a
// template changed later leaves the rows alone. Calendar cadences (daily /
// weekly / monthly) are deliberately not a card schedule — see the
// Board-level automation note in scheduler/mod.rs.
import { isReview, reviewRow, executionPlan, stageText, queueHistory, cancelQueueList } from './queue-review.js';
import { chainWhenSuffix, contextLabel, fmtWhen, listStartCalls, localizedChainQuietHint, qMeta } from './scheduler-model.js';
import { $, ctx, inv, listen, state, uev } from './state.js';
import { blockedBy, chainQuietHint, CHAIN_QUIET_SECS, contextStatusKey, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, isoDate, isoTime, itemDead, listKey, listRepeats, listScheduleArgs, localEpoch, MAX_QUIET_SECS, MIN_QUIET_SECS, minToHM, nextFire, promptSummary, promptTooltip, quietSecsOf, winHas } from './pure.js';
export { blockedBy, chainQuietHint, contextStatusKey, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, itemDead, minToHM, nextFire, promptSummary, promptTooltip, winHas };
import { autoGrowField, confirmDangerDialog, confirmDialog, inlineRename, toast, promptDialog } from './dialogs.js';
import { pollNow, provider } from './board.js';
import { strToB64 } from './layout.js';
import { openTemplates } from './templates.js';
import { formatInterval, formatNumber, onLocaleChange, t } from './i18n.js';

/* ---------- scheduled prompts ---------- */
let queueFetchedAt = 0;
export async function refreshQueue() {
  try { ctx.queueCache = await inv('queue_list'); queueFetchedAt = Date.now(); } catch (e) { ctx.queueCache.plans = []; }
  renderQueueUI();
}
/* the backend emits queue-changed only on mutations, and a list that is
   merely waiting (gap, quiet, not-before) mutates nothing — so an open panel
   re-fetches its read-only plans every 15 s, well inside the 40 s after which
   stageText reports them as stale. Closed panels never fetch. */
const PLAN_REFRESH_MS = 15_000;
export function refreshQueuePlans() {
  if (!ctx.queueOpen || state.view !== 'session') return;
  if (Date.now() - queueFetchedAt < PLAN_REFRESH_MS) return;
  queueFetchedAt = Date.now();
  refreshQueue();
}

export const sessionQueue = session => ctx.queueCache.items.filter(i => i.session === session);

/* Which multi-line prompts are opened to their full text, and what is typed
   into a list's footer but not yet added. The panel is rebuilt from scratch
   on every poll, so neither can live in the DOM; both are pruned against
   the live queue on each render so removed lists do not leak keys. */
const expandedRows = new Set();
const drafts = new Map();   // list key → { text, quiet }

export function setQueueChip(chip, card) {
  if (!chip) return;
  const q = sessionQueue(card.session);
  chip.textContent = q.length ? '⏰' + q.length : '';
  chip.title = q.length
    ? t('queue.next', { when: fmtWhen(q[0]), prompt: promptTooltip(q[0].text) })
    : '';
}

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

export async function saveListAsTemplate(g) {
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

/* ---------- rows ---------- */
const fail = (key, focus) => { toast(t(key)); if (focus) focus.focus(); return null; };

/* the "once quiet for" control a list footer carries: a select of common
   waits plus a custom number+unit pair; `read()` is the seconds or null
   (with a toast) when the custom pair is out of range */
function quietControl(initial = String(CHAIN_QUIET_SECS)) {
  const el = document.createElement('span');
  el.className = 'qg-quiet';
  const label = document.createElement('span');
  label.className = 'lbl';
  label.textContent = t('queue.quietFor');
  const sel = document.createElement('select');
  for (const [value, text] of [
    ...[30, 60, 180, 600, 1800, 3600].map(secs => [String(secs), formatInterval(secs)]),
    ['custom', t('queue.quiet.custom')],
  ]) {
    const o = document.createElement('option');
    o.value = value; o.textContent = text;
    sel.appendChild(o);
  }
  sel.value = initial;
  if (sel.selectedIndex < 0) sel.value = 'custom';
  const n = document.createElement('input');
  n.type = 'number'; n.min = '1'; n.step = '1'; n.value = '3';
  const unit = document.createElement('select');
  for (const [value, key] of [['1', 'queue.unit.s'], ['60', 'queue.unit.min'], ['3600', 'queue.unit.h']]) {
    const o = document.createElement('option');
    o.value = value; o.textContent = t(key);
    unit.appendChild(o);
  }
  unit.value = '60';
  if (sel.value === 'custom' && /^\d+$/.test(initial)) {
    const secs = Number(initial);
    const u = secs % 3600 === 0 ? 3600 : secs % 60 === 0 ? 60 : 1;
    unit.value = String(u); n.value = String(secs / u);
  }
  const sync = () => { const custom = sel.value === 'custom'; n.hidden = !custom; unit.hidden = !custom; };
  sel.addEventListener('change', sync);
  sync();
  el.append(label, sel, n, unit);
  return {
    el,
    value: () => (sel.value === 'custom' ? `${n.value}*${unit.value}` : sel.value),
    read() {
      const secs = sel.value === 'custom' ? Math.round(Number(n.value) * Number(unit.value)) : Number(sel.value);
      if (!(secs >= MIN_QUIET_SECS && secs <= MAX_QUIET_SECS)) return fail('queue.badQuiet', n);
      return secs;
    },
  };
}

/* the rows a list shows: real queue items, and for a repeating list the
   rule's embedded steps (edited through the rule). Each row carries the key
   its expanded/collapsed state is remembered under. */
function listRows(g) {
  const rows = [];
  for (const i of g.rows) {
    rows.push({ text: i.text, item: i, key: i.id });
    if (i.steps) i.steps.forEach((step, n) => rows.push({ text: step, item: null, rule: i, step: n, key: `${i.id}#${n}` }));
  }
  return rows;
}

const withSteps = (rule, steps, operation) =>
  inv('queue_update', { id: rule.id, steps }).catch(() => toast(t('error.operation', { operation })));

function rowEl(r, g, rows, k) {
  if (isReview(r.item)) return reviewRow(r.item, refreshQueue, () => toggleQueuePanel(false));
  const { first, extra } = promptSummary(r.text);
  const expanded = extra > 0 && expandedRows.has(r.key);
  const row = document.createElement('div');
  row.className = 'qg-row' + (expanded ? ' open' : '');
  row.dataset.qkey = r.key;
  const i = r.item;
  const dead = i && itemDead(i);
  const ambiguous = i && i.state === 'ambiguous';
  const contextBlocked = i && i.last_context && i.last_context.status !== 'ready'
    && !ambiguous && i.state !== 'firing';
  const manualAllowed = i && !ambiguous && i.state !== 'firing' && !dead
    && !g.rows.some(p => isReview(p) && p.state === 'review' && p.seq < i.seq);
  row.innerHTML = '<span class="tree"></span><button class="q-chev"></button>'
    + '<span class="q-text"></span><span class="q-nl"></span><span class="row-meta"></span>'
    + (contextBlocked ? '<button class="q-wait"></button>' : '')
    + (manualAllowed ? '<button class="q-now"></button>' : '')
    + (ambiguous ? '<button class="q-ack"></button><button class="q-risk-retry"></button>' : '')
    + (dead ? '<button class="q-retry">↻</button><button class="q-skip">⏭</button>' : '')
    + '<button class="q-del">✕</button>';
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
  row.querySelectorAll('button').forEach((b, n) => { b.dataset.queueFocus = `${r.key}:${n}`; });
  const txt = row.querySelector('.q-text');
  txt.textContent = expanded ? r.text : first;
  txt.title = t('common.edit');
  txt.onclick = () => {
    /* edit the WHOLE prompt, so open the row first: a collapsed row shows
       one line and the editor is about to show all of them. The re-render
       replaces this node, so the click continues on the new one — one
       gesture, whatever the row's state was. */
    if (extra && !expanded) {
      expandedRows.add(r.key);
      renderQueueUI();
      const fresh = document.querySelector(`#queue-list .qg-row[data-qkey="${r.key}"] .q-text`);
      if (fresh) fresh.click();
      return;
    }
    /* the row must stop clipping while it holds a growing editor — a
       single-line prompt is edited in an unopened row */
    row.classList.add('editing');
    inlineRename(txt, r.text, v => {
      row.classList.remove('editing');
      if (v && v !== r.text) {
        if (i) {
          inv('queue_update', { id: i.id, text: v }).catch(() => toast(t('error.operation', { operation: t('common.edit') })));
        } else {
          withSteps(r.rule, r.rule.steps.map((s, n) => (n === r.step ? v : s)), t('common.edit'));
        }
      } else {
        setTimeout(renderQueueUI, 0);   // after blur, so the guard won't skip
      }
    }, { multiline: true });
  };
  const del = row.querySelector('.q-del');
  del.title = t('queue.removePrompt');
  del.onclick = async () => {
    if (g.rows.some(item => item.review_each) && !await confirmDialog(t('queue.review.skipConfirm'))) return;
    if (i) await inv('queue_remove', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('common.delete') })));
    else await withSteps(r.rule, r.rule.steps.filter((_, n) => n !== r.step), t('common.delete'));
  };
  if (!i) {
    row.querySelector('.row-meta').textContent = t('queue.repeatRow', { quiet: formatInterval(CHAIN_QUIET_SECS) });
    return row;
  }
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
  if (riskRetry) {
    riskRetry.textContent = t('queue.riskRetry'); riskRetry.title = t('queue.riskRetryTitle');
    riskRetry.onclick = async () => {
      if (!(await confirmDialog(t('queue.retryAmbiguousConfirm')))) return;
      inv('queue_retry', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('queue.riskRetry') })));
    };
  }
  const sb = row.querySelector('.q-skip');
  if (sb) {
    sb.title = t('queue.skipStep');
    sb.onclick = async () => {
      if (i.review_each && !await confirmDialog(t('queue.review.skipConfirm'))) return;
      await inv('queue_skip', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('queue.skipStep') })));
    };
  }
  const bits = [stageText(i)];
  if (i.tpl && i.mode !== 'every') bits.push(`tpl·${i.tpl} ${i.tpl_idx}/${i.tpl_total}`);
  if (i.mode === 'chain') bits.push(t('queue.rowQuiet', { quiet: formatInterval(quietSecsOf(i)) }));
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
  return row;
}

/* ---------- lists ---------- */
const queueBase = card => ({ session: card.session, cardId: card.id, dir: card.dir, cmd: card.cmd });

/* add rows to an existing list: chain items that name the list, or (for a
   repeating list) more embedded steps */
async function appendRows(g, card, texts, quietSecs) {
  if (listRepeats(g)) {
    await withSteps(g.head, [...g.head.steps, ...texts], t('common.add'));
    return;
  }
  for (const text of texts) {
    await inv('queue_add', { args: { ...queueBase(card), mode: 'chain', quietSecs, group: listKey(g), text } });
  }
}

function listFooter(g, card) {
  const key = listKey(g);
  const draft = drafts.get(key) || { text: '', quiet: String(CHAIN_QUIET_SECS) };
  const foot = document.createElement('div');
  foot.className = 'qg-add';
  const field = document.createElement('textarea');
  field.rows = 1; field.autocomplete = 'off'; field.spellcheck = false;
  field.placeholder = t('queue.rowPlaceholder');
  field.value = draft.text;
  const quiet = listRepeats(g) ? null : quietControl(draft.quiet);
  const add = document.createElement('button');
  add.className = 'btn';
  add.textContent = t('common.add');
  const remember = () => drafts.set(key, { text: field.value, quiet: quiet ? quiet.value() : draft.quiet });
  field.addEventListener('input', () => { autoGrowField(field); remember(); });
  if (quiet) quiet.el.addEventListener('change', remember);
  const submit = async () => {
    const text = field.value.trim();
    if (!text) { field.focus(); return; }
    const secs = quiet ? quiet.read() : null;
    if (quiet && secs == null) return;
    try {
      await appendRows(g, card, [text], secs);
      drafts.delete(key);
      field.value = '';
    } catch (e) {
      toast(t('error.operation', { operation: t('common.add') }));
    }
  };
  add.onclick = submit;
  field.addEventListener('keydown', e => {
    if (e.key !== 'Enter' || e.isComposing || e.keyCode === 229 || !(e.metaKey || e.ctrlKey)) return;
    e.preventDefault();
    submit();
  });
  foot.appendChild(field);
  if (quiet) foot.appendChild(quiet.el);
  foot.appendChild(add);
  setTimeout(() => autoGrowField(field), 0);
  return { foot, quiet };
}

export function groupEl(g, card, otherLists = 0) {
  const rule = listRepeats(g);
  const el = document.createElement('div');
  el.className = 'q-group' + (rule ? ' rule' : '') + (g.head.paused ? ' paused' : '');

  const head = document.createElement('div');
  head.className = 'qg-head';
  head.innerHTML = '<span class="qg-when"></span><span class="qg-meta"></span><span class="qg-act">'
    + (rule ? '<button class="qg-pause"></button>' : '')
    + (rule && itemDead(g.head) ? '<button class="qg-retry">↻</button>' : '')
    + '<button class="qg-tpl">📋</button>'
    + '<button class="qg-del">✕</button></span>';
  const whenEl = head.querySelector('.qg-when');
  whenEl.textContent = isReview(g.head) ? t('queue.review.checkpoint') : fmtWhen(g.head) + chainWhenSuffix(g.head, card);
  /* chain heads get their quiet counter refreshed on every poll tick */
  if (!isReview(g.head) && g.head.mode === 'chain') whenEl.dataset.quiet = String(quietSecsOf(g.head));
  const n = groupSteps(g).length;
  head.querySelector('.qg-meta').textContent =
    [qMeta(g.head), n > 1 ? t('queue.followups', { count: formatNumber(n - 1) }) : '']
      .filter(Boolean).join(' · ');
  const pb = head.querySelector('.qg-pause');
  if (pb) {
    pb.textContent = g.head.paused ? '▶' : '⏸';
    pb.title = t(g.head.paused ? 'queue.resumeList' : 'queue.pauseList');
    pb.onclick = () => inv('queue_pause', { id: g.head.id, paused: !g.head.paused }).catch(() => toast(t('error.operation', { operation: t('queue.meta.paused') })));
  }
  const hr = head.querySelector('.qg-retry');
  if (hr) {
    hr.title = t('queue.retryList');
    hr.onclick = () => inv('queue_retry', { id: g.head.id }).catch(() => toast(t('error.operation', { operation: t('queue.riskRetry') })));
  }
  const { foot, quiet } = listFooter(g, card);
  const tplBtn = head.querySelector('.qg-tpl');
  tplBtn.title = t('queue.listMenu');
  tplBtn.onclick = e => {
    e.stopPropagation();
    showTplPop(tplBtn, {
      insert: async template => {
        const secs = quiet ? quiet.read() : null;
        if (quiet && secs == null) return;
        try { await appendRows(g, card, template.steps.slice(), secs); }
        catch (err) { toast(t('error.operation', { operation: t('common.add') })); }
      },
      save: () => saveListAsTemplate(g),
    });
  };
  head.querySelector('.qg-del').title = t('queue.removeList');
  /* an ordinary list is removed at once, exactly as before C v01; only a
     list with checkpoints (sent rows it must not resend or mark inspected)
     goes through the explicit cancel confirmation */
  head.querySelector('.qg-del').onclick = () => {
    if (g.rows.some(i => i.review_each || isReview(i))) {
      cancelQueueList(g.head, refreshQueue).catch(() => toast(t('queue.review.failed')));
      return;
    }
    for (const i of g.rows) inv('queue_remove', { id: i.id }).catch(() => toast(t('error.operation', { operation: t('common.delete') })));
  };
  el.appendChild(head);
  el.appendChild(executionPlan(g, card, otherLists, refreshQueue));
  const rows = listRows(g);
  rows.forEach((r, k) => el.appendChild(rowEl(r, g, rows, k)));
  el.appendChild(foot);
  return el;
}

export function renderQueueUI() {
  /* don't clobber an in-progress inline edit or a footer being typed into */
  if (document.activeElement && ['INPUT', 'TEXTAREA'].includes(document.activeElement.tagName)
      && document.activeElement.closest('#queue-list')) return;
  const card = state.view === 'session' && provider.get(state.sessionId);
  if (card) {
    const q = sessionQueue(card.session);
    $('queue-cnt').textContent = q.length || '';
    const live = new Set();
    const lists = groupQueue(q);
    for (const i of q) {
      live.add(i.id);
      (i.steps || []).forEach((_, n) => live.add(`${i.id}#${n}`));
    }
    for (const key of expandedRows) if (!live.has(key)) expandedRows.delete(key);
    const liveLists = new Set(lists.map(listKey));
    for (const key of drafts.keys()) if (!liveLists.has(key)) drafts.delete(key);
    if (ctx.queueOpen) {
      const list = $('queue-list');
      const focus = document.activeElement?.dataset.queueFocus;
      list.innerHTML = '';
      for (const g of lists) list.appendChild(groupEl(g, card, lists.length - 1));
      list.appendChild(queueHistory(card));
      if (focus) {
        const target = [...list.querySelectorAll('[data-queue-focus]')].find(el => el.dataset.queueFocus === focus);
        (target || list).focus();
      }
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
    resetListForm();
    renderQueueUI();
    $('q-text').focus();
  } else if (ctx.term) {
    ctx.term.focus();
  }
}

/* ---------- the new-list form ---------- */
/* which controls the form shows: a control is visible when EVERY facet it
   is tagged with is active — a sub-control (the start date, the custom
   window, the stop instant) needs both its parent choice and its own */
export function activeFacets() {
  const on = new Set();
  if ($('q-start').value === 'date') on.add('startc');
  if ($('q-every').value) {
    on.add('every');
    if ($('q-win').value === 'custom') on.add('winc');
    if ($('q-until').value === 'date') on.add('untilc');
  }
  return on;
}

export function syncForm() {
  const on = activeFacets();
  document.querySelectorAll('#q-new .q-p').forEach(el => {
    const facets = [...el.classList].filter(c => c.startsWith('q-p-')).map(c => c.slice(4));
    el.hidden = !facets.every(f => on.has(f));
  });
}

/* a fresh form: a one-shot list starting now */
export function resetListForm() {
  $('q-start').value = '';
  $('q-review').checked = false;
  $('q-every').value = '';
  $('q-win').value = '';
  $('q-until').value = '';
  syncForm();
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
  const sel = $('q-every');
  const keep = sel.value;
  sel.innerHTML = '';
  for (const [value, label] of [
    ['', t('queue.repeat.none')],
    ...[5, 15, 30, 60, 120, 240].map(min => [String(min), t('queue.repeat.every', { interval: formatInterval(min * 60) })]),
  ]) {
    const o = document.createElement('option');
    o.value = value; o.textContent = label;
    sel.appendChild(o);
  }
  sel.value = keep;
  if (sel.selectedIndex < 0) sel.selectedIndex = 0;
}

/* the form → the schedule half of QueueAddArgs; null plus a toast when a
   field the form needs is missing or contradictory */
export function readSchedule() {
  const now = Math.floor(Date.now() / 1000);
  const form = { notBefore: null, every: null, winFrom: null, winTo: null, untilN: null, untilAt: null };
  if ($('q-start').value === 'date') {
    form.notBefore = localEpoch($('q-date').value, $('q-time').value);
    if (form.notBefore == null) return fail('queue.setTime', $($('q-date').value ? 'q-time' : 'q-date'));
  }
  if ($('q-every').value) {
    form.every = Number($('q-every').value) * 60;
    const wv = $('q-win').value;
    if (wv === 'custom') {
      const a = $('q-win-a').value, b = $('q-win-b').value;
      if (!a || !b) return fail('queue.setWindow', $(a ? 'q-win-b' : 'q-win-a'));
      form.winFrom = hmToMin(a);
      form.winTo = hmToMin(b);
    } else if (wv) {
      [form.winFrom, form.winTo] = wv.split('-').map(Number);
    }
    const uv = $('q-until').value;
    if (uv.startsWith('n')) {
      form.untilN = Number(uv.slice(1));
    } else if (uv === 'date') {
      form.untilAt = localEpoch($('q-until-d').value, $('q-until-t').value);
      if (form.untilAt == null) return fail('queue.setStop', $($('q-until-d').value ? 'q-until-t' : 'q-until-d'));
    }
  }
  const read = listScheduleArgs(form, now);
  if (read.error) return fail(read.error, $(read.focus));
  return { ...read.args, reviewEach: $('q-review').checked };
}

/* start a new list from `steps`: a one-shot list is an "at" head plus chain
   rows that join it (the head is the newest group, so no name is needed); a
   repeating list is one rule holding the whole template — steps 2..N
   re-enqueue as chain items on every fire */
async function startList(card, sched, steps, tpl) {
  const base = { ...queueBase(card), reviewEach: sched.reviewEach === true };
  for (const [command, payload] of listStartCalls(base, sched, steps, tpl)) await inv(command, payload);
}

/* templates live on the project object → persisted inside the board file */
export function projTemplates(card) {
  const p = card && provider.project(card.projectId);
  if (!p) return [];
  return p.templates || [];
}

export function hideTplPop() { $('tpl-pop').style.display = 'none'; }

/* the 📋 menu: the project's templates to insert, and (on a list head) the
   way to save that list as one. `anchor` is the button it opens under. */
export function showTplPop(anchor, { insert, save = null }) {
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
  add('t-head').textContent = t(save ? 'queue.insertTemplate' : 'queue.startFromTemplate', { project: (proj && proj.name) || '' });
  if (!tpls.length) {
    const r = add('t-row', '<span class="t-name" style="color:var(--faint)"></span>');
    r.querySelector('.t-name').textContent = t('queue.noTemplates');
  }
  for (const template of tpls) {
    const r = add('t-row', '<span class="t-name"></span><span class="t-n"></span>');
    r.querySelector('.t-name').textContent = template.name;
    r.querySelector('.t-name').title = promptTooltip(template.steps.join('\n'));
    r.querySelector('.t-n').textContent = t('queue.steps', { count: formatNumber(template.steps.length) });
    r.onclick = () => { hideTplPop(); insert(template); };
  }
  if (save) {
    add('t-sep');
    const r = add('t-row', '<span class="t-name"></span>');
    r.querySelector('.t-name').textContent = t('queue.saveList');
    r.onclick = () => { hideTplPop(); save(); };
  }
  add('t-sep');
  const manage = add('t-row', '<span class="t-name"></span>');
  manage.querySelector('.t-name').textContent = '◈ ' + t('queue.manageTemplates');
  manage.onclick = () => { hideTplPop(); openTemplates(anchor); };
  const panel = $('queue-panel');
  const a = anchor.getBoundingClientRect(), p = panel.getBoundingClientRect();
  pop.style.left = Math.max(0, a.left - p.left) + 'px';
  pop.style.top = (a.bottom - p.top + panel.scrollTop + 6) + 'px';
  pop.style.display = 'block';
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initScheduler() {
  $('queue-btn').onclick = () => toggleQueuePanel();

  fillFormOptions();
  onLocaleChange(() => { fillFormOptions(); renderQueueUI(); });
  for (const id of ['q-start', 'q-every', 'q-win', 'q-until']) $(id).addEventListener('change', syncForm);
  $('q-start').addEventListener('change', () => {
    if ($('q-start').value === 'date' && !$('q-date').value) applyQuickOffset(300);
  });
  /* the quick list is a verb, not a value: it fills the date and time and
     shows its placeholder again */
  $('q-quick').addEventListener('change', () => {
    if ($('q-quick').value) applyQuickOffset(Number($('q-quick').value));
    $('q-quick').value = '';
  });

  $('q-tpl').onclick = e => {
    e.stopPropagation();
    if ($('tpl-pop').style.display === 'block') { hideTplPop(); return; }
    showTplPop($('q-tpl'), {
      insert: async template => {
        const card = provider.get(state.sessionId);
        const sched = card && readSchedule();
        if (!sched) return;
        try {
          await startList(card, sched, template.steps.slice(), template);
          resetListForm();
        } catch (err) {
          toast(t('error.operation', { operation: t('common.add') }));
        }
      },
    });
  };

  document.addEventListener('click', e => {
    if (!e.target.closest('#tpl-pop') && !e.target.closest('#q-tpl') && !e.target.closest('.qg-tpl')) hideTplPop();
  });

  $('q-add-btn').onclick = async () => {
    const card = provider.get(state.sessionId);
    if (!card) return;
    const text = $('q-text').value.trim();
    if (!text) { $('q-text').focus(); return; }
    const sched = readSchedule();
    if (!sched) return;
    try {
      await startList(card, sched, [text], null);
      $('q-text').value = '';
      autoGrowField($('q-text'));
      resetListForm();
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
