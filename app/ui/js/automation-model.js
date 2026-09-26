// automation-model.js — the automations drawer's DOM-free half: what a rule
// and its runs READ as, how the editor's fields become a rule, how a saved
// rule joins the settings list, and a Slack badge rule's delivery approval.
//
// Approval (`approveRule` / `grantState`, backend twin scheduler/authority.rs):
// the user's explicit consent that Deck may send THIS version of the rule's
// template steps without a per-row send-now. It is content-addressed — the
// digest covers the rule's id, trigger, badge, project, directory, full
// command, template name, finish and reviewEach plus every step's SHA-256,
// class and the external-content acknowledgment — so editing any of them
// (or the template) leaves a `stale` approval that grants nothing until the
// user approves again; name, column and pause state are presentation. A step
// is `fixed` (the owner's text) or `bounded` (owner text with `{{msg.*}}`
// placeholders); bounded steps are approved only with `external`, the
// explicit acceptance that untrusted Slack text may reach the agent. The
// approval authorizes content, never readiness: a fresh agent, an input
// request or a checkpoint still pauses the run. Hashes only — no prompt text
// is copied into settings. automation.js owns the drawer's DOM and
// imports these; node tests exercise them directly
// (../test/automation-model.test.mjs). Keep this module free of
// document/window access and Tauri APIs; "now" comes in as an argument.
import { badgeTaken, hmToMin, INBOUND_BADGE_RE, INBOUND_PLACEHOLDERS, minToHM, nextScheduleSlot, normalizeTemplateStep } from './pure.js';
import { DEFAULT_GRACE_MIN, GRACE_CHOICES } from './settings-model.js';
import { formatNumber, t } from './i18n.js';
import { fmtClock } from './scheduler-model.js';

/* "15 min" / "3 h" / "the rest of the day" / "never" in the user's language */
export function graceText(minutes) {
  if (minutes === 0) return t('automation.grace.none');
  if (minutes >= 1440) return t('automation.grace.day');
  if (minutes % 60 === 0) return t('automation.grace.hours', { count: formatNumber(minutes / 60) });
  return t('automation.grace.minutes', { count: formatNumber(minutes) });
}

/* the grace select's values: the two choices plus, when a saved rule has
   another value (an older choice, a hand-edited settings file), that value
   too — an edit never silently rewrites it */
export const graceOptions = value => [...new Set([...GRACE_CHOICES, value])].sort((a, b) => a - b);

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

/* what a rule's trigger reads as on its head line */
export const triggerText = rule => (rule.source === 'clock'
  ? scheduleText(rule.schedule)
  : rule.source === 'channel' ? t('automation.trigger.channelSummary', { count: formatNumber(rule.channelIds.length) })
    : t('automation.trigger.badge', { badge: rule.badge }));

export const ruleLabel = rule => rule.name || (rule.source === 'clock' || rule.source === 'channel' ? rule.id : `:${rule.badge}:`);

/* one recorded run as a line: its outcome decides the mark and the words;
   `openable` says a running run still has a card to open */
export function runSummary(run, now = new Date()) {
  const started = fmtClock(run.started, now);
  if (run.outcome === 'running') {
    return { outcome: 'running', text: `● ${started} · ${t('automation.run.running')}`, openable: !!run.card };
  }
  if (run.outcome === 'closed') {
    const minutes = run.ended ? Math.max(1, Math.round((run.ended - run.started) / 60)) : null;
    return {
      outcome: 'closed',
      mark: '✓ ',
      text: `${started}${minutes ? ' · ' + t('automation.run.minutes', { count: formatNumber(minutes) }) : ''} · ${t('automation.run.closed')}`,
    };
  }
  return {
    outcome: 'skipped',
    text: `– ${started} · ${t('automation.run.skipped')}${run.reason ? ' (' + t(`automation.run.${run.reason}`) + ')' : ''}`,
  };
}

/* the key/value rows under a rule: target, command, template, inspection,
   finish, then the trigger's own facts (grace and next slot for a clock rule,
   the connection state for a Slack rule) */
