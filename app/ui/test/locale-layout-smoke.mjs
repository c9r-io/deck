// What every UI language LOOKS like in the real window: the `locale-layout`
// WKWebView smoke. Debug carrier only.
//
// For each of the six dictionaries the production page is switched to that
// language (display only: the settings file is never written) and shows the
// Board with three real (empty) shell cards and every Settings section, at the
// window's own size. Per language one checkpoint `ll-<lang>`:
//   a  passes when no translated element is empty, the page never scrolls
//      sideways and the Settings box stays inside the window;
//   b  counts the text elements whose words overflow their box without an
//      ellipsis (a candidate for a shorter translation, not a failure).
// Before each snapshot those elements get a red outline, so the PNGs in
// <data dir>/evidence/ show where. No agent runs and nothing is sent.
export async function runLocaleLayoutSmoke() {
  const { $, inv, state } = await import('../js/state.js');
  const { provider } = await import('../js/board.js');
  const { openSettings, selectSettingsSection } = await import('../js/settings.js');
  const { backToBoard } = await import('../js/layout.js');
  const { LOCALES, applyTranslations, setLocale } = await import('../js/i18n.js');
  const { SETTINGS_SECTIONS } = await import('../js/settings-search-model.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  let failed = false;
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -Math.max(1, a), b });
  };
  const metric = (name, a = 0, b = 0) => inv('ui_event', { code: 'smoke-check', detail: name, a: Math.trunc(a), b: Math.trunc(b) });
  const snapshot = async name => { await pause(150); await inv('smoke_native_snapshot', { name }).catch(() => -9); };
  const SHORT = { en: 'en', 'zh-Hans': 'zh', ja: 'ja', ko: 'ko', es: 'es', de: 'de' };

  /* a leaf of visible text whose words run past its own box, not truncated on purpose */
  const overflowing = root => [...root.querySelectorAll('*')].filter(el => {
    if (el.closest('.xterm, svg, select, input, textarea, canvas, [hidden]')) return false;
    if (![...el.childNodes].some(node => node.nodeType === 3 && node.textContent.trim())) return false;
    if (el.getClientRects().length === 0 || el.clientWidth === 0) return false;
    return getComputedStyle(el).textOverflow !== 'ellipsis' && el.scrollWidth > el.clientWidth + 1;
  });
  const mark = nodes => { for (const node of nodes) node.style.outline = '2px solid #ff2d55'; };
  const unmark = nodes => { for (const node of nodes) node.style.outline = ''; };
  const emptyTranslations = () => [...document.querySelectorAll('[data-i18n]')].filter(el => !el.textContent.trim()).length;
  const sideways = () => document.documentElement.scrollWidth > innerWidth + 1;
  const inWindow = node => { const r = node.getBoundingClientRect(); return r.left >= -1 && r.right <= innerWidth + 1 && r.top >= -1 && r.bottom <= innerHeight + 1; };

  let stage = 0;
  try {
    stage = 1;
    await metric('ll-m-window', innerWidth, innerHeight);
    state.view === 'board' || backToBoard();
    const project = provider.projects()[0];
    const column = project.columns[0];
    for (const title of ['scheduler refactor', 'docs', 'release notes and a longer card title']) {
      await provider.createStarted({ projectId: project.id, columnId: column.id, title, cmd: '', dir: '/tmp' });
    }
    await pause(600);

    for (const locale of LOCALES) {
      stage = 2;
      const short = SHORT[locale];
      setLocale(locale);
      applyTranslations();
      await pause(300);
      let overflow = 0, empty = 0, outside = false, scrolled = false;

      state.view === 'board' || backToBoard();
      await pause(300);
      const board = overflowing(document.body);
      overflow += board.length; empty += emptyTranslations(); scrolled ||= sideways();
      mark(board); await snapshot(`ll-${short}-board`); unmark(board);

      await openSettings();
      for (const section of SETTINGS_SECTIONS) {
        selectSettingsSection(section.id);
        await pause(200);
        const found = overflowing($('settings-box'));
        overflow += found.length; scrolled ||= sideways(); outside ||= !inWindow($('settings-box'));
        if (['general', 'terminal', 'agents', 'integrations'].includes(section.id)) {
          mark(found); await snapshot(`ll-${short}-settings-${section.id}`); unmark(found);
        }
      }
      $('set-close').click();
      await pause(200);
      await report(`ll-${short}`, empty === 0 && !scrolled && !outside, 1 + (empty ? 2 : 0) + (scrolled ? 4 : 0) + (outside ? 8 : 0), overflow);
    }
    setLocale('system');
    await report('done', !failed, 1, 0);
  } catch (error) {
    await inv('ui_event', { code: 'js-reject', detail: (error && error.name) || 'error', a: 22, b: stage });
    await report('done', false, 0, 22);
  }
}
