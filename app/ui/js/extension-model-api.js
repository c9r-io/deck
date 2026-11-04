// Model APIs Settings contribution: an OpenAI and an Anthropic API key, kept
// in the macOS Keychain by model_api.rs. They draft card titles
// (card_title.rs, on the user's click) and, behind their own switch, write
// turn summaries (turn_summary.rs); saving or checking a key only asks
// that provider for its model list. A key goes to
// the backend once and never comes back; the page shows whether one is
// stored and the last check of it in this run, or that this is unknown.
// The turn-summary switch is Claude Code's Stop hook itself
// (`turn_summary_status`/`turn_summary_set` in agent_status/hooks.rs): its
// state is read from the hooks file, and it needs those hooks on.
// One action per provider at a time; a newer status read wins.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const MODEL_API_PROVIDERS = Object.freeze(['openai', 'anthropic']);

const PANEL = `<div class="set-group" id="set-item-model-api" data-setting-id="model-api" tabindex="-1" role="group" aria-labelledby="set-item-model-api-title">
<h3 class="set-group-title" id="set-item-model-api-title" data-i18n="settings.modelApi">Model APIs</h3>
<div class="set-hint" data-i18n="settings.modelApiHint">API keys for OpenAI and Anthropic, kept in the macOS Keychain. They draft card titles: for a card that is not running Claude Code, “Draft title” sends the end of that card’s terminal to the provider stored here (Anthropic first). Saving or checking a key only asks that provider for its list of models.</div>
<div class="set-row"><label for="set-model-openai" data-i18n="settings.modelApiOpenAi">OpenAI API key</label><input type="password" id="set-model-openai" autocomplete="off" spellcheck="false" placeholder="sk-…"><button class="btn" type="button" id="set-model-openai-check" data-i18n="settings.modelApiCheck">Check</button><button class="btn" type="button" id="set-model-openai-clear" data-i18n="common.clear">Clear</button></div>
<div class="set-hint" id="set-model-openai-state" role="status" aria-live="polite"></div>
<div class="set-row"><label for="set-model-anthropic" data-i18n="settings.modelApiAnthropic">Anthropic API key</label><input type="password" id="set-model-anthropic" autocomplete="off" spellcheck="false" placeholder="sk-ant-…"><button class="btn" type="button" id="set-model-anthropic-check" data-i18n="settings.modelApiCheck">Check</button><button class="btn" type="button" id="set-model-anthropic-clear" data-i18n="common.clear">Clear</button></div>
<div class="set-hint" id="set-model-anthropic-state" role="status" aria-live="polite"></div>
<div class="set-row"><label for="set-model-summary" data-i18n="settings.turnSummary">Summarize turn endings</label><input type="checkbox" id="set-model-summary"></div>
<div class="set-hint" data-i18n="settings.turnSummaryHint">When Claude Code ends a turn, its last message goes once to the provider stored here (Anthropic first) and comes back as one line on the card and in Needs attention. A one-line message is shown as it is and sent nowhere. The switch changes Deck’s own Claude Code status hooks. Claude Code that is already running may need a restart to pick up the change.</div>
<div class="set-hint" id="set-model-summary-state" role="status" aria-live="polite"></div>
</div>
`;

/* the backend's closed words → what the user is told */
const ERRORS = Object.freeze({
  shape: 'error.modelApiShape', auth: 'error.modelApiAuth', permission: 'error.modelApiPermission',
  busy: 'error.modelApiBusy', network: 'error.modelApiNetwork', response: 'error.modelApiResponse',
  keychain: 'error.modelApiKeychain',
});
export const modelApiError = code => ERRORS[code] || 'error.modelApiFailed';

/* the switch's closed words → what the user is told */
const SUMMARY_ERRORS = Object.freeze({
  'hooks-off': 'settings.turnSummaryNeedsHooks', 'release-only': 'error.turnSummaryRelease',
});
export const turnSummaryError = code => SUMMARY_ERRORS[code] || 'error.turnSummaryFailed';

/* The switch's line: the change under way, what it needs, or its state
   (with no key stored only one-line messages show); unknown when the
   hooks file could not be read (`summary` null). */
export function turnSummaryLine(summary, keyStored, busy, { t }) {
  if (busy) return t('settings.turnSummaryChanging');
  if (!summary) return t('settings.readFailed');
  if (!summary.hooks) return t('settings.turnSummaryNeedsHooks');
  if (!summary.on) return t('settings.turnSummaryOff');
  return t(keyStored ? 'settings.turnSummaryOn' : 'settings.turnSummaryNoKey');
}

/* One provider's line: the step under way, or the stored key's state, or
   unknown when the status could not be read (`entry` null). */
