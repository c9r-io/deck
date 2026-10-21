// terminal-find.js — one pane's find bar (⌘F) over tmux's own search.
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// The decisions live in terminal-find-model.js and the search in
// `terminal_search.rs`; this controller owns the bar's DOM and the request
// order.
// - The bar belongs to one pane and is built on its first open. Typing
//   searches after FIND_DEBOUNCE_MS (never mid-composition: an IME preedit
//   is not a query); Enter / ⌘G steps to an older match, ⇧Enter / ⇧⌘G to a
//   newer one; Esc and ✕ close it.
// - At most one request is in flight. A request asked for meanwhile replaces
//   any earlier waiting one and runs when the reply lands, against the query
//   as it is then.
// - Closing keeps the view where it is (the match stays on screen, the
//   scrollback chip says the view is frozen) and keeps the query for the
//   next open. Clearing the query of a bar that searched goes live.
// - Every reply goes to `onResult`, which the pane uses for its copy-mode
//   presentation (layout.js). A refusal or an unattached pane shows
//   "unavailable"; nothing here retries.
import { FIND_DEBOUNCE_MS, findQueryState, findStatus, findStep } from './terminal-find-model.js';

const composing = event => !!(event?.isComposing || event?.keyCode === 229);

export function createTerminalFind(options) {
  const { mount, run, live } = options;
  const doc = options.doc ?? globalThis.document;
  const onResult = options.onResult ?? (() => {});
  const available = options.available ?? (() => true);
  const focusTerminal = options.focusTerminal ?? (() => {});
  const t = options.t ?? (key => key);
  const setTimer = options.setTimer ?? ((fn, ms) => setTimeout(fn, ms));
  const clearTimer = options.clearTimer ?? (id => clearTimeout(id));
  let el = null, input = null, label = null;
  let timer = null, busy = false, waiting = null;
  let last = null, lastQuery = null, searched = false;
  let shown = { key: '', screenOnly: false };

  const paint = status => {
    shown = status;
    if (!label) return;
    const text = status.key ? t(status.key, { count: status.count }) : '';
    label.textContent = status.screenOnly && text ? `${text} · ${t('find.screenOnly')}` : text;
    el.dataset.state = status.key === 'find.none' || status.key === 'find.unavailable'
      || status.key === 'find.invalid' ? 'miss' : '';
  };

  const button = (cls, text, title, onclick) => {
    const node = doc.createElement('button');
    node.className = cls;
    node.type = 'button';
    node.textContent = text;
    node.title = title;
    node.setAttribute('aria-label', title);
    node.addEventListener('mousedown', event => event.preventDefault());
    node.onclick = event => { event?.stopPropagation?.(); onclick(); };
    return node;
  };

  function build() {
    el = doc.createElement('div');
    el.className = 'tfind';
    el.setAttribute('role', 'search');
    input = doc.createElement('input');
    input.className = 'tfind-q';
    input.type = 'text';
    input.spellcheck = false;
    input.setAttribute('autocomplete', 'off');
    input.setAttribute('aria-label', t('find.placeholder'));
    input.placeholder = t('find.placeholder');
    label = doc.createElement('span');
    label.className = 'tfind-n';
    label.setAttribute('aria-live', 'polite');
    el.append(input, label,
      button('tfind-older', '↑', t('find.older'), () => request('older')),
      button('tfind-newer', '↓', t('find.newer'), () => request('newer')),
      button('tfind-x', '✕', t('find.close'), () => close('user')));
    input.addEventListener('input', event => { if (!composing(event)) schedule(); });
    input.addEventListener('compositionend', schedule);
    input.addEventListener('keydown', keydown);
    // the pane head's mousedown focuses the terminal; the bar keeps focus
    el.addEventListener('mousedown', event => event.stopPropagation());
    mount.appendChild(el);
  }

  function keydown(event) {
    if (composing(event)) return;
    const key = String(event.key || '');
    let action = null;
    if (key === 'Enter') action = event.shiftKey ? 'newer' : 'older';
    else if (key.toLowerCase() === 'g' && event.metaKey) action = event.shiftKey ? 'newer' : 'older';
    else if (key === 'Escape') action = 'close';
    if (!action) return;
    event.preventDefault();
    event.stopPropagation();
    if (action === 'close') close('user');
    else request(action);
  }

  function schedule() {
    if (timer != null) clearTimer(timer);
    timer = setTimer(() => { timer = null; request('start'); }, FIND_DEBOUNCE_MS);
  }

  function request(direction) {
    if (!el || el.hidden) return;
    if (direction !== 'start' && timer != null) {
      clearTimer(timer);
      timer = null;
      direction = 'start';
    }
    if (busy) {
      waiting = waiting === 'start' ? 'start' : direction;
      return;
    }
    send(direction);
  }

  function send(direction) {
    const query = input.value;
    const state = findQueryState(query);
    if (!available()) { paint(findStatus({ unavailable: true })); return; }
    if (state === 'empty') {
      last = null; lastQuery = null;
      paint(findStatus({ state }));
      if (searched) { searched = false; live(); }
      return;
    }
    if (state !== 'ok') { paint(findStatus({ state })); return; }
    const next = findStep({ last, lastQuery, query, direction });
    busy = true;
    Promise.resolve()
      .then(() => run({ query, step: next.step, turn: next.turn }))
      .then(result => {
        last = next.last; lastQuery = query; searched = true;
        paint(findStatus({ result }));
        onResult(result);
      }, () => {
        last = null; lastQuery = null;
        paint(findStatus({ error: true }));
      })
      .finally(() => {
        busy = false;
        const again = waiting;
        waiting = null;
        if (again && el && !el.hidden) send(again);
      });
  }

  function open() {
    if (!el) build();
    el.hidden = false;
    if (!available()) paint(findStatus({ unavailable: true }));
    else if (shown.key === 'find.unavailable') paint(findStatus({}));
    input.focus();
    input.select();
  }

  function close(reason = 'user') {
    if (timer != null) { clearTimer(timer); timer = null; }
    waiting = null;
    if (!el || el.hidden) return;
    el.hidden = true;
    if (reason === 'user') focusTerminal();
  }

  return {
    open,
    close,
    isOpen: () => !!el && !el.hidden,
    dispose() { close('dispose'); el?.remove(); el = null; },
    view: () => ({ open: !!el && !el.hidden, query: input?.value ?? '', label: label?.textContent ?? '', state: el?.dataset.state ?? '' }),
  };
}
