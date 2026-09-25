// The Board card's attention badge painter (attention.js) against the real
// tracker, on a minimal element: hidden-not-removed, text, kind, stale mark
// and title, repainted in place. The Board lifecycle through cardEl,
// refreshBoardAttention and markSessionSeen runs in the WKWebView attention
// smoke (`attention-card-badge`).
import test from 'node:test';
import assert from 'node:assert/strict';
import { fakeDocument } from './fixtures/dom-fixture.mjs';

globalThis.document = fakeDocument;
globalThis.window = { __TAURI__: null, __DECK_DEBUG: false };

const { paintCardAttentionBadge } = await import('../js/attention.js');
const { createAttentionTracker } = await import('../js/attention-model.js');
const { ctx } = await import('../js/state.js');
const { setLocale, t } = await import('../js/i18n.js');

class Badge {
  constructor() { this.hidden = true; this.textContent = ''; this.dataset = {}; this.attrs = {}; this.classes = new Set(); }
  get classList() {
    return { toggle: (name, on) => (on ? this.classes.add(name) : this.classes.delete(name)) };
  }
  get title() { return this.attrs.title; }
  set title(value) { this.attrs.title = value; }
  removeAttribute(name) { delete this.attrs[name]; }
}

test('the badge is painted from the tracker and repainted in place', () => {
  setLocale('en');
  const card = { id: 'c1', session: 's1' };
  const info = agent => [{ name: 's1', alive: true, agent, idle_secs: 1 }];
  ctx.attention = createAttentionTracker();
  const el = new Badge();
  paintCardAttentionBadge(el, card);
  assert.equal(el.hidden, true, 'unknown: nothing shown');
  ctx.attention.record([card], info('needs-input'), new Set(), 1);
  paintCardAttentionBadge(el, card);
  assert.deepEqual([el.hidden, el.dataset.kind, el.textContent], [false, 'input', t('attention.filter.input')]);
  assert.equal(el.title, t('attention.input'), 'the text says the state; color is not the only carrier');
  ctx.attention.record([card], info('turn-done'), new Set(), 2);
  paintCardAttentionBadge(el, card);
  assert.deepEqual([el.hidden, el.dataset.kind, el.textContent], [false, 'done', t('attention.filter.done')]);
  ctx.attention.fail();
  paintCardAttentionBadge(el, card);
  assert.ok(el.classes.has('stale') && el.title.endsWith(t('attention.old')), 'an old snapshot says so');
  ctx.attention.record([card], info('turn-done'), new Set(), 3);
  ctx.attention.saw(card);
  paintCardAttentionBadge(el, card);
  assert.deepEqual([el.hidden, el.dataset.kind, el.textContent, el.title, el.classes.size],
    [true, undefined, '', undefined, 0], 'viewed ending: the same element is cleared, not replaced');
  paintCardAttentionBadge(null, card);
});
