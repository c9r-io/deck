//! Automation delivery authority: the user's explicit, revision-bound
//! approval for a Slack badge automation to send its approved steps without
//! a per-row send-now.
//!
//! # Contract
//! Deck may coordinate an Agent session; it does not own the Agent's
//! execution. This module decides ONE thing — whether an external-origin
//! follow-up row carries content the user pre-approved — and nothing about
//! when the agent is ready for it. Three facts stay separate:
//!
//! - **provenance** — `QueueItem.external`, set only by `admit_external`,
//!   is never cleared or rewritten by an approval;
//! - **content authority** — `QueueItem.authority` (`StepAuthority`), set
//!   only here, on the external admission path, after the claim was checked
//!   against the CURRENT settings grant (`verify_claim`);
//! - **input readiness** — the scheduler's holds (`select.rs`): needs-input,
//!   the first-interaction gate, Codex Signal trust, review checkpoints,
//!   pause, ambiguity, group order, send gap, target identity and the
//!   bracketed-paste check. An approval releases only the external
//!   follow-up hold and never any of these.
//!
//! The grant (`inbound::AutoSend`, stored on the rule in settings.json) is
//! content-addressed: `grant_digest` hashes the canonical manifest of every
//! meaning-bearing rule field (id, trigger, badge, project, directory, full
//! command, template name, finish, reviewEach) together with each template
//! step's SHA-256, its content class and the external-content
//! acknowledgment. A grant whose stored digest no longer matches its rule is
//! void, so an edit that changes what is sent or where requires a fresh
//! approval; the display name, the board column and `enabled` are outside
//! the manifest. The webview computes the same digest (automation-model.js
//! `grantDigest`; one fixture pins both).
//!
//! - `verify_claim` admits a row's claim `{rule, grant, step}` only when the
//!   rule is a Slack badge rule, its grant is valid and is the claimed one,
//!   the row's command is the rule's command, the text is not a verbatim
//!   external message, and the step's class allows it:
//!   - `fixed` needs the row text's SHA-256 to be the approved step's;
//!   - `bounded` (owner text with `{{msg.*}}` placeholders) needs the rule's
//!     explicit `external` acknowledgment AND a native proof: the claim's
//!     skeleton must hash to the approved step, carry a known placeholder,
//!     and — expanded by `expand_bounded` (the byte-for-byte twin of the
//!     webview's `fillInboundTemplate`, pinned by shared vectors) over the
//!     backend's OWN copy of the Slack event the claim names
//!     (`inbound::pending_event`: source, key, badge and rule must match) —
//!     equal the row text exactly. So an authority-bearing bounded row holds
//!     no byte outside the deterministic expansion of the approved step over
//!     the exact admitted event. Deck does not claim the message is safe,
//!     only that the user accepted it through that bounded placeholder.
//! - A refused claim admits the row WITHOUT authority: it still runs, one
//!   send-now at a time. No proof, no approval (a replay after a restart
//!   before the event is announced again admits its bounded rows manual).
//! - The revocation fence: an automatic send re-reads the authority source
//!   and decides (`fence`) inside the transaction that persists its firing
//!   intent, holding `storage::settings_fence`, the lock every settings
//!   write takes. The irreversible boundary is that persisted firing intent:
//!   once a revoking settings write has returned, no automatic send can
//!   still begin under the revoked grant (the fence strips the row instead,
//!   `Revoked`); a send already past it completes and a crash there stays
//!   ambiguous. Send-now does not consult the fence — the user is acting.
//! - An unreadable authority source is no proof either way: rows and their
//!   stored authority are kept (nothing is revoked or written), and every
//!   automatic send that relies on an approval holds — at selection
//!   (`Hold::AuthorityUnverified`, stage `authority-unverified`) and at the
//!   fence (`Unverified`) — until settings can be read; send-now still works.
//! - `revoke_stale` (the scheduler tick) strips authority from every unsent
//!   row whose grant is no longer the rule's valid grant — revoked, edited,
//!   the rule deleted — and bumps its revision, so the panel and disk agree
//!   with the fence. Firing/ambiguous rows keep their crash semantics.
//!   Editing a row's text strips its authority too (`ops::update_text`).
//! - Content snapshot vs authority lifetime: a run's prompt bytes are frozen
//!   when it is materialized and NO later rule or template edit rewrites
//!   them. Its authority is not frozen: it lives exactly as long as the
//!   grant version it was admitted under. Unticking the approval, deleting
//!   the rule, or editing anything the grant covers (which retires that
//!   version — a re-approval is a new grant) removes automatic delivery from
//!   the run's unsent rows; they keep their exact text for send-now.
//! - Durable vs transient: the grant and a row's authority survive restarts;
//!   agent interaction evidence does not (`agent_status::Evidence`), so after
//!   a restart or a new agent generation the first-interaction gate holds
//!   again until the agent proves interaction.
//! - No agent hook word, quiet time or output activity reads or writes
//!   anything here (`tests/signal_census.rs`): Signal can hold an approved
//!   row, never create or restore its approval.
//! - Audit: a delivery record copies the row's authority (ids, step, class,
//!   trigger — no text) and whether the user sent it by hand; log lines
//!   carry closed codes only.
//! - Compatibility, no schema door: every new field can only WITHDRAW
//!   automatic delivery on an older reader. An older Deck ignores a row's
//!   `authority` and a delivery's audit fields (it keeps holding the
//!   external row, exactly as Stable 0.7.16) and ignores or drops a rule's
//!   `autoSend`. Legacy rules and rows carry nothing and keep the Stable
//!   behaviour. The closed words `ContentClass`/`TriggerClass` have no
//!   catch-all: adding one is a queue.json schema change (see `ItemState`).
//! - Scope: only Slack badge rules. Slack channel monitors (no per-message
//!   human action), Connector rows (a paired phone is not prompt authority)
//!   and verbatim scratchpad copies stay send-now only; clock rows are owner
//!   text and never needed an approval.

