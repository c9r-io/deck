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
   and, when present, the foreground process before literal paste. When a
   foreground process is expected, the same condition also requires the pane
   to have bracketed paste enabled, for single-line text too: an agent that
   is still starting, or has just exited, has not enabled it. The separate
   Enter repeats the whole condition. The refusal branch deletes the buffer
   and sends nothing.

Only the transition into `firing` increments attempts and creates the pending
ledger. Context waiting cannot become ambiguous after a crash. Existing
post-intent finalize, audit, retry, group blocking and ambiguity semantics stay
unchanged.

## Closed outcomes and recovery

The scheduler persists only concrete outcomes: process matched, compatibility
target, foreground different, identity changed, session missing, startup
failed/timeout, probe failed, or cancelled/revised. It does not infer agent readiness from terminal
text, quiet time, output activity or hooks.

- Foreground mismatch: keep waiting or cancel/reschedule. Manual immediate
  send applies the same guard and reports the mismatch; there is no bypass.
  A list can hold external (Slack channel) text, and a row admitted before
  the `external` mark existed carries no origin, so no path may clear a
  row's expected process.
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
| Final probe -> paste | The synchronous tmux condition checks exact identity and, for a process-bound row, the foreground process and bracketed paste; any change takes the refusal branch. |
| Paste landed -> program reads it | Not closable from tmux. If the agent exits after the atomic check and before it reads the bytes, they stay in the tty for the next reader (the shell). They are wrapped in bracketed-paste marks, which zsh's default binding inserts into the line editor instead of running (not verified for every shell configuration), and the Enter step is refused because the foreground changed. |
| Intent persisted -> accepted send | Existing pending-ledger and ambiguous-on-crash contract applies. |
| Delete during send | Existing tombstone and session-reaping contract applies. |

## Agent hold

Context protection decides WHERE a row may go; the agent hold only decides
that a row must not go YET. The agent status hook's closed word can hold a
row, never release or target one (`scheduler/select.rs`, `agent_holds`):

- No automatic row of any mode is selected while the session's agent reports
  `needs-input`. A permission prompt is quiet output, and the paste plus its
  separate Enter would answer it — typically accepting the highlighted
  "Yes" — instead of reaching the prompt box.
- A row marked `external` — admitted through `channel_queue_add*` (Slack
  channel and badge rules, every Connector-originated row) or queued with
  `externalText` (a verbatim Slack buffer entry) — that follows a previous
  row (`chain`) is never selected automatically; the user sends it by hand
  (stage `external`) — unless it carries **content authority** (below).
  No hook word releases it: `turn-done` ends an interaction but the agent
  may still own background work and resume on its own. The first row of a
  run is not held without hooks, since the agent has had no turn yet.
