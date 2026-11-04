// follow-up-model.js — preset follow-ups on a card: pure helpers.
// Part of deck's no-build frontend: native ES modules, no bundler.
//
// # Contract
// - A template's follow-ups (`followUps`, pure.js `templateFollowUps`) are
//   the owner's own fixed replies. A badge, clock, Agent or channel run
//   freezes them into its plan when it opens a card (`inboundPlan` /
//   `channelRun`), so a later template edit never changes an open card and
//   a card no automation opened has none.
// - The card shows them as buttons, in its pane and on its Needs attention
//   row (follow-up-bar.js). A click is the user's own act: Board's
//   `queueFollowUp` puts that text in the card's queue through the owner
//   command and wakes the scheduler, whose holds decide when it is sent.
//   Nothing here reads an agent word, and no follow-up is ever sent without
//   a click (tests/signal_census.rs, tests/external_admission.rs).
// - With turn summaries on, the model may name the follow-up that fits the
//   turn's final message (`suggestion`): it only marks a button. The index
//   must name one of this card's follow-ups and the reason is shown as text.
import { FOLLOWUPS_MAX } from './pure.js';

/* The follow-ups frozen on `card`, in order. */
export function cardFollowUps(card) {
  const list = card?.inboundPlan?.followUps ?? card?.channelRun?.followUps;
  return Array.isArray(list)
    ? list.filter(text => typeof text === 'string' && text).slice(0, FOLLOWUPS_MAX)
    : [];
}

/* Whether `text` already waits in the queue of `session`: its button says
   so instead of queueing it a second time. */
export function followUpQueued(items, session, text) {
  return (items || []).some(item => item.session === session && item.text === text);
}

/* The model's pick for the turn ending the snapshot shows: `{index, reason}`
   (index from 0) when it names one of `count` follow-ups, else null. A
   stale or ended observation shows none, like its summary. */
export function followUpSuggestion(snapshot, count) {
  const choice = snapshot?.followChoice;
  if (!snapshot || snapshot.stale || !snapshot.alive) return null;
  if (!Number.isInteger(choice) || choice < 1 || choice > count) return null;
  const reason = typeof snapshot.followReason === 'string' ? snapshot.followReason : '';
  return { index: choice - 1, reason };
}
