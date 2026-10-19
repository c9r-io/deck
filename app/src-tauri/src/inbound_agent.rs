//! The Agent trigger of automations ("自动化"): a delegated MCP client names
//! one of the user's Agent rules and hands over one classified item
//! (`mcp/automation.rs`). This module is the durable staging between the
//! control socket and the dispatcher, and the dispatcher's source for those
//! items; it never decides what an item means.
//!
//! # Contract
//! - ONE file, `~/.deck/agent-inbox.json` (0600, atomic writes, bounded,
//!   `version` 1). An entry is one accepted invocation. Its identity is the
//!   MCP operation id minted for it, which is also the dispatcher's
//!   `Event.key`; its request identity is `(client, request_id)`, unique per
//!   file, so two clients' equal request ids never merge and a retry of the
//!   same request (same arguments) finds its own entry.
//! - Three states, forward only: `staged` (written BEFORE the MCP operation
//!   and the client's trigger sequence are persisted; never offered to the
//!   dispatcher, because its operation may not exist) → `committed` (the
//!   operation is on disk; `commit` is idempotent and is also run for every
//!   accepted trigger operation when MCP loads its ledger) → `handled` (the
//!   webview acknowledged the item: `done` with the card it made, or
//!   `skipped` with a closed reason). `handled`, like a rejection, is a
//!   consumption fact independent of any card: an entry is never deleted on
//!   acknowledgment and never returns to pending, so a lost or closed card
//!   cannot revive it. Entries leave only by the horizon.
//! - What is frozen at acceptance is what the client was promised: the rule
//!   id (`rule`, the automation id the client named), the rule's `revision`
//!   (`current_revision`, the same manifest digest the webview's
//!   `grantDigest` computes, over the rule and its template's steps as they
//!   are NOW, so any edit to what is sent or where changes it), the text and
//!   the three bounded context fields, canonical at birth (`admission::
//!   strip_invisible`, control characters removed, byte bounds). The
//!   dispatcher offers an entry's event with that rule; `inbound_pending`
//!   compares the frozen revision with the current one and marks the item
//!   `blocked: rule-changed` so the webview skips it instead of creating a
//!   card under a rule the client did not see. A rule deleted after
//!   acceptance is `skipped: no-rule` by the poll itself.
//! - `handled` writes the entry first and then tells MCP the operation's
//!   outcome (`mcp::automation_outcome`): `done` → `committed` with the card
//!   id, `skipped` → `rejected` with the reason's closed code. A client whose
//!   delegation is withdrawn (`reject_client`), or the feature being
//!   disabled (`reject_all`), retires its unhandled entries as `skipped:
//!   client-revoked`; MCP has rejected their operations in the same
//!   transaction. Both happen before any card exists.
//! - Capacity: at most `MAX_ENTRIES` unhandled entries and `MAX_LEDGER` in
//!   all; a full inbox refuses the stage (`CAPACITY_EXCEEDED` to the client)
//!   and nothing is written. The horizon (`HORIZON_SECS`) prunes handled
//!   entries and orphaned `staged` ones.
//! - Privacy: the file holds the item text the client submitted (the user's
//!   own delegated content, like a staged channel message), the client's
//!   public id and the operation id; never a credential. Log lines carry
//!   counts and closed codes only.
//! - The source: `enabled` while an enabled Agent rule exists, `poll` offers
//!   every committed entry whose rule exists (live by nature: no baseline),
//!   `set_live` is a no-op, and the dispatcher's seen ledger plus its
//!   pending list keep one entry from becoming two items.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::inbound::{Config, Event, Rule, Source, SourceStatus};
use crate::sync::LockRecover;

