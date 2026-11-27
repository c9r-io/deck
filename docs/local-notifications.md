# Local notifications

Any program of your macOS user on this Mac can hand Deck a notification with
the bundled `deck-notify` command. Deck stores it, shows it under
**Notifications** beside your Slack mentions, and routes it through your
automations — by your click, or automatically by a rule you saved. The program
owns its platform (credentials, polling, catch-up, conversion); Deck owns
receiving, showing, attention and routing; the Agent the run starts owns the
work. A new producer needs no change to Deck.

Contract owners: `app/src-tauri/src/local_notify.rs` (inbox, socket, claims),
`app/src-tauri/notify-cli/src/main.rs` (the CLI), `app/ui/js/notices.js`
(the page), `scheduler/authority.rs` / `scheduler/first_send.rs` (the
`local` trigger).

## What you can do when it is set up

- **Send from a script.** `deck-notify publish < n.json` returns one JSON
  receipt once Deck has stored the notification; the page shows it within a
  second, whether or not any rule matches.
- **Retry safely.** Sending the same `event_id` again with the same content
  answers `duplicate` with the original receipt; no second row, card or queue
  item. Same id, other content: `CONFLICT`.
- **Update in place.** Several events with one `notification_key` update one
  row; `revision` keeps a late old event from overwriting a newer state.
- **Resolve or withdraw.** The row moves to *Settled*; a card already started
  keeps running and is never moved or closed.
- **Ask what happened.** `deck-notify status --producer P --event-id E`
  reports the stored receipt and how the notification was routed (rule, card,
  outcome).
- **Route by hand.** In Notifications, pick a local notification rule in the
  row's Route menu (rules whose match fits are marked; any enabled local rule
  may be chosen, as a Slack mention may go to any badge rule); Deck creates
  the card and queues the template exactly as for a Slack badge.
- **Route automatically.** A local notification rule with *Handle matching
  local notifications automatically* takes each matching notification
  received after you saved it — one card per notification, once. With the
  rule's approval ticked, later template steps continue without Send now;
  with its first-step option, the first step goes to a freshly started Claude
  or `codex --no-daemon` without waiting for interaction evidence.
- **Dismiss.** Dismissed rows leave the count; an update keeps them dismissed
  (marked *updated*); a new occurrence after a resolve opens them again.

## What it does not do

- It does not prove who sent a notification. "Local" means a process of the
  same OS user on this Mac (an SSH session of that user included); `producer`
  is a name the program chose. Treat body text as untrusted input.
- It never answers the producer or the platform, and never syncs state back.
- It never starts work from a notification received before a rule was
  enabled; there is no back-fill.
- It does not deliver into an existing card, interject into a running Agent,
  collect several notifications into one card, or run third-party plug-ins.
- There is no remote access: no TCP/HTTP listener, nothing over the Secure
  Tunnel, not an MCP tool. Deck must be running; there is no daemon.
- Sandboxed apps: a Mac App Store-style sandbox normally blocks both running
  an executable inside another app and connecting to `~/.deck/notify.sock`.
  Supported callers are ordinary scripts, CLIs, launch agents of your own and
  unsandboxed apps.
- Dedupe is not forever (see Retention) and delivery is not end-to-end
  exactly-once.

## Turning it on and finding the CLI

1. Open **Notifications** in Deck's sidebar and tick **Accept local
   notifications from programs on this Mac** (stored as
   `inbound.sources.local.enabled` in settings.json; off = absent).
2. The page shows the CLI's path; **Copy path** copies it. In a release
   install it is `/Applications/deck.app/Contents/MacOS/deck-notify`. It is
   signed and updated with the app; nothing is installed on `PATH` — call it by
   path or add your own alias.
3. While the switch is off, or Deck is not running, the socket does not exist
   and the CLI exits 5 (`UNAVAILABLE`).

## Commands

```sh
deck-notify publish [--json-stdin] < notification.json
deck-notify status --producer <producer> --event-id <event_id>
deck-notify socket          # prints the socket it would use
deck-notify --version
```

Options: `--socket <abs path>/notify.sock` (or `DECK_NOTIFY_SOCKET`) to talk
to another Deck data directory (an isolated test instance); `--timeout <1-60>`
seconds (default 10). stdin is parsed as JSON (no shell evaluation);
for `publish`, `status`, `socket` and `--version`, **stdout** is exactly one
JSON line (also on errors); **stderr** a one-line human diagnostic. `help`
prints the usage to stderr only.

## The notification (version 1)

