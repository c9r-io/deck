'use strict';
// stall.cjs — TEST ONLY. probe.cjs with a WITNESS (only2.c): a hold that lasts longer than R1_PATIENCE_MS is
// ended by a thread the hold never suspends, which first writes down where every thread stood. What follows is
// probe.cjs's own text; only `alone`, the two measured loops and the pause before a hold differ.
//
// probe.cjs — TEST ONLY. Runs unchanged in Node and in Bun and answers, for
// the runtime it runs in, two questions with every OTHER thread of the
// process held (only.c): does `net.createConnection({ path })` leave a
// connection that already exists when the call returns, and do the child
// process APIs make their child? What the kernel did for this process while
// the others could not run, the thread that runs this JavaScript asked for.
//
//   R1_ROOT   the experiment root (bin/, out/, the sockets and fifos)
//   R1_NAME   the name results are written under
//   R1_PHASE  ONE of: identity, control, connect, spawn, forkers, worker, race, race-fork, editor-race
//   R1_COUNT  how many times (all but identity)
//   R1_API    which child process API (spawn)
//
// One JSON line per result goes to out/<name>.jsonl; connect and spawn write
// one line per CHUNK of runs, each complete in itself, so that a process
// that had to be stopped leaves what it had finished. Before every hold the
// run it is about to make is written to out/<name>.progress. Nothing is read
// from or written outside the root.
//
// A held thread may hold something this thread needs (an allocator's lock, a
// collector's job): then this thread waits for it for ever. That is a run
// that proves nothing; the harness sees that nothing moves, stops the
// process through the registry and starts another for what is left.
const fs = require('node:fs');
const net = require('node:net');
const cp = require('node:child_process');
const crypto = require('node:crypto');
const path = require('node:path');
const { Worker, isMainThread, workerData, parentPort } = require('node:worker_threads');

const ROOT = process.env.R1_ROOT;
const NAME = process.env.R1_NAME;
const PHASE = process.env.R1_PHASE;
const COUNT = Number(process.env.R1_COUNT || 0);
const OUT = path.join(ROOT, 'out');
const KID = path.join(ROOT, 'bin/kid');
const SOCK = path.join(ROOT, 's.sock');
const SOCK_ALL = path.join(ROOT, 'a.sock');
const only = require(path.join(ROOT, 'bin/only2.node'));
const IS_BUN = typeof Bun !== 'undefined';

const cell = new Int32Array(new SharedArrayBuffer(4));
const sleep = (ms) => { Atomics.wait(cell, 0, 0, ms); };
const turn = (ms) => new Promise((done) => setTimeout(done, ms));

function kidFile(tag) { return path.join(OUT, 'kids', `${NAME}-${tag}.json`); }

/** What a child wrote about itself, waited for WITHOUT leaving this thread. */
function readKid(file, patience) {
  const until = Date.now() + patience;
  for (;;) {
    try {
      const text = fs.readFileSync(file, 'utf8');
      if (text.endsWith('\n')) return JSON.parse(text);
    } catch (_) { /* not there yet */ }
    if (Date.now() > until) return null;
    sleep(1);
  }
}

/** What a child was born with beyond its three standard descriptors, and whether any of it is a connection. */
function beyond(kid) {
  return kid.fds.filter((f) => f.fd > 2 || (f.kind === 'sock' && [SOCK, SOCK_ALL].includes(f.peer))).map((f) => `${f.fd}:${f.kind}:${f.peer}`);
}

/** One descriptor of a child, as a word. */
const shown = (f) => `${f.fd}:${f.kind}${f.peer ? `:${f.peer}` : ''}${f.state ? `:${f.state}:wrote ${f.wrote}` : ''}`;
/** What the children that were born with a socket of this process found it to be, and whether their line was taken. */
function held(kids) {
  const socks = kids.flatMap((k) => k.fds.filter((f) => f.fd > 2 && f.kind === 'sock'));
  return {
    // "open": connected to the listener when the child looked; "eof": it had been connected and the listener
    // had let go of it; "unconnected": this process had not connected it (yet)
    sockets_found_connected: socks.filter((f) => f.state === 'open').length,
    sockets_found_closed_by_the_listener: socks.filter((f) => f.state === 'eof').length,
    sockets_found_unconnected: socks.filter((f) => f.state === 'unconnected').length,
    children_whose_line_the_kernel_took: kids.filter((k) => k.fds.some((f) => f.fd > 2 && f.kind === 'sock' && f.wrote > 0)).length,
  };
}

