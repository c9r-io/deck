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
// file, so the card queue (☆ / ✎) and the Board-level manager (`templates.js`,
// the `◈ Templates` button in the board head) edit the same object through the
// ordinary Board transaction — one mutation per user action, a failed write
// leaves the template as it is on disk. The manager exists so a template can
// be created and edited without owning a card; it never starts a session,
// queues a prompt or moves a card. A step is ONE queued prompt flattened to a
// single line (`normalizeTemplateStep`, same reason as inbound: the queue
// pastes a literal buffer and a raw newline submits early), the name bound is
// the one `settings-model` already enforces on an inbound rule, and both lists
// are bounded. Inbound rules name a template by NAME: renaming or deleting one
// that a rule uses is confirmed with the count of affected rules — deck warns,
// and never rewrites the user's rules for them.
import { $, ctx, state } from './state.js';
import { provider } from './board.js';
import { confirmDialog, toast } from './dialogs.js';
import {
  TEMPLATES_MAX, TEMPLATE_NAME_MAX, TEMPLATE_STEPS_MAX,
  inboundRulesUsingTemplate, moveTemplateStep, nextTemplateName, normalizeTemplateStep,
  templateNameProblem,
} from './pure.js';
import { formatNumber, onLocaleChange, t } from './i18n.js';

let projectId = null;
let selected = null;      // name of the template being edited
let nameShown = null;     // whose name the editor field currently holds
let unsubscribe = null;

const isOpen = () => $('tpl-modal').style.display === 'flex';
const project = () => (projectId ? provider.project(projectId) : null);
const templates = () => {
  const p = project();
  return (p && p.templates) || [];
};
const current = () => templates().find(tp => tp.name === selected) || null;
const inboundRules = () => (ctx.settings && ctx.settings.inbound && ctx.settings.inbound.rules) || [];

export function openTemplates() {
  projectId = state.projectId;
  const list = templates();
  selected = list.length ? list[0].name : null;
  nameShown = null;
  $('tpl-search').value = '';
  $('tpl-modal').style.display = 'flex';
  /* while open, follow the board: another transaction (or a project delete)
     must not leave a stale editor on screen */
  if (!unsubscribe) {
    unsubscribe = provider.subscribe(ev => {
      if (ev !== 'projects') return;
      const active = document.activeElement;
      if (active && active.tagName === 'INPUT' && active.closest('#tpl-box')) return;
      renderTemplates();
    });
  }
  renderTemplates();
  (selected ? $('tpl-name') : $('tpl-new')).focus();
}

export function closeTemplates() {
  $('tpl-modal').style.display = 'none';
  if (unsubscribe) { unsubscribe(); unsubscribe = null; }
  $('board-tpl').focus();
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

function stepRow(tpl, step, index) {
  const row = document.createElement('div');
  row.className = 'tpl-step';

  const idx = document.createElement('span');
  idx.className = 'idx';
  idx.textContent = formatNumber(index + 1);

  const input = document.createElement('input');
  input.type = 'text';
  input.value = step;
  input.spellcheck = false;
  input.addEventListener('change', async () => {
    const text = normalizeTemplateStep(input.value);
    if (!text || text === step) { input.value = step; return; }
    const next = tpl.steps.slice();
    next[index] = text;
    await persist(tpl.name, next);
    renderTemplates();
  });

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
  const move = async delta => {
    const next = moveTemplateStep(tpl.steps, index, delta);
    if (next === tpl.steps) return;
    await persist(tpl.name, next);
    renderTemplates();
  };
  const remove = act('✕', t('templates.removeStep'), false, async () => {
    await persist(tpl.name, tpl.steps.filter((_, i) => i !== index));
    renderTemplates();
  });
  remove.classList.add('del');

  row.append(
    idx, input,
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
  if (!all.some(tp => tp.name === selected)) selected = all.length ? all[0].name : null;
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
  if (await persist(tpl.name, [...tpl.steps, text])) $('tpl-step-text').value = '';
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
  $('board-tpl').onclick = () => openTemplates();

  $('tpl-done').onclick = () => closeTemplates();

  $('tpl-modal').addEventListener('mousedown', event => {
    if (event.target === $('tpl-modal')) closeTemplates();
  });

  $('tpl-search').addEventListener('input', () => renderTemplates());

  $('tpl-name').addEventListener('change', () => commitName());

  $('tpl-step-add').onclick = () => addStep();

  $('tpl-step-text').addEventListener('keydown', event => {
    if (event.key !== 'Enter') return;
    if (event.isComposing || event.keyCode === 229) return;   // IME commit, not submit
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
