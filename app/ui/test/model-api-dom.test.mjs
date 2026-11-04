// The Model APIs Settings contribution driven through its real controller
// over the shared fake document: what each provider's line says, that a key
// goes to the backend once and never comes back into the page, one action per
// provider at a time, and that a newer status read wins over an older one.
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument, ids } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };
const { CONTRIBUTION, MODEL_API_PROVIDERS, modelApiError, modelApiLine, turnSummaryError, turnSummaryLine } = await import('../js/extension-model-api.js');
const { t } = await import('../js/i18n.js');

const $ = id => fakeDocument.getElementById(id);
const tick = () => new Promise(resolve => setImmediate(resolve));
const formatDateTime = ms => `T${ms / 1000}`;

/* a controller mounts once in the app; each test gets fresh elements so an
   earlier test's listeners cannot answer for this one */
function harness() {
  for (const id of MODEL_API_PROVIDERS) for (const suffix of ['', '-check', '-clear', '-state']) ids.delete(`set-model-${id}${suffix}`);
  for (const id of ['set-model-summary', 'set-model-summary-state']) ids.delete(id);
  const calls = []; const toasts = []; const answers = {};
  let confirm = true;
  const inv = async (name, args) => {
    calls.push({ name, args });
    const answer = answers[name];
    return typeof answer === 'function' ? answer(args) : answer;
  };
  const host = { $, inv, t, toast: message => toasts.push(message), confirmDialog: async () => confirm, formatDateTime };
  const controller = CONTRIBUTION.create(host);
  controller.mount();
  return { controller, calls, toasts, answers, setConfirm: value => { confirm = value; } };
}

const status = (openai, anthropic = { saved: false, check: null }) => ({ openai, anthropic });

test('each line says the stored key’s state, the step under way, or that it is unknown', () => {
  const env = { t, formatDateTime };
  assert.equal(modelApiLine(null, false, env), t('settings.readFailed'));
  assert.equal(modelApiLine({ saved: false, check: null }, false, env), t('settings.modelApiNotSet'));
  assert.equal(modelApiLine({ saved: true, check: null }, false, env), t('settings.modelApiSaved'));
  assert.equal(modelApiLine({ saved: true, check: { ok: true, code: null, at: 60 } }, false, env),
    t('settings.modelApiValid', { time: 'T60' }));
  assert.equal(modelApiLine({ saved: true, check: { ok: false, code: 'auth', at: 60 } }, false, env),
    t('settings.modelApiInvalid', { time: 'T60', reason: t('error.modelApiAuth') }));
  assert.equal(modelApiLine({ saved: true, check: null }, true, env), t('settings.modelApiChecking'));
  // every closed word of model_api.rs has its sentence; anything else is the generic one
  for (const code of ['shape', 'auth', 'permission', 'busy', 'network', 'response', 'keychain']) {
    assert.notEqual(t(modelApiError(code)), '', code);
    assert.notEqual(modelApiError(code), 'error.modelApiFailed', code);
  }
  for (const code of ['missing', 'provider', 'worker', 'Error: x']) assert.equal(modelApiError(code), 'error.modelApiFailed');
  assert.deepEqual(MODEL_API_PROVIDERS, ['openai', 'anthropic']);
});

test('a refresh paints both providers, and an unreadable status is unknown with nothing to act on', async () => {
  const { controller, answers } = harness();
  answers.model_api_status = status({ saved: true, check: { ok: true, code: null, at: 120 } });
  await controller.refresh();
  assert.equal($('set-model-openai-state').textContent, t('settings.modelApiValid', { time: 'T120' }));
  assert.equal($('set-model-openai-check').disabled, false);
  assert.equal($('set-model-openai-clear').disabled, false);
  assert.equal($('set-model-anthropic-state').textContent, t('settings.modelApiNotSet'));
  assert.equal($('set-model-anthropic-check').disabled, true, 'nothing stored, nothing to check');
  assert.equal($('set-model-anthropic-clear').disabled, true);
  assert.equal($('set-model-anthropic').disabled, false);
  answers.model_api_status = () => { throw new Error('ipc'); };
  await controller.refresh();
  for (const id of MODEL_API_PROVIDERS) {
    assert.equal($(`set-model-${id}-state`).textContent, t('settings.readFailed'));
    assert.equal($(`set-model-${id}`).disabled, true);
    assert.equal($(`set-model-${id}-check`).disabled, true);
  }
});

test('a saved key goes to the backend once, trimmed, and leaves the page; a refusal keeps the draft and says why', async () => {
  const { calls, toasts, answers } = harness();
  answers.model_api_status = status({ saved: false, check: null });
  let release; answers.model_api_set_key = () => new Promise(resolve => { release = resolve; });
  $('set-model-openai').value = '  sk-proj-abc  ';
  $('set-model-openai').fire('change');
  await tick();
  assert.equal($('set-model-openai-state').textContent, t('settings.modelApiChecking'));
  assert.equal($('set-model-openai').disabled, true);
  $('set-model-openai').fire('change');   // a second save of the same provider while one runs
  release(); await tick(); await tick();
  const saves = calls.filter(call => call.name === 'model_api_set_key');
  assert.deepEqual(saves, [{ name: 'model_api_set_key', args: { provider: 'openai', value: 'sk-proj-abc' } }]);
  assert.equal($('set-model-openai').value, '', 'the key leaves the page');
  assert.equal(toasts.at(-1), t('settings.modelApiStored'));
  // refused: the draft stays, nothing is said to be stored
  answers.model_api_set_key = () => Promise.reject('auth');
  $('set-model-anthropic').value = 'sk-ant-api03-bad';
  $('set-model-anthropic').fire('change');
  await tick(); await tick();
  assert.equal($('set-model-anthropic').value, 'sk-ant-api03-bad');
  assert.equal(toasts.at(-1), t('settings.modelApiNotStored', { reason: t('error.modelApiAuth') }));
  // an empty field saves nothing
  const before = calls.length;
  $('set-model-anthropic').value = '   ';
  $('set-model-anthropic').fire('change');
  await tick();
  assert.equal(calls.length, before);
  // no status answer ever carries a key into the page
  for (const call of calls.filter(entry => entry.name === 'model_api_status')) assert.equal(call.args, undefined);
});

