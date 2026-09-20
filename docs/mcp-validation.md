# MCP validation record

Date: 2026-09-20. Status words below are evidence labels, not forecasts.

## Automated evidence

- **PASS** — official-SDK STDIO adapter initializes, lists all 11 tools,
  advertises strict schemas and annotations, rejects unknown arguments, calls
  a separate mock Deck service, returns structured content, and emits only MCP
  JSON on stdout: `cargo test -p deck-mcp --test stdio`.
- **PASS** — production runner executes a real zsh child, ignores a forged
  completion-looking output line, reports exit 17, routes interactive input to
  the exact job, refuses late input, sends SIGINT to the owned process group,
  and confirms signal 2: `cargo test -p deck-mcp-runner --test runner`.
- **PASS** — direct isolated runner exercise covered Unicode/multiline output,
  exit 0, exit 23, cursor read, interactive input, late-input rejection,
  interrupt, human takeover, and return.
- **PASS** — a fresh `deck-smoke-mcp-e2e-20260920` tmux server, private smoke
  data directory, disposable Git repository, production Deck WebView, bundled
  Adapter, and bundled runner completed the real protocol loop. The client
  discovered 11 tools, created a visible card despite the project's `codex`
  default, observed failing exit 1, fixed the file and observed exit 0, read
  the real diff, continued a running job by cursor, supplied bound Unicode
  stdin, refused late stdin, confirmed an interrupt-requested exit 130, and
  closed through the Board transaction. The final persisted Board had zero
  cards and the smoke socket had zero sessions; final cleanup then stopped the
  deliberately persistent empty smoke server.
- **PASS** — local UI takeover fenced epoch 1; a real Adapter call received
  `CONTROL_REVOKED`. After the human shell exited, **Return to MCP** issued
  epoch 3, a new MCP job exited 0, and the MCP close operation committed.
- **PASS** — the isolated `app.log` and `mcp.json` contained none of the test
  script, Unicode stdin, test output, or the deliberately forbidden stale
  command. Data directory/file/socket modes were 0700/0600/0600.
- **PASS** — `cargo test --workspace`, `cargo clippy --workspace --all-targets
  -- -D warnings`, `scripts/ui-tests`, and `node app/ui/js/check.mjs` passed.
- **PASS** — the checked-in independent client `scripts/mcp-e2e.mjs` repeated
  the production loop and additionally proved same-id replay returned the same
  job while changed arguments returned `REQUEST_ID_CONFLICT`.

The repository-wide gate results and any environmental blockers are recorded
in the implementation handoff for the change that introduced this document.

## Ordinary ChatGPT manual acceptance

Status: **MANUAL_PENDING**. Protocol and local execution can be automated;
account login, workspace policy, Tunnel association, model tool availability,
and ChatGPT confirmations require the user.

1. Start the installed Deck app; enable MCP and authorize only a disposable
   test project.
2. Install/start the official Secure MCP Tunnel client according to its current
   documentation. Configure it to launch Deck's copied STDIO command. Do not
   expose a public endpoint.
3. In ChatGPT, enable Developer mode under **Settings → Security and login** if
   available. Record account/workspace policy restrictions.
4. Open ChatGPT Plugins, add a Tunnel connection, select the tunnel id, review
   the discovered tools, then start a new ordinary Chat conversation with that
   connection enabled.
5. Record client/version, mode (`Chat`, not Work/Codex), model, transport,
   authorization method, write-confirmation behavior, and selected tools.
6. Ask for capabilities, then ask ChatGPT to create a disposable session,
   create a small failing program/test, obtain a real nonzero exit, inspect and
   fix it, rerun to exit zero, and show `git diff`.
7. Run a long task and continue with `deck_job_read`; test stdin and interrupt.
8. In Deck choose **Take control**. Confirm the running process remains visible
   and subsequent old-epoch MCP writes fail. Exit the human shell, choose
   **Return to MCP**, and continue with the new epoch.
9. Close the test card, revoke the client, and remove only the disposable test
   project. Confirm no Codex CLI, Claude Code, LLM API, or agent process ran in
   the managed session.

Expected report: each step is `PASS`, `FAIL`, or `MANUAL_PENDING`; do not treat
a Codex smoke, API Playground run, or unchanged usage display as proof of a
successful ordinary ChatGPT Chat or of any quota behavior.
