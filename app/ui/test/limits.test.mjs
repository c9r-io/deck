import { REMINDER_NOTE_BYTES } from '../js/reminder-model.js';
// Frontend ↔ backend mirrored constants. This is the JS half of the mirror:
// test/fixtures/limits.json is the one list, this file holds the frontend
// exports to it, and src-tauri/src/limits_mirror.rs holds the Rust constants
// to the same file. The backend stays authoritative (it refuses on save);
// the frontend copies exist so the UI never offers what the backend refuses.
// A length bound's key says its unit. Where the UI DOES offer what the Board
// refuses, the fixture says so (`unit_gaps`: characters here, bytes there)
// and both halves hold their side to it; the gap is known and left open.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  AGENT_STATES, CHAIN_QUIET_SECS, CONTEXT_STATUS_KEYS, INBOUND_BADGE_RE, ITEM_MAX_ATTEMPTS,
  LOCAL_ID_RE, MAX_DROP_BYTES, MAX_QUIET_SECS, MCP_ERROR_KEYS, MIN_QUIET_SECS,
  TEMPLATE_STEPS_MAX, templateNameProblem,
} from '../js/pure.js';
import {
  ACCENTS, AUTO_SEND_CLASSES, AUTO_SEND_MAX_STEPS, DEFAULT_GRACE_MIN, FINISH_MODES, FONT_SCALE_MAX, FONT_SCALE_MIN, INBOUND_CMD_MAX,
  INBOUND_NAME_MAX, INBOUND_RULE_ID_MAX, INBOUND_SOURCES, MAX_GRACE_MIN, MAX_INBOUND_RULES,
  SCHEDULE_UNITS, SHORTCUT_MAX_LEN, TEMPLATE_NAME_MAX, THEMES, UPDATE_CHANNELS,
} from '../js/settings-model.js';
import {
  CHANNEL_IDLE_MAX, CHANNEL_IDS_MAX, CHANNEL_KEYWORD_MAX_CHARS, CHANNEL_KEYWORDS_MAX, CHANNEL_RULES_MAX,
} from '../js/channel-model.js';
import {
  BUFFER_MAX_BYTES, BUFFER_MAX_COPIES, BUFFER_MAX_ENTRIES, BUFFER_MAX_ENTRY_BYTES, BUFFER_MAX_SERIALIZED_BYTES,
} from '../js/buffer-model.js';
import { normalizeTaskPreset, PRESET_MAX } from '../js/connector-model.js';
import { dictionaries, LOCALE_CHOICES } from '../js/i18n.js';
import { ACCENT_IDS, THEME_IDS } from '../js/theme.js';
import { LABEL_TITLE_MAX_BYTES, NOTIFY_STATUS_WORDS } from '../js/notify-model.js';
import { CODEX_SIGNAL_TRUST } from '../js/attention-model.js';
import { MAX_TRANSLATION_BYTES, MAX_LIVE_TRANSLATION_BYTES } from '../js/local-intelligence.js';

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
    auto_send_max_steps: AUTO_SEND_MAX_STEPS, auto_send_classes: [...AUTO_SEND_CLASSES],
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
  assert.equal(MAX_TRANSLATION_BYTES, limits.translation_max_bytes);
  assert.equal(MAX_LIVE_TRANSLATION_BYTES, limits.translation_live_bytes);
  assert.deepEqual(limits.translation_document_choices, [8192, 16384]);
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
  assert.deepEqual([...CODEX_SIGNAL_TRUST], limits.codex_signal_trust);
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

test('reminder note bounds match the native fixture', () => {
  assert.equal(REMINDER_NOTE_BYTES, limits.reminder_note_bytes);
});

/* The Board's bounds (documents.rs). The frontend holds the preset ones in
   `normalizeTaskPreset`, without constants of its own, so they are read off
   its behaviour: a value at the bound is kept and the next one is dropped. */
const utf8 = value => new TextEncoder().encode(value).byteLength;
const presetKept = fields => !!normalizeTaskPreset(
  { id: 'R1', name: 'n', columnId: 'C1', title: 't', dir: '~/w', cmd: 'codex', steps: ['s'], ...fields }, [{ id: 'C1' }]);

test('Board mirrors: plan steps, and the preset bounds the frontend counts in the same unit', () => {
  const b = limits.board;
  assert.deepEqual(Object.keys(b), ['plan_steps_max', 'plan_template_name_max_bytes', 'preset_name_max_bytes',
    'preset_title_max_bytes', 'preset_dir_max_bytes', 'preset_cmd_max_bytes', 'preset_steps_max', 'preset_step_max_bytes']);
  assert.equal(TEMPLATE_STEPS_MAX, b.plan_steps_max, 'a template never has more steps than a plan may freeze');
  const dir = bytes => '/' + 'a'.repeat(bytes - 1);
  assert.ok(presetKept({ dir: dir(b.preset_dir_max_bytes) }));
  assert.ok(!presetKept({ dir: dir(b.preset_dir_max_bytes + 1) }));
  const cjkDir = '/' + '模'.repeat(Math.floor(b.preset_dir_max_bytes / 3) + 1);
  assert.ok([...cjkDir].length < b.preset_dir_max_bytes && utf8(cjkDir) > b.preset_dir_max_bytes);
  assert.ok(!presetKept({ dir: cjkDir }), 'a directory is counted in bytes here too');
  const cmd = bytes => 'codex ' + 'a'.repeat(bytes - 6);
  assert.ok(presetKept({ cmd: cmd(b.preset_cmd_max_bytes) }));
  assert.ok(!presetKept({ cmd: cmd(b.preset_cmd_max_bytes + 1) }));
  assert.ok(presetKept({ steps: Array(b.preset_steps_max).fill('s') }));
  assert.ok(!presetKept({ steps: Array(b.preset_steps_max + 1).fill('s') }));
  assert.ok(presetKept({ steps: ['a'.repeat(b.preset_step_max_bytes)] }));
  assert.ok(!presetKept({ steps: ['a'.repeat(b.preset_step_max_bytes + 1)] }));
  assert.ok(!presetKept({ steps: ['模'.repeat(Math.floor(b.preset_step_max_bytes / 3) + 1)] }), 'a step is counted in bytes here too');
});

