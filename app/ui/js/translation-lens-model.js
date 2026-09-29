// DOM-free Local Translation coordinator state. A context (open, mode, pane)
// has an epoch; a ticket from an older epoch never reaches the display.
// A completed snapshot may trail the current source, but its source text
// stays paired with its translation. Observing a source is not the same as
// translating it: a failed or interrupted source is attempted again on a
// fresh user intent, and transient failures retry a bounded number of times.
// Reading never pauses translation; a live DOM selection may only defer
// *showing* a newer result until the selection or focus ends.
export const FIRST_OUTPUT_MS = 350;
export const OUTPUT_INTERVAL_MS = 650;
export const SCROLL_SETTLE_MS = 250;
export const RETRY_DELAYS_MS = Object.freeze([400, 1200]);
const TRANSIENT = new Set(['translation-failed']);
const FRAME = Object.freeze({ layoutFrame: true });
const bytes = text => new TextEncoder().encode(text).length;

export class TranslationLensModel {
  constructor() {
    this.open = false; this.epoch = 0; this.sourceRevision = 0;
    this.reset('live');
  }
  reset(mode) {
    this.mode = mode; this.epoch++; this.sourceRevision++;
    this.source = ''; this.pane = null; this.observed = false;
    this.running = null; this.pending = null; this.display = null; this.shown = null; this.holding = false;
    this.error = null; this.failure = null; this.clipboardReady = false;
  }
  show(mode = 'live') { this.open = true; this.reset(mode); }
  close() { this.open = false; this.reset('live'); }
  modeTo(mode) {
    if (!this.open || !['live', 'clipboard'].includes(mode) || mode === this.mode) return false;
    this.reset(mode); return true;
  }
  // The focused pane changed: another pane's text never stays on screen.
  retarget() { if (!this.open || this.mode !== 'live') return false; this.reset('live'); return true; }
  // Focus loss or a hidden window drops queued work; the display remains.
  interrupt() { this.running = null; this.pending = null; if (this.failure) this.failure.scheduled = false; }
  working() { return !!(this.running || this.pending || this.failure?.scheduled); }
  covers(revision) {
    return this.running?.revision === revision || this.pending?.revision === revision
      || this.display?.revision === revision
      || (this.error?.revision === revision && !TRANSIENT.has(this.error.code));
  }
  enqueue(ticket) {
    if (this.running) { this.pending = ticket; return null; }
    this.running = ticket; return ticket;
  }
  // `intent` is a fresh user expression (open, tab, focus return, pane
  // switch): it re-attempts the same source unless work already covers it.
  observeLive(text, pane, { intent = false, maxBytes = 4096 } = {}) {
    if (!this.open || this.mode !== 'live') return null;
    if (this.observed && pane !== this.pane) this.reset('live');
    if (this.observed && text === this.source) {
      if (!intent || this.covers(this.sourceRevision)) return null;
    } else {
      this.source = text; this.pane = pane; this.observed = true; this.sourceRevision++;
    }
    this.error = null; this.failure = null;
    if (!text.trim()) { this.pending = null; this.display = null; this.shown = null; return null; }
    if (bytes(text) > maxBytes) {
      this.pending = null; this.error = { code: 'view-too-large', revision: this.sourceRevision };
      return null;
    }
    return this.enqueue({ epoch: this.epoch, revision: this.sourceRevision, text, pane, mode: 'live' });
  }
  // Selection and copied text are explicit snapshots: they replace queued
  // work, never follow the terminal, and are refused (not truncated) oversize.
  snapshot(text, mode, maxBytes = 16384) {
    if (!this.open || !['selection', 'clipboard'].includes(mode)) return null;
    if (mode !== this.mode) this.reset(mode);
    this.source = text; this.observed = true; this.sourceRevision++;
    this.error = null; this.failure = null; this.pending = null; this.running = null;
    if (!text.trim()) { this.error = { code: 'text-empty', revision: this.sourceRevision }; return null; }
    if (bytes(text) > maxBytes) {
      this.error = { code: 'text-too-large', revision: this.sourceRevision }; return null;
    }
    this.running = { epoch: this.epoch, revision: this.sourceRevision, text, pane: null, mode };
    return this.running;
  }
  // A closed failure observed outside a translation (clipboard read).
  fail(code) {
    if (!this.open) return;
    this.sourceRevision++; this.error = { code, revision: this.sourceRevision };
  }
  finish(ticket, text, error = null) {
    const none = { accepted: false, next: null, retry: null };
    if (!ticket || ticket !== this.running) return none;
    this.running = null;
    if (!this.open || ticket.epoch !== this.epoch) return none;
    let accepted = false, retry = null;
    if (text) {
      if (!this.display || ticket.revision > this.display.revision) {
        this.display = { text, source: ticket.text, revision: ticket.revision };
        accepted = true;
      }
      if (this.error && this.error.revision <= ticket.revision) this.error = null;
      if (this.failure && this.failure.ticket.revision <= ticket.revision) this.failure = null;
    } else if (error) {
      if (!this.error || this.error.revision <= ticket.revision) this.error = { code: error, revision: ticket.revision };
      if (TRANSIENT.has(error) && ticket.revision === this.sourceRevision && !this.pending) {
        const attempts = (this.failure?.ticket.revision === ticket.revision ? this.failure.attempts : 0) + 1;
        this.failure = { ticket, attempts, scheduled: attempts <= RETRY_DELAYS_MS.length };
        if (this.failure.scheduled) retry = RETRY_DELAYS_MS[attempts - 1];
      }
    }
    const next = this.pending; this.pending = null;
    if (next) this.running = next;
    return { accepted, next, retry };
  }
  retry() {
    const failure = this.failure;
    if (!this.open || !failure?.scheduled || this.running) return null;
    failure.scheduled = false;
    const { ticket } = failure;
    if (ticket.epoch !== this.epoch || ticket.revision !== this.sourceRevision) return null;
    this.error = null;
    return this.enqueue({ ...ticket });
  }
  // What the result area shows. While `held` (a live selection in the
  // result) the previous snapshot stays; Copy Source follows what is shown.
  present(held = false) {
    this.holding = held && !!this.shown;
    if (!this.holding) this.shown = this.display;
    return this.shown;
  }
  heldBack() { return !!this.holding && this.shown !== this.display; }
  sourceForCopy() { return this.shown?.source || ''; }
  resultText() { return this.shown?.text || ''; }
  // Deck's own copies (buttons, ⌘C inside the result) never feed back.
  ownText(text) {
    const views = [this.shown, this.display].filter(Boolean);
    return views.some(view => text === view.source || text === view.text
      || (text.length > 0 && view.text.includes(text)));
  }
  newCopy(text) { return !this.ownText(text) && (text !== this.source || !!this.error); }
  isUpdating() { return !!this.display && this.display.revision !== this.sourceRevision && this.working(); }
  // One closed status, in priority order: error, not-ready, empty, work, stale, ready.
  status({ active = true } = {}) {
    if (!this.open) return null;
    if (this.error) return `translation.error.${this.error.code}`;
    if (!active && this.mode !== 'selection' && !this.working()) {
      return this.display ? 'translation.background' : 'translation.backgroundEmpty';
    }
    if (this.mode === 'clipboard' && !this.observed) {
      return this.clipboardReady ? 'translation.waiting' : 'translation.preparing';
    }
    if (this.mode === 'live' && !this.observed) return 'translation.preparing';
    if (this.mode === 'live' && !this.source.trim()) return 'translation.empty';
    if (!this.display) return 'translation.translating';
    if (this.heldBack()) return 'translation.held';
    if (this.display.revision !== this.sourceRevision) {
      return this.working() ? 'translation.updating' : 'translation.stale';
    }
    return this.mode === 'selection' ? 'translation.selected' : 'translation.ready';
  }
}

