# Decide what needs attention

Use Needs attention in the sidebar to gather input requests and unread turn endings across projects, reducing how often you open terminals just to check them.

## Keep a card in view

Click ☆ on a card to **Follow card**. The ★ mark keeps it in Needs attention across projects until you explicitly unfollow it, including after viewing results, status changes, moving groups or restarting deck. Existing stars become follow-up marks automatically. No status integration is needed for manual follow-up.

The **Followed** filter includes all starred cards. In Needs attention, input requests come first, then held list deliveries (**Delivery waiting**), terminal bells nobody has looked at, unread endings, due card reminders, and finally cards kept solely by manual follow-up. A card appears once even when several reasons apply; its row names each reason and keeps its live status visible. Use the star on the card or attention row to unfollow it; any other remaining reason still keeps it in the list.

When no status has been received, the followed count is still available. A total such as **3+** means at least three followed or reminder cards; additional agent requests are unknown. Stopped or stale sessions still lead to the original card.

## Enable the matching status integration

In **Settings → Agents & notifications**, under **Agent status**, enable **Claude Code** or **Codex**, depending on the CLI you use. Both are off by default. Enabling adds deck hooks to the CLI's configuration; read the confirmation for details. Other existing hooks are preserved.

Hooks report only fixed status words, pane identifiers and the agent's opaque turn id, not prompts, output or paths. They depend on the CLI's event support. Other CLIs still work as terminals but do not automatically gain these explicit states.

After enabling, use the matching CLI in a deck session. If an existing run has not reported a state, check whether its configuration has taken effect. Missing reports do not mean that no attention is needed. For Codex, also review and trust the Deck hooks in Codex `/hooks`. Codex on its shared background service (the default since 0.157) cannot be reliably tied to a terminal, so deck rejects those events and the card shows **Agent status unavailable**. **Agent status not connected** means no reliable Codex status has arrived since the terminal or deck started. **Why?** on the card explains the checks. `codex --no-daemon` in a new session is a compatible path.

## Be told while you are away

In **Settings → Agents & notifications**, under **Away notifications**, turn on **Notify me when away** (off by default). When the deck window is not in front, macOS shows a notification when an agent asks for input or ends a turn, when a list's delivery stops to wait for you, or when a program rings the terminal bell in a session without agent state. The Dock icon counts input requests, unread turn endings, waiting deliveries and unseen bells; due card reminders are counted even with the switch off. The notification carries the card title, the project name and one fixed phrase, never prompts, output or paths; click it to open the card. Agent input requests and turn endings need the status integration above; deliveries and bells do not. macOS asks for permission when you turn the switch on. **Play a sound** is a separate switch, off by default, and available only while away notifications are on.

## Read each signal

| Signal | What it tells you | What to do |
| --- | --- | --- |
| Input requested | An agent input request was reported | Open the terminal to read the question or permission choice |
| Turn ended · unread | A turn ending was reported and has not been successfully opened | Read the result and decide what comes next; it does not prove success |
| Agent working | The agent reports ongoing work | Wait or inspect as needed |
| Recent output · no valid agent state | Output activity was observed | Treat it as activity, not readiness |
| Quiet · no valid agent state | No output for a while | It may be computing, waiting for input or idle; inspect as needed |
| Agent status not connected / Agent status unavailable | No reliable Codex status for this terminal | Click **Why?** on the card; until a reliable status arrives, the dot shows output activity only |
| Stopped, Status not received, or · old snapshot | The original session stopped, or current information is missing | Locate the original card and check its terminal and connection |

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
