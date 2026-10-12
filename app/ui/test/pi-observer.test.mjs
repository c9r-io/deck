// Deck's Pi observer (app/src-tauri/resources/pi/), run here the way Pi runs
// it: the files are loaded as they are, with no compiler, against a stand-in
// for Pi's extension API that offers `on` and records every other thing the
// observer reaches for, and a stand-in for Deck's status socket that speaks
// the server half of the `pi-observer` wire: one message per connection, and
// in every answer the nonce the next message must carry. The backend half is
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

// Every connection the observer makes, as this process sees it: how many of
// them exist at one time. The observer closes one before it makes the next
// (`destroy` closes the descriptor there and then on both runtimes Pi ships
// on: `docs/evidence/pi-followup-02-20261012/`), and Deck's proof of whose
// bytes a line is rests on that.
const live = new Set();
const seen = { made: 0, most: 0 };
const connect = net.createConnection;
net.createConnection = function createConnection(...args) {
  const socket = connect.apply(this, args);
  seen.made += 1;
  live.add(socket);
  seen.most = Math.max(seen.most, live.size);
  const destroy = socket.destroy;
  socket.destroy = function destroyed(...rest) {
    live.delete(socket);
    return destroy.apply(this, rest);
  };
  return socket;
};
test.after(() => { net.createConnection = connect; });

const common = { protocol: 'pi-observer', v: 2, source: 'pi' };
/** A line without its nonce: what it says. */
const plain = ({ nonce: _nonce, ...line }) => line;

/** A stand-in for Deck's status socket: every connection carries ONE line and
 *  gets one line of answer. `mode`: `auto` answers as Deck does, `manual`
 *  answers nothing by itself, `refuse:<code>` gives a hello its nonce and
 *  refuses everything else. `next` holds answers for the lines to come, each
 *  a function of the line: a reply, `null` for no answer at all, `undefined`
 *  for what `mode` would say. `broken` lists every line that did not carry
 *  the nonce of the answer before it. */
let decks = 0;
function deck(name, mode = 'auto') {
  decks += 1;
  const id = decks;
  const directory = join(scratch, `d${id}`);
  mkdirSync(directory, { mode: 0o700 });
  const path = join(directory, 'status.sock');
  const state = {
    name, mode, path, connections: [], leases: 0, minted: 0, usable: null, next: [],
    loaded: new Map(), expected: new Map(), broken: [],
  };
  state.kind = line => ('lease' in line ? 'frame' : 'nonce' in line ? 'attest' : 'hello');
  /** A nonce nobody has seen, kept as what `key` must carry next. */
  state.give = (key) => {
    state.minted += 1;
    const nonce = `${id.toString(16).padStart(4, '0')}${state.minted.toString(16).padStart(28, '0')}`;
    state.expected.set(key, nonce);
    return nonce;
  };
  state.bits = loaded => (state.usable
    ? { signal: loaded.signal && state.usable.signal, lifecycle: loaded.lifecycle && state.usable.lifecycle }
    : loaded);
  state.say = (connection, reply) => {
    connection.said = reply;
    connection.socket.end(typeof reply === 'string' ? reply : `${JSON.stringify({ protocol: 'pi-observer', v: 2, ...reply })}\n`);
  };
  /** What Deck answers this connection's line with. */
  state.answer = (connection) => {
    const line = connection.line;
    const kind = state.kind(line);
    if (kind === 'hello') return state.say(connection, { nonce: state.give('hello') });
    if (state.mode.startsWith('refuse:')) return state.say(connection, { refused: state.mode.slice(7) });
    if (kind === 'attest') {
      state.leases += 1;
      state.loaded.set(state.leases, line.capabilities);
      return state.say(connection, { accepted: state.leases, nonce: state.give(state.leases), capabilities: state.bits(line.capabilities) });
    }
    return state.say(connection, { ack: line.seq, nonce: state.give(line.lease), capabilities: state.bits(state.loaded.get(line.lease)) });
  };
  const server = net.createServer(socket => {
    const connection = { socket, line: null, more: false, closed: false, said: null };
    state.connections.push(connection);
    let buffer = '';
    socket.setEncoding('utf8');
    socket.on('data', chunk => {
      buffer += chunk;
      const at = buffer.indexOf('\n');
      if (connection.line) { connection.more = true; return; }
      if (at < 0) return;
      connection.line = JSON.parse(buffer.slice(0, at));
      connection.more = buffer.length > at + 1;
      const line = connection.line;
      const kind = state.kind(line);
      if (kind !== 'hello' && line.nonce !== state.expected.get(kind === 'attest' ? 'hello' : line.lease)) {
        state.broken.push(plain(line));
      }
      const scripted = state.next.length ? state.next.shift()(line, kind) : undefined;
      if (scripted === null) return;
      if (scripted !== undefined) state.say(connection, scripted);
      else if (state.mode !== 'manual') state.answer(connection);
    });
    socket.on('error', () => {});
    socket.on('close', () => { connection.closed = true; });
  });
  state.lines = () => state.connections.map(connection => connection.line).filter(Boolean);
  state.hellos = () => state.lines().filter(line => state.kind(line) === 'hello');
  state.attests = () => state.lines().filter(line => state.kind(line) === 'attest');
  /** Every frame of a lease but its heartbeats, in the order Deck read them. */
  state.frames = (lease = state.leases) => state.lines()
    .filter(line => line.lease === lease && Object.keys(line).length > 6);
  state.pings = (lease = state.leases) => state.lines()
    .filter(line => line.lease === lease && Object.keys(line).length === 6);
  /** The connection that carries line number `index`, once its line is read. */
  state.arrival = index => until(`line ${index + 1} of ${name}`, () => state.connections[index]?.line && state.connections[index]);
  state.listening = new Promise(done => server.listen(path, () => { chmodSync(path, 0o600); done(); }));
  state.close = () => new Promise(done => {
    for (const connection of state.connections) connection.socket.destroy();
    server.close(done);
  });
  return state;
}

