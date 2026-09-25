import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { dictionaries } from '../js/i18n.js';
import {
  SETTINGS_SECTIONS, SETTING_ITEMS, isSettingsSection, normalizeSearchQuery, searchSettings, sectionItems, settingItem,
} from '../js/settings-search-model.js';

const html = readFileSync(new URL('../index.html', import.meta.url), 'utf8');
const search = query => searchSettings(query, dictionaries);
const ids = query => (search(query) || []).flatMap(result => result.items);

test('the sections follow user intent, in sidebar order, and every one is in the markup', () => {
  assert.deepEqual(SETTINGS_SECTIONS.map(section => section.id),
    ['general', 'shortcuts', 'terminal', 'agents', 'integrations', 'remote', 'data', 'about']);
  const navOrder = [...html.matchAll(/id="set-nav-([a-z]+)"/g)].map(match => match[1]);
  assert.deepEqual(navOrder, SETTINGS_SECTIONS.map(section => section.id));
  for (const { id, titleKey } of SETTINGS_SECTIONS) {
    assert.match(html, new RegExp(`id="set-panel-${id}" aria-labelledby="set-heading-${id}"`));
    for (const dictionary of Object.values(dictionaries)) assert.ok(dictionary[titleKey], titleKey);
  }
  assert.equal(dictionaries['zh-Hans']['settings.agents'], 'Agent 与通知');
  assert.equal(dictionaries['zh-Hans']['settings.integrations'], '集成');
  assert.equal(dictionaries['zh-Hans']['settings.remote'], '远程访问');
  assert.equal(dictionaries['zh-Hans']['settings.data'], '数据与隐私');
});

test('every setting has a unique stable id, a real section, translated labels and a group in its panel', () => {
  assert.equal(new Set(SETTING_ITEMS.map(item => item.id)).size, SETTING_ITEMS.length);
  const panels = Object.fromEntries(SETTINGS_SECTIONS.map(({ id }) => {
    const start = html.indexOf(`id="set-panel-${id}"`);
    return [id, html.slice(start, html.indexOf('</section>', start))];
  }));
  for (const item of SETTING_ITEMS) {
    assert.ok(isSettingsSection(item.section), item.id);
    assert.match(panels[item.section], new RegExp(`id="set-item-${item.id}" data-setting-id="${item.id}"`), item.id);
    for (const key of item.labelKeys)
      for (const dictionary of Object.values(dictionaries)) assert.ok(dictionary[key], `${item.id}: ${key}`);
    for (const keyword of item.keywords) assert.equal(keyword, normalizeSearchQuery(keyword), 'keywords are stored normalized');
  }
  assert.deepEqual(sectionItems('agents').map(item => item.id), ['agent-status', 'away-notifications']);
  assert.deepEqual(sectionItems('remote').map(item => item.id), ['connector', 'mcp']);
});

test('item lookups never guess', () => {
  assert.equal(settingItem('mcp').section, 'remote');
  for (const bad of ['nope', '', null, undefined, 42, { id: 'mcp' }, 'constructor', '__proto__']) assert.equal(settingItem(bad), null);
  assert.equal(isSettingsSection('integrations'), true);
  assert.equal(isSettingsSection('Integrations'), false);
});

test('an empty or blank query is normal navigation, not a search', () => {
  assert.equal(search(''), null);
  assert.equal(search('   '), null);
  assert.equal(search(undefined), null);
  assert.equal(normalizeSearchQuery('  Play   A  Sound '), 'play a sound');
});

test('English and Chinese labels find the setting in either UI locale', () => {
  assert.deepEqual(ids('Notify me when away'), ['away-notifications']);
  assert.deepEqual(ids('离席时通知我'), ['away-notifications']);
  assert.deepEqual(ids('Font size'), ['appearance']);
  assert.deepEqual(ids('字号'), ['appearance']);
  assert.deepEqual(ids('文件打开方式'), ['editor']);
  assert.deepEqual(ids('Open files in'), ['editor']);
  assert.deepEqual(ids('Shell service'), ['shell-service']);
});

test('aliases reach the setting a user means', () => {
  assert.deepEqual(ids('dock'), ['away-notifications']);
  assert.deepEqual(ids('提醒'), ['away-notifications']);
  assert.deepEqual(ids('iphone'), ['connector']);
  assert.deepEqual(ids('配对'), ['connector']);
  assert.deepEqual(ids('chatgpt'), ['mcp']);
  assert.deepEqual(ids('授权'), ['mcp']);
  assert.deepEqual(ids('tmux'), ['shell-service']);
});

test('a notification search shows only the notification group, not the pages around it', () => {
  for (const query of ['通知', 'notification', 'Notifications']) {
    assert.deepEqual(search(query), [{ section: 'agents', items: ['away-notifications'] }], query);
  }
  assert.deepEqual(search('Play a sound'), [{ section: 'agents', items: ['away-notifications'] }]);
});

test('an MCP search lands on MCP terminal control under Remote access', () => {
  assert.deepEqual(search('MCP'), [{ section: 'remote', items: ['mcp'] }]);
  assert.deepEqual(search('mcp 终端控制'), [{ section: 'remote', items: ['mcp'] }]);
  assert.deepEqual(search('remote'), [{ section: 'remote', items: ['connector', 'mcp'] }]);
});

test('text that lives only in hints or Learn more never surfaces a group', () => {
  // each phrase appears in a hint in both dictionaries, never in a label
  for (const phrase of ['0600', 'Socket Mode', 'OAuth', 'pinned HTTPS', '~/.claude/settings.json', 'scratchpads', '256 KB']) {
    assert.ok(Object.values(dictionaries.en).some(text => text.includes(phrase)), `fixture: ${phrase} is in a hint`);
    assert.deepEqual(search(phrase), [], phrase);
  }
});

test('matching is case-insensitive, trims whitespace and needs every term', () => {
  assert.deepEqual(search('  SLACK  '), search('slack'));
  assert.deepEqual(ids('slack'), ['slack-reactions']);
  assert.deepEqual(ids('slack channel'), ['slack-reactions']);
  assert.deepEqual(ids('codex'), ['agent-status', 'mcp']);
  assert.deepEqual(ids('codex hook'), ['agent-status']);
});

test('a section title shows that section when no setting names the query', () => {
  assert.deepEqual(search('Data & privacy'), [{ section: 'data', items: ['history', 'shell-data', 'logs'] }]);
  assert.deepEqual(search('数据与隐私'), [{ section: 'data', items: ['history', 'shell-data', 'logs'] }]);
  assert.deepEqual(search('agents & notifications'), [{ section: 'agents', items: ['agent-status', 'away-notifications'] }]);
  assert.deepEqual(search('About & updates'), [{ section: 'about', items: ['updates'] }]);
});

test('a section title and a setting name narrow together; a named setting beats its section title', () => {
  assert.deepEqual(search('terminal'), [
    { section: 'terminal', items: ['voice', 'editor', 'session-restore', 'shell-service'] },
    { section: 'remote', items: ['mcp'] },
  ]);
  assert.deepEqual(search('terminal voice'), [{ section: 'terminal', items: ['voice'] }]);
  assert.deepEqual(search('集成'), [{ section: 'integrations', items: ['slack-reactions'] }]);
});

test('an unknown query has no results', () => {
  assert.deepEqual(search('no such setting xyz'), []);
  assert.deepEqual(search('通知 xyz'), []);
});
