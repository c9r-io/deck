# Codex shared-daemon Signal trust contract and certification

Recorded 2026-10-02. This document preserves the completed investigation; it
does not change Signal admission, hooks, scheduler, readiness or product behavior.

## Long-term trust contract

Codex shared-daemon Signal can become **Trusted Signal only when an event is
verifiably bound to a specific interactive client and that client's current pane
foreground generation**. This is a provenance requirement, not a version allowlist
or a permanent ban on shared daemons. A future release must supply fresh proof,
through trustworthy official client provenance or a topology that satisfies
Deck's ownership model; a version upgrade alone cannot restore trust.

None of these is pane ownership proof:

- inherited `TMUX_PANE` or other inherited environment;
- cwd, timing, executable name or transcript path;
- `session_id`, `thread_id` or `turn_id`.

Conversation/turn identity does not establish terminal ownership. The existing
model uses kernel peer PID, ancestry to the pane process and its current foreground
generation, and strict terminal continuity. Presentation evidence such as a terminal
title cannot upgrade `CodexSignalTrust` or release scheduler/readiness gates.

## Certification history

| Version / mode | Date | Attribution result |
| --- | --- | --- |
| Codex 0.157.1 shared daemon | 2026-09-27 | **BLOCKED**: daemon environment and cross-client ancestry do not prove pane ownership. |
| Codex 0.160.0 shared daemon | 2026-10-02 | **BLOCKED**: two-client runtime probe reproduced the same ownership discontinuity. |
| `codex --no-daemon` embedded | Verified through 0.160.0 on 2026-10-02 | Attribution topology can satisfy the existing Deck model; this is not certification of full attention-event coverage or the installed Deck ingestion path. |

Earlier records remain historical: [diagnosis](codex-signal-diagnosis-20260927.md),
[implementation](codex-signal-implementation-20260927.md) and
[terminal-title probe](codex-title-probe.md). A title presentation result does not
certify hook attribution.

## Codex 0.160.0 evidence

The investigated binary reported `codex-cli 0.160.0`. The stable release was
[`rust-v0.160.0`](https://github.com/openai/codex/releases/tag/rust-v0.160.0),
published 2026-10-01T20:19:13Z. The reviewed source revision was
`a956835d020762cb2b570053af06f643a11c0ecc`; its annotated tag object was
`79b1b666f2e8551f8abbbca34957227f67f3f553`.

- The complete `hooks/src/schema.rs` was byte-identical to 0.157.1. No new
  interactive-client/PID/tty binding appeared in command-hook input.
- `Hooks::new` is created during core session initialization and snapshots the
  **hosting process** environment. In shared mode that process is the daemon.
  Reconfiguration preserves the snapshot. The command runner replays it into
  a child launched in a new session; this does not create per-client provenance.
- Two interactive TUIs in different panes used the same cwd and one isolated
  daemon, with distinct pane and harmless socket markers. Neither used `-c`
  overrides or hook-trust bypass. Hooks were trusted through the normal TUI UI.
- A-only, B-only, overlapping second turns, a no-op approval request, interrupt
  and subsequent turns produced separate session/turn IDs. The overlapping
  turns overlapped for approximately 18.20 seconds. B's hooks still inherited
  A's pane/marker and descended from the daemon beneath A, not B's TUI.
- Shared hooks passed ancestry checks for the **reported A pane**, but failed
  terminal continuity: the hook and daemon occupied different terminal-less
  process groups. They could not prove ownership of B's current foreground.
- Two embedded controls each completed three turns, inherited their own
  pane/marker and descended directly from their own foreground TUI through only
  the hook's detached group. Eight start/stop kernel-peer samples distinguished
  the topologies: shared continuity failed, embedded continuity passed.
- The app-server exposes thread status and waiting flags, but no observer-safe
  authoritative mapping from an existing TUI process/pane generation to a thread.

Source anchors: [session initialization](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/session/session.rs#L1585),
[environment snapshot](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/hooks/src/registry.rs#L79),
[command spawn](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/hooks/src/engine/command_runner.rs#L386),
[hook schema](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/hooks/src/schema.rs#L501) and
[subscription registry](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/thread_state.rs#L342).

## Policy, limits and cleanup

**Keep `CodexSignalTrust::Unavailable` and all existing scheduler/readiness gates
unchanged for the blocked shared-daemon topology.** Runtime admission remains
evidence-based; this record does not add version detection or mark an unproven
pane unavailable by association with another client.

Attribution and coverage are separate. Hooks do not provide full ordinary
request-user-input, Plan, MCP elicitation or authentication waiting coverage.
PermissionRequest can precede automatic resolution; Stop can precede continuation.
Stop, Ready, idle, no spinner and silence never mean task success, no background
work, readiness for input or permission to close a card.

Reconnect/restart, resume/fork, same-thread multi-client and the full attention UI
runtime matrix were not certified. The probe checked attribution structure, not
the signed installed Deck `ingest` path. These limits do not negate the observed
cross-client ownership failure.

All experiments used temporary HOME/CODEX_HOME, tmux/socket and configuration.
Only content-free metadata was retained. The isolated daemon stop and tmux cleanup
both exited 0; owned processes, sockets/locks, credentials and temporary runtime
state were removed and audited absent. No user daemon was stopped, and neither
Deck nor Codex product code was modified during the investigation.

**Can normal shared-daemon interactive Codex sessions now provide trustworthy full
Codex Signal without Deck owning execution? NO.** Client-to-current-pane ownership
proof remains missing. Deck owns attention around work; Codex owns the work.