- No automatic row of any mode is selected into an existing session whose
  active pane runs Codex — the foreground is `codex`, a Codex trust proof
  exists for the current process, or the row is configured for Codex
  (covering `node` and wrapper launches) — unless Codex's status is
  *trusted* for that Codex process: one of its hooks was accepted as
  provably from that pane. Until then (a fresh process, or Deck just
  restarted), and once any of its hooks is refused as
  `terminal-discontinuity` (Codex 0.157's shared background service) —
  which also withdraws its earlier status and is never healed by a later
  hook of the same process — Deck cannot see a Codex permission prompt, so
  "no hook word" must not fall back to the quiet-only rule. A failed pane
  listing sends nothing that tick. A Deck server POSITIVELY proven absent
  (tmux's own no-server reply) or reachable with zero sessions (its exact
  `no current target` answer, confirmed by a fresh probe of the same
  server) is not a failure but an empty listing
  (`tmux_lifecycle::scheduler_pane_listing`): there is no agent to protect,
  so a due automation row is selected and its session started — which for
  Claude or Codex is `StartedAwaitingInteraction`, never a paste. Every
  other failure (timeout, malformed output, an unreachable or inconsistent
  probe, a server with sessions) still selects nothing.
- **Agent Bootstrap Input Safety.** Unattended prompt delivery to Claude or
  Codex requires the Agent Status integration and at least one trustworthy
  interaction in the current agent process generation: for Codex its status
  is *trusted* (above); for Claude one of its interaction hooks
  (UserPromptSubmit, the permission notification, Stop) was accepted from
  that exact process. Until then every automatic row configured for that
  agent is held at stage `first-send` ("Waiting for first agent
  interaction"). Process identity is not input readiness and time is not
  input authority: the agent being in the foreground, a quiet pane,
  bracketed paste or a startup delay proves nothing, because a startup
  dialog — an update offer, folder trust, first-run setup, an MCP or hooks
  review — may own Enter. A new agent process and a Deck restart both start
  without evidence; nothing is persisted to skip this. Without the
  integration the stage never clears and the row waits for **send now** —
  an intentional degradation. Interacting with the agent once (so its hook
  is accepted) releases it; send now does not by itself, since only an
  accepted hook is evidence. The evidence is only this prerequisite: every
  other hold still applies after it. Programs other than Claude and Codex
  are unaffected.
- **A newly started agent is never sent its first prompt automatically.**
  When a successful pane listing shows the session absent, the scheduler
  may start it (a clock automation's card starts this way), bind its pane
  and stop there: nothing is pasted, no Enter is sent, and the row is not
  consumed, retried, counted or advanced — it stays pending at
  `first-send`. Other process-bound programs keep the old fresh-start path
  (a 2.5 s settle, then delivery).
- Owner rows without a hook word keep the quiet-only rule (Codex: once
  trusted). Manual immediate
  send is not held. A stale `needs-input` (a question dismissed with Esc
  fires no Stop hook) holds until the next hook word or until poll
  reconciliation sees the agent leave the foreground.
- The hold never moves a card and consumes no attempt. The panel plan
  names each hold as its own closed stage — `agent` (an input or permission
  request), `external` (external content no approval covers), `codex-signal`
  (Codex Signal cannot be attributed to this process), `first-send` — and
  says whether the row carries an approval ("Approved step · …"); the hook
  observation is shown beside it.

## Automation delivery authority

Deck may coordinate an Agent session; it does not own the Agent's execution.
Deck owns attention around work; the Agent owns the work. Three facts about a
row are kept apart (`scheduler/authority.rs`):

- **Provenance** — `external`: the row entered through the external
  admission. Never cleared or rewritten by anything below.
- **Content authority** — `authority`: the user explicitly approved this
  exact step of this exact version of a Slack badge automation for
  automatic delivery. External provenance ≠ unapproved content.
- **Input readiness** — every hold above. Authorization ≠ input readiness.

An approval is stored on the rule (`autoSend` in settings.json) as a
content-addressed grant: a SHA-256 over the rule's id, trigger, badge,
project, directory, full command, template name, finish mode and review mode,
together with each template step's SHA-256, its class and the external-content
acknowledgment. Hashes only; no prompt text is copied. Any change to those
fields or to the template voids it (the drawer shows "needs approval again");
the name, the board column and pause state are presentation. Step classes:

| Class | Example | Automatic delivery |
|---|---|---|
| fixed — owner text only | `Review the latest changes and run the tests.` | with an approval |
| bounded — owner text with `{{msg.*}}` | `Investigate the issue below: {{msg.text}}` | only with the separate, explicit acknowledgment that untrusted Slack text may reach the agent — Deck does not claim the text is safe |
| verbatim external — the message is the prompt (a scratchpad copy, a Connector message) | `{{msg.text}}` alone is refused by admission; copies use `externalText` | never; send-now only |
| generated / derived content | — | out of scope: no agent result, hook word or Signal creates content |

The webview freezes a run's approval into its `inboundPlan` (`{rule, grant,
trigger, classes, event, skeletons}`) and claims step k for row k on
`channel_queue_add*` only; the owner commands refuse a claim. The backend
re-checks each claim against the CURRENT settings grant (`verify_claim`: a
Slack badge rule, a valid grant equal to the claimed one, the rule's exact
command, not verbatim text) and proves the row's bytes natively:

- a **fixed** step: the row text's SHA-256 equals the approved step's;
- a **bounded** step: the claim's skeleton hashes to the approved step and
  carries a known placeholder, and its deterministic expansion
  (`expand_bounded`, the twin of the webview's `fillInboundTemplate`, both
  pinned by the vectors in `ui/test/fixtures/automation-grant.json`) over
  the backend's own copy of the claimed Slack event (still pending until
  the whole plan is queued; source, key, badge and rule must match) equals
  the row text byte for byte. An authority-bearing bounded row therefore
  holds no byte outside the expansion of the approved step over the exact
  admitted event: replacement text, another event, another step index,
  another skeleton of the same shape, an edited message or a different
  class all fail closed.

A refused claim admits the row without authority: it still runs by
send-now. An approved external follow-up is then selected like an owner
row — quiet time, group order, send gap, pause, review checkpoints,
needs-input, Codex trust, the first-interaction gate, target identity,
expected process and the bracketed-paste check all still apply.

- **Signal never authorizes.** `working`, `needs-input`, `turn-done`,
  quiet time and output activity can hold an approved row; none creates,
  restores or upgrades an approval (`tests/signal_census.rs`
  `signal_never_writes_content_authority`; scheduler test
  `turn_done_may_release_no_authority_even_when_unattended_grant_exists`).
- **Authorization survives; readiness does not.** A row's authority is
  durable (queue.json); interaction evidence is in memory only, so after a
  Deck restart or a new agent generation the first-interaction gate holds
  the approved row again until the agent proves an interaction.
- **Fresh agents.** An approved first step for a fresh Claude or Codex is
  started and never typed into (`StartedAwaitingInteraction`), exactly as
  without an approval. Zero-click bootstrap of a fresh interactive agent is
  not supported: the one real interaction (or send-now) there is a safety
  boundary, not a confirmation to optimize away.
- **Revocation fence.** The irreversible boundary is the persisted firing
  intent. An automatic send of an approved row re-reads settings and
  re-validates the approval inside the very transaction that persists that
  intent, holding `storage::settings_fence` — the lock every settings write
  takes. So once a revoking write (unticked, the rule or template edited,
  the rule deleted) has returned, no automatic send can begin under the
  revoked grant: the fence strips the row's authority (revision bumped)
  and sends nothing. A send already past the boundary completes; a crash
  there stays ambiguous and is resolved by the user as before. The tick's
  sweep (`revoke_stale`) strips the same rows so the panel and disk agree;
  a failed save of it sends nothing that tick. Re-approving creates a new
  grant: a revoked run stays manual. Editing a row's text also drops its
  authority. Send-now never consults the fence.
- **Unreadable settings hold.** A failed read is no proof either way: no
  row, and no stored authority, is removed or rewritten, but every
  automatic send that relies on an approval holds (stage
  `authority-unverified`, and the fence refuses) until settings can be read
  again. Send-now still works.
- **Content snapshot vs authority lifetime.** A run's prompt bytes are frozen
  when it is created; no rule or template edit ever rewrites them. Its
  authority is not frozen: it lives exactly as long as the grant version it
  was admitted under. Editing anything the grant covers retires that
  version (this is deliberately conservative: an edited definition is a
  different approval), so the run's unsent rows keep their text and lose
  only automatic delivery.
- **Scope.** Only Slack badge rules carry approvals. Slack channel monitors
  (a message triggers without a per-message human action), Connector rows
  and scratchpad copies stay manual; clock rules are owner text and
  unchanged.
- **Audit.** A delivery record copies the row's authority (rule id, grant
  digest, step, class, trigger) and whether the user sent it by hand; the
  ⏱ history shows "approved step N" and "sent by you". Log lines carry
  closed words and counts only (`tests/log_privacy.rs`).
- **Compatibility.** No schema door is needed: an older Deck ignores the
  row's `authority` (it keeps holding the external row) and a rule's
  `autoSend` (its drawer drops it on the next edit, which only withdraws
  automatic sending). Legacy rules have no approval and keep the Stable
  behaviour.

A verbatim external message is also refused at admission when its first
visible character is `!`, `/` or `#` (after Unicode whitespace and format
characters): `ops::leading_command` is authoritative, and buffer-model.js
`leadingCommand` is its UI twin.

## Self-audit

No hook, agent class, readiness label, quiet state or output heuristic is a
necessary condition for delivery; an `external` follow-up row is delivered
only by the user's send-now or under the user's explicit, revision-bound
automation approval (agent hold and delivery authority above). A resolvable pane owned by the card is
always necessary, and the identity read from it must stay stable from the
readiness probe through the atomic paste. Foreground equality is necessary
only when deck captured an expected executable automatically. Compatibility
delivery with no expected process deliberately retains the residual risk that
literal input can be interpreted by a shell — including by a shell that came
back after a restart before the user relaunched their agent; the UI explains
that fact without asking the user to configure a policy.

Saving a list that holds external text as a project template is the user
adopting that text as their own: a template has no origin, and a template
used on a shell card is compatibility delivery.


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