test('clearing asks first and sends an empty key; a check names what the provider said', async () => {
  const { calls, toasts, answers, setConfirm } = harness();
  answers.model_api_status = status({ saved: true, check: null }, { saved: true, check: null });
  answers.model_api_set_key = () => undefined;
  setConfirm(false);
  $('set-model-anthropic-clear').fire('click'); await tick(); await tick();
  assert.equal(calls.filter(call => call.name === 'model_api_set_key').length, 0, 'declined: nothing removed');
  setConfirm(true);
  $('set-model-anthropic-clear').fire('click'); await tick(); await tick();
  assert.deepEqual(calls.filter(call => call.name === 'model_api_set_key').map(call => call.args), [{ provider: 'anthropic', value: '' }]);
  assert.equal(toasts.at(-1), t('settings.modelApiCleared'));
  answers.model_api_check = () => Promise.reject('busy');
  $('set-model-openai-check').fire('click'); await tick(); await tick();
  assert.deepEqual(calls.filter(call => call.name === 'model_api_check').map(call => call.args), [{ provider: 'openai' }]);
  assert.equal(toasts.at(-1), t('error.modelApiBusy'));
});

test('an older status read never paints over a newer one', async () => {
  const { controller, answers } = harness();
  const pending = [];
  answers.model_api_status = () => new Promise(resolve => pending.push(resolve));
  const first = controller.refresh(); const second = controller.refresh();
  pending[1](status({ saved: true, check: null }));
  await second;
  pending[0](status({ saved: false, check: null }));
  await first;
  assert.equal($('set-model-openai-state').textContent, t('settings.modelApiSaved'));
});

test('the turn-summary line says what the switch needs, its state, and that it is unknown', () => {
  const env = { t };
  assert.equal(turnSummaryLine(null, true, false, env), t('settings.readFailed'));
  assert.equal(turnSummaryLine({ hooks: false, on: false }, true, false, env), t('settings.turnSummaryNeedsHooks'));
  assert.equal(turnSummaryLine({ hooks: true, on: false }, true, false, env), t('settings.turnSummaryOff'));
  assert.equal(turnSummaryLine({ hooks: true, on: true }, true, false, env), t('settings.turnSummaryOn'));
  assert.equal(turnSummaryLine({ hooks: true, on: true }, false, false, env), t('settings.turnSummaryNoKey'), 'no key: one-line messages only');
  assert.equal(turnSummaryLine({ hooks: true, on: true }, true, true, env), t('settings.turnSummaryChanging'));
  assert.equal(turnSummaryError('hooks-off'), 'settings.turnSummaryNeedsHooks');
  assert.equal(turnSummaryError('release-only'), 'error.turnSummaryRelease');
  for (const code of ['write', 'unreadable', 'Error: x']) assert.equal(turnSummaryError(code), 'error.turnSummaryFailed');
});

test('the switch is read from the hooks, turned by the backend, and offered only with the hooks on', async () => {
  const { controller, calls, toasts, answers } = harness();
  answers.model_api_status = status({ saved: false, check: null }, { saved: true, check: null });
  answers.turn_summary_status = { hooks: true, on: false };
  await controller.refresh();
  const box = $('set-model-summary');
  assert.equal(box.checked, false);
  assert.equal(box.disabled, false);
  assert.equal($('set-model-summary-state').textContent, t('settings.turnSummaryOff'));
  // on: one call, then the state read back from the file
  answers.turn_summary_set = () => { answers.turn_summary_status = { hooks: true, on: true }; };
  box.checked = true; box.fire('change');
  await tick(); await tick(); await tick();
  assert.deepEqual(calls.filter(call => call.name === 'turn_summary_set').map(call => call.args), [{ enable: true }]);
  assert.equal(toasts.at(-1), t('settings.turnSummaryEnabled'));
  assert.equal(box.checked, true);
  assert.equal($('set-model-summary-state').textContent, t('settings.turnSummaryOn'));
  // a refusal says why, and the box shows the file's state again
  answers.turn_summary_set = () => Promise.reject('release-only');
  box.checked = false; box.fire('change');
  await tick(); await tick(); await tick();
  assert.equal(toasts.at(-1), t('error.turnSummaryRelease'));
  assert.equal(box.checked, true, 'still on: the file says so');
  // without the hooks, turning on is not offered
  answers.turn_summary_status = { hooks: false, on: false };
  await controller.refresh();
  assert.equal(box.disabled, true);
  assert.equal($('set-model-summary-state').textContent, t('settings.turnSummaryNeedsHooks'));
  // an unreadable file: unknown, nothing to act on
  answers.turn_summary_status = () => { throw new Error('ipc'); };
  await controller.refresh();
  assert.equal(box.disabled, true);
  assert.equal($('set-model-summary-state').textContent, t('settings.readFailed'));
});
