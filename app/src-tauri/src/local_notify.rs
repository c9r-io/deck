//! Local notifications: the one generic entry through which programs of the
//! same macOS user hand Deck a notification, the durable inbox that holds
//! them, and the dispatcher source that routes them to the user's local
//! notification rules ("本机通知").
//!
//! # Contract
//! - Division of labour. A producer (a script, a CI watcher, an adapter for
//!   some platform) owns its platform's credentials, collection, catch-up and
//!   conversion into a notification. Deck receives it reliably, shows it,
//!   keeps its attention state and routes it through the user's rules into
//!   the ordinary card and queue paths. The Agent the run starts owns the
//!   work. A new producer needs no Deck code: `producer` is data.
//! - The entry. ONE stream Unix socket, `~/.deck/notify.sock` (0600 inside
//!   the 0700 data directory), bound only while Notifications ›
//!   "Accept local notifications" (`inbound.sources.local.enabled`) is on and
//!   unlinked when it is turned off (`reconcile`). A peer must be the same
//!   uid (`getpeereid`); one newline-terminated JSON request of at most
//!   `MAX_REQUEST_BYTES` per connection, complete within `REQUEST_DEADLINE`;
//!   at most `MAX_CONNECTIONS` at once; at most `RATE_PER_MINUTE` accepted
//!   publishes a minute. It is not an MCP tool, never reaches the Secure
//!   Tunnel, and there is no TCP or HTTP listener. The bundled client is
//!   `deck-notify` (`deck-notify/`); the wire is documented for it only, the
//!   public contract is the CLI (`docs/local-notifications.md`).
//! - What "local" proves. A connection from a process of the same OS user on
//!   this Mac (an SSH session of that user included). It does not prove who
//!   is at the machine, which program connected, or that the text came from
//!   the service it names. `producer` is a namespace for display, filtering
//!   and rule matching, never a verified identity; Deck records the entry
//!   (`local-cli`) and its own receive time, and nothing in the input can
//!   claim to be Slack, an Agent's Signal or anything else Deck trusts.
//! - The public input (version 1) carries notification content and
//!   classification ONLY: identity (`producer`, `event_id`,
//!   `notification_key`), `operation`, `revision`, `type`, `title`, `body`,
//!   `url`, `labels`, `occurred_at`. Unknown fields are refused, so no input
//!   can name a project, command, template, card, session, callback or
//!   approval: what happens to a notification is decided by the user's
//!   rules in Deck. Labels may be matched by a rule; they never become
//!   permission and are not part of what a template can send.
//! - Normalization, then fingerprint. Text fields are canonicalized
//!   (`admission::strip_invisible`, control characters removed, a title is
//!   one line, a body keeps newlines and tabs, trimmed), defaults are filled
//!   in, and only then is the SHA-256 of the canonical JSON taken
//!   (`fingerprint`). Bounds are checked on the normalized values and refuse
//!   (never truncate), so what is compared, stored and shown is one thing.
//! - Three identities. The EVENT `(producer, event_id)` is the idempotency
//!   key: the same event with the same fingerprint is `duplicate` (the
//!   original receipt), with another fingerprint `CONFLICT`. The OBJECT
//!   `(producer, notification_key)`, `notification_key` defaulting to
//!   `event_id`, is the notification several events update. ORDER is
//!   `revision` (default 0): an event applies to an existing object only when
//!   its revision is greater than the stored one, or both are 0 (arrival
//!   order, for producers that do not version); otherwise it is accepted and
//!   recorded but changes nothing (`applied: false`, `stale-revision`). The
//!   caller's `occurred_at` is shown, never used for order or authority.
//! - Accepted means stored. `publish` answers `accepted` only after the
//!   normalized notification and its event record are written atomically
//!   (`ledger`/`datadir::atomic_write`); `inbound::offer` or a webview event
//!   alone never counts. Accepted is not "shown", "routed", "received by an
//!   Agent" or "done": routing is reported separately (`status`).
//! - Two state axes. EXTERNAL (`active`, `resolved`, `withdrawn`) is what the
//!   producer reports; ATTENTION (`open`, `dismissed`, `routed`) is Deck's.
//!   A resolve or withdraw never cancels a run, moves or closes a card; an
//!   upsert on a resolved or withdrawn notification starts its next
//!   OCCURRENCE (attention open again). Updates while active refresh what is
//!   shown and never start work; a dismissed notification stays dismissed
//!   and is shown as updated, not counted again.
//! - The claim. Every route — the user's choice in the list
//!   (`local_notification_route`) and a rule's automatic one (`auto_route`)
//!   — goes through `claim`, under the inbox lock, in one write: the
//!   notification still exists, its occurrence (and, for the user, the
//!   revision they saw) is current, and that occurrence has no live claim.
//!   The exclusive key is the occurrence, never the rule, so two rules or a
//!   rule and a click cannot both take it; the loser gets the existing
//!   claim. The claim freezes the content (`Frozen`) and the rule's revision
//!   (`inbound_agent::revision_in`: rule, template steps, approval), and
//!   gets a fresh id (the dispatcher event key, unique per attempt).
//!   Automatic: the FIRST enabled local rule in settings order with `auto`
//!   on whose match fits, for an active, open notification whose current
//!   occurrence began at or after the rule's `since` (a rule never reaches
//!   back into history) and was not tried automatically before.
//! - Dispatch. A claim is offered to the dispatcher (`inbound::offer`, live)
//!   as `Event {source: local, key: claim id, badge: rule id}`; the poll
//!   (`LocalSource`) offers every pending claim again, so a restart before
//!   the card resumes. From there it is the badge path unchanged: the
//!   webview's `planInbound` (a card whose origin is the claim is the
//!   duplicate that reconciles a crash between card and ack), the channel
//!   admission, the frozen plan, the queue, `inbound_ack` → `handled`.
//!   `inbound_pending` marks a claim `blocked: rule-changed` when its rule
//!   or template moved since it was frozen.
//! - Consumption facts. A claim's outcome (`done` with its card, `skipped`
//!   with a closed reason) lives in the inbox, independent of any card: a
//!   deleted card, a cleared list, a restart or a rule edit never revives it.
//!   A skipped claim gives the notification back (attention open) without
//!   another automatic attempt; the user may route it again. A claim whose
//!   rule was deleted is skipped `no-rule` by the poll. An ambiguous claim
//!   (card perhaps created, ack not recorded) stays pending and is
//!   reconciled by origin, never released into a second card.
//! - Authority. A local rule's automation approval (`TriggerClass::Local`)
//!   and first-send choice (`FirstSendOrigin::Local`) are its own, judged by
//!   `scheduler/authority.rs` and `scheduler/first_send.rs` over the
//!   dispatcher's own copy of the claimed event, exactly as for a Slack badge
//!   rule; the grant also covers the rule's match. Being local is never
//!   content authority; the rows stay `external`.
//! - Retention and capacity. Event records are the dedupe window: kept
//!   while their notification exists, at most `EVENTS_PER_NOTICE` per
//!   notification, and pruned `RETENTION_SECS` after their notification
//!   left. A notification leaves `RETENTION_SECS` after its last change once
//!   it holds no pending claim and is no longer open and active; an open,
//!   active one is never evicted — a full inbox refuses (`CAPACITY`) and
//!   writes nothing. Dedupe is therefore not forever and delivery is not
//!   end-to-end exactly-once.
//! - Privacy. The file holds what producers submitted (no credential);
//!   app.log gets counts and closed codes, never text, producer names or ids.
//!   The change event `local-notifications-changed` is content-free.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use crate::applog::applog;
use crate::datadir::now_epoch;
use crate::error::{DeckError, ErrorKind};
use crate::inbound::{Config, Event, Rule, SourceStatus};
use crate::sync::LockRecover;

