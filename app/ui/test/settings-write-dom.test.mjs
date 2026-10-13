// Settings writes are isolated edits over the last durable file. These tests
// run the production module and DOM handlers; only the native settings file is
// simulated, with each write held at a separately controlled promise.
import test, { afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { FakeElement, fakeDocument } from './fixtures/dom-fixture.mjs';

fakeDocument.querySelectorAll = () => [];
fakeDocument.documentElement = new FakeElement('html');
fakeDocument.documentElement.style.setProperty = function setProperty(name, value) { this[name] = value; };
globalThis.document = fakeDocument;
globalThis.window = {
  __TAURI__: null,
  dispatchEvent() {},
  matchMedia: () => ({ matches: true, addEventListener() {}, removeEventListener() {} }),
};

const { ctx } = await import('../js/state.js');
const { getLocale, t } = await import('../js/i18n.js');
const { formatShortcut } = await import('../js/shortcuts.js');
const { normalizeSettings, serializeSettings } = await import('../js/settings-model.js');
const {
  initSettings, loadSettings, persistThemeChoice, setFontScale, setShortcut,
} = await import('../js/settings.js');

const get = id => fakeDocument.getElementById(id);
const deferred = () => {
  let resolve;
  const promise = new Promise(done => { resolve = done; });
  return { promise, resolve };
};
const turn = async (times = 3) => {
  for (let index = 0; index < times; index += 1) await new Promise(resolve => setImmediate(resolve));
};
const livePlans = new Set();
const regression = (name, fn) => test(name, { timeout: 5000 }, fn);

afterEach(async () => {
  for (const plan of livePlans) plan.release.resolve();
  await turn(6);
  livePlans.clear();
});

initSettings();

function plannedSave(ok) {
  return { ok, started: deferred(), release: deferred() };
}

async function scene(outcomes, extension = {}, nativeOutcomes = []) {
  const initial = normalizeSettings({
    editor: 'Visual Studio Code', locale: 'en', theme: 'deck-dark', accent: 'teal', fontScale: 1,
    shortcuts: { toggleSidebar: 'Meta+KeyB', splitRight: 'Meta+KeyD', futureAction: 'Meta+F12' },
    futureExtension: { revision: 7, mode: 'keep' },
    ...extension,
  });
  const disk = { data: serializeSettings(initial), writes: [] };
  const plans = outcomes.map(plannedSave);
  const nativePlans = nativeOutcomes.map(plannedSave);
  const nativeQueue = [...nativePlans];
  for (const plan of [...plans, ...nativePlans]) livePlans.add(plan);
  const calls = [];
  let nativeArmed = false;
  window.__TAURI__ = { event: { listen: async () => () => {} }, core: { invoke: async (command, args) => {
    calls.push({ command, args: structuredClone(args) });
    if (command === 'load_settings') return { data: disk.data };
    if (command === 'set_native_locale' && nativeArmed && nativeQueue.length) {
      const plan = nativeQueue.shift();
      plan.started.resolve();
      await plan.release.promise;
      if (!plan.ok) throw new Error('mock native locale update failed');
      return null;
    }
    if (command !== 'save_settings') return null;
    const plan = plans[disk.writes.length];
    assert.ok(plan, `unexpected settings write ${disk.writes.length + 1}`);
    const write = { args: structuredClone(args), landed: false };
    disk.writes.push(write);
    plan.started.resolve();
    await plan.release.promise;
    if (!plan.ok) throw new Error('mock settings write failed');
    disk.data = args.data;
    write.landed = true;
    return disk.data;
  } } };
  await loadSettings();
  await turn();
  calls.length = 0;
  get('toasts').replaceChildren();
  nativeArmed = true;
  get('set-theme').value = ctx.settings.theme;
  get('set-accent').value = ctx.settings.accent;
  get('set-editor').value = ctx.settings.editor;
  get('set-locale').value = ctx.settings.locale;
  return { initial, disk, plans, nativePlans, calls };
}

const written = write => JSON.parse(write.args.data);
const durable = disk => JSON.parse(disk.data);
const finish = plan => plan.release.resolve();
const capture = action => get('set-shortcuts').children
  .flatMap(row => row.children).find(element => element.dataset.action === action);

for (const themeOk of [true, false]) {
  for (const fontOk of [true, false]) {
    regression(`theme ${themeOk ? 'success' : 'failure'} followed by font ${fontOk ? 'success' : 'failure'} stays isolated`, async () => {
      const { disk, plans } = await scene([themeOk, fontOk]);
      get('set-theme').value = 'light';
      get('set-accent').value = 'blue';
      const theme = persistThemeChoice();
      await plans[0].started.promise;
      const font = setFontScale(1.2);

      assert.deepEqual([ctx.settings.theme, ctx.settings.accent, ctx.settings.fontScale], ['light', 'blue', 1.2]);
      assert.deepEqual([document.documentElement.dataset.theme, document.documentElement.dataset.accent], ['light', 'blue']);
      assert.equal(get('set-font-value').textContent, '120%');

      finish(plans[0]);
      await theme;
      await plans[1].started.promise;
      assert.equal(ctx.settings.fontScale, 1.2, 'the later pending font preview survives the theme settlement');
      assert.equal(get('set-font-value').textContent, '120%');
      assert.equal(written(disk.writes[1]).theme, themeOk ? 'light' : 'deck-dark',
        'the font write carries only a theme edit that actually landed');
      finish(plans[1]);
      await font;

      const expected = [themeOk ? 'light' : 'deck-dark', themeOk ? 'blue' : 'teal', fontOk ? 1.2 : 1];
      assert.deepEqual([ctx.settings.theme, ctx.settings.accent, ctx.settings.fontScale], expected);
      assert.deepEqual([get('set-theme').value, get('set-accent').value], expected.slice(0, 2));
      assert.equal(get('set-font-value').textContent, `${Math.round(expected[2] * 100)}%`);
      assert.deepEqual([durable(disk).theme, durable(disk).accent, durable(disk).fontScale], expected);
    });
  }
}

for (const firstOk of [true, false]) {
  for (const secondOk of [true, false]) {
    regression(`two font edits: first ${firstOk ? 'succeeds' : 'fails'}, second ${secondOk ? 'succeeds' : 'fails'}`, async () => {
      const { disk, plans } = await scene([firstOk, secondOk]);
      const first = setFontScale(1.2);
      await plans[0].started.promise;
      const second = setFontScale(1.4);
      assert.equal(ctx.settings.fontScale, 1.4);
      assert.equal(get('set-font-value').textContent, '140%');

      finish(plans[0]);
      await first;
      await plans[1].started.promise;
      assert.equal(ctx.settings.fontScale, 1.4, 'the newer pending preview survives the older settlement');
      assert.equal(get('set-font-value').textContent, '140%');
      assert.equal(written(disk.writes[1]).fontScale, 1.4);
      finish(plans[1]);
      await second;

      const expected = secondOk ? 1.4 : firstOk ? 1.2 : 1;
      assert.equal(ctx.settings.fontScale, expected);
      assert.equal(get('set-font-value').textContent, `${Math.round(expected * 100)}%`);
      assert.equal(durable(disk).fontScale, expected);
    });
  }
}

for (const firstOk of [true, false]) {
  for (const secondOk of [true, false]) {
    regression(`distinct shortcuts: first ${firstOk ? 'succeeds' : 'fails'}, second ${secondOk ? 'succeeds' : 'fails'}`, async () => {
      const { disk, plans } = await scene([firstOk, secondOk]);
      const first = setShortcut('toggleSidebar', 'Meta+KeyJ');
      await plans[0].started.promise;
      const second = setShortcut('splitRight', 'Meta+KeyK');
      finish(plans[0]);
      await first;
      await plans[1].started.promise;

      assert.equal(ctx.settings.shortcuts.splitRight, 'Meta+KeyK');
      assert.equal(capture('splitRight').textContent, formatShortcut('Meta+KeyK'));
      assert.equal(written(disk.writes[1]).shortcuts.toggleSidebar, firstOk ? 'Meta+KeyJ' : 'Meta+KeyB',
        'the second action cannot carry the first action after its save failed');
      if (!firstOk) assert.ok(get('toasts').children.some(el => el.textContent === t('error.shortcutSave')),
        'an edit of another shortcut does not hide this action\'s failure');
      finish(plans[1]);
      await second;

      const expectedFirst = firstOk ? 'Meta+KeyJ' : 'Meta+KeyB';
      const expectedSecond = secondOk ? 'Meta+KeyK' : 'Meta+KeyD';
      assert.deepEqual(
        [ctx.settings.shortcuts.toggleSidebar, ctx.settings.shortcuts.splitRight],
        [expectedFirst, expectedSecond],
      );
      assert.deepEqual(
        [capture('toggleSidebar').textContent, capture('splitRight').textContent],
        [formatShortcut(expectedFirst), formatShortcut(expectedSecond)],
      );
      assert.deepEqual(
        [durable(disk).shortcuts.toggleSidebar, durable(disk).shortcuts.splitRight],
        [expectedFirst, expectedSecond],
      );
    });
  }
}

regression('a failed editor handler restores the setting and select', async () => {
  const { disk, plans } = await scene([false]);
  get('set-editor').value = 'Cursor';
  const saving = get('set-editor').onchange();
  await plans[0].started.promise;
  assert.equal(ctx.settings.editor, 'Cursor');
  finish(plans[0]);
  await saving;
  assert.equal(ctx.settings.editor, 'Visual Studio Code');
  assert.equal(get('set-editor').value, 'Visual Studio Code');
  assert.equal(durable(disk).editor, 'Visual Studio Code');
});

for (const ok of [false, true]) {
  regression(`a locale handler ${ok ? 'notifies native only after its save lands' : 'restores locale without native precommit'}`, async () => {
    const { disk, plans, calls } = await scene([ok]);
    get('set-locale').value = 'zh-Hans';
    const saving = get('set-locale').onchange();
    await plans[0].started.promise;
    assert.equal(ctx.settings.locale, 'zh-Hans');
    assert.equal(get('set-locale').value, 'zh-Hans');
    assert.equal(getLocale(), 'zh-Hans');
    assert.equal(calls.filter(call => call.command === 'set_native_locale').length, 0,
      'native locale is not changed before durability');

    finish(plans[0]);
    await saving;
    await turn();
    const expected = ok ? 'zh-Hans' : 'en';
    assert.equal(ctx.settings.locale, expected);
    assert.equal(get('set-locale').value, expected);
    assert.equal(getLocale(), expected);
    assert.equal(durable(disk).locale, expected);
    const native = calls.filter(call => call.command === 'set_native_locale');
    assert.equal(native.length, ok ? 1 : 0);
    if (ok) assert.equal(native[0].args.locale, 'zh-Hans');
  });
}

regression('a native locale failure does not roll back a locale already written to settings', async () => {
  const { disk, plans, nativePlans } = await scene([true], {}, [false]);
  get('set-locale').value = 'zh-Hans';
  const saving = get('set-locale').onchange();
  await plans[0].started.promise;
  finish(plans[0]);
  await nativePlans[0].started.promise;
  assert.equal(durable(disk).locale, 'zh-Hans', 'the file landed before the native effect ran');
  finish(nativePlans[0]);
  await saving;

  assert.equal(ctx.settings.locale, 'zh-Hans');
  assert.equal(get('set-locale').value, 'zh-Hans');
  assert.equal(getLocale(), 'zh-Hans');
  assert.equal(durable(disk).locale, 'zh-Hans');
  assert.ok(get('toasts').children.some(el => el.textContent === t('error.settingApply')),
    'feedback distinguishes an apply failure from a failed save');
});

regression('queued locale effects use the committed locale, not a later optimistic preview', async () => {
  const { disk, plans, nativePlans, calls } = await scene([true, false], {}, [true]);
  get('set-locale').value = 'zh-Hans';
  const first = get('set-locale').onchange();
  await plans[0].started.promise;
  get('set-locale').value = 'en';
  const second = get('set-locale').onchange();
  assert.equal(ctx.settings.locale, 'en', 'the later edit is visible while the first write is pending');

  finish(plans[0]);
  await nativePlans[0].started.promise;
  const native = calls.filter(call => call.command === 'set_native_locale');
  assert.equal(native.length, 1);
  assert.equal(native[0].args.locale, 'zh-Hans', 'the first post-commit effect receives its own durable document');
  assert.equal(ctx.settings.locale, 'en', 'the later preview remains visible during the first native effect');
  finish(nativePlans[0]);
  await first;
  await plans[1].started.promise;
  finish(plans[1]);
  await second;

  assert.equal(calls.filter(call => call.command === 'set_native_locale').length, 1,
    'the failed second write has no native effect');
  assert.equal(ctx.settings.locale, 'zh-Hans');
  assert.equal(get('set-locale').value, 'zh-Hans');
  assert.equal(getLocale(), 'zh-Hans');
  assert.equal(durable(disk).locale, 'zh-Hans');
});

regression('an unrelated successful edit round-trips future settings extensions', async () => {
  const { disk, plans } = await scene([true]);
  const saving = setFontScale(1.2);
  await plans[0].started.promise;
  finish(plans[0]);
  await saving;
  assert.deepEqual(durable(disk).futureExtension, { revision: 7, mode: 'keep' });
  assert.equal(durable(disk).shortcuts.futureAction, 'Meta+F12');
  assert.deepEqual(ctx.settings.futureExtension, { revision: 7, mode: 'keep' });
});
