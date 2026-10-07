// Information layering (UX governance): what is always visible, what is
// conditional and what is on demand. These pin meaning and placement, not
// sentences: a risk stays outside every disclosure, a conditional fact is
// said only where it applies, and showing or opening anything calls nothing.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fakeDocument } from './fixtures/dom-fixture.mjs';
globalThis.document = fakeDocument; globalThis.window = { __TAURI__: null };
const { ctx } = await import('../js/state.js');
const { dictionaries, setLocale, t } = await import('../js/i18n.js');
const { reviewRow, rowStage, sendPlan, sessionFacts, stageText } = await import('../js/queue-review.js');

const read = name => readFileSync(new URL(`../${name}`, import.meta.url), 'utf8');
const en = dictionaries.en, zh = dictionaries['zh-Hans'];
const now = () => Math.floor(Date.now() / 1000);
/* every text node below `el`, with whether it sits inside a <details> */
const texts = (el, inside = false, out = []) => {
  const here = inside || el.tagName === 'DETAILS';
  if (!el.children.length && el.textContent) out.push({ text: el.textContent, disclosed: here && el.tagName !== 'SUMMARY' });
  for (const child of el.children) texts(child, here, out);
  return out;
};
const visible = el => texts(el).filter(entry => !entry.disclosed).map(entry => entry.text);
const disclosed = el => texts(el).filter(entry => entry.disclosed).map(entry => entry.text);
const list = (head, rows = [head]) => ({ head, rows });
const calls = [];
const invoke = replies => { calls.length = 0; window.__TAURI__ = { core: { invoke: async (cmd, args) => { calls.push(cmd); return replies[cmd]?.(args) ?? null; } } }; };

test('a row says its own stage; one that only waits its turn says nothing, a stale one still does', () => {
  setLocale('en');
  ctx.queueCache = { items: [], last_fired: {}, plans: [
    { item: 'a', stage: 'quiet', quiet_remaining: 30, checked_at: now() },
    { item: 'b', stage: 'previous', checked_at: now() },
    { item: 'c', stage: 'previous', checked_at: now() - 120 },
    { item: 'e', stage: 'agent', checked_at: now() },
  ] };
  assert.match(rowStage({ id: 'a' }), /quiet/i);
  assert.equal(rowStage({ id: 'b' }), '', 'the order already says it');
  assert.equal(rowStage({ id: 'c' }), t('queue.stage.unknown'), 'a stale plan is never silence');
  assert.equal(rowStage({ id: 'd' }), t('queue.stage.unknown'), 'nor is a missing one');
  assert.equal(rowStage({ id: 'e' }), stageText({ id: 'e' }), 'a hold that needs the user stays on its row');
});

test('the five kinds of waiting read differently, in both languages', () => {
  const kinds = { machine: 'queue.stage.quiet', user: 'queue.stage.review', uncertain: 'queue.meta.ambiguous',
    revoked: 'queue.stage.channelStopped', unverifiable: 'queue.stage.authorityUnverified' };
  for (const dict of [en, zh]) assert.equal(new Set(Object.values(kinds).map(key => dict[key])).size, 5);
  assert.match(en['queue.stage.authorityUnverified'], /can’t verify/); assert.match(zh['queue.stage.authorityUnverified'], /无法核验/);
  assert.match(en['queue.stage.channelStopped'], /stopped/); assert.match(zh['queue.stage.channelStopped'], /已停止/);
  // unverifiable is temporary and says so; it is never worded as off or revoked
  assert.doesNotMatch(en['queue.stage.authorityUnverified'], /revoked|turned off/i);
  assert.doesNotMatch(zh['queue.stage.authorityUnverified'], /已撤销|已关闭/);
  assert.match(en['queue.meta.ambiguous'], /can.t be confirmed|cannot|unconfirmed|uncertain/i);
});

test('times promise an earliest moment, never a delivery at that moment', () => {
  assert.match(en['queue.explainer'], /not before/); assert.match(zh['queue.explainer'], /不早于/);
  assert.match(en['queue.explainer'], /does not promise/); assert.match(zh['queue.explainer'], /不保证/);
  assert.match(en['queue.notBefore'], /not before/); assert.match(zh['queue.notBefore'], /不早于/);
  assert.match(en['queue.plan.conditions'], /quiet does not mean ready/); assert.match(zh['queue.plan.conditions'], /静默不代表就绪/);
});

