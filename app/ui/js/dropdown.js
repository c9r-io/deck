// dropdown.js — deck's own dropdown in place of the native <select> popup
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// WKWebView draws a <select>'s menu with the macOS system popup, which
// matches nothing else in deck. Every <select> in the document is therefore
// progressively ENHANCED: it stays in the DOM as the single source of truth
// (its options, `value`, `selectedIndex`, `change` events, `hidden` and
// `disabled` are what every other module reads and writes, unchanged) and
// is wrapped in a `.dd` whose button shows the selected option's text and
// opens ONE shared `#dd-menu` listbox. Choosing an item sets the select's
// value and dispatches a bubbling `change` on it, so callers never learn
// the dropdown exists. The button follows the select: options rebuilt or
// re-translated (childList / text mutations), `hidden` / `disabled`
// toggled (attribute mutations), and `value` / `selectedIndex` assigned
// (instance-level accessors, because a property write fires no event).
// Selects created later (a list footer's quiet control) are enhanced by
// a document-wide observer. Keyboard: Enter / Space / ↓ open, ↑↓ Home End
// move, Enter / Space choose, Escape closes and returns focus. The menu is
// position:fixed under (or, near the bottom, above) its button and closes
// on any outside pointer, scroll or resize.
import { $ } from './state.js';

const wrappers = new WeakMap();   // select → { wrap, btn, text, sync }
let open = null;                  // { sel, btn } while the menu is shown

const menu = () => $('dd-menu');

const labelOf = sel => {
  const o = sel.options[sel.selectedIndex];
  return o ? o.textContent : '';
};

function closeMenu(refocus = false) {
  if (!open) return;
  const { btn } = open;
  open = null;
  const m = menu();
  m.hidden = true;
  m.innerHTML = '';
  btn.setAttribute('aria-expanded', 'false');
  if (refocus) btn.focus();
}

function place(m, btn) {
  const r = btn.getBoundingClientRect();
  const gap = 4;
  m.style.minWidth = Math.ceil(r.width) + 'px';
  m.style.left = '0px'; m.style.top = '0px';
  const h = m.offsetHeight, w = m.offsetWidth;
  const below = r.bottom + gap + h <= window.innerHeight;
  const top = below ? r.bottom + gap : Math.max(gap, r.top - gap - h);
  const left = Math.max(gap, Math.min(r.left, window.innerWidth - w - gap));
  m.style.left = left + 'px';
  m.style.top = top + 'px';
}

function openMenu(sel, btn) {
  closeMenu();
  const m = menu();
  m.innerHTML = '';
  const items = [];
  [...sel.options].forEach((o, i) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'dd-item' + (i === sel.selectedIndex ? ' on' : '');
    b.setAttribute('role', 'option');
    b.setAttribute('aria-selected', i === sel.selectedIndex ? 'true' : 'false');
    b.textContent = o.textContent;
    b.disabled = o.disabled;
    b.onclick = () => {
      if (sel.selectedIndex !== i) {
        sel.selectedIndex = i;
        sel.dispatchEvent(new Event('change', { bubbles: true }));
      }
      closeMenu(true);
    };
    m.appendChild(b);
    items.push(b);
  });
  open = { sel, btn };
  m.hidden = false;
  place(m, btn);
  btn.setAttribute('aria-expanded', 'true');
  (items[sel.selectedIndex] || items[0])?.focus();
}

function menuKeydown(event) {
  if (!open) return;
  const items = [...menu().querySelectorAll('.dd-item:not(:disabled)')];
  const at = items.indexOf(document.activeElement);
  const go = i => { items[Math.max(0, Math.min(items.length - 1, i))]?.focus(); };
  switch (event.key) {
    case 'ArrowDown': event.preventDefault(); go(at + 1); break;
    case 'ArrowUp': event.preventDefault(); go(at - 1); break;
    case 'Home': event.preventDefault(); go(0); break;
    case 'End': event.preventDefault(); go(items.length - 1); break;
    /* Escape is consumed here: the document's own Escape leaves the session view */
    case 'Escape': event.preventDefault(); event.stopPropagation(); closeMenu(true); break;
    case 'Tab': closeMenu(); break;
    default: break;
  }
}

export function enhanceSelect(sel) {
  if (wrappers.has(sel) || !sel.parentNode) return;
  const wrap = document.createElement('span');
  wrap.className = 'dd';
  if (sel.id) wrap.dataset.for = sel.id;
  const btn = document.createElement('button');
  btn.type = 'button';
  btn.className = 'dd-btn';
  btn.setAttribute('aria-haspopup', 'listbox');
  btn.setAttribute('aria-expanded', 'false');
  const text = document.createElement('span');
  text.className = 'dd-text';
  const chev = document.createElement('span');
  chev.className = 'dd-chev';
  chev.setAttribute('aria-hidden', 'true');
  btn.append(text, chev);
  sel.parentNode.insertBefore(wrap, sel);
  wrap.append(sel, btn);
  const sync = () => {
    text.textContent = labelOf(sel);
    btn.disabled = sel.disabled;
    wrap.hidden = sel.hidden;
    /* the select's own label, for assistive tech, is the wrapper's too */
    const labelled = sel.getAttribute('aria-label') || (sel.id && document.querySelector(`label[for="${sel.id}"]`)?.textContent);
    if (labelled) btn.setAttribute('aria-label', labelled.trim());
  };
  wrappers.set(sel, { wrap, btn, text, sync });
  /* a property write fires no event: hook the instance so the label follows */
  for (const prop of ['value', 'selectedIndex']) {
    const d = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, prop);
    Object.defineProperty(sel, prop, {
      configurable: true,
      get() { return d.get.call(this); },
      set(v) { d.set.call(this, v); sync(); },
    });
  }
  new MutationObserver(sync).observe(sel, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ['hidden', 'disabled', 'aria-label'] });
  sel.addEventListener('change', sync);
  btn.onclick = event => {
    event.stopPropagation();
    if (open && open.sel === sel) closeMenu(); else openMenu(sel, btn);
  };
  btn.addEventListener('keydown', event => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      openMenu(sel, btn);
    }
  });
  sync();
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initDropdowns(root = document) {
  root.querySelectorAll('select').forEach(enhanceSelect);
  new MutationObserver(records => {
    for (const r of records) {
      for (const n of r.addedNodes) {
        if (!(n instanceof Element)) continue;
        if (n.tagName === 'SELECT') enhanceSelect(n);
        else n.querySelectorAll?.('select').forEach(enhanceSelect);
      }
    }
  }).observe(root.body || root, { childList: true, subtree: true });
  const m = menu();
  m.addEventListener('keydown', menuKeydown);
  document.addEventListener('pointerdown', event => {
    if (open && !m.contains(event.target) && event.target !== open.btn && !open.btn.contains(event.target)) closeMenu();
  }, true);
  document.addEventListener('scroll', event => { if (event.target !== m) closeMenu(); }, true);
  window.addEventListener('resize', () => closeMenu());
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape' && open && !m.contains(document.activeElement)) {
      event.stopPropagation();
      closeMenu(true);
    }
  }, true);
}
