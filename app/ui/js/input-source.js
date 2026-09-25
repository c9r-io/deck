// Ambient macOS input-source display. Rust sequences both native change
// events and snapshots, so a late snapshot cannot overwrite a newer event.
import { $, inv, listen } from './state.js';
import { onLocaleChange, t } from './i18n.js';

export function createInputSourceState(paint) {
  let sequence = 0;
  let name = null;
  let icon = null;
  return {
    apply(view) {
      if (!Number.isSafeInteger(view?.sequence) || view.sequence <= sequence) return false;
      sequence = view.sequence;
      name = typeof view.name === 'string' && view.name.length ? view.name : null;
      icon = typeof view.icon === 'string' && view.icon.startsWith('data:image/png;base64,') ? view.icon : null;
      paint({ name, icon });
      return true;
    },
    repaint() { paint({ name, icon }); },
  };
}

const state = createInputSourceState(({ name, icon }) => {
  const indicator = $('input-source-indicator');
  if (!indicator) return;
  indicator.hidden = !name;
  const image = $('input-source-icon');
  image.hidden = !name || !icon;
  image.src = name && icon ? icon : '';
  const label = $('input-source-name');
  label.hidden = !!icon;
  label.textContent = name || '';
  indicator.setAttribute('aria-label', name ? t('inputSource.current', { name }) : '');
  indicator.title = name ? t('inputSource.current', { name }) : '';
});

export function refreshInputSource() {
  return inv('input_source_snapshot').then(view => state.apply(view)).catch(() => {});
}

export async function initInputSource() {
  await listen('input-source-changed', event => state.apply(event.payload)).catch(() => {});
  onLocaleChange(() => state.repaint());
  await refreshInputSource();
}