/** The end of one test's Deck: every line carried the nonce it had to. */
async function closed(server, broken = []) {
  assert.deepEqual(server.broken, broken, `${server.name}: lines that did not carry the nonce before them`);
  await server.close();
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

/** The same, once Deck has accepted it. */
async function bound(name) {
  const { server, host } = await started(name);
  await until(`the lease of ${name}`, () => server.leases === 1);
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
  await closed(server);
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
  await closed(server);
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
  await closed(server);
});

test('binding is a hello and an attest, and frames are numbered run boundaries that carry the last nonce', async () => {
  const { server, host } = await bound('runs');
  const [hello, attest] = server.lines();
  // the hello claims nothing; the attest carries the nonce it was answered with
  assert.deepEqual(hello, common);
  assert.deepEqual(attest, {
    ...common, nonce: server.connections[0].said.nonce, socket: 'deck-dev', server_pid: 4242, pane: '%7',
    observerVersion: 2, piVersion: '1.1.0', mode: 'tui', capabilities: { signal: true, lifecycle: false },
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
  assert.deepEqual(server.frames().map(plain), [
    { ...common, lease: 1, seq: 1, run: 1, state: 'working' },
    { ...common, lease: 1, seq: 2, run: 1, state: 'turn-done' },
    { ...common, lease: 1, seq: 3, run: 2, state: 'working' },
    { ...common, lease: 1, seq: 4, run: 2, state: 'turn-done' },
  ]);
  // every message on a connection of its own, each with the nonce of the answer before it
  assert.equal(server.connections.length, 6);
  assert.deepEqual(server.frames().map(line => line.nonce), server.connections.slice(1, 5).map(connection => connection.said.nonce));
  assert.equal(new Set(server.connections.map(connection => connection.said.nonce)).size, 6);
  assert.ok(server.connections.every(connection => !connection.more), 'one line per connection');
  for (const line of server.lines()) assert.ok(JSON.stringify(line).length <= 1024);
  // nothing of Pi was read: no event field, no context field but the mode
  assert.deepEqual(host.touched, []);
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server);
});

test('one message waits for its answer at a time, and no second connection is made meanwhile', async () => {
  const { server, host } = await started('ordered', 'manual');
  const hello = await server.arrival(0);
  // a run that starts before the lease is reported once there is one
  host.emit('agent_start');
  host.emit('agent_settled');
  host.emit('agent_start');
  await sleep(100);
  assert.equal(server.connections.length, 1, 'nothing follows an unanswered hello');
  server.answer(hello);
  const attest = await server.arrival(1);
  await sleep(60);
  assert.equal(server.connections.length, 2, 'nothing follows an unanswered attest');
  server.answer(attest);
  const first = await server.arrival(2);
  assert.deepEqual(plain(first.line), { ...common, lease: 1, seq: 1, run: 1, state: 'working' });
  host.emit('agent_settled');
  host.emit('agent_start');
  await sleep(100);
  assert.equal(server.connections.length, 3, 'the second frame waits for the first answer');
  server.answer(first);
  const second = await server.arrival(3);
  await sleep(50);
  assert.equal(server.connections.length, 4);
  server.answer(second);
  const third = await server.arrival(4);
  assert.deepEqual([second.line, third.line].map(line => [line.seq, line.run, line.state]), [[2, 1, 'turn-done'], [3, 2, 'working']]);
  // each connection was closed before the next one was made
  await until('the earlier connections to close', () => server.connections.slice(0, 4).every(connection => connection.closed));
  assert.equal(third.closed, false);
  server.answer(third);
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server);
});

