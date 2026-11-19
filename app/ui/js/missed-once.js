// missed-once.js — one-time automations whose day passed without a run,
// under Needs attention
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - A one-time clock rule runs once, on its date; Deck starts it late that
//   same day if it was not running at the time. After that day nothing
//   starts it: this module lists it (automation-model.js `onceState` is
//   `missed`) until the user gives it a new date in the automations editor
//   or removes it. Nothing here runs a rule, and nothing is removed or
//   rescheduled without the user's click.
// - "Remove" deletes the rule through the one settings writer and then the
//   template only that rule used (`orphanedTemplates`), the same rule as
//   the drawer's "Clear ended".
// - The run list (`inbound_runs`) is read only as the fallback `onceState`
//   documents: a rule whose run record could not be written is not missed.
import { ctx, inv, listen, uev } from './state.js';
import { confirmDialog, toast } from './dialogs.js';
import { persistInbound } from './settings.js';
import { missedOnceRules, orphanedTemplates, ruleLabel, scheduleText } from './automation-model.js';
import { formatNumber, t } from './i18n.js';

let deps = { provider: null, refresh: () => {}, switchProject: () => {}, openRuleEditor: async () => {} };
let runs = [];
let started = null;

const allRules = () => [
  ...(ctx.settings?.inbound?.rules || []),
  ...(ctx.settings?.inbound?.channelRules || []).map(rule => ({ ...rule, source: 'channel' })),
];

export const missedNow = () => missedOnceRules(ctx.settings?.inbound?.rules, runs);

export const missedCountText = () => {
  const count = missedNow().length;
  return count ? t('automation.missed.countShort', { count: formatNumber(count) }) : '';
};

async function refreshRuns() {
  try { runs = await inv('inbound_runs'); } catch (_) { return; }
  deps.refresh();
}

const node = (tag, cls = '', text = '') => {
  const el = document.createElement(tag);
  if (cls) el.className = cls;
  if (text) el.textContent = text;
  return el;
};

async function remove(rule) {
  if (!(await confirmDialog(t('automation.deleteConfirm', { name: ruleLabel(rule) })))) return;
  const templates = orphanedTemplates(allRules(), [rule.id], rule.projectId);
  const current = ctx.settings.inbound;
  const ok = await persistInbound({ ...current, rules: current.rules.filter(r => r.id !== rule.id) });
  if (!ok) return;
  for (const name of templates) {
    try { await deps.provider.deleteTemplate(rule.projectId, name); } catch (_) { toast(t('error.templateSave')); }
  }
  uev('inbound', 'once-removed');
  deps.refresh();
}

/* one Needs attention row for a missed one-time automation */
export function missedRow(existing, rule) {
  const el = existing || (() => {
    const row = node('div', 'attention-row missed-row');
    row.append(node('span', 'attention-card-name'), node('span', 'attention-origin'), node('span', 'attention-reason'), node('span', 'attention-viewed'));
    const acts = node('span', 'missed-acts');
    const edit = node('button', 'btn missed-reschedule'); edit.type = 'button';
    const del = node('button', 'btn missed-remove'); del.type = 'button';
    acts.append(edit, del);
    row.append(acts);
    return row;
  })();
  el.querySelector('.attention-card-name').textContent = ruleLabel(rule);
  el.querySelector('.attention-origin').textContent = deps.provider?.project(rule.projectId)?.name || '—';
  el.querySelector('.attention-reason').textContent = t('automation.missed.reason', { when: scheduleText(rule.schedule) });
  const edit = el.querySelector('.missed-reschedule');
  edit.textContent = t('automation.missed.reschedule');
  edit.setAttribute('aria-label', t('automation.missed.rescheduleNamed', { name: ruleLabel(rule) }));
  edit.onclick = async () => {
    uev('inbound', 'once-rescheduled');
    deps.switchProject(rule.projectId);
    await deps.openRuleEditor(rule.id);
  };
  const del = el.querySelector('.missed-remove');
  del.textContent = t('automation.missed.remove');
  del.setAttribute('aria-label', t('automation.missed.removeNamed', { name: ruleLabel(rule) }));
  del.onclick = () => remove(rule);
  return el;
}

export function initMissedOnce(next) { deps = { ...deps, ...next }; }

/* the listener and the first pull; called once the Board is the user's */
export function startMissedOnce() {
  if (started) return started;
  started = listen('inbound-changed', refreshRuns)
    .catch(() => uev('listen-fail', 'inbound-changed'))
    .then(refreshRuns);
  return started;
}

export const missedOnceForTests = { setRuns: list => { runs = list; }, remove };
