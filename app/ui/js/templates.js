// templates.js — the Board-level manager for a project's prompt templates.
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// A template lives on the project object (persisted inside the board file)
// and is the SAME object the card queue inserts with ✎/☆. This manager only
// removes the requirement to own a card before a template can exist: every
// change goes through the ordinary Board transaction, one mutation at a
// time, and a rejected write leaves the template exactly as it was on disk.
// Nothing here starts a session, queues a prompt or moves a card.
//
// # Contract
// Prompt templates (`{name, steps[]}`) live on the PROJECT inside the board
// file, so the card queue (☆ / ✎) and the project-level manager (`templates.js`,
// reached from "Templates…" in the Board's New session ▾ menu, a list's 📋
// menu and the automation editor — never a standing Board button) edit the same object through the
// ordinary Board transaction — one mutation per user action, a failed write
// leaves the template as it is on disk. The manager exists so a template can
// be created and edited without owning a card; it never starts a session,
// queues a prompt or moves a card. A step is ONE queued prompt flattened to a
// MULTI-LINE prompt (`normalizeTemplateStep`: the queue pastes inside
// bracketed-paste marks and presses Enter separately, so only a CR would
// submit early). The step list stays one row per step — the row shows the
// first line plus a `⏎N` badge, and exactly ONE row at a time opens in place
// into the full editor, so a 20-step template is still scannable. Enter
// inside that editor types a newline; ⌘↵ commits and Escape restores. A
// click away saves nothing: the editor stays open with what was typed,
// through any re-render (`openDraft`), and an action that would take an
// unsaved edit away (another row, a move, closing the manager) asks first
// (`leaveOpenEdit`). A template's follow-ups (`followUps`, at most FOLLOWUPS_MAX) are
// edited the same way, in their own list under the steps: fixed replies a
// run freezes onto the card it opens (follow-up-model.js), refused when they
// hold a `{{msg.*}}` placeholder, since nothing ever fills one. A follow-up
// edit takes no approval away: an approval covers the steps alone.
// The name bound is the one `settings-model` already enforces on an
// inbound rule, and both lists are bounded. Inbound rules name a template by
// NAME: renaming or deleting one that a rule uses is confirmed with the count
// of affected rules — deck warns, and never rewrites the user's rules for them.
// The Board `provider` arrives through `initTemplates({ provider })`, so the
// modules that open the manager (scheduler, terminal, automation) can import
// this one without a cycle.
import { $, ctx, state } from './state.js';
import { autoGrowField, confirmDialog, toast } from './dialogs.js';
import {
  FOLLOWUPS_MAX, TEMPLATES_MAX, TEMPLATE_NAME_MAX, TEMPLATE_STEP_MAX, TEMPLATE_STEPS_MAX,
  followUpProblem, inboundRulesUsingTemplate, moveTemplateStep, nextTemplateName, normalizeTemplateStep,
  promptSummary, templateNameProblem,
} from './pure.js';
import { formatNumber, onLocaleChange, t } from './i18n.js';
import { approvalsVoidedBy, ruleLabel } from './automation-model.js';

let provider = null;      // the Board, from initTemplates
let projectId = null;
let selected = null;      // name of the template being edited
let nameShown = null;     // whose name the editor field currently holds
let openItem = null;      // {list, index}: the one row opened into the full editor
let openDraft = null;     // {key, text}: what that editor holds that is not saved
let focusOpened = false;  // the next editor built was just opened: it takes the focus
let unsubscribe = null;
let opener = null;        // element (or resolver) that opened the manager; focus returns there
// Rules already told, while this manager is open, that a step edit took
// their automatic-continue approval away: one notice per rule, not per save.
const voidedTold = new Set();

const isOpen = () => $('tpl-modal').style.display === 'flex';
const project = () => (projectId ? provider.project(projectId) : null);
const templates = () => {
  const p = project();
  return (p && p.templates) || [];
};
const current = () => templates().find(tp => tp.name === selected) || null;
const inboundRules = () => (ctx.settings && ctx.settings.inbound && ctx.settings.inbound.rules) || [];

