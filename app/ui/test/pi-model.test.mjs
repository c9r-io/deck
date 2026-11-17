// pi-model.js: what the Settings row and a card may say about Pi. Closed
// values in, dictionary keys out; nothing here is a backend fact.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  PI_COVERAGE, PI_INSTALLATIONS, piCoverage, piDetailKeys, piErrorKey, piSettingsView,
  piStatus, piStatusKey,
} from '../js/pi-model.js';
import { createAttentionTracker, attentionBadge, DELIVERY_WAITS, deliveryWaits } from '../js/attention-model.js';
import { dictionaries } from '../js/i18n.js';

const limits = JSON.parse(readFileSync(new URL('./fixtures/limits.json', import.meta.url), 'utf8'));

test('the closed words are one list with the backend', () => {
  assert.deepEqual([...PI_COVERAGE], limits.pi_coverage);
  assert.deepEqual([...PI_INSTALLATIONS], limits.pi_installations);
  assert.equal(limits.pi_session, undefined, 'no word about a native Pi session exists');
});

test('the Settings row is read from the backend and from nothing else', () => {
  assert.deepEqual(piStatus({ pi: true, pi_installation: 'current' }), { signal: true, replies: false, installation: 'current' });
  assert.deepEqual(piSettingsView({ pi: true, pi_installation: 'current' }),
    { signal: true, replies: false, usable: true, note: 'settings.pi.note.current' });
  // off, nothing to say
  assert.deepEqual(piSettingsView({ pi: false, pi_installation: 'absent' }),
    { signal: false, replies: false, usable: true, note: null });
  // the two switches are two answers of the backend, shown as they are answered
  assert.deepEqual(piSettingsView({ pi: true, pi_replies: true, pi_installation: 'current' }),
    { signal: true, replies: true, usable: true, note: 'settings.pi.note.current' });
  assert.deepEqual(piSettingsView({ pi: true, pi_replies: true, pi_installation: 'needs-reload' }),
    { signal: true, replies: true, usable: true, note: 'settings.pi.note.needsReload' });
  for (const value of ['true', 1, null, undefined]) {
    assert.equal(piStatus({ pi: true, pi_replies: value, pi_installation: 'current' }).replies, false, String(value));
  }
  assert.equal(piStatus({ pi_replies: true, pi_installation: 'somewhere' }).replies, false, 'an unknown state is no state');
  // an answer of a build that still had session observation says nothing of a final message
  assert.equal(piStatus({ pi: true, pi_lifecycle: true, pi_installation: 'current' }).replies, false);
  // configured is not connected: the note says what the entry is, never that Pi loaded it
  assert.equal(piSettingsView({ pi: true, pi_installation: 'needs-reload' }).note, 'settings.pi.note.needsReload');
  // the switch shows what the file says even where this build cannot change it
  assert.deepEqual(piSettingsView({ pi: true, pi_installation: 'unsupported-location' }),
    { signal: true, replies: false, usable: false, note: 'settings.pi.note.unsupportedLocation' });
  for (const installation of ['conflict', 'unreadable']) {
    const view = piSettingsView({ pi: false, pi_installation: installation });
    assert.equal(view.usable, true, installation);
    assert.match(view.note, /^settings\.pi\.note\./);
  }
  // a word this build does not know is no state: off, unusable, and said so
  for (const status of [null, undefined, {}, { pi: true }, { pi: true, pi_installation: 'installed' },
    { pi: true, pi_installation: 7 }, { pi: 'yes', pi_installation: 'current' }]) {
    const view = piSettingsView(status);
    assert.equal(view.signal, status?.pi === true && status.pi_installation === 'current' ? true : false);
    if (status?.pi_installation !== 'current') {
      assert.deepEqual(view, { signal: false, replies: false, usable: false, note: 'settings.pi.note.unknown' });
    }
  }
  // every state has its sentence in both languages
  for (const installation of PI_INSTALLATIONS) {
    const { note } = piSettingsView({ pi: false, pi_installation: installation });
    for (const [locale, dictionary] of Object.entries(dictionaries)) {
      if (note) assert.equal(typeof dictionary[note], 'string', `${locale} ${installation}`);
    }
  }
});