export function ruleFacts(rule, { columnName = null, home = '', slackConnected = false, approval = 'none', nowSecs = Math.floor(Date.now() / 1000) } = {}) {
  const rows = [
    ['automation.kv.target', `${columnName || t('automation.missingTarget')} · ${rule.dir || home}`],
    ['automation.kv.cmd', rule.cmd || t('automation.shellOnly')],
    ['automation.kv.template', rule.template],
  ];
  if (rule.source !== 'channel') {
    rows.push(['queue.plan', t(rule.reviewEach ? 'queue.review.enabled' : 'queue.review.disabled')]);
    rows.push(['automation.kv.finish', t(rule.finish === 'close' ? 'automation.finish.close' : 'automation.finish.keep')]);
  }
  if (rule.source === 'clock') {
    const next = rule.enabled ? nextScheduleSlot(rule.schedule, nowSecs, rule.since) : null;
    rows.push(['automation.kv.grace', graceText(rule.graceMin ?? DEFAULT_GRACE_MIN)]);
    rows.push(['automation.kv.next', rule.enabled ? (next ? fmtClock(next, new Date(nowSecs * 1000)) : '—') : t('automation.paused')]);
  } else if (rule.source === 'channel') {
    rows.push(['automation.kv.scope', [...rule.channelIds, ...rule.senderUserIds, ...rule.senderBotIds].join(', ')]);
    rows.push(['automation.kv.idle', rule.idleMinutes === 0 ? t('automation.manualStop') : t('automation.idleValue', { count: formatNumber(rule.idleMinutes) })]);
  } else {
    rows.push(['automation.kv.connection', t(slackConnected ? 'automation.slackOn' : 'automation.slackOff')]);
    rows.push(['automation.kv.autoSend', approvalText(rule, approval)]);
  }
  return rows;
}

/* the runs that belong to a rule, newest first, at most four */
export const recentRuns = (runs, rule) => runs
  .filter(r => r.rule === (rule.source === 'clock' ? rule.id : rule.badge))
  .slice(-4)
  .reverse();

/* The editor's fields → a rule, or `{ error, focus, params }` naming the
   translation key of what is missing and the control to focus. `previous`
   is the rule being edited (null for a new one); `rules` are all inbound
   rules, for the one-badge-per-rule check; `genId` mints an id when the
   trigger changed or the rule is new. A clock rule's id doubles as its
   badge (the backend spells it lowercase) and its `since` moves to now when
   the schedule changed, so a slot earlier today never fires late. */
export function composeRule(fields, { previous = null, rules = [], projectId, genId, nowSecs = Math.floor(Date.now() / 1000) }) {
  const fail = (error, focus = null, params = {}) => ({ error, focus, params });
  const name = (fields.name || '').trim();
  if ([...name].length > 120) return fail('automation.longName', 'auto-name');
  if (!fields.columnId) return fail('automation.needsColumn', 'auto-column');
  if (!fields.template) return fail('automation.needsTemplate', 'auto-template');
  const shared = {
    projectId, columnId: fields.columnId,
    cmd: (fields.cmd || '').trim(), template: fields.template, dir: (fields.dir || '').trim(),
    name, enabled: previous ? previous.enabled : true,
    finish: fields.finish === 'keep' ? 'keep' : 'close',
    ...(fields.reviewEach ? { reviewEach: true } : {}),
  };
  if (fields.trigger === 'slack') {
    const badge = (fields.badge || '').trim().replace(/^:|:$/g, '');
    if (!INBOUND_BADGE_RE.test(badge)) return fail('automation.invalidBadge', 'auto-badge');
    const id = previous && previous.source === 'slack' ? previous.id : genId('R');
    if (badgeTaken(rules, badge, id)) return fail('automation.badgeTaken', 'auto-badge', { badge });
    return { rule: { id, source: 'slack', badge, ...shared, enabled: true } };
  }
  if (!name) return fail('automation.needsName', 'auto-name');
  const unit = fields.unit;
  const days = unit === 'week' ? fields.days : unit === 'month' ? [Number(fields.dayOfMonth)] : [];
  if (unit === 'week' && !days.length) return fail('automation.needsDays');
  if (!fields.time) return fail('automation.needsTime', 'auto-time');
  const id = previous && previous.source === 'clock' ? previous.id : genId('a');
  const schedule = { unit, days, minute: hmToMin(fields.time) };
  const changed = !previous || previous.source !== 'clock' || JSON.stringify(previous.schedule) !== JSON.stringify(schedule);
  return {
    rule: {
      id, source: 'clock', badge: id, ...shared, schedule,
      graceMin: Number(fields.graceMin),
      since: changed ? nowSecs : previous.since,
    },
  };
}

