// automation.js — 自动化: the project's scheduled automations drawer
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// An automation is an inbound rule with `source: 'clock'` (settings.json
// `inbound.rules`, validated by settings-model.js and inbound.rs alike): a
// schedule, the column it creates cards in, directory, command, template and
// a finish mode. This module is only the drawer that lists the CURRENT
// project's clock rules, edits them through `persistInbound` (one durable
// settings write, the same path Slack rules take), and shows each rule's next
// slot plus its last runs from the backend ledger (`inbound_runs`). Firing
// is the backend clock source + inbound.js; closing a finished run is
// board.js's poll. A rule whose project no longer exists is dropped on the
// next Board change, so a deleted project never leaves a rule that fires
// into nothing. Weekday chips and day-of-month options are built here (their
// labels go through the locale), so the editor is rebuilt on a language
// change; the drawer itself is re-rendered on every Board transaction and
// every inbound change while open. A rule's `since` moves to now whenever
// its schedule changes or it is resumed from pause, so a slot earlier that
// day is never caught up by the edit itself; how long after its slot a run
// may still start is the rule's `graceMin` (the backend records a later
// slot as missed).
import { $, ctx, genId, inv, listen, state, store, uev } from './state.js';
import { activeProject, provider } from './board.js';
import { openSession } from './layout.js';
import { confirmDialog, persistInbound, toast } from './dialogs.js';
import { hmToMin, minToHM, nextScheduleSlot, toggleClockRule } from './pure.js';
import { formatNumber, onLocaleChange, t } from './i18n.js';
import { DEFAULT_GRACE_MIN, GRACE_CHOICES } from './settings-model.js';
import { fmtClock } from './scheduler.js';

let editing = null;   // null | { id } (existing) | { id: null } (new)
let runsCache = [];
let unsubscribe = null;

export const clockRules = (projectId = state.projectId) =>
  ((ctx.settings && ctx.settings.inbound && ctx.settings.inbound.rules) || [])
    .filter(r => r.source === 'clock' && (!projectId || r.projectId === projectId));

export const clockRuleById = id => clockRules(null).find(r => r.id === id) || null;

export const isOpen = () => !$('auto-drawer').hidden;

/* "15 min" / "3 h" / "rest of the day" / "never" in the user's language */
export function graceText(minutes) {
  if (minutes === 0) return t('automation.grace.none');
  if (minutes >= 1440) return t('automation.grace.day');
  if (minutes % 60 === 0) return t('automation.grace.hours', { count: formatNumber(minutes / 60) });
  return t('automation.grace.minutes', { count: formatNumber(minutes) });
}

/* "every Mon, Wed, Fri at 09:00" in the user's language */
export function scheduleText(schedule) {
  const time = minToHM(schedule.minute);
  if (schedule.unit === 'week') {
    const days = schedule.days.map(d => t(`automation.wd.${d}`)).join(t('automation.listSeparator'));
    return t('automation.schedule.week', { days, time });
  }
  if (schedule.unit === 'month') {
    const days = schedule.days.map(d => formatNumber(d)).join(t('automation.listSeparator'));
    return t('automation.schedule.month', { days, time });
  }
  return t('automation.schedule.day', { time });
}

export function refreshAutomationBadge() {
  const n = clockRules().length;
  $('board-auto-count').textContent = n ? formatNumber(n) : '';
  $('board-auto').classList.toggle('on', isOpen());
}

async function refreshRuns() {
  try { runsCache = await inv('inbound_runs'); } catch (e) { runsCache = []; }
}

/* ---------- list ---------- */
function runLine(run) {
  const line = document.createElement('span');
  const started = fmtClock(run.started);
  if (run.outcome === 'running') {
    line.className = 'live';
    line.textContent = `● ${started} · ${t('automation.run.running')}`;
    const card = run.card && provider.get(run.card);
    if (card) {
      const open = document.createElement('button');
      open.textContent = ' · ' + t('automation.run.open');
      open.onclick = () => openSession(card.id);
      line.appendChild(open);
    }
    return line;
  }
  if (run.outcome === 'closed') {
    const minutes = run.ended ? Math.max(1, Math.round((run.ended - run.started) / 60)) : null;
    const mark = document.createElement('span');
    mark.className = 'ok';
    mark.textContent = '✓ ';
    line.append(mark, `${started}${minutes ? ' · ' + t('automation.run.minutes', { count: formatNumber(minutes) }) : ''} · ${t('automation.run.closed')}`);
    return line;
  }
  line.textContent = `– ${started} · ${t('automation.run.skipped')}${run.reason ? ' (' + t(`automation.run.${run.reason}`) + ')' : ''}`;
  return line;
}

