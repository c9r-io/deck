// "Draft title" for one card: asks the backend for a draft (card_title.rs:
// Claude Code's own session title, or the stored model) and opens the card's
// rename field with it. Nothing is renamed here: the field's Enter is the
// user's decision, Escape or an untouched click away drops the draft.
// One draft per card at a time; a slow one says it is under way. Failures
// are the backend's closed words, each with its sentence.
// Part of deck's no-build frontend: native ES modules, no bundler.

/* the backend's closed words → what the user is told; model_api.rs words
   reuse the Model APIs sentences */
const ERRORS = Object.freeze({
  session: 'error.titleSession', empty: 'error.titleEmpty', 'no-source': 'error.titleNoSource',
  refused: 'error.titleRefused', model: 'error.titleModel',
  auth: 'error.modelApiAuth', permission: 'error.modelApiPermission', busy: 'error.modelApiBusy',
  network: 'error.modelApiNetwork', response: 'error.modelApiResponse', keychain: 'error.modelApiKeychain',
});
export const titleDraftError = code => ERRORS[code] || 'error.titleFailed';

/** @param {{inv: Function, toast: Function, t: Function, slowMs?: number,
 *   setTimer?: Function, clearTimer?: Function}} host */
export function createTitleDrafter({ inv, toast, t, slowMs = 600, setTimer = setTimeout, clearTimer = clearTimeout }) {
  const busy = new Set();
  /* `card` is a Board card ({id, session}); `rename(title)` opens its
     rename field with the draft. Resolves true when the field was opened. */
  return async function draft(card, rename) {
    if (!card || !card.session || busy.has(card.id)) return false;
    busy.add(card.id);
    const slow = setTimer(() => toast(t('title.drafting')), slowMs);
    try {
      const reply = await inv('card_title_draft', { session: card.session });
      const title = reply && typeof reply.title === 'string' ? reply.title.trim() : '';
      if (!title) throw 'response';
      clearTimer(slow);
      rename(title);
      return true;
    } catch (error) {
      clearTimer(slow);
      toast(t(titleDraftError(String(error))));
      return false;
    } finally {
      busy.delete(card.id);
    }
  };
}
