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
// inside that editor types a newline; ⌘↵ commits, Escape restores, blur
// commits. The name bound is the one `settings-model` already enforces on an
// inbound rule, and both lists are bounded. Inbound rules name a template by
// NAME: renaming or deleting one that a rule uses is confirmed with the count
// of affected rules — deck warns, and never rewrites the user's rules for them.
import { $, ctx, state } from './state.js';
import { provider } from './board.js';
import { autoGrowField, confirmDialog, toast } from './dialogs.js';
import {
  TEMPLATES_MAX, TEMPLATE_NAME_MAX, TEMPLATE_STEP_MAX, TEMPLATE_STEPS_MAX,
  inboundRulesUsingTemplate, moveTemplateStep, nextTemplateName, normalizeTemplateStep,
  promptSummary, templateNameProblem,
} from './pure.js';
import { formatNumber, onLocaleChange, t } from './i18n.js';

let projectId = null;
let selected = null;      // name of the template being edited
let nameShown = null;     // whose name the editor field currently holds
let openStep = null;      // index of the one step opened into the full editor
let unsubscribe = null;
let opener = null;        // element (or resolver) that opened the manager; focus returns there

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
  projectId = state.projectId;
  const list = templates();
  selected = list.length ? list[0].name : null;
  nameShown = null;
  openStep = null;
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
  openStep = null;
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
  try {
    await provider.saveTemplate(projectId, name, steps);
    return true;
  } catch (_) {
    toast(t('error.templateSave'));
    renderTemplates();
    return false;
  }
}

/* The collapsed row: the step's first line, plus how many more there are.
   Clicking it is what opens the full editor — the same gesture that edits a
   single-line step today. */
function collapsedStep(step, index) {
  const body = document.createElement('div');
  body.className = 'body';
  const { first, extra } = promptSummary(step);

  const one = document.createElement('div');
  one.className = 'one';
  one.textContent = first;
  one.title = t('common.edit');
  one.onclick = () => {
    openStep = index;
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
function openStepEditor(tpl, step, index) {
  const body = document.createElement('div');
  body.className = 'body editing';

  const field = document.createElement('textarea');
  field.className = 'inline-multiline';
  field.rows = 1;
  field.spellcheck = false;
  field.value = step;

  const hint = document.createElement('div');
  hint.className = 'row-hint';
  const keys = document.createElement('span');
  keys.textContent = t('templates.editKeys');
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
    done = true;
    openStep = null;
    const text = commit ? normalizeTemplateStep(field.value) : '';
    if (!text || text === step) { renderTemplates(); return; }
    const next = tpl.steps.slice();
    next[index] = text;
    await persist(tpl.name, next);
    renderTemplates();
  };
  field.addEventListener('input', () => { autoGrowField(field); showSize(); });
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
  field.addEventListener('blur', () => finish(true));

  body.append(field, hint);
  /* size and focus once the row is in the document, so scrollHeight is real */
  queueMicrotask(() => {
    if (!field.isConnected) return;
    autoGrowField(field);
    field.focus();
    field.setSelectionRange(field.value.length, field.value.length);
  });
  return body;
}

function stepRow(tpl, step, index) {
  const editing = openStep === index;
  const row = document.createElement('div');
  row.className = 'tpl-step' + (editing ? ' open' : '');

  const idx = document.createElement('span');
  idx.className = 'idx';
  idx.textContent = formatNumber(index + 1);

  const body = editing
    ? openStepEditor(tpl, step, index)
    : collapsedStep(step, index);

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
    const next = moveTemplateStep(tpl.steps, index, delta);
    if (next === tpl.steps) return;
    openStep = null;
    await persist(tpl.name, next);
    renderTemplates();
  };
  const remove = act('✕', t('templates.removeStep'), false, async () => {
    openStep = null;
    await persist(tpl.name, tpl.steps.filter((_, i) => i !== index));
    renderTemplates();
  });
  remove.classList.add('del');

  row.append(
    idx, body,
    act('↑', t('templates.moveUp'), index === 0, () => move(-1)),
    act('↓', t('templates.moveDown'), index === tpl.steps.length - 1, () => move(1)),
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
    openStep = null;
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
    row.onclick = () => {
      selected = tp.name;
      openStep = null;
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
  const steps = $('tpl-steps');
  steps.replaceChildren();
  if (!tpl.steps.length) {
    const blank = document.createElement('p');
    blank.className = 'set-hint tpl-wide';
    blank.textContent = t('templates.noSteps');
    steps.appendChild(blank);
  }
  tpl.steps.forEach((step, index) => steps.appendChild(stepRow(tpl, step, index)));
  const full = tpl.steps.length >= TEMPLATE_STEPS_MAX;
  $('tpl-step-text').disabled = full;
  $('tpl-step-add').disabled = full;
}

async function addStep() {
  const tpl = current();
  if (!tpl) return;
  const text = normalizeTemplateStep($('tpl-step-text').value);
  if (!text) { $('tpl-step-text').focus(); return; }
  if (tpl.steps.length >= TEMPLATE_STEPS_MAX) {
    toast(t('templates.maxSteps', { max: formatNumber(TEMPLATE_STEPS_MAX) }));
    return;
  }
  openStep = null;
  if (await persist(tpl.name, [...tpl.steps, text])) {
    $('tpl-step-text').value = '';
    autoGrowField($('tpl-step-text'));
  }
  renderTemplates();
  $('tpl-step-text').focus();
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
export function initTemplates() {
  $('tpl-done').onclick = () => closeTemplates();

  $('tpl-modal').addEventListener('mousedown', event => {
    if (event.target === $('tpl-modal')) closeTemplates();
  });

  $('tpl-search').addEventListener('input', () => renderTemplates());

  $('tpl-name').addEventListener('change', () => commitName());

  $('tpl-step-add').onclick = () => addStep();

  $('tpl-step-text').addEventListener('input', () => autoGrowField($('tpl-step-text')));

  $('tpl-step-text').addEventListener('keydown', event => {
    if (event.key !== 'Enter') return;
    if (event.isComposing || event.keyCode === 229) return;   // IME commit, not submit
    /* a step can be many lines, so Enter types one; ⌘↵ is what adds */
    if (!(event.metaKey || event.ctrlKey)) return;
    event.preventDefault();
    addStep();
  });

  $('tpl-new').onclick = async () => {
    const all = templates();
    if (all.length >= TEMPLATES_MAX) {
      toast(t('templates.maxTemplates', { max: formatNumber(TEMPLATES_MAX) }));
      return;
    }
    const name = nextTemplateName(t('templates.newName'), all);
    if (!(await persist(name, []))) return;
    selected = name;
    openStep = null;
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
