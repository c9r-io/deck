// observer.ts — Deck's read-only observer for the Pi coding agent.
//
// Pi loads the entry beside this file (`signal.ts`) from the signed Deck
// bundle; it calls `deckPiObserver` with its capability bits and nothing
// else. This file is the whole implementation. The backend half and the wire
// contract are `app/src-tauri/src/agent_pi.rs`.
//
// # Contract
// - Content-free. It subscribes to `session_start`, `session_shutdown` and,
//   with the signal capability, `agent_start` and `agent_settled`. It never
//   subscribes to an event that carries a prompt, messages, a title, a tool
//   call or a result, and never reads one. The session manager is never
//   touched: no session id, path, cwd, model, transcript or environment
//   value other than the three routing variables is read.
// - Observation only. It registers no tool, command, provider, shortcut or
//   flag, sends no message, starts, switches or forks no session, runs no
//   tool or process, and opens no network connection. Its one output is one
//   stream connection to Deck's private status socket.
// - Routing is `DECK_STATUS_SOCK`, `TMUX` and `TMUX_PANE`, which Deck's own
//   tmux server exports to its panes. All three must be present and well
//   formed; there is no fallback path. The socket path is used only as a Unix
//   path: it must be absolute, end in `/status.sock`, fit a socket address,
//   name a socket owned by this user with mode 0600 in a directory of this
//   user that nobody else can enter. These are routing claims, not proof:
//   Deck proves who connected from the kernel.
// - Only an interactive terminal session (`ctx.mode === "tui"`) connects.
//   The factory registers callbacks and starts nothing; the connection is
//   opened by `session_start`.
// - Protocol `pi-observer` v1, one JSON line per frame, at most 1024 bytes.
//   `hello` first; then `signal`, `reset` and `ping`, numbered from 1
//   without a gap. ONE frame waits for its acknowledgement at a time.
//   An acknowledgement that takes more than two seconds, a refusal, an
//   unexpected line or a full queue closes the connection and forgets what
//   was queued: nothing queued is ever sent on a later connection.
// - A run is the interval from `agent_start` to `agent_settled`. A second
//   `agent_start` before the settlement (a retry, a queued follow-up, a
//   continuation) is the same run. `agent_settled` without a run reports
//   nothing. Run numbers belong to one connection: a connection accepted in
//   the middle of a run reports that run as its run 1, and a connection
//   accepted while idle reports nothing until the next start. An ending
//   that happened while disconnected is never reported afterwards.
// - `session_shutdown` marks this runtime disposed before anything else:
//   timers stop, later callbacks do nothing, and nothing reconnects. The
//   last frame (`reset`, with Pi's own reason) gets at most 100 ms; then the
//   socket is closed whatever was acknowledged. Deck treats a missing last
//   frame as a lost connection.
// - A callback never throws into Pi and never waits for the socket. An
//   internal error disposes this runtime's observer. The factory does not
//   throw either: Pi refuses to start when an extension fails to load, so
//   if a callback cannot be registered the observer is disposed and Pi
//   starts as if this entry were absent.
// - Reconnecting waits 1, 2, 5, then 10 seconds, on timers that do not keep
//   Pi alive. A refusal that cannot change while this runtime lives (this
//   capability is turned off in Deck, this Pi release is not supported)
//   ends the observer until Pi loads its extensions again.
//
// The files here are JavaScript in TypeScript's syntax subset on purpose:
// Pi loads them as they are, and Deck's gates check them without a compiler.
import fs from "node:fs";
import net from "node:net";

const PROTOCOL = "pi-observer";
const WIRE_VERSION = 1;
const OBSERVER_VERSION = 1;
const MAX_LINE = 1024;
const ACK_TIMEOUT_MS = 2000;
const PING_EVERY_MS = 5000;
const RECONNECT_MS = [1000, 2000, 5000, 10000];
const SHUTDOWN_DRAIN_MS = 100;
/* frames that may wait behind the one that awaits its acknowledgement */
const QUEUE_LIMIT = 8;
/* a Unix socket address holds 104 bytes on macOS, the terminator included */
const MAX_SOCKET_PATH = 103;

const SHUTDOWN_REASONS = ["quit", "reload", "new", "resume", "fork"];
/* refusals that hold for as long as this runtime lives */
const FINAL_REFUSALS = [
  "capability-disabled", "unsupported-pi-version", "bad-frame", "bad-version", "bad-mode", "other-server",
];

