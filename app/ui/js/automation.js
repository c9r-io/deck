// automation.js — 自动化: the project's automations drawer
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// C v01 reviewEach is explicit and applies only to newly dispatched runs;
// ordinary rules and existing runs keep their existing timing.
// An automation is an inbound rule (settings.json `inbound.rules`, validated
// by settings-model.js and inbound.rs alike) with one of two TRIGGERS: a
// clock (`source: 'clock'`, a schedule; its badge IS its id) or a Slack
// badge (`source: 'slack'`, the emoji name; one automation per badge across
// every project, because the dispatcher matches a badge to ONE rule). Both
// share the rest of the shape — the column it creates cards in, directory,
// command, template and a finish mode — and both go through the same
// dispatch (inbound.js) and the same run ledger (`inbound_runs`), so a
// badge-started card is finished by `board.js` exactly like a clock run.
// This module is only the drawer that lists the CURRENT project's rules of
// either trigger, edits them through `persistInbound` (one durable settings
// write), and shows each rule's next slot (clock) or last runs. What a rule
// and its runs read as, how the editor's fields become a rule
// (`composeRule`) and how a saved rule joins the list (`mergeRules`) are
// DOM-free in automation-model.js, where node tests pin them. Firing is
// the backend sources + inbound.js; closing a finished run is board.js's
// poll. The Slack CONNECTION (its switch and tokens) stays in Settings: it
// is account-level, a rule is project-level. A rule whose project no longer
// exists is dropped on the next Board change, whatever its trigger, so a
// deleted project never leaves a rule that fires into nothing. Weekday chips
// and day-of-month options are built here (their labels go through the
// locale), so the editor is rebuilt on a language change; the drawer itself
// is re-rendered on every Board transaction and every inbound change while
// open. A clock rule's `since` moves to now whenever its schedule changes
// or it is resumed from pause, so a slot earlier that day is never caught up
// by the edit itself; how long after its slot a run may still start is the
// rule's `graceMin` (two choices; a saved value outside them stays offered
// so an edit never silently rewrites it). A Slack rule has no pause: the
// backlog it would collect while paused has no honest reading, so it is
// removed instead.
//
// ENTRY POINTS (06 B v01): the Board head keeps ONE persistent action, the
// "New session ▾" split button. Its menu (`showNewSessionMenu`, built in the
// shared #ctx with arrow-key navigation) holds "new session now", the two
// automatic ways a session starts (a clock, a Slack badge — each opens this
// drawer with the editor preset to that trigger), and the two managers
// (Automations…, Templates…). A direct "↻ N automation(s)" chip appears in
// the head only while the project has rules, so the drawer is one click
// away exactly when there is something in it. Closing the drawer returns
// focus to whichever of those opened it. With project defaults (04 A v01)
// the first item's second line says what it will do (directory · command →
// group), a "New shell only" item appears while the project has a default
// command, and "Project defaults…" sits in the manage section. A NEW rule's
// editor starts from those defaults; existing rules keep their own values.
import { $, ctx, genId, inv, listen, state, store, uev } from './state.js';
import { activeProject, newSessionSummary, openProjectDefaults, projectDefaultsSummary, provider } from './board.js';
import { openSession } from './layout.js';
import { confirmDialog, persistInbound, toast } from './dialogs.js';
import { minToHM, projectDefaults, projectRules, ruleByOrigin, toggleClockRule } from './pure.js';
import { composeRule, graceOptions, graceText, liveRules, mergeRules, recentRuns, ruleFacts, ruleLabel, runSummary, triggerText } from './automation-model.js';
import { formatNumber, onLocaleChange, t } from './i18n.js';
import { formatShortcut } from './shortcuts.js';
import { DEFAULT_GRACE_MIN } from './settings-model.js';
import { openTemplates } from './templates.js';
import { newDefaultSession } from './terminal.js';

let opener = null;                 // element that opened the drawer; focus returns there

let editing = null;   // null | { id } (existing) | { id: null } (new)
let runsCache = [];
let unsubscribe = null;

const allRules = () => ((ctx.settings && ctx.settings.inbound && ctx.settings.inbound.rules) || []);

export const rulesOf = (projectId = state.projectId) => projectRules(allRules(), projectId);

/* the automation behind a card's origin, whatever its trigger */
export const ruleOf = origin => ruleByOrigin(allRules(), origin);

export const isOpen = () => !$('auto-drawer').hidden;

const slackConnected = () => !!(ctx.settings && ctx.settings.inbound && ctx.settings.inbound.sources.slack.enabled);