test('`again` is answered with the same message and the new nonce, a few times and then not at all', async () => {
  const { server, host } = await bound('again');
  // Pi made a child for a tool: Deck could not tell whose line that was, twice
  const again = line => ({ again: true, nonce: server.give(line.lease) });
  server.next.push(again, again);
  host.emit('agent_start');
  await until('the frame, three times', () => server.frames().length === 3);
  const sent = server.frames();
  assert.deepEqual(sent.map(plain), Array(3).fill({ ...common, lease: 1, seq: 1, run: 1, state: 'working' }));
  assert.equal(new Set(sent.map(line => line.nonce)).size, 3, 'each with the nonce of the answer before it');
  // the lease goes on, numbered without a gap
  host.emit('agent_settled');
  await until('the ending', () => server.frames().length === 4);
  assert.deepEqual(plain(server.frames()[3]), { ...common, lease: 1, seq: 2, run: 1, state: 'turn-done' });
  // more of them than the observer tries: it lets go of the lease, says so once, and binds again
  for (let attempt = 0; attempt < 7; attempt += 1) server.next.push(again);
  host.emit('agent_start');
  await until('the farewell', () => server.frames().some(line => line.reason === 'fault'), 3000);
  const third = server.frames().slice(4);
  assert.deepEqual(third.map(plain), [
    ...Array(7).fill({ ...common, lease: 1, seq: 3, run: 2, state: 'working' }),
    { ...common, lease: 1, seq: 3, reason: 'fault' },
  ]);
  await until('the next lease', () => server.leases === 2, 3000);
  // the run that is still open is that lease's run 1
  await until('its run', () => server.frames(2).length === 1);
  assert.deepEqual(plain(server.frames(2)[0]), { ...common, lease: 2, seq: 1, run: 1, state: 'working' });
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server);

  // an attest that is asked for again carries the new nonce and the same claims
  const other = deck('again-attest');
  await other.listening;
  let asked = 0;
  const twice = (line, kind) => {
    if (kind === 'attest' && asked < 2) {
      asked += 1;
      return { again: true, nonce: other.give('hello') };
    }
    return undefined;
  };
  other.next.push(twice, twice, twice, twice);
  const host2 = pi();
  entry(host2.api);
  route(routed(other));
  host2.emit('session_start');
  await until('the lease', () => other.leases === 1);
  assert.equal(other.hellos().length, 1, 'no second hello: the answer gave the nonce');
  assert.equal(other.attests().length, 3);
  assert.equal(new Set(other.attests().map(line => line.nonce)).size, 3);
  assert.equal(new Set(other.attests().map(line => JSON.stringify(plain(line)))).size, 1);
  await host2.emit('session_shutdown', { reason: 'quit' });
  await closed(other);
});

test('an answer that does not come ends the lease: Deck is told once and nothing is replayed', async () => {
  const { server, host } = await bound('silent');
  host.emit('agent_start');
  await until('the start', () => server.frames().length === 1);
  // Deck reads the next frame and says nothing
  server.next.push(() => null);
  const from = Date.now();
  host.emit('agent_settled');
  await until('the frame', () => server.frames().length === 2);
  const unanswered = server.connections.at(-1);
  // queued behind it, and forgotten with the lease
  host.emit('agent_start');
  // After two seconds the observer closes that connection and tells Deck that
  // it let go: a reset in the place of the frame. Deck, which may have read
  // the frame, asks for it again with a new nonce.
  server.next.push(line => ({ again: true, nonce: server.give(line.lease) }));
  await until('the farewell, twice', () => server.frames().length === 4, 4000);
  const waited = Date.now() - from;
  assert.ok(waited >= 1500 && waited < 3500, `let go after ${waited} ms`);
  assert.equal(unanswered.closed, true);
  assert.deepEqual(server.frames().slice(2).map(plain), Array(2).fill({ ...common, lease: 1, seq: 2, reason: 'fault' }));
  assert.notEqual(server.frames()[2].nonce, server.frames()[3].nonce);
  // a new lease: the run that is open now is its run 1; what was queued is not sent
  await until('the next lease', () => server.leases === 2, 3000);
  await until('its run', () => server.frames(2).length === 1);
  assert.deepEqual(plain(server.frames(2)[0]), { ...common, lease: 2, seq: 1, run: 1, state: 'working' });
  await sleep(100);
  assert.equal(server.frames(2).length, 1);
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server);
});

test('a refusal ends the lease, and a lasting one ends the observer', async () => {
  // turned off in Deck: nothing binds again until Pi loads its extensions again
  const off = await started('refused-off', 'refuse:capability-disabled');
  await until('the refusal to be read', () => off.server.connections[1]?.closed);
  off.host.emit('agent_start');
  await sleep(1400);
  assert.equal(off.server.connections.length, 2, 'no further connection after a lasting refusal');
  await closed(off.server);

  // a refusal that can pass (Deck is busy) is tried again
  const busy = await started('refused-busy', 'refuse:busy');
  await until('the second attest and its refusal', () => busy.server.attests().length === 2 && busy.server.connections.at(-1).closed, 3000);
  busy.server.mode = 'auto';
  await until('the lease', () => busy.server.leases === 1, 6000);
  busy.host.emit('agent_start');
  await until('a frame', () => busy.server.frames().length === 1);
  // Deck no longer has that lease (it started again): nothing is said about it, and a new one is asked for
  busy.server.next.push(() => ({ refused: 'late' }));
  busy.host.emit('agent_settled');
  await until('the next lease', () => busy.server.leases === 2, 3000);
  assert.equal(busy.server.lines().filter(line => 'reason' in line).length, 0);
  // a refusal that says nothing against the message: the same line once more
  busy.server.next.push(() => ({ refused: 'sender-mismatch' }));
  busy.host.emit('agent_start');
  await until('the frame, twice', () => busy.server.frames().length === 2);
  assert.deepEqual(busy.server.frames()[0], busy.server.frames()[1]);
  await busy.host.emit('session_shutdown', { reason: 'quit' });
  await closed(busy.server);

  // taken away for good in the middle of a lease
  const { server, host } = await bound('revoked-for-good');
  host.emit('agent_start');
  await until('the start', () => server.frames().length === 1);
  server.next.push(() => ({ refused: 'capability-disabled' }));
  host.emit('agent_settled');
  await until('the refused frame', () => server.frames().length === 2);
  host.emit('agent_start');
  await sleep(1400);
  assert.equal(server.lines().length, 4, 'neither a farewell nor a new binding');
  await closed(server);

  // a Deck that speaks another version says so in ITS framing; that holds for this runtime as well
  const other = await started('other-version', 'manual');
  other.server.say(await other.server.arrival(0), { v: 1, refused: 'bad-version' });
  await sleep(1300);
  assert.equal(other.server.connections.length, 1);
  await closed(other.server);
});

