// Frontend ↔ backend mirrored constants. This is the JS half of the mirror:
// test/fixtures/limits.json is the one list, this file holds the frontend
// exports to it, and src-tauri/src/limits_mirror.rs holds the Rust constants
// to the same file. The backend stays authoritative (it refuses on save);
// the frontend copies exist so the UI never offers what the backend refuses.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  AGENT_STATES, CHAIN_QUIET_SECS, CONTEXT_STATUS_KEYS, INBOUND_BADGE_RE, ITEM_MAX_ATTEMPTS,
  LOCAL_ID_RE, MAX_DROP_BYTES, MAX_QUIET_SECS, MCP_ERROR_KEYS, MIN_QUIET_SECS,
} from '../js/pure.js';
import {
  ACCENTS, DEFAULT_GRACE_MIN, FINISH_MODES, FONT_SCALE_MAX, FONT_SCALE_MIN, INBOUND_CMD_MAX,
  INBOUND_NAME_MAX, INBOUND_RULE_ID_MAX, INBOUND_SOURCES, MAX_GRACE_MIN, MAX_INBOUND_RULES,
  SCHEDULE_UNITS, SHORTCUT_MAX_LEN, TEMPLATE_NAME_MAX, THEMES, UPDATE_CHANNELS,
} from '../js/settings-model.js';
import {
  CHANNEL_IDLE_MAX, CHANNEL_IDS_MAX, CHANNEL_KEYWORD_MAX_CHARS, CHANNEL_KEYWORDS_MAX, CHANNEL_RULES_MAX,
} from '../js/channel-model.js';
import {
  BUFFER_MAX_BYTES, BUFFER_MAX_COPIES, BUFFER_MAX_ENTRIES, BUFFER_MAX_ENTRY_BYTES, BUFFER_MAX_SERIALIZED_BYTES,
} from '../js/buffer-model.js';
import { PRESET_MAX } from '../js/connector-model.js';
import { dictionaries, LOCALE_CHOICES } from '../js/i18n.js';
import { ACCENT_IDS, THEME_IDS } from '../js/theme.js';
import { NOTIFY_STATUS_WORDS } from '../js/notify-model.js';

const limits = JSON.parse(readFileSync(new URL('./fixtures/limits.json', import.meta.url), 'utf8'));

test('scheduler mirrors', () => {
  assert.deepEqual(
    { chain_quiet_secs_default: CHAIN_QUIET_SECS, quiet_secs_min: MIN_QUIET_SECS,
      quiet_secs_max: MAX_QUIET_SECS, item_max_attempts: ITEM_MAX_ATTEMPTS },
    limits.scheduler);
});

test('inbound rule mirrors', () => {
  assert.deepEqual({
    sources: [...INBOUND_SOURCES], schedule_units: [...SCHEDULE_UNITS], finish_modes: [...FINISH_MODES],
    grace_min_default: DEFAULT_GRACE_MIN, grace_min_max: MAX_GRACE_MIN, rules_max: MAX_INBOUND_RULES,
    rule_id_max: INBOUND_RULE_ID_MAX, rule_name_max_chars: INBOUND_NAME_MAX,
    rule_cmd_max_chars: INBOUND_CMD_MAX, template_name_max_chars: TEMPLATE_NAME_MAX,
  }, limits.inbound);
});

test('identifier and badge spellings: one pattern each, same vectors as the backend', () => {
  assert.equal(LOCAL_ID_RE.source, limits.local_id.pattern);
  assert.equal(INBOUND_BADGE_RE.source, limits.badge.pattern);
  for (const [re, spec] of [[LOCAL_ID_RE, limits.local_id], [INBOUND_BADGE_RE, limits.badge]]) {
    for (const value of spec.valid) assert.ok(re.test(value), `${re} accepts ${JSON.stringify(value)}`);
    for (const value of spec.invalid) assert.ok(!re.test(value), `${re} refuses ${JSON.stringify(value)}`);
  }
});

test('channel, buffer, preset and drop mirrors', () => {
  assert.deepEqual({
    rules_max: CHANNEL_RULES_MAX, channel_ids_max: CHANNEL_IDS_MAX, keywords_max: CHANNEL_KEYWORDS_MAX,
    keyword_max_chars: CHANNEL_KEYWORD_MAX_CHARS, idle_minutes_max: CHANNEL_IDLE_MAX,
  }, limits.channel);
  assert.deepEqual({
    max_entries: BUFFER_MAX_ENTRIES, max_copies: BUFFER_MAX_COPIES, max_entry_bytes: BUFFER_MAX_ENTRY_BYTES,
    max_bytes: BUFFER_MAX_BYTES, max_serialized_bytes: BUFFER_MAX_SERIALIZED_BYTES,
  }, limits.buffer);
  assert.equal(PRESET_MAX, limits.presets_max);
  assert.equal(MAX_DROP_BYTES, limits.drop_max_bytes);
});

test('settings mirrors', () => {
  assert.deepEqual({
    font_scale_min: FONT_SCALE_MIN, font_scale_max: FONT_SCALE_MAX, themes: [...THEMES], accents: [...ACCENTS],
    locales: [...LOCALE_CHOICES], update_channels: [...UPDATE_CHANNELS], shortcut_max_len: SHORTCUT_MAX_LEN,
  }, limits.settings);
  assert.deepEqual([...THEME_IDS], limits.settings.themes);
  assert.deepEqual([...ACCENT_IDS], limits.settings.accents);
});

test('closed status vocabularies', () => {
  assert.deepEqual([...AGENT_STATES], limits.agent_states);
  assert.deepEqual([...NOTIFY_STATUS_WORDS], limits.notify_status_words);
  assert.deepEqual(Object.keys(CONTEXT_STATUS_KEYS), limits.context_statuses);
  assert.deepEqual(Object.keys(MCP_ERROR_KEYS), limits.mcp_local_errors);
  // storage.rs StorageNotice codes: each one has a sentence in both languages
  for (const code of limits.storage_notices) {
    for (const [locale, dictionary] of Object.entries(dictionaries)) {
      assert.equal(typeof dictionary[`notice.${code}`], 'string', `${locale} notice.${code}`);
    }
  }
});
