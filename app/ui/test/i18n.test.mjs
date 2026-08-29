import test from 'node:test';
import assert from 'node:assert/strict';
import { createDefaultColumns, migrateColumnSemantics } from '../js/board-defaults.js';
import {
  dictionaries, dictionaryAudit, getLocale, resolveLocale, resolveSystemLocale,
  applyTranslations, setLocale, t,
} from '../js/i18n.js';
import { parseSettings, serializeSettings } from '../js/settings-model.js';

test('system locale recognizes only Simplified Chinese variants', () => {
  assert.equal(resolveSystemLocale(['zh-CN']), 'zh-Hans');
  assert.equal(resolveSystemLocale(['zh-SG', 'en-US']), 'zh-Hans');
  assert.equal(resolveSystemLocale(['zh-Hans-CN']), 'zh-Hans');
  assert.equal(resolveSystemLocale(['zh-Hant-TW', 'ja-JP']), 'en');
  assert.equal(resolveSystemLocale(['fr-FR']), 'en');
  assert.equal(resolveLocale('bogus', ['en-US']), 'en');
  assert.equal(resolveLocale('zh-Hans', ['en-US']), 'zh-Hans');
});

test('English and Simplified Chinese dictionaries have exactly the same keys', () => {
  const audit = dictionaryAudit();
  for (const [locale, result] of Object.entries(audit)) {
    assert.deepEqual(result, { missing: [], orphan: [] }, locale);
  }
  assert.deepEqual(Object.keys(dictionaries.en).sort(), Object.keys(dictionaries['zh-Hans']).sort());
});

test('an untranslated Simplified Chinese entry falls back to English', () => {
  const key = 'app.settings';
  const saved = dictionaries['zh-Hans'][key];
  delete dictionaries['zh-Hans'][key];
  try {
    setLocale('zh-Hans');
    assert.equal(t(key), dictionaries.en[key]);
  } finally {
    dictionaries['zh-Hans'][key] = saved;
  }
});

test('interpolation remains inert text and missing production keys never expose keys', () => {
  setLocale('en');
  const hostile = '<img src=x onerror="globalThis.pwned=1">';
  const rendered = t('queue.templateSaved', { name: hostile });
  assert.equal(rendered, `template saved: ${hostile}`);
  const sink = { textContent: '' };
  sink.textContent = rendered;
  assert.equal(sink.textContent, rendered, 'parameters are assigned as text, not parsed as HTML');
  assert.equal(t('definitely.missing'), '', 'a missing key is never shown to users');
});

test('locale switching is immediate and keeps the preference separate from effective locale', () => {
  setLocale('en');
  assert.equal(getLocale(), 'en');
  assert.equal(t('app.settings'), 'Settings');
  setLocale('zh-Hans');
  assert.equal(getLocale(), 'zh-Hans');
  assert.equal(t('app.settings'), '设置');
  setLocale('system', ['zh-Hant-TW']);
  assert.equal(getLocale(), 'en');
});

test('switching locale immediately refreshes text, titles, placeholders and lang', () => {
  const text = { dataset: { i18n: 'app.settings' }, textContent: '' };
  const titled = { dataset: { i18nTitle: 'queue.title' }, title: '' };
  const input = { dataset: { i18nPlaceholder: 'queue.placeholder' }, placeholder: '' };
  const root = { querySelectorAll(selector) {
    return selector === '[data-i18n]' ? [text]
      : selector === '[data-i18n-title]' ? [titled]
      : selector === '[data-i18n-placeholder]' ? [input] : [];
  } };
  globalThis.document = { documentElement: { lang: '' } };
  setLocale('zh-Hans');
  applyTranslations(root);
  assert.equal(text.textContent, '设置');
  assert.match(titled.title, /定时 prompt/);
  assert.match(input.placeholder, /prompt/);
  assert.equal(globalThis.document.documentElement.lang, 'zh-Hans');
  delete globalThis.document;
});

test('scheduler Chinese copy preserves sent, failed, blocked, ambiguous and quiet semantics', () => {
  setLocale('zh-Hans');
  assert.match(t('queue.sent', { session: 's' }), /已发送/);
  assert.match(t('queue.meta.failed', { attempts: 2 }), /发送失败/);
  assert.match(t('queue.blocked'), /已阻止/);
  assert.match(t('queue.meta.ambiguous'), /无法确认是否已发送/);
  assert.match(t('queue.quiet.progress', { seconds: 10, total: 180 }), /安静/);
  assert.doesNotMatch(t('queue.quiet.progress', { seconds: 10, total: 180 }), /等待输入|已完成/);
  assert.match(t('queue.explainer'), /shell/);
  assert.match(t('queue.context.replaced'), /替换/);
  assert.match(t('queue.context.differentProcess', { process: 'codex' }), /codex.*前台/);
  assert.doesNotMatch(t('queue.quiet.done'), /就绪|可发送/);
});

test('default Boards localize only at creation and existing names never change', () => {
  let n = 0;
  setLocale('zh-Hans');
  const columns = createDefaultColumns(() => `C${++n}`, t);
  assert.deepEqual(columns.map(c => c.name), ['需要关注', '进行中', '队列中', '已搁置']);
  setLocale('en');
  assert.deepEqual(columns.map(c => c.name), ['需要关注', '进行中', '队列中', '已搁置']);

  const legacy = [{ columns: [{ id: 'a', name: 'Working' }, { id: 'b', name: 'My renamed Board' }] }];
  migrateColumnSemantics(legacy);
  assert.equal(legacy[0].columns[0].semantic, 'working');
  assert.equal(legacy[0].columns[0].name, 'Working');
  assert.equal(legacy[0].columns[1].semantic, undefined);
  assert.equal(legacy[0].columns[1].name, 'My renamed Board');
});

test('old settings migrate to system locale and unknown fields round-trip', () => {
  const old = parseSettings('{"editor":"Zed","debug":true,"future":{"kept":1}}');
  assert.equal(old.locale, 'system');
  assert.deepEqual(old.future, { kept: 1 });
  const saved = JSON.parse(serializeSettings({ ...old, locale: 'zh-Hans' }));
  assert.equal(saved.locale, 'zh-Hans');
  assert.deepEqual(saved.future, { kept: 1 });
});