pub(crate) const SOURCE: &str = "local";
/// The socket wire between `deck-notify` and Deck.
pub(crate) const WIRE_PROTOCOL: u64 = 1;
/// The public notification schema `version`.
pub(crate) const INPUT_VERSION: u64 = 1;
const FILE_VERSION: u32 = 1;
pub(crate) const MAX_REQUEST_BYTES: usize = 64 * 1024;
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
const MAX_CONNECTIONS: usize = 8;
const RATE_PER_MINUTE: usize = 600;
const MAX_FILE_BYTES: usize = 48 * 1024 * 1024;
pub(crate) const MAX_NOTICES: usize = 1000;
pub(crate) const MAX_EVENTS: usize = 20_000;
const EVENTS_PER_NOTICE: usize = 256;
const CLAIMS_PER_NOTICE: usize = 16;
const RETENTION_SECS: u64 = 30 * 24 * 3600;
pub(crate) const MAX_ID_BYTES: usize = 128;
pub(crate) const MAX_NAME_BYTES: usize = 64;
pub(crate) const MAX_TITLE_BYTES: usize = 512;
pub(crate) const MAX_BODY_BYTES: usize = 16 * 1024;
pub(crate) const MAX_URL_BYTES: usize = 2048;
pub(crate) const MAX_LABELS: usize = 16;
pub(crate) const MAX_LABEL_VALUE_BYTES: usize = 256;
const DEFAULT_TYPE: &str = "notification";
/// Closed reasons a skipped claim may carry: the dispatcher's skip reasons
/// plus `no-rule` (written here).
const HANDLED_REASONS: &[&str] = &[
    "busy",
    "no-rule-target",
    "no-template",
    "missed",
    "blocked",
    "rule-changed",
    "no-rule",
];

/* ---------- errors on the wire ---------- */

/// A refusal with its closed code; `retryable` tells the caller whether the
/// same request may succeed later.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Refusal {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub(crate) fn retryable(&self) -> bool {
        matches!(
            self.code,
            "DISABLED" | "CAPACITY" | "RATE_LIMITED" | "BUSY" | "INTERNAL"
        )
    }
    fn to_value(&self) -> Value {
        serde_json::json!({
            "protocol": WIRE_PROTOCOL,
            "status": "error",
            "code": self.code,
            "message": self.message,
            "retryable": self.retryable(),
        })
    }
}

fn invalid(message: &str) -> Refusal {
    Refusal::new("INVALID_INPUT", message)
}

/* ---------- the public input and its normalization ---------- */

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    #[allow(dead_code)]
    version: Value,
    producer: String,
    event_id: String,
    #[serde(default)]
    notification_key: Option<String>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    occurred_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Operation {
    Upsert,
    Resolve,
    Withdraw,
}

impl Operation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Resolve => "resolve",
            Self::Withdraw => "withdraw",
        }
    }
}

/// One event as Deck keeps it: every field normalized, defaults filled in.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Normalized {
    pub(crate) producer: String,
    pub(crate) event_id: String,
    pub(crate) key: String,
    pub(crate) operation: Operation,
    pub(crate) revision: u64,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) url: String,
    pub(crate) labels: BTreeMap<String, String>,
    pub(crate) occurred_at: Option<u64>,
}

/// `producer`, `type` and label names: lower-case ASCII words.
pub(crate) fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_NAME_BYTES
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-')
        })
}

/// `event_id` and `notification_key`: printable ASCII identifiers.
pub(crate) fn valid_ident(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= MAX_ID_BYTES
        && b[0].is_ascii_alphanumeric()
        && b.iter().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-' | b':' | b'@' | b'/' | b'+')
        })
}