pub(crate) const SOURCE: &str = "agent";
const FILE_VERSION: u32 = 1;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
/// Unhandled (staged or committed) entries the inbox holds at most.
pub(crate) const MAX_ENTRIES: usize = 1000;
/// Entries in all, handled ones included, before the horizon prunes.
pub(crate) const MAX_LEDGER: usize = 5000;
/// The item text, after canonicalization.
pub(crate) const MAX_TEXT_BYTES: usize = 16 * 1024;
/// Each of `from`, `where` and `link`, after canonicalization.
pub(crate) const MAX_FIELD_BYTES: usize = 1024;
const HORIZON_SECS: u64 = 45 * 24 * 3600;
/// Closed reasons a handled entry may carry (the dispatcher's skip reasons
/// plus the two this module writes itself).
pub(crate) const HANDLED_REASONS: &[&str] = &[
    "busy",
    "no-rule-target",
    "no-template",
    "missed",
    "blocked",
    "rule-changed",
    "no-rule",
    "client-revoked",
];

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EntryState {
    Staged,
    Committed,
    Handled,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Handled {
    pub(crate) at: u64,
    /// `done` | `skipped`
    pub(crate) outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) card: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    /// The MCP operation id; the dispatcher's event key.
    pub(crate) id: String,
    pub(crate) client: String,
    pub(crate) request_id: String,
    pub(crate) request_hash: String,
    /// The automation id the client named (the rule's badge).
    pub(crate) rule: String,
    pub(crate) revision: String,
    pub(crate) text: String,
    pub(crate) from: String,
    #[serde(rename = "where")]
    pub(crate) where_: String,
    pub(crate) link: String,
    pub(crate) staged_at: u64,
    pub(crate) state: EntryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) handled: Option<Handled>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Doc {
    version: u32,
    #[serde(default)]
    entries: Vec<Entry>,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            version: FILE_VERSION,
            entries: Vec::new(),
        }
    }
}

/// What the control service hands over for one accepted invocation.
pub(crate) struct Submission {
    pub(crate) client: String,
    pub(crate) request_id: String,
    pub(crate) request_hash: String,
    pub(crate) operation_id: String,
    pub(crate) rule: String,
    pub(crate) revision: String,
    pub(crate) text: String,
    pub(crate) from: String,
    pub(crate) where_: String,
    pub(crate) link: String,
}

/// The outcome of `stage`: a fresh entry, or the entry an earlier attempt of
/// the same request left (its id is the operation id to use).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Staged {
    New,
    Existing,
}

fn sha_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn one_line(s: &str) -> bool {
    !s.chars().any(char::is_control)
}

fn valid_entry(e: &Entry) -> bool {
    use crate::inbound::{bounded_id, valid_badge};
    bounded_id(&e.id, 128)
        && bounded_id(&e.client, 128)
        && bounded_id(&e.request_id, 128)
        && sha_hex(&e.request_hash)
        && valid_badge(&e.rule)
        && sha_hex(&e.revision)
        && !e.text.is_empty()
        && e.text.len() <= MAX_TEXT_BYTES
        && e.text
            .chars()
            .all(|c| !c.is_control() || c == '\n' || c == '\t')
        && [&e.from, &e.where_, &e.link]
            .iter()
            .all(|f| f.len() <= MAX_FIELD_BYTES && one_line(f))
        && e.staged_at > 0
        && match (e.state, &e.handled) {
            (EntryState::Handled, Some(h)) => {
                h.at > 0
                    && matches!(h.outcome.as_str(), "done" | "skipped")
                    && h.card.as_ref().is_none_or(|c| bounded_id(c, 128))
                    && h.reason
                        .as_ref()
                        .is_none_or(|r| HANDLED_REASONS.contains(&r.as_str()))
            }
            (EntryState::Handled, None) => false,
            (_, None) => true,
            (_, Some(_)) => false,
        }
}

/// Text as the item was handed over, made safe to show and to paste:
/// invisible formatting and bidi controls removed, control characters other
/// than newline and tab dropped.
fn canonical_text(s: &str) -> String {
    crate::admission::strip_invisible(s)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .trim()
        .to_string()
}