function ruleEl(rule) {
  const project = activeProject();
  const column = project && project.columns.find(c => c.id === rule.columnId);
  const el = document.createElement('div');
  el.className = 'auto-rule' + (rule.enabled ? '' : ' off');
  el.innerHTML = '<div class="ar-head"><span class="ar-name"></span><span class="ar-when"></span>'
    + '<span class="ar-acts"><button class="ar-pause"></button><button class="ar-edit">✎</button><button class="ar-del">✕</button></span></div>'
    + '<div class="ar-body"><div class="ar-kv"></div><div class="ar-runs"></div></div>';
  el.querySelector('.ar-name').textContent = rule.name || rule.id;
  el.querySelector('.ar-when').textContent = scheduleText(rule.schedule);
  const pause = el.querySelector('.ar-pause');
  pause.textContent = rule.enabled ? '⏸' : '▶';
  pause.title = t(rule.enabled ? 'automation.pause' : 'automation.resume');
  pause.onclick = () => saveRule(toggleClockRule(rule, Math.floor(Date.now() / 1000)));
  el.querySelector('.ar-edit').title = t('automation.edit');
  el.querySelector('.ar-edit').onclick = () => openEditor(rule);
  el.querySelector('.ar-del').title = t('automation.delete');
  el.querySelector('.ar-del').onclick = async () => {
    if (!(await confirmDialog(t('automation.deleteConfirm', { name: rule.name || rule.id })))) return;
    await persistInbound({ ...ctx.settings.inbound, rules: ctx.settings.inbound.rules.filter(r => r.id !== rule.id) });
    renderAutomations();
  };
  const kv = el.querySelector('.ar-kv');
  const next = rule.enabled ? nextScheduleSlot(rule.schedule, Math.floor(Date.now() / 1000), rule.since) : null;
  const rows = [
    ['automation.kv.target', `${column ? column.name : t('settings.inboundRuleMissingTarget')} · ${rule.dir || ctx.HOME}`],
    ['automation.kv.cmd', rule.cmd || t('settings.inboundRuleShellOnly')],
    ['automation.kv.template', rule.template],
    ['automation.kv.finish', t(rule.finish === 'close' ? 'automation.finish.close' : 'automation.finish.keep')],
    ['automation.kv.grace', graceText(rule.graceMin ?? DEFAULT_GRACE_MIN)],
    ['automation.kv.next', rule.enabled ? (next ? fmtClock(next) : '—') : t('automation.paused')],
  ];
  for (const [key, value] of rows) {
    const k = document.createElement('span'); k.textContent = t(key);
    const v = document.createElement('b'); v.textContent = value; v.title = value;
    kv.append(k, v);
  }
  const runs = el.querySelector('.ar-runs');
  const mine = runsCache.filter(r => r.rule === rule.id).slice(-4).reverse();
  if (!mine.length) runs.hidden = true;
  for (const run of mine) runs.appendChild(runLine(run));
  return el;
}

export function renderAutomations() {
  refreshAutomationBadge();
  if (!isOpen()) return;
  const list = $('auto-list');
  list.innerHTML = '';
  const rules = clockRules();
  if (!rules.length) {
    const empty = document.createElement('div');
    empty.className = 'auto-empty';
    empty.textContent = t('automation.empty');
    list.appendChild(empty);
  }
  for (const rule of rules) list.appendChild(ruleEl(rule));
}

/* ---------- editor ---------- */
const segGet = id => $(id).querySelector('button[aria-pressed="true"]')?.dataset.v || '';
const segSet = (id, v) => $(id).querySelectorAll('button').forEach(b => b.setAttribute('aria-pressed', b.dataset.v === v ? 'true' : 'false'));
const pressedDays = () => [...$('auto-days').querySelectorAll('.q-day[aria-pressed="true"]')].map(b => Number(b.dataset.d));