export function refreshAutomationBadge() {
  const n = rulesOf().length;
  const chip = $('board-auto');
  $('board-auto-count').textContent = n ? t('automation.chip', { count: formatNumber(n) }) : '';
  chip.hidden = !n && !isOpen();
  chip.classList.toggle('on', isOpen());
}

async function refreshRuns() {
  try { runsCache = await inv('inbound_runs'); } catch (e) { runsCache = []; }
}

/* ---------- list ---------- */
function runLine(run) {
  const line = document.createElement('span');
  const summary = runSummary(run);
  if (summary.outcome === 'running') {
    line.className = 'live';
    line.textContent = summary.text;
    const card = summary.openable && provider.get(run.card);
    if (card) {
      const open = document.createElement('button');
      open.textContent = ' · ' + t('automation.run.open');
      open.onclick = () => openSession(card.id);
      line.appendChild(open);
    }
    return line;
  }
  if (summary.outcome === 'closed') {
    const mark = document.createElement('span');
    mark.className = 'ok';
    mark.textContent = summary.mark;
    line.append(mark, summary.text);
    return line;
  }
  line.textContent = summary.text;
  return line;
}

function ruleEl(rule) {
  const project = activeProject();
  const column = project && project.columns.find(c => c.id === rule.columnId);
  const clock = rule.source === 'clock';
  const el = document.createElement('div');
  el.className = 'auto-rule' + (rule.enabled ? '' : ' off');
  el.innerHTML = '<div class="ar-head"><span class="ar-name"></span><span class="ar-when"></span>'
    + '<span class="ar-acts">' + (clock ? '<button class="ar-pause"></button>' : '')
    + '<button class="ar-edit">✎</button><button class="ar-del">✕</button></span></div>'
    + '<div class="ar-body"><div class="ar-kv"></div><div class="ar-runs"></div></div>';
  el.querySelector('.ar-name').textContent = ruleLabel(rule);
  el.querySelector('.ar-when').textContent = triggerText(rule);
  const pause = el.querySelector('.ar-pause');
  if (pause) {
    pause.textContent = rule.enabled ? '⏸' : '▶';
    pause.title = t(rule.enabled ? 'automation.pause' : 'automation.resume');
    pause.onclick = () => saveRule(toggleClockRule(rule, Math.floor(Date.now() / 1000)));
  }
  el.querySelector('.ar-edit').title = t('automation.edit');
  el.querySelector('.ar-edit').onclick = () => openEditor(rule);
  el.querySelector('.ar-del').title = t('automation.delete');
  el.querySelector('.ar-del').onclick = async () => {
    if (!(await confirmDialog(t('automation.deleteConfirm', { name: ruleLabel(rule) })))) return;
    await persistInbound({ ...ctx.settings.inbound, rules: ctx.settings.inbound.rules.filter(r => r.id !== rule.id) });
    renderAutomations();
  };
  const kv = el.querySelector('.ar-kv');
  const rows = ruleFacts(rule, { columnName: column ? column.name : null, home: ctx.HOME, slackConnected: slackConnected() });
  for (const [key, value] of rows) {
    const k = document.createElement('span'); k.textContent = t(key);
    const v = document.createElement('b'); v.textContent = value; v.title = value;
    kv.append(k, v);
  }
  const runs = el.querySelector('.ar-runs');
  const mine = recentRuns(runsCache, rule);
  if (!mine.length) runs.hidden = true;
  for (const run of mine) runs.appendChild(runLine(run));
  return el;
}

