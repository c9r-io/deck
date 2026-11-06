// Isolated real-WKWebView carrier for terminal rendering cadence: the numbers
// behind "terminal scrolling keeps display cadence" (f5c3c1d, 2026-08-30,
// about 40 Hz → 60 Hz) and two more a user feels, measured through the app's
// own chain (this webview → Tauri IPC → tmux → the attach client → xterm).
// - perf-display: the page's own frame rate while idle (a = Hz × 10), the
//   ceiling every other rate is read against.
// - perf-wheel: a wheel event on every display frame for 3 s, the production
//   handler (layout.js) with its one request in flight. a = completed
//   `scroll_session` requests per second × 10, b = their p95 round trip in ms.
// - perf-wheel-paint: xterm paints per second during that gesture (a × 10)
//   and the longest gap between display frames in ms (b).
// - perf-flood: `seq 1 200000` (≈1.3 MB) in one pane. a = ms from the
//   command to its last line painted, b = the longest frame gap in ms.
// - perf-flood-fps: display frames per second during the flood (a × 10) and
//   xterm paints per second (b × 10).
// - perf-echo: 30 keystrokes through xterm's input path to `cat` with tty
//   echo, each timed to the xterm paint that showed it (onRender). a = median ms × 10,
//   b = p95 ms × 10.
// Every check is a metric: the numbers are the result. Synthetic wheel
// events skip the native compositor, as the original measurement could not
// avoid either; the board poll stays on, as for a user. No terminal text is
// logged; the fixture is numbers printed by `seq`.
export async function runTerminalPerfSmoke() {
  const { ctx, inv } = await import('../js/state.js');
  const { panes, provider, render } = await import('../js/board.js');
  const { backToBoard, openSession } = await import('../js/layout.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 8000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) return false;
      await pause(20);
    }
    return true;
  };
  const metric = (name, a = 0, b = 0) =>
    inv('ui_event', { code: 'smoke-check', detail: name, a: Math.round(a), b: Math.round(b) });
  const quantile = (values, q) => {
    if (!values.length) return 0;
    const sorted = [...values].sort((x, y) => x - y);
    return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  };
  /* display frames between start() and stop(): count and longest gap */
  const frames = () => {
    let last = 0, count = 0, longest = 0, running = true;
    const tick = now => {
      if (!running) return;
      if (last) longest = Math.max(longest, now - last);
      last = now; count++;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    return () => { running = false; return { count, longest }; };
  };

  const systemFetch = window.fetch;
  const scrolls = [];
  let failed = false, stage = 0, card = null;
  try {
    stage = 1;
    window.fetch = async function observed(url, init) {
      const match = /^(?:ipc:\/\/localhost|https?:\/\/ipc\.localhost)\/([^?#]+)/.exec(String(url?.url || url));
      if (!match || decodeURIComponent(match[1]) !== 'scroll_session') return systemFetch.call(window, url, init);
      const started = performance.now();
      const reply = await systemFetch.call(window, url, init);
      scrolls.push({ started, done: performance.now() });
      return reply;
    };
    await until(() => provider.projects().length > 0);
    await inv('smoke_native_input', { input: { kind: 'activate' } });
    await until(() => !document.hidden, 5000);
    const project = provider.projects()[0];
    card = (await provider.createStarted({ projectId: project.id, columnId: project.columns[0].id,
      title: 'terminal-perf', cmd: '', dir: '/tmp' })).card;
    render();
    await openSession(card.id);
    await pause(1100);
    const pane = () => panes.get(card.session);
    const raw = text => inv('pty_write', { name: card.session, gen: ctx.ptyGens.get(card.session), dataB64: strToB64(text) });
    let paints = 0, awaiting = null;
    /* a paint that shows what a keystroke waits for stamps its time */
    const painted = pane().term.onRender(() => {
      paints++;
      if (awaiting && awaiting.check()) { awaiting.at = performance.now(); awaiting = null; }
    });
    const lastLine = () => {
      const buffer = pane().term.buffer.active;
      for (let i = buffer.length - 1; i >= 0; i--) {
        const text = buffer.getLine(i)?.translateToString(true).trim();
        if (text) return text;
      }
      return '';
    };
    const shown = text => [...pane().body.querySelectorAll('.xterm-rows > div')]
      .some(row => row.textContent.replace(/ /g, ' ').trim() === text);
    await raw("PS1='$ '; PROMPT='$ '; RPROMPT=''\r"); await pause(300);
    await raw('seq 1 20000\r');
    await until(() => shown('20000'), 15000);
    await pause(500);

    stage = 2;
    let stop = frames();
    await pause(1000);
    const idle = stop();
    const displayHz = idle.count;
    await metric('perf-display', displayHz * 10, idle.longest);

    stage = 3;
    const screen = pane().body.querySelector('.xterm-screen');
    const rect = screen.getBoundingClientRect();
    const wheel = () => screen.dispatchEvent(new WheelEvent('wheel', { deltaY: -34, deltaMode: 0,
      bubbles: true, cancelable: true, clientX: rect.left + 40, clientY: rect.top + 40 }));
    scrolls.length = 0;
    paints = 0;
    stop = frames();
    const gestureStart = performance.now();
    await new Promise(done => {
      const step = now => {
        if (now - gestureStart >= 3000) { done(); return; }
        wheel();
        requestAnimationFrame(step);
      };
      requestAnimationFrame(step);
    });
    const gesture = stop();
    const seconds = (performance.now() - gestureStart) / 1000;
    const finished = scrolls.filter(s => s.done - gestureStart <= seconds * 1000);
    await metric('perf-wheel', finished.length / seconds * 10, quantile(finished.map(s => s.done - s.started), 0.95));
    await metric('perf-wheel-paint', paints / seconds * 10, gesture.longest);
    await pause(400);
    await inv('scroll_bottom', { name: card.session });
    await pause(600);

    stage = 4;
    await raw('clear\r'); await pause(400);
    paints = 0;
    stop = frames();
    const floodStart = performance.now();
    await raw('seq 1 200000\r');
    const floodDone = { check: () => lastLine() === '$' && shown('200000'), at: 0 };
    awaiting = floodDone;
    const flooded = await until(() => floodDone.at > 0, 60000);
    awaiting = null;
    const floodMs = (floodDone.at || performance.now()) - floodStart;
    const flood = stop();
    await metric('perf-flood', flooded ? floodMs : -1, flood.longest);
    await metric('perf-flood-fps', flood.count / (floodMs / 1000) * 10, paints / (floodMs / 1000) * 10);

    stage = 5;
    await raw('clear; cat\r'); await pause(600);
    const samples = [];
    for (let i = 0; i < 30; i++) {
      const wait = { check: () => lastLine().length === i + 1, at: 0 };
      awaiting = wait;
      const sent = performance.now();
      pane().term.input(String.fromCharCode(97 + (i % 26)));
      if (await until(() => wait.at > 0, 2000)) samples.push(wait.at - sent);
      awaiting = null;
      await pause(60);
    }
    await metric('perf-echo', quantile(samples, 0.5) * 10, samples.length === 30 ? quantile(samples, 0.95) * 10 : -1);
    await raw('\x03'); await pause(200);
    painted.dispose();
  } catch (error) {
    failed = true;
    await inv('ui_event', { code: 'smoke-check', detail: 'perf-exception', a: -(stage || 1), b: 0 });
  } finally {
    window.fetch = systemFetch;
    try { backToBoard(); if (card) await provider.close(card.id); } catch (_) { failed = true; }
  }
  await inv('ui_event', { code: 'smoke-check', detail: 'done', a: failed ? -1 : 1, b: 0 });
}
