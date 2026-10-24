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
  assert.match(titled.title, /列表/);
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
  // the shell risk is said where it applies (a list without an expected
  // program, a card without a launch command), not as standing copy
  assert.match(t('queue.plan.compatibility'), /shell/);
  assert.match(t('queue.shellRisk'), /shell/);
  assert.match(t('queue.explainer'), /不早于/);
  assert.match(t('queue.context.replaced'), /替换/);
  assert.match(t('queue.context.differentProcess', { process: 'codex' }), /codex.*前台/);
  assert.doesNotMatch(t('queue.quiet.done'), /就绪|可发送/);
});

test('channel monitor disconnect copy never promises reaction catch-up', () => {
  setLocale('en');
  assert.equal(t('settings.channelStatus.disconnected'), 'not connected (no backfill)');
  setLocale('zh-Hans');
  assert.equal(t('settings.channelStatus.disconnected'), '未连接（无历史回填）');
});

/* "Reopen Agent session" (重新进入 Agent 会话) is about the Agent's own NATIVE
   session, a different object from deck's session (which stays "session" in
   zh). 会话 is allowed for that object only, in exactly these strings; a key
   that is not listed here follows the rule like the rest of the dictionary,
   whatever its namespace. Each entry says why it names the native session. */
const NATIVE_SESSION_KEYS = new Map([
  ['settings.claudeReentry', 'the setting is named after the Claude session whose start and end it observes'],
  ['settings.claudeReentryHint', 'Claude session start/end hooks and the Claude sessions offered for reopening'],
  ['settings.claudeReentryEnableConfirm', 'the Claude session a client is in when it starts or exits'],
  ['settings.claudeReentryEnabled', 'names the setting (Claude session re-entry observation)'],
  ['settings.claudeReentryDisabled', 'names the setting (Claude session re-entry observation)'],
  ['settings.codexReentry', 'the setting is named after the Codex session whose start and end it observes'],
  ['settings.codexReentryHint', 'Codex session start/end hooks and the Codex sessions offered for reopening'],
  ['settings.codexReentryEnableConfirm', 'the Codex session a client is in when it starts or exits'],
  ['settings.codexReentryEnabled', 'names the setting (Codex session re-entry observation)'],
  ['settings.codexReentryDisabled', 'names the setting (Codex session re-entry observation)'],
  ['reentry.note.codex', 'the native session Codex reports entering at its first turn, and one that may continue on its own'],
  ['reentry.reason.observation-off', 'quotes the setting name (session re-entry observation) of the row\'s Agent'],
  ['reentry.result.started', 'the native session the Codex client exited in and was started with'],
  ['reentry.result.startedDetail', 'the native session Deck did not observe being entered'],
  ['reentry.heading', 'the product action: reopen the Agent session'],
  ['reentry.summary', 'counts Claude sessions that can be reopened'],
  ['reentry.summary.none', 'no Agent session can be reopened'],
  ['reentry.summary.unknown', 'which Agent sessions could be reopened is not known yet'],
  ['reentry.summary.unavailable', 'counts Claude sessions that were found'],
  ['reentry.setupHint', 'quotes the setting name (Claude session re-entry observation)'],
  ['reentry.policy.bypass', 'Claude sessions launched with a bypass profile'],
  ['reentry.policy.bypassNone', 'Claude sessions launched with a bypass profile'],
  ['reentry.policy.scope', 'the selected, verified Claude sessions of this restart'],
  ['reentry.policy.bypassUnavailable', 'the selected Claude sessions and their launch profile'],
  ['reentry.policy.unknownCount', 'the selected Claude sessions whose launch profile is unverified'],
  ['reentry.note.scope', 'the native session a client was in when it exited'],
  ['reentry.note.otherClients', 'the same native session open in another terminal or device'],
  ['reentry.confirm', 'the number of Agent sessions the click reopens'],
  ['reentry.confirmManaged', 'the Agent sessions the click reopens; the MCP ones it closes are deck sessions and say "session"'],
  ['reentry.stage.preparing', 'the stage that prepares the selected Agent sessions for re-entry: their clients exit and the session each ended in is read (approved wording)'],
  ['reentry.stage.reopening', 'the stage that reopens the Agent sessions'],
  ['reentry.result.confirmed', 'the Claude session whose re-entry was confirmed (approved sentence)'],
  ['reentry.result.confirmedOrdinary', 'the Claude session whose re-entry was confirmed'],
  ['reentry.reason.final-session-unconfirmed', 'the native session the client ended in'],
  ['reentry.reason.native-session-not-found', 'the original native session Claude could not find'],
  ['reentry.reason.user-action-required', 'the Claude session that asks for the user during its startup'],
  ['settings.piLifecycle', 'the setting is named after the native Pi session it observes'],
  ['settings.piLifecycleHint', 'the native Pi session a terminal currently runs, and that it is not reopened'],
  ['settings.piLifecycleEnableConfirm', 'the native Pi session whose id the extension reports'],
  ['settings.piLifecycleEnabled', 'names the setting (Pi session observation)'],
  ['settings.piLifecycleDisabled', 'names the setting (Pi session observation)'],
  ['signal.pi.session', 'the native Pi session a card currently runs'],
  ['signal.pi.sessionUnsupported', 'the native Pi session whose id Deck does not support'],
  ['recovery.title', 'the native Claude sessions a lost shell service interrupted'],
  ['recovery.message', 'the native Claude session each card was in when the service was lost'],
  ['recovery.note', 'how a native Claude session is resumed by the recovery'],
  ['recovery.recordedAt', 'when the native Claude session of the card was recorded'],
  ['recovery.blocked.observation-off', 'names the setting (Claude session re-entry observation)'],
  ['recovery.result.title', 'the native Claude sessions the recovery was asked for'],
  ['recovery.result.footer', 'the native Claude sessions that are not offered again'],
]);
// The two agent-status hints predate this rule and stay as they were.
const SESSION_TERM_LEGACY = /^settings.(agentHooks|codexHooks)/;