/// A context field is one line.
fn canonical_field(s: &str) -> String {
    crate::admission::strip_invisible(s)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

struct Store {
    path: PathBuf,
    doc: Doc,
}

impl Store {
    fn load(path: PathBuf) -> Result<Self, DeckError> {
        let doc = crate::ledger::load_bounded::<Doc>(&path, MAX_FILE_BYTES, "agent inbox")?
            .unwrap_or_default();
        if doc.version != FILE_VERSION {
            return Err(DeckError::new(
                ErrorKind::NewerSchema,
                "agent inbox is from a newer deck",
            ));
        }
        let mut ids = std::collections::HashSet::new();
        let mut requests = std::collections::HashSet::new();
        let unique = doc.entries.iter().all(|e| {
            ids.insert(e.id.clone()) && requests.insert((e.client.clone(), e.request_id.clone()))
        });
        let unhandled = doc
            .entries
            .iter()
            .filter(|e| e.state != EntryState::Handled)
            .count();
        if !unique
            || unhandled > MAX_ENTRIES
            || doc.entries.len() > MAX_LEDGER
            || doc.entries.iter().any(|e| !valid_entry(e))
        {
            return Err(DeckError::new(
                ErrorKind::Recovery,
                "agent inbox exceeds its bounds",
            ));
        }
        Ok(Self { path, doc })
    }

    fn save_doc(&self, doc: &Doc) -> Result<(), DeckError> {
        let bytes = crate::ledger::encode(doc, "agent inbox")?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(DeckError::new(ErrorKind::DiskFull, "agent inbox is full"));
        }
        crate::datadir::atomic_write(&self.path, &bytes)
    }

    fn mutate<T>(
        &mut self,
        f: impl FnOnce(&mut Doc) -> Result<T, DeckError>,
    ) -> Result<T, DeckError> {
        let mut next = self.doc.clone();
        let out = f(&mut next)?;
        self.save_doc(&next)?;
        self.doc = next;
        Ok(out)
    }
}

fn prune(doc: &mut Doc, now: u64) {
    doc.entries.retain(|e| match e.state {
        EntryState::Handled => e
            .handled
            .as_ref()
            .is_some_and(|h| h.at.saturating_add(HORIZON_SECS) >= now),
        EntryState::Staged => e.staged_at.saturating_add(HORIZON_SECS) >= now,
        EntryState::Committed => true,
    });
}

fn inbox_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_PATH.lock_or_recover().clone() {
        return path;
    }
    crate::datadir::deck_dir().join("agent-inbox.json")
}

static STORE: OnceLock<Mutex<Result<Store, DeckError>>> = OnceLock::new();
#[cfg(test)]
static TEST_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Serializes the tests that redirect the store (here and in `mcp`).
#[cfg(test)]
pub(crate) static STORE_TESTS: Mutex<()> = Mutex::new(());
#[cfg(test)]
thread_local! {
    /// A test's answer in place of the committed Board's template
    /// (`Some(answer)`), so the revision check runs without a Board.
    pub(crate) static REVISION_FOR_TESTS: std::cell::RefCell<Option<Option<String>>> =
        const { std::cell::RefCell::new(None) };
}

fn with_store<T>(f: impl FnOnce(&mut Store) -> Result<T, DeckError>) -> Result<T, DeckError> {
    let state = STORE.get_or_init(|| Mutex::new(Store::load(inbox_path())));
    let mut guard = state.lock_or_recover();
    #[cfg(test)]
    {
        // tests redirect the path; reload when it moved
        let wanted = inbox_path();
        let stale = match &*guard {
            Ok(store) => store.path != wanted,
            Err(_) => true,
        };
        if stale {
            *guard = Store::load(wanted);
        }
    }
    match &mut *guard {
        Ok(store) => f(store),
        Err(e) => Err(e.clone()),
    }
}

