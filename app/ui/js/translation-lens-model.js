// DOM-free Local Translation coordinator presentation state. A completed Live
// snapshot may be stale, but its source stays paired with its translation.
export class TranslationLensModel {
  constructor() {
    this.open = false; this.mode = 'live'; this.paused = false;
    this.sourceRevision = 0; this.displayRevision = null;
    this.source = ''; this.pane = null; this.result = ''; this.displaySource = '';
    this.running = null; this.pending = null; this.error = null;
  }
  isUpdating() { return this.mode === 'live' && !this.paused && !!this.result
    && this.displayRevision !== this.sourceRevision; }
  sourceForCopy() { return this.result ? this.displaySource : ''; }
  clearDisplay() { this.result = ''; this.displaySource = ''; this.displayRevision = null; }
  show(mode = 'live') { this.open = true; this.mode = mode; this.paused = false;
    this.source = ''; this.pane = null; this.running = null; this.pending = null;
    this.error = null; this.sourceRevision++; this.clearDisplay(); }
  close() { this.open = false; this.paused = false; this.mode = 'live';
    this.source = ''; this.pane = null; this.running = null; this.pending = null;
    this.error = null; this.sourceRevision++; this.clearDisplay(); }
  modeTo(mode) { if (!['live', 'clipboard'].includes(mode)) return false;
    this.mode = mode; this.paused = false; this.source = ''; this.pane = null;
    this.running = null; this.pending = null; this.error = null;
    this.sourceRevision++; this.clearDisplay(); return true; }
  observeLive(text, pane, maxBytes = 4096) {
    if (!this.open || this.mode !== 'live' || this.paused) return null;
    if (text === this.source && pane === this.pane) return null;
    this.source = text; this.pane = pane; this.sourceRevision++; this.error = null;
    if (!text.trim()) { this.pending = null; this.clearDisplay(); return null; }
    if (new TextEncoder().encode(text).length > maxBytes) {
      this.pending = null; this.error = 'view-too-large'; return null;
    }
    const ticket = { revision: this.sourceRevision, text, pane, mode: 'live' };
    if (this.running) { this.pending = ticket; return null; }
    this.running = ticket; return ticket;
  }
  snapshot(text, mode) {
    if (!this.open || !['selection', 'clipboard'].includes(mode)) return null;
    this.mode = mode; this.paused = false; this.source = text; this.pane = null;
    this.sourceRevision++; this.clearDisplay(); this.error = null; this.pending = null;
    const ticket = { revision: this.sourceRevision, text, pane: null, mode };
    this.running = ticket; return ticket;
  }
  finish(ticket, text, error = null) {
    if (ticket !== this.running) return { accepted: false, next: null };
    this.running = null;
    const current = this.open && !this.paused && ticket.mode === this.mode;
    let accepted = false;
    if (current && text && (this.mode !== 'live' || ticket.revision > (this.displayRevision ?? -1))) {
      this.result = text; this.displaySource = ticket.text;
      this.displayRevision = ticket.revision;
      if (!(this.mode === 'live' && this.sourceRevision !== ticket.revision && this.error === 'view-too-large')) {
        this.error = null;
      }
      accepted = true;
    } else if (current && error && !this.result) { this.error = error; }
    const next = current && this.mode === 'live' && this.pending ? this.pending : null;
    this.pending = null;
    if (next) this.running = next;
    return { accepted, next };
  }
  pause() { if (!this.open || this.mode !== 'live' || this.paused) return false;
    this.paused = true; this.pending = null; this.running = null; return true; }
  resume() { if (!this.open || this.mode !== 'live') return false;
    this.paused = false; this.source = ''; this.pane = null; this.pending = null;
    this.running = null; this.error = null; return true; }
}

export function translationShortcutAction(hasSelection, lensOpen) {
  return hasSelection ? 'selection' : lensOpen ? 'close' : 'live';
}

// First dirty event schedules a capture without trailing debounce. While
// writes continue the next capture cannot be postponed indefinitely.
export class LiveCadence {
  constructor({ onCapture, clock = globalThis }) {
    this.onCapture = onCapture; this.clock = clock; this.timer = null;
    this.lastCapture = null; this.generation = 0;
  }
  dirty() {
    if (this.timer !== null) return;
    const now = this.clock.now();
    const delay = this.lastCapture === null ? 350 : Math.max(0, 650 - (now - this.lastCapture));
    const generation = this.generation;
    this.timer = this.clock.setTimeout(() => {
      this.timer = null; if (generation !== this.generation) return;
      this.lastCapture = this.clock.now(); this.onCapture();
    }, delay);
  }
  stop() { this.generation++; if (this.timer !== null) this.clock.clearTimeout(this.timer);
    this.timer = null; this.lastCapture = null; }
}
