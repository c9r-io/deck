//! Account quota for an orchestrator (`deck_quota_snapshot`, approved
//! 2026-11-28): the usage windows of every agent account Deck observed in
//! this run, whether or not a card runs under it now. The contract is in
//! `mod.rs` ("Account quota"); the observations are `quota.rs`'s.
//!
//! Needs the client's own choice `allow_quota` and nothing else; without
//! it the tool is `PERMISSION_DENIED`, whatever else the client holds
//! (`deck_worker_list` `quota` keeps its card-linked projection under the
//! worker choice). Each account's `cards` lists only cards the caller sees
//! already: with the worker choice, its own worker cards and its calling
//! card (`workers::quota_cards`, `origin::sees`); without it, none. Read
//! only and presentation only: it starts, scans beyond `quota.rs`'s bounds,
//! notifies and decides nothing.

use super::*;

/// Whether the ledger holds the quota choice (`STATE_VERSION_QUOTA`).
pub(super) fn uses_quota(doc: &DiskDoc) -> bool {
    doc.config.clients.iter().any(|c| c.allow_quota)
}

pub(super) fn quota_snapshot(
    runtime: &Runtime,
    client_id: &str,
    arguments: Value,
) -> Result<Value, Value> {
    parse::<Empty>(arguments)?;
    let orchestrator = super::origin::orchestrator();
    let cards = runtime
        .read(|doc| {
            permitted(doc, client_id, Choice::Quota).map(|client| {
                client
                    .allow_workers
                    .then(|| super::workers::quota_cards(doc, client_id, orchestrator.as_deref()))
            })
        })
        .map_err(map_error)?
        .map_err(map_error)?;
    Ok(crate::quota::mcp_snapshot(cards.as_ref()))
}