use serde::{Deserialize, Serialize};

use super::*;
use crate::inbound::{AutoSend, Config, Rule};

/// What an approved step's prompt is made of.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ContentClass {
    /// the rule owner's template text only
    Fixed,
    /// owner text with bounded `{{msg.*}}` placeholders (flattened to one
    /// line each by the webview's `fillInboundTemplate`)
    Bounded,
}

impl ContentClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ContentClass::Fixed => "fixed",
            ContentClass::Bounded => "bounded",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        match word {
            "fixed" => Some(ContentClass::Fixed),
            "bounded" => Some(ContentClass::Bounded),
            _ => None,
        }
    }
}

/// Which trigger may carry an approval. Closed: Slack channel monitors and
/// Connector rows stay manual in this version, and clock rows are owner
/// text that never needed one.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TriggerClass {
    SlackBadge,
}

/// The webview's statement of which approved step a row is (from the frozen
/// inbound plan). A request only: `verify_claim` decides. `event` names the
/// inbound event the run was made from (its origin key) and `skeletons[k]`
/// is text k's approved template step before expansion — PROOF material for
/// a bounded step, never trusted as such: the backend checks the skeleton
/// against the grant's step hash and re-expands it over its own copy of the
/// event. Both are omitted when absent so older fingerprints stay identical.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorityClaim {
    pub(crate) rule: String,
    pub(crate) grant: String,
    pub(crate) step: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) event: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) skeletons: Vec<Option<String>>,
}

/// What `verify_claim` checks a bounded step against: the approved
/// skeleton the claim carried and the backend-owned inbound event it names
/// (`inbound::pending_event`: still pending, since the webview acks only
/// after every row is queued).
#[derive(Clone, Copy, Default)]
pub(crate) struct Proof<'a> {
    pub(crate) skeleton: Option<&'a str>,
    pub(crate) event: Option<&'a crate::inbound::Event>,
}

/// The durable fact on a row: this exact step of this exact grant was
/// approved for automatic delivery. Ids and closed words only.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct StepAuthority {
    pub(crate) rule: String,
    pub(crate) grant: String,
    pub(crate) step: u32,
    pub(crate) class: ContentClass,
    pub(crate) trigger: TriggerClass,
}

fn finish_word(rule: &Rule) -> &'static str {
    if rule.finish == "close" {
        "close"
    } else {
        "keep"
    }
}

/// SHA-256 of the canonical authority manifest (module header). The
/// webview's `grantDigest` builds the identical compact JSON array.
pub(crate) fn grant_digest(rule: &Rule, grant: &AutoSend) -> String {
    let manifest = serde_json::json!([
        "deck-automation-grant",
        1,
        rule.id,
        rule.source,
        rule.badge,
        rule.project_id,
        rule.dir,
        rule.cmd,
        rule.template,
        finish_word(rule),
        rule.review_each,
        grant.steps,
        grant.classes,
        grant.external,
    ]);
    crate::ledger::sha(manifest.to_string().as_bytes())
}

