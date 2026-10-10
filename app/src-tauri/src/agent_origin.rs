//! The kernel-side origin of a hook report, shared by the two consumers of
//! the status socket: Signal admission (`agent_status::ingest`) and
//! lifecycle receipts (`agent_reentry::lifecycle::receive`).
//!
//! # Contract
//! Pure functions over ONE process-table snapshot taken while the reporting
//! helper is still connected: how far its ancestry is walked, what was read,
//! and whether the chain from the helper to a foreground leader stays on the
//! pane's terminal. No store, no vocabulary, no side effect lives here; each
//! consumer keeps its own admission rules and neither reads the other's
//! state. A rule relaxed here is relaxed for both: change it only with both
//! consumers' tests.

use crate::agent_status::ProcessTable;

/// How far up from the reporting process the pane's process may be. A hook
/// is the agent's child (or a `sh -c` wrapper's grandchild) under the
/// pane's shell; 32 hops is far beyond any real layering.
pub(crate) const ORIGIN_HOPS: usize = 32;

/// What the listener read while the helper was still connected: the
/// kernel's peer pid and one process-table snapshot. The peer's ancestry,
/// the pane's foreground leader and its birth instant all come from this one
/// snapshot, so the compared facts describe the same instant.
pub(crate) struct Origin {
    pub(crate) peer: Option<u32>,
    pub(crate) table: ProcessTable,
}

/// Terminal continuity (the Codex shared-daemon FR): from the reporting
/// helper up to the pane's foreground leader, every process is on the
/// pane's controlling terminal — except the ONE process group the agent
/// detached the hook into. Runtime proof (Claude Code 2.1.283, Codex
/// 0.157.0 embedded): both spawn each hook in a fresh session, so the hook
/// (its `sh` and the helper) has no terminal and leads its own group, and
/// that group leader's parent is the agent on the pane's tty. That leading
/// run is allowed only when it is terminal-less, is all the helper's group,
/// and ends at the group's leader; everything above it must hold the pane's
/// non-zero tty. In the blocked 0.157.1 and 0.160.0 certifications, the shared
/// app-server daemon is a second terminal-less group between the hook and
/// the TUI that started it, so its
/// hooks fail here even though the starter pane passes `foreign-pane` and
/// the generation check. Deck does not claim the discontinuity IS a daemon;
/// any topology it cannot prove this way fails closed. One snapshot, no
/// extra reads. Accepted residual: an agent host that ran hooks inside its
/// OWN terminal-less group (no new group per hook) would look like a hook
/// wrapper; neither certified shared-daemon topology does.
pub(crate) fn terminal_continuous(
    table: &ProcessTable,
    chain: &[u32],
    leader: u32,
    pane_pid: u32,
) -> bool {
    let Some(tty) = table.get(&pane_pid).map(|p| p.tty).filter(|tty| *tty != 0) else {
        return false;
    };
    let Some(end) = chain.iter().position(|pid| *pid == leader) else {
        return false;
    };
    let Some(segment) = chain.get(..=end).and_then(|pids| {
        pids.iter()
            .map(|pid| table.get(pid))
            .collect::<Option<Vec<_>>>()
    }) else {
        return false;
    };
    let hook_group = segment[0].pgid;
    let detached = segment.iter().take_while(|p| p.tty != tty).count();
    let (hook, above) = segment.split_at(detached);
    hook.iter().all(|p| p.tty == 0 && p.pgid == hook_group)
        && hook.last().is_none_or(|top| top.pid == hook_group)
        && !above.is_empty()
        && above.iter().all(|p| p.tty == tty)
}