function buildDayControls() {
  const days = $('auto-days');
  const pressed = days.childElementCount ? new Set(pressedDays()) : new Set([1, 2, 3, 4, 5]);
  days.innerHTML = '';
  for (let d = 1; d <= 7; d++) {
    const b = document.createElement('button');
    b.type = 'button'; b.className = 'q-day'; b.dataset.d = String(d);
    b.textContent = t(`automation.wd.${d}`);
    b.setAttribute('aria-pressed', pressed.has(d) ? 'true' : 'false');
    b.onclick = () => b.setAttribute('aria-pressed', b.getAttribute('aria-pressed') === 'true' ? 'false' : 'true');
    days.appendChild(b);
  }
  const dom = $('auto-dom');
  const keep = dom.value || '1';
  dom.innerHTML = '';
  for (let d = 1; d <= 31; d++) {
    const o = document.createElement('option');
    o.value = String(d); o.textContent = formatNumber(d);
    dom.appendChild(o);
  }
  dom.value = keep;
  buildGraceOptions(Number($('auto-grace').value) || DEFAULT_GRACE_MIN);
}

/* the grace select: the fixed choices plus, when a saved rule has another
   value (an older default, a hand-edited settings file), that value too */
function buildGraceOptions(value) {
  const sel = $('auto-grace');
  sel.innerHTML = '';
  for (const m of [...new Set([...GRACE_CHOICES, value])].sort((a, b) => a - b)) {
    const o = document.createElement('option');
    o.value = String(m); o.textContent = graceText(m);
    sel.appendChild(o);
  }
  sel.value = String(value);
}

function syncEditor() {
  const unit = $('auto-unit').value;
  $('auto-days').hidden = unit !== 'week';
  $('auto-dom').hidden = unit !== 'month';
}

function fillTargets(rule) {
  const project = activeProject();
  const cs = $('auto-column'), ts = $('auto-template');
  cs.innerHTML = '';
  for (const c of (project ? project.columns : [])) {
    const o = document.createElement('option');
    o.value = c.id; o.textContent = c.name;
    cs.appendChild(o);
  }
  if (rule && project && project.columns.some(c => c.id === rule.columnId)) cs.value = rule.columnId;
  ts.innerHTML = '';
  const tpls = (project && project.templates) || [];
  if (!tpls.length) {
    const o = document.createElement('option');
    o.value = ''; o.textContent = t('settings.inboundNoTemplates');
    ts.appendChild(o);
  }
  for (const tp of tpls) {
    const o = document.createElement('option');
    o.value = tp.name; o.textContent = `${tp.name} · ${t('queue.steps', { count: formatNumber(tp.steps.length) })}`;
    ts.appendChild(o);
  }
  if (rule && tpls.some(tp => tp.name === rule.template)) ts.value = rule.template;
}

export function openEditor(rule) {
  editing = { id: rule ? rule.id : null };
  const schedule = rule ? rule.schedule : { unit: 'day', days: [], minute: 540 };
  $('auto-name').value = rule ? rule.name : '';
  $('auto-unit').value = schedule.unit;
  buildDayControls();
  if (schedule.unit === 'week') {
    $('auto-days').querySelectorAll('.q-day').forEach(b => b.setAttribute('aria-pressed', schedule.days.includes(Number(b.dataset.d)) ? 'true' : 'false'));
  }
  if (schedule.unit === 'month') $('auto-dom').value = String(schedule.days[0] || 1);
  $('auto-time').value = minToHM(schedule.minute);
  buildGraceOptions(rule ? (rule.graceMin ?? DEFAULT_GRACE_MIN) : DEFAULT_GRACE_MIN);
  $('auto-dir').value = rule ? rule.dir : '';
  $('auto-cmd').value = rule ? rule.cmd : 'claude';
  fillTargets(rule);
  segSet('auto-finish', rule && rule.finish === 'keep' ? 'keep' : 'close');
  syncEditor();
  $('auto-editor').hidden = false;
  $('auto-name').focus();
}

function closeEditor() {
  editing = null;
  $('auto-editor').hidden = true;
}