/// Stage one accepted invocation before its operation is persisted. The
/// same `(client, request_id)` with the same request hash is the same
/// invocation and returns the entry an earlier attempt left; a different
/// hash is a conflict.
pub(crate) fn stage(s: Submission, now: u64) -> Result<(String, Staged), DeckError> {
    let entry = Entry {
        id: s.operation_id,
        client: s.client,
        request_id: s.request_id,
        request_hash: s.request_hash,
        rule: s.rule,
        revision: s.revision,
        text: canonical_text(&s.text),
        from: canonical_field(&s.from),
        where_: canonical_field(&s.where_),
        link: canonical_field(&s.link),
        staged_at: now,
        state: EntryState::Staged,
        handled: None,
    };
    if !valid_entry(&entry) {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "automation item does not fit the inbox shape",
        ));
    }
    with_store(|store| {
        if let Some(existing) = store
            .doc
            .entries
            .iter()
            .find(|e| e.client == entry.client && e.request_id == entry.request_id)
        {
            if existing.request_hash != entry.request_hash {
                return Err(DeckError::new(
                    ErrorKind::RequestConflict,
                    "request id was already used with different arguments",
                ));
            }
            return Ok((existing.id.clone(), Staged::Existing));
        }
        let id = entry.id.clone();
        store.mutate(|doc| {
            prune(doc, now);
            let unhandled = doc
                .entries
                .iter()
                .filter(|e| e.state != EntryState::Handled)
                .count();
            if unhandled >= MAX_ENTRIES || doc.entries.len() >= MAX_LEDGER {
                return Err(DeckError::new(ErrorKind::DiskFull, "agent inbox is full"));
            }
            if doc.entries.iter().any(|e| e.id == entry.id) {
                return Err(DeckError::new(
                    ErrorKind::RequestConflict,
                    "operation id is already staged",
                ));
            }
            doc.entries.push(entry);
            Ok(())
        })?;
        Ok((id, Staged::New))
    })
}

/// The operation for `(client, request_id)` is on disk under `operation_id`:
/// the entry may be offered. Idempotent; a handled entry stays handled.
pub(crate) fn commit(client: &str, request_id: &str, operation_id: &str) -> Result<(), DeckError> {
    with_store(|store| {
        let Some(index) = store
            .doc
            .entries
            .iter()
            .position(|e| e.client == client && e.request_id == request_id)
        else {
            return Err(DeckError::new(
                ErrorKind::Missing,
                "automation item is not staged",
            ));
        };
        let entry = &store.doc.entries[index];
        if entry.id != operation_id {
            return Err(DeckError::new(
                ErrorKind::RequestConflict,
                "automation item is staged under another operation",
            ));
        }
        if entry.state != EntryState::Staged {
            return Ok(());
        }
        store.mutate(|doc| {
            doc.entries[index].state = EntryState::Committed;
            Ok(())
        })
    })
}

/// The client that handed over the item `key` (the operation id), in any
/// state: proof material for the step authority a row admitted from it
/// carries (`scheduler/authority.rs`).
pub(crate) fn pending_client(key: &str) -> Option<String> {
    with_store(|store| {
        Ok(store
            .doc
            .entries
            .iter()
            .find(|e| e.id == key)
            .map(|e| e.client.clone()))
    })
    .ok()
    .flatten()
}

/// The rule revision frozen for `key` at acceptance.
pub(crate) fn revision_of(key: &str) -> Option<String> {
    with_store(|store| {
        Ok(store
            .doc
            .entries
            .iter()
            .find(|e| e.id == key)
            .map(|e| e.revision.clone()))
    })
    .ok()
    .flatten()
}

/// MCP's closed outcome code for a handled entry.
fn outcome_code(outcome: &str, reason: Option<&str>) -> Option<&'static str> {
    if outcome == "done" {
        return None;
    }
    Some(match reason.unwrap_or_default() {
        "no-rule-target" => "TARGET_MISSING",
        "no-template" => "TEMPLATE_MISSING",
        "blocked" => "RULE_BLOCKED",
        "rule-changed" => "RULE_CHANGED",
        "no-rule" => "RULE_MISSING",
        "client-revoked" => "CLIENT_REVOKED",
        _ => "SKIPPED",
    })
}

