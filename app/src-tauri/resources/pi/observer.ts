// observer.ts — Deck's read-only observer for the Pi coding agent.
//
// Pi loads ONE of the three entries beside this file (`signal.ts`,
// `lifecycle.ts`, `both.ts`) from the signed Deck bundle; each calls
// `deckPiObserver` with its two capability bits and nothing else. This file is
// the whole implementation. The backend half and the wire contract are
// `app/src-tauri/src/agent_pi.rs`.
//
// # Contract
// - Content-free. It subscribes to `session_start`, `session_shutdown` and,
//   with the signal capability, `agent_start` and `agent_settled`. It never
//   subscribes to an event that carries a prompt, messages, a title, a tool
//   call or a result, and never reads one. With the lifecycle capability it
//   reads ONE value through Pi's public API, the native session id; without
//   it the session manager is never touched. No path, cwd, model, transcript
//   or environment value other than the three routing variables is read.
// - Observation only. It registers no tool, command, provider, shortcut or
//   flag, sends no message, starts, switches or forks no session, runs no
//   tool or process, and opens no network connection. Its one output is
//   short connections to Deck's private status socket, one per message.
// - Routing is `DECK_STATUS_SOCK`, `TMUX` and `TMUX_PANE`, which Deck's own
//   tmux server exports to its panes. All three must be present and well
//   formed; there is no fallback path. The socket path is used only as a Unix
//   path: it must be absolute, end in `/status.sock`, fit a socket address,
//   name a socket owned by this user with mode 0600 in a directory of this
//   user that nobody else can enter. These are routing claims, not proof:
//   Deck proves who connected from the kernel.
// - Only an interactive terminal session (`ctx.mode === "tui"`) speaks.
//   The factory registers callbacks and starts nothing; the first message is
//   sent by `session_start`.
// - Protocol `pi-observer` v2, one JSON line per message, at most 1024
//   bytes, and ONE message per connection: connect, write the line, read
//   Deck's one line of answer, close. No connection is ever kept or used
//   twice, and no second one is made while one exists — for every observer
//   this module made, whichever asks (`line`). Closing is immediate on both
//   runtimes Pi ships on (measured), so a child that Pi makes at any instant
//   starts with at most one of these connections, and that one carries one
//   message. Deck does not take that on trust: it believes a line only on a
//   connection that was made after it last looked at this process
//   (`agent_pi/transport.rs`, "Whose bytes"). One connection at a time is
//   what keeps being asked `again` the exception.
// - Every answer of Deck's that is not a refusal carries a nonce, and the
//   next message carries it back; it is kept in this closure and nowhere
//   else. Binding takes two messages. `hello` carries nothing and is
//   answered with a nonce; `attest` carries it with this runtime's claims
//   (pane, versions, mode, the capabilities it loaded). Deck answers
//   `accepted` with a lease number and the capabilities that lease may use.
//   Every frame after that names the lease and is numbered from 1 without a
//   gap: `signal`, `lifecycle`, `unavailable`, `reset` and `ping`. ONE frame
//   waits for its answer at a time.
// - `again` is an answer, not an error: Deck could not tell this process's
//   line from a child's — Pi made a child for a tool, or an answer went
//   missing — and read nothing into anything. The same message is sent once
//   more with the new nonce, a few times at short intervals and then not at
//   all. An answer that takes more than two seconds, a refusal, an
//   unexpected line or a full queue ends the lease as far as this observer
//   is concerned and forgets what was queued: nothing queued is ever sent
//   under a later lease. When Deck may still hold that lease it is told so
//   in one last `reset`.
// - Deck may take a capability away while this runtime lives, and never
//   gives one back. Every answer to a frame, and the acceptance itself, says
//   what the lease may still do: from then on nothing of a lost capability
//   is read, queued or sent — without the lifecycle capability the session
//   manager is not touched again — and what is left goes on under the same
//   lease. A frame that was on its way is answered `revoked` instead of
//   `ack`; that is an answer, not an error. A later lease of this runtime
//   claims only what is left. Getting a capability back takes Pi loading its
//   extensions again.
// - A run is the interval from `agent_start` to `agent_settled`. A second
//   `agent_start` before the settlement (a retry, a queued follow-up, a
//   continuation) is the same run. `agent_settled` without a run reports
//   nothing. Run numbers belong to one lease: a lease accepted in the middle
//   of a run reports that run as its run 1, and a lease accepted while idle
//   reports nothing until the next start. An ending that happened without a
//   lease is never reported afterwards.
// - With the lifecycle capability the first lease reports the session this
//   runtime started in (`start`, with Pi's own reason); a later lease
//   reports only what the API says now (`snapshot`), never a start again.
//   `session_shutdown` reads the id once more, then reports `shutdown` with
//   Pi's reason. An id is sent whole or not at all: one that does not fit a
//   line is reported as the closed word `unsupported-native-id` in place of
//   the frame that would have carried it — never cut short, never hashed,
//   never passed over in silence — and after that word this lease says
//   nothing more about sessions. An id that does not even have the shape of
//   a Pi session id is this observer's error like any other: it ends the
//   observer and reports nothing.
// - `session_shutdown` marks this runtime disposed before anything else:
//   timers stop, later callbacks do nothing, and nothing binds again. The
//   last frames (`shutdown` with the lifecycle capability, then `reset`,
//   each with Pi's own reason) get at most 100 ms; then this observer's
//   connection, if it has one, is closed and it never makes another — a
//   runtime Pi loads afterwards meets none of this one's. Deck ends a lease
//   that says nothing for fifteen seconds, and at once when its process is
//   gone.
// - A callback never throws into Pi and never waits for the socket. An
//   internal error disposes this runtime's observer. The factory does not
//   throw either: Pi refuses to start when an extension fails to load, so
//   if a callback cannot be registered the observer is disposed and Pi
//   starts as if this entry were absent.
// - Binding again waits 1, 2, 5, then 10 seconds, on timers that do not keep
//   Pi alive. A refusal that cannot change while this runtime lives (this
//   capability is turned off in Deck, this Pi release is not supported, Deck
//   speaks another version of this protocol) ends the observer until Pi
//   loads its extensions again. So does being told for twenty seconds on end
//   that another observer of this process holds the lease: a lease this
//   process let go of is over within fifteen, so that one is held by a
//   second copy of Deck's entry in the same Pi — which Deck's Settings call
//   a conflict and never write — and two observers in one process would be
//   two connections at a time.
//
// The four files are JavaScript in TypeScript's syntax subset on purpose:
// Pi loads them as they are, and Deck's gates check them without a compiler.
import fs from "node:fs";
import net from "node:net";