/** Keys of a zh dictionary that say 会话 without being allowed to. */
function sessionTermViolations(zh) {
  return Object.entries(zh)
    .filter(([key, value]) => /会话/.test(value) && !SESSION_TERM_LEGACY.test(key) && !NATIVE_SESSION_KEYS.has(key))
    .map(([key]) => key);
}

test('会话 names only the Agent native session, in an enumerated list of strings', () => {
  const zh = dictionaries['zh-Hans'];
  assert.deepEqual(sessionTermViolations(zh), []);
  // the list is exact: every entry exists, says 会话, and carries its reason
  for (const [key, reason] of NATIVE_SESSION_KEYS) {
    assert.match(zh[key] ?? '', /会话/, `${key} is listed but does not use the term`);
    assert.match(key, /^(?:reentry\.|recovery\.|settings\.claudeReentry|settings\.codexReentry|settings\.piLifecycle|signal\.pi\.session(?:Unsupported)?$)/,
      `${key}: only re-entry and recovery copy and Pi's session observation are about a native session`);
    assert.ok(reason.length > 10, key);
  }
  // no namespace is exempt: a new re-entry or settings string is held to the rule
  for (const key of ['reentry.future', 'reentry.result.future', 'reentry.reason.future', 'settings.claudeReentryFuture', 'tmux.future']) {
    assert.deepEqual(sessionTermViolations({ ...zh, [key]: '此会话已保存' }), [key], key);
  }
  // an allowed string does not cover its neighbours, and saying "session" is always fine
  assert.deepEqual(sessionTermViolations({ 'reentry.heading': '重新进入 Agent 会话', 'reentry.headingNote': '会话', 'reentry.kind.shell': 'Shell session' }),
    ['reentry.headingNote']);
});

/* 06 B v01: one name per object. The page is 看板 / Board, a column is
   栏目 / group, a session is "session" in both languages, a split is 窗格 /
   pane; the automation editor's two rows are not both "触发". */
test('every object has exactly one name in each dictionary', () => {
  const zh = dictionaries['zh-Hans'], en = dictionaries.en;
  const columnKeys = ['app.newBoard', 'board.newName', 'board.deleteTitle', 'board.atLeastOne', 'board.delete', 'automation.column', 'automation.needsColumn', 'error.targetSave'];
  for (const key of columnKeys) {
    assert.match(zh[key], /栏目/, key);
    assert.doesNotMatch(zh[key], /Board/, key);
    assert.match(en[key], /group/i, key);
    assert.doesNotMatch(en[key], /Board|Column/, key);
  }
  for (const key of ['app.board', 'session.back', 'error.boardLoad', 'error.projectSave', 'error.sessionSave'])
    assert.match(zh[key], /看板/, key);
  /* Everything except the two agent-hook hints follows the 会话 rule, and
     inside the re-entry copy only the enumerated keys below may say it. */
  assert.deepEqual(sessionTermViolations(zh), [], 'zh keeps "session" untranslated');
  const zhAll = Object.entries(zh).filter(([k]) => !/^settings.(agentHooks|codexHooks)/.test(k)).map(([, v]) => v).join('\n');
  // deck's own session keeps its one name inside the re-entry copy too
  for (const key of ['reentry.kind.shell', 'reentry.reason.multiple-panes', 'reentry.reason.identity-unavailable',
    'reentry.reason.impact-changed', 'reentry.confirmManaged']) assert.match(zh[key], /session/, key);
  // the first progress stage is about the Agent sessions being reopened (approved wording)
  assert.equal(zh['reentry.stage.preparing'], '准备会话');
  assert.equal(en['reentry.stage.preparing'], 'Preparing sessions');
  // the approved sentences of policy B, word for word: what choosing it does,
  // and what a confirmed entry says — a flag that was sent, never a
  // permission that is in effect
  assert.equal(zh['reentry.policy.bypassWarning'], '如果运行期间降低过权限，此次重新施加 bypass 会再次启用 bypass。');
  assert.equal(en['reentry.policy.bypassWarning'], 'If you lowered permissions while the session ran, reapplying bypass now will enable bypass again.');
  assert.equal(zh['reentry.result.confirmed'], '已确认重新进入 {agent} 会话；已按本次授权发送 bypass 参数。');
  assert.equal(en['reentry.result.confirmed'], '{agent} session re-entry confirmed. The bypass flag was sent as authorized.');
  assert.equal(zh['reentry.warning.bypass-reauthorized'], 'Deck 未独立验证 {agent} 当前有效权限。');
  assert.equal(zh['reentry.result.unconfirmed'], '已尝试发送重入请求，结果未确认');
  for (const [key, value] of [...Object.entries(zh), ...Object.entries(en)].filter(([key]) => key.startsWith('reentry.'))) {
    assert.doesNotMatch(value, /已恢复|权限已验证|有效权限为|bypass (?:was |is )?restored|permission (?:was |is )?verified|bypass is (?:on|active|in effect)/i, key);
    assert.doesNotMatch(value, /sha-?256|digest|摘要|认证版本|未经认证|certified|[0-9]+\.[0-9]+\.[0-9]{2,}/i, key);
  }
  assert.doesNotMatch(zhAll, /个 pane|当前 pane/, 'zh calls a pane 窗格');
  assert.notEqual(zh['automation.trigger'], zh['automation.when']);
  assert.equal(zh['app.sessions'], 'session');
  for (const key of ['session.status.running', 'session.status.waiting']) {
    assert.doesNotMatch(zh[key], /可能正在等待输入|已就绪/, key);
    assert.doesNotMatch(en[key], /may be waiting|is ready/, key);
  }
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
  assert.equal(old.sessionRestore, false, 'recovery is opt-in when no explicit preference exists');
  assert.equal('debug' in old, false, 'retired user debug setting is discarded');
  assert.deepEqual(old.future, { kept: 1 });
  const saved = JSON.parse(serializeSettings({ ...old, locale: 'zh-Hans', sessionRestore: false }));
  assert.equal(saved.locale, 'zh-Hans');
  assert.equal(saved.sessionRestore, false);
  assert.equal('debug' in saved, false);
  assert.deepEqual(saved.future, { kept: 1 });
});