```json
{
  "version": 1,
  "producer": "ci-monitor",
  "event_id": "build-1842-failed",
  "notification_key": "build-1842",
  "operation": "upsert",
  "revision": 1,
  "type": "build.failed",
  "title": "Deck build failed",
  "body": "Build 1842 failed in the test stage.",
  "url": "https://ci.example.com/builds/1842",
  "labels": { "project": "deck", "environment": "development" },
  "occurred_at": 1795800000
}
```

| Field | Required | Default | Rule |
|---|---|---|---|
| `version` | yes | — | integer `1`; another integer → `UNSUPPORTED_VERSION` |
| `producer` | yes | — | 1–64 of `a-z 0-9 . _ -`, starting with a letter or digit; name each account or instance apart (`ci-main`, `ci-staging`) |
| `event_id` | yes | — | 1–128 of `A-Z a-z 0-9 . _ - : @ / +`; one per change; keep it on retries |
| `notification_key` | no | `event_id` | same alphabet; the row several events update |
| `operation` | no | `upsert` | `upsert`, `resolve`, `withdraw` |
| `revision` | no | `0` | integer ≤ 2^53 (see Order) |
| `type` | no | `notification` | like `producer`; e.g. `build.failed` |
| `title` | upsert | — | one line, ≤ 512 bytes after normalization |
| `body` | no | `""` | multi-line, ≤ 16 KiB |
| `url` | no | `""` | `http(s)://…`, ≤ 2048 bytes, no spaces |
| `labels` | no | `{}` | ≤ 16; names like `type`, values one line of 1–256 bytes |
| `occurred_at` | no | — | epoch seconds; shown only, never used for order |

`resolve` and `withdraw` carry identity, `revision` and `occurred_at` only.
Any other field is refused (`INVALID_INPUT`): there is no way to name a
project, command, template, card, session, callback or approval — what
happens is decided by your rules in Deck. Labels may be matched by a rule;
they never grant anything and are not part of what a template sends.

**Normalization** happens before anything is compared: invisible formatting
and bidi controls are removed, control characters dropped (the body keeps
newlines and tabs; the title and label values become one line), text is
trimmed. Limits apply to the normalized text and refuse rather than truncate.
The request line, including the wrapper, is at most 64 KiB.

## Receipts, errors, exit codes

```json
{"protocol":1,"status":"accepted","producer":"ci-monitor","event_id":"build-1842-failed",
 "notification_key":"build-1842","received_at":1795800003,"applied":true,
 "notification":{"external":"active","attention":"open","revision":1,"occurrence":1}}
```

- `accepted` — stored durably (atomic write of the inbox) **before** the
  answer. It does not mean shown, routed, received by an Agent or done.
  `applied: false` with `reason` `stale-revision` or `unknown-notification`
  means it was recorded but changed nothing.
- `duplicate` — this exact event was already stored; the original receipt.
- `found` / `not-found` — answers to `status`. `status` adds `routing`
  (`by` auto/manual, `rule`, `state` pending/done/skipped, `card`, `reason`).
- Errors: `{"status":"error","code":…,"message":…,"retryable":…}`.

| Exit | Codes | What to do |
|---|---|---|
| 0 | accepted, duplicate, found | done |
| 1 | not-found | nothing stored under that id |
| 2 | `USAGE` | fix the command line |
| 3 | `INVALID_INPUT`, `UNSUPPORTED_VERSION`, `TOO_LARGE` | fix the input; do not retry unchanged |
| 4 | `CONFLICT` | this `event_id` was used for other content; use a new id |
| 5 | `UNAVAILABLE`, `DISABLED`, `CAPACITY`, `RATE_LIMITED`, `BUSY`, `INTERNAL` | retry later with the **same** `event_id` |
| 6 | `NO_RECEIPT` | it may have been stored: retry with the same `event_id`, or ask `status` |
| 7 | `UNSUPPORTED_PROTOCOL` | use the `deck-notify` inside the running Deck.app |

## Identity, order, lifecycle

- **Event** `(producer, event_id)`: idempotency. The fingerprint is the
  SHA-256 of the normalized event, so `"  Build failed "` and `"Build failed"`
  are the same content. Every field counts, `occurred_at` included: a retry
  must resend the same event, so take times from the change itself, never
  from "now" at send time (the reference adapter's first version did, and its
  retry one second later was a `CONFLICT`).
- **Notification** `(producer, notification_key)`: the row.
- **Order**: an event applies to an existing row only when its `revision` is
  greater than the row's, or both are 0 (arrival order, for producers that do
  not version). Once a row has a revision, unversioned events no longer
  change it.
