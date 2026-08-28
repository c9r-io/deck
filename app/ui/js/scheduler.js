// scheduler.js — scheduled prompts: queue groups, recurring rules, templates
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, inv, listen, state, uev } from './state.js';
import { blockedBy, chainQuietHint, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, itemDead, minToHM, nextFire, winHas } from './pure.js';
export { blockedBy, chainQuietHint, fmtEvery, groupQueue, groupSteps, hasWindow, hmToMin, itemDead, minToHM, nextFire, winHas };
import { saveBoard } from './persistence.js';
import { confirmDialog, inlineRename, toast, promptDialog } from './dialogs.js';
import { pollNow, provider } from './board.js';
import { strToB64 } from './layout.js';

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
  chip.title = q.length ? `next ${fmtWhen(q[0])}: ${q[0].text}` : '';
}

export const fmtClock = ts => {
  const d = new Date(ts * 1000);
  const today = new Date().toDateString() === d.toDateString();
  return (today ? '' : (d.getMonth() + 1) + '/' + d.getDate() + ' ') + d.toTimeString().slice(0, 5);
};
export function fmtWhen(i) {
  if (i.mode === 'chain') return '↳ after prev';
  if (i.mode === 'every') return '↻ every ' + fmtEvery(i.every);
  return fmtClock(i.at);
}

const chainWhenSuffix = (i, card) =>
  i.mode === 'chain' && card ? chainQuietHint(card.idle, card.status !== 'stopped') : '';

/* refresh the quiet counters in place on every poll tick — text-only, so an
   open panel never gets its DOM (hover/click targets, inline edits) rebuilt.
   The panel is per-session, so every chain head shares one hint. */
export function updateQuietHints() {
  if (!queueOpen || state.view !== 'session') return;
  const card = provider.get(state.sessionId);
  if (!card) return;
  const suffix = chainQuietHint(card.idle, card.status !== 'stopped');
  document.querySelectorAll('#queue-list .qg-when[data-quiet]').forEach(el => {
    el.textContent = '↳ after prev' + suffix;
  });
}

export function qMeta(i) {
  const parts = [];
  if (i.state === 'ambiguous') parts.push('⚠ delivery outcome unknown — decide below');
  else if (i.state === 'firing') parts.push('sending…');
  if (i.tpl) parts.push(`tpl·${i.tpl} ${i.tpl_idx}/${i.tpl_total}`);
  if (i.mode === 'every') {
    if (hasWindow(i)) parts.push(minToHM(i.win_from) + '–' + minToHM(i.win_to));
    if (i.paused) {
      parts.push('paused');
    } else {
      const nm = new Date();
      const sleeping = hasWindow(i) && !winHas(nm.getHours() * 60 + nm.getMinutes(), i.win_from, i.win_to);
      parts.push((sleeping ? 'sleeps — resumes ' : 'next ') + fmtClock(nextFire(i)));
    }
    if (i.fired) parts.push(i.fired + '×' + (i.until_n ? '/' + i.until_n : ''));
    if (i.state === 'failed') parts.push('⚠ send failed ×' + i.attempts + (i.last_error ? ': ' + i.last_error : ''));
    else if (i.until_n) parts.push('stops after ' + i.until_n + '×');
    if (i.until_at) parts.push('until ' + fmtClock(i.until_at));
  }
  return parts.join(' · ');
}


export async function saveGroupAsTemplate(g) {
  const card = provider.get(state.sessionId);
  if (!card) return;
  const steps = groupSteps(g);
  const name = await promptDialog(
    `Save this group (${steps.length} prompt${steps.length > 1 ? 's, in order' : ''}) as a template for this project:`,
    g.head.tpl || '');
  if (!name) return;
  const tpls = projTemplates(card);
  const ex = tpls.find(t => t.name === name);
  if (ex) ex.steps = steps; else tpls.push({ name, steps });
  saveBoard();
  toast('template saved: ' + name);
}

