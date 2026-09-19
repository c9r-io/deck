// Terminal copy routing and exact clipboard writes. Selection snapshots and
// native/web transports are injected or bound here, never owned by layout.
import { inv, uev } from './state.js';

// A pane owns its copy attempts and notification throttle. Capture diagnostic
// identity before any await; never choose another pane's selection implicitly.
export function createTerminalCopy({ selection, term, copySelection, elsewhere, write, log, notice, now = Date.now }) {
  let attempt = 0, noticeAt = -Infinity, noticeKey = null;
  const notify = key => {
    if (key === noticeKey && now() - noticeAt < 1500) return;
    noticeKey = key; noticeAt = now(); notice(key);
  };
  return event => {
    if (event.type !== 'keydown' || !event.metaKey || String(event.key || '').toLowerCase() !== 'c') return false;
    event.preventDefault();
    const route = terminalCopyRoute(event, selection.hasSelection(), term.hasSelection());
    const context = { ...selection.traceContext(), attempt: ++attempt };
    if (!route) {
      const other = elsewhere();
      log('terminal-copy', other.count ? 'keydown-elsewhere' : 'keydown-none', other.count, other.ageMs, context);
      if (other.context) log('terminal-copy', 'source-elsewhere', context.pane, context.attempt, other.context);
      selection.traceUnavailable(context);
      notify(other.count ? 'error.copyElsewhere' : 'error.copyEmpty');
    } else {
      log('terminal-copy', route === 'deck' ? 'keydown-deck' : 'keydown-native', null, null, context);
      copyTerminalText({
        read: () => route === 'deck' ? copySelection() : term.getSelection(),
        write: text => write(text, context),
      }).then(outcome => {
        log('terminal-copy', outcome, null, null, context);
        if (outcome !== 'success') notify(outcome === 'selection-vanished' ? 'error.copyEmpty' : 'error.copy');
      });
    }
    return true;
  };
}

export async function copyExact(text, writer) {
  await writer(text);
  return text.length;
}

/** Resolve Command-C ownership without depending on xterm or the DOM. Deck's
 * token selection wins while present; otherwise xterm's native word/line
 * selection may supply the clipboard text. */
export function terminalCopyRoute(event, hasDeckSelection, hasNativeSelection) {
  if (!event || event.type !== 'keydown' || !event.metaKey
      || String(event.key || '').toLowerCase() !== 'c') return null;
  if (hasDeckSelection) return 'deck';
  if (hasNativeSelection) return 'native';
  return null;
}

/** Copy has one terminal outcome. Losing a selection while awaiting its
 * snapshot must not write an empty clipboard or report success. Error text
 * stays private; callers log only this closed outcome and their captured IDs. */
export async function copyTerminalText({ read, write }) {
  let text;
  try { text = await read(); } catch (error) { return selectionCopyFailureCode(error); }
  if (typeof text !== 'string' || text.length === 0) return 'selection-vanished';
  try { await write(text); } catch { return 'clipboard-write-failed'; }
  return 'success';
}

/* Closed reason codes derived from the backend's error SUFFIXES, never from
   error text, so they are safe to log. */
export const selectionCopyFailureCode = error => (String(error || '').includes('selection-missing')
  ? 'selection-missing' : 'snapshot-failed');
export async function writeClipboard(text, context = null) {
  try {
    const result = await copyExact(text, value => inv('write_clipboard', { text: value }));
    return result;
  } catch (nativeError) {
    uev('clipboard-write', 'pbcopy-failed', text.length, null, context);
    if (!navigator.clipboard || !navigator.clipboard.writeText) {
      uev('clipboard-write', 'web-unavailable', text.length, null, context);
      throw nativeError;
    }
    try {
      const result = await copyExact(text, value => navigator.clipboard.writeText(value));
      return result;
    } catch (webError) {
      uev('clipboard-write', 'web-failed', text.length, null, context);
      throw webError;
    }
  }
}