const PROTOCOL = "pi-observer";
const WIRE_VERSION = 2;
const OBSERVER_VERSION = 2;
const MAX_LINE = 1024;
const ACK_TIMEOUT_MS = 2000;
const PING_EVERY_MS = 5000;
const RECONNECT_MS = [1000, 2000, 5000, 10000];
/* how long after an `again` the same message is sent once more; then not at all */
const AGAIN_MS = [0, 2, 5, 10, 20, 50];
const SHUTDOWN_DRAIN_MS = 100;
/* frames that may wait behind the one that awaits its answer */
const QUEUE_LIMIT = 8;
/* messages that may wait for the one connection; one more is not sent */
const WAITING_LIMIT = 8;
/* how long this runtime goes on asking while another observer of its process holds the lease */
const DUPLICATE_MS = 20000;
/* a Unix socket address holds 104 bytes on macOS, the terminator included */
const MAX_SOCKET_PATH = 103;
/* an id longer than this does not fit a line: Deck is told so in one closed
   word instead. Deck itself keeps at most 256 bytes and says so on the card. */
const MAX_NATIVE_ID = 512;
const UNSUPPORTED_NATIVE_ID = "unsupported-native-id";
/* the shape Pi itself gives a session id */
const NATIVE_ID_SHAPE = /^[A-Za-z0-9](?:[A-Za-z0-9._-]*[A-Za-z0-9])?$/;