/// The rule's grant when it is a Slack badge rule's and still matches the
/// rule (an approval for another version of the rule is void).
pub(crate) fn valid_grant(rule: &Rule) -> Option<&AutoSend> {
    let grant = rule.auto_send.as_ref()?;
    (rule.source == "slack" && grant.digest == grant_digest(rule, grant)).then_some(grant)
}

/// Why a claim was not admitted: closed codes for the log.
pub(crate) type Refusal = &'static str;

/// Check a row's claim against the current settings (`None` = unreadable,
/// which grants nothing). `text` is the row's normalized prompt. A bounded
/// step is admitted only when `text` is byte-for-byte the deterministic
/// expansion (`expand_bounded`) of the approved skeleton over the exact
/// admitted event: an authority-bearing bounded row can hold no byte outside
/// that expansion.
pub(crate) fn verify_claim(
    config: Option<&Config>,
    claim: &AuthorityClaim,
    cmd: &str,
    text: &str,
    external_text: bool,
    proof: Proof<'_>,
) -> Result<StepAuthority, Refusal> {
    if external_text {
        return Err("verbatim");
    }
    let config = config.ok_or("settings-unreadable")?;
    let rule = config
        .rules
        .iter()
        .find(|r| r.id == claim.rule)
        .ok_or("no-rule")?;
    let grant = valid_grant(rule).ok_or("no-grant")?;
    if grant.digest != claim.grant {
        return Err("stale");
    }
    if rule.cmd != cmd {
        return Err("command");
    }
    let step = claim.step as usize;
    let class = grant
        .classes
        .get(step)
        .and_then(|c| ContentClass::parse(c))
        .ok_or("step")?;
    match class {
        ContentClass::Fixed
            if grant.steps.get(step) != Some(&crate::ledger::sha(text.as_bytes())) =>
        {
            return Err("content")
        }
        ContentClass::Fixed => {}
        ContentClass::Bounded => {
            if !grant.external {
                return Err("external-not-acknowledged");
            }
            let skeleton = proof.skeleton.ok_or("no-skeleton")?;
            if grant.steps.get(step) != Some(&crate::ledger::sha(skeleton.as_bytes())) {
                return Err("skeleton");
            }
            if !has_placeholder(skeleton) {
                return Err("class");
            }
            let event = proof.event.ok_or("no-event")?;
            if event.source != "slack"
                || event.badge != rule.badge
                || claim.event.as_deref() != Some(event.key.as_str())
            {
                return Err("event");
            }
            if super::normalize_prompt(&expand_bounded(skeleton, event)) != text {
                return Err("expansion");
            }
        }
    }
    Ok(StepAuthority {
        rule: rule.id.clone(),
        grant: grant.digest.clone(),
        step: claim.step,
        class,
        trigger: TriggerClass::SlackBadge,
    })
}

/* ---------- the deterministic expansion (twin of the webview's) ---------- */

/// JavaScript's `\s` (and `String.prototype.trim`) set.
fn js_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(js_space)
}