/** The route Deck's tmux server gave this pane, or null. */
export function route(env) {
  const path = env.DECK_STATUS_SOCK;
  const tmux = env.TMUX;
  const pane = env.TMUX_PANE;
  if (typeof path !== "string" || typeof tmux !== "string" || typeof pane !== "string") return null;
  if (!path.startsWith("/") || !path.endsWith("/status.sock") || path.includes("\0")) return null;
  if (Buffer.byteLength(path, "utf8") > MAX_SOCKET_PATH) return null;
  if (!/^%[0-9]{1,9}$/.test(pane)) return null;
  const fields = tmux.split(",");
  if (fields.length < 2) return null;
  const socket = fields[0].slice(fields[0].lastIndexOf("/") + 1);
  if (!/^[A-Za-z0-9._-]{1,64}$/.test(socket) || !/^[0-9]{1,10}$/.test(fields[1])) return null;
  const serverPid = Number(fields[1]);
  if (!(serverPid > 1 && serverPid <= 0xffffffff)) return null;
  return { path, socket, serverPid, pane };
}

/** `path` names a socket of this user, mode 0600, in a directory of this
 *  user that no one else can enter. */
export function privateSocket(path) {
  try {
    const uid = process.getuid();
    const socket = fs.lstatSync(path);
    const directory = fs.statSync(path.slice(0, path.lastIndexOf("/")) || "/");
    return socket.isSocket() && socket.uid === uid && (socket.mode & 0o777) === 0o600
      && directory.isDirectory() && directory.uid === uid && (directory.mode & 0o077) === 0;
  } catch (_) {
    return false;
  }
}

const closedWord = (words, word) => (words.includes(word) ? word : null);