const START_REASONS = ["startup", "reload", "new", "resume", "fork"];
const SHUTDOWN_REASONS = ["quit", "reload", "new", "resume", "fork"];
/* refusals that hold for as long as this runtime lives */
const FINAL_REFUSALS = [
  "capability-disabled", "unsupported-pi-version", "bad-frame", "bad-version", "bad-mode", "other-server",
];
/* refusals that say nothing against the message: the same one may be sent again */
const AGAIN_REFUSALS = ["sender-mismatch", "busy", "no-peer"];
/* refusals after which Deck may still hold the lease */
const UNTOUCHED_REFUSALS = ["bad-frame", "bad-version", "sender-mismatch", "busy", "no-peer"];

/* The one connection that exists right now for every observer this module
   made, and the messages that wait for it to be gone (file header). */
const line = { open: null, waiting: [] };

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

/** What Pi gave as its session id: `{ id }`, `{ unsupported: true }` for an
 *  id that does not fit a line, `null` for none, and a throw for one that is
 *  no Pi session id at all. */
function nativeIdOf(value) {
  if (typeof value !== "string" || value.length === 0) return null;
  if (value.length <= MAX_NATIVE_ID) return { id: value };
  if (!NATIVE_ID_SHAPE.test(value)) throw new Error("native id");
  return { unsupported: true };
}

/** The two capability bits of an answer, or null when they are not exactly that. */
function bitsOf(value) {
  return !!value && typeof value === "object" && Object.keys(value).length === 2
    && typeof value.signal === "boolean" && typeof value.lifecycle === "boolean" ? value : null;
}

/** The nonce of an answer, or null when it carries none. */
function nonceOf(reply) {
  return !!reply && typeof reply.nonce === "string" && /^[0-9a-f]{32}$/.test(reply.nonce) ? reply.nonce : null;
}

/** The closed word of a refusal, or null for any other answer. */
function refusalOf(reply) {
  return !!reply && typeof reply.refused === "string" ? reply.refused : null;
}

const textOf = (message) => JSON.stringify({ protocol: PROTOCOL, v: WIRE_VERSION, source: "pi", ...message });

const later = (wait, work) => {
  const timer = setTimeout(work, wait);
  timer.unref();
  return timer;
};

