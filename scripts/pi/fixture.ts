// fixture.ts — TEST ONLY. Drives a real interactive Pi without a model
// account: a deterministic local provider and a few commands that make Pi
// itself retry, queue a follow-up, continue after a settlement, run one of
// its own tools, raise UI prompts and replace its session. It is loaded with `-e` by
// `scripts/pi_native.py` inside an experiment root and is never packaged:
// Deck's observer (`app/src-tauri/resources/pi/`) uses none of these APIs.
//
// It writes one line per SOURCE event to `FIXTURE_LOG` (a private file in the
// experiment root): the event name, Pi's reason words and the native session
// id. The id is there so a run can compare what Pi said with what Deck
// observed; a log that leaves the experiment root is made anonymous first.
import fs from "node:fs";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";

export default function (pi) {
  const log = process.env.FIXTURE_LOG;
  let order = 0;
  let failNext = 0;
  let slowNext = 0;
  let toolNext = null;
  let followUp = false;
  let afterSettle = false;
  let continueOnce = false;
  const record = (event, ctx, extra = {}) => {
    if (!log) return;
    let id = null;
    try { id = ctx ? ctx.sessionManager.getSessionId() : null; } catch (_) { id = null; }
    fs.appendFileSync(log, `${JSON.stringify({ event, order: ++order, pid: process.pid, id, t: Date.now(), ...extra })}\n`);
  };

  for (const name of ["session_start", "session_shutdown", "agent_start", "agent_end", "agent_settled", "ui_prompt_start", "ui_prompt_end"]) {
    pi.on(name, (event, ctx) => record(name, ctx, { reason: event.reason, kind: event.kind, aborted: event.aborted }));
  }

  pi.registerProvider("deck-fixture", {
    baseUrl: "http://127.0.0.1:1",
    apiKey: "not-a-secret",
    api: "deck-fixture-api",
    models: [{
      id: "fixture", name: "Local fixture", reasoning: false, input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 1000,
    }],
    streamSimple(model, _context, options) {
      record("fixture_provider_request", null);
      const stream = createAssistantMessageEventStream();
      const failed = failNext > 0;
      if (failed) failNext -= 1;
      const wait = slowNext || 100;
      slowNext = 0;
      // one call of Pi's own shell tool, when a command asked for one: Pi runs it and asks again
      const tool = failed ? null : toolNext;
      if (tool) toolNext = null;
      const answer = () => {
        const aborted = !!(options && options.signal && options.signal.aborted);
        const content = tool
          ? [{ type: "toolCall", id: `call_fixture_${order}`, name: "bash", arguments: { command: tool } }]
          : [{ type: "text", text: "fixture response" }];
        const message = {
          role: "assistant", content, api: model.api,
          provider: model.provider, model: model.id,
          usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
          stopReason: aborted ? "aborted" : failed ? "error" : tool ? "toolUse" : "stop", timestamp: Date.now(),
          ...(failed && !aborted ? { errorMessage: "overloaded_error" } : {}),
        };
        if (aborted) stream.push({ type: "error", reason: "aborted", error: message });
        else if (failed) stream.push({ type: "error", reason: "error", error: message });
        else stream.push({ type: "done", reason: tool ? "toolUse" : "stop", message });
        stream.end();
      };
      const timer = setTimeout(answer, wait);
      if (options && options.signal) {
        options.signal.addEventListener("abort", () => { clearTimeout(timer); answer(); }, { once: true });
      }
      return stream;
    },
  });

  // each command only arranges what PI will do by itself in the next run
  pi.registerCommand("fx-retry", { handler: async () => { failNext = 1; pi.sendUserMessage("fixture retry"); } });
  pi.registerCommand("fx-error", { handler: async () => { failNext = 99; pi.sendUserMessage("fixture error"); } });
  pi.registerCommand("fx-ok", { handler: async () => { failNext = 0; } });
  pi.registerCommand("fx-slow", { handler: async (text) => { slowNext = Number(text) || 4000; pi.sendUserMessage("fixture slow"); } });
  pi.registerCommand("fx-tool", { handler: async (text) => { toolNext = String(text || "true"); pi.sendUserMessage("fixture tool"); } });
  pi.registerCommand("fx-follow", { handler: async () => { followUp = true; pi.sendUserMessage("fixture follow"); } });
  pi.registerCommand("fx-settled", { handler: async () => { afterSettle = true; pi.sendUserMessage("fixture settled"); } });
  pi.registerCommand("fx-continue", { handler: async () => { continueOnce = true; pi.sendUserMessage("fixture continue"); } });
  pi.on("agent_end", () => {
    if (!followUp) return;
    followUp = false;
    pi.sendUserMessage("fixture follow-up", { deliverAs: "followUp" });
  });
  pi.on("agent_before_settle", (event) => {
    if (!continueOnce) return undefined;
    continueOnce = false;
    // Pi continues only with something for the model to answer: one context entry
    const entry = { type: "custom_message", customType: "deck-fixture", content: "fixture continues", display: false };
    return { entries: [...event.entries, entry], continue: true };
  });
  pi.on("agent_settled", () => {
    if (!afterSettle) return;
    afterSettle = false;
    pi.sendUserMessage("fixture later");
  });

  const done = (ctx, name) => record("fixture_command_done", ctx, { command: name });
  pi.registerCommand("fx-confirm", { handler: async (_, ctx) => { await ctx.ui.confirm("Fixture", "Harmless test"); done(ctx, "confirm"); } });
  pi.registerCommand("fx-input", { handler: async (_, ctx) => { await ctx.ui.input("Fixture input"); done(ctx, "input"); } });
  pi.registerCommand("fx-select", { handler: async (_, ctx) => { await ctx.ui.select("Fixture select", ["one", "two"]); done(ctx, "select"); } });
  pi.registerCommand("fx-editor", { handler: async (_, ctx) => { await ctx.ui.editor("Fixture editor", "text"); done(ctx, "editor"); } });
  pi.registerCommand("fx-timed", { handler: async (_, ctx) => { await ctx.ui.confirm("Fixture timed", "Closes by itself", { timeout: 100 }); done(ctx, "timed"); } });
  pi.registerCommand("fx-nested", { handler: async (_, ctx) => {
    await Promise.all([ctx.ui.confirm("Fixture one", "First of two"), ctx.ui.input("Fixture two")]);
    done(ctx, "nested");
  } });
  // a component of the extension's own: any key, or five seconds, closes it
  pi.registerCommand("fx-custom", { handler: async (_, ctx) => {
    await ctx.ui.custom((_tui, _theme, _keys, close) => {
      const timer = setTimeout(() => close("timeout"), 5000);
      return {
        render: () => ["Fixture custom"],
        invalidate() {},
        handleInput() { clearTimeout(timer); close("key"); },
        dispose() { clearTimeout(timer); },
      };
    });
    done(ctx, "custom");
  } });
  // Pi's own thread does nothing for a while: no event, no timer, no socket
  pi.registerCommand("fx-freeze", { handler: async (text, ctx) => {
    const until = Date.now() + (Number(text) || 1000);
    record("fixture_freeze_start", ctx);
    while (Date.now() < until) { /* busy on purpose */ }
    record("fixture_freeze_end", ctx);
  } });
  // Pi dies where it stands: no shutdown event, no teardown
  pi.registerCommand("fx-crash", { handler: async () => { process.kill(process.pid, "SIGKILL"); } });

  pi.registerCommand("fx-new", { handler: async (_, ctx) => { await ctx.newSession(); } });
  pi.registerCommand("fx-resume", { handler: async (text, ctx) => { await ctx.switchSession(text || process.env.FIXTURE_RESUME); } });
  pi.registerCommand("fx-fork", { handler: async (_, ctx) => {
    const entry = ctx.sessionManager.getBranch().find((e) => e.type === "message" && e.message.role === "user");
    if (entry) await ctx.fork(entry.id);
  } });
}