fn tell_mcp(id: &str, outcome: &str, card: Option<&str>, reason: Option<&str>) {
    let state = if outcome == "done" {
        "committed"
    } else {
        "rejected"
    };
    if let Err(e) = crate::mcp::automation_outcome(id, state, outcome_code(outcome, reason), card) {
        applog(&format!(
            "[inbound] agent outcome not recorded ({})",
            e.code()
        ));
    }
}

/// The webview acknowledged item `key`: record the consumption fact, then
/// tell MCP the operation's outcome. Returns whether the entry was newly
/// handled (a repeat is a no-op, an unknown key an error).
pub(crate) fn handled(
    key: &str,
    outcome: &str,
    card: Option<&str>,
    reason: Option<&str>,
    now: u64,
) -> Result<bool, DeckError> {
    if !matches!(outcome, "done" | "skipped")
        || reason.is_some_and(|r| !HANDLED_REASONS.contains(&r))
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "automation outcome is out of shape",
        ));
    }
    let newly = with_store(|store| {
        let Some(index) = store.doc.entries.iter().position(|e| e.id == key) else {
            return Err(DeckError::new(
                ErrorKind::Missing,
                "automation item is unknown",
            ));
        };
        if store.doc.entries[index].state == EntryState::Handled {
            return Ok(false);
        }
        store.mutate(|doc| {
            let entry = &mut doc.entries[index];
            entry.state = EntryState::Handled;
            entry.handled = Some(Handled {
                at: now,
                outcome: outcome.into(),
                card: card.map(str::to_string),
                reason: reason.map(str::to_string),
            });
            prune(doc, now);
            Ok(true)
        })
    })?;
    if newly {
        tell_mcp(key, outcome, card, reason);
    }
    Ok(newly)
}

fn retire(now: u64, select: impl Fn(&Entry) -> bool) -> Result<Vec<String>, DeckError> {
    with_store(|store| {
        let ids: Vec<String> = store
            .doc
            .entries
            .iter()
            .filter(|e| e.state != EntryState::Handled && select(e))
            .map(|e| e.id.clone())
            .collect();
        if ids.is_empty() {
            return Ok(ids);
        }
        store.mutate(|doc| {
            for entry in doc
                .entries
                .iter_mut()
                .filter(|e| e.state != EntryState::Handled && select(e))
            {
                entry.state = EntryState::Handled;
                entry.handled = Some(Handled {
                    at: now,
                    outcome: "skipped".into(),
                    card: None,
                    reason: Some("client-revoked".into()),
                });
            }
            Ok(())
        })?;
        Ok(ids)
    })
}

/// A client's delegation was withdrawn: its unhandled entries will not be
/// offered. MCP rejects their operations itself; returns the entry ids.
pub(crate) fn reject_client(client: &str, now: u64) -> Result<Vec<String>, DeckError> {
    retire(now, |e| e.client == client)
}

/// The feature was disabled: no unhandled entry will be offered.
pub(crate) fn reject_all(now: u64) -> Result<Vec<String>, DeckError> {
    retire(now, |_| true)
}

/// The rule's current revision: the digest the webview's `grantDigest`
/// computes for this rule and its template as the committed Board holds it
/// now (only the current Board answers, never a backup). `None` when the
/// Board or the template is unavailable.
pub(crate) fn current_revision(rule: &Rule) -> Option<String> {
    #[cfg(test)]
    if let Some(forced) = REVISION_FOR_TESTS.with(|cell| cell.borrow().clone()) {
        return forced;
    }
    let board = crate::documents::board_authority()?;
    revision_in(rule, &board)
}

/// The state an entry is in, with a handled entry's reason (tests only).
#[cfg(test)]
pub(crate) fn entry_state(key: &str) -> Option<(EntryState, Option<String>)> {
    with_store(|store| {
        Ok(store
            .doc
            .entries
            .iter()
            .find(|e| e.id == key)
            .map(|e| (e.state, e.handled.as_ref().and_then(|h| h.reason.clone()))))
    })
    .ok()
    .flatten()
}