export function openTemplates(from = null) {
  opener = from;
  voidedTold.clear();
  projectId = state.projectId;
  const list = templates();
  selected = list.length ? list[0].name : null;
  nameShown = null;
  openItem = null;
  openDraft = null;
  $('tpl-search').value = '';
  $('tpl-modal').style.display = 'flex';
  /* while open, follow the board: another transaction (or a project delete)
     must not leave a stale editor on screen */
  if (!unsubscribe) {
    unsubscribe = provider.subscribe(ev => {
      if (ev !== 'projects') return;
      const active = document.activeElement;
      if (active && ['INPUT', 'TEXTAREA'].includes(active.tagName)
          && active.closest('#tpl-box')) return;
      renderTemplates();
    });
  }
  renderTemplates();
  (selected ? $('tpl-name') : $('tpl-new')).focus();
}

export function closeTemplates() {
  if (openItem && openDraft) {
    leaveOpenEdit().then(ok => { if (ok) closeTemplates(); });
    return;
  }
  openItem = null;
  openDraft = null;
  $('tpl-modal').style.display = 'none';
  if (unsubscribe) { unsubscribe(); unsubscribe = null; }
  const target = typeof opener === 'function' ? opener() : opener;
  const back = target && target.isConnected ? target : $('board-new-more');
  opener = null;
  back.focus();
}

/* One transaction per user action. A failure is reported and the editor is
   rebuilt from the committed board, so the screen never shows a change the
   user was told failed. */
async function persist(name, steps) {
  const before = templates().find(tp => tp.name === name)?.steps || [];
  try {
    await provider.saveTemplate(projectId, name, steps);
  } catch (_) {
    toast(t('error.templateSave'));
    renderTemplates();
    return false;
  }
  // after the save, and outside its failure handling: a notice that cannot
  // be made must never read as a save that failed
  await tellVoidedApprovals(name, before, steps);
  return true;
}

/* Whether an action may take the open editor away: yes when it holds
   nothing unsaved, or once the user agreed to discard that edit. A click
   away never saves it; this is what asks before it would be lost. */
async function leaveOpenEdit() {
  if (!openItem || !openDraft) return true;
  if (!(await confirmDialog(t('templates.discardEdit')))) {
    $(LISTS[openItem.list].container).querySelector('textarea')?.focus();
    return false;
  }
  openDraft = null;
  return true;
}

/* The follow-ups are saved like the steps, one transaction per action;
   no approval covers them, so nothing is said about one. */
async function persistFollowUps(name, followUps) {
  try {
    await provider.saveTemplateFollowUps(projectId, name, followUps);
  } catch (_) {
    toast(t('error.templateSave'));
    renderTemplates();
    return false;
  }
  return true;
}

/* The two lists a template edits the same way: its steps (queued by a run)
   and its follow-ups (buttons on the run's card). */
const LISTS = {
  steps: {
    items: tpl => tpl.steps,
    save: (tpl, next) => persist(tpl.name, next),
    max: TEMPLATE_STEPS_MAX, full: 'templates.maxSteps', remove: 'templates.removeStep',
    container: 'tpl-steps', field: 'tpl-step-text', add: 'tpl-step-add', empty: 'templates.noSteps',
  },
  followUps: {
    items: tpl => (Array.isArray(tpl.followUps) ? tpl.followUps : []),
    save: (tpl, next) => persistFollowUps(tpl.name, next),
    max: FOLLOWUPS_MAX, full: 'templates.maxFollowUps', remove: 'templates.removeFollowUp',
    container: 'tpl-follows', field: 'tpl-follow-text', add: 'tpl-follow-add', empty: null,
    refuse: text => followUpProblem(text) === 'placeholder' && 'templates.followUpPlaceholder',
  },
};

/* An approval covers the template's steps by hash, so any step edit takes it
   away (automation-model.js `approvalsVoidedBy`). Say so when it happens and
   name the rules: until now the user met it at the next run, stopped at its
   second step. A notice only — the approval was already void the moment the
   steps changed, nothing is asked, and re-approving stays in the rule editor.
   A hashing failure says nothing rather than failing the save it follows. */
async function tellVoidedApprovals(name, before, after) {
  try {
    const rules = await approvalsVoidedBy(inboundRules(), projectId, name, before, after);
    const fresh = rules.filter(rule => !voidedTold.has(rule.id));
    if (!fresh.length) return;
    fresh.forEach(rule => voidedTold.add(rule.id));
    const labels = fresh.slice(0, 3).map(ruleLabel).join(' ');
    toast(t('templates.approvalVoided', { rules: fresh.length > 3 ? `${labels} +${formatNumber(fresh.length - 3)}` : labels }));
  } catch (_) { /* nothing is said */ }
}

