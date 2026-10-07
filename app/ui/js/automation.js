// automation.js — 自动化: the project's automations drawer
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// C v01 reviewEach is explicit and applies only to newly dispatched runs;
// ordinary rules and existing runs keep their existing timing.
// An automation is an inbound rule (settings.json `inbound.rules`, validated
// by settings-model.js and inbound.rs alike) with one of two TRIGGERS: a
// clock (`source: 'clock'`, a schedule), Slack badge (`source: 'slack'`, the
// emoji name), or a scoped read-only Slack channel monitor. Badge rules are
// one automation per badge across
// every project, because the dispatcher matches a badge to ONE rule). Both
// share the rest of the shape — the column it creates cards in, directory,
// command, template and a finish mode — and both go through the same
// dispatch (inbound.js) and the same run ledger (`inbound_runs`), so a
// badge-started card is finished by `board.js` exactly like a clock run.
// Both Slack triggers carry other people's text, so a badge rule obeys the
// channel admission (`channelBlockReason`): the editor refuses to save one
// that fails it and the list shows a stored one as blocked.
// A Slack badge rule may carry the user's approval to send its steps
// automatically (automation-model.js `approveRule`): the editor's checkbox
// is ticked only while the stored approval is valid for the rule and its
// template, any edit to a field the approval covers unticks it (the user
// approves the new version by ticking again), and a template carrying
// `{{msg.*}}` needs the second, explicit external-content box for those
// steps. Saving unticked drops the approval, which also stops the unsent
// rows of runs that relied on it (scheduler/authority.rs). The list shows
// each Slack rule's approval as on / off / needs approval again.
// Clock and Slack badge rules share a separate, unticked-by-default box to send
// its FIRST step without agent readiness (automation-model.js
// `withFirstSend`, scheduler/first_send.rs): ticking it asks for an explicit
// confirmation of the startup-dialog risk, unticking is immediate, and it
// is independent of the approval box (which governs the later steps).
// Channel rules instead expose ONE native-backed first-step permission.
// Checkbox plus save accepts future scoped run heads, external template
// content and startup-input risk together; no per-event approval is added.
// Semantic edits use Save and update first-step permission. Ordinary saves
// preserve the grant; turning off never prompts. No later step or tool
// permission is authorized by this switch.
// This module is only the drawer that lists the CURRENT project's rules of
// either trigger, edits them through `persistInbound` (one durable settings
// write), and shows each rule's next slot (clock) or last runs. What a rule
// and its runs read as, how the editor's fields become a rule
// (`composeRule`) and how a saved rule joins the list (`mergeRules`) are
// DOM-free in automation-model.js, where node tests pin them. Firing is
// the backend sources + inbound.js; closing a finished run is board.js's
// poll. The Slack CONNECTION (its switch and tokens) stays in Settings: it
// is account-level, a rule is project-level. A rule whose project no longer
// exists is dropped on the next project change of the user's Board, whatever
// its trigger, so a deleted project never leaves a rule that fires into
// nothing. That begins with `startOrphanPruning`, which app.js calls once a
// Board loaded or after the lost Board's way out: against the placeholder of
// a failed load every rule looks orphaned, and one project event there (an
// operation that changed nothing announces one) would save settings without
// any of them. Weekday chips
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
// A badge rule's approval is shown from the rule first: one without
// `autoSend` reads as off in the list and unticked in the editor at once,
// whatever was computed for it earlier. The computed state (valid, stale)
// is kept with the rule and template steps it was checked against and read
// only for those; a computation that finishes after a newer render began is
// dropped. An approval not yet checked against the present rule is neither:
// the list says it is being checked, and the editor opens with the box held
// and waiting (indeterminate) and checks it for that editor alone. The
// answer settles the box only if the user decided nothing meanwhile (the
// box, or an edit to what the approval covers) and that editor is still
// open; Save pressed before the answer waits for it and goes on by itself.
// So an ordinary save never takes away an approval because it was not
// computed yet, and never renews one that turned out stale. A check that
// fails saves nothing until the user unticks the box or it can be checked.
// A tick stands for the steps it was made for, whether it carries a saved
// approval over or is the user's own: when the template's steps change
// while the editor is open the tick is withdrawn with a line saying so, and
// a save signs a new approval only from a tick the user made in that editor
// for the steps as they are. A carried-over approval goes back unchanged.
// Showing computes and writes nothing.
//
// INFORMATION LAYERS: the drawer's head states what an automation is and the
// one standing constraint; how runs behave is a named disclosure. In the
// editor every option carries its effect and its risk next to the control,
// always visible; mechanism and scope notes are named disclosures under it.
// Opening or closing one reads and writes nothing. The empty-state sentence
// is shown only while no rule exists AND no editor is open. A first-step box
// shows what it amounts to for the fields as they are (`syncFirstSendState`).
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
// A channel rule's automatic first step is bound to the verified Slack
// identity, which the native transport publishes only while a channel rule
// needs it. So the save that turns the option on asks for it first
// (`channelIdentityForSave`, native `slack_channel_prepare`): the wait is
// shown in the editor with the read fields held still, `pending` is retried
// for a bounded time, and the rule is saved with its permission in that same
// action. Giving the save up (below) drops a result arriving later; a wait
// that ends without an identity saves nothing and says so in the editor.
// Nothing is ever reported saved before the native save returned.
// A SAVE BELONGS TO THE EDIT IT WAS STARTED FROM. Pressing Save fixes, before
// anything is awaited, the edit (`editing`, a new object for every opened
// editor), the id of the rule that edit replaces, the rule read from the
// fields and the first-step choice (`saving`). Nothing after an await reads
// `editing` to decide what is replaced or permitted. Opening another rule,
// opening the editor for a new rule, Cancel, Escape and closing the drawer
// drop a save that has not been sent (`dropSave`): it writes nothing and
// asks for no permission, whenever its Slack answer arrives, and it is asked
// once more by the settings writer when its turn comes (`proceed`), so a
// save queued behind another write is withdrawn too. Until the native save
// answered that it was written, the save's rules are in no shared settings
// object, so no other save can write them in its place, whether it was
// given up or refused (settings.js). Losing focus or being
// covered drops nothing. A save that goes ahead is merged into the settings
// as they are then, so rules saved meanwhile stay, and a rule deleted while
// its save waited stays deleted. A dropped save gives back its own locks and
// its own native hold at the moment it is dropped and touches nothing later:
// not the next save's wait, message, locks, cancel handle or hold, and not
// the editor that replaced it. Once the native save has been sent it is not
// taken back; its result closes only the editor it came from. A list action
// (pause, delete) belongs to no edit and never replaces the rule in the
// editor.
// This module is a LEAF of the import graph: what it needs from board.js,
// layout.js and terminal.js arrives through `initAutomation(deps)`.
import { $, ctx, genId, inv, listen, state, store, uev } from './state.js';
import { confirmDialog, toast } from './dialogs.js';
import { persistInbound } from './settings.js';
import { minToHM, normalizeTemplateStep, projectDefaults, projectRules, ruleByOrigin, toggleClockRule } from './pure.js';
import { approveRule, composeRule, finishHintShown, firstSendNeedsConfirm, firstSendSupported, firstSendText, withFirstSend, graceOptions, graceText, grantDetail, liveRules, mergeRules, recentRuns, ruleFacts, ruleLabel, runSummary, templateCarriesMessage, triggerText } from './automation-model.js';
import { formatNumber, onLocaleChange, t } from './i18n.js';
import { formatShortcut } from './shortcuts.js';
import { DEFAULT_GRACE_MIN } from './settings-model.js';
import { openTemplates } from './templates.js';
import { awaitChannelIdentity, CHANNEL_IDLE_DEFAULT, channelBlockReason, channelFirstSendNeedsUpdate, channelFirstSendRecipe, normalizeChannelRule } from './channel-model.js';