/* a saved rule replaces its own entry in place (or the entry under the id
   it had before a trigger change gave it a new one) and a new rule appends */
export function mergeRules(rules, rule, oldId = null) {
  const rest = rules.filter(r => r.id !== rule.id && r.id !== oldId);
  const at = rules.findIndex(r => r.id === rule.id || r.id === oldId);
  return at < 0 ? [...rest, rule] : [...rest.slice(0, at), rule, ...rest.slice(at)];
}

/* rules pointing at a project that is gone would fire into nothing forever:
   the rules that still have a project, or null when nothing changed */
export function liveRules(rules, projects) {
  const live = new Set(projects.map(p => p.id));
  const kept = rules.filter(r => live.has(r.projectId));
  return kept.length === rules.length ? null : kept;
}

/* ---------- delivery approval (Slack badge rules) ---------- */

const PLACEHOLDER = /\{\{\s*msg\.([a-z]+)\s*\}\}/g;

/* 'bounded' when the step pastes message content through a known
   placeholder (an unknown one stays literal text), else 'fixed' */
export const stepClass = step => ([...String(step).matchAll(PLACEHOLDER)]
  .some(match => INBOUND_PLACEHOLDERS.includes(match[1])) ? 'bounded' : 'fixed');

export const templateCarriesMessage = template => (template?.steps || []).some(step => stepClass(step) === 'bounded');

async function sha256(text) {
  const bytes = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return [...new Uint8Array(bytes)].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

/* the canonical manifest the digest covers — byte-identical to
   `grant_digest` in scheduler/authority.rs (test/fixtures/automation-grant.json) */
export const grantManifest = (rule, steps, classes, external) => JSON.stringify([
  'deck-automation-grant', 1, rule.id, rule.source, rule.badge, rule.projectId,
  rule.dir || '', rule.cmd || '', rule.template, rule.finish === 'close' ? 'close' : 'keep',
  rule.reviewEach === true, steps, classes, external === true,
]);

export const grantDigest = (rule, grant) => sha256(grantManifest(rule, grant.steps, grant.classes, grant.external));

async function templateGrant(template) {
  const normalized = (template?.steps || []).map(normalizeTemplateStep);
  return { steps: await Promise.all(normalized.map(sha256)), classes: normalized.map(stepClass) };
}

/* `rule` approved against `template`; `external` is the separate consent
   for bounded steps (meaningless, and stored false, without one) */
export async function approveRule(rule, template, { external = false } = {}) {
  const { steps, classes } = await templateGrant(template);
  const grant = { steps, classes, external: external === true && classes.includes('bounded') };
  return { ...rule, autoSend: { digest: await grantDigest(rule, grant), ...grant } };
}

/* 'none' (never approved) | 'valid' | 'stale' (the rule or its template
   changed since approval: grants nothing) */
export async function grantState(rule, template) {
  const grant = rule?.autoSend;
  if (!grant) return 'none';
  if (rule.source !== 'slack' || !template) return 'stale';
  const current = await templateGrant(template);
  if (JSON.stringify(current.steps) !== JSON.stringify(grant.steps)
    || JSON.stringify(current.classes) !== JSON.stringify(grant.classes)) return 'stale';
  return (await grantDigest(rule, grant)) === grant.digest ? 'valid' : 'stale';
}

/* the approval row of a Slack badge rule's facts, from its `grantState` */
export const approvalText = (rule, state) => t(state === 'valid'
  ? (rule.autoSend.external ? 'automation.autoSend.onExternal' : 'automation.autoSend.on')
  : state === 'stale' ? 'automation.autoSend.stale' : 'automation.autoSend.off');