// ---------- a worker: the second thread that runs JavaScript ------------------------
if (!isMainThread) {
  const flag = new Int32Array(workerData.flag);
  parentPort.postMessage({ ready: true, self: only.self() });
  parentPort.on('message', ({ files, options }) => {
    for (const file of files) cp.spawnSync(KID, [file], { stdio: 'ignore', ...options });
    Atomics.store(flag, 0, 1);
    Atomics.notify(flag, 0);
  });
  return;
}

// ---------- the main thread ------------------------------------------------------
const lines = fs.openSync(path.join(OUT, `${NAME}.jsonl`), 'a');
const say = (row) => fs.writeSync(lines, `${JSON.stringify({ phase: PHASE, ...row })}\n`);
const progress = fs.openSync(path.join(OUT, `${NAME}.progress`), 'w');
const about = (text) => fs.writeSync(progress, `${text} @${Date.now()}`.padEnd(120), 0);

// the harness registers this process before anything is measured
while (!fs.existsSync(path.join(OUT, `${NAME}.go`))) sleep(5);

const ctl = fs.openSync(path.join(ROOT, 'ctl'), 'r+');
const ack = fs.openSync(path.join(ROOT, 'ack'), 'r+');
const answer = Buffer.alloc(96);
/** One question to the listener and its answer, on this thread, start to end. */
function ask(kind, n) {
  fs.writeSync(ctl, `${kind} ${n}\n`);
  let text = '';
  while (!text.endsWith('\n')) text += answer.toString('latin1', 0, fs.readSync(ack, answer, 0, answer.length, null));
  const [id, flag, pid] = text.trim().split(' ').map(Number);
  if (id !== n) throw new Error(`answer ${id} to question ${n}`);
  return { flag, pid };
}

const me = only.self();
// the witness ends a hold that lasts longer than this and writes down where every thread stood
const PATIENCE = Number(process.env.R1_PATIENCE_MS || 1500);
const WITNESS = only.witness(PATIENCE, path.join(OUT, `${NAME}.witness.jsonl`));
if (!WITNESS) throw new Error('the witness did not start');
// how long the others are given before a hold (probe.cjs: 2 ms; its first version: none)
const PAUSE = Number(process.env.R1_PAUSE_MS || 2);
// when each step of a hold was reached (ms)
const marks = new Float64Array(8);
const mark = (k) => { marks[k] = Date.now(); };
const STEPS = ['hold()', 'threads(), first reading', 'the work, up to the measured call', 'the measured call', 'the work, after the measured call', 'threads(), last reading', 'release()'];
const steps = () => Object.fromEntries(STEPS.map((name, k) => [name, marks[k] && marks[k + 1] ? marks[k + 1] - marks[k] : null]));

/** Run `work` with every other thread held; say what the kernel shows of them before and after. */
function alone(work) {
  marks.fill(0);
  const rescues = only.rescued();
  mark(0);
  const held = only.hold();
  if (held < 0) throw new Error('the hold was refused');
  mark(1);
  const first = only.threads();
  mark(2);
  let result = null;
  let failed = null;
  try { result = work(); } catch (error) { failed = String(error && error.message || error); }
  mark(5);
  const last = only.threads();
  mark(6);
  only.release();
  mark(7);
  // 1 when the witness ended this hold: this thread got here only because the others ran again
  const rescued = only.rescued() - rescues;
  // the witness writes its line after it let the others go: wait for it, so that an exit does not cut it off
  if (rescued) { const until = Date.now() + 3000; while (only.written() < only.rescued() && Date.now() < until) sleep(5); }
  const ids = (list) => list.map((t) => t.tid).sort((a, b) => a - b).join(',');
  const others = (list) => list.filter((t) => t.tid !== me.tid && !t.witness);
  const same_threads = ids(first) === ids(last);
  const all_held = others(first).every((t) => t.suspend >= 1) && others(last).every((t) => t.suspend >= 1);
  // how many of the others stand somewhere else in their own code than at the first reading
  const others_moved = others(last).filter((t) => {
    const was = first.find((x) => x.tid === t.tid);
    return !was || !t.at || was.at !== t.at;
  }).length;
  // which of the others the kernel charged any processor time in between, and how much (nanoseconds)
  const charged = others(last).map((t) => {
    const was = first.find((x) => x.tid === t.tid);
    return was ? { name: t.name, user_ns: t.user_ns - was.user_ns, system_ns: t.system_ns - was.system_ns } : { name: t.name, came: true };
  }).filter((d) => d.came || d.user_ns !== 0 || d.system_ns !== 0);
  return {
    result, failed, held, threads: first.length, same_threads, all_held, others_moved, charged,
    rescued, steps: rescued ? steps() : null,
    // A hold proves something only when it was whole: no thread came or went, every other one was suspended at
    // both readings, and none of them stands anywhere else in its own code.
    whole: !rescued && same_threads && all_held && others_moved === 0,
  };
}