/** The extension factory of one entry. `piVersion` is Pi's exported VERSION. */
export function deckPiObserver(capabilities, piVersion) {
  const signal = capabilities.signal === true;
  const lifecycle = capabilities.lifecycle === true;
  return function deckPiObserverFactory(pi) {
    let disposed = false;
    let ended = false; // a final refusal: nothing binds again in this runtime
    let sealed = false; // the shutdown is over: no connection is made again
    // what this runtime may still do: Deck only ever takes away
    const may = { signal, lifecycle };
    let target = null;
    let sessions = null; // this runtime's session manager, lifecycle only
    let first = null; // the start this runtime may report under its first lease
    let silent = false; // this lease said that its session id cannot be sent
    let active = false; // a run started and has not settled
    let lease = null; // the number Deck accepted this observer under
    let nonce = null; // what Deck's last answer said the next message must carry
    let binding = false; // a hello or an attest is on its way
    let busy = false; // a frame is on its way
    let own = null; // this observer's connection, while it is the one that exists
    let seq = 0;
    let run = 0;
    let queue = [];
    let ping = null;
    let retry = null;
    let attempt = 0;
    let duplicate = null; // since when another observer of this process has held the lease
    let drained = null;

    const nativeId = (manager) => {
      let id = null;
      try {
        id = manager.getSessionId();
      } catch (_) {
        return null;
      }
      return nativeIdOf(id);
    };

    /* one session frame, or the closed word for an id that cannot be sent */
    const session = (native, frame) => {
      if (!native || silent) return null;
      if (native.unsupported) {
        silent = true;
        return { unavailable: UNSUPPORTED_NATIVE_ID };
      }
      return { ...frame, nativeSessionId: native.id };
    };

    const stopTimers = () => {
      if (ping) clearInterval(ping);
      if (retry) clearTimeout(retry);
      ping = null;
      retry = null;
    };

    /* Deck took a capability away: nothing of it is read, queued or sent again */
    const narrow = (now) => {
      const lost = { signal: may.signal && !now.signal, lifecycle: may.lifecycle && !now.lifecycle };
      if (lost.signal) {
        may.signal = false;
        active = false;
      }
      if (lost.lifecycle) {
        may.lifecycle = false;
        sessions = null;
        first = null;
      }
      queue = queue.filter((frame) => !(lost.signal && "state" in frame)
        && !(lost.lifecycle && ("event" in frame || "unavailable" in frame)));
    };

    const dispose = () => {
      disposed = true;
      stopTimers();
      first = null;
      sessions = null;
    };

    /* One message: a connection of its own, one line, Deck's one line of
       answer, the end. `done` hears the answer, or null when none came, and
       never before this call has returned. While another connection exists
       the message waits for it to be gone. */
    const exchange = (message, done) => {
      const heard = (reply) => {
        try {
          done(reply);
        } catch (_) {
          dispose();
        }
      };
      const start = () => {
        const text = textOf(message);
        let which = null;
        let finished = false;
        let buffer = "";
        let timer = null;
        const finish = (reply) => {
          if (finished) return;
          finished = true;
          if (timer) clearTimeout(timer);
          if (which) {
            // closed here and now: the next connection is made after this one is gone
            try { which.destroy(); } catch (_) { /* already closed */ }
            if (line.open === which) line.open = null;
            if (own === which) own = null;
          }
          heard(reply);
          // what waited for the line goes next, unless the answer's own next message took it
          if (!line.open && line.waiting.length) line.waiting.shift()();
        };
        if (sealed || text.length > MAX_LINE || !privateSocket(target.path)) {
          later(0, () => finish(null));
          return;
        }
        try {
          which = net.createConnection({ path: target.path });
        } catch (_) {
          which = null;
          later(0, () => finish(null));
          return;
        }
        line.open = which;
        own = which;
        timer = later(ACK_TIMEOUT_MS, () => finish(null));
        which.setEncoding("utf8");
        // this socket never keeps Pi alive
        which.unref();
        which.on("connect", () => {
          if (!finished) which.write(`${text}\n`);
        });
        which.on("data", (chunk) => {
          buffer += chunk;
          if (buffer.length > 4 * MAX_LINE) {
            finish(null);
            return;
          }
          const at = buffer.indexOf("\n");
          if (at < 0) return;
          let reply = null;
          try { reply = JSON.parse(buffer.slice(0, at)); } catch (_) { reply = null; }
          const ours = !!reply && typeof reply === "object" && reply.protocol === PROTOCOL
            // a refusal is read in whatever version it is said: `bad-version` is how another one says so
            && (reply.v === WIRE_VERSION || refusalOf(reply) !== null);
          finish(ours ? reply : null);
        });
        which.on("error", () => finish(null));
        which.on("close", () => finish(null));
      };
      if (!line.open) start();
      else if (line.waiting.length < WAITING_LIMIT) line.waiting.push(start);
      else later(0, () => heard(null));
    };

    /* this observer is over: its connection is closed and it makes no other */
    const seal = () => {
      sealed = true;
      lease = null;
      nonce = null;
      queue = [];
      if (own) {
        try { own.destroy(); } catch (_) { /* already closed */ }
      }
    };

    const rebind = () => {
      if (disposed || ended || retry || binding || lease !== null) return;
      const wait = RECONNECT_MS[Math.min(attempt, RECONNECT_MS.length - 1)];
      attempt += 1;
      retry = setTimeout(() => {
        retry = null;
        bind();
      }, wait);
      retry.unref();
    };

    /* Tell Deck, once, that this observer let go of a lease Deck may still
       hold: a reset in the place of the frame that would have come next. A
       frame Deck did not expect there ends the lease as well. */
    const farewell = (mine, given, number, tries, then) => {
      exchange({ lease: mine, nonce: given, seq: number, reason: "fault" }, (reply) => {
        const next = nonceOf(reply);
        if (reply && reply.again === true && next && tries < AGAIN_MS.length) {
          later(AGAIN_MS[tries], () => farewell(mine, next, number, tries + 1, then));
          return;
        }
        then();
      });
    };

    /* The lease is over as far as this observer goes: nothing of it is kept
       or sent again. `reply` is Deck's refusal when that is why. */
    const lost = (reply) => {
      const mine = lease;
      const given = nonce;
      const number = seq + 1;
      lease = null;
      nonce = null;
      busy = false;
      queue = [];
      silent = false;
      first = null;
      if (ping) clearInterval(ping);
      ping = null;
      const refused = refusalOf(reply);
      if (refused && FINAL_REFUSALS.includes(refused)) ended = true;
      const then = () => {
        if (drained) drained();
        else if (disposed) seal();
        else if (!ended) rebind();
      };
      if (mine !== null && given && (!refused || UNTOUCHED_REFUSALS.includes(refused))) {
        farewell(mine, given, number, 0, then);
      } else {
        then();
      }
    };

    const send = (frame, tries) => {
      const mine = lease;
      const number = seq + 1;
      exchange({ lease: mine, nonce, seq: number, ...frame }, (reply) => {
        // given up in the meantime: Deck has been told, or is being told
        if (lease !== mine) return;
        const next = nonceOf(reply);
        const asked = !!reply && reply.again === true && next !== null && Object.keys(reply).length === 4;
        if (asked) nonce = next;
        // not believed and not blamed: the same frame once more
        if ((asked || AGAIN_REFUSALS.includes(refusalOf(reply))) && tries < AGAIN_MS.length) {
          later(AGAIN_MS[tries], () => {
            if (lease === mine) send(frame, tries + 1);
          });
          return;
        }
        const bits = reply ? bitsOf(reply.capabilities) : null;
        // `revoked` answers a frame whose capability was taken while it was on its way
        if (asked || !bits || !next || Object.keys(reply).length !== 5
          || (reply.ack !== number && reply.revoked !== number)) {
          lost(asked ? null : reply);
          return;
        }
        seq = number;
        nonce = next;
        busy = false;
        narrow(bits);
        // a reset that was answered is the end of this lease
        if ("reason" in frame && !("event" in frame)) {
          lease = null;
          nonce = null;
          queue = [];
          if (ping) clearInterval(ping);
          ping = null;
        }
        pump();
      });
    };

    const pump = () => {
      while (lease !== null && !busy && queue.length) {
        const frame = queue.shift();
        // a frame that cannot be written is not numbered: the next one takes its place
        if (textOf({ lease, nonce, seq: seq + 1, ...frame }).length > MAX_LINE) continue;
        busy = true;
        send(frame, 0);
      }
      if (drained && !busy && (lease === null || !queue.length)) drained();
    };

    const push = (frame) => {
      if (lease === null) return;
      if (queue.length >= QUEUE_LIMIT) {
        lost(null);
        return;
      }
      queue.push(frame);
      pump();
    };

    const onAccepted = (number, given, bits) => {
      lease = number;
      nonce = given;
      seq = 0;
      run = 0;
      attempt = 0;
      duplicate = null;
      silent = false;
      narrow(bits);
      if (may.lifecycle) {
        const start = first;
        first = null;
        const frame = start
          ? session(start.native, { event: "start", reason: start.reason })
          : session(nativeId(sessions), { event: "snapshot" });
        if (frame) push(frame);
      }
      if (may.signal && active) {
        run = 1;
        push({ run, state: "working" });
      }
      if (ping) clearInterval(ping);
      ping = setInterval(() => {
        if (lease !== null && !busy && !queue.length) push({});
      }, PING_EVERY_MS);
      ping.unref();
    };

    /* a binding that did not end in a lease: the start is never reported later */
    const unbound = (reply) => {
      binding = false;
      first = null;
      const refused = refusalOf(reply);
      if (refused !== "duplicate-observer") duplicate = null;
      else if (duplicate === null) duplicate = Date.now();
      // held by a second observer of this process, not by a lease this one let go of
      const second = duplicate !== null && Date.now() - duplicate > DUPLICATE_MS;
      if (FINAL_REFUSALS.includes(refused) || second) ended = true;
      if (!disposed && !ended) rebind();
    };

    const attest = (given, tries) => {
      exchange({
        nonce: given,
        socket: target.socket,
        server_pid: target.serverPid,
        pane: target.pane,
        observerVersion: OBSERVER_VERSION,
        piVersion,
        mode: "tui",
        capabilities: { signal: may.signal, lifecycle: may.lifecycle },
      }, (reply) => {
        const next = nonceOf(reply);
        // not believed and not blamed: the same claims once more, with the new nonce
        if (!disposed && reply && reply.again === true && next && Object.keys(reply).length === 4
          && tries < AGAIN_MS.length) {
          later(AGAIN_MS[tries], () => attest(next, tries + 1));
          return;
        }
        const bits = reply ? bitsOf(reply.capabilities) : null;
        if (disposed || !bits || !next || Object.keys(reply).length !== 5
          || !Number.isSafeInteger(reply.accepted) || reply.accepted < 1) {
          unbound(reply);
          return;
        }
        binding = false;
        onAccepted(reply.accepted, next, bits);
      });
    };

    const bind = () => {
      if (disposed || ended || binding || lease !== null || !target) return;
      // a runtime that lost both capabilities has nothing to bind for
      if (!may.signal && !may.lifecycle) {
        ended = true;
        return;
      }
      binding = true;
      exchange({}, (reply) => {
        const given = reply && Object.keys(reply).length === 3 ? nonceOf(reply) : null;
        if (!given || disposed) {
          unbound(reply);
          return;
        }
        attest(given, 0);
      });
    };

    /* an internal error ends this runtime's observer and never reaches Pi */
    const guarded = (work) => (event, ctx) => {
      if (disposed) return undefined;
      try {
        return work(event, ctx);
      } catch (_) {
        dispose();
        lost(null);
        return undefined;
      }
    };

    const onStart = guarded((event, ctx) => {
      if (target || ctx.mode !== "tui") return;
      if (typeof piVersion !== "string" || !/^[0-9A-Za-z.+-]{1,32}$/.test(piVersion)) return;
      target = route(process.env);
      if (!target) return;
      if (may.lifecycle) {
        sessions = ctx.sessionManager;
        const native = nativeId(sessions);
        const reason = closedWord(START_REASONS, event.reason);
        first = native && reason ? { native, reason } : null;
      }
      bind();
    });

    const onAgentStart = guarded(() => {
      if (!target || active || !may.signal) return;
      active = true;
      if (lease === null) return;
      run += 1;
      push({ run, state: "working" });
    });

    const onAgentSettled = guarded(() => {
      if (!target || !active || !may.signal) return;
      active = false;
      if (lease !== null && run > 0) push({ run, state: "turn-done" });
    });

    const onShutdown = guarded((event, ctx) => {
      // the last id, read before this runtime is disposed — and only by a
      // lease that may still observe sessions and has not said it cannot
      const native = may.lifecycle && target && !silent ? nativeId(ctx.sessionManager) : null;
      dispose();
      if (lease === null) {
        seal();
        return undefined;
      }
      const reason = closedWord(SHUTDOWN_REASONS, event.reason);
      // what was queued and not sent belongs to a runtime that is over
      queue = [];
      const last = reason ? session(native, { event: "shutdown", reason }) : null;
      if (last) queue.push(last);
      queue.push({ reason: reason || "fault" });
      return new Promise((resolve) => {
        let finished = false;
        const finish = () => {
          if (finished) return;
          finished = true;
          clearTimeout(limit);
          drained = null;
          seal();
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