export function renderAutomations() {
  refreshAutomationBadge();
  if (!isOpen()) return;
  const list = $('auto-list');
  list.innerHTML = '';
  const rules = rulesOf();
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

/* the grace select: the two choices plus a saved rule's other value */
function buildGraceOptions(value) {
  const sel = $('auto-grace');
  sel.innerHTML = '';
  for (const m of graceOptions(value)) {
    const o = document.createElement('option');
    o.value = String(m); o.textContent = graceText(m);
    sel.appendChild(o);
  }
  sel.value = String(value);
}

/* the editor shows the controls of the chosen trigger and nothing of the
   other: every trigger-bound control carries `q-p-clock` or `q-p-slack` */
function syncEditor() {
  const trigger = segGet('auto-trigger');
  document.querySelectorAll('#auto-editor .q-p').forEach(el => {
    el.hidden = !el.classList.contains(`q-p-${trigger}`);
  });
  const unit = $('auto-unit').value;
  $('auto-days').hidden = trigger !== 'clock' || unit !== 'week';
  $('auto-dom').hidden = trigger !== 'clock' || unit !== 'month';
  $('auto-slack-state').textContent = t(slackConnected() ? 'automation.slackOn' : 'automation.slackOffHint');
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
    o.value = ''; o.textContent = t('automation.noTemplates');
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
  const clock = !rule || rule.source === 'clock';
  const schedule = clock && rule ? rule.schedule : { unit: 'day', days: [], minute: 540 };
  segSet('auto-trigger', clock ? 'clock' : 'slack');
  $('auto-name').value = rule ? rule.name : '';
  $('auto-badge').value = rule && !clock ? rule.badge : '';
  $('auto-unit').value = schedule.unit;
  buildDayControls();
  if (schedule.unit === 'week') {
    $('auto-days').querySelectorAll('.q-day').forEach(b => b.setAttribute('aria-pressed', schedule.days.includes(Number(b.dataset.d)) ? 'true' : 'false'));
  }
  if (schedule.unit === 'month') $('auto-dom').value = String(schedule.days[0] || 1);
  $('auto-time').value = minToHM(schedule.minute);
  buildGraceOptions(rule && clock ? (rule.graceMin ?? DEFAULT_GRACE_MIN) : DEFAULT_GRACE_MIN);
  /* a new rule starts from the project's defaults (04): saved into the rule
     on Save, editable here; an existing rule keeps what it has */
  const defaults = projectDefaults(activeProject());
  $('auto-dir').value = rule ? rule.dir : defaults.dir;
  $('auto-cmd').value = rule ? rule.cmd : (defaults.cmd || 'claude');
  fillTargets(rule);
  $('auto-review').checked = rule?.reviewEach === true;
  segSet('auto-finish', rule ? (rule.finish === 'close' ? 'close' : 'keep') : 'close');
  syncEditor();
  $('auto-editor').hidden = false;
  $('auto-name').focus();
}

function closeEditor() {
  editing = null;
  $('auto-editor').hidden = true;
}

/* the editor → a rule (`composeRule`); null plus a toast when something is
   missing, with focus on the control that needs it */
function readEditor() {
  const project = activeProject();
  if (!project) return null;
  const previous = editing.id ? allRules().find(r => r.id === editing.id) || null : null;
  const fields = {
    trigger: segGet('auto-trigger'),
    name: $('auto-name').value, columnId: $('auto-column').value, template: $('auto-template').value,
    cmd: $('auto-cmd').value, dir: $('auto-dir').value,
    finish: segGet('auto-finish'), reviewEach: $('auto-review').checked,
    badge: $('auto-badge').value,
    unit: $('auto-unit').value, days: pressedDays(), dayOfMonth: $('auto-dom').value,
    time: $('auto-time').value, graceMin: $('auto-grace').value,
  };
  const read = composeRule(fields, { previous, rules: allRules(), projectId: project.id, genId });
  if (read.error) {
    toast(t(read.error, read.params));
    if (read.focus) $(read.focus).focus();
    return null;
  }
  return read.rule;
}

async function saveRule(rule) {
  /* a trigger change gives the rule a new id: the old entry goes */
  const rules = mergeRules(ctx.settings.inbound.rules, rule, editing && editing.id);
  const ok = await persistInbound({ ...ctx.settings.inbound, rules });
  renderAutomations();
  return ok;
}

/* rules pointing at a project that is gone fire into nothing forever;
   drop them the moment the Board says so */
async function pruneOrphans() {
  const kept = liveRules(allRules(), store.projects);
  if (!kept) return;
  uev('inbound', 'rule-orphaned');
  await persistInbound({ ...ctx.settings.inbound, rules: kept });
}

/* ---------- drawer ---------- */
/* `from` is the element to return focus to, or a function resolving it at
   close time (a project tab is rebuilt by every render, so the element the
   menu was opened on is gone by then); `trigger` ('clock' | 'slack') opens
   the editor for a NEW rule preset to that trigger. */
export async function openAutomations({ from = null, trigger = null } = {}) {
  opener = from;
  $('auto-drawer').hidden = false;
  closeEditor();
  await refreshRuns();
  renderAutomations();
  if (trigger) { openEditor(null); segSet('auto-trigger', trigger); syncEditor(); }
  else if (!rulesOf().length) openEditor(null);
}

export function closeAutomations() {
  closeEditor();
  $('auto-drawer').hidden = true;
  refreshAutomationBadge();
  const target = typeof opener === 'function' ? opener() : opener;
  const back = target && target.isConnected && !target.hidden ? target : $('board-new-more');
  opener = null;
  back.focus();
}

export function toggleAutomations(from = null) {
  if (isOpen()) closeAutomations(); else openAutomations({ from });
}

/* ---------- the "New session ▾" menu ---------- */
const menuItem = (label, opts = {}) => {
  const b = document.createElement('button');
  b.type = 'button';
  b.setAttribute('role', 'menuitem');
  b.textContent = label;
  if (opts.key) { const k = document.createElement('span'); k.className = 'ctx-key'; k.textContent = opts.key; b.appendChild(k); }
  if (opts.sub) { const h = document.createElement('span'); h.className = 'ctx-hint'; h.textContent = opts.sub; b.appendChild(h); }
  b.onclick = opts.run;
  return b;
};
const menuLabel = (text, sub = false) => {
  const d = document.createElement('div');
  d.className = 'ctx-label' + (sub ? ' ctx-sub' : '');
  d.textContent = text;
  return d;
};

export function showNewSessionMenu(anchor) {
  const menu = $('ctx');
  const close = () => { menu.style.display = 'none'; anchor.setAttribute('aria-expanded', 'false'); menu.onkeydown = null; };
  if (menu.style.display === 'block' && anchor.getAttribute('aria-expanded') === 'true') { close(); return; }
  const go = fn => () => { close(); fn(); };
  const p = activeProject();
  const tplCount = p ? (p.templates || []).length : 0;
  const ruleCount = rulesOf().length;
  const hasCmd = !!projectDefaults(p).cmd;
  menu.replaceChildren(
    menuItem(t('menu.newSessionNow'), { key: formatShortcut(ctx.settings?.shortcuts?.newSession), sub: p ? newSessionSummary(p) : '', run: go(() => newDefaultSession()) }),
    ...(hasCmd ? [menuItem(t('menu.newShellOnly'), { sub: newSessionSummary(p, { shellOnly: true }), run: go(() => newDefaultSession({ shellOnly: true })) })] : []),
    document.createElement('hr'),
    menuLabel(t('menu.autoHeading'), true),
    menuItem('◷ ' + t('menu.autoClock'), { run: go(() => openAutomations({ from: anchor, trigger: 'clock' })) }),
    menuItem('◇ ' + t('menu.autoSlack'), { run: go(() => openAutomations({ from: anchor, trigger: 'slack' })) }),
    document.createElement('hr'),
    menuLabel(t('menu.manageHeading')),
    menuItem('⚑ ' + t('menu.projectDefaults'), { sub: p ? projectDefaultsSummary(p) : '', run: go(() => openProjectDefaults(p && p.id, anchor)) }),
    menuItem('↻ ' + t('menu.automations'), { key: ruleCount ? formatNumber(ruleCount) : '', run: go(() => openAutomations({ from: anchor })) }),
    menuItem('◈ ' + t('menu.templates'), { key: tplCount ? formatNumber(tplCount) : '', run: go(() => openTemplates(anchor)) }),
  );
  menu.setAttribute('role', 'menu');
  menu.onclick = null;   // items carry their own handlers; the document click closes the rest
  const items = () => [...menu.querySelectorAll('button')];
  menu.onkeydown = event => {
    const list = items();
    const i = list.indexOf(document.activeElement);
    const move = j => { event.preventDefault(); list[(j + list.length) % list.length].focus(); };
    if (event.key === 'ArrowDown') move(i + 1);
    else if (event.key === 'ArrowUp') move(i - 1);
    else if (event.key === 'Home') move(0);
    else if (event.key === 'End') move(list.length - 1);
    else if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); anchor.focus(); }
  };
  const r = anchor.getBoundingClientRect();
  menu.style.display = 'block';
  menu.style.left = Math.min(r.left, innerWidth - menu.offsetWidth - 8) + 'px';
  menu.style.top = (r.bottom + 6) + 'px';
  anchor.setAttribute('aria-expanded', 'true');
  items()[0].focus();
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initAutomation() {
  $('board-auto').onclick = () => toggleAutomations($('board-auto'));
  $('board-new-more').onclick = e => { e.stopPropagation(); showNewSessionMenu($('board-new-more')); };
  $('auto-tpl-manage').onclick = () => openTemplates($('auto-tpl-manage'));
  $('auto-close').onclick = () => closeAutomations();
  $('auto-new').onclick = () => openEditor(null);
  $('auto-cancel').onclick = () => closeEditor();
  $('auto-save').onclick = async () => {
    const rule = readEditor();
    if (!rule) return;
    if (await saveRule(rule)) { closeEditor(); toast(t('automation.saved')); }
  };
  $('auto-unit').addEventListener('change', syncEditor);
  $('auto-trigger').querySelectorAll('button').forEach(b => {
    b.onclick = () => { segSet('auto-trigger', b.dataset.v); syncEditor(); };
  });
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