const PROOF = () => ({ whole_holds: 0, a_thread_came_or_went: 0, not_all_held: 0, others_moved_runs: 0, others_charged_runs: 0, charged_user_ns_max: 0, charged_system_ns_max: 0, charged_names: [], held_min: Infinity, held_max: 0 });
function count(tally, r) {
  tally.whole_holds += r.whole ? 1 : 0;
  tally.a_thread_came_or_went += r.same_threads ? 0 : 1;
  tally.not_all_held += r.all_held ? 0 : 1;
  tally.others_moved_runs += r.others_moved ? 1 : 0;
  tally.others_charged_runs += r.charged.length ? 1 : 0;
  tally.held_min = Math.min(tally.held_min, r.held);
  tally.held_max = Math.max(tally.held_max, r.held);
  for (const d of r.charged) {
    tally.charged_user_ns_max = Math.max(tally.charged_user_ns_max, d.user_ns || 0);
    tally.charged_system_ns_max = Math.max(tally.charged_system_ns_max, d.system_ns || 0);
    if (!tally.charged_names.includes(d.name)) tally.charged_names.push(d.name);
  }
}
const brief = (r) => ({ held: r.held, threads: r.threads, same_threads: r.same_threads, all_held: r.all_held, others_moved: r.others_moved, charged: r.charged, failed: r.failed });

const APIS = {
  'child_process.spawnSync': (file) => { const r = cp.spawnSync(KID, [file], { stdio: 'ignore' }); return { pid: r.pid }; },
  'child_process.spawn': (file) => { const c = cp.spawn(KID, [file], { stdio: 'ignore' }); c.on('error', () => {}); return { pid: c.pid, child: c }; },
  // the pipes a tool's output is read through, and a session of its own: how Pi starts a shell
  'child_process.spawn pipes+detached': (file) => { const c = cp.spawn(KID, [file], { stdio: ['ignore', 'pipe', 'pipe'], detached: true }); c.on('error', () => {}); return { pid: c.pid, child: c }; },
  'child_process.execFile': (file) => { const c = cp.execFile(KID, [file], () => {}); return { pid: c.pid, child: c }; },
  'child_process.exec': (file) => { const c = cp.exec(`'${KID}' '${file}'`, () => {}); return { child: c }; },
  'child_process.execFileSync': (file) => { cp.execFileSync(KID, [file], { stdio: 'ignore' }); return {}; },
  'child_process.execSync': (file) => { cp.execSync(`'${KID}' '${file}'`, { stdio: 'ignore' }); return {}; },
  'Bun.spawnSync': (file) => { const r = Bun.spawnSync({ cmd: [KID, file], stdin: 'ignore', stdout: 'ignore', stderr: 'ignore' }); return { pid: r.pid }; },
  'Bun.spawn': (file) => { const c = Bun.spawn({ cmd: [KID, file], stdin: 'ignore', stdout: 'ignore', stderr: 'ignore' }); return { pid: c.pid }; },
};

