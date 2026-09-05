// scheduler.js — scheduled prompts: queue groups, recurring rules, templates
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, inv, listen, state, uev } from './state.js';
import { blockedBy, chainQuietHint, contextStatusKey, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, itemDead, minToHM, nextFire, winHas } from './pure.js';
export { blockedBy, chainQuietHint, contextStatusKey, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, itemDead, minToHM, nextFire, winHas };
import { confirmDangerDialog, confirmDialog, inlineRename, toast, promptDialog } from './dialogs.js';
import { pollNow, provider } from './board.js';
import { strToB64 } from './layout.js';
import { formatDateTime, formatInterval, formatNumber, t } from './i18n.js';

/* ---------- scheduled prompts ---------- */
export async function refreshQueue() {
  try { queueCache = await inv('queue_list'); } catch (e) { return; }
  renderQueueUI();
}

export const sessionQueue = session => queueCache.items.filter(i => i.session === session);

export function setQueueChip(chip, card) {
  if (!chip) return;
  const q = sessionQueue(card.session);
  chip.textContent = q.length ? '⏰' + q.length : '';
  chip.title = q.length ? t('queue.next', { when: fmtWhen(q[0]), prompt: q[0].text }) : '';
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

export function localizedChainQuietHint(idleSecs, alive) {
  if (!alive) return t('queue.quiet.stopped');
  if (idleSecs == null) return '';
  const total = 180;
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
  i.mode === 'chain' && card ? localizedChainQuietHint(card.idle, card.status !== 'stopped') : '';

/* refresh the quiet counters in place on every poll tick — text-only, so an
   open panel never gets its DOM (hover/click targets, inline edits) rebuilt.
   The panel is per-session, so every chain head shares one hint. */
export function updateQuietHints() {
  if (!queueOpen || state.view !== 'session') return;
  const card = provider.get(state.sessionId);
  if (!card) return;
  const suffix = localizedChainQuietHint(card.idle, card.status !== 'stopped');
  document.querySelectorAll('#queue-list .qg-when[data-quiet]').forEach(el => {
    el.textContent = t('queue.afterPrevious') + suffix;
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
      parts.push(t(sleeping ? 'queue.meta.sleeping' : 'queue.meta.next', { time: fmtClock(nextFire(i)) }));
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
  if (g.head.mode === 'chain') whenEl.dataset.quiet = '1';
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

  /* rows: real queue items, plus a rule's embedded steps (read-only) */
  const rows = [];
  for (const i of g.rows) {
    rows.push({ text: i.text, item: i });
    if (i.steps) for (const s of i.steps) rows.push({ text: s, item: null });
  }
  rows.forEach((r, k) => {
    const row = document.createElement('div');
    row.className = 'qg-row' + (r.item ? '' : ' ro');
    const dead = r.item && itemDead(r.item);
    const ambiguous = r.item && r.item.state === 'ambiguous';
    const contextBlocked = r.item && r.item.last_context && r.item.last_context.status !== 'ready'
      && !ambiguous && r.item.state !== 'firing';
    const manualAllowed = r.item && !ambiguous && r.item.state !== 'firing' && !dead;
    row.innerHTML = '<span class="tree"></span><span class="q-text"></span><span class="row-meta"></span>'
      + (contextBlocked ? '<button class="q-wait"></button>' : '')
      + (manualAllowed ? '<button class="q-now"></button>' : '')
      + (ambiguous ? '<button class="q-ack"></button><button class="q-risk-retry"></button>' : '')
      + (dead ? '<button class="q-retry">↻</button><button class="q-skip">⏭</button>' : '')
      + (r.item ? '<button class="q-del">✕</button>' : '');
    row.querySelector('.tree').textContent =
      rows.length === 1 || k === 0 ? '' : (k === rows.length - 1 ? '└' : '├');
    const txt = row.querySelector('.q-text');
    txt.textContent = r.text;
    if (r.item) {
      const i = r.item;
      txt.title = t('common.edit');
      txt.onclick = () => {
        inlineRename(txt, i.text, v => {
          if (v && v !== i.text) {
            inv('queue_update', { id: i.id, text: v }).catch(() => toast(t('error.operation', { operation: t('common.edit') })));
          } else {
            setTimeout(renderQueueUI, 0);   // after blur, so the guard won't skip
          }
        });
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
  if (document.activeElement && document.activeElement.tagName === 'INPUT'
      && document.activeElement.closest('#queue-list')) return;
  const card = state.view === 'session' && provider.get(state.sessionId);
  if (card) {
    const q = sessionQueue(card.session);
    $('queue-cnt').textContent = q.length || '';
    if (queueOpen) {
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
  queueOpen = open !== undefined ? open : !queueOpen;
  $('queue-panel').style.display = queueOpen ? 'flex' : 'none';
  if (queueOpen) {
    const card = provider.get(state.sessionId);
    $('q-when').value = card && sessionQueue(card.session).length ? 'chain' : '300';
    syncSentence();
    renderQueueUI();
    $('q-text').focus();
  } else if (term) {
    term.focus();
  }
}

export function syncSentence() {
  const w = $('q-when').value;
  $('q-time').style.display = w === 'custom' ? '' : 'none';
  const rec = w.startsWith('e');
  $('q-win').style.display = rec ? '' : 'none';
  $('q-until').style.display = rec ? '' : 'none';
  const wc = rec && $('q-win').value === 'custom';
  for (const id of ['q-win-a', 'q-win-dash', 'q-win-b']) $(id).style.display = wc ? '' : 'none';
  $('q-until-t').style.display = rec && $('q-until').value === 't' ? '' : 'none';
}
export const nextEpochFor = t => {
  const [h, m] = t.split(':').map(Number);
  const d = new Date();
  d.setHours(h, m, 0, 0);
  if (d.getTime() <= Date.now()) d.setDate(d.getDate() + 1);   // past → tomorrow
  return Math.floor(d.getTime() / 1000);
};

/* templates live on the project object → persisted inside the board file */
export function projTemplates(card) {
  const p = card && provider.project(card.projectId);
  if (!p) return [];
  return p.templates || [];
}

export function setQSrc(tpl) {
  qTpl = tpl;
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
  if (qTpl) {
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
    r.querySelector('.t-name').title = template.steps.join('\n');
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
        if (qTpl === template) setQSrc({ ...template, name });
      }
    };
    r.querySelector('.t-del').onclick = async e => {
      e.stopPropagation();
      if (!(await confirmDialog(t('queue.deleteTemplate', { name: template.name })))) return;
      await provider.deleteTemplate(card.projectId, template.name);
      if (qTpl === template) setQSrc(null);
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

  $('q-when').addEventListener('change', syncSentence);

  $('q-win').addEventListener('change', syncSentence);

  $('q-until').addEventListener('change', syncSentence);

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
    const w = $('q-when').value;
    let mode = 'at', at = null, every = null, winFrom = null, winTo = null, untilN = null, untilAt = null;
    if (w === 'chain') {
      mode = 'chain';
    } else if (w === 'custom') {
      const startTime = $('q-time').value;
      if (!startTime) { $('q-time').focus(); return; }
      at = nextEpochFor(startTime);
    } else if (w.startsWith('e')) {
      mode = 'every';
      every = parseInt(w.slice(1), 10) * 60;
      const wv = $('q-win').value;
      if (wv === 'custom') {
        const a = $('q-win-a').value, b = $('q-win-b').value;
        if (!a || !b) { toast(t('queue.setWindow')); return; }
        winFrom = hmToMin(a);
        winTo = hmToMin(b);
      } else if (wv) {
        [winFrom, winTo] = wv.split('-').map(Number);
      }
      const uv = $('q-until').value;
      if (uv.startsWith('n')) untilN = parseInt(uv.slice(1), 10);
      else if (uv === 't') {
        const stopTime = $('q-until-t').value;
        if (!stopTime) { toast(t('queue.setStop')); return; }
        untilAt = nextEpochFor(stopTime);
      }
    } else {
      at = Math.floor(Date.now() / 1000) + parseInt(w, 10) * 60;
    }
    const base = {
      session: card.session, cardId: card.id, dir: card.dir, cmd: card.cmd,
    };
    try {
      if (qTpl) {
        const steps = qTpl.steps.slice();
        if (mode === 'every') {
          /* one standing rule holds the whole template; steps 2..N re-enqueue
             as chain items on every fire */
          await inv('queue_add', { args: { ...base, text: steps[0], mode, at: null, every, winFrom, winTo, untilN, untilAt,
            steps: steps.slice(1), tpl: qTpl.name, tplIdx: 1, tplTotal: steps.length } });
        } else {
          for (let k = 0; k < steps.length; k++) {
            await inv('queue_add', { args: { ...base, text: steps[k],
              mode: k === 0 ? mode : 'chain', at: k === 0 ? at : null,
              tpl: qTpl.name, tplIdx: k + 1, tplTotal: steps.length } });
          }
        }
        setQSrc(null);
      } else {
        const text = $('q-text').value.trim();
        if (!text) { $('q-text').focus(); return; }
        await inv('queue_add', { args: { ...base, text, mode, at, every, winFrom, winTo, untilN, untilAt } });
        $('q-text').value = '';
      }
      $('q-when').value = 'chain';   // natural default for the next one
      syncSentence();
    } catch (e) {
      toast(t('error.operation', { operation: t('common.add') }));
    }
  };

  $('q-text').addEventListener('keydown', e => { if (e.key === 'Enter') $('q-add-btn').click(); });

  listen('menu-clear', () => {
    if (state.view === 'session' && term) {
      term.clear();
      if (attachedName) inv('pty_write', { name: attachedName, dataB64: strToB64('\x0c') }).catch(() => {});
    }
  }).catch(() => uev('listen-fail', 'menu-clear'));

  listen('queue-changed', refreshQueue).catch(() => uev('listen-fail', 'queue-changed'));

  listen('queue-fired', ev => {
    toast(t('queue.sent', { session: ev.payload.session }));
    pollNow();
  }).catch(() => uev('listen-fail', 'queue-fired'));
}