test('a compatibility target is said outside the disclosure; an expected program is detail', () => {
  setLocale('en'); ctx.queueCache = { items: [], last_fired: {}, plans: [] };
  const shell = sendPlan(list({ id: 'h1', mode: 'at' }), async () => {});
  assert.ok(visible(shell).includes(t('queue.plan.compatibility')), 'input may reach a shell: always visible');
  assert.match(t('queue.plan.compatibility'), /shell/);
  const bound = sendPlan(list({ id: 'h2', mode: 'at', expected_process: 'codex' }), async () => {});
  assert.equal(visible(bound).includes(t('queue.plan.compatibility')), false);
  assert.ok(disclosed(bound).includes(t('queue.plan.expected', { process: 'codex' })));
  // the standing sentence no longer carries it; the form says it for a card without a command
  assert.match(en['queue.shellRisk'], /shell/); assert.match(zh['queue.shellRisk'], /shell/);
  assert.match(read('js/scheduler.js'), /\$\('q-shell-risk'\)\.hidden = !!String\(card\.cmd \|\| ''\)\.trim\(\);/);
});

test('what holds for the session is said once, and interleaving only with more than one list', () => {
  setLocale('en'); ctx.attention = { get: () => ({ agent: 'needs-input' }) };
  const one = sessionFacts({ id: 'K1' }, 1), two = sessionFacts({ id: 'K1' }, 2);
  assert.deepEqual(visible(one), [t('queue.observation', { state: t('queue.signal.input') })], 'one list: only the observation summary');
  assert.ok(visible(two).includes(t('queue.plan.others', { count: 2 })));
  assert.equal(JSON.stringify(texts(one)).includes('0'), false, 'never "0 other lists"');
  assert.ok(disclosed(two).includes(t('queue.plan.unverified')), 'the observation is evidence, apart from any list');
  const scheduler = read('js/scheduler.js');
  assert.match(scheduler, /if \(lists\.length\) list\.appendChild\(sessionFacts\(card, lists\.length\)\);/);
  assert.equal((scheduler.match(/sessionFacts\(/g) || []).length, 1, 'rendered once per panel, not per list');
  ctx.attention = { get: () => ({ agent: 'working', stale: true }) };
  assert.deepEqual(visible(sessionFacts({ id: 'K1' }, 1)), [t('queue.observation', { state: t('queue.stage.unknown') })], 'a stale observation is unknown, not its old word');
});

test('a disclosure remembers being open across renders and calls nothing', () => {
  invoke({}); ctx.queueCache = { items: [], last_fired: {}, plans: [] };
  const g = list({ id: 'h3', mode: 'at' });
  const details = () => sendPlan(g, async () => {}).children.find(el => el.tagName === 'DETAILS');
  const first = details();
  assert.equal(first.open, false, 'closed by default');
  first.open = true; first.ontoggle();
  assert.equal(details().open, true, 'the next render keeps it open');
  const again = details(); again.open = false; again.ontoggle();
  assert.equal(details().open, false);
  assert.deepEqual(calls, [], 'no backend call, save or release');
  assert.ok(first.children[0].dataset.queueFocus, 'the summary can take the restored focus');
});

test('the inspection dialog names the next row and target; the last row says only what applies', async () => {
  setLocale('en');
  const preview = { decision: { id: 'r1', revision: 3 }, next_text: 'run the tests', current_process: 'zsh', expected_process: 'codex' };
  const press = async (item, last, reply) => {
    invoke({ queue_review_preview: () => reply });
    const row = reviewRow(item, async () => {}, () => {}, last);
    const confirm = row.children[1].children.find(el => el.dataset.queueFocus === `${item.id}:confirm`);
    const done = confirm.onclick();
    await new Promise(resolve => setTimeout(resolve, 5));
    const message = fakeDocument.getElementById('cfm-msg').textContent;
    (await import('../js/dialogs.js')).cfmDone(false); await done;
    return message;
  };
  const item = { id: 'r1', state: 'review', text: 'sent row' };
  const next = await press(item, {}, preview);
  for (const part of ['run the tests', 'zsh', 'codex', 'quiet time', 'send gap', 'target checks', 'does not approve permission requests']) assert.ok(next.includes(part), part);
  assert.deepEqual(calls, ['queue_review_preview'], 'a declined dialog confirms nothing');
  const last = { ...preview, next_text: null };
  const plain = await press(item, {}, last);
  assert.equal(plain, t('queue.review.lastConfirm'));
  assert.equal(plain.includes(t('queue.review.lastRepeat')) || plain.includes(t('queue.review.lastClose')), false, 'no exception that does not apply');
  assert.ok((await press(item, { repeats: true }, last)).includes(t('queue.review.lastRepeat')));
  const closing = await press(item, { closes: true }, last);
  assert.ok(closing.includes(t('queue.review.lastClose')) && !closing.includes(t('queue.review.lastRepeat')));
  assert.match(t('queue.review.lastClose'), /after the program exits/, 'never "after the task is done"');
  // an inspected checkpoint offers no second release, and "viewed" is not a control here
  const approved = reviewRow({ id: 'r2', state: 'review-approved', text: 'x' }, async () => {}, () => {});
  assert.equal(approved.children[1].children.some(el => el.dataset.queueFocus === 'r2:confirm'), false);
  assert.match(en['queue.review.disableConfirm'], /remain and are not released/);
  assert.match(zh['queue.review.disableConfirm'], /不会因此放行/);
});

test('the re-check button is named for what it does; waiting needs no approval', () => {
  for (const dict of [en, zh]) { assert.equal('queue.keepWaiting' in dict, false); assert.equal(typeof dict['queue.recheck'], 'string'); }
  assert.match(en['queue.recheck'], /check/i); assert.match(zh['queue.recheck'], /检查/);
  assert.match(en['queue.recheckDone'], /on its own/); assert.match(zh['queue.recheckDone'], /无需再点/);
  assert.match(read('js/scheduler.js'), /wait\.onclick = \(\) => refreshItemProbe\(i\)/, 'still only a probe');
});

test('automation risks stay next to their control, outside every disclosure', () => {
  const html = read('index.html');
  const editor = html.slice(html.indexOf('<div id="auto-editor"'), html.indexOf('id="auto-channel-verify"'));
  const outside = id => {
    const at = editor.indexOf(`id="${id}"`); assert.ok(at > 0, id);
    const before = editor.slice(0, at);
    assert.equal(before.lastIndexOf('<details') > before.lastIndexOf('</details>'), false, `${id} is not inside a disclosure`);
  };
  for (const id of ['auto-first-send', 'auto-first-send-hint', 'auto-first-send-state', 'auto-channel-first-send',
    'auto-channel-first-send-hint', 'auto-channel-first-send-facts', 'auto-send', 'auto-send-hint', 'auto-send-external',
    'auto-review', 'auto-finish', 'auto-finish-hint', 'auto-finish-keep-hint']) outside(id);
  for (const [dict, words] of [[en, ['not readiness proof', 'Enter', '--no-daemon']], [zh, ['不是就绪证明', '回车', '--no-daemon']]]) {
    for (const key of ['automation.firstSend.hint', 'presets.firstSend.hint']) for (const word of words) assert.ok(dict[key].includes(word), `${key}: ${word}`);
  }
  // the channel permission: future events, outside content, startup risk, first step only, what turning off does
  const facts = ['scope', 'content', 'risk', 'excludes', 'revoke'].map(name => `automation.channelFirstSend.fact.${name}`);
  for (const key of facts) assert.ok(editor.includes(`data-i18n="${key}"`), key);
  assert.match(en['automation.channelFirstSend.hint'], /future events/); assert.match(zh['automation.channelFirstSend.hint'], /未来/);
  assert.match(en[facts[1]], /outside Slack messages/); assert.match(zh[facts[1]], /外部消息/);
  assert.match(en[facts[2]], /Enter/); assert.match(zh[facts[2]], /回车/);
  assert.match(en[facts[3]], /tool permissions.*later/); assert.match(zh[facts[3]], /工具权限.*后续/);
  assert.match(en[facts[4]], /not retracted/); assert.match(zh[facts[4]], /不会撤回|也不会撤回/);
  // the three triggers keep three different grants
  assert.notEqual(en['automation.channelFirstSend.option'], en['automation.firstSend.option']);
  assert.notEqual(en['automation.autoSend.option'], en['automation.firstSend.option']);
});

test('the drawer head is a definition and a pointer; the run contract is one named disclosure', () => {
  for (const dict of [en, zh]) assert.ok(dict['automation.hint'].length < 140, 'short');
  assert.match(en['automation.hint'], /must be running/); assert.match(zh['automation.hint'], /保持运行/);
  const html = read('index.html');
  const help = html.slice(html.indexOf('class="set-details auto-help"'), html.indexOf('<div id="auto-list">'));
  for (const key of ['busy', 'finish', 'firstStep', 'channel', 'delivery']) assert.ok(help.includes(`data-i18n="automation.help.${key}"`), key);
  assert.match(en['automation.help.finish'], /program exits/); assert.match(zh['automation.help.finish'], /程序退出/);
  assert.equal(en['automation.finish'], 'After the program exits'); assert.equal(zh['automation.finish'], '程序退出后');
  // a clock rule's skipped slot is visible for both finish choices, not only in help
  const js = read('js/automation.js');
  assert.match(js, /\$\('auto-finish-keep-hint'\)\.hidden = !\(segGet\('auto-trigger'\) === 'clock' && segGet\('auto-finish'\) === 'keep'\);/);
  assert.match(en['automation.finish.keepHint'], /next slot is skipped/); assert.match(zh['automation.finish.keepHint'], /跳过/);
  // the teaching empty state gives way to an open editor
  assert.match(js, /if \(!rules\.length && !editing\) \{/);
});

test('a first-step box ticked on a command it cannot reach reads as not in effect', async () => {
  const { firstSendText } = await import('../js/automation-model.js');
  setLocale('en');
  assert.equal(firstSendText({ firstSendWithoutReadiness: true, cmd: 'codex' }), t('automation.firstSend.unsupported'));
  assert.equal(firstSendText({ firstSendWithoutReadiness: true, cmd: 'codex --no-daemon' }), t('automation.firstSend.on'));
  assert.equal(firstSendText({ firstSendWithoutReadiness: false, cmd: 'claude' }), t('automation.firstSend.off'));
  assert.doesNotMatch(t('automation.firstSend.on'), /ready(?!iness)/i, 'a short wait is never called ready');
  assert.match(read('js/automation.js'), /state\.textContent = t\('automation\.current', \{ state: firstSendText\(rule\) \}\);/);
});

test('phone task presets: being paired is not approval, and the list says what a preset amounts to', () => {
  assert.match(en['presets.autoSend.scope.body'], /Being paired is not an approval/); assert.match(zh['presets.autoSend.scope.body'], /已配对不等于已批准/);
  assert.match(en['presets.autoSend.hint'], /sent from the phone are not covered/); assert.match(zh['presets.autoSend.hint'], /不在批准之内/);
  assert.match(en['presets.autoSend.scope.body'], /Revoking a paired device/); assert.match(zh['presets.autoSend.scope.body'], /撤销已配对的设备/);
  const dialogs = read('js/dialogs.js');
  assert.match(dialogs, /if \(!preset\.autoSend\) say\('presets\.later\.manual'\);/);
  assert.match(dialogs, /say\('presets\.later\.checking'\);/, 'an approval not compared yet is neither on nor off');
  assert.match(dialogs, /say\(valid \? 'presets\.later\.auto' : 'presets\.later\.stale'\)/);
  assert.match(dialogs, /\(\) => say\('presets\.later\.unknown'\)/);
  const states = ['auto', 'manual', 'stale', 'checking', 'unknown'].map(name => `presets.later.${name}`);
  for (const dict of [en, zh]) assert.equal(new Set(states.map(key => dict[key])).size, 5);
});

test('reminders: registration is never worded as delivery, and the note limit is bytes', () => {
  assert.match(en['reminder.registered'], /not guaranteed/); assert.match(zh['reminder.registered'], /不保证送达/);
  const three = ['reminder.registered', 'reminder.registrationFailed', 'reminder.saved'];
  for (const dict of [en, zh]) assert.equal(new Set(three.map(key => dict[key])).size, 3);
  assert.match(en['reminder.registrationFailed'], /saved/); assert.match(zh['reminder.registrationFailed'], /已保存/);
  for (const key of ['reminder.noteUsed', 'reminder.noteOver']) {
    assert.match(en[key], /UTF-8 bytes/, key); assert.match(zh[key], /UTF-8 字节/, key);
    assert.doesNotMatch(zh[key], /\{max\} ?(个)?字(?!节)/, 'never the same number of characters');
  }
  const dialogs = read('js/dialogs.js');
  assert.match(dialogs, /new TextEncoder\(\)\.encode\(note\.value\)\.length; const over = used > REMINDER_NOTE_BYTES;/);
  assert.match(dialogs, /const valid = timeValid && noteValid\(note\.value\);/, 'an over-long note still blocks Save; nothing is truncated');
  assert.doesNotMatch(dialogs, /note\.maxLength|note\.value = note\.value\.slice/);
});

test('templates count steps, and placeholders have a named way in', () => {
  assert.match(en['queue.steps'], /steps/); assert.match(zh['queue.steps'], /步/);
  assert.doesNotMatch(zh['queue.steps'], /行/); assert.doesNotMatch(en['queue.steps'], /rows|lines/);
  assert.match(read('index.html'), /<summary data-i18n="templates\.placeholders\.title">[^<]+<\/summary>\s*<div class="set-hint tpl-wide" data-i18n="templates\.placeholders">/);
  assert.match(en['templates.approvalVoided'], /approve again/);
});

test('an unknown attention status keeps its meaning where it shows', () => {
  assert.match(en['attention.unknownHint'], /does not mean there is nothing/); assert.match(zh['attention.unknownHint'], /不表示没有事项/);
  assert.match(en['attention.unknownHint'], /N\+/); assert.match(zh['attention.unknownHint'], /N\+/);
  assert.doesNotMatch(en['attention.hint'], /N\+/, 'said with the unknown state, not as standing copy');
  assert.match(en['attention.hint'], /Viewed does not mean handled/); assert.match(zh['attention.hint'], /已看过不等于已处理/);
});