pub(crate) fn revision_in(rule: &Rule, board: &Value) -> Option<String> {
    use crate::scheduler::authority::{grant_digest, has_placeholder, normalize_template_step};
    let project = board["projects"]
        .as_array()?
        .iter()
        .find(|p| p["id"].as_str() == Some(rule.project_id.as_str()))?;
    let template = project["templates"]
        .as_array()?
        .iter()
        .find(|t| t["name"].as_str() == Some(rule.template.as_str()))?;
    let normalized: Vec<String> = template["steps"]
        .as_array()?
        .iter()
        .map(|s| normalize_template_step(s.as_str().unwrap_or_default()))
        .collect();
    let grant = crate::inbound::AutoSend {
        digest: String::new(),
        steps: normalized
            .iter()
            .map(|s| crate::ledger::sha(s.as_bytes()))
            .collect(),
        classes: normalized
            .iter()
            .map(|s| {
                if has_placeholder(s) {
                    "bounded".into()
                } else {
                    "fixed".into()
                }
            })
            .collect(),
        external: rule.auto_send.as_ref().is_some_and(|g| g.external),
    };
    Some(grant_digest(rule, &grant))
}

/// The poll: every committed entry whose rule still exists, as the
/// dispatcher's event. An entry whose rule is gone is retired here.
fn committed_events(cfg: &Config, now: u64) -> Result<Vec<Event>, DeckError> {
    let (events, orphans) = with_store(|store| {
        let mut events = Vec::new();
        let mut orphans = Vec::new();
        for e in store
            .doc
            .entries
            .iter()
            .filter(|e| e.state == EntryState::Committed)
        {
            let rule = cfg
                .rules
                .iter()
                .find(|r| r.source == SOURCE && r.badge == e.rule);
            if rule.is_none() {
                orphans.push(e.id.clone());
                continue;
            }
            events.push(Event {
                source: SOURCE.into(),
                key: e.id.clone(),
                badge: e.rule.clone(),
                text: e.text.clone(),
                from: e.from.clone(),
                where_: e.where_.clone(),
                link: e.link.clone(),
            });
        }
        Ok((events, orphans))
    })?;
    for id in orphans {
        let _ = handled(&id, "skipped", None, Some("no-rule"), now);
    }
    Ok(events)
}

#[derive(Default)]
pub(crate) struct AgentSource {
    last_poll: Option<u64>,
    last_error: Option<&'static str>,
}

impl Source for AgentSource {
    fn id(&self) -> &'static str {
        SOURCE
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.rules.iter().any(|r| r.source == SOURCE && r.enabled)
    }

    fn poll(&mut self, cfg: &Config, _badges: &[String]) -> Result<Vec<Event>, &'static str> {
        let now = crate::datadir::now_epoch();
        self.last_poll = Some(now);
        match committed_events(cfg, now) {
            Ok(events) => {
                self.last_error = None;
                Ok(events)
            }
            Err(e) => {
                self.last_error = Some(e.code());
                Err(e.code())
            }
        }
    }

    fn set_live(&mut self, _app: &AppHandle, _wanted: bool, _badges: &[String]) {}

    fn status(&self) -> SourceStatus {
        SourceStatus {
            live: false,
            last_poll: self.last_poll,
            last_error: self.last_error,
        }
    }

    fn polled_events_are_live(&self) -> bool {
        true
    }
}