test('a refusal has its own sentence for a closed code and the generic one otherwise', () => {
  const codes = ['pi-never-ran', 'pi-config-unsupported-location', 'pi-config-unreadable', 'pi-config-layout',
    'pi-config-conflict', 'pi-config-busy', 'pi-config-unlocated', 'pi-resource-missing'];
  for (const code of codes) {
    const key = piErrorKey(code);
    assert.match(key, /^error\.pi\./, code);
    for (const [locale, dictionary] of Object.entries(dictionaries)) {
      assert.equal(typeof dictionary[key], 'string', `${locale} ${code}`);
    }
  }
  // free text of any kind never becomes a sentence of its own
  for (const other of ['', 'unknown agent', '/Users/u/.pi/agent/settings.json: permission denied', undefined, null, 7,
    'toString', '__proto__', 'constructor']) {
    assert.equal(piErrorKey(other), 'error.agentHooks');
  }
});

test('coverage is said per live card and is never an agent state', () => {
  const card = { id: 'c', session: 'pane' };
  const tracker = createAttentionTracker();
  const poll = (rest) => tracker.record([card], [{ name: 'pane', alive: true, idle_secs: 200, ...rest }]);
  poll({});
  assert.equal(piCoverage(tracker.get(card)), null);
  assert.equal(piStatusKey(tracker.get(card)), null);
  assert.equal(piDetailKeys(tracker.get(card)), null);
  for (const coverage of PI_COVERAGE) {
    poll({ pi_signal: coverage });
    const snapshot = tracker.get(card);
    assert.equal(piCoverage(snapshot), coverage);
    assert.equal(snapshot.agent, null, 'coverage is not a word');
    assert.equal(tracker.category(card), 'unavailable', 'and not an attention category of its own');
    assert.equal(attentionBadge(tracker, card), null, `${coverage}: no badge`);
    assert.equal(tracker.counts([card]).pending, 0, `${coverage}: not pending`);
    const [hint, hold] = piDetailKeys(snapshot);
    assert.match(hint, /^signal\.pi\./);
    assert.equal(hold, 'signal.pi.hold', 'the paused automatic sending is said under every state');
    for (const key of [piStatusKey(snapshot), hint, hold]) {
      for (const [locale, dictionary] of Object.entries(dictionaries)) {
        assert.equal(typeof dictionary[key], 'string', `${locale} ${key}`);
      }
    }
  }
  // unknown words, a stopped card and a replaced session say nothing
  for (const word of ['trusted', 'unknown', 'needs-input', 'ready', true, 1, null]) {
    poll({ pi_signal: word });
    assert.equal(piCoverage(tracker.get(card)), null, String(word));
  }
  tracker.record([card], [{ name: 'pane', alive: false, pi_signal: 'connected' }]);
  assert.equal(piCoverage(tracker.get(card)), null);
  poll({ pi_signal: 'connected' });
  assert.equal(piCoverage(tracker.get({ ...card, session: 'replacement' })), null);
  // a real word keeps its own text; the limited-coverage note stays available
  poll({ pi_signal: 'connected', agent: 'turn-done', episode: 4 });
  assert.equal(tracker.get(card).agent, 'turn-done');
  assert.equal(tracker.category(card), 'done', 'a real ending is attention as for every agent');
  assert.deepEqual(piDetailKeys(tracker.get(card)), ['signal.pi.connectedHint', 'signal.pi.hold']);
  // a Pi never has an input request to show: nothing here can make one
  poll({ pi_signal: 'connected' });
  assert.notEqual(tracker.category(card), 'input');
});

test('a row held for a Pi target is a delivery that waits for a person', () => {
  assert.ok(DELIVERY_WAITS.includes('pi-input-unverified'));
  const items = [{ id: 'a', session: 's', mode: 'at', state: 'pending', added: 1 }];
  assert.equal(deliveryWaits(items, [{ item: 'a', stage: 'pi-input-unverified' }]).get('s'), 'pi-input-unverified');
  // the model takes the hold from the backend's plan and from nowhere else
  assert.equal(deliveryWaits(items, []).get('s'), undefined);
  assert.equal(deliveryWaits(items, [{ item: 'other', stage: 'pi-input-unverified' }]).get('s'), undefined);
  for (const dictionary of Object.values(dictionaries)) {
    assert.equal(typeof dictionary['attention.waiting.pi-input-unverified'], 'string');
    assert.equal(typeof dictionary['queue.stage.piInputUnverified'], 'string');
  }
});

