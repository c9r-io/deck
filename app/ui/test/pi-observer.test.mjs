// Deck's Pi observer (app/src-tauri/resources/pi/), run here the way Pi runs
// it: the files are loaded as they are, with no compiler, against a stand-in
// for Pi's extension API that offers `on` and records every other thing the
// observer reaches for, and a stand-in for Deck's status socket that speaks
// the server half of the `pi-observer` wire. The backend half is
// `app/src-tauri/src/agent_pi.rs`; what a real Pi does is the native run
// (`scripts/pi_native.py`). Nothing here reaches a real Deck: the three
// routing variables are replaced before any callback is called.
import test from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import { chmodSync, copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');
const resources = resolve(root, 'app/src-tauri/resources/pi');
// under /tmp on purpose: a Unix socket address holds 104 bytes, and the
// observer refuses a path that does not fit
const scratch = mkdtempSync('/tmp/deck-pi-obs-');
test.after(() => rmSync(scratch, { recursive: true, force: true }));

// The two files, byte for byte, under names Node loads without a flag. The
// entry's one relative specifier follows the renamed file; nothing else of
// either file is touched, so a line that is not plain JavaScript fails here.
const entryText = readFileSync(join(resources, 'signal.ts'), 'utf8');
const loaded = join(scratch, 'loaded');
mkdirSync(join(loaded, 'node_modules/@earendil-works/pi-coding-agent'), { recursive: true });
writeFileSync(join(loaded, 'node_modules/@earendil-works/pi-coding-agent/package.json'),
  JSON.stringify({ name: '@earendil-works/pi-coding-agent', type: 'module', exports: './index.js' }));
writeFileSync(join(loaded, 'node_modules/@earendil-works/pi-coding-agent/index.js'), 'export const VERSION = "1.1.0";\n');
copyFileSync(join(resources, 'observer.ts'), join(loaded, 'observer.mjs'));
const entries = {};
for (const name of ['signal', 'lifecycle', 'both']) {
  const text = readFileSync(join(resources, `${name}.ts`), 'utf8');
  writeFileSync(join(loaded, `${name}.mjs`), text.replace('"./observer.ts"', '"./observer.mjs"'));
  entries[name] = { text, factory: (await import(pathToFileURL(join(loaded, `${name}.mjs`)).href)).default };
}
const observer = await import(pathToFileURL(join(loaded, 'observer.mjs')).href);
const entry = entries.signal.factory;

const sleep = ms => new Promise(done => setTimeout(done, ms));
async function until(what, predicate, ms = 4000) {
  const end = Date.now() + ms;
  for (;;) {
    const value = predicate();
    if (value) return value;
    if (Date.now() > end) assert.fail(`timed out waiting for ${what}`);
    await sleep(10);
  }
}

/** A stand-in for Deck's status socket. `mode`: `auto` accepts and
 *  acknowledges, `manual` answers nothing by itself, `refuse:<code>` refuses
 *  the hello. Every line of every connection is kept. `name` is for the
 *  reader; the directory's own name stays short. */
let decks = 0;
function deck(name, mode = 'auto') {
  decks += 1;
  const directory = join(scratch, `d${decks}`);
  mkdirSync(directory, { mode: 0o700 });
  const path = join(directory, 'status.sock');
  const state = { mode, path, connections: [], closed: 0 };
  const server = net.createServer(socket => {
    const connection = { socket, lines: [], closed: false };
    state.connections.push(connection);
    let buffer = '';
    socket.setEncoding('utf8');
    socket.on('data', chunk => {
      buffer += chunk;
      for (let at = buffer.indexOf('\n'); at >= 0; at = buffer.indexOf('\n')) {
        const frame = JSON.parse(buffer.slice(0, at));
        buffer = buffer.slice(at + 1);
        connection.lines.push(frame);
        if (state.mode === 'auto') state.answer(connection, frame);
        else if (state.mode.startsWith('refuse:')) state.say(connection, { refused: state.mode.slice(7) });
      }
    });
    socket.on('error', () => {});
    socket.on('close', () => { connection.closed = true; state.closed += 1; });
  });
  state.say = (connection, reply) => connection.socket.write(`${JSON.stringify({ protocol: 'pi-observer', v: 1, ...reply })}\n`);
  state.answer = (connection, frame) => state.say(connection, 'capabilities' in frame ? { accepted: true } : { ack: frame.seq });
  state.frames = (index = state.connections.length - 1) => state.connections[index]?.lines.slice(1) ?? [];
  state.listening = new Promise(done => server.listen(path, () => { chmodSync(path, 0o600); done(); }));
  state.close = () => new Promise(done => {
    for (const connection of state.connections) connection.socket.destroy();
    server.close(done);
  });
  return state;
}

/** Pi's extension API as far as the observer may use it: `on`. Anything
 *  else it reaches for — on the API, a context or an event — is recorded. */
function pi() {
  const handlers = new Map();
  const touched = [];
  const only = (label, allowed) => new Proxy({}, {
    get(_target, name) {
      if (Object.hasOwn(allowed, name)) return allowed[name];
      if (typeof name === 'string') touched.push(`${label}.${name}`);
      return undefined;
    },
    set(_target, name) { touched.push(`${label}.${String(name)}=`); return true; },
    has(_target, name) { touched.push(`${String(name)} in ${label}`); return false; },
    ownKeys() { touched.push(`keys(${label})`); return []; },
  });
  const api = only('pi', { on: (event, handler) => { handlers.set(event, handler); } });
  return {
    api,
    handlers,
    touched,
    context: mode => only('ctx', { mode }),
    emit: (event, payload = only(`${event} event`, {}), ctx = only('ctx', { mode: 'tui' })) => handlers.get(event)(payload, ctx),
  };
}

const saved = {};
function route(values) {
  for (const name of ['DECK_STATUS_SOCK', 'TMUX', 'TMUX_PANE']) {
    if (!(name in saved)) saved[name] = process.env[name];
    if (values[name] === undefined) delete process.env[name];
    else process.env[name] = values[name];
  }
}
test.after(() => route(saved));
const routed = server => ({ DECK_STATUS_SOCK: server.path, TMUX: '/private/tmp/t/tmux-501/deck-dev,4242,0', TMUX_PANE: '%7' });

/** One loaded observer with its session started against `server`. */
async function started(name, mode = 'auto') {
  const server = deck(name, mode);
  await server.listening;
  const host = pi();
  entry(host.api);
  route(routed(server));
  host.emit('session_start');
  return { server, host };
}

test('each entry is two imports and one call, and the files load as plain JavaScript', () => {
  for (const [name, signal, lifecycle] of [['signal', true, false], ['lifecycle', false, true], ['both', true, true]]) {
    assert.equal(entries[name].text.split('\n').filter(line => line && !line.startsWith('//')).join('\n'), [
      'import { VERSION } from "@earendil-works/pi-coding-agent";',
      'import { deckPiObserver } from "./observer.ts";',
      `export default deckPiObserver({ signal: ${signal}, lifecycle: ${lifecycle} }, VERSION);`,
    ].join('\n'), name);
    assert.equal(typeof entries[name].factory, 'function');
  }
  assert.equal(entryText, entries.signal.text);
  assert.deepEqual(Object.keys(observer).sort(), ['deckPiObserver', 'privateSocket', 'route']);
});

test('the factory asks Pi for four events and starts nothing', async () => {
  const server = deck('factory');
  await server.listening;
  route(routed(server));
  const host = pi();
  assert.equal(entry(host.api), undefined);
  assert.deepEqual([...host.handlers.keys()], ['session_start', 'agent_start', 'agent_settled', 'session_shutdown']);
  assert.deepEqual(host.touched, []);
  // a loaded extension without a session start connects to nothing
  host.emit('agent_start');
  host.emit('agent_settled');
  await sleep(150);
  assert.equal(server.connections.length, 0);
  // a capability the entry does not carry subscribes to nothing of it
  const quiet = pi();
  observer.deckPiObserver({ signal: false, lifecycle: false }, '1.1.0')(quiet.api);
  assert.deepEqual([...quiet.handlers.keys()], ['session_start', 'session_shutdown']);
  await server.close();
});

test('the route is three well-formed variables or nothing', () => {
  const good = { DECK_STATUS_SOCK: '/Users/a/.deck/status.sock', TMUX: '/private/tmp/tmux-501/deck,4242,3', TMUX_PANE: '%12' };
  assert.deepEqual(observer.route(good), { path: '/Users/a/.deck/status.sock', socket: 'deck', serverPid: 4242, pane: '%12' });
  const bad = change => assert.equal(observer.route({ ...good, ...change }), null, JSON.stringify(change));
  for (const name of Object.keys(good)) bad({ [name]: undefined });
  bad({ DECK_STATUS_SOCK: 'status.sock' });
  bad({ DECK_STATUS_SOCK: '/Users/a/.deck/other.sock' });
  bad({ DECK_STATUS_SOCK: '/Users/a/.deck/status.sock\0' });
  bad({ DECK_STATUS_SOCK: `/${'d'.repeat(92)}/status.sock` });
  assert.ok(observer.route({ ...good, DECK_STATUS_SOCK: `/${'d'.repeat(90)}/status.sock` }), 'a path of 103 bytes fits');
  bad({ DECK_STATUS_SOCK: `/${'目'.repeat(31)}/status.sock` });
  bad({ TMUX_PANE: '12' });
  bad({ TMUX_PANE: '%' });
  bad({ TMUX_PANE: '%1;kill-server' });
  bad({ TMUX_PANE: '%1234567890' });
  bad({ TMUX: '/private/tmp/tmux-501/deck' });
  bad({ TMUX: '/private/tmp/tmux-501/deck,x,3' });
  bad({ TMUX: '/private/tmp/tmux-501/deck,1,3' });
  bad({ TMUX: '/private/tmp/tmux-501/deck,4294967296,3' });
  bad({ TMUX: '/private/tmp/tmux-501/de ck,4242,3' });
  bad({ TMUX: `/private/tmp/tmux-501/${'d'.repeat(65)},4242,3` });
  bad({ TMUX: '/private/tmp/tmux-501/,4242,3' });
});

test('the socket must be this user\'s, private, in a private directory', async () => {
  const server = deck('private');
  await server.listening;
  assert.equal(observer.privateSocket(server.path), true);
  chmodSync(server.path, 0o644);
  assert.equal(observer.privateSocket(server.path), false, 'a socket others may write');
  chmodSync(server.path, 0o600);
  chmodSync(dirname(server.path), 0o755);
  assert.equal(observer.privateSocket(server.path), false, 'a directory others may enter');
  chmodSync(dirname(server.path), 0o700);
  writeFileSync(join(dirname(server.path), 'file'), '');
  chmodSync(join(dirname(server.path), 'file'), 0o600);
  assert.equal(observer.privateSocket(join(dirname(server.path), 'file')), false, 'a file is no socket');
  assert.equal(observer.privateSocket(join(dirname(server.path), 'missing')), false);
  // the observer does not connect to a socket that is not private
  chmodSync(server.path, 0o666);
  const host = pi();
  entry(host.api);
  route(routed(server));
  host.emit('session_start');
  await sleep(200);
  assert.equal(server.connections.length, 0);
  host.emit('session_shutdown', { reason: 'quit' });
  await server.close();
});

test('only an interactive terminal session connects, and only with a route', async () => {
  const server = deck('modes');
  await server.listening;
  for (const mode of ['rpc', 'json', 'print', undefined]) {
    const host = pi();
    entry(host.api);
    route(routed(server));
    host.emit('session_start', undefined, host.context(mode));
    host.emit('agent_start');
    host.emit('agent_settled');
  }
  for (const missing of ['DECK_STATUS_SOCK', 'TMUX', 'TMUX_PANE']) {
    const host = pi();
    entry(host.api);
    route({ ...routed(server), [missing]: undefined });
    host.emit('session_start');
    host.emit('agent_start');
  }
  // a version Pi could not have exported is not sent anywhere
  for (const version of ['', '1.1.0 beta', '1'.repeat(33), 110, undefined]) {
    const host = pi();
    observer.deckPiObserver({ signal: true, lifecycle: false }, version)(host.api);
    route(routed(server));
    host.emit('session_start');
  }
  await sleep(250);
  assert.equal(server.connections.length, 0);
  await server.close();
});

test('the hello is one closed line and frames are numbered run boundaries', async () => {
  const { server, host } = await started('runs');
  await until('the hello', () => server.connections[0]?.lines.length === 1);
  assert.deepEqual(server.connections[0].lines[0], {
    protocol: 'pi-observer', v: 1, source: 'pi', socket: 'deck-dev', server_pid: 4242, pane: '%7',
    observerVersion: 1, piVersion: '1.1.0', mode: 'tui', capabilities: { signal: true, lifecycle: false },
  });
  // a settlement without a run reports nothing
  host.emit('agent_settled');
  // a run: its start, a second start before the settlement (a retry, a queued
  // follow-up, a continuation) is the same run, then its one ending
  host.emit('agent_start');
  await until('the start', () => server.frames().length === 1);
  host.emit('agent_start');
  host.emit('agent_start');
  host.emit('agent_settled');
  host.emit('agent_settled');
  host.emit('agent_start');
  host.emit('agent_settled');
  await until('four frames', () => server.frames().length === 4);
  await sleep(100);
  const common = { protocol: 'pi-observer', v: 1, source: 'pi' };
  assert.deepEqual(server.frames(), [
    { ...common, seq: 1, run: 1, state: 'working' },
    { ...common, seq: 2, run: 1, state: 'turn-done' },
    { ...common, seq: 3, run: 2, state: 'working' },
    { ...common, seq: 4, run: 2, state: 'turn-done' },
  ]);
  for (const line of server.connections[0].lines) assert.ok(JSON.stringify(line).length <= 1024);
  // nothing of Pi was read: no event field, no context field but the mode
  assert.deepEqual(host.touched, []);
  await host.emit('session_shutdown', { reason: 'quit' });
  await server.close();
});

test('one frame waits for its acknowledgement at a time', async () => {
  const { server, host } = await started('ordered', 'manual');
  const connection = await until('the connection', () => server.connections[0]);
  await until('the hello', () => connection.lines.length === 1);
  // a run that starts before the hello is accepted is reported once it is
  host.emit('agent_start');
  host.emit('agent_settled');
  host.emit('agent_start');
  await sleep(100);
  assert.equal(connection.lines.length, 1, 'nothing follows an unanswered hello');
  server.say(connection, { accepted: true });
  await until('the first frame', () => connection.lines.length === 2);
  host.emit('agent_settled');
  host.emit('agent_start');
  await sleep(100);
  assert.equal(connection.lines.length, 2, 'the second frame waits for the first acknowledgement');
  assert.deepEqual(connection.lines[1], { protocol: 'pi-observer', v: 1, source: 'pi', seq: 1, run: 1, state: 'working' });
  server.say(connection, { ack: 1 });
  await until('the second frame', () => connection.lines.length === 3);
  await sleep(50);
  assert.equal(connection.lines.length, 3);
  server.say(connection, { ack: 2 });
  await until('the third frame', () => connection.lines.length === 4);
  assert.deepEqual(connection.lines.slice(2).map(line => [line.seq, line.run, line.state]),
    [[2, 1, 'turn-done'], [3, 2, 'working']]);
  await host.emit('session_shutdown', { reason: 'quit' });
  await server.close();
});

test('a lost connection is replaced without replaying anything', async () => {
  const { server, host } = await started('reconnect');
  host.emit('agent_start');
  await until('the start', () => server.frames(0).length === 1);
  // the connection goes away mid-run and the run settles meanwhile
  server.connections[0].socket.destroy();
  await until('the observer to notice', () => server.connections[0].closed);
  host.emit('agent_settled');
  host.emit('agent_start');
  host.emit('agent_settled');
  await until('the next connection', () => server.connections.length === 2, 3000);
  await until('its hello', () => server.connections[1].lines.length === 1);
  await sleep(200);
  assert.deepEqual(server.frames(1), [], 'what ended while disconnected is never reported');
  // numbering starts again with the connection
  host.emit('agent_start');
  await until('a new run', () => server.frames(1).length === 1);
  assert.deepEqual([server.frames(1)[0].seq, server.frames(1)[0].run, server.frames(1)[0].state], [1, 1, 'working']);
  // lost again while that run is active: the next connection reports it as its run 1
  server.connections[1].socket.destroy();
  await until('the third connection', () => server.connections.length === 3, 3000);
  await until('the active run', () => server.frames(2).length === 1);
  assert.deepEqual([server.frames(2)[0].seq, server.frames(2)[0].run, server.frames(2)[0].state], [1, 1, 'working']);
  host.emit('agent_settled');
  await until('its ending', () => server.frames(2).length === 2);
  assert.deepEqual([server.frames(2)[1].seq, server.frames(2)[1].run, server.frames(2)[1].state], [2, 1, 'turn-done']);
  await host.emit('session_shutdown', { reason: 'quit' });
  await server.close();
});

test('a refusal ends the stream, and a lasting one ends the observer', async () => {
  // turned off in Deck: nothing reconnects until Pi loads its extensions again
  const off = await started('refused-off', 'refuse:capability-disabled');
  await until('the refusal to be read', () => off.server.connections[0]?.closed);
  off.host.emit('agent_start');
  await sleep(1400);
  assert.equal(off.server.connections.length, 1, 'no second connection after a lasting refusal');
  await off.server.close();
  // a refusal that can pass (Deck is busy) is retried
  const busy = await started('refused-busy', 'refuse:busy');
  await until('the retry and its refusal', () => busy.server.connections.length === 2 && busy.server.connections[1].closed, 3000);
  busy.server.mode = 'auto';
  await until('the next connection', () => busy.server.connections.length === 3, 6000);
  busy.host.emit('agent_start');
  await until('a frame on the accepted connection', () => busy.server.frames(2).length === 1);
  await busy.host.emit('session_shutdown', { reason: 'quit' });
  await busy.server.close();
  // revoked in the middle of a stream
  const { server, host } = await started('revoked');
  host.emit('agent_start');
  await until('the start', () => server.frames(0).length === 1);
  server.say(server.connections[0], { refused: 'capability-disabled' });
  await until('the stream to close', () => server.connections[0].closed);
  host.emit('agent_settled');
  host.emit('agent_start');
  await sleep(1400);
  assert.equal(server.connections.length, 1);
  await server.close();
});

test('an answer the observer did not ask for closes the connection', async () => {
  for (const [name, reply] of [
    ['a wrong number', { ack: 7 }],
    ['another protocol', { protocol: 'agent-lifecycle', ack: 1 }],
    ['another version', { v: 2, ack: 1 }],
    ['an acceptance in place of an acknowledgement', { accepted: true }],
  ]) {
    const { server, host } = await started(`unexpected-${name.replaceAll(' ', '-')}`, 'manual');
    const connection = await until('the connection', () => server.connections[0]);
    await until('the hello', () => connection.lines.length === 1);
    server.say(connection, { accepted: true });
    host.emit('agent_start');
    await until('the frame', () => connection.lines.length === 2);
    connection.socket.write(`${JSON.stringify({ protocol: 'pi-observer', v: 1, ...reply })}\n`);
    await until(`${name} to close the connection`, () => connection.closed);
    await host.emit('session_shutdown', { reason: 'quit' });
    await server.close();
  }
  // a line that is not JSON at all
  const { server, host } = await started('unexpected-garbage', 'manual');
  const connection = await until('the connection', () => server.connections[0]);
  await until('the hello', () => connection.lines.length === 1);
  connection.socket.write('accepted\n');
  await until('the connection to close', () => connection.closed);
  await host.emit('session_shutdown', { reason: 'quit' });
  await server.close();
});

test('an unanswered frame and a full queue close the connection', async () => {
  // no acknowledgement for two seconds
  const silent = await started('silent', 'manual');
  const first = await until('the connection', () => silent.server.connections[0]);
  const from = Date.now();
  await until('the observer to give up', () => first.closed, 4000);
  const waited = Date.now() - from;
  assert.ok(waited >= 1500 && waited < 3500, `gave up after ${waited} ms`);
  await silent.host.emit('session_shutdown', { reason: 'quit' });
  await silent.server.close();
  // more frames than may wait behind the unanswered one
  const { server, host } = await started('flood', 'manual');
  const connection = await until('the connection', () => server.connections[0]);
  await until('the hello', () => connection.lines.length === 1);
  server.say(connection, { accepted: true });
  host.emit('agent_start');
  await until('the frame', () => connection.lines.length === 2);
  for (let pair = 0; pair < 4; pair += 1) {
    host.emit('agent_settled');
    host.emit('agent_start');
  }
  await sleep(100);
  assert.equal(connection.closed, false, 'eight frames may wait');
  host.emit('agent_settled');
  await until('the overflow to close the connection', () => connection.closed);
  assert.equal(connection.lines.length, 2, 'nothing queued was written');
  await host.emit('session_shutdown', { reason: 'quit' });
  await server.close();
});

test('a shutdown says why once, briefly, and nothing follows it', async () => {
  for (const [given, sent] of [['quit', 'quit'], ['reload', 'reload'], ['new', 'new'], ['resume', 'resume'], ['fork', 'fork'], ['exit', 'fault'], [undefined, 'fault']]) {
    const { server, host } = await started(`shutdown-${given}`);
    host.emit('agent_start');
    await until('the start', () => server.frames(0).length === 1);
    await host.emit('session_shutdown', { reason: given });
    await until('the stream to close', () => server.connections[0].closed);
    assert.deepEqual(server.frames(0).at(-1), { protocol: 'pi-observer', v: 1, source: 'pi', seq: 2, reason: sent });
    // this runtime is over: no callback does anything and nothing reconnects
    host.emit('agent_settled');
    host.emit('agent_start');
    host.emit('session_start');
    await sleep(120);
    assert.equal(server.connections.length, 1);
    assert.equal(server.frames(0).length, 2);
    await server.close();
  }
  // Deck does not answer: the shutdown still returns within its 100 ms
  const { server, host } = await started('shutdown-unanswered', 'manual');
  const connection = await until('the connection', () => server.connections[0]);
  await until('the hello', () => connection.lines.length === 1);
  server.say(connection, { accepted: true });
  host.emit('agent_start');
  await until('the frame', () => connection.lines.length === 2);
  const from = Date.now();
  await host.emit('session_shutdown', { reason: 'quit' });
  assert.ok(Date.now() - from < 400, 'the shutdown did not wait for Deck');
  await until('the stream to close', () => connection.closed);
  assert.equal(connection.lines.length, 2, 'the reset stayed behind the unanswered frame and was dropped');
  await server.close();
  // before the hello was accepted there is nothing to withdraw
  const early = await started('shutdown-early', 'manual');
  const pending = await until('the connection', () => early.server.connections[0]);
  assert.equal(early.host.emit('session_shutdown', { reason: 'quit' }), undefined);
  await until('the stream to close', () => pending.closed);
  await early.server.close();
});

test('the factory never throws into Pi, whatever registering a callback does', async () => {
  // Pi 1.1.0 refuses to start when an extension's factory throws (native run:
  // "Failed to load extension"), so a Pi whose API changed must still start
  const server = deck('factory-fault');
  await server.listening;
  route(routed(server));
  const handlers = new Map();
  let calls = 0;
  const failing = { on(event, handler) { calls += 1; if (calls === 2) throw new Error('no such event'); handlers.set(event, handler); } };
  assert.doesNotThrow(() => entry(failing));
  assert.deepEqual([...handlers.keys()], ['session_start'], 'registration stopped where it failed');
  // what was registered before the failure does nothing: the observer is disposed
  const ctx = { mode: 'tui' };
  assert.doesNotThrow(() => handlers.get('session_start')({}, ctx));
  await sleep(200);
  assert.equal(server.connections.length, 0);
  for (const broken of [{}, { on: null }, null, undefined]) assert.doesNotThrow(() => entry(broken));
  await server.close();
});

test('an error inside a callback ends the observer and never reaches Pi', async () => {
  const { server, host } = await started('fault');
  host.emit('agent_start');
  await until('the start', () => server.frames(0).length === 1);
  // a shutdown event Pi would never send: reading its reason throws
  assert.doesNotThrow(() => host.emit('session_shutdown', null));
  await until('the stream to close', () => server.connections[0].closed);
  host.emit('agent_settled');
  host.emit('agent_start');
  host.emit('session_start');
  await sleep(1300);
  assert.equal(server.connections.length, 1, 'a disposed observer does not reconnect');
  assert.equal(server.frames(0).length, 1);
  await server.close();
  // and one in the first callback of all
  const fresh = deck('fault-start');
  await fresh.listening;
  const other = pi();
  entry(other.api);
  route(routed(fresh));
  assert.doesNotThrow(() => other.emit('session_start', undefined, null));
  other.emit('session_start');
  await sleep(200);
  assert.equal(fresh.connections.length, 0);
  await fresh.close();
});

/* ---------- session observation: the lifecycle capability ---------- */

/** A context whose session manager answers `getSessionId` and records any
 *  other thing asked of it. */
function sessionContext(host, id) {
  const state = { id, asked: 0 };
  const manager = new Proxy({}, {
    get(_target, name) {
      if (name === 'getSessionId') return () => { state.asked += 1; if (state.id instanceof Error) throw state.id; return state.id; };
      if (typeof name === 'string') host.touched.push(`sessionManager.${name}`);
      return undefined;
    },
  });
  const ctx = new Proxy({}, {
    get(_target, name) {
      if (name === 'mode') return 'tui';
      if (name === 'sessionManager') return manager;
      if (typeof name === 'string') host.touched.push(`ctx.${name}`);
      return undefined;
    },
  });
  return { state, ctx };
}

async function observing(which, id, reason = 'startup') {
  const server = deck(`lifecycle-${which}`);
  await server.listening;
  const host = pi();
  entries[which].factory(host.api);
  route(routed(server));
  const session = sessionContext(host, id);
  host.emit('session_start', { reason }, session.ctx);
  return { server, host, session };
}
const common = { protocol: 'pi-observer', v: 1, source: 'pi' };

test('without the lifecycle capability the session manager is never asked', async () => {
  const { server, host, session } = await observing('signal', 'session-one');
  host.emit('agent_start', undefined, session.ctx);
  await until('the start', () => server.frames().length === 1);
  host.emit('agent_settled', undefined, session.ctx);
  await until('the ending', () => server.frames().length === 2);
  await host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  await until('the stream to close', () => server.connections[0].closed);
  assert.equal(session.state.asked, 0);
  assert.deepEqual(server.connections[0].lines[0].capabilities, { signal: true, lifecycle: false });
  assert.ok(server.connections[0].lines.every(line => !('nativeSessionId' in line) && !('event' in line)));
  assert.deepEqual(host.touched, []);
  await server.close();
});

test('the session a runtime starts in, what the API says after a reconnect, and its teardown', async () => {
  const { server, host, session } = await observing('both', 'session-one', 'resume');
  await until('the start', () => server.frames().length === 1);
  assert.deepEqual(server.connections[0].lines[0].capabilities, { signal: true, lifecycle: true });
  assert.deepEqual(server.frames()[0], { ...common, seq: 1, event: 'start', nativeSessionId: 'session-one', reason: 'resume' });
  // both capabilities are ONE stream with one numbering
  host.emit('agent_start', undefined, session.ctx);
  await until('the run', () => server.frames().length === 2);
  assert.deepEqual(server.frames()[1], { ...common, seq: 2, run: 1, state: 'working' });
  // the connection is lost; the next one says what the API says NOW, as a snapshot, never a start again
  session.state.id = 'session-two';
  server.connections[0].socket.destroy();
  await until('the second connection', () => server.connections.length === 2, 3000);
  await until('its frames', () => server.frames(1).length === 2);
  assert.deepEqual(server.frames(1), [
    { ...common, seq: 1, event: 'snapshot', nativeSessionId: 'session-two' },
    { ...common, seq: 2, run: 1, state: 'working' },
  ]);
  // the teardown: the id is read once more, then the shutdown and the reset with Pi's reason
  session.state.id = 'session-three';
  await host.emit('session_shutdown', { reason: 'fork' }, session.ctx);
  await until('the stream to close', () => server.connections[1].closed);
  assert.deepEqual(server.frames(1).slice(2), [
    { ...common, seq: 3, event: 'shutdown', nativeSessionId: 'session-three', reason: 'fork' },
    { ...common, seq: 4, reason: 'fork' },
  ]);
  assert.deepEqual(host.touched, [], 'nothing but the mode and the session id was read');
  await server.close();
});

test('session observation alone subscribes to no run and reports none', async () => {
  const { server, host, session } = await observing('lifecycle', 'fixture.custom-ID_123');
  assert.deepEqual([...host.handlers.keys()], ['session_start', 'session_shutdown']);
  await until('the start', () => server.frames().length === 1);
  assert.deepEqual(server.connections[0].lines[0].capabilities, { signal: false, lifecycle: true });
  assert.deepEqual(server.frames()[0], { ...common, seq: 1, event: 'start', nativeSessionId: 'fixture.custom-ID_123', reason: 'startup' });
  await host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  await until('the stream to close', () => server.connections[0].closed);
  assert.deepEqual(server.frames().slice(1).map(frame => frame.event ?? `reset:${frame.reason}`), ['shutdown', 'reset:quit']);
  await server.close();
});

test('an id or a reason the observer cannot vouch for is not sent', async () => {
  // a start reason Pi does not have: only what the API says is reported, as a snapshot
  const odd = await observing('lifecycle', 'session-one', 'restored');
  await until('the snapshot', () => odd.server.frames().length === 1);
  assert.deepEqual(odd.server.frames()[0], { ...common, seq: 1, event: 'snapshot', nativeSessionId: 'session-one' });
  // a shutdown reason Pi does not have: no shutdown is claimed, the stream is withdrawn as a fault
  await odd.host.emit('session_shutdown', { reason: 'exit' }, odd.session.ctx);
  await until('the stream to close', () => odd.server.connections[0].closed);
  assert.deepEqual(odd.server.frames().slice(1), [{ ...common, seq: 2, reason: 'fault' }]);
  await odd.server.close();
  // no id, an id that is no string, one too long to send, a manager that throws: nothing is said of a session
  for (const id of ['', null, 42, 'a'.repeat(513), new Error('no session')]) {
    const { server, host, session } = await observing('lifecycle', id);
    await until('the hello', () => server.connections[0]?.lines.length === 1);
    await sleep(150);
    assert.deepEqual(server.frames(), [], String(id).slice(0, 12));
    assert.doesNotThrow(() => host.emit('session_shutdown', { reason: 'quit' }, session.ctx));
    await until('the stream to close', () => server.connections[0].closed);
    assert.ok(server.frames().every(frame => !('nativeSessionId' in frame)));
    await server.close();
  }
  // the longest id that is sent at all (Deck keeps at most 256 bytes and says so itself)
  const long = await observing('lifecycle', 'a'.repeat(512));
  await until('the start', () => long.server.frames().length === 1);
  assert.equal(long.server.frames()[0].nativeSessionId.length, 512);
  assert.ok(JSON.stringify(long.server.frames()[0]).length <= 1024);
  await long.host.emit('session_shutdown', { reason: 'quit' }, long.session.ctx);
  await long.server.close();
});
