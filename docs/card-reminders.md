# Card reminders

A reminder means “bring me back to this card at this time.” It is independent
of Automation, Agent state, starred cards, task completion and process life.
It never sends input, starts a session, moves a card or keeps a process alive.

Use **Set reminder…** in a card's context menu or the Session header. A saved
reminder also has a clickable date label on the Board, after the session's
name in the sidebar and at the right of the card's status line: the time for
a reminder due today, month/day otherwise (the card adds the time; the
sidebar, where the session's name comes first, does not), **Due** once it
is. The full date, weekday and time zone are its tooltip. Clicking it opens
the editor and nothing else. The editor shows the
full date, weekday and original time zone before saving. Shortcuts fill an
hour from the action, tomorrow at 09:00, or Monday in the next natural week.
Repeated local times require a UTC occurrence choice; nonexistent and past
times are refused. Notes are one line, at most 280 UTF-8 bytes. Use the existing
scratchpad for longer material. Notes do not enter notifications or logs.

Saving is intent, not confirmation of system registration or visible delivery.
The editor separately shows authorization and the last observed registration
result. A registration error preserves the reminder and its deletion protection.
If notifications are denied or unavailable, explicitly choose **Remind only
inside Deck**. Reminder intent has no global enable switch. Agent **Notify
when away** does not disable reminders. Sound uses the existing sound setting.
Permission is requested only from a user's reminder configuration action.

At the chosen UTC instant, the system request may display a notification even
while Deck is in front or has quit. Notification content is a card title,
project and fixed phrase only. Open locates the current stable card, attaching
only to a positively observed live session; stopped cards remain stopped.
The system Snooze action sets a new version exactly one hour from the actual
response time. In Deck, use the date label to remind later or explicitly end
the reminder. Viewing a card or dismissing a system banner does not end it.

When Deck is not running, the system starts it to deliver a Snooze. That
launch does that one thing: Deck saves the new time, waits until the system
has confirmed the new request, and exits again. No window appears, the
application you were using keeps the keyboard, and nothing else Deck would
do on its own (lists, automations, Slack, the Connector) starts. If the
answer cannot be saved or registered, or you ask for Deck in the meantime
(the Dock icon, or a click on a banner), Deck opens normally instead and
shows what happened. A Snooze answered while Deck is running changes
nothing about its window.

Due reminders remain visible on the card, Session entry and Needs attention.
**Due reminders** filters due intent; **All reminders** includes future intent.
These sort a derived list, never the Board. Agent input requests and unread
turn endings are independent reasons. Dock counts their card-ID union: Agent
reasons count only when Notify when away is enabled; due reminders count
independently, including plain shells without Agent hooks. Future reminders
are not pending attention. A durable due latch survives clock rollback.

Future and due reminders protect a card from automatic shell retirement and
Automation finish=close. Manual close names the reminder and requires explicit
“Cancel reminder and close”; project deletion requires current-version
confirmation for every protected card. Non-cancelling/remote close is refused.
Existing batch/maintenance close callers cannot silently remove protection.
Ending a reminder does not release an earlier blocked retirement: the exact
shell generation or automation run stays retained. A genuinely new shell
lifecycle can still retire normally. This protection does not keep a shell alive.

The system controls display timing, focus modes, sound and authorization.
Registration does not guarantee a visible banner during sleep, lock, shutdown
or restricted notification settings. Recovery shows missed reminders inside
Deck without re-posting past notifications. Cross-process crashes cannot offer
absolute exactly-once visible presentation. Platform acceptance is reported
separately in [the verification guide](reminder-verification.md).