/* the Board, layout and terminal actions this drawer calls, handed in by
   `initAutomation(deps)` so board.js and terminal.js may import this module
   without a cycle */
let activeProject, newSessionSummary, openProjectDefaults, projectDefaultsSummary, provider, openSession, newDefaultSession;

let opener = null;                 // element that opened the drawer; focus returns there

let editing = null;   // null | { id } (existing) | { id: null } (new); a new object per opened editor
let saving = null;    // the save in hand: { edit, replaced, existed, wait, hold, sent }
let runsCache = [];
let unsubscribe = null;
let channelAcceptanceChanged = false;
let channelLabelRevision = 0;

const ordinaryRules = () => ctx.settings?.inbound?.rules || [];
const channelRules = () => (ctx.settings?.inbound?.channelRules || []).map(rule => ({ ...rule, source: 'channel' }));
const allRules = () => [...ordinaryRules(), ...channelRules()];

export const rulesOf = (projectId = state.projectId) => projectRules(allRules(), projectId);

/* the automation behind a card's origin, whatever its trigger */
export const ruleOf = origin => origin?.source === 'channel'
  ? channelRules().find(rule => rule.id === origin.badge) || null : ruleByOrigin(ordinaryRules(), origin);

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

/* rule id → its approval state for the list, computed before a render
   (hashing is async), with the rule and template steps it was computed from
   (`key`). A state is read only for that same rule and steps (`approvalOf`):
   a rule without an approval is not approved whatever was computed earlier.
   A rule that holds one nothing was computed for yet reads as 'checking':
   not on, and not off either. 'unknown' is a computation that failed. */
