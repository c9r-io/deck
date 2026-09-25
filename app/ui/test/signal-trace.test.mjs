// Signal Trace harness, frontend half (FR-SI-05). The same trace file the
// Rust runner (src-tauri/src/signal_trace.rs) replays against the real
// backend, and — as its only input about the backend — the golden
// projection stream that runner produced and checks: exactly what
// poll_sessions returned at every poll, normalized (episode labels, no
// paths or content). This file drives the REAL frontend derivations with it
// — attention-model (categories, badge, Needs Attention rows, freshness),
// notify-model (the exact dismissals), runFinishHolds — and checks the
// same `expect` steps. Keys both runners check (`agent`, `unread`, `dock`,
// `pending`) are the cross-layer agreement: the Board, Needs Attention and
// the Dock/notification state cannot disagree while both suites pass.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { attentionBadge, attentionRows, createAttentionTracker } from '../js/attention-model.js';
import { NOTIFY_COUNTED_FILTERS, seenDismissals } from '../js/notify-model.js';
import { runFinishHolds } from '../js/pure.js';

const read = name => JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'));
const { traces } = read('signal-traces.json');
const golden = new Map(read('signal-trace-projection.json').traces.map(t => [t.name, t.records]));
const SHELL = /^-?(zsh|bash|fish|sh|dash)$/;
const name = label => `deck-trace-${label}`;
const episodeNumber = label => (label == null ? null : Number(label.slice(1)));

function replay(trace) {
  const records = new Map((golden.get(trace.name) || []).map(r => [r.step, r]));
  const cards = trace.sessions.map(label => ({ id: label, session: name(label), projectId: 'P', columnId: 'C' }));
  const byLabel = new Map(cards.map(c => [c.id, c]));
  const projects = [{ id: 'P', name: 'P', columns: [{ id: 'C', name: 'C' }] }];
  let tracker = createAttentionTracker();
  const inflight = new Set();
  const lastInfo = new Map();
  let now = 1000;

  const dismiss = (step, subset, at) => {
    const due = seenDismissals(subset, tracker, inflight);
    const declared = step.dismiss.map(([session, episode]) => [session, episode ?? lastInfo.get(session)?.episodeLabel]);
    assert.deepEqual(due.map(d => [d.card.id, `e${d.episode}`]), declared, `${at}: the exact dismissals the webview sends`);
    if (step.transport !== 'fail') for (const d of due) tracker.confirmViewed(d.card, d.episode);
  };

  trace.steps.forEach((step, i) => {
    const at = `${trace.name} step ${i}: ${JSON.stringify(step)}`;
    if (step.poll !== undefined) {
      if (step.poll === 'fail') { tracker.fail(); return; }
      const record = records.get(i);
      assert.ok(record, `${at}: golden record`);
      const omit = new Set(record.omit || []);
      const infos = [];
      for (const [label, s] of Object.entries(record.sessions)) {
        const info = {
          name: name(label), alive: s.alive, agent: s.agent, idle_secs: 0,
          episode: episodeNumber(s.episode), episode_viewed: s.episode_viewed,
          finish_fg: s.finish === 'shell' ? 'zsh' : s.finish === 'agent' ? 'claude' : null,
        };
        lastInfo.set(label, { ...info, episodeLabel: s.episode });
        if (!omit.has(label)) infos.push(info);
      }
      tracker.record(cards, infos, new Set(), now += 1000);
    } else if (step.recreate) {
      tracker = createAttentionTracker();
      inflight.clear();
    } else if (step.view) {
      const card = byLabel.get(step.view);
      tracker.saw(card);
      dismiss(step, [card], at);
    } else if (step.sync) {
      dismiss(step, cards, at);
    } else if (step.expect) {
      const { sessions = {}, dock, pending } = step.expect;
      for (const [label, want] of Object.entries(sessions)) {
        const card = byLabel.get(label);
        const snapshot = tracker.get(card);
        const category = tracker.category(card);
        if ('agent' in want) assert.equal(snapshot?.agent ?? null, want.agent, `${at}: ${label} agent`);
        // `local`: the one sanctioned, transient divergence — the webview
        // shows its own view before notify_dismiss is acknowledged (a
        // failed call is retried); the backend stays unread until then
        if ('unread' in want && !want.local) {
          assert.equal(category === 'done', want.unread, `${at}: ${label} unread (frontend)`);
        }
        if ('category' in want) assert.equal(category, want.category, `${at}: ${label} category`);
        if ('badge' in want) assert.equal(attentionBadge(tracker, card)?.kind ?? null, want.badge, `${at}: ${label} badge`);
        if ('stale' in want) assert.equal(snapshot?.stale === true, want.stale, `${at}: ${label} stale`);
        if ('finish' in want) {
          const info = lastInfo.get(label);
          const eligible = runFinishHolds({
            rule: { finish: 'close' }, queued: false, agent: snapshot?.agent || undefined,
            fg: info?.finish_fg ?? undefined, alive: snapshot?.alive === true, stopped: false, viewing: false,
          }, SHELL);
          assert.equal(eligible, want.finish, `${at}: ${label} retirement eligibility`);
        }
      }
      if (dock !== undefined) {
        const counted = cards.filter(c => NOTIFY_COUNTED_FILTERS.includes(tracker.category(c))).length;
        assert.equal(counted, dock, `${at}: Dock count (frontend view of the same set)`);
      }
      if (pending !== undefined) {
        const rows = attentionRows(projects, cards, tracker, 'pending').map(r => r.card.id).sort();
        assert.deepEqual(rows, [...pending].sort(), `${at}: Needs Attention`);
      }
    }
  });
}

test('every Signal trace holds across the frontend on the backend\'s own projection stream', () => {
  assert.ok(traces.length >= 17);
  assert.deepEqual([...golden.keys()], traces.map(t => t.name), 'the golden covers exactly the traces');
  for (const trace of traces) replay(trace);
});

test('the golden stream is Signal-only and content-free', () => {
  const text = JSON.stringify([...golden.values()]);
  assert.doesNotMatch(text, /\/|Users|zsh|claude|deck-trace/, 'no paths, commands or session names');
  for (const records of golden.values()) {
    for (const record of records) {
      for (const s of Object.values(record.sessions || {})) {
        assert.deepEqual(Object.keys(s).sort(), ['agent', 'alive', 'episode', 'episode_viewed', 'finish']);
        assert.ok(s.episode === null || /^e\d+$/.test(s.episode));
      }
    }
  }
});