/// A title or label value: one line, invisible formatting removed.
fn one_line(s: &str) -> String {
    let stripped = crate::admission::strip_invisible(s);
    let mut out = String::with_capacity(stripped.len());
    for c in stripped.chars() {
        let c = if c.is_control() || c.is_whitespace() {
            ' '
        } else {
            c
        };
        if !(c == ' ' && out.ends_with(' ')) {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// A body: newlines and tabs kept, other control characters dropped.
fn multi_line(s: &str) -> String {
    crate::admission::strip_invisible(&s.replace("\r\n", "\n").replace('\r', "\n"))
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .trim()
        .to_string()
}

fn valid_url(s: &str) -> bool {
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        !host.is_empty()
            && s.len() <= MAX_URL_BYTES
            && !s.chars().any(|c| c.is_control() || c.is_whitespace())
    })
}

/// Parse and normalize one notification (module header). The order is
/// fixed: shape, version, normalization, bounds on the normalized values.
pub(crate) fn normalize(raw: &Value) -> Result<Normalized, Refusal> {
    let obj = raw
        .as_object()
        .ok_or_else(|| invalid("a notification is a JSON object"))?;
    match obj.get("version") {
        None => return Err(invalid("`version` is required (1)")),
        Some(v) if v.as_u64() == Some(INPUT_VERSION) => {}
        Some(v) if v.is_u64() => {
            return Err(Refusal::new(
                "UNSUPPORTED_VERSION",
                "this Deck reads notification version 1",
            ))
        }
        Some(_) => return Err(invalid("`version` must be the integer 1")),
    }
    let input: Input = serde_json::from_value(raw.clone()).map_err(|e| {
        let text = e.to_string();
        if text.starts_with("unknown field") {
            invalid(text.split(',').next().unwrap_or("unknown field"))
        } else {
            invalid("a field has the wrong type")
        }
    })?;
    if !valid_name(&input.producer) {
        return Err(invalid(
            "`producer` is 1-64 of a-z 0-9 . _ - starting with a letter or digit",
        ));
    }
    if !valid_ident(&input.event_id) {
        return Err(invalid("`event_id` is 1-128 of A-Z a-z 0-9 . _ - : @ / +"));
    }
    let key = input
        .notification_key
        .unwrap_or_else(|| input.event_id.clone());
    if !valid_ident(&key) {
        return Err(invalid(
            "`notification_key` is 1-128 of A-Z a-z 0-9 . _ - : @ / +",
        ));
    }
    let operation = match input.operation.as_deref() {
        None | Some("upsert") => Operation::Upsert,
        Some("resolve") => Operation::Resolve,
        Some("withdraw") => Operation::Withdraw,
        Some(_) => return Err(invalid("`operation` is upsert, resolve or withdraw")),
    };
    let revision = input.revision.unwrap_or(0);
    if revision > (1u64 << 53) {
        return Err(invalid("`revision` is an integer up to 2^53"));
    }
    if input.occurred_at.is_some_and(|t| t > (1u64 << 53)) {
        return Err(invalid("`occurred_at` is epoch seconds"));
    }
    if operation != Operation::Upsert {
        if input.title.is_some()
            || input.body.is_some()
            || input.url.is_some()
            || input.labels.is_some()
            || input.kind.is_some()
        {
            return Err(invalid(
                "resolve and withdraw carry identity, revision and occurred_at only",
            ));
        }
        return Ok(Normalized {
            producer: input.producer,
            event_id: input.event_id,
            key,
            operation,
            revision,
            kind: String::new(),
            title: String::new(),
            body: String::new(),
            url: String::new(),
            labels: BTreeMap::new(),
            occurred_at: input.occurred_at,
        });
    }
    let kind = input.kind.unwrap_or_else(|| DEFAULT_TYPE.to_string());
    if !valid_name(&kind) {
        return Err(invalid("`type` is 1-64 of a-z 0-9 . _ -"));
    }
    let title = one_line(input.title.as_deref().unwrap_or_default());
    if title.is_empty() {
        return Err(invalid("`title` is required for upsert"));
    }
    if title.len() > MAX_TITLE_BYTES {
        return Err(Refusal::new(
            "TOO_LARGE",
            format!("`title` is at most {MAX_TITLE_BYTES} bytes"),
        ));
    }
    let body = multi_line(input.body.as_deref().unwrap_or_default());
    if body.len() > MAX_BODY_BYTES {
        return Err(Refusal::new(
            "TOO_LARGE",
            format!("`body` is at most {MAX_BODY_BYTES} bytes"),
        ));
    }
    let url = input.url.unwrap_or_default().trim().to_string();
    if !url.is_empty() && !valid_url(&url) {
        return Err(invalid("`url` is an http(s) URL of at most 2048 bytes"));
    }
    let mut labels = BTreeMap::new();
    for (name, value) in input.labels.unwrap_or_default() {
        if !valid_name(&name) {
            return Err(invalid("a label name is 1-64 of a-z 0-9 . _ -"));
        }
        let value = one_line(&value);
        if value.is_empty() || value.len() > MAX_LABEL_VALUE_BYTES {
            return Err(invalid("a label value is one line of 1-256 bytes"));
        }
        labels.insert(name, value);
    }
    if labels.len() > MAX_LABELS {
        return Err(invalid("at most 16 labels"));
    }
    Ok(Normalized {
        producer: input.producer,
        event_id: input.event_id,
        key,
        operation,
        revision,
        kind,
        title,
        body,
        url,
        labels,
        occurred_at: input.occurred_at,
    })
}

/// SHA-256 of the normalized event's canonical JSON (module header).
pub(crate) fn fingerprint(n: &Normalized) -> String {
    let canonical = serde_json::json!([
        "deck-local-notification",
        INPUT_VERSION,
        n.producer,
        n.event_id,
        n.key,
        n.operation.as_str(),
        n.revision,
        n.kind,
        n.title,
        n.body,
        n.url,
        n.labels,
        n.occurred_at,
    ]);
    crate::ledger::sha(canonical.to_string().as_bytes())
}

/* ---------- the inbox file ---------- */

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum External {
    Active,
    Resolved,
    Withdrawn,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClaimState {
    Pending,
    Done,
    Skipped,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ClaimBy {
    Auto,
    Manual,
}

/// The content a claim took, frozen: later events never change it.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) struct Frozen {
    pub(crate) revision: u64,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Claim {
    pub(crate) id: String,
    pub(crate) occurrence: u32,
    pub(crate) rule: String,
    pub(crate) rule_revision: String,
    pub(crate) by: ClaimBy,
    pub(crate) at: u64,
    pub(crate) frozen: Frozen,
    pub(crate) state: ClaimState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) card: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) handled_at: Option<u64>,
}

impl Claim {
    fn live(&self) -> bool {
        matches!(self.state, ClaimState::Pending | ClaimState::Done)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Notice {
    /// `n-` + a digest of `(producer, key)`.
    pub(crate) id: String,
    pub(crate) producer: String,
    pub(crate) key: String,
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) url: String,
    pub(crate) labels: BTreeMap<String, String>,
    pub(crate) revision: u64,
    pub(crate) occurrence: u32,
    /// when the current occurrence began (Deck's clock)
    pub(crate) occurrence_at: u64,
    /// what the producer last reported (`external` on disk and on the wire)
    #[serde(rename = "external")]
    pub(crate) reported: External,
    /// Deck's receive time of the first event
    pub(crate) first_at: u64,
    /// Deck's receive time of the last event that changed it
    pub(crate) changed_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) occurred_at: Option<u64>,
    /// `(occurrence, at)` when the user dismissed it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) dismissed: Option<(u32, u64)>,
    /// the occurrence a rule already tried automatically
    #[serde(default)]
    pub(crate) auto_tried: u32,
    #[serde(default)]
    pub(crate) claims: Vec<Claim>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Attention {
    Open,
    Dismissed,
    Routed,
}

impl Notice {
    fn live_claim(&self) -> Option<&Claim> {
        self.claims
            .iter()
            .rev()
            .find(|c| c.occurrence == self.occurrence && c.live())
    }
    pub(crate) fn attention(&self) -> Attention {
        if self.live_claim().is_some() {
            Attention::Routed
        } else if self
            .dismissed
            .is_some_and(|(occurrence, _)| occurrence == self.occurrence)
        {
            Attention::Dismissed
        } else {
            Attention::Open
        }
    }
    fn prunable(&self, now: u64) -> bool {
        let pending = self.claims.iter().any(|c| c.state == ClaimState::Pending);
        let open = self.reported == External::Active && self.attention() == Attention::Open;
        !pending && !open && self.changed_at.saturating_add(RETENTION_SECS) < now
    }
    /// What `{{msg.*}}` sees of a claim (module header of the docs).
    fn event(&self, claim: &Claim) -> Event {
        let f = &claim.frozen;
        Event {
            source: SOURCE.into(),
            key: claim.id.clone(),
            badge: claim.rule.clone(),
            text: if f.body.is_empty() {
                f.title.clone()
            } else {
                format!("{}\n{}", f.title, f.body)
            },
            from: self.producer.clone(),
            where_: f.kind.clone(),
            link: f.url.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventRecord {
    pub(crate) producer: String,
    pub(crate) event_id: String,
    pub(crate) fingerprint: String,
    pub(crate) notice: String,
    pub(crate) key: String,
    pub(crate) operation: Operation,
    pub(crate) revision: u64,
    pub(crate) received_at: u64,
    pub(crate) applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Doc {
    version: u32,
    #[serde(default)]
    seq: u64,
    #[serde(default)]
    notices: Vec<Notice>,
    #[serde(default)]
    events: Vec<EventRecord>,
}

impl Default for Doc {
    fn default() -> Self {
        Self {
            version: FILE_VERSION,
            seq: 0,
            notices: Vec::new(),
            events: Vec::new(),
        }
    }
}

fn notice_id(producer: &str, key: &str) -> String {
    let digest = crate::ledger::sha(format!("deck-local-notice\n{producer}\n{key}").as_bytes());
    format!("n-{}", &digest[..32])
}

fn valid_doc(doc: &Doc) -> bool {
    use crate::inbound::{bounded_id, valid_badge};
    let mut ids = std::collections::HashSet::new();
    let mut events = std::collections::HashSet::new();
    doc.notices.len() <= MAX_NOTICES
        && doc.events.len() <= MAX_EVENTS
        && doc.notices.iter().all(|n| {
            ids.insert(n.id.clone())
                && n.id == notice_id(&n.producer, &n.key)
                && valid_name(&n.producer)
                && valid_ident(&n.key)
                && valid_name(&n.kind)
                && n.title.len() <= MAX_TITLE_BYTES
                && n.body.len() <= MAX_BODY_BYTES
                && n.url.len() <= MAX_URL_BYTES
                && n.labels.len() <= MAX_LABELS
                && n.occurrence >= 1
                && n.claims.len() <= CLAIMS_PER_NOTICE
                && n.claims.iter().all(|c| {
                    bounded_id(&c.id, 128)
                        && valid_badge(&c.rule)
                        && c.card.as_ref().is_none_or(|card| bounded_id(card, 128))
                        && c.reason
                            .as_ref()
                            .is_none_or(|r| HANDLED_REASONS.contains(&r.as_str()))
                })
        })
        && doc
            .events
            .iter()
            .all(|e| events.insert((e.producer.clone(), e.event_id.clone())))
}

struct Store {
    path: PathBuf,
    doc: Doc,
}

impl Store {
    fn load(path: PathBuf) -> Result<Self, DeckError> {
        let doc = crate::ledger::load_bounded::<Doc>(&path, MAX_FILE_BYTES, "notification inbox")?
            .unwrap_or_default();
        if doc.version != FILE_VERSION {
            return Err(DeckError::new(
                ErrorKind::NewerSchema,
                "notification inbox is from a newer deck",
            ));
        }
        if !valid_doc(&doc) {
            return Err(DeckError::new(
                ErrorKind::Recovery,
                "notification inbox exceeds its bounds",
            ));
        }
        Ok(Self { path, doc })
    }

    fn mutate<T>(&mut self, f: impl FnOnce(&mut Doc) -> Result<T, Refusal>) -> Result<T, Refusal> {
        let mut next = self.doc.clone();
        let out = f(&mut next)?;
        // never write a file the next load would refuse (a notification
        // holding more pending claims than it may keep, say): refuse instead
        if !valid_doc(&next) {
            return Err(Refusal::new("CAPACITY", "the notification inbox is full"));
        }
        let bytes = crate::ledger::encode(&next, "notification inbox")
            .map_err(|_| Refusal::new("INTERNAL", "the notification inbox could not be encoded"))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Refusal::new("CAPACITY", "the notification inbox is full"));
        }
        #[cfg(test)]
        if FAIL_WRITE.with(|f| f.get()) {
            return Err(Refusal::new(
                "INTERNAL",
                "the notification inbox could not be written",
            ));
        }
        crate::datadir::atomic_write(&self.path, &bytes).map_err(|e| {
            applog(&format!("[local] inbox save FAILED ({})", e.code()));
            Refusal::new("INTERNAL", "the notification inbox could not be written")
        })?;
        self.doc = next;
        Ok(out)
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_WRITE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn inbox_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = TEST_PATH.lock_or_recover().clone() {
        return path;
    }
    crate::datadir::deck_dir().join("notifications.json")
}

static STORE: Mutex<Option<Result<Store, DeckError>>> = Mutex::new(None);
#[cfg(test)]
static TEST_PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
pub(crate) static STORE_TESTS: Mutex<()> = Mutex::new(());

fn with_store<T>(f: impl FnOnce(&mut Store) -> Result<T, Refusal>) -> Result<T, Refusal> {
    let mut guard = STORE.lock_or_recover();
    let wanted = inbox_path();
    let stale = match &*guard {
        Some(Ok(store)) => store.path != wanted,
        // an unreadable inbox is read again on the next use: a file the user
        // repaired or a disk that came back is picked up without a restart
        Some(Err(_)) | None => true,
    };
    if stale {
        *guard = Some(Store::load(wanted));
    }
    match guard.as_mut().expect("loaded above") {
        Ok(store) => f(store),
        Err(e) => {
            if e.kind() == ErrorKind::NewerSchema {
                Err(Refusal::new(
                    "INTERNAL",
                    "the notification inbox was written by a newer Deck",
                ))
            } else {
                Err(Refusal::new(
                    "INTERNAL",
                    "the notification inbox cannot be read",
                ))
            }
        }
    }
}

fn prune(doc: &mut Doc, now: u64) {
    doc.notices.retain(|n| !n.prunable(now));
    let live: std::collections::HashSet<&str> = doc.notices.iter().map(|n| n.id.as_str()).collect();
    doc.events.retain(|e| {
        live.contains(e.notice.as_str()) || e.received_at.saturating_add(RETENTION_SECS) >= now
    });
}

/* ---------- publish and status ---------- */

/// What a caller is told about one event: `status` is `accepted` or
/// `duplicate`.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub(crate) struct Receipt {
    pub(crate) protocol: u64,
    pub(crate) status: &'static str,
    pub(crate) producer: String,
    pub(crate) event_id: String,
    pub(crate) notification_key: String,
    pub(crate) received_at: u64,
    pub(crate) applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    pub(crate) notification: NoticeState,
}

/// A notification's state as a caller sees it.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub(crate) struct NoticeState {
    pub(crate) external: Option<External>,
    pub(crate) attention: Option<Attention>,
    pub(crate) revision: Option<u64>,
    pub(crate) occurrence: Option<u32>,
    /// the latest claim of the current occurrence: who took it and how far
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) routing: Option<Routing>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub(crate) struct Routing {
    pub(crate) by: ClaimBy,
    pub(crate) rule: String,
    pub(crate) state: ClaimState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) card: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

fn notice_state(n: Option<&Notice>) -> NoticeState {
    let Some(n) = n else {
        return NoticeState {
            external: None,
            attention: None,
            revision: None,
            occurrence: None,
            routing: None,
        };
    };
    NoticeState {
        external: Some(n.reported),
        attention: Some(n.attention()),
        revision: Some(n.revision),
        occurrence: Some(n.occurrence),
        routing: n
            .claims
            .iter()
            .rev()
            .find(|c| c.occurrence == n.occurrence)
            .map(|c| Routing {
                by: c.by,
                rule: c.rule.clone(),
                state: c.state,
                card: c.card.clone(),
                reason: c.reason.clone(),
            }),
    }
}

fn receipt(doc: &Doc, record: &EventRecord, status: &'static str) -> Receipt {
    Receipt {
        protocol: WIRE_PROTOCOL,
        status,
        producer: record.producer.clone(),
        event_id: record.event_id.clone(),
        notification_key: record.key.clone(),
        received_at: record.received_at,
        applied: record.applied,
        reason: record.reason.clone(),
        notification: notice_state(doc.notices.iter().find(|n| n.id == record.notice)),
    }
}

/// Accepted publishes in the last minute (in memory: a restart forgets).
static RATE: Mutex<Vec<Instant>> = Mutex::new(Vec::new());

fn rate_allows() -> bool {
    let mut window = RATE.lock_or_recover();
    window.retain(|t| t.elapsed() < Duration::from_secs(60));
    window.len() < RATE_PER_MINUTE
}

fn rate_note() {
    RATE.lock_or_recover().push(Instant::now());
}

/// Receive one event (module header). `Ok((receipt, changed))`: `changed`
/// when the inbox now holds something a rule may route.
pub(crate) fn publish(raw: &Value, now: u64) -> Result<(Receipt, bool), Refusal> {
    let n = normalize(raw)?;
    let fp = fingerprint(&n);
    with_store(|store| {
        if let Some(existing) = store
            .doc
            .events
            .iter()
            .find(|e| e.producer == n.producer && e.event_id == n.event_id)
        {
            if existing.fingerprint != fp {
                return Err(Refusal::new(
                    "CONFLICT",
                    "this producer already used this event_id for different content",
                ));
            }
            return Ok((receipt(&store.doc, existing, "duplicate"), false));
        }
        if !rate_allows() {
            return Err(Refusal::new(
                "RATE_LIMITED",
                "too many notifications this minute; retry later",
            ));
        }
        let out = store.mutate(|doc| {
            prune(doc, now);
            let id = notice_id(&n.producer, &n.key);
            let at = doc.notices.iter().position(|x| x.id == id);
            if doc.events.len() >= MAX_EVENTS || (at.is_none() && doc.notices.len() >= MAX_NOTICES)
            {
                return Err(Refusal::new(
                    "CAPACITY",
                    "the notification inbox is full; resolve, dismiss or route some first",
                ));
            }
            let (applied, reason, changed) = apply(doc, at, &id, &n, now);
            let record = EventRecord {
                producer: n.producer.clone(),
                event_id: n.event_id.clone(),
                fingerprint: fp.clone(),
                notice: id.clone(),
                key: n.key.clone(),
                operation: n.operation,
                revision: n.revision,
                received_at: now,
                applied,
                reason,
            };
            doc.events.push(record.clone());
            // the dedupe window of one notification is its last events
            let mine: Vec<usize> = doc
                .events
                .iter()
                .enumerate()
                .filter(|(_, e)| e.notice == id)
                .map(|(i, _)| i)
                .collect();
            if mine.len() > EVENTS_PER_NOTICE {
                let drop: std::collections::HashSet<usize> = mine[..mine.len() - EVENTS_PER_NOTICE]
                    .iter()
                    .copied()
                    .collect();
                let mut i = 0;
                doc.events.retain(|_| {
                    let keep = !drop.contains(&i);
                    i += 1;
                    keep
                });
            }
            Ok((receipt(doc, &record, "accepted"), changed))
        })?;
        rate_note();
        Ok(out)
    })
}

/// Apply a fresh event to the doc: `(applied, reason, routable change)`.
fn apply(
    doc: &mut Doc,
    at: Option<usize>,
    id: &str,
    n: &Normalized,
    now: u64,
) -> (bool, Option<String>, bool) {
    let Some(at) = at else {
        if n.operation != Operation::Upsert {
            return (false, Some("unknown-notification".into()), false);
        }
        doc.notices.push(Notice {
            id: id.to_string(),
            producer: n.producer.clone(),
            key: n.key.clone(),
            kind: n.kind.clone(),
            title: n.title.clone(),
            body: n.body.clone(),
            url: n.url.clone(),
            labels: n.labels.clone(),
            revision: n.revision,
            occurrence: 1,
            occurrence_at: now,
            reported: External::Active,
            first_at: now,
            changed_at: now,
            occurred_at: n.occurred_at,
            dismissed: None,
            auto_tried: 0,
            claims: Vec::new(),
        });
        return (true, None, true);
    };
    let notice = &mut doc.notices[at];
    let applies = n.revision > notice.revision || (n.revision == 0 && notice.revision == 0);
    if !applies {
        return (false, Some("stale-revision".into()), false);
    }
    notice.revision = n.revision;
    notice.changed_at = now;
    if n.occurred_at.is_some() {
        notice.occurred_at = n.occurred_at;
    }
    match n.operation {
        Operation::Upsert => {
            let reopened = notice.reported != External::Active;
            if reopened {
                notice.occurrence = notice.occurrence.saturating_add(1);
                notice.occurrence_at = now;
                notice.reported = External::Active;
            }
            notice.kind = n.kind.clone();
            notice.title = n.title.clone();
            notice.body = n.body.clone();
            notice.url = n.url.clone();
            notice.labels = n.labels.clone();
            (true, None, reopened)
        }
        Operation::Resolve => {
            notice.reported = External::Resolved;
            (true, None, false)
        }
        Operation::Withdraw => {
            notice.reported = External::Withdrawn;
            (true, None, false)
        }
    }
}

/// The receipt of `(producer, event_id)` as it stands now, or `None`.
pub(crate) fn status(producer: &str, event_id: &str) -> Result<Option<Receipt>, Refusal> {
    with_store(|store| {
        Ok(store
            .doc
            .events
            .iter()
            .find(|e| e.producer == producer && e.event_id == event_id)
            .map(|e| receipt(&store.doc, e, "found")))
    })
}

/* ---------- matching and the claim ---------- */

/// A rule pattern: the exact value, or a prefix ending in `*` (`*` alone
/// matches every value).
pub(crate) fn pattern_matches(pattern: &str, value: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => value.starts_with(prefix),
        None => pattern == value,
    }
}

/// Whether `rule`'s match fits notification `n`.
pub(crate) fn rule_matches(rule: &Rule, n: &Notice) -> bool {
    let Some(m) = rule.local_match.as_ref() else {
        return false;
    };
    pattern_matches(&m.producer, &n.producer)
        && (m.types.is_empty() || m.types.iter().any(|t| pattern_matches(t, &n.kind)))
        && m.labels
            .iter()
            .all(|(k, v)| n.labels.get(k).is_some_and(|have| pattern_matches(v, have)))
}

/// Why a claim was refused (closed words for the webview).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ClaimRefusal {
    Missing,
    /// the notification changed since the caller saw it
    Stale,
    /// the occurrence already has a live claim
    Taken,
    Unavailable,
}

impl ClaimRefusal {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::Taken => "taken",
            Self::Unavailable => "unavailable",
        }
    }
}