- **External state** (from the producer): `active` → `resolved` /
  `withdrawn`. An `upsert` after a resolve or withdraw starts the next
  **occurrence**: the row is open again and a rule may take it again.
- **Attention** (Deck's): `open`, `dismissed`, `routed`. Only open + active
  rows are counted.

## Routing and its authority

- A **local notification rule** (Automations drawer → *Local notification*)
  names a producer and optional types and labels. A value ending in `*`
  matches by prefix; `*` alone matches anything.
- **One claim per occurrence.** A click and a rule, or two rules, cannot both
  take one occurrence: the backend claims it atomically, under the inbox lock,
  after checking that the occurrence (and, for a click, the revision you saw)
  is current. The claim freezes the notification's content and the rule's
  revision (rule, template steps, approval). Later updates never rewrite a
  queued run.
- **Automatic**: the first enabled rule, in the order of the rules list, whose
  match fits and whose *Handle … automatically* is on, for an active, open
  notification whose occurrence began after the rule was saved with that
  match. Each occurrence is tried automatically once.
- From the claim on it is the Slack badge path: one card (`origin.source:
  "local"`, the claim id as key), the template's steps queued as **external**
  rows (an Agent command, no line led by a placeholder), the head row held by
  the first-interaction gate unless the rule's first-step option is on, and
  follow-ups held for Send now unless the rule's approval is ticked and still
  valid. Placeholders: `{{msg.text}}` = title and body, `{{msg.from}}` =
  producer, `{{msg.where}}` = type, `{{msg.link}}` = url; each value is fenced
  as `<external_message>` material.
- The approval is the rule's own (`TriggerClass::Local`): it covers the rule,
  its match (including `auto`: what the rule takes by itself) and its
  template's steps, and editing any of them voids it. Your Route click on one
  notification is your own act on it, whatever the rule's match says.
- Rule label values are compared exactly after normalization (one line,
  whitespace runs as one space); a value typed in the editor cannot contain a
  comma. Unticking, deleting the rule or a damaged settings file stops
  automatic sending at the pre-fire fence; rows keep their text for Send now.
- A claim that cannot become a card (project, column or template gone, the
  command refused, the rule changed since the claim) is **skipped** with its
  reason and the row is open again; it is not retried automatically.

## Recovery, retention, capacity

| Window | What happens |
|---|---|
| Stored, receipt lost | retry with the same id → `duplicate` |
| Stored, Deck quits before routing | the next poll (≤30 s after start) claims it |
| Claimed, card not created | the claim is offered again after restart; the same key is used |
| Card created, ack not recorded | the card's origin is found → the claim is acknowledged; no second card |
| Card deleted afterwards | the claim stays `done`; nothing is revived |

- Inbox: `~/.deck/notifications.json` (0600, atomic writes, file version 1).
- Event records are kept while their notification exists (the last 256 per
  notification) and 30 days after it leaves; that is the dedupe window.
- A notification leaves 30 days after its last change once it is settled
  (dismissed, routed or resolved) and has no pending claim. Open, active
  notifications are never evicted: at 1000 notifications or 20 000 event
  records, publishing answers `CAPACITY` and writes nothing.
- At most 600 accepted publishes per minute, 8 connections at once, 5 s per
  request.

## Compatibility and diagnosis

- settings.json with a local rule or the switch, and queue.json with a row
  admitted under the `local` trigger, take sticky schema **v14**; older Deck
  builds refuse them untouched instead of misreading them.
- Protocol: the CLI and the app speak wire protocol 1; a mismatch is exit 7.
- app.log lines `[local] …` carry counts and closed codes only, never text,
  producer names or ids. `deck-notify socket` prints the path the CLI uses.

## Reference adapter

`tools/local-notify-example/ci_watch.py` plays a CI watcher with no account:

```sh
export DECK_NOTIFY=/Applications/deck.app/Contents/MacOS/deck-notify
tools/local-notify-example/ci_watch.py fail 1842            # accepted
tools/local-notify-example/ci_watch.py fail 1842            # duplicate (retry)
tools/local-notify-example/ci_watch.py fail 1842 --attempt 2 --title "Build 1842 failed again"   # updates the row
tools/local-notify-example/ci_watch.py status 1842 --attempt 2   # receipt + routing
tools/local-notify-example/ci_watch.py pass 1842 --attempt 3     # resolves
tools/local-notify-example/ci_watch.py fail 1842 --attempt 4     # next occurrence
```

A one-off from the shell:

```sh
printf '%s' '{"version":1,"producer":"backup","event_id":"nightly-2026-11-28","title":"Nightly backup finished"}' \
  | "$DECK_NOTIFY" publish
```