let approvals = new Map();
let renderSeq = 0;
const approvalKey = rule => JSON.stringify([rule, ruleTemplate(rule)?.steps ?? null]);
function approvalOf(rule) {
  if (!rule?.autoSend) return 'none';
  const entry = approvals.get(rule.id);
  return entry && entry.key === approvalKey(rule) ? entry.detail : 'checking';
}

function ruleTemplate(rule) {
  const project = store.projects.find(p => p.id === rule.projectId);
  return (project?.templates || []).find(tp => tp.name === rule.template) || null;
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
  const blocked = rule.source !== 'clock' && channelBlockReason(rule, project);
  if (blocked) {
    el.classList.add('off');
    el.querySelector('.ar-when').textContent = t(blocked === 'command'
      ? 'automation.channelBlocked.command' : 'automation.channelBlocked.template');
  }
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
    await persistInbound(rule.source === 'channel'
      ? { ...ctx.settings.inbound, channelRules: ctx.settings.inbound.channelRules.filter(r => r.id !== rule.id) }
      : { ...ctx.settings.inbound, rules: ctx.settings.inbound.rules.filter(r => r.id !== rule.id) });
    renderAutomations();
  };
  const kv = el.querySelector('.ar-kv');
  const rows = ruleFacts(rule, { columnName: column ? column.name : null, home: ctx.HOME, slackConnected: slackConnected(),
    approval: approvalOf(rule) });
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
  const seq = ++renderSeq;
  paintAutomations();
  /* approval states hash asynchronously: repaint once, if they changed and
     no newer render took over */
  refreshApprovals(seq).then(changed => {
    if (changed && seq === renderSeq && isOpen()) paintAutomations();
  }).catch(() => {});
}

/* Compute the approval states for render `seq`. Only the newest render's
   result is kept: one that finishes after a newer render started was
   computed from rules as they were, and is dropped unpublished. */
async function refreshApprovals(seq) {
  const next = new Map();
  for (const rule of rulesOf().filter(r => r.source === 'slack')) {
    const key = approvalKey(rule);
    next.set(rule.id, { key, detail: await grantDetail(rule, ruleTemplate(rule)).catch(() => 'unknown') });
  }
  if (seq !== renderSeq) return false;
  const same = (a, b) => !!a && a.key === b.key && a.detail === b.detail;
  const changed = next.size !== approvals.size || [...next].some(([id, value]) => !same(approvals.get(id), value));
  approvals = next;
  return changed;
}

