// pi-model.js: what the Settings row and a card may say about Pi. Closed
// values in, dictionary keys out; nothing here is a backend fact.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  PI_COVERAGE, PI_INSTALLATIONS, PI_SESSION, piCoverage, piDetailKeys, piErrorKey, piSessionKey, piSessionObserved, piSettingsView,
  piStatus, piStatusKey,
} from '../js/pi-model.js';
import { createAttentionTracker, attentionBadge, DELIVERY_WAITS, deliveryWaits } from '../js/attention-model.js';
import { dictionaries } from '../js/i18n.js';

const limits = JSON.parse(readFileSync(new URL('./fixtures/limits.json', import.meta.url), 'utf8'));

test('the closed words are one list with the backend', () => {
  assert.deepEqual([...PI_COVERAGE], limits.pi_coverage);
  assert.deepEqual([...PI_INSTALLATIONS], limits.pi_installations);
  assert.deepEqual([...PI_SESSION], limits.pi_session);
});

test('the Settings row is read from the backend and from nothing else', () => {
  assert.deepEqual(piStatus({ pi: true, pi_installation: 'current' }), { signal: true, lifecycle: false, installation: 'current' });
  assert.deepEqual(piSettingsView({ pi: true, pi_installation: 'current' }),
    { signal: true, lifecycle: false, usable: true, note: 'settings.pi.note.current' });
  // off, nothing to say
  assert.deepEqual(piSettingsView({ pi: false, pi_installation: 'absent' }),
    { signal: false, lifecycle: false, usable: true, note: null });
  // the two switches are two answers of the backend: neither implies the other
  assert.deepEqual(piSettingsView({ pi: false, pi_lifecycle: true, pi_installation: 'current' }),
    { signal: false, lifecycle: true, usable: true, note: 'settings.pi.note.current' });
  assert.deepEqual(piSettingsView({ pi: true, pi_lifecycle: true, pi_installation: 'needs-reload' }),
    { signal: true, lifecycle: true, usable: true, note: 'settings.pi.note.needsReload' });
  for (const value of ['true', 1, null, undefined]) {
    assert.equal(piStatus({ pi: true, pi_lifecycle: value, pi_installation: 'current' }).lifecycle, false, String(value));
  }
  assert.equal(piStatus({ pi_lifecycle: true, pi_installation: 'somewhere' }).lifecycle, false, 'an unknown state is no state');
  // configured is not connected: the note says what the entry is, never that Pi loaded it
  assert.equal(piSettingsView({ pi: true, pi_installation: 'needs-reload' }).note, 'settings.pi.note.needsReload');
  // the switch shows what the file says even where this build cannot change it
  assert.deepEqual(piSettingsView({ pi: true, pi_installation: 'unsupported-location' }),
    { signal: true, lifecycle: false, usable: false, note: 'settings.pi.note.unsupportedLocation' });
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
      assert.deepEqual(view, { signal: false, lifecycle: false, usable: false, note: 'settings.pi.note.unknown' });
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
  // session observation says what it is and what it is not, in both languages
  for (const key of ['settings.piLifecycleHint', 'settings.piLifecycleEnableConfirm', 'signal.pi.session']) {
    assert.match(dictionaries.en[key], /does not (support )?reopen/i, key);
    assert.match(dictionaries['zh-Hans'][key], /不会自动重新打开|暂不支持自动重入/, key);
  }
  assert.match(dictionaries['zh-Hans']['settings.piLifecycleHint'], /观察当前原生会话；此安装暂不支持自动重入。/);
  // the id is never promised to be shown, and the hint says where it stays
  assert.match(dictionaries.en['settings.piLifecycleHint'], /in memory only[^.]*never shows, logs or stores it/);
  // the one fixed sentence of the work-status switch, in both languages
  assert.match(dictionaries.en['settings.piSignalHint'],
    /Shows when work starts and when an interaction ends\. Deck currently cannot reliably observe when Pi is waiting for input, so automatic sending to Pi sessions is paused\./);
  assert.match(dictionaries['zh-Hans']['settings.piSignalHint'], /显示工作开始和交互结束。当前无法可靠观察输入等待；自动发送暂停。/);
  // "configured" is never worded as "connected"
  for (const key of ['settings.pi.note.current', 'settings.piSignalEnabled']) {
    assert.doesNotMatch(dictionaries.en[key], /connected to|is connected|loaded/i, key);
  }
});

