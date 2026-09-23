//! The argv of the two long-lived tmux clients deck attaches: the Board's
//! persistent query client (`tmux.rs`) and a visible pane's PTY attach
//! (`pty.rs`). Pure and dependency-free so `tests/tmux_contract.rs` includes
//! this file (`#[path]`) and attaches exactly these clients.
//!
//! # Contract
//! tmux resolves an ambient target client for every one-shot command sent
//! without `-c` (the most recently active attached client), and commands and
//! format expansion evaluate in that client's context. With no pane open the
//! query client is the ONLY client, so its flags decide whether a card's
//! launch `send-keys`, a delivery Enter, or an ambient `#{pane_tty}` works:
//! a read-only (`-r`) query client refused every launch command and delivery
//! Enter while the Board showed no terminal (0.7.1–0.7.6), and an ambient
//! `#{pane_tty}` wrote restored history into another card's pane. The query
//! client is therefore NOT read-only; its stdin carries only the compiled
//! `list-panes` query, so `-r` guarded nothing a user value can reach.
//!
//! `QUERY_CLIENT_FLAGS` is also the identity `tmux_lifecycle.rs` verifies in
//! `list-clients` before subtracting Deck's own client from restart impact.
//! The tmux contract suite runs its client-sensitive behaviour contracts
//! with no client, with this query client, and with this query client plus a
//! PTY pane client, so any change here is exercised under all three.

/// `-f` flags of the query client, exactly as tmux reports them back in
/// `#{client_flags}` (comma-separated, order-free).
pub(crate) const QUERY_CLIENT_FLAGS: &str = "ignore-size,no-output";

/// Control-mode attach of the Board's query client to `target` (an exact
/// `=session` target). Callers prepend `-f <conf> -L <socket>`.
pub(crate) fn query_client_args(target: &str) -> [&str; 6] {
    [
        "-C",
        "attach-session",
        "-f",
        QUERY_CLIENT_FLAGS,
        "-t",
        target,
    ]
}

/// Ordinary tty attach of a visible pane to `target` (an exact `=session`
/// target), run inside a PTY. Callers prepend `-f <conf> -L <socket>`.
pub(crate) fn pane_client_args(target: &str) -> [&str; 3] {
    ["attach-session", "-t", target]
}
