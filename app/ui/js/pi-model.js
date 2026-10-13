// pi-model.js — the DOM-free half of Pi's Settings row and of what a card
// says about a Pi (settings.js and attention.js render it). The backend owns
// every fact: this module only reduces what it answered to closed values and
// names the sentences for them.
//
// # Settings
// `agent_hooks_status` carries `pi` (the work-status switch), `pi_lifecycle`
// (the session-observation switch) and `pi_installation` (the state of
// Deck's one entry in Pi's own settings file). All are derived from that
// file by the backend; nothing here keeps
// a copy, and a word this build does not know is no state at all: the reduced
// flags are false; settings.js shows a mixed, unusable switch. "Configured" is never shown as
// "connected": whether a running Pi has loaded the observer is known only
// per card, from the backend's `pi_signal`.
// `piErrorKey` maps the closed refusal codes of the two setters to their own
// sentences; any other text gets the generic one and is never shown as it
// arrived.
//
// # Card coverage
// `pi_signal` is the backend's word about a recognized Pi's Signal coverage,
// not an agent state: `connected` is LIMITED coverage — Deck shows when work
// starts and when an interaction ends, and never that Pi waits for input,
// because Pi gives it no reliable way to observe that. `unavailable` claims
// no cause. Coverage creates no attention episode, joins no pending count
// and no badge, and nothing here derives readiness from it: a recognized Pi
// is never sent to automatically, whatever its coverage (the backend's plan
// names that hold, `pi-input-unverified`).
// `pi_session` is `observed` while a live observer reports which native
// session its Pi is in, and `unsupported` while that observer named a
// session id this build does not keep — said, because a session that is not
// observed must not look like one that is, or like none. The id never
// reaches the webview. Either word adds one line to the card's explanation
// and nothing else: it is no state, and it never means that Deck can reopen
// that session.
// Part of deck's no-build frontend: native ES modules, no bundler.

export const PI_INSTALLATIONS = Object.freeze(['absent', 'current', 'needs-reload', 'conflict', 'unreadable', 'unsupported-location']);
export const PI_COVERAGE = Object.freeze(['connected', 'unavailable', 'incompatible']);
export const PI_SESSION = Object.freeze(['observed', 'unsupported']);

const NOTE_KEYS = Object.freeze({
  current: 'settings.pi.note.current',
  'needs-reload': 'settings.pi.note.needsReload',
  conflict: 'settings.pi.note.conflict',
  unreadable: 'settings.pi.note.unreadable',
  'unsupported-location': 'settings.pi.note.unsupportedLocation',
});

const ERROR_KEYS = Object.freeze({
  'pi-never-ran': 'error.pi.neverRan',
  'pi-config-unsupported-location': 'error.pi.unsupportedLocation',
  'pi-config-unreadable': 'error.pi.unreadable',
  'pi-config-layout': 'error.pi.unreadable',
  'pi-config-conflict': 'error.pi.conflict',
  'pi-config-busy': 'error.pi.busy',
  'pi-config-unlocated': 'error.pi.unlocated',
  'pi-resource-missing': 'error.pi.resourceMissing',
});

const own = (map, code) => (typeof code === 'string' && Object.hasOwn(map, code) ? map[code] : null);

/** What the backend said about Pi's settings entry, reduced to closed values.
 *  `installation` is null for a word this build does not know. */
export function piStatus(status) {
  const installation = PI_INSTALLATIONS.includes(status?.pi_installation) ? status.pi_installation : null;
  return {
    signal: installation != null && status.pi === true,
    lifecycle: installation != null && status.pi_lifecycle === true,
    installation,
  };
}

/** The Settings rows: whether each switch is on, whether they can be used,
 *  and the one note under them (a dictionary key, or null). */
export function piSettingsView(status) {
  const { signal, lifecycle, installation } = piStatus(status);
  return {
    signal,
    lifecycle,
    // a build that may not write Pi's settings, or a state it cannot name
    usable: installation != null && installation !== 'unsupported-location',
    note: installation == null ? 'settings.pi.note.unknown' : own(NOTE_KEYS, installation),
  };
}

/** The sentence for a refused switch: its own for a closed code, else the
 *  generic one. The backend's text is never shown as it arrived. */
export function piErrorKey(message) {
  return own(ERROR_KEYS, message) || 'error.agentHooks';
}

/** The coverage word of a live card's snapshot, or null. */
export function piCoverage(snapshot) {
  if (!snapshot?.alive) return null;
  return PI_COVERAGE.includes(snapshot.piSignal) ? snapshot.piSignal : null;
}

const STATUS_KEYS = Object.freeze({
  connected: 'signal.pi.connected',
  unavailable: 'signal.pi.unavailable',
  incompatible: 'signal.pi.incompatible',
});
const HINT_KEYS = Object.freeze({
  connected: 'signal.pi.connectedHint',
  unavailable: 'signal.pi.unavailableHint',
  incompatible: 'signal.pi.incompatibleHint',
});

/** What a card without an agent word says about its Pi, or null. */
export function piStatusKey(snapshot) {
  return own(STATUS_KEYS, piCoverage(snapshot));
}

const SESSION_KEYS = Object.freeze({
  observed: 'signal.pi.session',
  unsupported: 'signal.pi.sessionUnsupported',
});

/** Whether a live observer reports this card's native Pi session. */
export function piSessionObserved(snapshot) {
  return !!snapshot?.alive && snapshot.piSession === 'observed';
}

/** The line a live card says about its Pi's native session, or null. */
export function piSessionKey(snapshot) {
  return snapshot?.alive ? own(SESSION_KEYS, snapshot.piSession) : null;
}

/** The lines of the card's "Why?" dialog for a Pi, or null when the card has
 *  nothing to explain. The limited-coverage note stands under every state,
 *  a real `working` or `turn-done` included. */
export function piDetailKeys(snapshot) {
  const coverage = piCoverage(snapshot);
  if (!coverage) return null;
  const lines = [HINT_KEYS[coverage], 'signal.pi.hold'];
  const session = piSessionKey(snapshot);
  if (session) lines.push(session);
  return lines;
}