test('what the page says about Pi promises no more than Deck observes', () => {
  const said = Object.entries(dictionaries.en).filter(([key]) => /(^|\.)pi(\.|[A-Z])|\.pi-/.test(key));
  assert.ok(said.length > 20);
  for (const [key, text] of said) {
    // nothing claims readiness, a resumed or restored session, or re-entry for Pi
    assert.doesNotMatch(text, /\b(ready|resumed|restored|needs input)\b/i, key);
    // reopening is named only to say that Deck does NOT do it
    for (const sentence of text.split(/(?<=[.?!])\s+/).filter(part => /re-?ent(er|ry)|reopen/i.test(part))) {
      assert.match(sentence, /does not (support )?reopen/i, `${key}: ${sentence}`);
    }
  }
  // the final-message switch says what is read, where it goes and what it does not change — in every language
  for (const [language, dictionary] of Object.entries(dictionaries)) {
    for (const key of ['settings.piReplies', 'settings.piRepliesHint', 'settings.piRepliesEnableConfirm',
      'settings.piRepliesEnabled', 'settings.piRepliesDisabled']) {
      assert.ok(dictionary[key] && (language === 'en' || dictionary[key] !== dictionaries.en[key]), `${language} ${key}`);
    }
    // nothing of the capability that is gone is left to be said
    for (const key of Object.keys(dictionary)) assert.doesNotMatch(key, /piLifecycle|signal\.pi\.session/, `${language} ${key}`);
  }
  // it leaves the Mac only for a model provider, and only under the summary switch: said in the hint and the confirmation
  for (const key of ['settings.piRepliesHint', 'settings.piRepliesEnableConfirm']) {
    assert.match(dictionaries.en[key], /model provider[^.]*only while Summarize turn endings is on/, key);
    assert.match(dictionaries['zh-Hans'][key], /只有在开启了?“轮次结束时写摘要”[^。]*才会发给你的模型服务商/, key);
  }
  assert.match(dictionaries.en['settings.piRepliesHint'], /Nothing is written to disk or logged\./);
  assert.match(dictionaries.en['settings.piRepliesHint'], /no prompt, tool call or tool output is read/);
  assert.match(dictionaries['zh-Hans']['settings.piRepliesHint'], /不写入磁盘，不记日志。/);
  // and it lifts no hold: the confirmation says so
  assert.match(dictionaries.en['settings.piRepliesEnableConfirm'], /Automatic sending to Pi sessions stays paused\./);
  assert.match(dictionaries['zh-Hans']['settings.piRepliesEnableConfirm'], /向 Pi session 的自动发送保持暂停。/);
  // the switch whose entry reads nothing still promises exactly that
  assert.match(dictionaries.en['settings.piSignalEnableConfirm'], /no prompt text, output, or paths ever leave/);
  // the one fixed sentence of the work-status switch, in both languages
  assert.match(dictionaries.en['settings.piSignalHint'],
    /Shows when work starts and when an interaction ends\. Deck currently cannot reliably observe when Pi is waiting for input, so automatic sending to Pi sessions is paused\./);
  assert.match(dictionaries['zh-Hans']['settings.piSignalHint'], /显示工作开始和交互结束。当前无法可靠观察输入等待；自动发送暂停。/);
  // "configured" is never worded as "connected"
  for (const key of ['settings.pi.note.current', 'settings.piSignalEnabled']) {
    assert.doesNotMatch(dictionaries.en[key], /connected to|is connected|loaded/i, key);
  }
});

test('a card explains its coverage and the hold, and says nothing about a native session', () => {
  assert.deepEqual(piDetailKeys({ alive: true, piSignal: 'connected' }), ['signal.pi.connectedHint', 'signal.pi.hold']);
  assert.deepEqual(piDetailKeys({ alive: true, piSignal: 'unavailable' }), ['signal.pi.unavailableHint', 'signal.pi.hold']);
  // a word an earlier build's poll carried changes nothing
  assert.deepEqual(piDetailKeys({ alive: true, piSignal: 'connected', piSession: 'observed' }),
    ['signal.pi.connectedHint', 'signal.pi.hold']);
  assert.equal(piDetailKeys({ alive: true, piSession: 'observed' }), null);
  assert.equal(piStatusKey({ alive: true, piSignal: 'connected', piSession: 'observed' }),
    piStatusKey({ alive: true, piSignal: 'connected' }));
  // a missing settings directory is said as that, and not as "Pi never ran on this Mac"
  assert.doesNotMatch(dictionaries.en['error.pi.neverRan'], /this Mac|never|not been run/i);
  assert.doesNotMatch(dictionaries['zh-Hans']['error.pi.neverRan'], /这台 Mac|没有运行过/);
  // the snapshot the Board keeps takes nothing of a session from the poll
  const tracker = createAttentionTracker();
  const card = { id: 'c', session: 's' };
  tracker.record([card], [{ name: 's', alive: true, pi_signal: 'connected', pi_session: 'observed' }]);
  assert.equal('piSession' in tracker.get(card), false);
});
