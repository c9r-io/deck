# Codex 0.157.1 terminal-title probe

This prototype tests the narrow portion of Codex terminal titles that Deck
could display without interpreting project names, thread names, prompts, or
transcript content. Its reusable parser is offline. The runtime evidence below
came from disposable Codex homes and bundled-tmux sockets; it did not change
Deck, user configuration, or production sessions and daemons.

## Prototype contract

The parser assumes this exact selection, configured by the user through
Codex's normal persistent configuration UI or configuration file:

```toml
[tui]
terminal_title = ["activity", "run-state"]
```

Do not pass that setting with `codex -c` when certifying shared-daemon
behavior. In 0.157.1, configuration overrides are among the conditions that
select embedded operation, so that test would not establish shared-daemon
behavior.

For this selection, the 0.157.1 source renders a closed grammar:

- `Ready` while idle;
- one of ten Braille spinner frames, one space, then `Starting`, `Working`,
  `Thinking`, or `Waiting` while active;
- `[ ! ] Action Required` or its blink phase `[ . ] Action Required` when a
  bottom-pane view requests user action.

The action overlay replaces both the activity segment and run-state segment.
The parser rejects any additional segment. In particular, it excludes
`thread-id`: Codex truncates each terminal-title segment to 32 grapheme
clusters, so a 36-character UUID becomes a prefix ending in `...` and cannot
serve as complete identity. Realtime microphone activity adds another title
prefix; this prototype rejects that form and reports status unavailable rather
than widening the grammar.

`scripts/codex_title_probe.py` accepts bounded JSON Lines on stdin. Each input
object has `pane`, `generation`, `codex_foreground`, and `title`. The caller is
responsible for obtaining pane identity, a foreground-process generation, and
the title from the bundled tmux. Output contains only closed status words and
booleans; it never echoes input identifiers or title text. The reader limits
each physical line to 4096 bytes and drains an oversized line before parsing
the next one. Malformed JSON, duplicate fields, oversized lines, and capacity
failures reset all cached continuity, because an input gap invalidates prior
change evidence. A final bounded JSON record does not require a newline.

The first accepted title for every pane/generation is `cached-baseline`. A
tmux pane title survives the write that created it and can therefore predate a
Deck restart or a foreground-process change. Only an observed change between
two accepted titles in the same pane/generation establishes
`established_in_generation`. Losing Codex foreground, seeing an uncontrolled
title, or changing generation resets that evidence. Pane state is independent,
which prevents one pane's animation from freshening another pane's cache.

Example with synthetic input:

```sh
python3 scripts/codex_title_probe.py <<'EOF'
{"pane":"fixture-a","generation":"1","codex_foreground":true,"title":"Ready"}
{"pane":"fixture-a","generation":"1","codex_foreground":true,"title":"⠋ Working"}
EOF
```

The emitted records classify the first observation as `cached-baseline` and
the second as `changed`. They do not contain the pane, generation, or original
title.

## What this establishes

Source inspection of the `rust-v0.157.1` snapshot at commit prefix `ac0e23e`
established the exact rendering rules above:

- `tui/src/chatwidget/status_surfaces.rs` owns the default selection, closed
  run-state words, spinner frames, action overlay, and active/idle behavior;
- `tui/src/bottom_pane/title_setup.rs` owns configured order and separators;
- `tui/src/terminal_title.rs` emits OSC 0 and bounds the sanitized title;
- `tui/src/chatwidget/status_surfaces.rs::truncate_terminal_title_part`
  establishes the 32-grapheme thread-id truncation.

The 12 fixture tests cover every accepted state, both action phases, all cache
reset conditions, two-pane isolation, strict rejection of free text, the
content-free output boundary, bounded-line resynchronization, duplicate-field
rejection, and a final record without a newline. Run them with:

```sh
python3 -m unittest scripts/test_codex_title_probe.py
```

## Isolated runtime evidence