/* The collapsed row: the step's first line, plus how many more there are.
   Clicking it is what opens the full editor — the same gesture that edits a
   single-line step today. */
function collapsedStep(list, step, index) {
  const body = document.createElement('div');
  body.className = 'body';
  const { first, extra } = promptSummary(step);

  const one = document.createElement('div');
  one.className = 'one';
  one.textContent = first;
  one.title = t('common.edit');
  one.onclick = async () => {
    if (!(await leaveOpenEdit())) return;
    openItem = { list, index };
    openDraft = null;
    focusOpened = true;
    renderTemplates();
  };
  body.appendChild(one);
  if (extra) {
    const more = document.createElement('span');
    more.className = 'more';
    more.textContent = '⏎' + formatNumber(extra);
    more.title = t('templates.moreLines', { count: formatNumber(extra) });
    body.appendChild(more);
  }
  return body;
}

/* The opened row: one editor, committed once. A failed write leaves the
   template as it is on disk (`persist` re-renders from the committed board),
   so the editor never shows a change the user was told failed. */
function openStepEditor(list, tpl, step, index) {
  const body = document.createElement('div');
  body.className = 'body editing';

  const field = document.createElement('textarea');
  field.className = 'inline-multiline';
  field.rows = 1;
  field.spellcheck = false;
  /* the row, not the template's name: a rename while editing keeps the
     draft, and every switch of template clears it */
  const draftKey = `${list}:${index}`;
  field.value = openDraft?.key === draftKey ? openDraft.text : step;
  const remember = () => {
    openDraft = field.value === step ? null : { key: draftKey, text: field.value };
  };

  const hint = document.createElement('div');
  hint.className = 'row-hint';
  const keys = document.createElement('span');
  keys.textContent = t('common.editKeys');
  const size = document.createElement('span');
  const showSize = () => {
    const { extra } = promptSummary(field.value);
    size.textContent = t('templates.editSize', {
      lines: formatNumber(extra + 1),
      chars: formatNumber(Array.from(field.value).length),
      max: formatNumber(TEMPLATE_STEP_MAX),
    });
  };
  showSize();
  hint.append(keys, size);

  let done = false;
  const finish = async commit => {
    if (done) return;
    const text = commit ? normalizeTemplateStep(field.value) : '';
    /* refused text stays in the editor to be put right, never thrown away */
    const refused = text && text !== step && LISTS[list].refuse?.(text);
    if (refused) { toast(t(refused)); return; }
    done = true;
    openItem = null;
    openDraft = null;
    if (!text || text === step) { renderTemplates(); return; }
    const next = LISTS[list].items(tpl).slice();
    next[index] = text;
    await LISTS[list].save(tpl, next);
    renderTemplates();
  };
  field.addEventListener('input', () => { autoGrowField(field); showSize(); remember(); });
  field.addEventListener('keydown', event => {
    event.stopPropagation();
    if (event.key === 'Enter') {
      if (event.isComposing || event.keyCode === 229) return;
      if (!(event.metaKey || event.ctrlKey)) return;   // the newline is the content
      event.preventDefault();
      finish(true);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      finish(false);
    }
  });

  body.append(field, hint);
  /* size once the row is in the document, so scrollHeight is real; only an
     editor just opened takes the focus — one rebuilt by a re-render (a
     search, a save elsewhere) leaves it where the user put it */
  const focus = focusOpened;
  focusOpened = false;
  queueMicrotask(() => {
    if (!field.isConnected) return;
    autoGrowField(field);
    if (!focus) return;
    field.focus();
    field.setSelectionRange(field.value.length, field.value.length);
  });
  return body;
}

