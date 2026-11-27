# Interjected worker lines — isolated agent probes (2026-11-27)

What each agent does with a line Deck pastes and submits while it works, and
with the same keystrokes when one of its own dialogs is on screen. Each probe
ran in a private tmux server (bundled tmux 3.7c, copied into the probe's own
root under `/private/tmp`, registered with `scripts/edr_runtime.py experiment`
and stopped by its `cleanup`), with Deck's own delivery sequence reproduced by
hand: `load-buffer`, `paste-buffer -p`, 600 ms, `send-keys Enter`. Hook
timings come from a probe hook (Claude Code `--settings`, only the probe's
hooks, no MCP server) or from a stand-in status socket that the production
`deck-status-helper` wrote its line to (Codex, agy: the user's own Deck hook
entries, the pane's `DECK_STATUS_SOCK` pointed at the probe). No agent
setting of the account was changed: Claude Code with `--setting-sources
project --strict-mcp-config`, Codex with per-session `-s/-a` flags, agy with
its session-only `--settings` (folder trust and `queuedMessages`); the agy
trust list was read before and after and holds no probe entry. Every
"approved" command below was a harmless `touch` of an empty file inside the
probe root. Times are seconds; ids are left out.

## Claude Code 2.1.332 (Sonnet 5.5, this Mac)

| Case | Observed |
|---|---|
| Line submitted while the model writes (no tool) | Shown as "Queued · ctrl+x ctrl+s to send now"; read when the turn ended, as the next turn; the instruction was followed (`INTERJECT-A-OK`). `UserPromptSubmit` fired at submit time with the RUNNING turn's prompt id; the next turn had no `UserPromptSubmit` of its own, only a `Stop` (so the card shows two `turn-done` episodes ~0.6 s apart). |
| Line submitted while a tool runs (`sleep 20 && echo … `, auto mode) | Queued at +2.5 s into the tool (PreToolUse 9.73 → submit 12.2 → PostToolUse 31.04); read right after the tool, in the SAME turn (one `Stop`); step 2 wrote `result-interject.txt`, `result.txt` never existed. |
| Permission dialog (default mode, Bash `touch`) | Dialog on screen 0.08 s after `PreToolUse`; `Notification(permission_prompt)` 6.0 s after the dialog (twice measured: 5.99 s, 6.04 s), and none at all when the dialog is answered within ~1 s; `PermissionRequest` 0.037 s after `PreToolUse`, as the dialog appeared (the dialog does NOT wait for the hook: a 3 s hook, dialog at +0.035 s); same top-level `prompt_id` as the turn. |
| Paste + Enter into that dialog | The paste was swallowed; **Enter selected "1. Yes": the tool ran** (`probe-perm.txt` created). Esc declined (nothing created). |
| `AskUserQuestion` dialog | `PermissionRequest` +0.033 s after `PreToolUse`, Notification +6.0 s. **Paste + Enter answered the question with the pasted text** ("Which color do you prefer? → INTERJECT: …"). |
| Auto mode, auto-approved tool | No dialog, no `PermissionRequest`, no Notification. |

Consequence: with the `permission_prompt` Notification alone, an
interjected line had a ~6 s window per dialog in which Deck saw `working` and
its Enter would approve a tool or answer a question (an ordinary line's 10 s
quiet covers it). Deck's Claude Code hooks now also map `PermissionRequest`
to `needs-input` (`agent_status/hooks.rs`), and an interjected line reads the
pane's word again before its paste and before its Enter.

## Codex CLI 0.172.0 (`--no-daemon`, this Mac)

| Case | Observed |
|---|---|
| Line submitted while a command runs (`sleep 20 …`) | Injected into the running turn (steer): after the command, step 2 used the new name; one turn (`turn-done` once). |
| Line submitted while the model writes | Steered into the same turn; the output ended with the requested marker; the steer fired `UserPromptSubmit` (`working`) with the SAME interaction id, so the episode did not move. |
| Approval overlay (`-s read-only -a on-request`, `touch`) | `PermissionRequest` → `needs-input` reached the socket 0.054 s BEFORE a 50 ms poll saw the overlay. **Paste ignored, Enter approved "1. Yes, proceed"** (file created). No hook follows the approval until `turn-done`, so `needs-input` stays until the turn ends. |
| 0.172.0 options | `-a` takes `on-request` and `never` only (`untrusted` is gone). |

## agy 1.5.18 (Gemini 4 Neon, this Mac)

| Case | Observed |
|---|---|
| Hooks | `working` (PreInvocation turn start) and `turn-done` (Stop) only; nothing for a permission question. |
| Line submitted while a command runs, `queuedMessages` default (`queue`) | Shown as queued ("alt+m to send immediately"); agy put the long command in the background, ended its turn, then took the queued line as its NEXT turn and followed it. |
| Same with `queuedMessages: send-immediately` (session-only) | "Sending to agent…"; taken during the work ("noted, waiting for step 1"); followed. |
| Permission question (default mode, `touch`) | "Run this command? > 1. Yes, run command"; hooks still said only `working`. **Paste dropped, Enter approved: the command ran.** |

Consequence: agy takes no interjected line (`INTERJECT_UNSUPPORTED`). The same
question also applies to an ordinary agy worker line after its quiet time (a
dialog is quiet): see the report, it is a pre-existing gap.