On 2026-09-27, Codex CLI 0.157.1 was exercised with a unique mode-0700
`CODEX_HOME`, a persistent `config.toml` containing the exact title selection
above, and the bundled tmux on unique `deck-smoke-*` sockets. The test home had
plugins, imported plugin hooks, external memory, and MCP servers disabled. A
minimal authentication file was copied into that private home without reading
or logging its contents, then unlinked during cleanup.

Source inspection first established that the app-server control socket and
daemon state resolve beneath `CODEX_HOME`. The first of two detached,
interactive TUI clients started one daemon; the second attached while the same
daemon PID remained stable. The isolated home contained one daemon pidfile and
one control socket. This proves the two test clients shared the isolated
daemon rather than the production daemon. Neither client used `-c`, `exec`, or
headless app-server orchestration.

The captured initial pane title was an uncontrolled shell/cache value and was
therefore unavailable until the TUI wrote an accepted title. With tmux
`allow-rename` enabled, an ordinary harmless turn produced
`Ready -> Working -> Ready` while both clients were detached and off view.
During interleaved turns, client A was active while client B remained `Ready`,
then client B became active, and both returned to `Ready`. No cross-client
attribution appeared in this tested sequence. Title strings were filtered in
memory and were not retained in experiment logs.

This is a focused observation rather than release certification. Action
requests, `/new`, resume, fork, generation changes after the initial cache,
Codex or Deck restart, disabled animations, custom/disabled titles, and
short-turn loss measurement were not run. The experiment also does not make
OSC titles authenticated process evidence; the trust ceiling below still
applies.

## Embedded hook evidence

The same isolated home was then tested with interactive `codex --no-daemon`
and the signed-bundle `deck-status-helper`, writing to a private Unix socket.
The listener retained only closed v2 fields and equality of locally assigned
interaction slots; prompt text and source identifiers were discarded.

In the untrusted control, Codex displayed its hook review and “continue without
trusting” was selected. The harmless turn completed and the listener received
zero events. For one audited test invocation only,
`--dangerously-bypass-hook-trust` enabled the same isolated hook definitions.
A normal turn emitted exactly `working` followed by `turn-done`; both were v2,
both carried an interaction id, and both mapped to the same local interaction
slot. During a second normal turn with the listener briefly holding the
connection open, process inspection showed the helper descended from the pane
process and had no terminal, matching the documented hook-child topology.

The bypass is test scaffolding, not a production configuration recommendation.
Permission/action-required and interrupt/cancel hook paths were not run. The
experiment validates helper transport and normal start/end correlation only;
production admission and process-attribution policy remain separate concerns.

Cleanup stopped the two dedicated tmux servers, exact interactive clients,
private listener, and isolated daemon/pid-update process. The temporary
authentication copy and complete test home were removed. A final process and
filesystem inventory found none of those owned resources remaining.

## Trust ceiling and product meaning

Even after a generation-local change, terminal output remains presentation
evidence. Any process that can write to the pane can emit the same OSC title.
The title has no turn id or sequence number, and polling can miss an entire
short turn. A fixed `Ready` title may never pass the cache guard after Deck
starts observing it. Disabling animations also removes spinner/blink changes,
leaving only run-state transitions as freshness evidence.

The closed states have narrow meanings:

- `Action Required` is useful as a candidate attention overlay because Codex
  derives it from bottom-pane views that require user action.
- `Working` and `Thinking` show activity. `Starting` is startup activity.
- `Waiting` means a background terminal is still running; it does not mean the
  user is needed.
- `Ready` means the TUI is not running a task. It does not mean the user's task
  succeeded, all background work ended, or the card may close.

Accordingly, these observations must not upgrade `CodexSignalTrust`, release
queue/first-interaction gates, authorize input, create a `turn-done` event, or
move/close a card. Production certification must extend the tested two-client,
offscreen shared-daemon baseline with short-turn loss measurement,
`/new`/resume/fork, foreground generation changes, Codex and Deck restarts,
titles disabled/customized, and animations disabled. Each run must establish
the actual daemon topology without `-c` overrides.