function paintAutomations() {
  const list = $('auto-list');
  list.innerHTML = '';
  const rules = rulesOf();
  /* the empty state teaches what an automation is; with the editor open the
     user is already making one, so it gives way until the editor closes */
  if (!rules.length && !editing) {
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
   others: every trigger-bound control carries a matching `q-p-*` class */
function syncEditor() {
  const trigger = segGet('auto-trigger');
  document.querySelectorAll('#auto-editor .q-p').forEach(el => {
    el.hidden = !el.classList.contains(`q-p-${trigger}`);
  });
  const unit = $('auto-unit').value;
  $('auto-days').hidden = trigger !== 'clock' || unit !== 'week';
  $('auto-dom').hidden = trigger !== 'clock' || unit !== 'month';
  $('auto-slack-state').textContent = t(slackConnected() ? 'automation.slackOn' : 'automation.slackOffHint');
  $('auto-capture-row').hidden = trigger !== 'channel' || $('auto-match-kind').value !== 'regex';
  syncFinishHint();
  syncFirstSendState();
  syncApproval();
  syncChannelFirstSend();
}

async function syncChannelFirstSend() {
  const revision = ++channelLabelRevision;
  const on = segGet('auto-trigger') === 'channel' && $('auto-channel-first-send').checked;
  const prior = editing?.id && channelRules().find(rule => rule.id === editing.id);
  const rule = on && readChannelFields(prior, activeProject(), 'preview');
  const changed = on && (channelAcceptanceChanged || !rule || await channelFirstSendNeedsUpdate(rule, prior, activeProject()));
  if (revision !== channelLabelRevision) return;
  const key = changed ? prior?.firstSendGrant
    ? 'automation.channelFirstSend.saveUpdate' : 'automation.channelFirstSend.saveEnable' : 'common.save';
  $('auto-save').dataset.i18n = key;
  $('auto-save').textContent = t(key);
}

/* automation-model.js finishHintShown */
function syncFinishHint() {
  $('auto-finish-hint').hidden = !finishHintShown(segGet('auto-trigger'), segGet('auto-finish'));
  $('auto-finish-keep-hint').hidden = !(segGet('auto-trigger') === 'clock' && segGet('auto-finish') === 'keep');
}

/* what the first-step box amounts to for the fields as they are: the same
   words the list shows (automation-model.js `firstSendText`), so an option
   ticked on a command it cannot reach never reads as in effect */
function syncFirstSendState() {
  const state = $('auto-first-send-state');
  const rule = { firstSendWithoutReadiness: $('auto-first-send').checked, cmd: $('auto-cmd').value.trim() };
  state.textContent = t('automation.current', { state: firstSendText(rule) });
  state.classList.toggle('warn', rule.firstSendWithoutReadiness && !firstSendSupported(rule.cmd));
}

/* the external-content box exists only for a template that carries message
   content, and only as an addition to the approval itself */
function syncApproval() {
  const project = activeProject();
  const template = (project?.templates || []).find(tp => tp.name === $('auto-template').value);
  const slack = segGet('auto-trigger') === 'slack';
  $('auto-send-external-row').hidden = !slack || !templateCarriesMessage(template);
  $('auto-send-external').disabled = !$('auto-send').checked;
  if (!$('auto-send').checked) $('auto-send-external').checked = false;
}

/* What the tick in the open editor stands for (`editing.tick`): the steps
   of the chosen template when it was made, and `stored`, the saved approval
   it carries over, or null when the user ticked the box in this editor. A
   tick is good only for those steps: `withdrawDrifted` takes it away when
   they changed (the template manager saved, renamed or deleted it), and a
   save signs nothing the user did not tick for. */
const stepsNow = () => {
  const template = (activeProject()?.templates || []).find(tp => tp.name === $('auto-template').value);
  return template ? JSON.stringify(template.steps.map(normalizeTemplateStep)) : null;
};
const tickNow = (stored = null) => ({ steps: stepsNow(), stored });
function withdrawDrifted() {
  if (!editing?.tick || editing.tick.steps === stepsNow()) return;
  editing.tick = null;
  $('auto-send').checked = false;
  $('auto-send-external').checked = false;
  approvalNote('automation.autoSend.templateChanged');
  syncApproval();
}

function approvalNote(key) {
  $('auto-send-check').hidden = !key;
  $('auto-send-check').textContent = key ? t(key) : '';
}

/* The stored approval of the rule `edit` opened, checked for that editor.
   Until the answer the box is the rule's own approval, shown as waiting.
   The answer settles the box only while that editor is open and the user
   has decided nothing (`decideApproval`): valid for the rule and template
   steps as they are now keeps the tick, anything else withdraws it, as an
   approval known to be stale opens. A check that fails settles nothing. */
function checkApproval(edit, rule) {
  const key = approvalKey(rule);
  const check = { settled: false };
  check.done = grantDetail(rule, ruleTemplate(rule)).then(detail => {
    if (check.settled || editing !== edit) return;
    check.settled = true;
    const current = key === approvalKey(rule);
    const valid = detail === 'valid' && current;
    $('auto-send').indeterminate = false;
    $('auto-send').checked = valid;
    $('auto-send-external').checked = valid && rule.autoSend.external === true;
    edit.tick = valid ? tickNow(rule.autoSend) : null;
    approvalNote(detail === 'valid' && !current ? 'automation.autoSend.templateChanged' : null);
    syncApproval();
  }, () => { if (!check.settled && editing === edit) approvalNote('automation.autoSend.checkFailed'); });
  return check;
}

/* the user decided about the approval (the box, or an edit to what it
   covers): a check still under way has nothing left to settle */
function decideApproval() {
  const check = editing?.approval;
  if (!check || check.settled) return;
  check.settled = true;
  $('auto-send').indeterminate = false;
  approvalNote(null);
}

/* an edit to anything the approval covers withdraws the tick: the user
   approves the edited version explicitly (or saves without approval) */
function withdrawApproval() {
  decideApproval();
  if (editing) editing.tick = null;
  approvalNote(null);
  if (!$('auto-send').checked && !$('auto-send-external').checked) return;
  $('auto-send').checked = false;
  $('auto-send-external').checked = false;
  syncApproval();
  toast(t('automation.autoSend.withdrawn'));
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
  dropSave();
  editing = { id: rule ? rule.id : null };
  channelAcceptanceChanged = false;
  const clock = !rule || rule.source === 'clock';
  const channel = rule?.source === 'channel';
  const schedule = clock && rule ? rule.schedule : { unit: 'day', days: [], minute: 540 };
  segSet('auto-trigger', channel ? 'channel' : clock ? 'clock' : 'slack');
  $('auto-name').value = rule ? rule.name : '';
  $('auto-badge').value = rule?.source === 'slack' ? rule.badge : '';
  $('auto-channel-ids').value = channel ? rule.channelIds.join(', ') : '';
  $('auto-sender-users').value = channel ? rule.senderUserIds.join(', ') : '';
  $('auto-sender-bots').value = channel ? rule.senderBotIds.join(', ') : '';
  $('auto-match-kind').value = channel ? rule.match.kind : 'contains';
  $('auto-match-value').value = channel ? (rule.match.kind === 'keywords' ? rule.match.keywords.join(', ') : rule.match.value || '') : '';
  $('auto-match-capture').value = channel ? rule.match.groupCapture || '' : '';
  $('auto-match-case').checked = channel && rule.match.caseSensitive === true;
  $('auto-threads').checked = !channel || rule.includeThreads !== false;
  $('auto-idle').value = channel ? rule.idleMinutes : CHANNEL_IDLE_DEFAULT;
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
  const approval = rule?.source === 'slack' ? approvalOf(rule) : 'none';
  /* an approval nothing was computed for yet is the rule's own until it is
     checked: shown as held and waiting, not as on and not as taken away */
  const unchecked = approval === 'checking' || approval === 'unknown';
  const approved = approval === 'valid' || unchecked;
  $('auto-send').checked = approved;
  $('auto-send').indeterminate = unchecked;
  $('auto-send-external').checked = approved && rule.autoSend.external === true;
  approvalNote(unchecked ? 'automation.autoSend.checking' : null);
  editing.approval = unchecked ? checkApproval(editing, rule) : null;
  editing.tick = approval === 'valid' ? tickNow(rule.autoSend) : null;
  $('auto-first-send').checked = ['slack', 'clock'].includes(rule?.source) && rule.firstSendWithoutReadiness === true;
  $('auto-channel-first-send').checked = channel && rule.firstSend === true && !!rule.firstSendGrant;
  channelVerifyStatus(null);
  syncEditor();
  $('auto-editor').hidden = false;
  if (isOpen()) paintAutomations();
  $(channel ? 'auto-channel-ids' : 'auto-name').focus();
}

function closeEditor() {
  dropSave();
  channelVerifyStatus(null);
  editing = null;
  $('auto-editor').hidden = true;
  if (isOpen()) paintAutomations();
}

function readChannelFields(previous, project, newId) {
  const split = value => value.split(',').map(part => part.trim()).filter(Boolean);
  const kind = $('auto-match-kind').value;
  return normalizeChannelRule({ id: previous?.source === 'channel' ? previous.id : newId, enabled: true,
    connectionId: 'default', channelIds: split($('auto-channel-ids').value),
    senderUserIds: split($('auto-sender-users').value), senderBotIds: split($('auto-sender-bots').value),
    match: { kind, ...(kind === 'keywords' ? { keywords: split($('auto-match-value').value) }
      : { value: $('auto-match-value').value.trim() }), caseSensitive: $('auto-match-case').checked,
      ...(kind === 'regex' ? { groupCapture: $('auto-match-capture').value.trim() } : {}) },
    includeThreads: $('auto-threads').checked, projectId: project?.id, columnId: $('auto-column').value,
    dir: $('auto-dir').value, cmd: $('auto-cmd').value, template: $('auto-template').value,
    idleMinutes: Number($('auto-idle').value) });
}

/* the editor → a rule (`composeRule`); null plus a toast when something is
   missing, with focus on the control that needs it */
function readEditor() {
  const project = activeProject();
  if (!project) return null;
  const previous = editing.id ? allRules().find(r => r.id === editing.id) || null : null;
  if (segGet('auto-trigger') === 'channel') {
    const rule = readChannelFields(previous, project, previous?.source === 'channel' ? previous.id : genId('R'));
    if (!rule) { toast(t('automation.invalidChannelRule')); return null; }
    const blocked = channelBlockReason(rule, project);
    if (blocked) {
      toast(t(blocked === 'command' ? 'automation.invalidChannelCommand' : 'automation.invalidChannelTemplate'));
      $(blocked === 'command' ? 'auto-cmd' : 'auto-template').focus();
      return null;
    }
    if ($('auto-channel-first-send').checked) {
      if (!firstSendSupported(rule.cmd) || !channelFirstSendRecipe(rule, project)) {
        toast(t('automation.channelFirstSend.unsupported')); return null;
      }
      rule.firstSend = true;
      if (previous?.firstSendGrant) rule.firstSendGrant = structuredClone(previous.firstSendGrant);
    }
    return { ...rule, source: 'channel' };
  }
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
  const blocked = read.rule.source === 'slack' && channelBlockReason(read.rule, project);
  if (blocked) {
    toast(t(blocked === 'command' ? 'automation.invalidChannelCommand' : 'automation.invalidChannelTemplate'));
    $(blocked === 'command' ? 'auto-cmd' : 'auto-template').focus();
    return null;
  }
  return read.rule;
}

/* What a save is to do about the approval, fixed when Save is pressed
   (nothing after an await reads the box or the template again): null for no
   approval; otherwise the template as it is now and either the saved
   approval the tick carries over (`stored`) or the user's own tick, with the
   message-content consent as ticked. A tick whose steps moved on is
   withdrawn first, so it is never signed for steps the user did not see. */
function approvalIntent(rule) {
  withdrawDrifted();
  if (rule.source !== 'slack' || !$('auto-send').checked || !editing?.tick) return null;
  const template = ruleTemplate(rule);
  if (!template) return null;
  return { template: structuredClone(template), stored: editing.tick.stored, external: $('auto-send-external').checked };
}

/* The rule to store. A saved approval that is carried over goes back as it
   was, and only while it still covers this rule and these steps; a new one
   is made only from the user's own tick; without either there is none (a
   stale one never survives a save). */
async function withApproval(rule, intent) {
  const plain = { ...rule };
  delete plain.autoSend;
  if (!intent) return plain;
  if (!intent.stored) return approveRule(plain, intent.template, { external: intent.external });
  const kept = { ...plain, autoSend: intent.stored };
  return (await grantDetail(kept, intent.template)) === 'valid' ? kept : plain;
}

/* The controls a waiting first-send save holds still: what was read from
   them is what will be saved, so they cannot change under the wait. */
const CHANNEL_SAVE_LOCKS = ['auto-save', 'auto-new', 'auto-channel-first-send', 'auto-channel-ids', 'auto-sender-users',
  'auto-sender-bots', 'auto-match-kind', 'auto-match-value', 'auto-match-capture', 'auto-match-case', 'auto-threads',
  'auto-idle', 'auto-dir', 'auto-cmd', 'auto-template', 'auto-column'];
const VERIFY_TEXT = { timeout: 'automation.channelFirstSend.verifyTimeout', blocked: 'automation.channelFirstSend.verifyBlocked',
  failed: 'automation.channelFirstSend.verifyFailed' };

function channelVerifyStatus(key) {
  const status = $('auto-channel-verify');
  status.hidden = !key;
  status.textContent = key ? t(key) : '';
}

/* The verified Slack identity for the save `op`, or null when the save does
   not go ahead (channel-model.js `awaitChannelIdentity`). The wait is shown
   in the editor and ends on its own; `dropSave` gives it up, switching
   windows does not. A wait that ends without an identity says so in the
   editor, where it stays until the next save, with every field as it was.
   A dropped wait already gave its locks back, so its answer changes nothing
   here: the controls and the message may belong to another save by then. */
async function channelIdentityForSave(op) {
  const wait = { canceled: false, locked: [...CHANNEL_SAVE_LOCKS.map($), ...$('auto-trigger').querySelectorAll('button')]
    .filter(control => !control.disabled) };
  op.wait = wait;
  op.hold = true;
  wait.locked.forEach(control => { control.disabled = true; });
  channelVerifyStatus('automation.channelFirstSend.verifying');
  const result = await awaitChannelIdentity({ prepare: () => inv('slack_channel_prepare'), canceled: () => wait.canceled,
    waiting: () => { if (!wait.canceled) channelVerifyStatus('automation.channelFirstSend.stillVerifying'); } });
  if (wait.canceled) return null;
  op.wait = null;
  wait.locked.forEach(control => { control.disabled = false; });
  channelVerifyStatus(result.identity ? null : VERIFY_TEXT[result.error]);
  return result.identity || null;
}

/* the native hold `op` asked for is released once, by `op` alone */
function releaseHold(op) {
  if (!op?.hold) return;
  op.hold = false;
  inv('slack_channel_prepare_cancel').catch(() => {});
}

/* The edit a save came from is replaced or given up. A save that has not
   been sent is dropped: nothing is saved or granted later, and what it held
   (the editor's controls, the native hold) is given back now. One already
   sent runs to its end and reports to nobody but its own editor. */
function dropSave() {
  const op = saving;
  if (!op) return;
  saving = null;
  if (op.sent) return;
  if (op.wait) {
    op.wait.canceled = true;
    op.wait.locked.forEach(control => { control.disabled = false; });
    op.wait = null;
  }
  releaseHold(op);
  toast(t('automation.channelFirstSend.verifyCanceled'));
}

/* Save `rule`. `op` is the editor's save in hand (null for a list action,
   which replaces nothing and belongs to no edit): what it replaces was fixed
   when Save was pressed, and it goes ahead only while it is still the save
   in hand. The settings it is merged into are the ones current when it is
   written, never a copy from before a wait. */
async function saveRule(rule, op = null) {
  const live = () => !op || saving === op;
  /* asked by the settings writer when this save's turn comes: from a yes on
     the native save is under way */
  const proceed = () => { if (!live()) return false; if (op) op.sent = true; return true; };
  const replaced = op ? op.replaced : null;
  try {
    let requests = [];
    if (rule.source === 'channel') {
      const previous = channelRules().find(value => value.id === rule.id);
      const authorize = rule.firstSend && (channelAcceptanceChanged || await channelFirstSendNeedsUpdate(rule, previous, activeProject()));
      if (!live()) return false;
      /* the same save prepares the identity its permission is bound to; with
         the connection up this answers at once */
      const identity = authorize ? await channelIdentityForSave(op) : null;
      if (!live() || (authorize && !identity)) return false;
      requests = authorize ? [{ ruleId: rule.id, external: true, identity }] : [];
    }
    /* deleted while this save waited: the later decision stands */
    if (op?.existed && !allRules().some(value => value.id === replaced)) {
      toast(t('automation.channelFirstSend.verifyCanceled'));
      return false;
    }
    const current = ctx.settings.inbound;
    let inbound;
    if (rule.source === 'channel') {
      const stored = { ...rule }; delete stored.source;
      inbound = { ...current, channelRules: [...current.channelRules.filter(value => value.id !== rule.id && value.id !== replaced), stored],
        rules: current.rules.filter(value => value.id !== replaced) };
    } else {
      /* a trigger change gives the rule a new id: the old entry goes */
      inbound = { ...current, rules: mergeRules(current.rules, rule, replaced),
        channelRules: current.channelRules.filter(value => value.id !== replaced) };
    }
    const ok = await persistInbound(inbound, requests, { proceed });
    renderAutomations();
    return ok;
  } finally { releaseHold(op); }
}

/* rules pointing at a project that is gone fire into nothing forever;
   drop them the moment the Board says so */
async function pruneOrphans() {
  const kept = liveRules(ordinaryRules(), store.projects);
  const channelKept = liveRules(ctx.settings.inbound.channelRules, store.projects);
  if (!kept && !channelKept) return;
  uev('inbound', 'rule-orphaned');
  await persistInbound({ ...ctx.settings.inbound, ...(kept ? { rules: kept } : {}), ...(channelKept ? { channelRules: channelKept } : {}) });
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
  if (trigger) {
    openEditor(null); segSet('auto-trigger', trigger); syncEditor();
    if (trigger === 'channel') $('auto-channel-ids').focus();
  }
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
    menuItem('▤ ' + t('menu.autoChannel'), { run: go(() => openAutomations({ from: anchor, trigger: 'channel' })) }),
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
   without a document; `deps` are the Board/layout/terminal actions above. */
export function initAutomation(deps) {
  ({ activeProject, newSessionSummary, openProjectDefaults, projectDefaultsSummary, provider, openSession, newDefaultSession } = deps);
  $('board-auto').onclick = () => toggleAutomations($('board-auto'));
  $('board-new-more').onclick = e => { e.stopPropagation(); showNewSessionMenu($('board-new-more')); };
  $('auto-tpl-manage').onclick = () => openTemplates($('auto-tpl-manage'));
  $('auto-close').onclick = () => closeAutomations();
  $('auto-new').onclick = () => openEditor(null);
  $('auto-cancel').onclick = () => closeEditor();
  $('auto-save').onclick = async () => {
    if (!editing || saving) return;
    const read = readEditor();
    if (!read) return;
    /* everything this save is about is fixed here, before the first await */
    const op = { edit: editing, replaced: editing.id, existed: !!editing.id && allRules().some(rule => rule.id === editing.id),
      wait: null, hold: false, sent: false };
    const firstSend = $('auto-first-send').checked;
    saving = op;
    try {
      /* an approval still being checked is neither kept nor dropped on a
         guess: the same press goes on once it is known */
      const check = op.edit.approval;
      if (check && !check.settled) {
        await check.done;
        if (saving !== op) return;
        if (!check.settled) { toast(t('automation.autoSend.checkFailedSave')); return; }
      }
      const rule = withFirstSend(await withApproval(read, approvalIntent(read)), firstSend);
      if (saving !== op) return;
      /* a save is reported in, and closes, only the editor it came from */
      if (await saveRule(rule, op) && editing === op.edit) { closeEditor(); toast(t('automation.saved')); }
    } finally { if (saving === op) saving = null; }
  };
  /* the user's own tick is for the steps as they are at that moment */
  const ticked = () => {
    decideApproval();
    editing.tick = $('auto-send').checked ? tickNow() : null;
    approvalNote(null);
  };
  $('auto-send').addEventListener('change', () => { if (editing) ticked(); syncApproval(); });
  $('auto-send-external').addEventListener('change', () => { if (editing) ticked(); });
  $('auto-channel-first-send').addEventListener('change', () => {
    channelAcceptanceChanged = $('auto-channel-first-send').checked;
    syncChannelFirstSend();
  });
  for (const id of ['auto-channel-ids', 'auto-sender-users', 'auto-sender-bots', 'auto-match-kind', 'auto-match-value',
    'auto-match-capture', 'auto-match-case', 'auto-threads', 'auto-idle', 'auto-dir', 'auto-cmd', 'auto-template', 'auto-column']) {
    $(id).addEventListener('input', syncChannelFirstSend);
    $(id).addEventListener('change', syncChannelFirstSend);
  }
  /* the first-send risk is accepted explicitly: the box stays unticked
     unless the confirmation is answered yes; unticking needs nothing */
  $('auto-first-send').addEventListener('change', async () => {
    const box = $('auto-first-send');
    if (!firstSendNeedsConfirm(false, box.checked)) return;
    box.checked = false;
    if (await confirmDialog(t('automation.firstSend.confirm'))) box.checked = true;
    syncFirstSendState();
  });
  $('auto-first-send').addEventListener('change', syncFirstSendState);
  $('auto-cmd').addEventListener('input', syncFirstSendState);
  for (const id of ['auto-badge', 'auto-dir', 'auto-cmd', 'auto-template', 'auto-review']) {
    $(id).addEventListener(id === 'auto-template' || id === 'auto-review' ? 'change' : 'input', withdrawApproval);
  }
  $('auto-template').addEventListener('change', syncApproval);
  $('auto-unit').addEventListener('change', syncEditor);
  $('auto-match-kind').addEventListener('change', syncEditor);
  $('auto-trigger').querySelectorAll('button').forEach(b => {
    b.onclick = () => {
      if (segGet('auto-trigger') !== b.dataset.v) withdrawApproval();
      segSet('auto-trigger', b.dataset.v); syncEditor();
    };
  });
  $('auto-finish').querySelectorAll('button').forEach(b => {
    b.onclick = () => {
      if (segGet('auto-finish') !== b.dataset.v) withdrawApproval();
      segSet('auto-finish', b.dataset.v);
      syncFinishHint();
    };
  });
  $('auto-drawer').addEventListener('keydown', event => {
    if (event.key !== 'Escape') return;
    event.preventDefault();
    if (editing) closeEditor(); else closeAutomations();
  });
  buildDayControls();
  onLocaleChange(() => { buildDayControls(); syncFirstSendState(); renderAutomations(); });
  unsubscribe = provider.subscribe(ev => {
    if (ev === 'projects') withdrawDrifted();
    if (ev === 'projects' || ev === 'list' || ev === 'channel-authority') renderAutomations();
  });
  listen('inbound-changed', async () => { await refreshRuns(); renderAutomations(); })
    .catch(() => uev('listen-fail', 'inbound-changed'));
}

/* Orphaned rules are dropped from here on: app.js calls this once the
   webview holds the user's Board (a Board that loaded, or the lost Board's
   way out), never for the placeholder of a failed load. */
export function startOrphanPruning() {
  provider.subscribe(ev => { if (ev === 'projects') pruneOrphans(); });
}

export const stopAutomation = () => { if (unsubscribe) { unsubscribe(); unsubscribe = null; } };