/* Signal Integrity FR-SI-02: agent hook words are interaction observations.
   The governed Signal copy may not claim more than the source can prove: a
   turn ENDED (never "finished"/"completed" work), input was REQUESTED (never
   a present "needs your input"/"waiting for you"). Only this explicit surface
   is checked — an unrelated string may say "completed" — and adding or
   removing a governed key is an intentional edit of this list. The status
   tooltip and hook-observation families are governed in full. */
const SIGNAL_COPY_KEYS = Object.freeze([
  'attention.summaryGlobal', 'attention.filter.input', 'attention.filter.done',
  'attention.input', 'attention.working', 'attention.doneRead', 'attention.doneUnread',
  'attention.noSignal', 'attention.noSignalHint', 'attention.viewed', 'attention.emptyPending',
  'session.status.running', 'session.status.waiting', 'session.status.attention',
  'session.status.done', 'session.status.stopped', 'session.doneTab',
  'settings.agentHooksHint', 'settings.codexHooksHint', 'settings.notifyAwayHint',
  'queue.signal.working', 'queue.signal.input', 'queue.signal.done', 'queue.signal.none',
  'queue.stage.agent', 'queue.stage.firstSend',
  'automation.hint', 'automation.help.finish', 'automation.help.firstStep', 'queue.plan.signals', 'queue.plan.firstStep',
  'automation.finish', 'automation.kv.finish',
  'automation.runClosed', 'automation.runCloseFailed',
]);
const SIGNAL_COPY_FAMILIES = Object.freeze(['session.status.', 'queue.signal.']);

test('agent signal wording never claims task completion or a present wait', () => {
  assert.equal(new Set(SIGNAL_COPY_KEYS).size, SIGNAL_COPY_KEYS.length, 'no key listed twice');
  for (const locale of ['en', 'zh-Hans']) {
    for (const key of SIGNAL_COPY_KEYS) assert.equal(typeof dictionaries[locale][key], 'string', `${locale} ${key}`);
    for (const key of Object.keys(dictionaries[locale])) {
      if (SIGNAL_COPY_FAMILIES.some(prefix => key.startsWith(prefix))) {
        assert.ok(SIGNAL_COPY_KEYS.includes(key), `${key} is Signal copy: list it`);
      }
    }
  }
  const en = /needs? (your )?input|turn finished|finished (its|a) turn|waiting for your (review|input)|task (is )?(complete|done|finished)/i;
  const zh = /已完成本轮|本轮完成|完成一轮|已完成一轮|等待你的输入|需要你的输入|正在等待你|任务已完成/;
  for (const key of SIGNAL_COPY_KEYS) {
    assert.doesNotMatch(dictionaries.en[key], en, key);
    assert.doesNotMatch(dictionaries['zh-Hans'][key], zh, key);
  }
  assert.match(dictionaries.en['session.status.done'], /turn ended/i);
  assert.match(dictionaries.en['attention.input'], /requested/i);
  assert.match(dictionaries['zh-Hans']['attention.input'], /请求输入/);
});
