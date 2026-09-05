# deck adoption validation

Status: local execution package prepared on 2026-08-30. External promotion and
Cloudflare publication have not started.

## Objective

Validate who adopts deck, why they keep using it, and which accurate message
helps the right people understand it. This phase does not test pricing or
commercial intent.

The decision at the end of the phase is one of:

1. expand promotion around one proven audience and value proposition;
2. revise the audience, message, onboarding, or product and run another cohort;
3. pause promotion until product or installation friction is resolved.

Commercialization is not the default next step.

## Product and privacy constraints

- deck remains MIT licensed, usable without an account, and distributed as a
  signed and notarized Apple Silicon DMG.
- Terminal prompts, commands, paths, session/card/project/repository names and
  output must never become analytics or research records.
- Quiet means no recent output. It does not mean finished, waiting for input,
  or ready for another prompt.
- Boards express user-controlled attention; cards do not move automatically.
- Scheduled prompts check target identity and optional foreground executable,
  but do not understand terminal content. deck must be running for delivery.
- The website initially contains no analytics, cookies, forms, remote fonts or
  third-party scripts.

## Initial ICP hypothesis

An individual developer who:

- uses an Apple Silicon Mac;
- manages at least five concurrent CLI agent or terminal sessions in a normal
  week;
- prefers a real local terminal to a hosted agent workspace;
- cares about session survival, project context and local privacy;
- currently switches among Terminal tabs/windows, tmux, IDE terminals, or a
  combination of them.

The five-session threshold is a hypothesis, not a permanent eligibility rule.
Record evidence from adjacent users without silently changing the primary
cohort.

## Value hypotheses

| Priority | Hypothesis | Evidence that supports it | Evidence against it |
| --- | --- | --- | --- |
| 1 | Activity/quiet/stopped overview reduces repeated pane checking | User names a concrete avoided check or missed session | Status colors are ignored or misunderstood |
| 2 | tmux-backed survival reduces fear of closing the UI | User intentionally quits/reopens and relies on survival | Existing tmux workflow already solves it with no added value |
| 3 | Project Boards reduce context switching | User organizes real work across projects for at least a week | Boards feel like duplicate project management |
| 4 | Split view improves comparison and supervision | User repeatedly opens two or more sessions together | User returns to external terminal windows |
| 5 | Scheduler is valuable to a bounded advanced segment | User creates and later manages real scheduled work | Safety boundaries or app-running requirement make it unusable |

Scheduler usage is not a universal activation requirement.

## Measurement model

The funnel has three layers that are not joined at user level by default.

### Acquisition

| Event | Purpose | Initial source | Allowed fields | Prohibited fields | Retention/deletion |
| --- | --- | --- | --- | --- | --- |
| Landing visit | Check reach | Unavailable until privacy-reviewed aggregate analytics is enabled | Daily aggregate, route, locale | IP, fingerprint, query text, stable visitor ID | If enabled later: document provider retention and deletion |
| Stable download click | Check message-to-install intent | Initially unavailable; link goes to GitHub latest Release | Daily aggregate and locale only | Stable visitor ID, referrer containing free text | Same as above |
| Release asset download | Check distribution volume | GitHub asset `download_count` | Asset name, aggregate count, observation date | Downloader identity | Store weekly aggregate in experiment notes |
| GitHub visit | Weak interest signal | GitHub public aggregates if available | Aggregate count | Identity or cross-site join | Store only if available |

Acquisition metrics are reach signals, not adoption or success.

### Product adoption

There is no outbound product telemetry in the initial phase. Collect the
following through a voluntary D3/D10 follow-up or user-shared local observation.
Do not request screenshots or raw data files.

