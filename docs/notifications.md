# Away notifications and the Dock badge

deck can tell you when an agent needs you while its window is not in
front, and count the cards that are waiting on the Dock icon. Both are
off by default and both are derived from the same agent-status hook
words the Board already uses; nothing is inferred from output.

## What triggers a notification

- An agent reports **needs input** (a question or permission prompt raised
  during a turn), or
- an agent reports **turn done** and that ending has not been viewed yet.

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
- Body: the project name and one of two fixed phrases — *needs your input*
  or *a turn has ended* (Chinese: 需要你的输入 / 一轮已结束), following
  the interface language.

Never a prompt, terminal output, a path or any other text. Card titles and
project names reach the notification only; app.log records closed codes
(`[notify] posted needs-input s=…` with the usual session tag).

Clicking a notification brings deck to the front and opens that card's
session (a stopped or stale card is located on the Board instead).

## Withdrawal and the badge

A notification is withdrawn when its cause is handled: a new turn starts
(*working*), you view the card (an unread ending becomes read), the state
changes to the other kind (the newer one replaces it), or the session
leaves the hook store (the agent exited).

The Dock badge is the number of cards in **needs input** plus cards with an
**unread turn ending** — the same rows as the Needs attention list minus
manual follow-up. It updates whether or not the window is focused, and it
is cleared and no longer updated when the switch is off. Viewing a card
that still needs input keeps it counted: the question is still open.

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
- Codex reports *turn done* when the model attempts to stop, so its ending
  can be slightly early (see the agent-status contract).

Contract and code: `app/src-tauri/src/notify.rs` (policy, tests),
`app/src-tauri/native/NotificationBridge.swift` (UNUserNotificationCenter
only, no-op outside a bundle; `scripts/test-notification-bridge`),
`app/ui/js/notify-model.js` (labels and viewed-dismissals), pinned by
`tests/edr_quiet.rs` and `tests/log_privacy.rs`.