fn take(
    doc: &mut Doc,
    at: usize,
    rule: &Rule,
    rule_revision: &str,
    by: ClaimBy,
    now: u64,
) -> Event {
    doc.seq = doc.seq.wrapping_add(1);
    let seq = doc.seq;
    let notice = &mut doc.notices[at];
    let digest = crate::ledger::sha(
        format!(
            "deck-local-claim\n{}\n{}\n{seq}",
            notice.id, notice.occurrence
        )
        .as_bytes(),
    );
    let claim = Claim {
        id: format!("ln-{}", &digest[..32]),
        occurrence: notice.occurrence,
        rule: rule.badge.clone(),
        rule_revision: rule_revision.to_string(),
        by,
        at: now,
        frozen: Frozen {
            revision: notice.revision,
            kind: notice.kind.clone(),
            title: notice.title.clone(),
            body: notice.body.clone(),
            url: notice.url.clone(),
        },
        state: ClaimState::Pending,
        card: None,
        reason: None,
        handled_at: None,
    };
    if by == ClaimBy::Auto {
        notice.auto_tried = notice.occurrence;
    }
    notice.claims.push(claim.clone());
    // keep the newest claims; a pending one is never the one dropped
    while notice.claims.len() > CLAIMS_PER_NOTICE {
        match notice
            .claims
            .iter()
            .position(|c| c.state != ClaimState::Pending)
        {
            Some(i) => {
                notice.claims.remove(i);
            }
            None => break,
        }
    }
    notice.event(&claim)
}