async function main() {
  fs.mkdirSync(path.join(OUT, 'kids'), { recursive: true });

  if (PHASE === 'identity') {
    say({
      name: NAME, pid: process.pid, versions: { node: process.versions.node, bun: process.versions.bun || null, uv: process.versions.uv || null },
      revision: IS_BUN ? Bun.revision : null, exec: process.execPath, self: me, threads: only.threads().map((t) => ({ tid: t.tid, name: t.name })),
    });
  }

  // ---- the witness's own control: a holder that waits for a HELD thread, on purpose
  if (PHASE === 'witness-control') {
    const flag = new Int32Array(new SharedArrayBuffer(4));
    const worker = new Worker(__filename, { workerData: { flag: flag.buffer } });
    worker.on('error', (error) => say({ error: String(error) }));
    await new Promise((done) => worker.once('message', done));
    for (let i = 0; i < COUNT; i++) {
      Atomics.store(flag, 0, 0);
      await turn(50);
      about(`witness-control ${i}`);
      const r = alone(() => {
        mark(3);
        // asked of the worker with the worker held: its answer can come only after the hold is over
        worker.postMessage({ files: [], options: {} });
        const woke = Atomics.wait(flag, 0, 0, 30000);
        mark(4);
        return { woke };
      });
      say({ i, stall: !!r.rescued, steps: r.steps, held: r.held, failed: r.failed, woke: r.result && r.result.woke });
    }
    await worker.terminate();
  }

  // ---- the control: work that DOES leave this thread must not finish while the others are held
  if (PHASE === 'control') {
    // once without a hold, so that the pool's threads exist before one is taken
    await new Promise((done) => crypto.pbkdf2('p', 's', 2000, 32, 'sha256', done));
    for (let i = 0; i < COUNT; i++) {
      let finished = false;
      await turn(20);
      about(`control ${i}`);
      const before = only.threads().length;
      const held = only.hold();
      crypto.pbkdf2('p', 's', 2000, 32, 'sha256', () => { finished = true; });
      let turns = 0;
      const from = Date.now();
      // this thread's own event loop keeps turning for 300 ms
      await new Promise((done) => { const tick = setInterval(() => { turns++; if (Date.now() - from >= 300) { clearInterval(tick); done(); } }, 10); });
      const under = finished;
      const during = only.threads().length;
      only.release();
      const released = Date.now();
      await new Promise((done) => { const tick = setInterval(() => { if (finished) { clearInterval(tick); done(); } }, 2); });
      say({ i, held, threads_before: before, threads_during: during, loop_turns_under_hold: turns, finished_under_hold: under, finished_after_release_ms: Date.now() - released });
    }
  }

  // ---- net.createConnection({ path }): does the connection exist when the call returns?
  if (PHASE === 'connect') {
    // the same calls without a hold first, so that what the runtime compiles or starts lazily exists already
    for (let i = 0; i < 300; i++) {
      const socket = net.createConnection({ path: SOCK });
      socket.on('error', () => {});
      ask('p', -1 - i);
      socket.destroy();
    }
    await turn(300);
    ask('d', 0);
    const CHUNK = 25;
    let tally = null;
    let odd = [];
    for (let i = 1; i <= COUNT; i++) {
      tally = tally || { runs: 0, nothing_waiting_before: 0, waiting_at_return: 0, peer_is_this_process: 0, proven: 0, failed: 0, ...PROOF() };
      // the others are given a moment to finish what the last run left them
      sleep(PAUSE);
      about(`connect ${i}`);
      let socket = null;
      const r = alone(() => {
        const before = ask('p', 2 * i);
        mark(3);
        socket = net.createConnection({ path: SOCK });
        mark(4);
        // nothing between the call's return and the question
        const after = ask('p', 2 * i + 1);
        return { before, after };
      });
      if (r.rescued) say({ stall: true, i, steps: r.steps, held: r.held, failed: r.failed, after_the_rescue: r.result });
      if (socket) { socket.on('error', () => {}); socket.destroy(); }
      tally.runs++;
      if (r.failed) { tally.failed++; odd.push({ i, ...brief(r) }); } else {
        tally.nothing_waiting_before += r.result.before.flag === 0 ? 1 : 0;
        tally.waiting_at_return += r.result.after.flag === 1 ? 1 : 0;
        tally.peer_is_this_process += r.result.after.pid === process.pid ? 1 : 0;
        // the whole claim for one run: nothing waited before, the connection waited at the return, it is this
        // process's, and no other thread of the process could have had a part in it
        tally.proven += r.whole && r.result.before.flag === 0 && r.result.after.flag === 1 && r.result.after.pid === process.pid ? 1 : 0;
        count(tally, r);
        if (!(r.whole && r.result.after.flag === 1) && odd.length < 10) odd.push({ i, result: r.result, ...brief(r) });
      }
      if (i % CHUNK === 0 || i === COUNT) {
        say({ upto: i, ...tally, odd });
        tally = null;
        odd = [];
        await turn(10);
      }
    }
    ask('d', 1);
  }

  // ---- one child process API: is the child made while the others are held?
  if (PHASE === 'spawn') {
    const api = process.env.R1_API;
    const make = APIS[api];
    // a connection of this process that stays open while the children are made
    const kept = net.createConnection({ path: SOCK });
    kept.on('error', () => {});
    await new Promise((done) => kept.once('connect', done));
    const keeping = ask('k', 7);
    const later = [];
    const settle = () => { for (const child of later.splice(0)) { try { child.stdout && child.stdout.destroy(); child.stderr && child.stderr.destroy(); child.unref(); } catch (_) { /* gone */ } } };
    // without a hold first: whatever the runtime sets up lazily exists before one is taken
    for (let w = 0; w < 30; w++) {
      const made = make(kidFile(`warm-${w}`));
      if (made.child) later.push(made.child);
      readKid(kidFile(`warm-${w}`), 5000);
    }
    settle();
    await turn(300);
    const CHUNK = 10;
    let tally = null;
    let odd = [];
    for (let i = 1; i <= COUNT; i++) {
      tally = tally || { api, runs: 0, connection_kept_meanwhile: keeping.flag === 1 && keeping.pid === process.pid, child_ran_under_hold: 0, child_of_this_process: 0, pid_returned: 0, pid_as_returned: 0, born_with_stdio_only: 0, proven: 0, failed: 0, ...PROOF() };
      const file = kidFile(`${i}`);
      sleep(PAUSE);
      about(`spawn ${api} ${i}`);
      const r = alone(() => {
        mark(3);
        const made = make(file);
        mark(4);
        if (made.child) { later.push(made.child); delete made.child; }
        // still on this thread, and still alone: wait for the child's own word
        return { made, kid: readKid(file, 4000) };
      });
      if (r.rescued) say({ stall: true, i, steps: r.steps, held: r.held, failed: r.failed, child_after_the_rescue: !!(r.result && r.result.kid) });
      tally.runs++;
      const kid = r.result && r.result.kid;
      if (r.failed || !kid) { tally.failed++; if (odd.length < 10) odd.push({ i, made: r.result && r.result.made, ...brief(r) }); } else {
        tally.child_ran_under_hold++;
        tally.child_of_this_process += kid.ppid === process.pid ? 1 : 0;
        if (typeof r.result.made.pid === 'number') { tally.pid_returned++; tally.pid_as_returned += r.result.made.pid === kid.pid ? 1 : 0; }
        tally.born_with_stdio_only += beyond(kid).length === 0 ? 1 : 0;
        // the whole claim for one run: the child ran, it is this process's child, and no other thread of the
        // process could have had a part in making it
        tally.proven += r.whole && kid.ppid === process.pid ? 1 : 0;
        count(tally, r);
        if (!(r.whole && beyond(kid).length === 0) && odd.length < 10) odd.push({ i, kid, ...brief(r) });
      }
      if (i % CHUNK === 0 || i === COUNT) {
        say({ upto: i, ...tally, odd });
        tally = null;
        odd = [];
        settle();
        await turn(30);
      }
    }
    kept.destroy();
  }

  // ---- the ways of making a child that call fork(): which thread calls it?
  if (PHASE === 'forkers') {
    const watching = only.watchForks();
    // a connection of this process that stays open while the children are made
    const kept = net.createConnection({ path: SOCK });
    kept.on('error', () => {});
    await new Promise((done) => kept.once('connect', done));
    const keeping = ask('k', 7);
    const cases = {
      // the ordinary way: posix_spawn, so no fork at all is expected
      'child_process.spawnSync': (file) => { cp.spawnSync(KID, [file], { stdio: 'ignore' }); },
    };
    if (!IS_BUN) {
      // libuv cannot hand a user or group id to posix_spawn and forks instead (deps/uv/src/unix/process.c)
      cases['child_process.spawnSync with uid and gid'] = (file) => { cp.spawnSync(KID, [file], { stdio: 'ignore', uid: process.getuid(), gid: process.getgid() }); };
    } else {
      // a child on a pseudo-terminal: Bun forks for it (src/jsc/bindings/bun-spawn.cpp)
      cases['Bun.spawn with a terminal'] = (file) => Bun.spawn([KID, file], { terminal: { cols: 80, rows: 24, data() {} } });
      // an editor is started from a thread of its own (src/cli/open.zig). The "editor" is this root's kid, named
      // by its absolute path: Bun then looks nowhere else for one, and nothing of the machine's is started.
      cases['Bun.openInEditor'] = (file) => { Bun.openInEditor(file, { editor: KID }); };
    }
    for (const [label, make] of Object.entries(cases)) {
      for (let i = 1; i <= COUNT; i++) {
        const file = kidFile(`${label.replace(/[^A-Za-z]+/g, '-')}-${i}`);
        about(`forkers ${label} ${i}`);
        only.forks();
        let threw = null;
        let made = null;
        try { made = make(file); } catch (error) { threw = String(error && error.message || error); }
        // this thread only sleeps until the child has said who it is
        const kid = readKid(file, 4000);
        await turn(100);
        const calls = only.forks();
        say({
          case: label, i, watching, threw, connection_kept_meanwhile: keeping.flag === 1 && keeping.pid === process.pid,
          child_ran: !!kid, child_of_this_process: !!kid && kid.ppid === process.pid,
          born_with: kid ? kid.fds.map((f) => `${f.fd}:${f.kind}${f.peer ? `:${f.peer}` : ''}`) : null,
          beyond_stdio: kid ? beyond(kid) : null,
          fork_calls: calls, this_thread: me,
          forks_by_this_thread: calls.filter((c) => c.tid === me.tid).length,
          forks_by_another_thread: calls.filter((c) => c.tid !== me.tid).length,
        });
        try { made && made.terminal && made.terminal.close(); } catch (_) { /* closed */ }
      }
    }
    kept.destroy();
  }

  // ---- a second thread that runs JavaScript makes children by itself
  if (PHASE === 'worker' || PHASE === 'race' || PHASE === 'race-fork') {
    const flag = new Int32Array(new SharedArrayBuffer(4));
    const worker = new Worker(__filename, { workerData: { flag: flag.buffer } });
    worker.on('error', (error) => say({ error: String(error) }));
    const ready = await new Promise((done) => worker.once('message', done));

    if (PHASE === 'worker') {
      // this thread does NOTHING while the worker makes its children: it sleeps in the kernel until woken
      const files = Array.from({ length: COUNT }, (_, i) => kidFile(`${i + 1}`));
      const mine = () => only.threads().find((t) => t.tid === me.tid);
      about('worker');
      const cpu0 = mine();
      const from = Date.now();
      worker.postMessage({ files, options: {} });
      const woke = Atomics.wait(flag, 0, 0, 60000);
      const until = Date.now();
      const cpu1 = mine();
      const kids = files.map((file) => readKid(file, 100));
      const stamps = files.map((file) => { try { return fs.statSync(file).mtimeMs; } catch (_) { return null; } });
      say({
        count: COUNT, worker: ready.self, main: me, woke,
        main_thread_cpu_ns_while_asleep: (cpu1.user_ns + cpu1.system_ns) - (cpu0.user_ns + cpu0.system_ns),
        asleep_ms: until - from,
        children_made: kids.filter(Boolean).length,
        children_of_this_process: kids.filter((k) => k && k.ppid === process.pid).length,
        // written between the instant this thread went to sleep and the instant it woke (file times: 2 ms slack)
        written_while_main_slept: stamps.filter((t) => t !== null && t >= from - 2 && t <= until + 2).length,
        born_with_stdio_only: kids.filter((k) => k && beyond(k).length === 0).length,
      });
    }

    if (PHASE === 'race' || PHASE === 'race-fork') {
      // this thread makes and drops connections as fast as it can, never leaving its loop body, while the worker
      // makes children: does any child come to hold one of them? In rounds, so that what was dropped is collected.
      // `race`: the worker's children are made the ordinary way (posix_spawn). `race-fork`: the worker asks for a
      // user and a group id, for which libuv forks and executes instead (deps/uv/src/unix/process.c).
      const options = PHASE === 'race-fork' ? { uid: process.getuid(), gid: process.getgid() } : {};
      const per = 50;
      const files = Array.from({ length: COUNT }, (_, i) => kidFile(`${i + 1}`));
      let made = 0;
      let took = 0;
      for (let at = 0; at < files.length; at += per) {
        about(`race ${at}`);
        Atomics.store(flag, 0, 0);
        const from = Date.now();
        worker.postMessage({ files: files.slice(at, at + per), options });
        while (Atomics.load(flag, 0) === 0 && Date.now() - from < 60000) {
          const socket = net.createConnection({ path: SOCK_ALL });
          socket.on('error', () => {});
          socket.destroy();
          made++;
        }
        took += Date.now() - from;
        await turn(30);
      }
      const kids = files.map((file) => readKid(file, 100));
      const holders = kids.filter((k) => k && beyond(k).length > 0);
      const withSocket = kids.filter((k) => k && k.fds.some((f) => f.fd > 2 && f.kind === 'sock'));
      say({
        worker_made_its_children_with: PHASE === 'race-fork' ? 'fork and exec (uid and gid asked for)' : 'posix_spawn (the ordinary way)',
        spawns_by_the_worker: COUNT, connections_made_by_main_meanwhile: made, took_ms: took,
        children_made: kids.filter(Boolean).length,
        children_of_this_process: kids.filter((k) => k && k.ppid === process.pid).length,
        children_born_with_more_than_stdio: holders.length,
        children_born_with_a_socket_of_this_process: withSocket.length,
        ...held(withSocket),
        examples: holders.slice(0, 5).map((k) => ({ pid: k.pid, ppid: k.ppid, fds: k.fds.map(shown) })),
      });
    }
    await worker.terminate();
  }

  // ---- Bun only: an editor is started from a thread of its own, by fork and exec, while this thread makes connections
  if (PHASE === 'editor-race') {
    if (!IS_BUN) {
      say({ skipped: 'Bun.openInEditor exists in Bun only' });
    } else {
      only.watchForks();
      only.forks();
      const files = Array.from({ length: COUNT }, (_, i) => kidFile(`${i + 1}`));
      let made = 0;
      const from = Date.now();
      for (let i = 0; i < files.length; i++) {
        if (i % 25 === 0) { about(`editor-race ${i}`); await turn(20); }
        // the "editor" is this root's kid, named by its absolute path (see `forkers`)
        Bun.openInEditor(files[i], { editor: KID });
        // while the thread Bun started for it forks and executes, this thread makes and drops connections
        const until = performance.now() + 1.5;
        while (performance.now() < until) {
          const socket = net.createConnection({ path: SOCK_ALL });
          socket.on('error', () => {});
          socket.destroy();
          made++;
        }
      }
      const took = Date.now() - from;
      await turn(500);
      const kids = files.map((file) => readKid(file, 300));
      const calls = only.forks();
      const withSocket = kids.filter((k) => k && k.fds.some((f) => f.fd > 2 && f.kind === 'sock'));
      say({
        editors_started: COUNT, connections_made_by_this_thread_meanwhile: made, took_ms: took,
        fork_calls: calls.length,
        forks_by_this_thread: calls.filter((c) => c.tid === me.tid).length,
        forks_by_another_thread: calls.filter((c) => c.tid !== me.tid).length,
        forking_thread_names: [...new Set(calls.map((c) => c.name))],
        children_made: kids.filter(Boolean).length,
        children_of_this_process: kids.filter((k) => k && k.ppid === process.pid).length,
        children_born_with_a_socket_of_this_process: withSocket.length,
        ...held(withSocket),
        examples: withSocket.slice(0, 5).map((k) => ({ pid: k.pid, ppid: k.ppid, fds: k.fds.map(shown) })),
      });
    }
  }

  say({ done: true });
  about('done');
}

main().then(() => process.exit(0), (error) => { say({ error: String(error && error.stack || error) }); process.exit(1); });