export function translationShortcutAction(hasSelection, lensOpen) {
  return hasSelection ? 'selection' : lensOpen ? 'close' : 'live';
}

// Live capture cadence. Three distinct triggers, one timer at a time:
// - intent: a user action (open, tab, focus, pane) captures after layout frames;
// - output: bounded throttle — continuous writes cannot postpone a capture forever;
// - scroll: trailing debounce on a raw user wheel gesture — no capture of
//   intermediate positions; output during the gesture waits for its end.
export class LiveCadence {
  constructor({ onCapture, clock = globalThis }) {
    this.onCapture = onCapture; this.clock = clock; this.timer = null; this.kind = null;
    this.lastCapture = null; this.generation = 0; this.scrolling = false; this.intentCarried = false;
  }
  arm(kind, delay) {
    const generation = this.generation;
    this.kind = kind;
    this.timer = this.clock.setTimeout(() => {
      this.timer = null; this.kind = null;
      if (generation !== this.generation) return;
      const intent = kind === 'intent' || this.intentCarried;
      if (kind === 'scroll') this.scrolling = false;
      this.intentCarried = false; this.lastCapture = this.clock.now();
      this.onCapture({ intent, kind });
    }, delay);
  }
  clear() {
    if (this.timer !== null && this.timer !== FRAME) this.clock.clearTimeout(this.timer);
    this.timer = null; this.kind = null;
  }
  intent() {
    if (this.scrolling) { this.intentCarried = true; return; }
    this.clear();
    const generation = ++this.generation;
    const frame = this.clock.frame || (fn => this.clock.setTimeout(fn, 16));
    // Two frames: the dock/overlay layout and the terminal fit it causes.
    this.kind = 'intent'; this.timer = FRAME;
    frame(() => frame(() => {
      if (generation !== this.generation) return;
      this.timer = null; this.arm('intent', 0);
    }));
  }
  output() {
    if (this.scrolling || this.timer !== null) return;
    const now = this.clock.now();
    this.arm('output', this.lastCapture === null ? FIRST_OUTPUT_MS
      : Math.max(0, OUTPUT_INTERVAL_MS - (now - this.lastCapture)));
  }
  scroll() {
    if (this.kind === 'intent') this.intentCarried = true;
    this.clear(); this.generation++;
    this.scrolling = true; this.arm('scroll', SCROLL_SETTLE_MS);
  }
  stop() {
    this.generation++; this.clear(); this.scrolling = false; this.intentCarried = false;
    this.lastCapture = null;
  }
}