/// The user's route of one notification to `rule` (module header): the
/// occurrence and revision they saw must still be current.
pub(crate) fn claim_manual(
    id: &str,
    occurrence: u32,
    revision: u64,
    rule: &Rule,
    rule_revision: &str,
    now: u64,
) -> Result<Event, ClaimRefusal> {
    with_store(|store| {
        let Some(at) = store.doc.notices.iter().position(|n| n.id == id) else {
            return Err(Refusal::new("MISSING", ""));
        };
        let n = &store.doc.notices[at];
        if n.occurrence != occurrence || n.revision != revision {
            return Err(Refusal::new("STALE", ""));
        }
        if n.live_claim().is_some() {
            return Err(Refusal::new("TAKEN", ""));
        }
        store.mutate(|doc| Ok(take(doc, at, rule, rule_revision, ClaimBy::Manual, now)))
    })
    .map_err(|r| match r.code {
        "MISSING" => ClaimRefusal::Missing,
        "STALE" => ClaimRefusal::Stale,
        "TAKEN" => ClaimRefusal::Taken,
        _ => ClaimRefusal::Unavailable,
    })
}

/// The automatic claims a poll may make now (module header). `revision`
/// gives a rule's current revision (`None`: not knowable now, so that rule
/// claims nothing this time).
pub(crate) fn auto_route(
    cfg: &Config,
    revision: impl Fn(&Rule) -> Option<String>,
    now: u64,
) -> Result<Vec<Event>, Refusal> {
    let rules: Vec<(&Rule, String)> = cfg
        .rules
        .iter()
        .filter(|r| {
            r.source == SOURCE && r.enabled && r.local_match.as_ref().is_some_and(|m| m.auto)
        })
        .filter_map(|r| revision(r).map(|rev| (r, rev)))
        .collect();
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    with_store(|store| {
        let wanted: Vec<(usize, usize)> = store
            .doc
            .notices
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.reported == External::Active
                    && n.attention() == Attention::Open
                    && n.auto_tried < n.occurrence
            })
            .filter_map(|(i, n)| {
                rules
                    .iter()
                    .position(|(r, _)| r.since <= n.occurrence_at && rule_matches(r, n))
                    .map(|k| (i, k))
            })
            .collect();
        if wanted.is_empty() {
            return Ok(Vec::new());
        }
        store.mutate(|doc| {
            Ok(wanted
                .iter()
                .map(|&(i, k)| take(doc, i, rules[k].0, &rules[k].1, ClaimBy::Auto, now))
                .collect())
        })
    })
}

