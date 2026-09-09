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


## Human inspection checkpoints (C v01)

Approved governance 05 C v01 adds an optional *schedule gate* after each
sent row. It does not make hook state a target credential or weaken the
automatic context checks above. The former contract releases a group head
once terminal delivery is accounted; that remains the default for old lists.
Explicit opt-in instead retains it as `review`, including the last row. This
addresses work dependencies by requiring a human decision; it does not claim
to validate the result. A hook-driven alternative requires an event protocol
that current hooks do not have and is outside this implementation.

A preview and its confirmation bind the checkpoint's delivery id/revision,
the exact next row id/revision/expected executable, and a fresh pane identity.
The decision is saved before release. `review-approved` retains that permission
until the successor is delivered. Editing/removing/retrying the successor or
observing a changed/unavailable target revokes it and restores `review`.
A repeated confirmation is a no-op, never approval of a later row. Crashes and
failed saves leave the last durable state. A stopped target must be reopened
and inspected; requesting a preview never starts it. Current foreground is
shown in the confirmation; all usual process and atomic identity checks still
run before sending. Manual immediate send cannot bypass an unchecked predecessor.

Only its own group waits: no exclusive session ownership, output attribution,
or promise against other lists interleaving. Every repeating iteration creates
fresh checkpoints and cannot start another iteration while one remains.
Disabling inspection changes future rows but retains existing checkpoints.
Skip/cancel explicitly omits work and never marks inspection. In-flight and
ambiguous rows still require existing resolution; acknowledging an ambiguous
row as sent creates its checkpoint without resending. Explicit last inspection
removes the final hold. For opted-in automation runs, a durable per-session
last-inspection marker additionally gates the existing automatic finish path;
queue emptiness after an enqueue failure or cancellation cannot substitute for
that decision. Finishing rules and pane-view protection otherwise stay unchanged.

Delivery and human-inspection audits are separate, each capped at 200 records;
they contain identifiers/times/source, not historical prompt contents. Pending
checkpoint text remains in the queue until consumed. Queue plans are read-only
snapshots of scheduling conditions. Hook observations remain separate and have
no delivery or business-success authority. An unavailable/stale plan is shown
as unknown; it cannot be used by the UI to authorize delivery.

### Compatibility and migration

`review_each`/`reviewEach` defaults false. Loading legacy lists does not insert
checkpoints, change due times, or change automatic finishing. Opt-in is explicit
on a new list, an existing list's plan, or an automation rule (new runs only).
Reviewed multi-row enqueue is one queue transaction; existing card creation
and inbound acknowledgement remain separate lifecycle operations.

The maximum readable envelope becomes v2; ordinary saves remain v1. Only
queue.json and settings.json join the door: they are written as v2 once they
contain inspection mode/state/history or an opted-in rule, because an older
reader would otherwise resend `review` rows as ordinary due rows or apply a
finish rule to a run it cannot see. deck.json stays v1 even when a card's
`origin.reviewEach` is set — an old build whose settings are refused has no
rules and cannot auto-close anything, so locking the whole Board would add no
protection. Before saving a reviewed queue, settings receives a v2 envelope
barrier with its current payload preserved under the shared storage save lock.
This prevents an older build from loading a finish rule while treating a refused
reviewed queue as empty. A barrier write failure prevents the queue mutation.
A subsequent queue-write failure can conservatively leave settings v2 with no
new queue rows. Versions are sticky: disabling the mode does not downgrade the
envelope or make an old backup safe to restore. Older v1 readers refuse v2
untouched. Use a checkpoint-aware build; reverting to old software requires a
separate deliberate data recovery plan, never silently stripping checkpoints.
