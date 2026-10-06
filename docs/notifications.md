# Away notifications and the Dock badge

deck can tell you when an agent asked for input or ended a turn while its
window is not in front, and count those cards on the Dock icon. Both are
off by default and both are derived from the same agent-status hook
words the Board already uses; nothing is inferred from output. The same
switch also tells you when a list's delivery has stopped to wait for you
(below); that part is Deck's own queue state and needs no agent
integration. The Dock count also includes cards whose
[reminder](card-reminders.md) is due, and that part does not depend on this
switch (see the last section).

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

### A delivery that waits for you

A list (the ⏱ panel) sends its rows by itself until one of them needs a
person. deck notifies you, in its own words, when a card's list reaches
such a row while you are away:

- a row was sent and waits for you to **check the result** (a list with
  inspection checkpoints);
- a delivery is **uncertain**: the text was pasted and the Enter was not
  confirmed;
- a row **stopped retrying** after repeated failures;
- the row whose turn it is carries **external content** that no automation
  approval covers, and waits for *Send now*;
- the first prompt for a freshly started agent waits for a **first agent
  interaction** (or *Send now*), because its automation did not accept
  sending without one;
- a row is held because **Codex Signal cannot be attributed** to that Codex
  process, and waits for *Send now*;
- a step you approved for automatic sending has been held for about a
  minute because deck **cannot verify the approval** (the settings or the
  Board could not be read as your current version — for instance after a
  damaged file was recovered from its backup), so automatic sending is
  paused. *Send now* still works.

These are the rows the Needs attention list shows as *Delivery waiting*.
A wait that passes by itself — a minimum gap, a quiet period, a time of
day, an earlier row — never notifies, and an agent's own input request is
announced as the agent's (above), not as a delivery. An approval deck could
not re-read for a moment is not announced either; it usually returns at the
next check, and only a hold that lasts is announced (the last item above).
That minute decides when you are told and nothing else: it is not a sign
that the agent is stuck, that the task failed or that anything was
approved, revoked or sent. A first step whose automation allows sending without readiness
is not waiting and is not announced. The last two kinds need no agent
integration to be announced, so a Codex session without attributable hooks
still tells you when its list has stopped.

It is information only: a notification never sends, retries or releases a
row. Each wait is announced once; the notification goes when that wait
ends. In a list with checkpoints every checked row is its own wait.

### A program rang the terminal bell

A session that reports no agent state — an ordinary shell, a long command,
an agent without the integration — can still call you: when a program in
it rings the terminal bell (`make test; printf '\a'`, or a tool set to
ring when it wants you) and nobody is looking, the card joins the Needs
attention list as *Bell* and, while you are away, a notification says *a
program rang the terminal bell*.

- "Nobody is looking" means: the deck window is not in front, or it is in
  front and no pane shows that card. A bell while you are at deck with the
  card's pane open is not announced — that is where your own keystrokes
  ring.
- One per card until you look: a program that keeps ringing costs one
  notice. Showing the card with deck in front marks it seen.
- Sessions with agent state are ignored: the integration says more.
- deck reads nothing else: not that a command ended or succeeded, not quiet
  time, not output. The phrase says only that the bell rang.
- Bells from before deck started are not reported. A bell is noticed
  within a few seconds while deck's window is on screen, and within about
  half a minute when macOS has put the window's timers to sleep (the
  window fully covered for a few minutes, for instance): deck's backend
  then looks for bells itself, so you do not have to come back to deck to
  be told.

**Away** means the deck window is not focused: hidden with ⌘W, behind
another app, or on another Space. A notification is posted at the moment
of the transition into one of the two states; a repeated report of the
same word posts nothing.

## What a notification contains

- Title: the card's title; one longer than 512 bytes is shown by its
  first whole characters.
- Body: the project name and one of four fixed phrases, following the
  interface language. Two say what the agent reported: *asked for your
  input* or *a turn has ended* (Chinese: 请求了你的输入 / 一轮已结束). One is
  deck's own: *a delivery is waiting* (有投递待处理). One passes on what a
  program did: *a program rang the terminal bell* (程序响了终端铃).

A card has one notification at a time. If more than one applies (an
agent's, a held delivery's, a bell's), the newest is shown; when one of
them ends, the others are not removed with it.

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
manual follow-up — plus cards with a **delivery waiting** (the kinds
above), plus cards whose reminder is due, each card counted
once. It updates whether or not the window is focused. When the switch is
off the agent rows and the waiting deliveries are no longer counted; a due
reminder still is. Viewing
a card with an input request keeps it counted until the agent reports
another word: viewing is not answering.

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

Codex cards without a reliable hook observation show **Agent status not
connected**, or **Agent status unavailable** when Deck has proved that events
cannot be attributed to that terminal. **Why?** explains the gap and opens
Agent status settings. Unknown does not diagnose a shared server: hooks may
be disabled, awaiting review in Codex `/hooks`, or simply not observed since
Deck restarted. The hint does not count as Needs attention, notify, or change
automatic-send holds. A failed poll marks this information as an old snapshot.

For a new interactive Codex session, `codex --no-daemon` is an optional hook
compatibility path, including when a shared server is already running. That
session does not use shared-server continuity. Review and trust the Deck
hooks in Codex `/hooks`; installed does not mean trusted. PermissionRequest
hooks do not cover every question or MCP elicitation, and a turn ending does
not prove task success. Deck never switches modes or approves hooks for you.

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

The terminal bell is read on the Board's poll, which the webview's timer
drives. When no poll has arrived for ten seconds the scheduler's own
thread makes the same bell observation on its 20-second tick, and hands
it back at the next poll (`bell.rs`, Cadence).

## Known limits

- Requires an installed `.app` bundle (Stable, Nightly, or `app/run.sh`).
  A bare binary or the unit-test process reports *unavailable* and posts
  nothing; the isolated smoke bundle is an app and can post.
- One notification per card at a time; the system identifier is the card's
  tmux session name.
- No history panel, no snooze, no per-project switch: one switch, two
  signals.
- Codex shared-daemon Signal can become Trusted only when events are
  verifiably bound to a specific interactive client and its current pane
  foreground generation. Inherited `TMUX_PANE`, cwd, timing, executable name,
  transcript path and session/thread/turn IDs are not pane ownership proof.
  The 0.157.1 and 0.160.0 (2026-10-02) certifications remain BLOCKED: the
  service runs every client's hooks with the environment of the client that
  started it. deck rejects those events (`terminal-discontinuity` in the
  log) rather than guessing, so such a card has no agent state, never
  notifies and never counts on the Dock, and no other card receives its
  events. The first such refusal also withdraws that Codex process's earlier
  status (the channel is now ambiguous), and no later hook of the same
  process restores it. Because Deck then cannot see a Codex permission
  prompt, lists hold every automatic send into that card (send-now still
  works). Installing the hooks does not by itself mean
  status works. Codex running without the shared service (embedded) has
  attribution topology that can satisfy the existing model. Current
  `CodexSignalTrust::Unavailable` and scheduler/readiness gates remain
  unchanged; a future release needs fresh client/pane ownership proof, not
  a version-based exemption. See [certification record](codex-signal-certification-20261002.md).
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

## Card reminders

[Card reminders](card-reminders.md) are saved attention intent independent of
Agent hooks and Notify when away. They use separate UTC system requests and
stable card/revision actions. The Dock deduplicates the union by card ID;
future reminders do not count. Agent viewing/withdrawal never handles a reminder
or removes its notification. See [ownership and verification](reminder-verification.md)
for persistence, platform limits and the current unattended acceptance boundary.