/** The extension factory of one entry. `piVersion` is Pi's exported VERSION. */
export function deckPiObserver(capabilities, piVersion) {
  const signal = capabilities.signal === true;
  return function deckPiObserverFactory(pi) {
    let disposed = false;
    let ended = false; // a final refusal: nothing reconnects in this runtime
    let target = null;
    let active = false; // a run started and has not settled
    let socket = null;
    let accepted = false;
    let buffer = "";
    let seq = 0;
    let run = 0;
    let queue = [];
    let awaiting = null; // { seq, timer }; seq 0 is the hello
    let ping = null;
    let retry = null;
    let attempt = 0;
    let drained = null;

    const stopTimers = () => {
      if (ping) clearInterval(ping);
      if (retry) clearTimeout(retry);
      ping = null;
      retry = null;
    };

    const forget = () => {
      if (awaiting) clearTimeout(awaiting.timer);
      awaiting = null;
      queue = [];
      buffer = "";
      accepted = false;
    };

    /* this connection is over: nothing of it is kept or sent again */
    const drop = (which) => {
      if (socket !== which) return;
      socket = null;
      forget();
      if (ping) clearInterval(ping);
      ping = null;
      try { which.destroy(); } catch (_) { /* already closed */ }
      if (drained) drained();
      else if (!disposed && !ended) reconnect();
    };

    const reconnect = () => {
      if (disposed || ended || retry || socket) return;
      const wait = RECONNECT_MS[Math.min(attempt, RECONNECT_MS.length - 1)];
      attempt += 1;
      retry = setTimeout(() => {
        retry = null;
        connect();
      }, wait);
      retry.unref();
    };

    const write = (which, frame, number) => {
      const line = JSON.stringify({ protocol: PROTOCOL, v: WIRE_VERSION, source: "pi", ...frame });
      if (line.length > MAX_LINE) return false;
      const timer = setTimeout(() => drop(which), ACK_TIMEOUT_MS);
      timer.unref();
      awaiting = { seq: number, timer };
      which.write(`${line}\n`);
      return true;
    };

    const pump = () => {
      while (socket && accepted && !awaiting && queue.length) {
        const frame = queue.shift();
        // a frame that cannot be written is not numbered: the next one takes its place
        if (write(socket, { seq: seq + 1, ...frame }, seq + 1)) seq += 1;
      }
      if (drained && socket && !awaiting && !queue.length) drained();
    };

    const push = (frame) => {
      if (!socket || !accepted) return;
      if (queue.length >= QUEUE_LIMIT) {
        drop(socket);
        return;
      }
      queue.push(frame);
      pump();
    };

    const onAccepted = () => {
      accepted = true;
      attempt = 0;
      if (signal && active) {
        run = 1;
        push({ run, state: "working" });
      }
      if (ping) clearInterval(ping);
      ping = setInterval(() => {
        if (accepted && !awaiting && !queue.length) push({});
      }, PING_EVERY_MS);
      ping.unref();
    };

    const onLine = (which, line) => {
      let reply = null;
      try { reply = JSON.parse(line); } catch (_) { reply = null; }
      if (!reply || typeof reply !== "object" || reply.protocol !== PROTOCOL || reply.v !== WIRE_VERSION) {
        drop(which);
        return;
      }
      // a refusal may arrive at any time: Deck revokes a stream that way too
      if (typeof reply.refused === "string") {
        if (FINAL_REFUSALS.includes(reply.refused)) ended = true;
        drop(which);
        return;
      }
      const waited = awaiting;
      const answered = !!waited && (waited.seq === 0 ? reply.accepted === true : reply.ack === waited.seq);
      if (!answered) {
        drop(which);
        return;
      }
      clearTimeout(waited.timer);
      awaiting = null;
      if (waited.seq === 0) onAccepted();
      pump();
    };

    const connect = () => {
      if (disposed || ended || socket || !target) return;
      if (!privateSocket(target.path)) {
        reconnect();
        return;
      }
      let which = null;
      try {
        which = net.createConnection({ path: target.path });
      } catch (_) {
        reconnect();
        return;
      }
      socket = which;
      seq = 0;
      run = 0;
      forget();
      which.setEncoding("utf8");
      // this socket never keeps Pi alive
      which.unref();
      which.on("connect", () => {
        if (socket !== which) return;
        const hello = {
          socket: target.socket,
          server_pid: target.serverPid,
          pane: target.pane,
          observerVersion: OBSERVER_VERSION,
          piVersion,
          mode: "tui",
          capabilities: { signal, lifecycle: false },
        };
        if (!write(which, hello, 0)) drop(which);
      });
      which.on("data", (chunk) => {
        if (socket !== which) return;
        buffer += chunk;
        if (buffer.length > 4 * MAX_LINE) {
          drop(which);
          return;
        }
        for (let at = buffer.indexOf("\n"); at >= 0 && socket === which; at = buffer.indexOf("\n")) {
          const line = buffer.slice(0, at);
          buffer = buffer.slice(at + 1);
          onLine(which, line);
        }
      });
      which.on("error", () => drop(which));
      which.on("close", () => drop(which));
    };

    const dispose = () => {
      disposed = true;
      stopTimers();
    };

    /* an internal error ends this runtime's observer and never reaches Pi */
    const guarded = (work) => (event, ctx) => {
      if (disposed) return undefined;
      try {
        return work(event, ctx);
      } catch (_) {
        const which = socket;
        dispose();
        if (which) drop(which);
        return undefined;
      }
    };

    const onStart = guarded((_event, ctx) => {
      if (target || ctx.mode !== "tui") return;
      if (typeof piVersion !== "string" || !/^[0-9A-Za-z.+-]{1,32}$/.test(piVersion)) return;
      target = route(process.env);
      if (!target) return;
      connect();
    });

    const onAgentStart = guarded(() => {
      if (!target || active) return;
      active = true;
      if (!accepted) return;
      run += 1;
      push({ run, state: "working" });
    });

    const onAgentSettled = guarded(() => {
      if (!target || !active) return;
      active = false;
      if (accepted && run > 0) push({ run, state: "turn-done" });
    });

    const onShutdown = guarded((event) => {
      const which = socket;
      const connected = accepted;
      dispose();
      if (!which) return undefined;
      if (!connected) {
        drop(which);
        return undefined;
      }
      const reason = closedWord(SHUTDOWN_REASONS, event.reason);
      // what was queued and not sent belongs to a runtime that is over
      queue = [];
      queue.push({ reason: reason || "fault" });
      return new Promise((resolve) => {
        let finished = false;
        const finish = () => {
          if (finished) return;
          finished = true;
          clearTimeout(limit);
          drained = null;
          if (socket === which) {
            socket = null;
            forget();
          }
          try { which.destroy(); } catch (_) { /* already closed */ }
          resolve();
        };
        const limit = setTimeout(finish, SHUTDOWN_DRAIN_MS);
        drained = finish;
        pump();
      });
    });

    try {
      pi.on("session_start", onStart);
      if (signal) {
        pi.on("agent_start", onAgentStart);
        pi.on("agent_settled", onAgentSettled);
      }
      pi.on("session_shutdown", onShutdown);
    } catch (_) {
      // registered or not, every callback of a disposed observer does nothing
      dispose();
    }
  };
}