/// Every pending claim whose rule exists, as the dispatcher's events; a
/// pending claim whose rule is gone is skipped `no-rule` here.
pub(crate) fn pending_events(cfg: &Config, now: u64) -> Result<Vec<Event>, Refusal> {
    with_store(|store| {
        let rule_exists = |badge: &str| {
            cfg.rules
                .iter()
                .any(|r| r.source == SOURCE && r.badge == badge)
        };
        let orphaned = store
            .doc
            .notices
            .iter()
            .flat_map(|n| n.claims.iter())
            .any(|c| c.state == ClaimState::Pending && !rule_exists(&c.rule));
        if orphaned {
            store.mutate(|doc| {
                for c in doc.notices.iter_mut().flat_map(|n| n.claims.iter_mut()) {
                    if c.state == ClaimState::Pending && !rule_exists(&c.rule) {
                        c.state = ClaimState::Skipped;
                        c.reason = Some("no-rule".into());
                        c.handled_at = Some(now);
                    }
                }
                Ok(())
            })?;
        }
        Ok(store
            .doc
            .notices
            .iter()
            .flat_map(|n| {
                n.claims
                    .iter()
                    .filter(|c| c.state == ClaimState::Pending)
                    .map(move |c| n.event(c))
            })
            .collect())
    })
}

/// The rule revision claim `key` froze (`inbound_pending`).
pub(crate) fn revision_of(key: &str) -> Option<String> {
    with_store(|store| {
        Ok(store
            .doc
            .notices
            .iter()
            .flat_map(|n| n.claims.iter())
            .find(|c| c.id == key)
            .map(|c| c.rule_revision.clone()))
    })
    .ok()
    .flatten()
}

/// The webview acknowledged claim `key`: `done` with its card or `skipped`
/// with a closed reason. Idempotent; a claim already handled keeps its
/// first outcome.
pub(crate) fn handled(
    key: &str,
    outcome: &str,
    card: Option<&str>,
    reason: Option<&str>,
    now: u64,
) -> Result<(), DeckError> {
    let reason = reason.filter(|r| HANDLED_REASONS.contains(r));
    with_store(|store| {
        let pending = store
            .doc
            .notices
            .iter()
            .flat_map(|n| n.claims.iter())
            .any(|c| c.id == key && c.state == ClaimState::Pending);
        if !pending {
            return Ok(());
        }
        store.mutate(|doc| {
            if let Some(c) = doc
                .notices
                .iter_mut()
                .flat_map(|n| n.claims.iter_mut())
                .find(|c| c.id == key)
            {
                if outcome == "done" {
                    c.state = ClaimState::Done;
                    c.card = card.map(str::to_owned);
                } else {
                    c.state = ClaimState::Skipped;
                    c.reason = reason.map(str::to_owned);
                }
                c.handled_at = Some(now);
            }
            Ok(())
        })
    })
    .map_err(|r| DeckError::new(ErrorKind::Other, r.message))
}

/// Set notifications aside (the user dismissed them): each `(id,
/// occurrence)` whose occurrence is current and holds no live claim.
pub(crate) fn dismiss(items: &[(String, u32)], now: u64) -> Result<usize, Refusal> {
    with_store(|store| {
        let wanted: Vec<usize> = store
            .doc
            .notices
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                items
                    .iter()
                    .any(|(id, occ)| *id == n.id && *occ == n.occurrence)
                    && n.attention() == Attention::Open
            })
            .map(|(i, _)| i)
            .collect();
        if wanted.is_empty() {
            return Ok(0);
        }
        store.mutate(|doc| {
            for &i in &wanted {
                let n = &mut doc.notices[i];
                n.dismissed = Some((n.occurrence, now));
            }
            Ok(wanted.len())
        })
    })
}

