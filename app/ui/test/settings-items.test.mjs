// The settings items the MCP settings summary names (`mcp/settings_read.rs`
// embeds `fixtures/settings-items.json`): every searchable setting's stable
// id, its section and, per UI language, the section title and the
// setting's title. One list, mirrored: this test holds the fixture equal to
// `settings-search-model.js` and the dictionaries. After changing either,
// regenerate it with
//   DECK_WRITE_SETTINGS_ITEMS=1 node --test app/ui/test/settings-items.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync } from 'node:fs';

import { dictionaries, LOCALES } from '../js/i18n.js';
import { SETTINGS_SECTIONS, SETTING_ITEMS, searchSettings } from '../js/settings-search-model.js';

const FIXTURE = new URL('./fixtures/settings-items.json', import.meta.url);

const title = (locale, key) => dictionaries[locale][key] ?? dictionaries.en[key];

function expected() {
  const sections = new Map(SETTINGS_SECTIONS.map(section => [section.id, section.titleKey]));
  return {
    items: SETTING_ITEMS.map(item => ({
      id: item.id,
      section: item.section,
      path: Object.fromEntries(LOCALES.map(locale =>
        [locale, [title(locale, sections.get(item.section)), title(locale, item.labelKeys[0])]])),
    })),
  };
}

test('the settings summary fixture mirrors the searchable settings and their titles', () => {
  const want = expected();
  if (process.env.DECK_WRITE_SETTINGS_ITEMS === '1') {
    writeFileSync(FIXTURE, `${JSON.stringify(want, null, 2)}\n`);
  }
  assert.deepEqual(JSON.parse(readFileSync(FIXTURE, 'utf8')), want,
    'regenerate: DECK_WRITE_SETTINGS_ITEMS=1 node --test app/ui/test/settings-items.test.mjs');
  for (const item of want.items) {
    for (const locale of LOCALES) {
      for (const part of item.path[locale]) assert.ok(typeof part === 'string' && part.length, `${item.id} ${locale}`);
    }
  }
});

test('typing a setting id into the search shows exactly that setting', () => {
  for (const item of SETTING_ITEMS) {
    assert.deepEqual(searchSettings(item.id, dictionaries), [{ section: item.section, items: [item.id] }], item.id);
    assert.deepEqual(searchSettings(`  ${item.id.toUpperCase()} `, dictionaries),
      [{ section: item.section, items: [item.id] }], item.id);
  }
  // a word that is not an id still searches as before
  assert.ok(searchSettings('notification', dictionaries).flatMap(r => r.items).includes('away-notifications'));
});