export function modelApiLine(entry, busy, { t, formatDateTime }) {
  if (busy) return t('settings.modelApiChecking');
  if (!entry) return t('settings.readFailed');
  if (!entry.saved) return t('settings.modelApiNotSet');
  const check = entry.check;
  if (!check) return t('settings.modelApiSaved');
  const time = formatDateTime(check.at * 1000);
  return check.ok ? t('settings.modelApiValid', { time })
    : t('settings.modelApiInvalid', { time, reason: t(modelApiError(check.code)) });
}

function createController(host) {
  const { $, inv, t, toast, confirmDialog, formatDateTime } = host;
  const busy = new Set();
  let status = null; let reads = 0;
  let summary = null; let summaryBusy = false;

  function paint() {
    for (const id of MODEL_API_PROVIDERS) {
      const entry = status?.[id] || null;
      const working = busy.has(id);
      $(`set-model-${id}-state`).textContent = modelApiLine(entry, working, { t, formatDateTime });
      $(`set-model-${id}`).disabled = working || !status;
      $(`set-model-${id}-check`).disabled = working || !entry?.saved;
      $(`set-model-${id}-clear`).disabled = working || !entry?.saved;
    }
    const keyStored = MODEL_API_PROVIDERS.some(id => status?.[id]?.saved === true);
    const box = $('set-model-summary');
    box.checked = summary?.on === true;
    /* the switch is the Stop hook: without the status hooks there is none */
    box.disabled = summaryBusy || !summary || !summary.hooks;
    $('set-model-summary-state').textContent = turnSummaryLine(summary, keyStored, summaryBusy, { t });
  }
  async function render() {
    const read = ++reads;
    const [next, switched] = await Promise.all([
      inv('model_api_status').catch(() => null),
      inv('turn_summary_status').catch(() => null),
    ]);
    if (read !== reads) return;
    status = next && typeof next === 'object' ? next : null;
    summary = switched && typeof switched === 'object'
      ? { hooks: switched.hooks === true, on: switched.on === true } : null;
    paint();
  }
  /* the turn-summary switch: one change at a time, then a fresh read */
  async function setSummary(enable) {
    if (summaryBusy) return false;
    summaryBusy = true; paint();
    try {
      await inv('turn_summary_set', { enable });
      toast(t(enable ? 'settings.turnSummaryEnabled' : 'settings.turnSummaryDisabled'));
      return true;
    } catch (error) {
      toast(t(turnSummaryError(String(error))));
      return false;
    } finally { summaryBusy = false; await render(); }
  }
  /* one action of one provider, then a fresh read; a failure is said once
     (a refused save also says that nothing was stored) */
  async function act(id, call, done, failure = reason => reason) {
    if (busy.has(id)) return false;
    busy.add(id); paint();
    try { await call(); done?.(); return true; }
    catch (error) { toast(failure(t(modelApiError(String(error))))); return false; }
    finally { busy.delete(id); await render(); }
  }
  function save(id) {
    const box = $(`set-model-${id}`);
    const value = box.value.trim();
    if (!value) return Promise.resolve(true);
    return act(id, () => inv('model_api_set_key', { provider: id, value }), () => {
      if (box.value.trim() === value) box.value = '';
      toast(t('settings.modelApiStored'));
    }, reason => t('settings.modelApiNotStored', { reason }));
  }
  async function clear(id) {
    if (!(await confirmDialog(t('settings.modelApiClearConfirm')))) return false;
    return act(id, () => inv('model_api_set_key', { provider: id, value: '' }), () => toast(t('settings.modelApiCleared')));
  }
  const check = id => act(id, () => inv('model_api_check', { provider: id }));
  return {
    refresh: render, localeChanged: paint, isPending: () => busy.size > 0 || summaryBusy,
    save, clear, check, setSummary,
    mount() {
      for (const id of MODEL_API_PROVIDERS) {
        $(`set-model-${id}`).addEventListener('change', () => save(id));
        $(`set-model-${id}-check`).onclick = () => check(id);
        $(`set-model-${id}-clear`).onclick = () => clear(id);
      }
      const box = $('set-model-summary');
      box.addEventListener('change', () => setSummary(box.checked));
    },
  };
}

export const CONTRIBUTION = Object.freeze({
  id: 'model-api',
  slots: Object.freeze([Object.freeze({ slotId: 'set-extension-integrations', html: PANEL })]),
  settings: Object.freeze([Object.freeze({
    id: 'model-api', section: 'integrations', order: 1,
    labelKeys: ['settings.modelApi', 'settings.modelApiOpenAi', 'settings.modelApiAnthropic', 'settings.turnSummary'],
    keywords: ['openai', 'anthropic', 'claude', 'gpt', 'llm', 'api key', 'keychain', 'summary', '模型', '大模型', '密钥', '钥匙串', '摘要'],
  })]),
  create: createController,
});