test('editor mirrors: a template name, a preset name and a preset title are counted in characters', () => {
  const names = limits.inbound.template_name_max_chars;
  for (const unit of ['a', '模', '😀']) {
    assert.equal(templateNameProblem(unit.repeat(names)), null, `template name: ${names} × ${unit}`);
    assert.equal(templateNameProblem(unit.repeat(names + 1)), 'long', `template name: ${names + 1} × ${unit}`);
  }
  for (const [field, key] of [['name', 'preset_name_max_chars'], ['title', 'preset_title_max_chars']]) {
    const max = limits.editor[key];
    for (const unit of ['a', '模', '😀']) {
      assert.ok(presetKept({ [field]: unit.repeat(max) }), `${field}: ${max} × ${unit}`);
      assert.ok(!presetKept({ [field]: unit.repeat(max + 1) }), `${field}: ${max + 1} × ${unit}`);
    }
  }
  assert.deepEqual(Object.keys(limits.editor), ['preset_name_max_chars', 'preset_title_max_chars']);
});

/* Known and left open: these names carry one number in two units. The
   editors below accept the fixture's sample; limits_mirror.rs shows that the
   Board refuses it, when the run card or the project defaults are saved.
   Changing the unit on either side breaks one of the two tests. */
test('the listed unit gaps are real on the editors\' side', () => {
  const { sample, pairs } = limits.unit_gaps;
  const at = key => key.split('.').reduce((value, part) => value[part], limits);
  const accepts = {
    'inbound.template_name_max_chars': () => templateNameProblem(sample) === null,
    'editor.preset_name_max_chars': () => presetKept({ name: sample }),
    'editor.preset_title_max_chars': () => presetKept({ title: sample }),
  };
  assert.deepEqual(pairs.map(([chars]) => chars), Object.keys(accepts), 'a new pair needs its editor named here');
  for (const [chars, bytes] of pairs) {
    assert.ok(chars.endsWith('_chars') && bytes.endsWith('_bytes'), `${chars} / ${bytes}`);
    assert.equal(at(chars), at(bytes), `${chars} / ${bytes}: one number, two units`);
    assert.ok([...sample].length <= at(chars), 'within the characters');
    assert.ok(utf8(sample) > at(bytes), 'over the bytes');
    assert.ok(accepts[chars](), `${chars}: the editor accepts what the Board refuses`);
  }
});

test('notification label mirrors: the title bound, in bytes', () => {
  assert.equal(LABEL_TITLE_MAX_BYTES, limits.notify.label_title_max_bytes);
  // the count bound is the backend's alone: the webview sends every card
  assert.deepEqual(Object.keys(limits.notify), ['labels_max', 'label_title_max_bytes']);
});

// links.rs LINK_FAILURES: a failed open says which reason applied. terminal.js
// is WKWebView-bound, so its table is read from the source, like static.test.
test('link failure reasons: one sentence each, in both languages', () => {
  const terminal = readFileSync(new URL('../js/terminal.js', import.meta.url), 'utf8');
  const table = terminal.match(/const LINK_FAILURE_KEYS = \{([\s\S]*?)\};/)?.[1] || '';
  const entries = [...table.matchAll(/'([a-z-]+)': '([A-Za-z.]+)'/g)].map(match => [match[1], match[2]]);
  assert.deepEqual(entries.map(([code]) => code), limits.link_failures);
  for (const [locale, dictionary] of Object.entries(dictionaries)) {
    const sentences = entries.map(([, key]) => dictionary[key]);
    for (const [index, sentence] of sentences.entries()) {
      assert.equal(typeof sentence, 'string', `${locale} ${entries[index][1]}`);
    }
    assert.equal(new Set([...sentences, dictionary['terminal.openFailed']]).size, entries.length + 1,
      `${locale}: every reason reads differently, and none is the bare fallback`);
    // "no editor" says where to choose one, in the Settings labels as they are now
    const noEditor = dictionary[Object.fromEntries(entries)['link-no-editor']];
    assert.ok(noEditor.includes(dictionary['settings.terminal']), `${locale} names the section`);
    assert.ok(noEditor.includes(dictionary['settings.openFiles']), `${locale} names the setting`);
  }
  // the backend's message is never shown: an unknown one falls back to the generic sentence
  assert.match(terminal, /t\(LINK_FAILURE_KEYS\[linkFailure\(err\)\] \|\| 'terminal\.openFailed'\)/);
  // only a path that was not there is retried with the wider reading; any
  // other reason means the narrow reading already resolved
  assert.match(terminal, /if \(!lookback \|\| linkFailure\(err\) !== 'link-path-missing'\) throw err;/);
});
