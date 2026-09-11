# Arrange follow-up input

Put later prompts for the same session into a list so deck remembers the timing and order. Where a result needs your judgment, choose to inspect every row before continuing.

## Create a list

1. Open a session and click **⏱** in its header.
2. Create a list, enter the first prompt, and add subsequent rows from that list's footer.
3. Set an optional **not before** date and time and the quiet conditions for subsequent rows.
4. Read the execution plan to check the session, next row and waiting conditions.

The first row is time-based; later rows wait for their configured quiet interval. A past “not before” time is refused, never rolled to tomorrow. Each session receives one prompt at a time, at least one minute apart. Different sessions deliver independently.

> Quiet means no output. It cannot confirm readiness, success of the previous task or that a permission menu has closed.

For example, first ask an agent to investigate failing tests, then propose a fix. If the second prompt requires your acceptance of the analysis, enable human inspection below instead of relying on a quiet interval as a dependency.

## Inspect before continuing

Explicitly enable **Inspect every row before continuing**, including the last row, on the list. Existing lists remain unchanged by default.

Every delivered row leaves a human checkpoint. Read the result in the terminal, then choose **Inspected, allow next…**. Check the next prompt and target shown in the dialog before confirming. Permission applies only to that displayed revision and target. The next row must still satisfy its schedule, quiet condition and minimum gap.

- Opening a pane, a turn-ending report, quiet or a permission wait does not count as inspection.
- Editing the next row, retrying or changing the target revokes unused permission.
- The last row also needs inspection. Confirming it only releases the final hold; it does not itself end a process.
- Skipping omits work. Cancelling does not count as inspection either.

Checkpoints persist across app restarts. The setting governs its own list; **other lists in the same session may still interleave deliveries**. Their arrangement does not establish work dependencies.

## Repeat and reuse

**Repeat** sends the whole list again into the same session at an interval from 5 minutes to 4 hours. You can limit it to a daily window and choose a count or end time. Windows may cross midnight, such as 20:00–08:00. Pause retains the settings. Repeat counts measure deliveries, not successful tasks.

With inspection enabled, every repetition needs fresh inspection. To create a new session for each run, use [automations](/guide/automations/) instead of a repeating list.

A list's **📋** menu can save it as a template or insert one. Insertion makes a copy; later template edits do not change existing rows.

## When a list does not continue

| Panel state | Next action |
| --- | --- |
| Waiting for time, quiet or the minimum gap | Read the remaining conditions in the execution plan and keep deck running |
| Waiting for inspection | Read the result and explicitly confirm the checkpoint; opening the terminal is not enough |
| Waiting for a program or context | Inspect the card's terminal and return the expected program to the foreground; avoid blind input into a shell or permission menu |
| Delivery failed | Resolve the cause and retry, or explicitly skip; later rows do not pass the failed row |
| Delivery uncertain | Check the terminal, then acknowledge as sent or explicitly retry accepting a possible duplicate. deck does not auto-resend |
| Sending; changes temporarily refused | Wait for the delivery to finish before editing, pausing or removing it |

Immediately before delivery, deck rechecks the card's terminal and any identifiable foreground program. When a launch command identifies the expected program, deck waits for it to return to the foreground. Otherwise it uses compatibility mode in the same terminal, where a shell may interpret the input; inspect the target before scheduling. A target change blocks that mismatched delivery; a later attempt can recheck a replacement session generation.

## Before you leave

**The app must be running** for lists to deliver, but the target pane may be closed. The execution plan, delivery records and agent observations are separate. A stale plan is not a live countdown. Delivery and inspection histories each keep up to 200 records of identifiers and times; a pending checkpoint still retains the sent text you need to inspect.

First use of human inspection upgrades relevant local data formats. Turning it off does not downgrade them. Read [updates and versions](/guide/input-and-settings/#updates-and-versions) before returning to an older build.