/// `flatten` in pure.js `fillInboundTemplate`: every whitespace run holding a
/// CR, LF or tab becomes one space, runs of spaces become one, then trim.
fn flatten(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut run = String::new();
    let close = |run: &mut String, out: &mut String| {
        if run.contains(['\r', '\n', '\t']) {
            out.push(' ');
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in value.chars() {
        if js_space(c) {
            run.push(c);
        } else {
            close(&mut run, &mut out);
            out.push(c);
        }
    }
    close(&mut run, &mut out);
    let mut collapsed = String::with_capacity(out.len());
    for c in out.chars() {
        if !(c == ' ' && collapsed.ends_with(' ')) {
            collapsed.push(c);
        }
    }
    js_trim(&collapsed).to_string()
}

/// `normalizeTemplateStep` in pure.js.
pub(crate) fn normalize_template_step(text: &str) -> String {
    let body = text
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', " ")
        .split('\n')
        .map(|line| line.trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n");
    js_trim(&body).chars().take(TEMPLATE_STEP_MAX).collect()
}

/// pure.js `TEMPLATE_STEP_MAX` (characters).
pub(crate) const TEMPLATE_STEP_MAX: usize = 2000;
/// pure.js `INBOUND_PLACEHOLDERS`.
pub(crate) const INBOUND_PLACEHOLDERS: &[&str] = &["text", "from", "where", "link"];

/// The placeholder at the start of `s` (`{{ msg.<name> }}`): its name and
/// byte length.
fn placeholder_at(s: &str) -> Option<(&str, usize)> {
    let rest = s.strip_prefix("{{")?;
    let body = rest.trim_start_matches(js_space);
    let body = body.strip_prefix("msg.")?;
    let name_len = body.bytes().take_while(u8::is_ascii_lowercase).count();
    if name_len == 0 {
        return None;
    }
    let (name, after) = body.split_at(name_len);
    let after = after.trim_start_matches(js_space).strip_prefix("}}")?;
    Some((name, s.len() - after.len()))
}

fn has_placeholder(skeleton: &str) -> bool {
    skeleton.char_indices().any(|(i, _)| {
        placeholder_at(&skeleton[i..]).is_some_and(|(n, _)| INBOUND_PLACEHOLDERS.contains(&n))
    })
}

/// `fillInboundTemplate(skeleton, msg)` over the backend's own event: each
/// known placeholder becomes its flattened value, unknown ones stay literal,
/// then the step is normalized like any template step.
pub(crate) fn expand_bounded(skeleton: &str, event: &crate::inbound::Event) -> String {
    let mut out = String::with_capacity(skeleton.len() + event.text.len());
    let mut i = 0;
    while i < skeleton.len() {
        if let Some((name, len)) = placeholder_at(&skeleton[i..]) {
            let value = match name {
                "text" => Some(&event.text),
                "from" => Some(&event.from),
                "where" => Some(&event.where_),
                "link" => Some(&event.link),
                _ => None,
            };
            match value {
                Some(v) => out.push_str(&flatten(v)),
                None => out.push_str(&skeleton[i..i + len]),
            }
            i += len;
        } else {
            let c = skeleton[i..].chars().next().expect("in bounds");
            out.push(c);
            i += c.len_utf8();
        }
    }
    normalize_template_step(&out)
}

/* ---------- the pre-fire fence ---------- */

/// Whether an automatic send of `i` depends on its approval: an external
/// follow-up (the only row the approval releases).
pub(crate) fn relies_on_authority(i: &QueueItem) -> bool {
    i.external && i.mode == "chain" && i.authority.is_some()
}

/// The authority check immediately before the irreversible boundary.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fence {
    /// no approval needed, or the approval is still the rule's valid grant
    Clear,
    /// the grant is gone or changed: strip it, send nothing
    Revoked,
    /// settings could not be read: keep the row and its authority, send
    /// nothing automatically
    Unverified,
}

/// Decide the fence for an automatic send of `i` under `config` (the
/// settings read made while holding `storage::settings_fence`).
pub(crate) fn fence(i: &QueueItem, config: Option<&Config>) -> Fence {
    let Some(authority) = i.authority.as_ref().filter(|_| relies_on_authority(i)) else {
        return Fence::Clear;
    };
    match config {
        None => Fence::Unverified,
        Some(config) if still_granted(config, authority) => Fence::Clear,
        Some(_) => Fence::Revoked,
    }
}

/// Whether `authority` is still backed by the rule's current valid grant.
fn still_granted(config: &Config, authority: &StepAuthority) -> bool {
    config
        .rules
        .iter()
        .find(|r| r.id == authority.rule)
        .and_then(valid_grant)
        .is_some_and(|grant| {
            grant.digest == authority.grant
                && grant
                    .classes
                    .get(authority.step as usize)
                    .map(String::as_str)
                    == Some(authority.class.as_str())
                && (authority.class == ContentClass::Fixed || grant.external)
        })
}

/// Rows the revocation sweep may touch: unsent and not mid-send/ambiguous.
fn revocable(i: &QueueItem) -> bool {
    i.authority.is_some() && matches!(i.state, ItemState::Pending | ItemState::Failed)
}

/// Whether any row carries a revocable authority (the tick reads settings
/// only then).
pub(crate) fn any_authority(q: &QueueState) -> bool {
    q.items.iter().any(revocable)
}

/// Strip authority from every unsent row whose grant is gone or changed;
/// the number of rows that lost it.
pub(crate) fn revoke_stale(q: &mut QueueState, config: &Config) -> usize {
    let mut n = 0;
    for item in q.items.iter_mut().filter(|i| revocable(i)) {
        if !item
            .authority
            .as_ref()
            .is_some_and(|a| still_granted(config, a))
        {
            item.authority = None;
            item.revision = item.revision.wrapping_add(1);
            n += 1;
        }
    }
    n
}