function stepRow(list, tpl, step, index) {
  const editing = openItem?.list === list && openItem.index === index;
  const { items, save } = LISTS[list];
  const row = document.createElement('div');
  row.className = 'tpl-step' + (editing ? ' open' : '');

  const idx = document.createElement('span');
  idx.className = 'idx';
  idx.textContent = formatNumber(index + 1);

  const body = editing
    ? openStepEditor(list, tpl, step, index)
    : collapsedStep(list, step, index);

  const act = (label, title, disabled, run) => {
    const button = document.createElement('button');
    button.className = 'act';
    button.textContent = label;
    button.title = title;
    button.setAttribute('aria-label', title);
    button.disabled = disabled;
    button.onclick = run;
    return button;
  };
  /* any change to the ORDER or LENGTH of the list invalidates the open
     index, so reordering and removing close the editor first */
  const move = async delta => {
    const next = moveTemplateStep(items(tpl), index, delta);
    if (next === items(tpl)) return;
    if (!(await leaveOpenEdit())) return;
    openItem = null;
    await save(tpl, next);
    renderTemplates();
  };
  const remove = act('✕', t(LISTS[list].remove), false, async () => {
    if (!(await leaveOpenEdit())) return;
    openItem = null;
    await save(tpl, items(tpl).filter((_, i) => i !== index));
    renderTemplates();
  });
  remove.classList.add('del');

  row.append(
    idx, body,
    act('↑', t('templates.moveUp'), index === 0, () => move(-1)),
    act('↓', t('templates.moveDown'), index === items(tpl).length - 1, () => move(1)),
    remove,
  );
  return row;
}

export function renderTemplates() {
  if (!isOpen()) return;
  const p = project();
  if (!p) { closeTemplates(); return; }
  $('tpl-title').textContent = t('templates.titleProject', { project: p.name });

  const all = templates();
  if (!all.some(tp => tp.name === selected)) {
    selected = all.length ? all[0].name : null;
    openItem = null;
    openDraft = null;
  }
  const query = $('tpl-search').value.trim().toLocaleLowerCase();
  const shown = all.filter(tp => !query || tp.name.toLocaleLowerCase().includes(query));

  const list = $('tpl-items');
  list.replaceChildren();
  for (const tp of shown) {
    const row = document.createElement('button');
    row.className = 'tpl-item';
    row.setAttribute('aria-current', tp.name === selected ? 'true' : 'false');
    const name = document.createElement('span');
    name.className = 'n';
    name.textContent = tp.name;
    const count = document.createElement('span');
    count.className = 'c';
    count.textContent = formatNumber(tp.steps.length);
    row.append(name, count);
    row.onclick = async () => {
      if (tp.name !== selected && !(await leaveOpenEdit())) return;
      selected = tp.name;
      if (openDraft === null) openItem = null;
      renderTemplates();
      $('tpl-name').focus();
    };
    list.appendChild(row);
  }
  $('tpl-no-results').hidden = !all.length || shown.length > 0;

  const tpl = current();
  $('tpl-empty').hidden = all.length > 0;
  $('tpl-form').hidden = !tpl;
  $('tpl-delete').disabled = !tpl;
  if (!tpl) return;

  /* keep an in-progress edit of THIS template's name, but always show the
     name of a template the user just switched to or created — WebKit does
     not focus a clicked button, so the field can still hold the focus */
  if (nameShown !== tpl.name || document.activeElement !== $('tpl-name')) {
    $('tpl-name').value = tpl.name;
  }
  nameShown = tpl.name;
  for (const [list, spec] of Object.entries(LISTS)) {
    const rows = $(spec.container);
    rows.replaceChildren();
    const items = spec.items(tpl);
    if (!items.length && spec.empty) {
      const blank = document.createElement('p');
      blank.className = 'set-hint tpl-wide';
      blank.textContent = t(spec.empty);
      rows.appendChild(blank);
    }
    items.forEach((text, index) => rows.appendChild(stepRow(list, tpl, text, index)));
    const full = items.length >= spec.max;
    $(spec.field).disabled = full;
    $(spec.add).disabled = full;
  }
}

async function addStep(list = 'steps') {
  const tpl = current();
  if (!tpl) return;
  const spec = LISTS[list];
  const field = $(spec.field);
  const text = normalizeTemplateStep(field.value);
  if (!text) { field.focus(); return; }
  const items = spec.items(tpl);
  if (items.length >= spec.max) {
    toast(t(spec.full, { max: formatNumber(spec.max) }));
    return;
  }
  const refused = spec.refuse?.(text);
  if (refused) { toast(t(refused)); field.focus(); return; }
  /* an append moves no row: an editor open above stays open, as typed */
  if (await spec.save(tpl, [...items, text])) {
    field.value = '';
    autoGrowField(field);
  }
  renderTemplates();
  field.focus();
}

