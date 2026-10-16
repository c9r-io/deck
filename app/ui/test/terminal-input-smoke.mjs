// Isolated real-WKWebView carrier for terminal input ordering and target
// identity (terminal-input.js, layout.js wireTerminalInput / ensureAttached,
// pty.rs pty_write). Every check runs through the app's own chain: this
// webview → Tauri IPC → pty_write → the attach client's pty → an isolated
// tmux session whose pane runs `cat` with echo off. `cat` executes nothing;
// a line it prints back is a line the pane received, in the order received.
// Input is fixed test text handed to xterm's public `input()` (the onData
// boundary) or, for the composition checks, synthetic composition events on
// xterm's textarea. Nothing here is a system input method. The page's content
// policy does not let it read its own vendor file, so the embedded xterm is
// identified by behaviour (`ti-composition-boundary` fails on the unpatched
// build), not by digest; the digest is pinned by static.test.mjs.
// Two things are scripted, in this page only: when the page RECEIVES the
// reply to `scroll_bottom`, `attach_session` or `save_dropped_file` (the request has already been
// sent and handled; the fetch reply is held back), and a read of each
// `pty_write` request's `gen` argument and whether its bytes contain the
// test-only stale file marker as it leaves the page; no bytes are logged.
export async function runTerminalInputSmoke() {
  const { ctx, inv } = await import('../js/state.js');
  const { panes, provider, render, stopPolling } = await import('../js/board.js');
  const { addSplit, backToBoard, focusPane, openSession, clearFocusedTerminal } = await import('../js/layout.js');
  const { acceptSuggestion, acceptGhost } = await import('../js/terminal.js');
  const { strToB64 } = await import('../js/terminal-bytes.js');
  const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
  const until = async (check, budget = 8000) => {
    const deadline = Date.now() + budget;
    while (!(await check())) {
      if (Date.now() >= deadline) return false;
      await pause(50);
    }
    return true;
  };
  let failed = false, stage = 0;
  const report = (name, ok, a = 1, b = 0) => {
    if (!ok) failed = true;
    return inv('ui_event', { code: 'smoke-check', detail: name, a: ok ? a : -a, b });
  };
  const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

  const systemFetch = window.fetch;
  const holds = {}, writes = [], oldFileWrites = [];
  let own = null, fileSaves = 0;
  const hold = command => { holds[command] = deferred(); };
  const release = command => { const held = holds[command]; delete holds[command]; if (held) held.resolve(); };
  try {
    stage = 1;
    window.fetch = async function observed(url, init) {
      const match = /^(?:ipc:\/\/localhost|https?:\/\/ipc\.localhost)\/([^?#]+)/.exec(String(url?.url || url));
      const command = match ? decodeURIComponent(match[1]) : null;
      if (command === 'save_dropped_file') fileSaves++;
      if (command === 'pty_write') {
        /* only the generation of this carrier's own pane A is kept */
        let gen = 'unread', name = null, oldFile = null;
        try { const args = JSON.parse(typeof init?.body === 'string' ? init.body : new TextDecoder().decode(init?.body)); gen = args.gen ?? null; name = args.name; oldFile = atob(args.dataB64).includes('ti-old-file.png'); } catch (_) { /* stays unread */ }
        if (name === null || name === own) { writes.push(gen); oldFileWrites.push(oldFile); }
      }
      const reply = await systemFetch.call(window, url, init);
      if (command && holds[command]) await holds[command].promise;
      return reply;
    };

    await until(() => provider.projects().length > 0);
    await inv('smoke_native_input', { input: { kind: 'activate' } });
    await until(() => !document.hidden, 5000);
    stopPolling();
    const project = provider.projects()[0];
    const create = async title => (await provider.createStarted({ projectId: project.id,
      columnId: project.columns[0].id, title, cmd: '', dir: '/tmp' })).card;
    const a = await create('terminal-input-a');
    const b = await create('terminal-input-b');
    own = a.session;
    render();
    await openSession(a.id); stopPolling();
    await pause(1100);
    const pane = () => panes.get(a.session);
    const lines = () => {
      const buffer = pane().term.buffer.active, out = [];
      for (let i = 0; i < buffer.length; i++) out.push(buffer.getLine(i)?.translateToString(true).trim() || '');
      return out;
    };
    const received = text => until(() => lines().includes(text));
    const absent = text => !lines().includes(text);
    const raw = text => inv('pty_write', { name: a.session, gen: ctx.ptyGens.get(a.session), dataB64: strToB64(text) });
    /* history to scroll into, then the receiver */
    await raw("PS1='$ '; PROMPT='$ '; RPROMPT=''\r"); await pause(300);
    await raw('seq 1 120\r'); await pause(400);
    await raw('stty -echo; cat\r'); await pause(500);
    pane().term.input('ti-ready\r');
    const ready = await received('ti-ready');
    await report('ti-ready', ready && typeof pane().inputGen === 'number');
    if (!ready) throw new Error('receiver not ready');

    stage = 2;
    const probeGen = pane().inputGen;
    await report('ti-ipc-spy', writes.length > 0 && writes[writes.length - 1] === probeGen, 1, writes.includes('unread') ? 0 : 1);

    /* the production wheel path puts the pane into copy-mode and marks the card */
    const scrollUp = async () => {
      const screen = pane().body.querySelector('.xterm-screen').getBoundingClientRect();
      pane().body.dispatchEvent(new WheelEvent('wheel', { deltaY: -600, deltaMode: 0, bubbles: true, cancelable: true,
        clientX: screen.left + 40, clientY: screen.top + 40 }));
      return until(() => !!provider.get(a.id)?.scrolled, 4000);
    };
    const composition = (type, data = '') => pane().term.textarea.dispatchEvent(new CompositionEvent(type, { bubbles: true, data }));

    stage = 3;
    let scrolled = await scrollUp();
    hold('scroll_bottom');
    let mark = writes.length;
    pane().term.input('ti-order-');
    composition('compositionstart'); composition('compositionend');
    pane().term.input('中文\r');
    await pause(400);
    const heldBack = writes.length === mark && absent('ti-order-中文');
    release('scroll_bottom');
    await report('ti-order-scroll', scrolled && heldBack && await received('ti-order-中文'), 1, writes.length - mark);

    stage = 4;
    const firstGen = pane().inputGen;
    scrolled = await scrollUp();
    hold('scroll_bottom');
    mark = writes.length;
    pane().term.input('ti-old\r');
    await pause(100);
    await openSession(b.id); stopPolling();
    release('scroll_bottom');
    await pause(400);
    const oldNeverSent = scrolled && writes.length === mark;

    stage = 5;
    hold('attach_session');
    const opening = openSession(a.id);
    const framed = await until(() => !!pane() && pane().renderedGen != null, 6000);
    const beforeReply = framed && pane().inputGen == null;
    await report('ti-early-frame', beforeReply, 1, framed ? 1 : 0);
    mark = writes.length;
    pane().term.input('ti-early\r');
    await pause(400);
    const waited = absent('ti-early') && writes.slice(mark).every(gen => typeof gen === 'number');
    release('attach_session');
    await opening; stopPolling();
    const early = await received('ti-early');
    const secondGen = pane().inputGen;
    await report('ti-early-input', waited && early && writes[writes.length - 1] === secondGen
      && writes.slice(mark).every(gen => typeof gen === 'number'), 1, writes.length - mark);
    await report('ti-switch-cancels-old', oldNeverSent && secondGen > firstGen && absent('ti-old'));

    stage = 6;
    let refusal = '';
    try { await inv('pty_write', { name: a.session, gen: firstGen, dataB64: strToB64('ti-stale\r') }); } catch (error) { refusal = String(error); }
    pane().term.input('ti-new\r');
    await report('ti-backend-refuses-stale', refusal === 'stale-attachment' && await received('ti-new') && absent('ti-stale'));

    stage = 7;
    await addSplit(a.id, 'row', false, b.id); stopPolling();
    await pause(600);
    focusPane(a.session);
    scrolled = await scrollUp();
    hold('scroll_bottom');
    pane().term.input('ti-split\r');
    await pause(100);
    focusPane(b.session);
    await pause(300);
    const stillWaiting = absent('ti-split') && ctx.attachedName === b.session;
    release('scroll_bottom');
    await report('ti-split-focus', scrolled && stillWaiting && await received('ti-split'));

    stage = 8;
    focusPane(a.session);
    const textarea = pane().term.textarea;
    textarea.value = '';
    pane().term.input('ti-ime-');
    composition('compositionstart');
    textarea.value = 'zhongwen'; composition('compositionupdate', 'zhongwen');
    await pause(40);
    /* the commit and the start of the next composition share one task */
    textarea.value = '中文'; composition('compositionend', '中文');
    composition('compositionstart');
    textarea.value = '中文c'; composition('compositionupdate', 'c');
    await pause(40);
    textarea.value = '中文ceshi'; composition('compositionupdate', 'ceshi');
    await pause(40);
    textarea.value = '中文测试'; composition('compositionend', '测试');
    await pause(80);
    pane().term.input('\r');
    await report('ti-composition-boundary', await received('ti-ime-中文测试') && absent('ti-ime-中文c测试'));

    stage = 9;
    const unboundRefusals = [];
    for (const binding of [{}, { gen: null }]) {
      try { await inv('pty_write', { name: a.session, ...binding, dataB64: strToB64('ti-unbound\r') }); }
      catch (error) { unboundRefusals.push(String(error)); }
    }
    pane().term.input('ti-bound\r');
    await report('ti-backend-refuses-unbound', unboundRefusals.length === 2
      && unboundRefusals.every(error => error === 'stale-attachment') && await received('ti-bound') && absent('ti-unbound'));

    stage = 10;
    acceptSuggestion('ti-suggestion-');
    ctx.ghostRemainder = 'ghost'; acceptGhost();
    pane().term.input('\r');
    await report('ti-completion-input', await received('ti-suggestion-ghost'));
    mark = writes.length;
    clearFocusedTerminal(); await pause(200);
    const clearWritten = writes.length === mark + 1 && writes[mark] === pane().inputGen;
    // Clear the receiver's canonical input after Ctrl+L, without leaving cat.
    pane().term.input('\x15ti-clear\r');
    await report('ti-clear-input', clearWritten && await received('ti-clear'));

    stage = 11;
    const dropFile = (name, paste = false) => {
      const transfer = new DataTransfer();
      transfer.items.add(new File(['fixture'], name, { type: 'image/png' }));
      pane().body.dispatchEvent(paste
        ? new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true })
        : new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
    };
    hold('save_dropped_file'); mark = writes.length;
    let saves = fileSaves;
    dropFile('ti-file.png');
    pane().term.input('ti-file-end\r');
    const saveStarted = await until(() => fileSaves > saves);
    const fileWaited = writes.length === mark;
    focusPane(b.session);
    release('save_dropped_file');
    const fileReceived = await until(() => lines().join('').replace(/\s/g, '').includes('ti-file.pngti-file-end'));
    await report('ti-file-order-focus', saveStarted && fileWaited && fileReceived
      && ctx.attachedName === b.session && writes.slice(mark).every(gen => gen === pane().inputGen));

    stage = 12;
    focusPane(a.session);
    const oldFileGen = pane().inputGen;
    hold('save_dropped_file'); mark = writes.length; saves = fileSaves;
    dropFile('ti-old-file.png', true);
    const pasteStarted = await until(() => fileSaves > saves);
    backToBoard(); stopPolling();
    await openSession(a.id); stopPolling();
    const replacedGen = pane().inputGen;
    release('save_dropped_file'); await pause(300);
    // Reattaching xterm can itself answer terminal queries. Those replies
    // belong to the replacement attachment; they are not the held file.
    // Refuse every old-generation write AND every write of the stale path,
    // including an accidental rebind of that path to the new generation.
    const noOldWrite = writes.slice(mark).every((gen, index) => gen === replacedGen
      && oldFileWrites[mark + index] === false);
    pane().term.input('ti-file-fresh\r');
    const fileReplacementChecks = [pasteStarted, replacedGen > oldFileGen, noOldWrite,
      await received('ti-file-fresh'), !lines().join('').includes('ti-old-file.png'),
      writes[writes.length - 1] === replacedGen];
    // Closed, content-free diagnostics identify a failed invariant without
    // weakening any check or logging terminal text or attachment identities.
    const fileReplacementMask = fileReplacementChecks.reduce((mask, ok, index) => mask | (ok ? 1 << index : 0), 0);
    await report('ti-file-replaced', fileReplacementChecks.every(Boolean), 1, fileReplacementMask);

  } catch (error) {
    failed = true;
    await report('ti-exception', false, stage || 1);
  } finally {
    for (const command of Object.keys(holds)) release(command);
    window.fetch = systemFetch;
  }
  await inv('ui_event', { code: 'smoke-check', detail: 'done', a: failed ? -1 : 1, b: 0 });
}
