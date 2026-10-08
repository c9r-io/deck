import { localCandidates, localParts, noteValid, REMINDER_NOTE_BYTES, shortcutTime } from './reminder-model.js';
// dialogs.js — confirm/prompt/choice dialogs, project defaults, toasts, inline rename
// The settings modal and everything it persists live in settings.js, which
// imports these primitives; nothing here knows the settings document.
// The lost Board's way out (`createBoardExit`) is the one flow here: the
// app hands it the Board queue and store, so it stays testable without one.
// Part of deck's no-build frontend: native ES modules, no bundler.
import { $, ctx, genId, inv } from './state.js';
import { inlineRenameValue, isComposingKeyEvent } from './pure.js';
import { formatDateTime, t } from './i18n.js';
import { normalizeTaskPreset, normalizeTaskPresets, presetApproved, withPresetApproval } from './connector-model.js';
import { firstSendNeedsConfirm, firstSendSupported } from './automation-model.js';

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

/* The lost Board's one way out, as the backend's closed answer names it
   (`board_recovery_state`): restore the kept copy — when it was written and
   how many cards it holds — or, only when nothing can be restored, start a
   new Board with its consequences spelled out. Neither choice is primary, so
   Cancel has the focus and a stray Enter takes nothing. Resolves 'restore',
   'new' or null. */
export function boardExitDialog(recovery) {
  const kept = recovery && recovery.kept;
  if (!kept) return choiceDialog(t('board.lostNew'), [{ id: 'new', label: t('board.startNew') }]);
  const date = formatDateTime(kept.savedAt, { dateStyle: 'medium', timeStyle: 'short' });
  return choiceDialog(t('board.lostRestorable', { date, n: kept.cards }),
    [{ id: 'restore', label: t('board.restoreKept') }]);
}

/* The lost Board's way out, end to end (documents.rs `lost_exit_at`):
   nothing could be loaded and the backend refuses every save. The returned
   `offer(known)` takes the backend's answer (`known`, the one boot already
   has, or a fresh `board_recovery_state`) and shows the dialog only for a
   lost Board — at most twice per run (boot, then the first save refused
   after a Cancel), and never while one is open. A choice runs the
   exit inside one queued Board transaction, so no other Board write lands
   between the backend committing the chosen Board and the webview holding
   it (`hold(json)`: the restored Board, or an empty one for a new start);
   `exited()` then lets the app carry on, and the chosen Board is saved like
   any change. A refused exit changes nothing; a failed save leaves the
   webview holding what the backend committed, and the next change saves
   it. The app owns the Board, so its queue and store arrive as `deps`. */
export function createBoardExit({ mutateBoard, hold, exited }) {
  let offers = 0, open = false;
  return async function offer(known) {
    if (open || offers >= 2) return;
    open = true;
    try {
      const recovery = known || await inv('board_recovery_state').catch(() => null);
      if (!recovery || recovery.state !== 'lost') return;
      offers += 1;
      const action = await boardExitDialog(recovery);
      if (!action) return;
      try {
        await mutateBoard(async () => {
          hold(await inv('board_lost_exit', { action }));
          return { noop: true };
        });
      } catch (e) {
        toast(t('error.boardLoad'));
        return;
      }
      exited();
      await mutateBoard(() => {}).catch(() => toast(t('error.firstBoardSave')));
    } finally {
      open = false;
    }
  };
}

/* ---------- project defaults dialog (04 A v01) ----------
   The project's default directory and launch command, edited together.
   Resolves { dir, cmd } (trimmed; blank = no default) on Save / Enter, null
   on Cancel / Escape / a click outside. `recent` are command chips that only
   FILL the command field — nothing in this dialog runs anything. */
/* how long a save waits for a preset approval's digest before saying it
   could not be checked (the hash is local; this bounds a broken one) */
