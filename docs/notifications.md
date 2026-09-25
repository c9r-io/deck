# Away notifications and the Dock badge

deck can tell you when an agent asked for input or ended a turn while its
window is not in front, and count those cards on the Dock icon. Both are
off by default and both are derived from the same agent-status hook
words the Board already uses; nothing is inferred from output.

The words are interaction observations, not task truth. **Input
requested** (`needs-input`) says the agent raised a question or permission
prompt; it does not prove the agent is still waiting when you look.
**Turn ended** (`turn-done`) says an interaction ended; it does not mean the
task is complete or successful, that the agent is idle, or that its program
or background work finished — Claude Code, for example, resumes by itself
when a background command it started completes. Unread/viewed is Deck's
attention bookkeeping about whether you have looked, and an *old snapshot*
is about freshness of the last poll; neither is agent state.

## What triggers a notification

- An agent reports an **input request** (a question or permission prompt
  raised during a turn), or
- an agent reports its **turn ended** and that ending has not been viewed
  yet.

Only the agent in a card's **active pane** counts — the pane of the
session's current window that deck delivers input to. An agent running in
another pane of the same tmux session (a manual tmux split) does not notify,
badge or change the card until you make its pane the active one; without a
single active pane the card has no agent state at all.

Both require the matching Agent status integration (Claude Code or Codex)
in **Settings → Agents & notifications**. Sessions without a hook state
never notify: the 15 s output heuristic is activity, not readiness, and
quiet never means ready. Nothing is posted for *working*, for a manual
follow-up star, or for a session that stopped.

**Away** means the deck window is not focused: hidden with ⌘W, behind
another app, or on another Space. A notification is posted at the moment
of the transition into one of the two states; a repeated report of the
same word posts nothing.

## What a notification contains

- Title: the card's title.
- Body: the project name and one of two fixed phrases — *asked for your
  input* or *a turn has ended* (Chinese: 请求了你的输入 / 一轮已结束), following
  the interface language.

Never a prompt, terminal output, a path or any other text. Card titles and
project names reach the notification only; app.log records closed codes
(`[notify] posted needs-input s=…` with the usual session tag).

Clicking a notification brings deck to the front and opens that card's
session (a stopped or stale card is located on the Board instead).

## Withdrawal and the badge

A notification is withdrawn when it is superseded or viewed — which does
not mean its cause was resolved: a new turn starts (*working*), you view
the card (an unread ending becomes read), the state changes to the other
kind (the newer one replaces it), or the session leaves the hook store
(the agent exited).

Viewing is tracked per **episode** — Deck's own opaque token for one
observed input request or turn ending, never an id from the agent. An
ending you viewed stays viewed when its pane becomes active again, after
the Deck window is reloaded, and when several turns pass between two
refreshes; a genuinely new ending is unread again. The viewed state lives
only while that observation does and is not kept across a Deck restart.

The Dock badge is the number of cards with an **input request** plus cards with an
**unread turn ending** — the same rows as the Needs attention list minus
manual follow-up. It updates whether or not the window is focused, and it
is cleared and no longer updated when the switch is off. Viewing a card
with an input request keeps it counted until the agent reports another
word: viewing is not answering.

## Finding the card when you come back

The same set is marked on the Board itself: a card with an **input
request** carries an *Input requested* (请求输入) badge in its top row, and a card whose
turn **ended unread** carries *Turn ended, unread* (结束未读). Returning to
deck after a notification or a Dock count, the badge shows which card it
was, without opening the Needs attention list. The badge is independent
of this feature: it shows with notifications off, with permission
blocked, and whether or not a notification was actually delivered.

Viewing an unread ending clears its badge at once (the card itself does
not change); a card that still needs input keeps its badge until the
agent moves on. A badge on a card whose status could not be refreshed is
drawn dashed and its tooltip says *old snapshot*. Badges never move a
card.

## Permission and status

Turning the switch on is the one moment deck asks macOS for notification
permission (alert, badge, sound). The row under the switch shows the
closed status: *allowed*, *delivered quietly*, *blocked* (allow deck in
System Settings → Notifications; deck never retries on its own),
*not answered yet*, or *unavailable in this build*. **Play a sound** adds
the default notification sound and is off by default.

## Why the trigger lives in the backend

macOS App Nap freezes the webview's timers while the window is in the
background — exactly the away situation. The decision is therefore made
in `notify.rs` on the agent-status listener thread as each hook event
arrives, not by the Board's poll. The webview only supplies card titles
and project names (kept in memory, never logged), tells the backend when
an unread ending was viewed, and passes the two settings.

## Known limits

- Requires an installed `.app` bundle (Stable, Nightly, or `app/run.sh`).
  A bare binary or the unit-test process reports *unavailable* and posts
  nothing; the isolated smoke bundle is an app and can post.
- One notification per card at a time; the system identifier is the card's
  tmux session name.
- No history panel, no snooze, no per-project switch: one switch, two
  signals.
- Codex reports *turn ended* when the model attempts to stop, so its ending
  can be slightly early, and an Esc-interrupt reports it too while
  background terminals may keep running (see the agent-status contract).
- A turn can end more than once for one task (an agent that resumes after
  background work ends a second turn); each ending notifies once. This is
  deliberate: no delay or debounce is applied, since a delay would change
  when you learn about an ending without making the ending mean more.

Contract and code: `app/src-tauri/src/notify.rs` (policy, tests),
`app/src-tauri/native/NotificationBridge.swift` (UNUserNotificationCenter
only, no-op outside a bundle; `scripts/test-notification-bridge`),
`app/ui/js/notify-model.js` (labels and viewed-dismissals),
`app/ui/js/attention-model.js` `attentionBadge` (the Board card badge), pinned by
`tests/edr_quiet.rs` and `tests/log_privacy.rs`.