/* ---------- the webview's view ---------- */

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoticeView {
    id: String,
    producer: String,
    key: String,
    #[serde(rename = "type")]
    kind: String,
    title: String,
    body: String,
    url: String,
    labels: BTreeMap<String, String>,
    revision: u64,
    occurrence: u32,
    external: External,
    attention: Attention,
    first_at: u64,
    changed_at: u64,
    occurred_at: Option<u64>,
    /// dismissed, then changed by its producer since
    updated: bool,
    routing: Option<Routing>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProducerView {
    producer: String,
    last_at: u64,
    day_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalView {
    enabled: bool,
    listening: bool,
    cli_path: Option<String>,
    /// `unreadable` when the inbox cannot be read: nothing is shown as gone
    error: Option<&'static str>,
    items: Vec<NoticeView>,
    producers: Vec<ProducerView>,
}

fn view(enabled: bool, now: u64) -> LocalView {
    let read = with_store(|store| {
        let mut items: Vec<NoticeView> = store
            .doc
            .notices
            .iter()
            .map(|n| NoticeView {
                id: n.id.clone(),
                producer: n.producer.clone(),
                key: n.key.clone(),
                kind: n.kind.clone(),
                title: n.title.clone(),
                body: n.body.clone(),
                url: n.url.clone(),
                labels: n.labels.clone(),
                revision: n.revision,
                occurrence: n.occurrence,
                external: n.reported,
                attention: n.attention(),
                first_at: n.first_at,
                changed_at: n.changed_at,
                occurred_at: n.occurred_at,
                updated: n
                    .dismissed
                    .is_some_and(|(occ, at)| occ == n.occurrence && n.changed_at > at),
                routing: notice_state(Some(n)).routing,
            })
            .collect();
        items.sort_by(|a, b| b.changed_at.cmp(&a.changed_at).then(a.id.cmp(&b.id)));
        let mut producers: BTreeMap<String, ProducerView> = BTreeMap::new();
        for e in &store.doc.events {
            let p = producers
                .entry(e.producer.clone())
                .or_insert_with(|| ProducerView {
                    producer: e.producer.clone(),
                    last_at: 0,
                    day_count: 0,
                });
            p.last_at = p.last_at.max(e.received_at);
            if now.saturating_sub(e.received_at) <= 24 * 3600 {
                p.day_count += 1;
            }
        }
        let mut producers: Vec<ProducerView> = producers.into_values().collect();
        producers.sort_by_key(|p| std::cmp::Reverse(p.last_at));
        Ok((items, producers))
    });
    let (items, producers, error) = match read {
        Ok((items, producers)) => (items, producers, None),
        Err(_) => (Vec::new(), Vec::new(), Some("unreadable")),
    };
    LocalView {
        enabled,
        listening: LISTENING.load(Ordering::SeqCst) == 1,
        cli_path: cli_path(),
        error,
        items,
        producers,
    }
}

/// The bundled `deck-notify` beside the running executable, when present.
pub(crate) fn cli_path() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join("deck-notify");
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

#[tauri::command(async)]
pub(crate) fn local_notifications() -> LocalView {
    let now = now_epoch();
    let config = crate::inbound::read_config_strict();
    // the dispatcher polls this source only while a local rule exists: a
    // claim whose rule was deleted with the last one is retired here, so it
    // never reads as "handing to …" for good
    if let Some(cfg) = config.as_ref() {
        if !cfg.rules.iter().any(|r| r.source == SOURCE) {
            let _ = pending_events(cfg, now);
        }
    }
    view(config.is_some_and(|c| c.local_enabled), now)
}

#[derive(Deserialize)]
pub(crate) struct DismissItem {
    id: String,
    occurrence: u32,
}

#[tauri::command(async)]
pub(crate) fn local_notification_dismiss<R: tauri::Runtime>(
    app: AppHandle<R>,
    items: Vec<DismissItem>,
) -> Result<usize, DeckError> {
    if items.len() > MAX_NOTICES {
        return Err(DeckError::new(ErrorKind::Invalid, "too many notifications"));
    }
    let items: Vec<(String, u32)> = items.into_iter().map(|i| (i.id, i.occurrence)).collect();
    let n =
        dismiss(&items, now_epoch()).map_err(|r| DeckError::new(ErrorKind::Other, r.message))?;
    if n > 0 {
        let _ = app.emit("local-notifications-changed", ());
    }
    Ok(n)
}

/// The user's route (module header). Errors are closed words the webview
/// turns into a sentence: `settings`, `no-rule`, `board`, and the claim's.
#[tauri::command(async)]
pub(crate) fn local_notification_route<R: tauri::Runtime>(
    app: AppHandle<R>,
    id: String,
    occurrence: u32,
    revision: u64,
    rule: String,
) -> Result<(), String> {
    let cfg = crate::inbound::read_config_strict().ok_or("settings")?;
    let chosen = cfg
        .rules
        .iter()
        .find(|r| r.id == rule && r.source == SOURCE && r.enabled)
        .ok_or("no-rule")?;
    let board = crate::documents::board_authority().ok_or("board")?;
    let rule_revision = crate::inbound_agent::revision_in(chosen, &board).ok_or("no-template")?;
    let event = claim_manual(
        &id,
        occurrence,
        revision,
        chosen,
        &rule_revision,
        now_epoch(),
    )
    .map_err(|e| e.code().to_string())?;
    applog("[local] notification routed by hand");
    crate::inbound::offer(&app, &cfg, vec![event], true);
    let _ = app.emit("local-notifications-changed", ());
    Ok(())
}

/* ---------- the dispatcher source ---------- */

/// The dispatcher's source for local notifications: each poll makes the
/// automatic claims and offers every pending claim (module header). It
/// reads the settings strictly: unreadable settings claim and retire
/// nothing.
#[derive(Default)]
pub(crate) struct LocalSource {
    status: SourceStatus,
}

impl crate::inbound::Source for LocalSource {
    fn id(&self) -> &'static str {
        SOURCE
    }
    fn enabled(&self, _cfg: &Config) -> bool {
        true
    }
    fn poll(&mut self, _cfg: &Config, _badges: &[String]) -> Result<Vec<Event>, &'static str> {
        let now = now_epoch();
        self.status.last_poll = Some(now);
        let out = poll_now(now);
        self.status.last_error = out.as_ref().err().copied();
        out
    }
    fn set_live(&mut self, _app: &AppHandle, _wanted: bool, _badges: &[String]) {}
    fn status(&self) -> SourceStatus {
        self.status.clone()
    }
    fn polled_events_are_live(&self) -> bool {
        true
    }
}

fn poll_now(now: u64) -> Result<Vec<Event>, &'static str> {
    let cfg = crate::inbound::read_config_strict().ok_or("settings-unreadable")?;
    let board = crate::documents::board_authority();
    let fresh = auto_route(
        &cfg,
        |rule| {
            board
                .as_ref()
                .and_then(|b| crate::inbound_agent::revision_in(rule, b))
        },
        now,
    )
    .map_err(|_| "inbox-unreadable")?;
    if !fresh.is_empty() {
        applog(&format!(
            "[local] {} notification(s) routed by rule",
            fresh.len()
        ));
    }
    let events = pending_events(&cfg, now).map_err(|_| "inbox-unreadable")?;
    // debug smoke only: the claims stay stored and unoffered, as if Deck
    // stopped between the claim and the dispatcher
    if !events.is_empty() && crate::smoke_faults::take("local-hold-offer") {
        return Ok(Vec::new());
    }
    Ok(events)
}

/// After an accepted event: route what a rule takes now and tell the view.
fn after_publish(changed: bool) {
    let Some(app) = APP.get() else { return };
    let _ = app.emit("local-notifications-changed", ());
    if !changed {
        return;
    }
    let now = now_epoch();
    if let (Ok(events), Some(cfg)) = (poll_now(now), crate::inbound::read_config_strict()) {
        if !events.is_empty() {
            crate::inbound::offer(app, &cfg, events, true);
        }
    }
}

/* ---------- the socket ---------- */

static APP: OnceLock<AppHandle> = OnceLock::new();

/// The app, once the socket thread started (`inbound_ack` repaints with it).
pub(crate) fn app_handle() -> Option<&'static AppHandle> {
    APP.get()
}
static CONNECTIONS: AtomicUsize = AtomicUsize::new(0);
/// 1 while the socket is bound.
static LISTENING: AtomicUsize = AtomicUsize::new(0);
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
/// How often the socket thread re-reads the switch without being woken.
const RECHECK: Duration = Duration::from_secs(5);

/// Re-read the switch now (every inbound settings save calls this).
pub(crate) fn wake() {
    let (flag, cv) = &WAKE;
    *flag.lock_or_recover() = true;
    cv.notify_all();
}

fn socket_path() -> PathBuf {
    crate::datadir::deck_dir().join("notify.sock")
}

#[cfg(target_os = "macos")]
fn same_uid(stream: &UnixStream) -> bool {
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: getpeereid writes two scalar outputs for this connected socket.
    unsafe {
        libc::getpeereid(std::os::fd::AsRawFd::as_raw_fd(stream), &mut uid, &mut gid) == 0
            && uid == libc::geteuid()
    }
}

#[cfg(not(target_os = "macos"))]
fn same_uid(_stream: &UnixStream) -> bool {
    false
}

/// Bind the private socket: a temporary name made 0600 and renamed into
/// place, so the path never exists with wider permissions.
fn bind_notify_socket(socket: &Path) -> Result<UnixListener, DeckError> {
    let parent = socket
        .parent()
        .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "notify socket path is invalid"))?;
    let temporary = parent.join(".notify-bind");
    let _ = std::fs::remove_file(&temporary);
    let listener = UnixListener::bind(&temporary).map_err(DeckError::from)?;
    if let Err(error) = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))
        .and_then(|_| std::fs::rename(&temporary, socket))
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(DeckError::from(error));
    }
    listener.set_nonblocking(true).map_err(DeckError::from)?;
    Ok(listener)
}

enum Line {
    Complete(Vec<u8>),
    TooLarge,
    Incomplete,
}