async function commitName() {
  const tpl = current();
  if (!tpl) return;
  const next = $('tpl-name').value.trim();
  if (next === tpl.name) { $('tpl-name').value = tpl.name; return; }
  const problem = templateNameProblem(next, templates(), tpl.name);
  if (problem) {
    toast(t(problem === 'empty' ? 'templates.nameEmpty'
      : problem === 'long' ? 'templates.nameLong' : 'templates.nameDuplicate',
    { max: formatNumber(TEMPLATE_NAME_MAX) }));
    $('tpl-name').value = tpl.name;
    return;
  }
  /* an inbound rule names its template as a string: renaming breaks that
     link, so say so instead of silently disarming the rule */
  const used = inboundRulesUsingTemplate(inboundRules(), projectId, tpl.name);
  if (used && !(await confirmDialog(t('templates.inboundRename', {
    name: tpl.name, next, count: formatNumber(used),
  })))) {
    $('tpl-name').value = tpl.name;
    return;
  }
  try {
    await provider.renameTemplate(projectId, tpl.name, next);
    selected = next;
  } catch (_) {
    toast(t('error.templateSave'));
  }
  renderTemplates();
}

/* Same modal contract as Settings: Escape closes (unless a confirm/prompt
   owns the keyboard), Tab stays inside the dialog. */

/* DOM wiring, run once at boot (app.js) so the module can be imported
   without a document. */
export function initTemplates(deps) {
  ({ provider } = deps);
  $('tpl-done').onclick = () => closeTemplates();

  $('tpl-modal').addEventListener('mousedown', event => {
    if (event.target === $('tpl-modal')) closeTemplates();
  });

  $('tpl-search').addEventListener('input', () => renderTemplates());

  $('tpl-name').addEventListener('change', () => commitName());

  for (const [list, spec] of Object.entries(LISTS)) {
    $(spec.add).onclick = () => addStep(list);
    $(spec.field).addEventListener('input', () => autoGrowField($(spec.field)));
    $(spec.field).addEventListener('keydown', event => {
      if (event.key !== 'Enter') return;
      if (event.isComposing || event.keyCode === 229) return;   // IME commit, not submit
      /* a step can be many lines, so Enter types one; ⌘↵ is what adds */
      if (!(event.metaKey || event.ctrlKey)) return;
      event.preventDefault();
      addStep(list);
    });
  }

  $('tpl-new').onclick = async () => {
    const all = templates();
    if (all.length >= TEMPLATES_MAX) {
      toast(t('templates.maxTemplates', { max: formatNumber(TEMPLATES_MAX) }));
      return;
    }
    if (!(await leaveOpenEdit())) return;
    const name = nextTemplateName(t('templates.newName'), all);
    if (!(await persist(name, []))) return;
    selected = name;
    openItem = null;
    openDraft = null;
    $('tpl-search').value = '';
    renderTemplates();
    $('tpl-name').focus();
    $('tpl-name').select();
  };

  $('tpl-delete').onclick = async () => {
    const tpl = current();
    if (!tpl) return;
    const used = inboundRulesUsingTemplate(inboundRules(), projectId, tpl.name);
    const message = used
      ? t('templates.inboundDelete', { name: tpl.name, count: formatNumber(used) })
      : t('queue.deleteTemplate', { name: tpl.name });
    if (!(await confirmDialog(message))) return;
    try {
      await provider.deleteTemplate(projectId, tpl.name);
      selected = null;   // a failed delete keeps the template selected
    } catch (_) {
      toast(t('error.templateSave'));
    }
    renderTemplates();
    $('tpl-new').focus();
  };

  $('tpl-box').addEventListener('keydown', event => {
    if (['cfm', 'ppd'].some(id => $(id).style.display === 'flex')) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      if ($('tpl-search').value) {
        $('tpl-search').value = '';
        renderTemplates();
        $('tpl-search').focus();
      } else closeTemplates();
    }
    if (event.key === 'Tab') {
      const controls = [...$('tpl-box').querySelectorAll('button, input, [tabindex="0"]')]
        .filter(control => !control.disabled && control.getClientRects().length);
      const first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    }
  });

  onLocaleChange(() => renderTemplates());
}
