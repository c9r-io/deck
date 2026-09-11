# Decide what needs attention

Use Needs attention in the sidebar to gather input requests and unread turn endings across projects, reducing how often you open terminals just to check them.

## Enable the matching status integration

In **Settings → Integrations & automation**, enable Agent status for **Claude Code** or **Codex**, depending on the CLI you use. Both are off by default. Enabling adds deck hooks to the CLI's configuration; read the confirmation for details. Other existing hooks are preserved.

Hooks report only fixed status words and pane identifiers, not prompts or output. They depend on the CLI's event support. Other CLIs still work as terminals but do not automatically gain these explicit states.

After enabling, use the matching CLI in a deck session. If an existing run has not reported a state, check whether its configuration has taken effect. Missing reports do not mean that no attention is needed.

## Read each signal

| Signal | What it tells you | What to do |
| --- | --- | --- |
| Needs input | An agent input request was reported | Open the terminal to read the question or permission choice |
| Unread turn ending | A turn ending was reported and has not been successfully opened | Read the result and decide what comes next; it does not prove success |
| Working · agent report | The agent reports ongoing work | Wait or inspect as needed |
| Recent output · no valid agent state | Output activity was observed | Treat it as activity, not readiness |
| Quiet · no valid agent state | No output for a while | It may be computing, waiting for input or idle; inspect as needed |
| Old snapshot, stale or stopped | Current information is missing or the original session stopped | Locate the original card and check its terminal and connection |

Without a valid agent state, green means output in the last 15 seconds and amber means quiet. **Quiet is not ready, and it is not a turn ending.** Valid agent reports take precedence over output heuristics.

## Open and return

1. Open **Needs attention** in the sidebar and choose a filter if needed.
2. Read the original project and group on each row, then open its session.
3. A turn ending is marked read only after the terminal attaches and displays successfully. A failed attach or merely locating a stale entry does not clear unread state.
4. Return after handling it; the previous filter and scroll position are restored.

An input request remains until its state changes. **Opened does not mean handled.** Marking a turn read here does not release a list's human inspection checkpoint. [Inspect before continuing →](/guide/prompts/#inspect-before-continuing)

## When the view is empty or stale

Check sessions under **No valid state**, then verify that the matching integration is enabled and the CLI is still running. Failed or incomplete polls keep a labeled old snapshot; old information is not current status.

Read markers last only for this app run. Unread history and event replay are not guaranteed across app restarts. A CLI's turn-ending report can also arrive before another hook requires it to continue, so verify important results yourself.
