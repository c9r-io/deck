# Decide what needs attention

Use Needs attention in the sidebar to gather input requests, unread turn endings, waiting deliveries and reminders across projects. Deck helps you decide where to look; the program and its actual results tell you what the work needs.

## Keep a card in view

Click ☆ on a card to **Follow card**. The ★ mark keeps it in Needs attention across projects until you explicitly unfollow it, including after viewing results, status changes, moving groups or restarting deck. The **Followed** filter includes these cards. No status integration is needed.

A card appears once even when several attention reasons apply. Unfollowing removes only the star: an input request, unread result, waiting delivery, unseen bell or due reminder can still keep it in the list. The Board's card positions do not change.

When status is unavailable, saved follow-up and reminder information remains useful. A total with **+** is a lower bound, not a complete count of Agent requests. Stopped or stale entries still lead to the original card.

## Enable the matching status integration

In **Settings → Agents & notifications**, enable Agent status for **Claude Code** or **Codex**, depending on the CLI you use. Both are off by default. Enabling adds Deck hooks to the CLI's configuration; read the confirmation for details. Other existing hooks are preserved.

Hooks report fixed status words, pane identifiers and opaque interaction IDs, not prompts, output or paths. Coverage depends on the CLI's events. Only the card's active pane in its current tmux window contributes Agent state; this is not coverage of every manual tmux split. Other CLIs still work as terminals without automatically gaining these states.

For Codex, installed hooks still need review and trust in `/hooks`. For a new interactive session, `codex --no-daemon` is an optional compatibility path that gives up shared-server continuity. Deck rejects events it cannot reliably attribute to the terminal. **Agent status not connected** means no reliable observation is available; **Agent status unavailable** means Deck has established an attribution gap. **Why?** explains the situation. Neither label invents an input request or counts as urgent attention. Input-request hooks do not cover every question or MCP elicitation.

## Be told while you are away

In **Settings → Agents & notifications**, turn on **Notify me when away**. It is off by default. When Deck is not in front, the switch allows notifications for reported input requests and unread turn endings. Those two signals need the matching Agent integration.

The same switch also covers **Delivery waiting** in a list, such as inspection, uncertain delivery, a failed send, unapproved external follow-ups or a first prompt waiting for interaction. These are Deck's queue states and need no Agent integration. A normal time, quiet or minimum-gap wait does not notify. A persistent inability to verify a saved approval can notify after about a minute; this delay neither approves nor sends anything.

Without Agent state, a program can also ring the terminal bell. If nobody is viewing that card, **Bell** brings it to Needs attention; while away, the switch allows a notification. Repeated bells count once until you view the card. A bell says only that the program rang, not that it finished or succeeded.

The Dock counts distinct cards with applicable input requests, unread endings, waiting deliveries and unseen bells while the switch is on. Due reminders count independently. Stars alone do not notify. Notifications contain the card title, project name and a fixed phrase, never prompt text, output or paths. Opening a notification locates its card; it never sends, retries or releases a queued row.

macOS asks for permission when you enable notifications. Check the shown authorization status; if blocked, allow Deck in System Settings → Notifications. **Play a sound** is separate and off by default. Focus modes and system settings can suppress a banner; a saved setting does not guarantee visible delivery.

## Set a reminder to return

Use **Set reminder…** in a card's context menu or Session header. Check the full date, time and time zone before saving. The saved date label opens the editor; **Due reminders** shows due items and **All reminders** also includes future ones. A reminder never sends input, starts a session, moves the card or keeps a program alive.

Reminder intent survives an app restart. Saving it is separate from macOS authorization and registration; the editor shows those results. If system notifications are unavailable or denied, explicitly choose **Remind only inside Deck**. Reminders are independent of Notify me when away. A registered reminder may notify even when Deck is in front or has quit, subject to macOS delivery conditions; missed reminders remain visible inside Deck without replaying old banners.

Viewing the card or dismissing a banner does not end its reminder. Change the date to remind later or explicitly end it. Future and due reminders protect the card from automatic closure; manual closure requires **Cancel reminder and close**. This protects the card, not its running process. [Closing and recovery →](/guide/sessions/)

## Read each signal

| Signal | What it tells you | What to do |
| --- | --- | --- |
| Input requested | An Agent input request was reported | Open the terminal to read the question or permission choice |
| Turn ended, unread | An interaction ended and has not been viewed | Read the result; it does not prove task completion, success or Agent idleness |
| Delivery waiting | A queued row needs a person | Read the reason in ⏱ and resolve that specific wait |
| Bell | A program rang the terminal bell | Open the card and check the program |
| Due reminder | Your saved return time has arrived | Review the card, then end or reschedule the reminder |
| Working · agent report | The Agent reports ongoing work | Wait or inspect as needed |
| Recent output · no valid agent state | Output activity was observed | Treat it as activity, not readiness |
| Quiet · no valid agent state | No output for a while | It may be computing, waiting for input or idle; inspect as needed |
| Old snapshot, stale or stopped | Current information is missing or the original session stopped | Locate the original card and check its terminal and connection |

Without a valid Agent state, green means output in the last 15 seconds and amber means quiet. **Quiet is not ready, and it is not a turn ending.** Valid Agent reports take precedence over output heuristics.

## Open and return

1. Open **Needs attention** in the sidebar and choose a filter if needed.
2. Read the original project and group on each row, then open its session.
3. A turn ending is marked read only after the terminal attaches and displays successfully. A failed attach or merely locating a stale entry does not clear unread state.
4. Return after handling it; the previous filter and scroll position are restored.

An input request remains until its state changes. **Opened does not mean handled.** Marking a turn read does not release a list's human inspection checkpoint. [Inspect before continuing →](/guide/prompts/#inspect-before-continuing)

## When the view is empty or stale

Check sessions under **No valid state**, then verify the matching integration, hook trust and running CLI. Failed or incomplete polls keep a labeled old snapshot; old information is not current status. An empty list is not proof that all work is done.

Viewed markers are not kept across a Deck restart; unread history and missed-event replay are not guaranteed. This differs from saved stars and reminders. An Agent may continue background work after a turn-ending report, so verify important results yourself.