#[cfg(test)]
pub(crate) fn redirect_for_tests(path: Option<PathBuf>) {
    *TEST_PATH.lock_or_recover() = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_dir::TestDir;
    use serde_json::json;

    fn submission(client: &str, request: &str, op: &str, text: &str) -> Submission {
        Submission {
            client: client.into(),
            request_id: request.into(),
            request_hash: "a".repeat(64),
            operation_id: op.into(),
            rule: "triage".into(),
            revision: "b".repeat(64),
            text: text.into(),
            from: "alice".into(),
            where_: "#ops".into(),
            link: "https://example.test/x".into(),
        }
    }

    fn config_with_rule() -> Config {
        crate::inbound::config_from_value(Some(&json!({"rules": [{
            "id": "r1", "source": "agent", "badge": "triage", "projectId": "P1",
            "columnId": "C1", "cmd": "claude", "template": "mention"
        }]})))
    }

    fn redirect(dir: &TestDir) {
        redirect_for_tests(Some(dir.join("agent-inbox.json")));
    }

    #[test]
    fn staging_is_keyed_by_client_and_request_and_never_dispatches_unconfirmed() {
        let _serial = STORE_TESTS.lock_or_recover();
        let dir = TestDir::new("agent-inbox");
        redirect(&dir);
        let (id, staged) = stage(submission("c1", "req-1", "op_1", "hi"), 100).unwrap();
        assert_eq!((id.as_str(), staged), ("op_1", Staged::New));
        // a retry of the same request finds the same entry, another client's
        // equal request id is its own entry, a different hash is a conflict
        let (again, staged) = stage(submission("c1", "req-1", "op_2", "hi"), 101).unwrap();
        assert_eq!((again.as_str(), staged), ("op_1", Staged::Existing));
        let (other, _) = stage(submission("c2", "req-1", "op_3", "hi"), 102).unwrap();
        assert_eq!(other, "op_3");
        let mut different = submission("c1", "req-1", "op_4", "hi");
        different.request_hash = "c".repeat(64);
        assert_eq!(
            stage(different, 103).unwrap_err().kind(),
            ErrorKind::RequestConflict
        );
        // staged entries are not offered; committed ones are
        let cfg = config_with_rule();
        assert!(committed_events(&cfg, 200).unwrap().is_empty());
        commit("c1", "req-1", "op_1").unwrap();
        commit("c1", "req-1", "op_1").unwrap();
        assert_eq!(
            commit("c1", "req-1", "op_9").unwrap_err().kind(),
            ErrorKind::RequestConflict
        );
        assert_eq!(
            commit("c9", "x", "op_1").unwrap_err().kind(),
            ErrorKind::Missing
        );
        let events = committed_events(&cfg, 200).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, "op_1");
        assert_eq!(events[0].source, "agent");
        assert_eq!(events[0].badge, "triage");
        assert_eq!(pending_client("op_1").as_deref(), Some("c1"));
        assert_eq!(
            revision_of("op_1").as_deref(),
            Some("b".repeat(64).as_str())
        );
        // handled is a consumption fact: no second offer, no revival
        assert!(handled("op_1", "done", Some("S1"), None, 300).unwrap());
        assert!(!handled("op_1", "done", Some("S1"), None, 301).unwrap());
        assert!(committed_events(&cfg, 302).unwrap().is_empty());
        assert_eq!(
            handled("op_x", "done", None, None, 1).unwrap_err().kind(),
            ErrorKind::Missing
        );
        assert_eq!(
            handled("op_3", "skipped", None, Some("nonsense"), 1)
                .unwrap_err()
                .kind(),
            ErrorKind::Invalid
        );
        // the file reloads as written
        redirect_for_tests(None);
        redirect(&dir);
        assert_eq!(pending_client("op_1").as_deref(), Some("c1"));
        redirect_for_tests(None);
    }

    #[test]
    fn text_is_canonical_bounded_and_the_inbox_has_capacity_limits() {
        let _serial = STORE_TESTS.lock_or_recover();
        let dir = TestDir::new("agent-inbox-bounds");
        redirect(&dir);
        let s = submission("c1", "r", "op_t", "a\u{202E}b\x07c\nd");
        let (id, _) = stage(s, 1).unwrap();
        commit("c1", "r", &id).unwrap();
        let events = committed_events(&config_with_rule(), 2).unwrap();
        assert_eq!(events[0].text, "abc\nd");
        let mut long = submission("c1", "r2", "op_l", "x");
        long.text = "x".repeat(MAX_TEXT_BYTES + 1);
        assert_eq!(stage(long, 1).unwrap_err().kind(), ErrorKind::Invalid);
        let mut field = submission("c1", "r3", "op_f", "x");
        field.link = "y".repeat(MAX_FIELD_BYTES + 1);
        assert_eq!(stage(field, 1).unwrap_err().kind(), ErrorKind::Invalid);
        let mut bad_rule = submission("c1", "r4", "op_r", "x");
        bad_rule.rule = "Not A Badge".into();
        assert_eq!(stage(bad_rule, 1).unwrap_err().kind(), ErrorKind::Invalid);
        let empty = submission("c1", "r5", "op_e", "   ");
        assert_eq!(stage(empty, 1).unwrap_err().kind(), ErrorKind::Invalid);
        redirect_for_tests(None);
    }

    #[test]
    fn revocation_retires_unhandled_entries_and_a_missing_rule_retires_at_poll() {
        let _serial = STORE_TESTS.lock_or_recover();
        let dir = TestDir::new("agent-inbox-revoke");
        redirect(&dir);
        for (client, req, op) in [
            ("c1", "a", "op_a"),
            ("c1", "b", "op_b"),
            ("c2", "c", "op_c"),
        ] {
            let (id, _) = stage(submission(client, req, op, "x"), 1).unwrap();
            commit(client, req, &id).unwrap();
        }
        handled("op_b", "done", Some("S"), None, 2).unwrap();
        let retired = reject_client("c1", 3).unwrap();
        assert_eq!(retired, ["op_a"]);
        assert!(reject_client("c1", 4).unwrap().is_empty());
        let cfg = config_with_rule();
        let events = committed_events(&cfg, 5).unwrap();
        assert_eq!(events.len(), 1, "only the other client's entry is offered");
        assert_eq!(events[0].key, "op_c");
        // the rule is gone: the entry is retired by the poll itself
        let empty = crate::inbound::config_from_value(Some(&json!({"rules": []})));
        assert!(committed_events(&empty, 6).unwrap().is_empty());
        assert!(committed_events(&cfg, 7).unwrap().is_empty());
        assert!(reject_all(8).unwrap().is_empty());
        redirect_for_tests(None);
    }

    #[test]
    fn the_revision_is_the_webview_grant_digest_over_the_current_template() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../ui/test/fixtures/automation-grant.json"))
                .unwrap();
        let mut rule: Rule = serde_json::from_value(fixture["rule"].clone()).unwrap();
        let board = json!({"projects": [{"id": rule.project_id, "templates": [
            {"name": rule.template, "steps": fixture["templateSteps"]}
        ]}]});
        let digest = fixture["rule"]["autoSend"]["digest"].as_str().unwrap();
        assert_eq!(revision_in(&rule, &board).as_deref(), Some(digest));
        // without the approval the revision only differs by the external flag
        rule.auto_send = None;
        let plain = revision_in(&rule, &board).unwrap();
        assert_eq!(plain.len(), 64);
        assert_ne!(plain, digest, "the fixture acknowledges external content");
        // the Board without the template: no revision
        assert!(revision_in(&rule, &json!({"projects": []})).is_none());
    }

    #[test]
    fn the_source_is_enabled_by_an_enabled_agent_rule_and_needs_no_baseline() {
        let source = AgentSource::default();
        assert_eq!(source.id(), "agent");
        assert!(source.polled_events_are_live());
        assert!(!source.enabled(&crate::inbound::config_from_value(Some(
            &json!({"rules": []})
        ))));
        assert!(source.enabled(&config_with_rule()));
        let paused = crate::inbound::config_from_value(Some(&json!({"rules": [{
            "id": "r1", "source": "agent", "badge": "triage", "projectId": "P1",
            "columnId": "C1", "cmd": "claude", "template": "mention", "enabled": false
        }]})));
        assert!(!source.enabled(&paused));
    }
}
