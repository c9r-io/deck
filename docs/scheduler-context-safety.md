# Scheduled prompt automatic context protection

This note defines the boundary between deciding that a scheduled item is due
and typing it into tmux. Context protection is automatic: there is no saved or
user-selectable safety policy, no agent-readiness hook contract, and no
agent-specific readiness state.

## Persisted model

Each item keeps its card id, revision, optional exact tmux binding, optional
`expected_process`, and a closed last-context result. The binding contains the
tmux server pid plus session, window, pane and pane-process ids. The expected
process is only a sanitized executable basename; arguments, paths, terminal
contents and raw commands are never copied into context state or logs.

At creation, deck derives `expected_process` from an explicit card launch
command, including assignment prefixes, `env`, and absolute executable paths.
If no executable can be derived and the session already exists, deck captures
the sanitized foreground basename only when it is not a shell. Otherwise the
field remains absent. An absent expected process is intentional compatibility
mode, not a failed readiness check.

Legacy `agent-ready`, `foreground-match`, and `force-generic` fields are
ignored on load and disappear on the next save. Legacy AgentClass and hook
results grant no authority. Schedule, grouping, recurrence and delivery state
remain intact.

## Automatic decision model

Every delivery uses the following order:

1. Re-select the item and reject pause, edit, removal, card deletion or
   revision changes.
2. If the session is absent, start it once and perform a bounded, cancellable
   metadata probe until the pane and any expected foreground process appear.
3. Persist the identity the probe just observed. The stored binding is a
   generation stamp, not a permanent target: a pane that returns under the
   same deck-owned session name with a new generation is adopted.
4. When `expected_process` exists, require the current sanitized foreground
   basename to match. A mismatch waits without consuming an attempt.
5. When `expected_process` is absent, a resolvable pane is sufficient. A
   shell, an agent without hooks, output activity, quiet output and unknown
   agent type do not block compatibility delivery.
6. Re-select and probe again immediately before persisting firing intent.
7. After intent, one synchronous tmux command queue loads prompt plus CR into
   a private buffer and atomically checks the identity persisted in step 3
   and, when present, the foreground process before literal paste. The
   refusal branch deletes the buffer and sends nothing.

Only the transition into `firing` increments attempts and creates the pending
ledger. Context waiting cannot become ambiguous after a crash. Existing
post-intent finalize, audit, retry, group blocking and ambiguity semantics stay
unchanged.

## Closed outcomes and recovery

The scheduler persists only concrete outcomes: process matched, compatibility
target, foreground different, identity changed, session missing, startup
failed/timeout, probe failed, or cancelled/revised. It does not infer agent readiness from terminal
text, quiet time, output activity or hooks.

- Foreground mismatch: keep waiting, cancel/reschedule, or request a one-shot
  immediate send. The latter requires a pointer-confirmed warning and bypasses
  only the process comparison for that send.
- Identity mismatch: only reachable while deck is waiting for a session it
  just started. The pane is churning, so the item keeps waiting and the next
  pass re-observes it.
- Session/startup unavailable: keep the item pending with its exact reason and
  retry on a later scheduler pass.

Adopting a new generation is not an override of the safety model. The target
is located by deck's own session name on deck's own private socket, a deleted
card tombstones every item of that session, and `expected_process` — not the
generation stamp — is what decides whether the pane is running the right
thing. A hard block there was instead a guaranteed false positive: every
production upgrade replaces the tmux server, so every item of every card was
blocked after every update, with chain groups stalled behind their head step.

## Race audit

| Window | Required result |
| --- | --- |
| Tick selection -> worker | Fresh selection honours pause/edit/remove/delete. |
| During startup probing | Cancellation/revision check stops promptly; no attempt exists. |
| Session or pane recreated between deliveries | The new generation is observed and persisted; `expected_process` still gates the send. |
| Pane replaced while deck waits for the session it started | Startup polling stops on the mismatch without an attempt. |
| Probe passed -> intent | Fresh item and revision comparison, then a second metadata probe. |
| Final probe -> paste | The synchronous tmux condition checks both exact identity and optional foreground process; either change takes the refusal branch. |
| Intent persisted -> accepted send | Existing pending-ledger and ambiguous-on-crash contract applies. |
| Delete during send | Existing tombstone and session-reaping contract applies. |

## Self-audit

No hook, agent class, readiness label, quiet state or output heuristic is a
necessary condition for delivery. A resolvable pane owned by the card is
always necessary, and the identity read from it must stay stable from the
readiness probe through the atomic paste. Foreground equality is necessary
only when deck captured an expected executable automatically. Compatibility
delivery with no expected process deliberately retains the residual risk that
literal input can be interpreted by a shell — including by a shell that came
back after a restart before the user relaunched their agent; the UI explains
that fact without asking the user to configure a policy.
