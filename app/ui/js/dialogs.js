// dialogs.js — confirm/prompt/choice dialogs, project defaults, toasts, inline rename
// The settings modal and everything it persists live in settings.js, which
// imports these primitives; nothing here knows the settings document.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, ctx, genId } from './state.js';
import { inlineRenameValue, isComposingKeyEvent } from './pure.js';
import { t } from './i18n.js';
import { normalizeTaskPreset, normalizeTaskPresets } from './connector-model.js';

/* ---------- confirm dialog (window.confirm is a silent no-op in WKWebView) ---------- */
let confirmPointerOnly = false;
export function confirmDialog(msg) {
  return new Promise(resolve => {
    confirmPointerOnly = false;
    ctx.cfmResolve = resolve;
    $('cfm-yes').textContent = t('common.confirm');
    $('cfm-msg').textContent = msg;
    $('cfm').style.display = 'flex';
    $('cfm-yes').focus();
  });
}
/* High-risk scheduler actions must not be accepted by an ordinary Enter or
   blur. The safe button receives focus and only an explicit activation of
   the confirm button can accept. */
export function confirmDangerDialog(msg, confirmLabel = t('common.confirm')) {
  return new Promise(resolve => {
    confirmPointerOnly = true;
    ctx.cfmResolve = resolve;
    $('cfm-yes').textContent = confirmLabel;
    $('cfm-msg').textContent = msg;
    $('cfm').style.display = 'flex';
    $('cfm-no').focus();
  });
}
export function cfmDone(v) {
  $('cfm').style.display = 'none';
  confirmPointerOnly = false;
  if (ctx.cfmResolve) { ctx.cfmResolve(v); ctx.cfmResolve = null; }
}

/* ---------- choice dialog: one question, explicit answers ----------
   Used where deck must not guess (a directory that no longer exists).
   Resolves the chosen id, or null on Cancel / Escape / a click outside.
   Enter is not bound: the focused button (the caller's primary choice)
   receives it natively, so a stray Enter can only take that one choice. */
let chdResolve = null;
export function choiceDialog(msg, choices) {
  return new Promise(resolve => {
    if (chdResolve) chdResolve(null);
    $('chd-msg').textContent = msg;
    const actions = $('chd-actions');
    actions.replaceChildren();
    const done = v => { $('chd').style.display = 'none'; $('chd').onkeydown = null; chdResolve = null; resolve(v); };
    chdResolve = done;
    const cancel = document.createElement('button');
    cancel.type = 'button'; cancel.className = 'btn'; cancel.textContent = t('common.cancel');
    cancel.onclick = () => done(null);
    actions.appendChild(cancel);
    let primary = null;
    for (const c of choices) {
      const b = document.createElement('button');
      b.type = 'button'; b.className = 'btn' + (c.primary ? ' primary' : ''); b.textContent = c.label;
      b.onclick = () => done(c.id);
      actions.appendChild(b);
      if (c.primary && !primary) primary = b;
    }
    $('chd').onkeydown = e => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(null); } };
    $('chd').style.display = 'flex';
    (primary || cancel).focus();
  });
}

/* ---------- project defaults dialog (04 A v01) ----------
   The project's default directory and launch command, edited together.
   Resolves { dir, cmd } (trimmed; blank = no default) on Save / Enter, null
   on Cancel / Escape / a click outside. `recent` are command chips that only
   FILL the command field — nothing in this dialog runs anything. */