export function groupEl(g, card) {
  const rule = g.head.mode === 'every';
  const el = document.createElement('div');
  el.className = 'q-group' + (rule ? ' rule' : '') + (g.head.paused ? ' paused' : '');

  const head = document.createElement('div');
  head.className = 'qg-head';
  head.innerHTML = '<span class="qg-when"></span><span class="qg-meta"></span><span class="qg-act">'
    + (rule ? '<button class="qg-pause"></button>' : '')
    + (rule && itemDead(g.head) ? '<button class="qg-retry" title="retry this rule (fresh attempts)">↻</button>' : '')
    + '<button class="qg-save" title="save this group as a project template">☆</button>'
    + '<button class="qg-del" title="remove the whole group">✕</button></span>';
  const whenEl = head.querySelector('.qg-when');
  whenEl.textContent = fmtWhen(g.head) + chainWhenSuffix(g.head, card);
  /* chain heads get their quiet counter refreshed on every poll tick */
  if (g.head.mode === 'chain') whenEl.dataset.quiet = '1';
  const n = groupSteps(g).length;
  head.querySelector('.qg-meta').textContent =
    [qMeta(g.head), n > 1 ? `then ${n - 1} follow-up${n > 2 ? 's' : ''}, in order` : '']
      .filter(Boolean).join(' · ');
  const pb = head.querySelector('.qg-pause');
  if (pb) {
    pb.textContent = g.head.paused ? '▶' : '⏸';
    pb.title = g.head.paused ? 'resume this rule' : 'pause this rule (keeps its settings)';
    pb.onclick = () => inv('queue_pause', { id: g.head.id, paused: !g.head.paused }).catch(e => toast('pause failed: ' + e));
  }
  const hr = head.querySelector('.qg-retry');
  if (hr) hr.onclick = () => inv('queue_retry', { id: g.head.id }).catch(e => toast('retry failed: ' + e));
  head.querySelector('.qg-save').onclick = () => saveGroupAsTemplate(g);
  head.querySelector('.qg-del').onclick = () => {
    for (const i of g.rows) inv('queue_remove', { id: i.id }).catch(e => toast('remove failed: ' + e));
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
    row.innerHTML = '<span class="tree"></span><span class="q-text"></span><span class="row-meta"></span>'
      + (ambiguous ? '<button class="q-ack" title="acknowledge as sent (does not resend)">✓ sent</button>'
              + '<button class="q-risk-retry" title="retry — may deliver this prompt twice">↻ retry</button>' : '')
      + (dead ? '<button class="q-retry" title="retry this step (fresh attempts)">↻</button>'
              + '<button class="q-skip" title="skip this failed step — later steps continue">⏭</button>' : '')
      + (r.item ? '<button class="q-del" title="remove this prompt">✕</button>' : '');
    row.querySelector('.tree').textContent =
      rows.length === 1 || k === 0 ? '' : (k === rows.length - 1 ? '└' : '├');
    const txt = row.querySelector('.q-text');
    txt.textContent = r.text;
    if (r.item) {
      const i = r.item;
      txt.title = 'click to edit';
      txt.onclick = () => {
        inlineRename(txt, i.text, v => {
          if (v && v !== i.text) {
            inv('queue_update', { id: i.id, text: v }).catch(e => toast('edit failed: ' + e));
          } else {
            setTimeout(renderQueueUI, 0);   // after blur, so the guard won't skip
          }
        });
      };
      row.querySelector('.q-del').onclick = () => inv('queue_remove', { id: i.id }).catch(e => toast('remove failed: ' + e));
      const rb = row.querySelector('.q-retry');
      if (rb) rb.onclick = () => inv('queue_retry', { id: i.id }).catch(e => toast('retry failed: ' + e));
      const ack = row.querySelector('.q-ack');
      if (ack) ack.onclick = () => inv('queue_acknowledge', { id: i.id })
        .catch(e => toast('acknowledge failed: ' + e));
      const riskRetry = row.querySelector('.q-risk-retry');
      if (riskRetry) riskRetry.onclick = async () => {
        if (!(await confirmDialog('Retry this ambiguous delivery? The earlier send may have succeeded, so this can send the prompt twice.'))) return;
        inv('queue_retry', { id: i.id }).catch(e => toast('retry failed: ' + e));
      };
      const sb = row.querySelector('.q-skip');
      if (sb) sb.onclick = () => inv('queue_skip', { id: i.id }).catch(e => toast('skip failed: ' + e));
      const bits = [];
      if (i.tpl && i.mode !== 'every') bits.push(`tpl·${i.tpl} ${i.tpl_idx}/${i.tpl_total}`);
      if (i.state === 'ambiguous') {
        bits.push('⚠ deck crashed during delivery; acknowledge it as sent or retry with duplicate risk');
      } else if (i.state === 'failed' && i.mode !== 'every') {
        bits.push('⚠ ' + (itemDead(i) ? 'gave up — blocks later steps until retried or skipped'
          : 'send failed, retrying') + (i.last_error ? ': ' + i.last_error : ''));
      } else if (blockedBy(i, g.rows)) {
        bits.push('⏸ waiting — an earlier step failed (retry or skip it)');
      }
      row.querySelector('.row-meta').textContent = bits.join(' · ');
    } else {
      txt.title = 'template step — fires as part of this rule, in order';
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
      if (!q.length) list.innerHTML = '<div class="q-hint">nothing scheduled for this session yet</div>';
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

$('queue-btn').onclick = () => toggleQueuePanel();
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
$('q-when').addEventListener('change', syncSentence);
$('q-win').addEventListener('change', syncSentence);
$('q-until').addEventListener('change', syncSentence);
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
  if (!p.templates) p.templates = [];
  return p.templates;
}


export function setQSrc(tpl) {
  qTpl = tpl;
  const btn = $('q-src'), inp = $('q-text');
  if (tpl) {
    btn.textContent = '📋';
    btn.classList.add('tpl');
    inp.value = `${tpl.name}  (${tpl.steps.length} steps, in order)`;
    inp.readOnly = true;
    $('q-add-btn').textContent = 'Add ' + tpl.steps.length;
  } else {
    btn.textContent = '✎';
    btn.classList.remove('tpl');
    if (inp.readOnly) inp.value = '';
    inp.readOnly = false;
    $('q-add-btn').textContent = 'Add';
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
  add('t-head').textContent = 'TEMPLATES — PROJECT ' + ((proj && proj.name) || '').toUpperCase();
  if (qTpl) {
    const r = add('t-row', '<span class="t-name">✎ type a prompt instead</span>');
    r.onclick = () => { setQSrc(null); hideTplPop(); $('q-text').focus(); };
  }
  if (!tpls.length) {
    add('t-row', '<span class="t-name" style="color:var(--faint)">no templates yet — queue prompts, then save them below</span>');
  }
  for (const t of tpls) {
    const r = add('t-row', '<span class="t-name"></span><span class="t-n"></span><button class="t-act" title="rename">✎</button><button class="t-act t-del" title="delete">✕</button>');
    r.querySelector('.t-name').textContent = t.name;
    r.querySelector('.t-name').title = t.steps.join('\n');
    r.querySelector('.t-n').textContent = t.steps.length + ' steps';
    r.querySelector('.t-name').onclick = () => { setQSrc(t); hideTplPop(); };
    r.querySelector('.t-act').onclick = async e => {
      e.stopPropagation();
      const name = await promptDialog('Rename template:', t.name);
      if (name && name !== t.name) { t.name = name; saveBoard(); if (qTpl === t) setQSrc(t); }
    };
    r.querySelector('.t-del').onclick = async e => {
      e.stopPropagation();
      if (!(await confirmDialog(`Delete template "${t.name}"? Prompts already queued are not affected.`))) return;
      const arr = projTemplates(card);
      arr.splice(arr.indexOf(t), 1);
      saveBoard();
      if (qTpl === t) setQSrc(null);
      hideTplPop();
    };
  }
  add('t-sep');
  add('t-row', '<span class="t-name" style="color:var(--faint)">☆ on a group header saves that group as a template</span>');
  const a = $('q-src');
  pop.style.left = a.offsetLeft + 'px';
  pop.style.top = (a.offsetTop + a.offsetHeight + 6) + 'px';
  pop.style.display = 'block';
}


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
    const t = $('q-time').value;
    if (!t) { $('q-time').focus(); return; }
    at = nextEpochFor(t);
  } else if (w.startsWith('e')) {
    mode = 'every';
    every = parseInt(w.slice(1), 10) * 60;
    const wv = $('q-win').value;
    if (wv === 'custom') {
      const a = $('q-win-a').value, b = $('q-win-b').value;
      if (!a || !b) { toast('set both ends of the time window'); return; }
      winFrom = hmToMin(a);
      winTo = hmToMin(b);
    } else if (wv) {
      [winFrom, winTo] = wv.split('-').map(Number);
    }
    const uv = $('q-until').value;
    if (uv.startsWith('n')) untilN = parseInt(uv.slice(1), 10);
    else if (uv === 't') {
      const t = $('q-until-t').value;
      if (!t) { toast('set the stop time'); return; }
      untilAt = nextEpochFor(t);
    }
  } else {
    at = Math.floor(Date.now() / 1000) + parseInt(w, 10) * 60;
  }
  const base = { session: card.session, dir: card.dir, cmd: card.cmd };
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
    toast('add failed: ' + e);
  }
};
$('q-text').addEventListener('keydown', e => { if (e.key === 'Enter') $('q-add-btn').click(); });

/* native menu: Terminal → Clear (⌘K) */
listen('menu-clear', () => {
  if (state.view === 'session' && term) {
    term.clear();
    if (attachedName) inv('pty_write', { name: attachedName, dataB64: strToB64('\x0c') }).catch(() => {});
  }
}).catch(() => uev('listen-fail', 'menu-clear'));

listen('queue-changed', refreshQueue).catch(() => uev('listen-fail', 'queue-changed'));
listen('queue-fired', ev => {
  toast(`scheduled prompt sent → ${ev.payload.session}`);
  pollNow();
}).catch(() => uev('listen-fail', 'queue-fired'));