test('an observed session is one more line of explanation and nothing else', () => {
  const live = { alive: true, piSignal: 'connected', piSession: 'observed' };
  assert.equal(piSessionObserved(live), true);
  assert.deepEqual(piDetailKeys(live), ['signal.pi.connectedHint', 'signal.pi.hold', 'signal.pi.session']);
  // not observed, a word this build does not know, a stopped card: no line
  for (const snapshot of [{ alive: true, piSignal: 'connected' }, { alive: true, piSignal: 'connected', piSession: 'resumable' },
    { alive: false, piSignal: 'connected', piSession: 'observed' }]) {
    assert.equal(piSessionObserved(snapshot), false);
  }
  assert.deepEqual(piDetailKeys({ alive: true, piSignal: 'unavailable' }), ['signal.pi.unavailableHint', 'signal.pi.hold']);
  // observation alone, without any coverage word, explains nothing on a card that is no recognized Pi
  assert.equal(piDetailKeys({ alive: true, piSession: 'observed' }), null);
  // A session id Deck does not support is SAID: neither an observed session nor nothing. One line, no state.
  const unsupported = { alive: true, piSignal: 'connected', piSession: 'unsupported' };
  assert.equal(piSessionObserved(unsupported), false);
  assert.equal(piSessionKey(unsupported), 'signal.pi.sessionUnsupported');
  assert.deepEqual(piDetailKeys(unsupported), ['signal.pi.connectedHint', 'signal.pi.hold', 'signal.pi.sessionUnsupported']);
  assert.equal(piStatusKey(unsupported), piStatusKey({ alive: true, piSignal: 'connected' }));
  assert.equal(piSessionKey({ alive: false, piSession: 'unsupported' }), null);
  assert.equal(piSessionKey({ alive: true, piSession: 'truncated' }), null);
  for (const dictionary of [dictionaries.en, dictionaries['zh-Hans']]) {
    const text = dictionary['signal.pi.sessionUnsupported'];
    assert.ok(text && text !== dictionary['signal.pi.session']);
    // no id, length or number is ever part of the sentence
    assert.doesNotMatch(text, /\d/);
  }
  // a missing settings directory is said as that, and not as "Pi never ran on this Mac"
  assert.doesNotMatch(dictionaries.en['error.pi.neverRan'], /this Mac|never|not been run/i);
  assert.doesNotMatch(dictionaries['zh-Hans']['error.pi.neverRan'], /这台 Mac|没有运行过/);
  // it is no state: the status text of the card does not change with it
  assert.equal(piStatusKey(live), piStatusKey({ alive: true, piSignal: 'connected' }));
  // the snapshot the Board keeps takes the closed word from the poll, or nothing
  const tracker = createAttentionTracker();
  const card = { id: 'c', session: 's' };
  tracker.record([card], [{ name: 's', alive: true, pi_signal: 'connected', pi_session: 'observed' }]);
  assert.equal(tracker.get(card).piSession, 'observed');
  tracker.record([card], [{ name: 's', alive: true, pi_signal: 'connected', pi_session: 'unsupported' }]);
  assert.equal(tracker.get(card).piSession, 'unsupported');
  tracker.record([card], [{ name: 's', alive: true, pi_signal: 'connected', pi_session: 'abc-123' }]);
  assert.equal(tracker.get(card).piSession, null, 'anything but the closed word is nothing');
});
