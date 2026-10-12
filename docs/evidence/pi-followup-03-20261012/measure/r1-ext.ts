// r1-ext.ts — TEST ONLY. Loaded with `-e` into a real Pi inside an experiment
// root, never packaged. It does three things and changes nothing Pi does:
//
// 1. A local provider (no model account) whose first answer asks Pi to run
//    ONE command with Pi's own shell tool, and whose next answer ends the run.
// 2. Around the runtime's own "make a child process" entry points it holds
//    every other thread of the Pi process (only.c) for the length of that one
//    call, and writes down which thread made the call, whether the hold was
//    whole, the child's pid and the JavaScript stack that led there. The
//    original function runs unchanged, on the thread Pi called it on.
// 3. It notes every fork() call of the process and writes the list when the run ends.
//
//   R1_ROOT  the experiment root      R1_NAME  the name results are written under
//   R1_TOOL  the command Pi's shell tool is asked to run
import fs from "node:fs";
import path from "node:path";
import { createRequire } from "node:module";
import { isMainThread, threadId } from "node:worker_threads";
import { createAssistantMessageEventStream } from "@earendil-works/pi-ai";

const require = createRequire(import.meta.url);
const ROOT = process.env.R1_ROOT!;
const NAME = process.env.R1_NAME!;
const log = path.join(ROOT, "out", `${NAME}.pi.jsonl`);
const say = (row: Record<string, unknown>) => fs.appendFileSync(log, `${JSON.stringify(row)}\n`);
const only = require(path.join(ROOT, "bin/only.node"));
const cp = require("node:child_process");
const IS_BUN = typeof (globalThis as any).Bun !== "undefined";

/** Run `call` with every other thread of this process held, and say what the kernel shows of them. */
function alone(what: string, detail: Record<string, unknown>, call: () => any, pidOf: (made: any) => unknown) {
  const me = only.self();
  const held = only.hold();
  const first = held >= 0 ? only.threads() : [];
  let made: any;
  let thrown: unknown = null;
  try { made = call(); } catch (error) { thrown = error; }
  const last = held >= 0 ? only.threads() : [];
  if (held >= 0) only.release();
  const others = (list: any[]) => list.filter((t) => t.tid !== me.tid);
  const ids = (list: any[]) => list.map((t) => t.tid).sort((a, b) => a - b).join(",");
  const moved = others(last).filter((t) => { const was = first.find((x: any) => x.tid === t.tid); return !was || !t.at || was.at !== t.at; }).length;
  const charged = others(last).map((t) => { const was = first.find((x: any) => x.tid === t.tid); return was ? { name: t.name, user_ns: t.user_ns - was.user_ns, system_ns: t.system_ns - was.system_ns } : { name: t.name, came: true }; })
    .filter((d: any) => d.came || d.user_ns !== 0 || d.system_ns !== 0);
  const same = ids(first) === ids(last);
  const all = others(first).every((t) => t.suspend >= 1) && others(last).every((t) => t.suspend >= 1);
  say({
    event: "child-made", what, ...detail, pid: thrown ? null : pidOf(made), threw: thrown ? String(thrown) : null,
    thread: { ...me, isMainThread, threadId },
    hold: { held, threads: first.length, same_threads: same, all_held: all, others_moved: moved, charged, whole: held >= 0 && same && all && moved === 0 },
    thread_names: first.map((t: any) => t.name),
    stack: String(new Error().stack).split("\n").slice(2, 14).map((line) => line.trim()),
  });
  if (thrown) throw thrown;
  return made;
}

const brief = (value: unknown) => String(value ?? "").slice(0, 160);
if (IS_BUN) {
  // In Bun, node:child_process calls Bun.spawn and Bun.spawnSync by name each time.
  const bun = (globalThis as any).Bun;
  const patched: string[] = [];
  for (const name of ["spawn", "spawnSync"]) {
    const original = bun[name];
    try {
      bun[name] = function (...args: any[]) {
        const cmd = Array.isArray(args[0]) ? args[0] : args[0] && args[0].cmd;
        return alone(`Bun.${name}`, { file: brief(cmd && cmd[0]), args: (cmd || []).slice(1, 4).map(brief) }, () => original.apply(bun, args), (made) => made && made.pid);
      };
      if (bun[name] !== original) patched.push(`Bun.${name}`);
    } catch (error) { say({ event: "not-patched", what: `Bun.${name}`, error: String(error) }); }
  }
  say({ event: "patched", runtime: "bun", version: bun.version, revision: bun.revision, patched });
} else {
  // In Node every asynchronous child goes through ChildProcess.prototype.spawn; the synchronous ones through
  // the exported functions, which are patched and published to the ES module view of the builtin as well.
  const original = cp.ChildProcess.prototype.spawn;
  cp.ChildProcess.prototype.spawn = function (options: any) {
    return alone("ChildProcess.prototype.spawn", { file: brief(options && options.file), args: ((options && options.args) || []).slice(1, 4).map(brief) },
      () => original.call(this, options), () => this.pid);
  };
  const patched = ["ChildProcess.prototype.spawn"];
  for (const name of ["spawnSync", "execSync", "execFileSync"]) {
    const sync = cp[name];
    cp[name] = function (...args: any[]) {
      return alone(`child_process.${name}`, { file: brief(args[0]), args: (Array.isArray(args[1]) ? args[1] : []).slice(0, 3).map(brief) }, () => sync.apply(cp, args), (made) => made && made.pid);
    };
    patched.push(`child_process.${name}`);
  }
  require("node:module").syncBuiltinESMExports();
  say({ event: "patched", runtime: "node", version: process.version, patched });
}

// From here on every thread of this process that calls fork() is noted (only.c): what Pi's own tool run
// leaves in that list is written at the end of the run. posix_spawn is no fork and leaves nothing.
const watching = only.watchForks();

export default function (pi: any) {
  say({ event: "loaded", watching_forks: watching, pid: process.pid, exec: process.execPath, thread: { ...only.self(), isMainThread, threadId }, thread_names: only.threads().map((t: any) => t.name) });
  let asked = false;
  pi.registerProvider("deck-fixture", {
    baseUrl: "http://127.0.0.1:1",
    apiKey: "not-a-secret",
    api: "deck-fixture-api",
    models: [{
      id: "fixture", name: "Local fixture", reasoning: false, input: ["text"],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 100000, maxTokens: 1000,
    }],
    streamSimple(model: any) {
      const stream = createAssistantMessageEventStream();
      const tool = asked ? null : process.env.R1_TOOL;
      asked = true;
      say({ event: "provider-request", asks_for_tool: !!tool });
      const content = tool
        ? [{ type: "toolCall", id: "call_r1", name: "bash", arguments: { command: tool } }]
        : [{ type: "text", text: "fixture response" }];
      const message = {
        role: "assistant", content, api: model.api, provider: model.provider, model: model.id,
        usage: { input: 1, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 2, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
        stopReason: tool ? "toolUse" : "stop", timestamp: Date.now(),
      };
      setTimeout(() => { stream.push({ type: "done", reason: tool ? "toolUse" : "stop", message }); stream.end(); }, 50);
      return stream;
    },
  });
  for (const name of ["agent_start", "agent_end", "session_shutdown"]) {
    try {
      pi.on(name, () => say({
        event: name, thread: { isMainThread, threadId }, thread_names: only.threads().map((t: any) => t.name),
        ...(name === "agent_end" ? { fork_calls_in_this_process_since_load: only.forks() } : {}),
      }));
    } catch (error) { say({ event: "not-subscribed", name, error: String(error) }); }
  }
}
