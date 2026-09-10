// Minimal document shared by production DOM event contract tests.
export class FakeElement {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.listeners = new Map();
    this.style = {};
    this.dataset = {};
    this.hidden = false;
    this.disabled = false;
    this.classList = { add() {}, remove() {}, toggle() {}, contains() { return false; } };
    this.isConnected = true;
    this.value = '';
    this._textContent = '';
  }
  addEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    list.push(fn); this.listeners.set(type, list);
  }
  fire(type, extra = {}) {
    const event = {
      key: '', keyCode: 0, isComposing: false,
      prevented: 0, stopped: 0,
      preventDefault() { this.prevented++; },
      stopPropagation() { this.stopped++; },
      ...extra,
    };
    for (const fn of this.listeners.get(type) || []) fn(event);
    if (typeof this[`on${type}`] === 'function') this[`on${type}`](event);
    return event;
  }
  get options() { return this.children; }
  setSelectionRange(start, end) { this.selectionStart = start; this.selectionEnd = end; }
  querySelectorAll(selector) {
    if (selector === '[data-voice-layout]') return this.children.filter(child => child.dataset.voiceLayout);
    return [];
  }
  replaceChildren(...nodes) { this.children = nodes; }
  appendChild(node) { this.children.push(node); return node; }
  append(node) { this.children.push(node); }
  remove() { this.isConnected = false; }
  focus() { fakeDocument.activeElement = this; }
  select() { this.selected = true; }
  closest() { return null; }
  setAttribute(name, value) { this[name] = value; }
  set textContent(value) { this._textContent = String(value); this.children = []; }
  get textContent() { return this._textContent; }
}

export const ids = new Map();
export const documentListeners = new Map();
export const fakeDocument = {
  activeElement: null,
  addEventListener(type, fn) {
    const list = documentListeners.get(type) || [];
    list.push(fn); documentListeners.set(type, list);
  },
  fire(type, extra = {}) {
    const event = {
      key: '', keyCode: 0, isComposing: false, prevented: 0, stopped: 0,
      preventDefault() { this.prevented++; },
      stopPropagation() { this.stopped++; },
      ...extra,
    };
    for (const fn of documentListeners.get(type) || []) fn(event);
    return event;
  },
  createElement: tag => new FakeElement(tag),
  getElementById(id) {
    if (!ids.has(id)) ids.set(id, new FakeElement());
    return ids.get(id);
  },
};