const PRESET_CHECK_MS = 5000;
let pdfResolve = null;
export function projectDefaultsDialog({ projectId = '', name, dir = '', cmd = '', recent = [], presets = [], columns = [] }) {
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
    let draftPresets = normalizeTaskPresets(presets, columns);
    /* ONE edit of one preset, from opening it to Done, Delete, another
       preset or the dialog closing. Three facts stay apart in it: `stored`
       (the approval the preset came with, carried on unchanged only while
       it still matches what is saved), `withdrawn` (a covered field was
       edited, so the stored approval is not carried) and `decision` (the
       user's own tick or untick, bound to the covered text it was made
       for). The box is how they are shown, never what a save reads. */
    let edit = null; let closed = false;
    const editor = $('pdf-preset-editor');
    const renderPresets = () => {
      const list = $('pdf-presets'); list.replaceChildren();
      for (const preset of draftPresets) {
        const button = document.createElement('button'); button.type = 'button'; button.className = 'btn preset-item';
        const name = document.createElement('span'); name.className = 'preset-name'; name.textContent = preset.name;
        /* what the preset amounts to, not its switches: a stored approval
           counts only for this version of it, and a direct first step only
           on a command it can reach. Until the digest is compared the line
           says it is being checked, never on and never off. */
        const summary = document.createElement('span'); summary.className = 'preset-summary';
        const first = t(preset.firstSend !== true ? 'presets.first.wait'
          : firstSendSupported(preset.cmd) ? 'presets.first.direct' : 'presets.first.unsupported');
        const say = later => { summary.textContent = `${t(later)} · ${first}`; };
        if (!preset.autoSend) say('presets.later.manual');
        else {
          say('presets.later.checking');
          presetApproved(projectId, preset).then(valid => say(valid ? 'presets.later.auto' : 'presets.later.stale'),
            () => say('presets.later.unknown'));
        }
        button.append(name, summary);
        button.onclick = () => openPreset(preset); list.appendChild(button);
      }
      $('pdf-preset-add').disabled = draftPresets.length >= 50;
    };
    /* ticked on a command the override cannot reach: say so, as a rule's
       facts do; the choice is kept and applies once the command fits */
    const syncFirstSend = () => {
      $('pdf-preset-first-send-unsupported').hidden = !$('pdf-preset-first-send').checked
        || firstSendSupported($('pdf-preset-cmd').value.trim());
    };
    /* the first-send risk is accepted explicitly, as on a rule: the box
       stays unticked unless the confirmation is answered yes */
    $('pdf-preset-first-send').onchange = async () => {
      const box = $('pdf-preset-first-send');
      if (firstSendNeedsConfirm(false, box.checked)) {
        box.checked = false;
        $('cfm').classList.add('cfm-over-dialog');
        try { if (await confirmDialog(t('presets.firstSend.confirm'))) box.checked = true; }
        finally { $('cfm').classList.remove('cfm-over-dialog'); }
      }
      syncFirstSend();
    };
    const covered = () => JSON.stringify([$('pdf-preset-dir').value, $('pdf-preset-cmd').value, $('pdf-preset-steps').value]);
    const approvalNote = key => { const note = $('pdf-preset-auto-send-state'); note.hidden = !key; note.textContent = key ? t(key) : ''; };
    /* a digest is hashed asynchronously: a wait for one is bounded, and a
       failure or a timeout is `undefined`, never yes and never no */
    const settled = promise => new Promise(resolve => {
      const timer = setTimeout(() => resolve(undefined), PRESET_CHECK_MS);
      promise.then(value => { clearTimeout(timer); resolve(value); }, () => { clearTimeout(timer); resolve(undefined); });
    });
    const openPreset = preset => {
      const opened = edit = { id: preset?.id || genId('R'), stored: preset?.autoSend?.digest || '', withdrawn: false, decision: null, commit: null };
      $('pdf-preset-name').value = preset?.name || '';
      $('pdf-preset-title').value = preset?.title || '';
      $('pdf-preset-dir').value = preset?.dir || dirInput.value.trim();
      $('pdf-preset-cmd').value = preset?.cmd || cmdInput.value.trim() || 'codex';
      $('pdf-preset-steps').value = (preset?.steps || []).join('\n');
      $('pdf-preset-first-send').checked = preset?.firstSend === true; syncFirstSend();
      /* ticked only for an approval of this exact version of the preset.
         Until that is known the box is neither on nor off, and the answer
         belongs to THIS edit of THIS text: not to a later edit of the same
         preset, and not once the user edited a covered field or chose. */
      const auto = $('pdf-preset-auto-send'); const text = covered();
      auto.checked = false; auto.indeterminate = !!opened.stored;
      approvalNote(opened.stored ? 'automation.autoSend.checking' : null);
      if (opened.stored) settled(presetApproved(projectId, preset)).then(valid => {
        if (edit !== opened || opened.withdrawn || opened.decision) return;
        if (covered() !== text) { auto.indeterminate = false; approvalNote(null); opened.withdrawn = true; return; }
        if (valid === undefined) { approvalNote('automation.autoSend.checkFailed'); return; }
        auto.indeterminate = false; auto.checked = valid;
        approvalNote(valid ? null : 'presets.autoSend.stale');
      });
      const target = $('pdf-preset-column'); target.replaceChildren();
      for (const column of columns) { const option = document.createElement('option'); option.value = column.id; option.textContent = column.name; target.appendChild(option); }
      target.value = preset?.columnId || columns[0]?.id || '';
      $('pdf-preset-delete').hidden = !preset;
      editor.hidden = false; $('pdf-preset-name').focus();
    };
    /* the approval covers the directory, the command and every step: an
       edit to one of them withdraws it, as in the rule editor, whether it
       was ticked, chosen a moment ago or still being checked, and the user
       approves the edited version explicitly (or saves without it) */
    const withdrawApproval = () => {
      if (!edit) return;
      const box = $('pdf-preset-auto-send'); const had = box.checked || box.indeterminate === true;
      edit.withdrawn = true; edit.decision = null;
      box.checked = false; box.indeterminate = false; approvalNote(null);
      if (had) toast(t('automation.autoSend.withdrawn'));
    };
    $('pdf-preset-dir').oninput = withdrawApproval;
    $('pdf-preset-steps').oninput = withdrawApproval;
    $('pdf-preset-cmd').oninput = () => { withdrawApproval(); syncFirstSend(); };
    /* the user's own answer, for the covered text as it stands now: it
       ends any check still under way */
    $('pdf-preset-auto-send').onchange = () => {
      if (!edit) return;
      const box = $('pdf-preset-auto-send'); box.indeterminate = false; approvalNote(null);
      edit.decision = { approve: box.checked === true, covered: covered() };
    };
    /* What `plain` is saved with: a NEW approval only from the user's tick
       for exactly this text; the STORED one carried on, digest untouched,
       only if it is the approval of exactly this text; null for none.
       `undefined` = could not be told, so nothing may be saved yet. */
    const approvalFor = async (current, plain) => {
      if (current.decision) {
        if (!current.decision.approve) return null;
        if (current.decision.covered !== covered()) { withdrawApproval(); return undefined; }
        /* a new approval that could not be computed is not a save: the
           tick and the draft stay, the edit that asked says so, and the
           same save again retries it */
        const minted = (await settled(withPresetApproval(projectId, plain, true)))?.autoSend;
        if (!minted && edit === current) { approvalNote('presets.autoSend.approveFailedSave'); toast(t('presets.autoSend.approveFailedSave')); }
        return minted || undefined;
      }
      if (current.withdrawn || !current.stored) return null;
      const stored = { digest: current.stored };
      const valid = await settled(presetApproved(projectId, { ...plain, autoSend: stored }));
      if (valid === undefined) { if (edit === current) toast(t('presets.autoSend.checkFailedSave')); return undefined; }
      return valid ? stored : null;
    };
    /* A save request belongs to the edit that was open when the user asked
       for it, decided at the click and never when its turn comes: Done
       followed at once by Save is ONE commit of that edit (the second
       request joins the first), and a request whose edit was since
       replaced, reopened, deleted or closed ends as not saved instead of
       taking the edit that is open by then. A commit that did not save is
       forgotten, so the same edit can be saved again.
       A commit owns nothing while it waits: the edit it started from must
       still be the open one, with the same fields and the same answer. */
    const commitPreset = () => {
      const current = edit;
      if (!current) return Promise.resolve(true);
      const forget = () => { current.commit = null; };
      return current.commit ||= commitEdit(current).then(saved => { if (!saved) forget(); return saved; },
        error => { forget(); throw error; });
    };
    const commitEdit = async current => {
      for (;;) {
        if (edit !== current) return false;
        const fields = () => ({ id: current.id, name: $('pdf-preset-name').value,
          columnId: $('pdf-preset-column').value, title: $('pdf-preset-title').value,
          dir: $('pdf-preset-dir').value, cmd: $('pdf-preset-cmd').value,
          steps: $('pdf-preset-steps').value.split('\n'),
          firstSend: $('pdf-preset-first-send').checked });
        const before = JSON.stringify([fields(), current.decision, current.withdrawn]);
        const plain = normalizeTaskPreset(fields(), columns);
        if (!plain) { toast(t('presets.invalid')); return false; }
        const approval = await approvalFor(current, plain);
        if (edit !== current) return false;
        if (approval === undefined) return false;
        if (JSON.stringify([fields(), current.decision, current.withdrawn]) !== before) continue;
        draftPresets = [...draftPresets.filter(value => value.id !== current.id), approval ? { ...plain, autoSend: approval } : plain];
        edit = null; editor.hidden = true; renderPresets(); return true;
      }
    };
    $('pdf-preset-add').onclick = () => openPreset(null);
    $('pdf-preset-done').onclick = commitPreset;
    $('pdf-preset-delete').onclick = () => {
      if (!edit) return;
      const id = edit.id; edit = null;
      draftPresets = draftPresets.filter(value => value.id !== id);
      editor.hidden = true; renderPresets();
    };
    renderPresets(); editor.hidden = true;
    const read = async () => await commitPreset() ? ({ dir: dirInput.value.trim(), cmd: cmdInput.value.trim(),
      ...(draftPresets.length || presets.length ? { presets: draftPresets } : {}) }) : null;
    /* closing ends the edit, once: a save or a check that finishes later
       belongs to nothing, and cannot close a dialog opened since */
    const done = v => {
      if (closed) return;
      closed = true; edit = null;
      $('pdf').style.display = 'none'; $('pdf').onkeydown = null; pdfResolve = null; resolve(v);
    };
    pdfResolve = done;
    $('pdf-yes').onclick = async () => { const value = await read(); if (value) done(value); };
    $('pdf-no').onclick = () => done(null);
    $('pdf').onkeydown = e => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); done(null); return; }
      if (e.key === 'Enter' && e.target && e.target.tagName === 'INPUT') {
        if (isComposingKeyEvent(e)) return;
        e.preventDefault(); e.stopPropagation();
        read().then(value => { if (value) done(value); });
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

/* Reminder input stays local: UTC candidates must round-trip exactly in the
   displayed zone. Fold choices are explicit; neither Date nor native guesses. */
export async function reminderDialog(current, registration = null) {
  const zone = current?.timeZone || Intl.DateTimeFormat().resolvedOptions().timeZone;
  const status = await inv('notify_status').catch(() => 'unsupported');
  return new Promise(resolve => {
    const overlay = document.createElement('div'); overlay.className = 'reminder-overlay';
    const form = document.createElement('form'); form.className = 'reminder-editor';
    const title = document.createElement('h2'); title.textContent = t(current && (current.due || current.dueAt <= Date.now()) ? 'reminder.later' : 'reminder.set'); form.append(title);
    const zoneLabel = document.createElement('p'); zoneLabel.textContent = zone; form.append(zoneLabel);
    const shortcuts = document.createElement('div');
    const date = document.createElement('input'); date.type = 'datetime-local'; date.id = 'reminder-date'; date.required = true;
    date.value = localParts(current?.dueAt || Date.now() + 3600000, zone);
    for (const kind of ['hour', 'tomorrow', 'monday']) {
      const button = document.createElement('button'); button.type = 'button'; button.className = 'btn'; button.textContent = t({ hour: 'reminder.hour', tomorrow: 'reminder.tomorrow', monday: 'reminder.monday' }[kind]);
      button.onclick = () => { const now = Date.now(); date.value = shortcutTime(kind, now, zone); update(); if (kind === 'hour') { relativeInstant = now + 3600000; fold.hidden = true; validate(); } }; shortcuts.append(button);
    }
    form.append(shortcuts, date);
    const fold = document.createElement('select'); fold.id = 'reminder-fold'; form.append(fold);
    const note = document.createElement('input'); note.type = 'text'; note.id = 'reminder-note'; note.value = current?.note || ''; note.placeholder = t('reminder.note');
    note.setAttribute('aria-label', t('reminder.note')); note.setAttribute('aria-describedby', 'reminder-note-feedback');
    /* the limit is UTF-8 bytes (reminder.rs), so the feedback counts bytes:
       nothing is truncated and nothing is saved over the limit */
    const noteFeedback = document.createElement('p'); noteFeedback.id = 'reminder-note-feedback'; noteFeedback.setAttribute('role', 'status'); noteFeedback.hidden = true;
    form.append(note, noteFeedback);
    const label = document.createElement('label');
    const inApp = document.createElement('input'); inApp.type = 'checkbox'; inApp.id = 'reminder-in-app'; inApp.checked = current?.inAppOnly === true;
    label.append(inApp, document.createTextNode(t('reminder.inApp'))); form.append(label);
    const preview = document.createElement('p'); preview.id = 'reminder-preview'; form.append(preview);
    const permissions = document.createElement('p'); permissions.textContent = t(`settings.notifyStatus.${status}`) + (current ? " · " + t({ scheduled: "reminder.registered", "registration-failed": "reminder.registrationFailed", saved: "reminder.saved" }[registration] || "reminder.saved") : ""); form.append(permissions);
    const saved = document.createElement('p'); saved.textContent = current ? t('reminder.savedHint') : t('reminder.intentHint'); form.append(saved);
    const actions = document.createElement('div'); actions.className = 'cfm-actions';
    const done = value => { overlay.remove(); resolve(value); };
    const cancel = document.createElement('button'); cancel.type = 'button'; cancel.className = 'btn'; cancel.textContent = t('common.cancel'); cancel.onclick = () => done(null); actions.append(cancel);
    if (current) {
      const end = document.createElement('button'); end.type = 'button'; end.className = 'btn'; end.id = 'reminder-end'; end.textContent = t('reminder.end'); end.onclick = () => done({ cancel: true }); actions.append(end);
    }
    const save = document.createElement('button'); save.type = 'submit'; save.className = 'btn primary'; save.id = 'reminder-save'; save.textContent = t('common.save'); actions.append(save); form.append(actions);
    let candidates = [];
    let relativeInstant = null;
    const update = () => {
      relativeInstant = null;
      candidates = localCandidates(date.value, zone);
      fold.replaceChildren(); fold.hidden = candidates.length < 2;
      if (candidates.length > 1) {
        const empty = document.createElement('option'); empty.value = ''; empty.textContent = t('reminder.ambiguous'); fold.append(empty);
        for (const instant of candidates) { const option = document.createElement('option'); option.value = String(instant); option.textContent = new Date(instant).toISOString(); fold.append(option); }
      }
      validate();
    };
    const validate = () => {
      const instant = relativeInstant || (candidates.length === 1 ? candidates[0] : Number(fold.value) || null);
      const timeValid = instant > Date.now();
      const valid = timeValid && noteValid(note.value);
      save.disabled = !valid || (!inApp.checked && !['not-determined', 'authorized', 'provisional'].includes(status));
      preview.textContent = timeValid ? `${t('reminder.preview')} ${new Intl.DateTimeFormat(undefined, { timeZone: zone, dateStyle: 'full', timeStyle: 'short' }).format(instant)} (${zone})` : t('reminder.invalid');
      const used = new TextEncoder().encode(note.value).length; const over = used > REMINDER_NOTE_BYTES;
      const line = !over && !noteValid(note.value);
      noteFeedback.hidden = !over && !line && used < REMINDER_NOTE_BYTES * 0.75;
      noteFeedback.className = over || line ? 'reminder-note-over' : '';
      noteFeedback.textContent = line ? t('reminder.noteLine') : t(over ? 'reminder.noteOver' : 'reminder.noteUsed', { used, max: REMINDER_NOTE_BYTES });
    };
    date.oninput = update; fold.onchange = validate; note.oninput = validate; inApp.onchange = validate;
    form.onsubmit = event => {
      event.preventDefault(); validate(); if (save.disabled) return;
      if (!inApp.checked && status === 'not-determined') inv('reminder_request_permission').catch(() => {});
      done({ dueAt: relativeInstant || (candidates.length === 1 ? candidates[0] : Number(fold.value)), timeZone: zone, note: note.value, inAppOnly: inApp.checked });
    };
    overlay.onkeydown = event => { if (event.key === 'Escape') { event.stopPropagation(); done(null); } };
    overlay.append(form); document.body.append(overlay); update(); date.focus();
  });
}