let pdfResolve = null;
export function projectDefaultsDialog({ name, dir = '', cmd = '', recent = [], presets = [], columns = [] }) {
  return new Promise(resolve => {
    if (pdfResolve) pdfResolve(null);
    $('pdf-title').textContent = t('projectDefaults.title', { name });
    const dirInput = $('pdf-dir'), cmdInput = $('pdf-cmd');
    dirInput.value = dir; cmdInput.value = cmd;
    const chips = $('pdf-chips');
    chips.replaceChildren();
    for (const c of recent.slice(0, 6)) {
      const b = document.createElement('button');
      b.type = 'button'; b.className = 'pdf-chip'; b.textContent = c;
      b.onclick = () => { cmdInput.value = c; cmdInput.focus(); };
      chips.appendChild(b);
    }
    chips.hidden = !recent.length;
    let draftPresets = normalizeTaskPresets(presets, columns); let editingPreset = null;
    const editor = $('pdf-preset-editor');
    const renderPresets = () => {
      const list = $('pdf-presets'); list.replaceChildren();
      for (const preset of draftPresets) {
        const button = document.createElement('button'); button.type = 'button'; button.className = 'btn'; button.textContent = preset.name;
        button.onclick = () => openPreset(preset); list.appendChild(button);
      }
      $('pdf-preset-add').disabled = draftPresets.length >= 50;
    };
    const openPreset = preset => {
      editingPreset = preset?.id || genId('R');
      $('pdf-preset-name').value = preset?.name || '';
      $('pdf-preset-title').value = preset?.title || '';
      $('pdf-preset-dir').value = preset?.dir || dirInput.value.trim();
      $('pdf-preset-cmd').value = preset?.cmd || cmdInput.value.trim() || 'codex';
      $('pdf-preset-steps').value = (preset?.steps || []).join('\n');
      const target = $('pdf-preset-column'); target.replaceChildren();
      for (const column of columns) { const option = document.createElement('option'); option.value = column.id; option.textContent = column.name; target.appendChild(option); }
      target.value = preset?.columnId || columns[0]?.id || '';
      $('pdf-preset-delete').hidden = !preset;
      editor.hidden = false; $('pdf-preset-name').focus();
    };
    const commitPreset = () => {
      if (!editingPreset) return true;
      const preset = normalizeTaskPreset({ id: editingPreset, name: $('pdf-preset-name').value,
        columnId: $('pdf-preset-column').value, title: $('pdf-preset-title').value,
        dir: $('pdf-preset-dir').value, cmd: $('pdf-preset-cmd').value,
        steps: $('pdf-preset-steps').value.split('\n') }, columns);
      if (!preset) { toast(t('presets.invalid')); return false; }
      draftPresets = [...draftPresets.filter(value => value.id !== editingPreset), preset];
      editingPreset = null; editor.hidden = true; renderPresets(); return true;
    };
    $('pdf-preset-add').onclick = () => openPreset(null);
    $('pdf-preset-done').onclick = commitPreset;
    $('pdf-preset-delete').onclick = () => {
      draftPresets = draftPresets.filter(value => value.id !== editingPreset);
      editingPreset = null; editor.hidden = true; renderPresets();
    };
    renderPresets(); editor.hidden = true;
    const read = () => commitPreset() ? ({ dir: dirInput.value.trim(), cmd: cmdInput.value.trim(),
      ...(draftPresets.length || presets.length ? { presets: draftPresets } : {}) }) : null;
    const done = v => { $('pdf').style.display = 'none'; $('pdf').onkeydown = null; pdfResolve = null; resolve(v); };
    pdfResolve = done;
    $('pdf-yes').onclick = () => { const value = read(); if (value) done(value); };
    $('pdf-no').onclick = () => done(null);
    $('pdf').onkeydown = e => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(null); return; }
      if (e.key === 'Enter' && e.target && e.target.tagName === 'INPUT') {
        if (isComposingKeyEvent(e)) return;
        e.preventDefault(); e.stopPropagation();
        const value = read(); if (value) done(value);
      }
    };
    $('pdf').style.display = 'flex';
    dirInput.focus();
    dirInput.select();
  });
}

/* ---------- toasts ---------- */
export function toast(msg) {
  const el = document.createElement('div');
  el.className = 'toast';
  el.textContent = msg;
  $('toasts').appendChild(el);
  setTimeout(() => el.remove(), 2600);
}