test('a second observer of one process stops asking once the lease cannot be one its process let go of', async () => {
  // another observer of this process holds the lease: asked again, as for a lease this process just let go of
  const { server, host } = await started('duplicate', 'refuse:duplicate-observer');
  await until('the first refusal', () => server.attests().length === 1 && server.connections.at(-1).closed);
  await until('the second attest, a second later', () => server.attests().length === 2, 3000);
  // twenty seconds of that: a lease one's own process let go of is over within fifteen
  const now = Date.now;
  Date.now = () => now() + 21000;
  try {
    await until('the third attest', () => server.attests().length === 3 && server.connections.at(-1).closed, 4000);
    host.emit('agent_start');
    await sleep(6000);
    assert.equal(server.attests().length, 3, 'and then no more in this runtime');
  } finally {
    Date.now = now;
  }
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server);
  // a lease that goes away in time is simply bound: the count starts again with every acceptance
  const other = await started('duplicate-passes', 'refuse:duplicate-observer');
  await until('a refusal', () => other.server.attests().length === 1 && other.server.connections.at(-1).closed);
  other.server.mode = 'auto';
  await until('the lease', () => other.server.leases === 1, 3000);
  await other.host.emit('session_shutdown', { reason: 'quit' });
  await closed(other.server);
});

test('an answer the observer did not ask for ends the lease, and Deck is told', async () => {
  const bits = { signal: true, lifecycle: false };
  for (const [name, reply] of [
    ['a wrong number', server => ({ ack: 7, nonce: server.give('nobody'), capabilities: bits })],
    ['another protocol', server => ({ protocol: 'agent-lifecycle', ack: 1, nonce: server.give('nobody'), capabilities: bits })],
    ['another version', server => ({ v: 3, ack: 1, nonce: server.give('nobody'), capabilities: bits })],
    ['an acceptance in place of an acknowledgement', server => ({ accepted: 1, nonce: server.give('nobody'), capabilities: bits })],
    ['no nonce', () => ({ ack: 1, capabilities: bits })],
    ['a nonce that is none', () => ({ ack: 1, nonce: 'N'.repeat(32), capabilities: bits })],
    ['capabilities that are not two bits', server => ({ ack: 1, nonce: server.give('nobody'), capabilities: { signal: true } })],
    ['a key too many', server => ({ ack: 1, nonce: server.give('nobody'), capabilities: bits, more: 1 })],
    ['a line that is not JSON', () => 'ack\n'],
  ]) {
    const { server, host } = await bound(`unexpected-${name.replaceAll(' ', '-')}`);
    server.next.push(() => reply(server));
    host.emit('agent_start');
    await until(`${name}: the farewell`, () => server.frames().length === 2);
    // in the place of the frame that would have come next, with the nonce the observer still holds
    const { run: _run, state: _state, ...frame } = server.frames()[0];
    assert.deepEqual(server.frames()[1], { ...frame, reason: 'fault' }, name);
    await host.emit('session_shutdown', { reason: 'quit' });
    await closed(server);
  }
  // a hello that is answered with something else binds nothing, and is asked again later
  const { server, host } = await started('unexpected-hello', 'manual');
  server.say(await server.arrival(0), 'accepted\n');
  await sleep(200);
  assert.equal(server.connections.length, 1);
  server.mode = 'auto';
  await until('the lease', () => server.leases === 1, 3000);
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server);
});

test('more frames than may wait end the lease, and its farewell waits for the connection that exists', async () => {
  const { server, host } = await started('flood', 'manual');
  server.answer(await server.arrival(0));
  server.answer(await server.arrival(1));
  host.emit('agent_start');
  const first = await server.arrival(2);
  for (let pair = 0; pair < 4; pair += 1) {
    host.emit('agent_settled');
    host.emit('agent_start');
  }
  await sleep(100);
  assert.equal(server.connections.length, 3, 'eight frames may wait');
  host.emit('agent_settled');
  await sleep(100);
  assert.equal(server.connections.length, 3, 'the farewell does not make a second connection');
  assert.equal(first.closed, false);
  // Deck answers the frame that was on its way; the observer has let go and does not read it
  server.answer(first);
  const farewell = await server.arrival(3);
  assert.deepEqual(plain(farewell.line), { ...common, lease: 1, seq: 1, reason: 'fault' });
  // it carries the nonce before that answer, so Deck asks for it again
  assert.deepEqual(server.broken, [plain(farewell.line)]);
  server.say(farewell, { again: true, nonce: server.give(1) });
  const again = await server.arrival(4);
  assert.deepEqual(plain(again.line), plain(farewell.line));
  server.answer(again);
  await sleep(100);
  assert.equal(server.connections.length, 5, 'nothing that was queued was written');
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server, [plain(farewell.line)]);
});