/* the editor → a rule; null plus a toast when something is missing */
function readEditor() {
  const project = activeProject();
  if (!project) return null;
  const fail = (key, focus) => { toast(t(key)); if (focus) $(focus).focus(); return null; };
  const name = $('auto-name').value.trim();
  if (!name) return fail('automation.needsName', 'auto-name');
  if ([...name].length > 120) return fail('automation.longName', 'auto-name');
  const unit = $('auto-unit').value;
  const days = unit === 'week' ? pressedDays() : unit === 'month' ? [Number($('auto-dom').value)] : [];
  if (unit === 'week' && !days.length) return fail('automation.needsDays');
  const time = $('auto-time').value;
  if (!time) return fail('automation.needsTime', 'auto-time');
  const columnId = $('auto-column').value;
  if (!columnId) return fail('automation.needsColumn', 'auto-column');
  const template = $('auto-template').value;
  if (!template) return fail('automation.needsTemplate', 'auto-template');
  /* the id doubles as the rule's badge, which the backend spells lowercase */
  const id = editing.id || genId('a');
  const previous = editing.id ? clockRuleById(editing.id) : null;
  const schedule = { unit, days, minute: hmToMin(time) };
  /* a changed schedule starts fresh: a slot earlier today never fires late */
  const changed = !previous || JSON.stringify(previous.schedule) !== JSON.stringify(schedule);
  return {
    id, source: 'clock', badge: id, projectId: project.id, columnId,
    cmd: $('auto-cmd').value.trim(), template, dir: $('auto-dir').value.trim(),
    name, enabled: previous ? previous.enabled : true, schedule,
    finish: segGet('auto-finish') === 'keep' ? 'keep' : 'close',
    graceMin: Number($('auto-grace').value),
    since: changed ? Math.floor(Date.now() / 1000) : previous.since,
  };
}

async function saveRule(rule) {
  const rules = ctx.settings.inbound.rules.some(r => r.id === rule.id)
    ? ctx.settings.inbound.rules.map(r => (r.id === rule.id ? rule : r))
    : [...ctx.settings.inbound.rules, rule];
  const ok = await persistInbound({ ...ctx.settings.inbound, rules });
  renderAutomations();
  return ok;
}

/* rules pointing at a project that is gone fire into nothing forever;
   drop them the moment the Board says so */
async function pruneOrphans() {
  const rules = (ctx.settings && ctx.settings.inbound && ctx.settings.inbound.rules) || [];
  const live = new Set(store.projects.map(p => p.id));
  const kept = rules.filter(r => r.source !== 'clock' || live.has(r.projectId));
  if (kept.length === rules.length) return;
  uev('inbound', 'rule-orphaned');
  await persistInbound({ ...ctx.settings.inbound, rules: kept });
}

/* ---------- drawer ---------- */
export async function openAutomations() {
  $('auto-drawer').hidden = false;
  closeEditor();
  await refreshRuns();
  renderAutomations();
  if (!clockRules().length) openEditor(null);
}

export function closeAutomations() {
  closeEditor();
  $('auto-drawer').hidden = true;
  refreshAutomationBadge();
  $('board-auto').focus();
}

export function toggleAutomations() {
  if (isOpen()) closeAutomations(); else openAutomations();
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initAutomation() {
  $('board-auto').onclick = () => toggleAutomations();
  $('auto-close').onclick = () => closeAutomations();
  $('auto-new').onclick = () => openEditor(null);
  $('auto-cancel').onclick = () => closeEditor();
  $('auto-save').onclick = async () => {
    const rule = readEditor();
    if (!rule) return;
    if (await saveRule(rule)) { closeEditor(); toast(t('automation.saved')); }
  };
  $('auto-unit').addEventListener('change', syncEditor);
  $('auto-finish').querySelectorAll('button').forEach(b => {
    b.onclick = () => segSet('auto-finish', b.dataset.v);
  });
  $('auto-drawer').addEventListener('keydown', event => {
    if (event.key !== 'Escape') return;
    event.preventDefault();
    if (editing) closeEditor(); else closeAutomations();
  });
  buildDayControls();
  onLocaleChange(() => { buildDayControls(); renderAutomations(); });
  unsubscribe = provider.subscribe(ev => {
    if (ev === 'projects') pruneOrphans();
    if (ev === 'projects' || ev === 'list') renderAutomations();
  });
  listen('inbound-changed', async () => { await refreshRuns(); renderAutomations(); })
    .catch(() => uev('listen-fail', 'inbound-changed'));
}

export const stopAutomation = () => { if (unsubscribe) { unsubscribe(); unsubscribe = null; } };