/* ---------- inline rename helper ----------
   A textarea that shows all of its content instead of one scrolling line.
   The ceiling is CSS (`max-height` on the field), so an over-tall height set
   here is simply clamped and the field scrolls — no computed style is read,
   and a host without layout (the DOM-contract tests) is left alone. */
export function autoGrowField(field) {
  if (!field || !field.style) return;
  field.style.height = 'auto';
  if (typeof field.scrollHeight === 'number') field.style.height = field.scrollHeight + 'px';
}

/* One-shot edit lifecycle for a value shown in place. `multiline` swaps the
   input for a growing textarea: a queued prompt or a template step can be
   many lines, so Enter there TYPES a newline and ⌘↵/⌃↵ is what commits.
   Escape restores, blur commits, and an IME's Enter never submits. */
export function inlineRename(host, current, onDone, { allowEmpty = false, multiline = false } = {}) {
  const field = document.createElement(multiline ? 'textarea' : 'input');
  field.value = current;
  if (multiline) {
    field.className = 'inline-multiline';
    field.rows = 1;
    field.spellcheck = false;
  }
  host.replaceChildren(field);
  if (multiline) autoGrowField(field);
  field.focus();
  field.select();
  let done = false;
  const finish = commit => {
    if (done) return;
    done = true;
    const value = inlineRenameValue(current, field.value, commit, allowEmpty);
    /* End the editing DOM/focus state before subscribers can render. Enter
       therefore looks committed in the same gesture and its subsequent blur
       is guaranteed to be a no-op. */
    host.textContent = value === null ? current : value;
    Promise.resolve(onDone(value)).catch(() => {
      if (host.isConnected) host.textContent = current;
      toast(t('error.changeNotSaved'));
    });
  };
  field.addEventListener('keydown', e => {
    e.stopPropagation();
    if (e.key === 'Enter') {
      if (e.isComposing || e.keyCode === 229) return;
      if (multiline && !(e.metaKey || e.ctrlKey)) return;   // the newline is the content
      e.preventDefault();
      finish(true);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      finish(false);
    }
  });
  if (multiline) field.addEventListener('input', () => autoGrowField(field));
  field.addEventListener('blur', () => finish(true));
  field.addEventListener('click', e => e.stopPropagation());
  field.addEventListener('dblclick', e => e.stopPropagation());
}


export function promptDialog(msg, initial = '') {
  return new Promise(res => {
    $('ppd-msg').textContent = msg;
    const inp = $('ppd-input');
    inp.value = initial;
    $('ppd').style.display = 'flex';
    inp.focus();
    inp.select();
    const done = v => { $('ppd').style.display = 'none'; inp.onkeydown = null; res(v); };
    $('ppd-yes').onclick = () => done(inp.value.trim() || null);
    $('ppd-no').onclick = () => done(null);
    inp.onkeydown = e => {
      if (e.key === 'Enter') {
        if (e.isComposing || e.keyCode === 229) return;
        done(inp.value.trim() || null);
      }
      if (e.key === 'Escape') done(null);
    };
  });
}

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initDialogs() {
  $('cfm-yes').onclick = () => cfmDone(true);
  $('cfm-no').onclick = () => cfmDone(false);

  $('cfm').addEventListener('mousedown', e => { if (e.target === $('cfm')) cfmDone(false); });
  $('chd').addEventListener('mousedown', e => { if (e.target === $('chd') && chdResolve) chdResolve(null); });
  $('pdf').addEventListener('mousedown', e => { if (e.target === $('pdf') && pdfResolve) pdfResolve(null); });

  document.addEventListener('keydown', e => {
    if ($('cfm').style.display !== 'flex') return;
    if (e.key === 'Enter') {
      e.stopPropagation(); e.preventDefault();
      if (!confirmPointerOnly) cfmDone(true);
    }
    if (e.key === 'Escape') { e.stopPropagation(); e.preventDefault(); cfmDone(false); }
  }, true);
}
