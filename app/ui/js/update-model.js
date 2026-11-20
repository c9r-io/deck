// update-model.js — what a failed update check says, DOM-free. The backend
// (`updater.rs` `check_failure`) answers with one closed word after the
// fixed prefix "update check failed: "; anything else is the generic text.
// "Are you online?" is right only for the network words: a feed the server
// does not serve, or one this build cannot read, is not fixed by the user's
// connection, and saying so stops them from debugging their Wi-Fi.

const PREFIX = 'update check failed: ';
const KEYS = {
  'timeout': 'update.checkFailedNetwork',
  'network': 'update.checkFailedNetwork',
  'feed-unavailable': 'update.checkFailedFeed',
  'feed-invalid': 'update.checkFailedInvalid',
};

export function updateCheckFailureKey(error) {
  const text = String(error);
  const code = text.startsWith(PREFIX) ? text.slice(PREFIX.length) : '';
  return Object.hasOwn(KEYS, code) ? KEYS[code] : 'update.checkFailed';
}