test('an idle lease says so every five seconds', async () => {
  const { server, host } = await bound('idle');
  await until('the heartbeat', () => server.pings().length === 1, 7000);
  assert.deepEqual(plain(server.pings()[0]), { ...common, lease: 1, seq: 1 });
  host.emit('agent_start');
  await until('the run', () => server.frames().length === 1);
  assert.deepEqual(plain(server.frames()[0]), { ...common, lease: 1, seq: 2, run: 1, state: 'working' });
  await host.emit('session_shutdown', { reason: 'quit' });
  await closed(server);
});

test('a shutdown says why once, briefly, and nothing follows it', async () => {
  for (const [given, sent] of [['quit', 'quit'], ['reload', 'reload'], ['new', 'new'], ['resume', 'resume'], ['fork', 'fork'], ['exit', 'fault'], [undefined, 'fault']]) {
    const { server, host } = await bound(`shutdown-${given}`);
    host.emit('agent_start');
    await until('the start', () => server.frames().length === 1);
    await host.emit('session_shutdown', { reason: given });
    assert.deepEqual(plain(server.frames().at(-1)), { ...common, lease: 1, seq: 2, reason: sent });
    // this runtime is over: no callback does anything and nothing binds again
    host.emit('agent_settled');
    host.emit('agent_start');
    host.emit('session_start');
    await sleep(120);
    assert.equal(server.lines().length, 4);
    await closed(server);
  }
  // Deck does not answer: the shutdown still returns within its 100 ms, the
  // connection that exists is closed and no other is made
  const { server, host } = await started('shutdown-unanswered', 'manual');
  server.answer(await server.arrival(0));
  server.answer(await server.arrival(1));
  host.emit('agent_start');
  const frame = await server.arrival(2);
  const from = Date.now();
  await host.emit('session_shutdown', { reason: 'quit' });
  assert.ok(Date.now() - from < 400, 'the shutdown did not wait for Deck');
  await until('the connection to close', () => frame.closed);
  await sleep(200);
  assert.equal(server.connections.length, 3, 'the reset stayed behind the unanswered frame and was dropped');
  await closed(server);
  // before there is a lease there is nothing to withdraw
  const early = await started('shutdown-early', 'manual');
  const pending = await early.server.arrival(0);
  assert.equal(early.host.emit('session_shutdown', { reason: 'quit' }), undefined);
  await until('the connection to close', () => pending.closed);
  await sleep(150);
  assert.equal(early.server.connections.length, 1);
  await closed(early.server);
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
  await closed(server);
});