| Milestone | Closed answer | Definition |
| --- | --- | --- |
| First launch | yes/no | App opened successfully after DMG installation |
| Activated | yes/no | At least two sessions created within ten minutes |
| Core use | yes/no | At least five sessions existed concurrently during week one |
| Split used | yes/no | User opened at least two sessions in deck split view |
| Scheduler used | yes/no | User created at least one scheduled prompt |
| D7 active | yes/no | On day 7 ±2 days, user created/opened a session, used split, moved a Board card, or managed scheduler |
| D30 active | yes/no | Same definition on day 30 ±5 days |

Do not count app launch, update checks or opening Settings as active use.

### Qualitative learning

For each interview, record only:

- cohort code assigned in the research notes;
- session-count bucket: `1-2`, `3-4`, `5-9`, `10+`;
- primary CLI category: `coding-agent`, `shell-tooling`, `mixed`, `other`;
- prior workflow category: `terminal-tabs`, `tmux`, `ide-terminal`, `mixed`;
- milestone yes/no answers above;
- primary value enum: `survival`, `signals`, `boards`, `split`, `scheduler`, `other`;
- friction enum: `install`, `understanding`, `terminal`, `organization`,
  `scheduler`, `reliability`, `missing-platform`, `other`;
- short researcher-written synthesis that contains no names, paths, project
  details, commands, prompts or terminal excerpts.

Contact details used for scheduling must live in the chosen scheduling tool,
not in this repository or product analytics.

## Initial decision thresholds

These thresholds are pre-registered hypotheses and may be revised only between
cohorts, with the reason recorded.

- Minimum evaluable cohort: 20 qualified installers, with at least 12 D7
  responses and 8 interviews.
- Message comprehension: at least 80% of interviewed users correctly describe
  deck as a local control surface for persistent real terminal sessions.
- Activation: at least 60% of D7 respondents created two sessions within ten
  minutes.
- Core use: at least 40% of activated respondents reached five concurrent
  sessions in week one.
- D7 retention: at least 50% of activated respondents report an actual session
  operation in the D7 window.
- Repeated value: at least half of retained interviewees independently name the
  same top one or two value categories and give a concrete workflow outcome.

Small samples are directional. Report numerator and denominator every time;
never present a percentage without both.

## Cohort protocol

1. Freeze one landing headline, one release and one primary channel for the
   cohort.
2. Record start/end date, channel, message variant and Stable version.
3. Recruit only through the approved post; do not silently scrape or contact
   people.
4. Ask volunteers for D3 and D10 follow-up consent separately.
5. Run the interview guide from `docs/promotion-kit.md`.
6. Store only the closed research fields above plus content-safe synthesis.
7. Do not change more than one of headline, demonstration order or channel in
   the next cohort.
8. Record failures and non-response. Do not remove them from the denominator
   after seeing the result.

## 30/60/90-day gates

### Day 30 — positioning and entry

Required evidence:

- landing page, download path, privacy statement and feedback form reviewed;
- first qualified installers and interviews recruited;
- status, Board and scheduler boundaries understood;
- installation friction categorized.

Decision: keep the message, change one message/channel variable, or fix the
entry experience before acquiring more traffic.

### Day 60 — adoption and retention

Required evidence:

- activation, five-session core use and D7 results with denominators;
- repeated use cases and alternatives from interviews;
- one primary ICP and one primary value proposition, or an explicit conclusion
  that evidence does not converge.

Decision: expand a converged cohort, revise product/message, or pause.

### Day 90 — promotion decision

Required evidence:

- at least one complete cohort and any deliberate follow-up cohort;
- D30 directional evidence where available;
- a list of product changes supported by repeated evidence rather than one-off
  requests;
- an explicit decision owner and next review date.

Decision: expand promotion, run a revised adoption experiment, or stop active
promotion. Commercial research requires a separate proposal.

## Result template

```text
Cohort:
Dates / Stable version / channel / headline:
Qualified installers:
D7 respondents:
Interviews:
Activation: numerator / denominator
Core use: numerator / denominator
D7 active: numerator / denominator
Primary value counts:
Primary friction counts:
What users understood correctly:
What users misunderstood:
Decision:
Single variable for next cohort:
```
