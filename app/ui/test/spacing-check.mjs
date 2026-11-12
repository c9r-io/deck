// spacing-check.mjs — the `ux-spacing` checkpoint of ux-layers-smoke.mjs.
// Opens the main surfaces at the default and the largest font size and
// measures, in the real WKWebView, the space between visible pieces of the
// same layer: controls, bordered boxes and block text. A finding is a pair
// side by side or stacked closer than 4px (overlaps included), a control
// closer than 3px to the inner edge of the bordered box around it, or a
// box that keeps its own parts (a settings group, a list group, the rule
// editor, a scratchpad note) with top and bottom space more than 6px apart,
// or text of three or more characters squeezed into a column narrower than
// two of them (`⫼`, one character per line), a tick or radio touching its
// own words (`☐`), or a floating layer whose content scrolls sideways (`⇹`).
// Designed joins are not findings: repeated siblings (same parent, tag and
// first class: menu items, list rows, tabs) and the parts of a segmented
// control, a split button or a menu. Rects are clipped by scrolling
// ancestors, and a control partly scrolled out of view is not held to its
// box's edge; content behind a closed <details> is skipped (WebKit keeps it
// in layout); floating layers (fixed position, dialogs, menus, drawers) are
// compared only within themselves. A surface with findings is snapshotted
// outlined (`ux-spacing-<surface>-<size>`) and as a table (`…-table`).
export async function runSpacingCheck(h) {
  const { $, state, render, openSession, toggleQueuePanel, refreshQueue, renderBufferUI, openTemplates,
    openSettings, openAutomations, closeAutomations, projectDefaultsDialog, reminderDialog, confirmDialog,
    cfmDone, applyFontScale, pause, snapshot, card, project } = h;
  const { showAttention } = await import('../js/attention.js');
  const { showMentions } = await import('../js/slack-mentions.js');
  const { showSessionCtx } = await import('../js/terminal.js');
  const { backToBoard } = await import('../js/layout.js');
  const { closeDropdownMenu } = await import('../js/dropdown.js');

  const CONTROL = 'button, input:not([type=hidden]), select, textarea, a[href], [role=button], [role=switch], [role=tab], [role=menuitem], summary, .btn, .chip, .badge';
  const TEXT = 'p, label, h1, h2, h3, h4, h5, legend, small, .hint, [class*="hint"], [class*="desc"], [class*="note"], [class*="title"], [class*="label"]';
  const EVEN = '.set-group, .q-group, #auto-editor, .buffer-row';
  const skip = el => !!el.closest('.xterm, #spacing-layer');
  const layerOf = el => {
    for (let n = el; n && n !== document.body; n = n.parentElement) {
      const s = getComputedStyle(n);
      if (s.position === 'fixed' || (s.position === 'absolute' && s.zIndex !== 'auto') || n.getAttribute('role') === 'dialog'
          || n.getAttribute('aria-modal') === 'true' || n.id === 'ctx') return n;
    }
    return document.body;
  };
  const clipped = (el, rect = el.getBoundingClientRect()) => {
    let { left, top, right, bottom } = rect;
    for (let n = el.parentElement; n && n !== document.documentElement; n = n.parentElement) {
      const s = getComputedStyle(n);
      if (s.overflowX !== 'visible' || s.overflowY !== 'visible') {
        const c = n.getBoundingClientRect();
        if (s.overflowX !== 'visible') { left = Math.max(left, c.left); right = Math.min(right, c.right); }
        if (s.overflowY !== 'visible') { top = Math.max(top, c.top); bottom = Math.min(bottom, c.bottom); }
      }
      if (s.position === 'fixed') break;
    }
    left = Math.max(left, 0); top = Math.max(top, 0); right = Math.min(right, innerWidth); bottom = Math.min(bottom, innerHeight);
    return right - left >= 2 && bottom - top >= 2 ? { left, top, right, bottom, width: right - left, height: bottom - top } : null;
  };
  const behindClosedDetails = el => {
    for (let d = el.parentElement?.closest('details'); d; d = d.parentElement?.closest('details')) {
      if (!d.open && !d.querySelector(':scope > summary')?.contains(el)) return true;
    }
    return false;
  };
  const shown = (el, r) => {
    if (behindClosedDetails(el)) return false;
    const s = getComputedStyle(el);
    if (s.visibility === 'hidden' || Number(s.opacity) === 0) return false;
    const hit = document.elementFromPoint(r.left + Math.min(r.width / 2, 8), r.top + r.height / 2);
    return !!hit && (hit === el || el.contains(hit) || hit.contains(el));
  };
  const bordered = el => {
    const s = getComputedStyle(el);
    const width = parseFloat(s.borderTopWidth) + parseFloat(s.borderBottomWidth) + parseFloat(s.borderLeftWidth) + parseFloat(s.borderRightWidth);
    const color = s.borderTopColor;
    return (width >= 2 && !/rgba\([^)]*,\s*0\)$/.test(color) && color !== 'transparent') || (s.boxShadow && s.boxShadow !== 'none');
  };
  const ownText = el => [...el.childNodes].some(n => n.nodeType === 3 && n.textContent.trim());
  const textRect = el => { const range = document.createRange(); range.selectNodeContents(el); return range.getBoundingClientRect(); };
  const textBlock = el => el.matches(TEXT) && ownText(el) && !el.matches(CONTROL) && !el.querySelector(CONTROL)
    && ['block', 'flex', 'grid', 'list-item', 'table-cell'].includes(getComputedStyle(el).display);
  const name = el => {
    const id = el.id ? `#${el.id}` : '';
    const cls = [...el.classList].slice(0, 2).map(c => `.${c}`).join('');
    return `${el.tagName.toLowerCase()}${id}${cls}`;
  };
  const relation = (a, b) => {
    const overlap = (a1, a2, b1, b2) => Math.min(a2, b2) - Math.max(a1, b1);
    const v = overlap(a.top, a.bottom, b.top, b.bottom), x = overlap(a.left, a.right, b.left, b.right);
    if (v >= 0.5 * Math.min(a.height, b.height)) {
      if (b.left >= a.right - 1) return { axis: '↔', gap: b.left - a.right };
      if (a.left >= b.right - 1) return { axis: '↔', gap: a.left - b.right };
      return x > 0 && v > 0 ? { axis: '⧉', gap: -Math.round(Math.min(x, v)) } : null;
    }
    if (x >= 0.3 * Math.min(a.width, b.width)) {
      if (b.top >= a.bottom - 1) return { axis: '↕', gap: b.top - a.bottom };
      if (a.top >= b.bottom - 1) return { axis: '↕', gap: a.top - b.bottom };
    }
    return null;
  };
  // repeated siblings: same parent, tag and first class (a state class like .active is not a kind)
  const sameKind = (a, b) => a.parentElement === b.parentElement && a.tagName === b.tagName
    && (a.classList[0] || '') === (b.classList[0] || '');
  const joinedBox = el => el.matches('.seg, .split, [role=radiogroup], [role=tablist], [role=menu], #ctx');
  const region = el => el.matches('section, aside, .session-tool, .spane-head');

  const findings = [];
  let measured = 0;
  const measure = async (surface, scale) => {
    measured++;
    const pool = new Set([...document.querySelectorAll(CONTROL)]);
    for (const el of document.querySelectorAll('body *')) {
      if (!skip(el) && (bordered(el) || textBlock(el))) pool.add(el);
    }
    const items = [];
    for (const el of pool) {
      if (skip(el)) continue;
      const text = !el.matches(CONTROL) && !bordered(el);
      const r = clipped(el, text ? textRect(el) : el.getBoundingClientRect());
      if (r && shown(el, r)) items.push({ el, r, layer: layerOf(el), text });
    }
    const local = [];
    for (let i = 0; i < items.length; i++) {
      for (let j = i + 1; j < items.length; j++) {
        const A = items[i], B = items[j];
        if (A.layer !== B.layer || A.el.contains(B.el) || B.el.contains(A.el) || (A.text && B.text) || region(A.el) || region(B.el)) continue;
        const rel = relation(A.r, B.r);
        if (!rel || rel.gap >= 4 || sameKind(A.el, B.el)) continue;
        if (A.el.parentElement === B.el.parentElement && joinedBox(A.el.parentElement)) continue;
        local.push({ kind: rel.axis, gap: Math.round(rel.gap), a: A, b: B });
      }
    }
    for (const C of items) {
      if (!C.el.matches(CONTROL)) continue;
      // a control partly scrolled out of its box meets the box's edge by clipping
      const whole = C.el.getBoundingClientRect();
      if (Math.abs(whole.top - C.r.top) > 1 || Math.abs(whole.bottom - C.r.bottom) > 1
          || Math.abs(whole.left - C.r.left) > 1 || Math.abs(whole.right - C.r.right) > 1) continue;
      let box = C.el.parentElement;
      while (box && box !== document.body && !bordered(box)) box = box.parentElement;
      if (!box || box === document.body || joinedBox(box)) continue;
      const r = box.getBoundingClientRect(), s = getComputedStyle(box);
      if (Math.abs(r.width - C.r.width) < 4 && Math.abs(r.height - C.r.height) < 4) continue; // it fills its own frame
      const inset = Math.min(C.r.left - r.left - parseFloat(s.borderLeftWidth), r.right - C.r.right - parseFloat(s.borderRightWidth),
        C.r.top - r.top - parseFloat(s.borderTopWidth), r.bottom - C.r.bottom - parseFloat(s.borderBottomWidth));
      if (inset < 3 && inset > -40) local.push({ kind: '▣', gap: Math.round(inset), a: C, b: { el: box, r } });
    }
    for (const B of items) {
      if (!B.el.matches(EVEN)) continue;
      const whole = B.el.getBoundingClientRect();
      if (Math.abs(whole.top - B.r.top) > 1 || Math.abs(whole.bottom - B.r.bottom) > 1) continue; // partly scrolled away
      const inner = [...B.el.querySelectorAll('*')].map(el => {
        if (!(el.matches(CONTROL) || bordered(el) || ownText(el))) return null;
        const r = clipped(el, el.matches(CONTROL) || bordered(el) ? el.getBoundingClientRect() : textRect(el));
        return r && shown(el, r) ? r : null;
      }).filter(Boolean);
      if (inner.length < 2) continue;
      const top = Math.min(...inner.map(r => r.top)), bottom = Math.max(...inner.map(r => r.bottom));
      const s = getComputedStyle(B.el);
      const above = top - B.r.top - parseFloat(s.borderTopWidth), below = B.r.bottom - parseFloat(s.borderBottomWidth) - bottom;
      if (above >= 0 && below >= 0 && Math.abs(above - below) > 6) local.push({ kind: '▤', gap: `${Math.round(above)}/${Math.round(below)}`, a: B, b: B });
    }
    // crushed text: a run of at least three characters laid out narrower
    // than two of them and taller than two lines, the one-character-per-
    // line column a stretched neighbour leaves (a tick box under the global
    // field width did it twice)
    for (const el of document.querySelectorAll('body *')) {
      if (skip(el) || !ownText(el)) continue;
      const text = [...el.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join('').replace(/\s+/g, '');
      if ([...text].length < 3) continue;
      const r = clipped(el, textRect(el));
      if (!r || !shown(el, r)) continue;
      const s = getComputedStyle(el), em = parseFloat(s.fontSize), line = parseFloat(s.lineHeight) || em * 1.2;
      if (r.width < 2 * em && r.height > 2.5 * line) local.push({ kind: '⫼', gap: Math.round(r.width), a: { el, r }, b: { el, r } });
    }
    // a tick or radio whose own words follow it with no gap (`☐`)
    for (const el of document.querySelectorAll('input[type=checkbox], input[type=radio]')) {
      const words = el.nextSibling;
      if (skip(el) || !words || words.nodeType !== 3 || !words.textContent.trim()) continue;
      const r = clipped(el);
      if (!r || !shown(el, r)) continue;
      const range = document.createRange(); range.selectNodeContents(words);
      const w = range.getBoundingClientRect();
      if (w.width > 0 && Math.abs((w.top + w.bottom) / 2 - (r.top + r.bottom) / 2) < r.height && w.left - r.right < 3) {
        local.push({ kind: '☐', gap: Math.round(w.left - r.right), a: { el, r }, b: { el, r } });
      }
    }
    // a floating layer (dialog, drawer, menu) that scrolls sideways: its
    // content is wider than the box and part of it is cut off (`⇹`)
    for (const el of document.querySelectorAll('body *')) {
      if (skip(el) || layerOf(el) === document.body || el.scrollWidth <= el.clientWidth + 1) continue;
      const s = getComputedStyle(el);
      if (!['auto', 'scroll', 'hidden', 'clip'].includes(s.overflowX) || el.matches('pre, code, textarea, input, select')) continue;
      const r = clipped(el);
      if (r && shown(el, r)) local.push({ kind: '⇹', gap: el.scrollWidth - el.clientWidth, a: { el, r }, b: { el, r } });
    }
    if (!local.length) return;
    findings.push(...local.map(f => `${surface}-${Math.round(scale * 100)} ${f.kind} ${f.gap}px ${name(f.a.el)} ${name(f.b.el)}`));
    const layer = document.createElement('div');
    layer.id = 'spacing-layer';
    layer.style.cssText = 'position:fixed;inset:0;pointer-events:none;z-index:2147483647';
    for (const [n, f] of local.entries()) {
      for (const [r, color] of [[f.a.r, '#e5484d'], [f.b.r, '#f5a524']]) {
        const o = document.createElement('div');
        o.style.cssText = `position:fixed;left:${r.left - 1}px;top:${r.top - 1}px;width:${r.width + 2}px;height:${r.height + 2}px;outline:2px solid ${color};`;
        if (color === '#e5484d') o.textContent = String(n + 1);
        layer.appendChild(o);
      }
    }
    document.body.appendChild(layer);
    await pause(60);
    await snapshot(`ux-spacing-${surface}-${Math.round(scale * 100)}`);
    layer.replaceChildren();
    const table = document.createElement('pre');
    table.style.cssText = 'position:fixed;inset:12px;margin:0;padding:12px;background:#111;color:#eee;font:11px/14px ui-monospace,monospace;white-space:pre-wrap;';
    table.textContent = local.map((f, n) => `${n + 1} ${f.kind} ${f.gap}px  ${name(f.a.el)}  ⟷  ${name(f.b.el)}`).join('\n');
    layer.appendChild(table);
    await pause(60);
    await snapshot(`ux-spacing-${surface}-${Math.round(scale * 100)}-table`);
    layer.remove();
  };

  const closeMenus = () => { try { closeDropdownMenu(); } catch (_) { /* none open */ } $('ctx').style.display = 'none'; };
  const surfaces = [
    ['board', async () => { backToBoard(); state.view = 'board'; render(); }, null],
    ['newmenu', async () => { backToBoard(); render(); $('board-new-more')?.click(); }, closeMenus],
    ['session', async () => { await openSession(card.id); }, null],
    ['cardmenu', async () => {
      await openSession(card.id);
      showSessionCtx({ preventDefault() {}, stopPropagation() {}, currentTarget: null, clientX: 360, clientY: 220 }, card.id);
    }, closeMenus],
    ['lists', async () => { await openSession(card.id); toggleQueuePanel(true); await refreshQueue(); }, () => toggleQueuePanel(false)],
    ['scratchpad', async () => { await openSession(card.id); if ($('buffer-panel').hidden) $('buffer-btn').click(); await pause(200); renderBufferUI(); },
      () => { if (!$('buffer-panel').hidden) $('buffer-close')?.click(); }],
    ['attention', async () => { showAttention(); }, () => $('attention-back')?.click()],
    ['attention-ended', async () => { showAttention(); document.querySelector('#attention-tools [data-filter="ended"]')?.click(); },
      () => { document.querySelector('#attention-tools [data-filter="pending"]')?.click(); $('attention-back')?.click(); }],
    ['mentions', async () => { backToBoard(); showMentions(); }, () => backToBoard()],
    ['templates', async () => { backToBoard(); openTemplates(null); await pause(200); document.querySelector('#tpl-items .tpl-item')?.click(); },
      () => $('tpl-done')?.click()],
    ['defaults', async () => {
      backToBoard();
      projectDefaultsDialog({ projectId: project.id, name: project.name, dir: '/tmp', cmd: 'claude',
        // a long recent command, the chip that once ran past the dialog
        recent: ['claude', '/Applications/Example.app/Contents/MacOS/codex resume --no-daemon --yolo 01a1ca71-c0df-78e0-962d-c370845b80b1'],
        presets: project.presets || [], columns: project.columns });
    }, () => $('pdf-no')?.click()],
    ['automations', async () => { backToBoard(); await openAutomations(); $('auto-drawer').scrollTop = 0; }, () => closeAutomations()],
    ['reminder', async () => { backToBoard(); reminderDialog(null); }, () => document.querySelector('.reminder-overlay .cfm-actions .btn')?.click()],
    ['confirm', async () => { confirmDialog('Close this session? The shell and everything running in it stop.'); }, () => cfmDone(false)],
  ];
  for (const scale of [1, 1.6]) {
    applyFontScale(scale);
    for (const [surface, open, close] of surfaces) {
      try { closeMenus(); await open(); await pause(400); await measure(surface, scale); } catch (_) { findings.push(`${surface}: did not open`); }
      try { await close?.(); } catch (_) { /* best effort */ }
      closeMenus();
      await pause(150);
    }
    try {
      backToBoard(); await openSettings(); await pause(350);
      for (const item of [...document.querySelectorAll('#set-nav .set-nav-item')].filter(el => !el.hidden)) {
        item.click(); await pause(250);
        await measure(`settings-${item.id.replace('set-nav-', '')}`, scale);
      }
      $('set-close')?.click(); await pause(150);
    } catch (_) { findings.push('settings: did not open'); }
  }
  applyFontScale(1);
  return { findings, measured };
}