test('an error inside a callback ends the observer and never reaches Pi', async () => {
  const { server, host } = await bound('fault');
  host.emit('agent_start');
  await until('the start', () => server.frames().length === 1);
  // a shutdown event Pi would never send: reading its reason throws
  assert.doesNotThrow(() => host.emit('session_shutdown', null));
  // Deck is told that nobody holds that lease any more, and that is all
  await until('the farewell', () => server.frames().length === 2);
  assert.deepEqual(plain(server.frames()[1]), { ...common, lease: 1, seq: 2, reason: 'fault' });
  host.emit('agent_settled');
  host.emit('agent_start');
  host.emit('session_start');
  await sleep(1300);
  assert.equal(server.lines().length, 4, 'a disposed observer binds nothing again');
  await closed(server);
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
  await closed(fresh);
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

async function observing(which, id, reason = 'startup', mode = 'auto') {
  const server = deck(`lifecycle-${which}`, mode);
  await server.listening;
  const host = pi();
  entries[which].factory(host.api);
  route(routed(server));
  const session = sessionContext(host, id);
  host.emit('session_start', { reason }, session.ctx);
  return { server, host, session };
}

/** Deck no longer has the lease: the frame that is sent next is refused, and
 *  the observer asks for a new one. `settle` then `start` keeps a run open. */
function forgotten(server, host, session) {
  server.next.push(() => ({ refused: 'late' }));
  host.emit('agent_settled', undefined, session.ctx);
  host.emit('agent_start', undefined, session.ctx);
}

test('without the lifecycle capability the session manager is never asked', async () => {
  const { server, host, session } = await observing('signal', 'session-one');
  await until('the lease', () => server.leases === 1);
  host.emit('agent_start', undefined, session.ctx);
  await until('the start', () => server.frames().length === 1);
  host.emit('agent_settled', undefined, session.ctx);
  await until('the ending', () => server.frames().length === 2);
  await host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  assert.equal(session.state.asked, 0);
  assert.deepEqual(server.attests()[0].capabilities, { signal: true, lifecycle: false });
  assert.ok(server.lines().every(line => !('nativeSessionId' in line) && !('event' in line)));
  assert.deepEqual(host.touched, []);
  await closed(server);
});

test('the session a runtime starts in, what the API says under a later lease, and its teardown', async () => {
  const { server, host, session } = await observing('both', 'session-one', 'resume');
  await until('the start', () => server.frames().length === 1);
  assert.deepEqual(server.attests()[0].capabilities, { signal: true, lifecycle: true });
  assert.deepEqual(plain(server.frames()[0]), { ...common, lease: 1, seq: 1, event: 'start', nativeSessionId: 'session-one', reason: 'resume' });
  // both capabilities are ONE lease with one numbering
  host.emit('agent_start', undefined, session.ctx);
  await until('the run', () => server.frames().length === 2);
  assert.deepEqual(plain(server.frames()[1]), { ...common, lease: 1, seq: 2, run: 1, state: 'working' });
  // the lease is lost; the next one says what the API says NOW, as a snapshot, never a start again
  session.state.id = 'session-two';
  forgotten(server, host, session);
  await until('the second lease', () => server.leases === 2, 3000);
  await until('its frames', () => server.frames().length === 2);
  assert.deepEqual(server.frames().map(plain), [
    { ...common, lease: 2, seq: 1, event: 'snapshot', nativeSessionId: 'session-two' },
    { ...common, lease: 2, seq: 2, run: 1, state: 'working' },
  ]);
  // the teardown: the id is read once more, then the shutdown and the reset with Pi's reason
  session.state.id = 'session-three';
  await host.emit('session_shutdown', { reason: 'fork' }, session.ctx);
  assert.deepEqual(server.frames().slice(2).map(plain), [
    { ...common, lease: 2, seq: 3, event: 'shutdown', nativeSessionId: 'session-three', reason: 'fork' },
    { ...common, lease: 2, seq: 4, reason: 'fork' },
  ]);
  assert.deepEqual(host.touched, [], 'nothing but the mode and the session id was read');
  await closed(server);
});

test('session observation alone subscribes to no run and reports none', async () => {
  const { server, host, session } = await observing('lifecycle', 'fixture.custom-ID_123');
  assert.deepEqual([...host.handlers.keys()], ['session_start', 'session_shutdown']);
  await until('the start', () => server.frames().length === 1);
  assert.deepEqual(server.attests()[0].capabilities, { signal: false, lifecycle: true });
  assert.deepEqual(plain(server.frames()[0]), { ...common, lease: 1, seq: 1, event: 'start', nativeSessionId: 'fixture.custom-ID_123', reason: 'startup' });
  await host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  assert.deepEqual(server.frames().slice(1).map(frame => frame.event ?? `reset:${frame.reason}`), ['shutdown', 'reset:quit']);
  await closed(server);
});

test('an id or a reason the observer cannot vouch for is not sent', async () => {
  // a start reason Pi does not have: only what the API says is reported, as a snapshot
  const odd = await observing('lifecycle', 'session-one', 'restored');
  await until('the snapshot', () => odd.server.frames().length === 1);
  assert.deepEqual(plain(odd.server.frames()[0]), { ...common, lease: 1, seq: 1, event: 'snapshot', nativeSessionId: 'session-one' });
  // a shutdown reason Pi does not have: no shutdown is claimed, the lease is withdrawn as a fault
  await odd.host.emit('session_shutdown', { reason: 'exit' }, odd.session.ctx);
  assert.deepEqual(odd.server.frames().slice(1).map(plain), [{ ...common, lease: 1, seq: 2, reason: 'fault' }]);
  await closed(odd.server);
  // no id, an id that is no string, a manager that throws: there is nothing to say of a session
  for (const id of ['', null, 42, new Error('no session')]) {
    const { server, host, session } = await observing('lifecycle', id);
    await until('the lease', () => server.leases === 1);
    await sleep(150);
    assert.deepEqual(server.frames(), [], String(id).slice(0, 12));
    await host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
    assert.deepEqual(server.frames().map(plain), [{ ...common, lease: 1, seq: 1, reason: 'quit' }]);
    await closed(server);
  }
  // the longest id that is sent at all (Deck keeps at most 256 bytes and says so itself)
  const long = await observing('lifecycle', 'a'.repeat(512));
  await until('the start', () => long.server.frames().length === 1);
  assert.equal(long.server.frames()[0].nativeSessionId.length, 512);
  assert.ok(JSON.stringify({ ...long.server.frames()[0], lease: 2 ** 53 - 1, seq: 2 ** 53 - 1 }).length <= 1024);
  await long.host.emit('session_shutdown', { reason: 'quit' }, long.session.ctx);
  await closed(long.server);
});

test('an id that does not fit a line is said in one closed word, once, and never passed over', async () => {
  const { server, host, session } = await observing('both', 'a'.repeat(513));
  await until('the word', () => server.frames().length === 1);
  // in place of the start: no id, no part of it, no length, no reason
  assert.deepEqual(plain(server.frames()[0]), { ...common, lease: 1, seq: 1, unavailable: 'unsupported-native-id' });
  // run boundaries of the same lease are not affected
  host.emit('agent_start', undefined, session.ctx);
  await until('the run', () => server.frames().length === 2);
  assert.deepEqual(plain(server.frames()[1]), { ...common, lease: 1, seq: 2, run: 1, state: 'working' });
  // a later lease says it again: it is what the API says now
  forgotten(server, host, session);
  await until('the second lease', () => server.leases === 2, 3000);
  await until('its frames', () => server.frames().length === 2);
  assert.deepEqual(server.frames().map(plain), [
    { ...common, lease: 2, seq: 1, unavailable: 'unsupported-native-id' },
    { ...common, lease: 2, seq: 2, run: 1, state: 'working' },
  ]);
  // after that word this lease says nothing more of sessions: the id is not even read at the teardown
  const asked = session.state.asked;
  session.state.id = 'session-short';
  await host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  assert.equal(session.state.asked, asked, 'not asked again');
  assert.deepEqual(server.frames().slice(2).map(plain), [{ ...common, lease: 2, seq: 3, reason: 'quit' }]);
  assert.ok(server.lines().every(line => !('nativeSessionId' in line) && !JSON.stringify(line).includes('aaaa')));
  assert.deepEqual(host.touched, []);
  await closed(server);

  // an id that turns too long only at the teardown: the word instead of the shutdown, then the reset
  const late = await observing('lifecycle', 'session-one');
  await until('the start', () => late.server.frames().length === 1);
  late.session.state.id = 'b'.repeat(2000);
  await late.host.emit('session_shutdown', { reason: 'new' }, late.session.ctx);
  assert.deepEqual(late.server.frames().slice(1).map(plain), [
    { ...common, lease: 1, seq: 2, unavailable: 'unsupported-native-id' },
    { ...common, lease: 1, seq: 3, reason: 'new' },
  ]);
  await closed(late.server);
});

test('an id that is too long AND not a session id at all is an error, not that word', async () => {
  const { server, host, session } = await observing('both', `../${'a'.repeat(600)}`);
  await sleep(300);
  // nothing connected and nothing was said: the observer of this runtime is over
  assert.equal(server.connections.length, 0);
  host.emit('agent_start', undefined, session.ctx);
  await sleep(1300);
  assert.equal(server.connections.length, 0, 'and it does not come back');
  assert.doesNotThrow(() => host.emit('session_shutdown', { reason: 'quit' }, session.ctx));
  await closed(server);
});

test('a capability Deck takes away is neither read nor sent again, and the other goes on', async () => {
  const { server, host, session } = await observing('both', 'session-one');
  await until('the start', () => server.frames().length === 1);
  host.emit('agent_start', undefined, session.ctx);
  await until('the run', () => server.frames().length === 2);
  const asked = session.state.asked;
  // Deck: session observation is turned off, in the middle of a run. The
  // observer hears it in the answer to its next frame.
  server.usable = { signal: true, lifecycle: false };
  // the run goes on to its end under the same lease, numbered without a gap
  host.emit('agent_settled', undefined, session.ctx);
  await until('the ending', () => server.frames().length === 3);
  assert.deepEqual(plain(server.frames()[2]), { ...common, lease: 1, seq: 3, run: 1, state: 'turn-done' });
  host.emit('agent_start', undefined, session.ctx);
  await until('the next run', () => server.frames().length === 4);
  assert.deepEqual(plain(server.frames()[3]), { ...common, lease: 1, seq: 4, run: 2, state: 'working' });
  // a later lease of this runtime claims only what is left, and says nothing of a session
  forgotten(server, host, session);
  await until('the second lease', () => server.leases === 2, 3000);
  await until('its run', () => server.frames().length === 1);
  assert.deepEqual(server.attests()[1].capabilities, { signal: true, lifecycle: false });
  assert.deepEqual(server.frames().map(plain), [{ ...common, lease: 2, seq: 1, run: 1, state: 'working' }]);
  // the teardown does not ask for the id either
  await host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  assert.equal(session.state.asked, asked, 'the session manager is not asked again');
  assert.deepEqual(server.frames().slice(1).map(plain), [{ ...common, lease: 2, seq: 2, reason: 'quit' }]);
  await closed(server);
});

test('losing work status leaves session observation, and losing both ends the observer', async () => {
  const { server, host, session } = await observing('both', 'session-one');
  await until('the start', () => server.frames().length === 1);
  host.emit('agent_start', undefined, session.ctx);
  await until('the run', () => server.frames().length === 2);
  // work status is turned off: the run's ending was on its way and is answered `revoked`
  server.usable = { signal: false, lifecycle: true };
  server.next.push(line => ({ revoked: line.seq, nonce: server.give(line.lease), capabilities: server.usable }));
  host.emit('agent_settled', undefined, session.ctx);
  await until('that frame', () => server.frames().length === 3);
  // no later run is sent
  host.emit('agent_start', undefined, session.ctx);
  host.emit('agent_settled', undefined, session.ctx);
  await sleep(150);
  assert.equal(server.frames().length, 3, 'no run boundary after the loss');
  // the session's teardown is what is left, numbered after the frame that was not committed
  await host.emit('session_shutdown', { reason: 'fork' }, session.ctx);
  assert.deepEqual(server.frames().slice(3).map(plain), [
    { ...common, lease: 1, seq: 4, event: 'shutdown', nativeSessionId: 'session-one', reason: 'fork' },
    { ...common, lease: 1, seq: 5, reason: 'fork' },
  ]);
  await closed(server);

  // both are taken: Deck ends the lease, and this runtime does not bind again
  const last = await observing('both', 'session-one');
  await until('the start', () => last.server.frames().length === 1);
  last.server.next.push(() => ({ refused: 'capability-disabled' }));
  last.host.emit('agent_start', undefined, last.session.ctx);
  await until('the refused frame', () => last.server.frames().length === 2);
  last.host.emit('agent_settled', undefined, last.session.ctx);
  await sleep(1300);
  assert.equal(last.server.lines().length, 4, 'no further line in this runtime');
  await closed(last.server);
});

test('nothing Deck says gives a capability back, and a frame answered `revoked` is answered', async () => {
  const { server, host, session } = await observing('both', 'session-one', 'startup', 'manual');
  server.answer(await server.arrival(0));
  server.answer(await server.arrival(1));
  const start = await server.arrival(2);
  // the start crossed a change of the settings: answered as not committed, with what is left
  server.say(start, { revoked: 1, nonce: server.give(1), capabilities: { signal: true, lifecycle: false } });
  host.emit('agent_start', undefined, session.ctx);
  const run = await server.arrival(3);
  assert.deepEqual(plain(run.line), { ...common, lease: 1, seq: 2, run: 1, state: 'working' });
  // an answer that names MORE than the observer has changes nothing: only ever less
  server.say(run, { ack: 2, nonce: server.give(1), capabilities: { signal: true, lifecycle: true } });
  await sleep(60);
  const asked = session.state.asked;
  const over = host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  const reset = await server.arrival(4);
  assert.deepEqual(plain(reset.line), { ...common, lease: 1, seq: 3, reason: 'quit' });
  server.answer(reset);
  await over;
  assert.equal(session.state.asked, asked, 'session observation did not come back');
  await closed(server);
});

test('bound with less than this runtime loaded: said with the acceptance, and nothing of the rest is sent', async () => {
  const { server, host, session } = await observing('both', 'session-one', 'startup', 'manual');
  server.answer(await server.arrival(0));
  const attest = await server.arrival(1);
  assert.deepEqual(attest.line.capabilities, { signal: true, lifecycle: true }, 'what this runtime loaded');
  // the session this runtime started in was read once, inside Pi, before any connection
  const asked = session.state.asked;
  // Deck's settings enable work status alone
  server.usable = { signal: true, lifecycle: false };
  server.answer(attest);
  assert.deepEqual(attest.said.capabilities, { signal: true, lifecycle: false });
  await sleep(120);
  assert.equal(server.connections.length, 2, 'no start frame: the session id never crosses the socket');
  // work status goes on, numbered from 1
  host.emit('agent_start', undefined, session.ctx);
  const run = await server.arrival(2);
  assert.deepEqual(plain(run.line), { ...common, lease: 1, seq: 1, run: 1, state: 'working' });
  server.answer(run);
  const over = host.emit('session_shutdown', { reason: 'quit' }, session.ctx);
  const reset = await server.arrival(3);
  assert.deepEqual(plain(reset.line), { ...common, lease: 1, seq: 2, reason: 'quit' });
  server.answer(reset);
  await over;
  assert.equal(session.state.asked, asked, 'the session manager is not asked again');
  assert.deepEqual(host.touched, []);
  await closed(server);
});

/* ---------- the one connection ---------- */

test('whichever observer asks, one connection exists at a time and each is closed before the next is made', async () => {
  const server = deck('line');
  await server.listening;
  route(routed(server));
  // three observers of this module in one process, each with its own lease
  const hosts = [pi(), pi(), pi()];
  for (const host of hosts) {
    entry(host.api);
    host.emit('session_start');
  }
  await until('three leases', () => server.leases === 3);
  for (let round = 1; round <= 10; round += 1) {
    for (const host of hosts) {
      host.emit('agent_start');
      host.emit('agent_settled');
    }
    await until(`round ${round}`, () => [1, 2, 3].every(lease => server.frames(lease).length === 2 * round));
  }
  await Promise.all(hosts.map(host => host.emit('session_shutdown', { reason: 'quit' })));
  for (const lease of [1, 2, 3]) {
    assert.deepEqual(server.frames(lease).map(line => line.seq), Array.from({ length: 21 }, (_, at) => at + 1));
  }
  assert.ok(server.connections.every(connection => !connection.more));
  await closed(server);
  // and so it was for every connection this file's observers made
  assert.equal(live.size, 0, 'none is left open');
  assert.equal(seen.most, 1, `of ${seen.made} connections, at most one existed at a time`);
  assert.ok(seen.made > 200, `${seen.made} connections were made`);
});
