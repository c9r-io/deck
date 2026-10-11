// pi-model.js: what the Settings row and a card may say about Pi. Closed
// values in, dictionary keys out; nothing here is a backend fact.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import {
  PI_COVERAGE, PI_INSTALLATIONS, piCoverage, piDetailKeys, piErrorKey, piSettingsView, piStatus, piStatusKey,
} from '../js/pi-model.js';
import { createAttentionTracker, attentionBadge, DELIVERY_WAITS, deliveryWaits } from '../js/attention-model.js';
import { dictionaries } from '../js/i18n.js';

const limits = JSON.parse(readFileSync(new URL('./fixtures/limits.json', import.meta.url), 'utf8'));

test('the closed words are one list with the backend', () => {
  assert.deepEqual([...PI_COVERAGE], limits.pi_coverage);
  assert.deepEqual([...PI_INSTALLATIONS], limits.pi_installations);
});

test('the Settings row is read from the backend and from nothing else', () => {
  assert.deepEqual(piStatus({ pi: true, pi_installation: 'current' }), { signal: true, installation: 'current' });
  assert.deepEqual(piSettingsView({ pi: true, pi_installation: 'current' }),
    { signal: true, usable: true, note: 'settings.pi.note.current' });
  // off, nothing to say
  assert.deepEqual(piSettingsView({ pi: false, pi_installation: 'absent' }), { signal: false, usable: true, note: null });
  // configured is not connected: the note says what the entry is, never that Pi loaded it
  assert.equal(piSettingsView({ pi: true, pi_installation: 'needs-reload' }).note, 'settings.pi.note.needsReload');
  // the switch shows what the file says even where this build cannot change it
  assert.deepEqual(piSettingsView({ pi: true, pi_installation: 'unsupported-location' }),
    { signal: true, usable: false, note: 'settings.pi.note.unsupportedLocation' });
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
      assert.deepEqual(view, { signal: false, usable: false, note: 'settings.pi.note.unknown' });
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
    assert.doesNotMatch(text, /re-?ent(er|ry)|reopen/i, key);
  }
  // the one fixed sentence of the work-status switch, in both languages
  assert.match(dictionaries.en['settings.piSignalHint'],
    /Shows when work starts and when an interaction ends\. Deck currently cannot reliably observe when Pi is waiting for input, so automatic sending to Pi sessions is paused\./);
  assert.match(dictionaries['zh-Hans']['settings.piSignalHint'], /显示工作开始和交互结束。当前无法可靠观察输入等待；自动发送暂停。/);
  // "configured" is never worded as "connected"
  for (const key of ['settings.pi.note.current', 'settings.piSignalEnabled']) {
    assert.doesNotMatch(dictionaries.en[key], /connected to|is connected|loaded/i, key);
  }
});