fn read_line(stream: &mut UnixStream) -> Line {
    let deadline = Instant::now() + REQUEST_DEADLINE;
    let mut line = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let Some(left) = deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
        else {
            return Line::Incomplete;
        };
        if stream.set_read_timeout(Some(left)).is_err() {
            return Line::Incomplete;
        }
        let count = match stream.read(&mut buffer) {
            Ok(0) => return Line::Incomplete,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Line::Incomplete,
        };
        let chunk = &buffer[..count];
        if let Some(end) = chunk.iter().position(|b| *b == b'\n') {
            line.extend_from_slice(&chunk[..end]);
            return if line.len() > MAX_REQUEST_BYTES {
                Line::TooLarge
            } else {
                Line::Complete(line)
            };
        }
        line.extend_from_slice(chunk);
        if line.len() > MAX_REQUEST_BYTES {
            return Line::TooLarge;
        }
    }
}

/// One request (the wire is `deck-notify`'s; module header): the response,
/// and whether an accepted publish left something a rule may route.
pub(crate) fn answer(line: &[u8], enabled: bool, now: u64) -> (Value, bool) {
    let request: Value = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(_) => return (invalid("the request is not JSON").to_value(), false),
    };
    if request.get("protocol").and_then(Value::as_u64) != Some(WIRE_PROTOCOL) {
        return (
            Refusal::new(
                "UNSUPPORTED_PROTOCOL",
                "deck-notify and Deck speak different protocols; use the deck-notify bundled with this Deck",
            )
            .to_value(),
            false,
        );
    }
    if !enabled {
        return (
            Refusal::new(
                "DISABLED",
                "local notifications are turned off in Deck (Notifications > Accept local notifications)",
            )
            .to_value(),
            false,
        );
    }
    match request.get("op").and_then(Value::as_str) {
        Some("publish") => {
            let Some(notification) = request.get("notification") else {
                return (invalid("publish needs a notification").to_value(), false);
            };
            match publish(notification, now) {
                Ok((receipt, changed)) => (
                    serde_json::to_value(receipt).unwrap_or(Value::Null),
                    changed,
                ),
                Err(r) => (r.to_value(), false),
            }
        }
        Some("status") => {
            let producer = request
                .get("producer")
                .and_then(Value::as_str)
                .unwrap_or("");
            let event_id = request
                .get("event_id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !valid_name(producer) || !valid_ident(event_id) {
                return (
                    invalid("status needs a producer and an event_id").to_value(),
                    false,
                );
            }
            match status(producer, event_id) {
                Ok(Some(receipt)) => (serde_json::to_value(receipt).unwrap_or(Value::Null), false),
                Ok(None) => (
                    serde_json::json!({"protocol": WIRE_PROTOCOL, "status": "not-found",
                        "producer": producer, "event_id": event_id}),
                    false,
                ),
                Err(r) => (r.to_value(), false),
            }
        }
        _ => (invalid("op is publish or status").to_value(), false),
    }
}

fn handle(mut stream: UnixStream, enabled: bool) {
    if !same_uid(&stream) {
        return;
    }
    if stream.set_nonblocking(false).is_err()
        || stream.set_write_timeout(Some(REQUEST_DEADLINE)).is_err()
    {
        return;
    }
    let (response, routable) = match read_line(&mut stream) {
        Line::Complete(line) => answer(&line, enabled, now_epoch()),
        Line::TooLarge => (
            Refusal::new(
                "TOO_LARGE",
                format!("a request is at most {MAX_REQUEST_BYTES} bytes"),
            )
            .to_value(),
            false,
        ),
        Line::Incomplete => (
            invalid("the request was not one complete line").to_value(),
            false,
        ),
    };
    // debug smoke only: the receipt of a stored publish is lost on the way
    let lost = response["status"] == "accepted" && crate::smoke_faults::take("local-no-receipt");
    if let (false, Ok(bytes)) = (lost, serde_json::to_vec(&response)) {
        let _ = stream.write_all(&bytes);
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
    drop(stream);
    if response["status"] == "accepted" {
        applog("[local] notification accepted");
        after_publish(routable);
    }
}

fn wait_for(listener: Option<&UnixListener>, d: Duration) {
    if let Some(listener) = listener {
        let mut fd = libc::pollfd {
            fd: std::os::fd::AsRawFd::as_raw_fd(listener),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: one valid pollfd for a listener this thread owns.
        unsafe { libc::poll(&mut fd, 1, 1000) };
        return;
    }
    let (flag, cv) = &WAKE;
    let mut woken = flag.lock_or_recover();
    if !*woken {
        woken = crate::sync::wait_timeout_or_recover(cv, woken, d);
    }
    *woken = false;
}

/// The socket's lifecycle and accept loop (module header).
pub(crate) fn spawn(app: AppHandle) {
    let _ = APP.set(app);
    std::thread::Builder::new()
        .name("deck-local-notify".into())
        .spawn(|| {
            let socket = socket_path();
            let mut listener: Option<UnixListener> = None;
            let mut checked: Option<Instant> = None;
            let mut enabled = false;
            let mut bind_failed = false;
            loop {
                let woken = std::mem::replace(&mut *WAKE.0.lock_or_recover(), false);
                if woken || checked.is_none_or(|t| t.elapsed() >= RECHECK) {
                    enabled = crate::inbound::read_config().local_enabled;
                    checked = Some(Instant::now());
                    if !enabled && listener.is_some() {
                        listener = None;
                        let _ = std::fs::remove_file(&socket);
                        LISTENING.store(0, Ordering::SeqCst);
                        applog("[local] notification socket closed");
                    } else if enabled && listener.is_none() {
                        match bind_notify_socket(&socket) {
                            Ok(bound) => {
                                bind_failed = false;
                                listener = Some(bound);
                                LISTENING.store(1, Ordering::SeqCst);
                                applog("[local] notification socket open");
                            }
                            // said once per run of failures, not every recheck
                            Err(e) if !bind_failed => {
                                bind_failed = true;
                                applog(&format!(
                                    "[local] notification socket bind FAILED ({})",
                                    e.code()
                                ));
                            }
                            Err(_) => {}
                        }
                    }
                }
                let Some(bound) = listener.as_ref() else {
                    wait_for(None, RECHECK);
                    continue;
                };
                match bound.accept() {
                    Ok((stream, _)) => {
                        if CONNECTIONS.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                            CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                            let mut stream = stream;
                            let _ = stream.set_nonblocking(false);
                            let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                            if let Ok(bytes) = serde_json::to_vec(
                                &Refusal::new("BUSY", "too many connections; retry").to_value(),
                            ) {
                                let _ = stream.write_all(&bytes);
                                let _ = stream.write_all(b"\n");
                            }
                            continue;
                        }
                        std::thread::spawn(move || {
                            handle(stream, enabled);
                            CONNECTIONS.fetch_sub(1, Ordering::SeqCst);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        wait_for(Some(bound), RECHECK);
                    }
                    Err(_) => {
                        listener = None;
                        LISTENING.store(0, Ordering::SeqCst);
                    }
                }
            }
        })
        .ok();
}

#[cfg(test)]
pub(crate) fn with_test_inbox<T>(f: impl FnOnce(&Path) -> T) -> T {
    let _serial = STORE_TESTS.lock_or_recover();
    let dir = crate::test_dir::TestDir::new("local-notify");
    let path = dir.join("notifications.json");
    *TEST_PATH.lock_or_recover() = Some(path.clone());
    *STORE.lock_or_recover() = None;
    RATE.lock_or_recover().clear();
    let out = f(&path);
    *TEST_PATH.lock_or_recover() = None;
    *STORE.lock_or_recover() = None;
    out
}

#[cfg(test)]
#[path = "local_notify/tests.rs"]
mod tests;
