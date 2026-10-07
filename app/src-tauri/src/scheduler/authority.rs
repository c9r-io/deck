//! Automation delivery authority: the user's explicit, revision-bound
//! approval for a Slack badge automation, or for a phone task preset, to
//! send its approved steps without a per-row send-now.
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
//!   The source is read without moving the settings file
//!   (`inbound::read_config_strict`, `storage::read_typed`), so this state
//!   lasts as long as its cause and never turns into "no settings, nothing
//!   granted".
//! - The backup is recovery material, never authority. settings.json.bak is
//!   the save BEFORE the last one, so it can hold exactly the grant the last
//!   save revoked. A damaged or set-aside main file whose backup still loads
//!   is therefore the same "unreadable source" as above — rows hold with what
//!   they carry, nothing is granted, revoked or sent automatically — until
//!   the owner saves settings again (the user committing what the webview
//!   recovered, grants included, as the current version). The fence orders a
//!   send against settings WRITES; it is this rule, not the fence, that keeps
//!   a later damaged file from undoing a revocation that had returned.
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
//! - Scope: Slack badge rules and phone task presets. Slack channel
//!   monitors (no per-message human action), every other Connector row (a
//!   message or a scratchpad copy sent from the phone: text the phone
//!   supplies, which no approval on this Mac ever saw) and verbatim
//!   scratchpad copies stay send-now only; clock rows are owner text and
//!   never needed an approval.
//! - A phone task preset (`TriggerClass::Connector`). Being paired is never
//!   prompt authority: the phone can only name a preset, and what is
//!   approved is the preset's own content, on this Mac. The grant is the
//!   preset's `autoSend.digest` in the Board (deck.json): `preset_digest`
//!   hashes the project id, the preset id, its directory, its full command
//!   and each step's SHA-256, so the steps need no second copy and an edit
//!   to anything sent, or to where, voids it; the name, the card title and
//!   the column are outside. Every step is `fixed`. The webview computes the
//!   same digest (connector-model.js `presetGrantDigest`; one fixture pins
//!   both). Everything above holds with the Board in place of settings:
//!   - `verify_preset_claim` admits step k of a run only against the
//!     CURRENT Board (`documents::board_authority`, never one answered from
//!     its backup): the preset's grant is valid and is the claimed one, its
//!     command is the row's, the row's text is exactly the preset's step k,
//!     and exactly one card was made from the claimed command handle whose
//!     still unqueued frozen run froze that grant and holds this row as step
//!     k. The Connector's own journal must say the handle is an applied
//!     `task-create` naming that card from a device that is still paired.
//!   - The sweep and the fence read the Board-side source
//!     (`first_send::PhoneTasks`): the current Board plus the devices that
//!     are still paired. Unticking, editing a covered field, deleting the
//!     preset or its project, or revoking the device whose command made the
//!     run strips the approval from that run's unsent rows. The fence is
//!     `documents::board_fence`, which every Board commit takes. A device
//!     revocation is not a Board write and takes no fence: it is seen by the
//!     next sweep and by every pre-fire read that follows it.
//!   - A Board recovered from its backup or from a kept copy never brings an
//!     approval back: the door hands it over without its presets' choices
//!     (`documents.rs`), so nothing is admitted on it, the sweep strips the
//!     approval from the steps still waiting, and the saves that follow
//!     write a Board that approves nothing until the user ticks the preset
//!     again. No committed Board at all, or an unreadable Connector state,
//!     is the same "unreadable source": nothing granted, revoked or sent
//!     automatically.
//!   - A preset's grant is its content digest and nothing else, so ticking
//!     an unchanged preset again is the same grant. Steps the sweep already
//!     stripped stay manual (a row never regains authority); a frozen run
//!     that was not queued yet claims the grant when it is queued.
//!   - deck.json and queue.json take sticky schema v6 when a preset carries
//!     a grant or a row or audit record this trigger: the closed word has no
//!     catch-all, and an older webview would drop the preset's grant on its
//!     next Board save.
//! - Not readiness, and not the first-send readiness override: a badge
//!   rule's separate risk acceptance for its runs' head row lives in
//!   `first_send.rs` and never writes or reads `StepAuthority`.

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
    /// a phone task preset's approval (`rule` is the preset id)
    Connector,
}

/// The preset a phone task's approval came from: its project, and the paired
/// device whose command made the run.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct PresetSource {
    pub(crate) project: String,
    pub(crate) device: String,
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
    /// Present for a phone task alone: `rule` is then a task preset of this
    /// project and `event` the Connector command handle. Omitted otherwise
    /// so older fingerprints stay identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preset_project: Option<String>,
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
    /// Present exactly for `TriggerClass::Connector`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) connector: Option<PresetSource>,
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
        connector: None,
    })
}

/* ---------- a phone task preset's approval (the Board) ---------- */

/// SHA-256 of a preset's canonical authority manifest (module header), or
/// `None` when the preset lacks a field the manifest needs. The webview's
/// `presetGrantDigest` builds the identical compact JSON array.
pub(crate) fn preset_digest(project: &str, preset: &serde_json::Value) -> Option<String> {
    let steps: Vec<String> = preset["steps"]
        .as_array()?
        .iter()
        .map(|step| {
            step.as_str()
                .map(|text| crate::ledger::sha(text.as_bytes()))
        })
        .collect::<Option<_>>()?;
    let manifest = serde_json::json!([
        "deck-preset-grant",
        1,
        project,
        preset["id"].as_str()?,
        preset["dir"].as_str()?,
        preset["cmd"].as_str()?,
        steps,
    ]);
    Some(crate::ledger::sha(manifest.to_string().as_bytes()))
}

/// The preset's grant digest when it still matches the preset (an approval
/// for another version of the preset is void).
fn valid_preset_grant<'a>(project: &str, preset: &'a serde_json::Value) -> Option<&'a str> {
    let stored = preset["autoSend"]["digest"].as_str()?;
    (preset_digest(project, preset).as_deref() == Some(stored)).then_some(stored)
}

/// Check a phone task row's claim against the current Board (`None` = none,
/// which grants nothing) and the Connector journal's own record of the
/// claimed command handle. `text` is the row's normalized prompt.
pub(crate) fn verify_preset_claim(
    board: Option<&serde_json::Value>,
    claim: &AuthorityClaim,
    args: &super::ops::QueueAddArgs,
    text: &str,
    proof: Option<&crate::connector::TaskProof>,
) -> Result<StepAuthority, Refusal> {
    if args.external_text {
        return Err("verbatim");
    }
    let project = claim.preset_project.as_deref().ok_or("no-preset")?;
    let handle = claim.event.as_deref().ok_or("no-event")?;
    let board = board.ok_or("board-unreadable")?;
    let preset = first_send::preset(board, project, &claim.rule).ok_or("no-preset")?;
    let grant = valid_preset_grant(project, preset).ok_or("no-grant")?;
    if grant != claim.grant {
        return Err("stale");
    }
    if preset["cmd"] != args.cmd.as_str() {
        return Err("command");
    }
    let step = claim.step as usize;
    let approved = preset["steps"][step].as_str().ok_or("step")?;
    if super::normalize_prompt(approved) != text {
        return Err("content");
    }
    let proof = proof.ok_or("no-event")?;
    if proof.card_id != args.card_id {
        return Err("event");
    }
    let dir = preset["dir"].as_str().unwrap_or_default();
    if first_send::connector_step(board, args, project, &claim.rule, handle, dir, step)
        .is_none_or(|run| run["autoSend"] != grant)
    {
        return Err("run");
    }
    Ok(StepAuthority {
        rule: claim.rule.clone(),
        grant: grant.to_owned(),
        step: claim.step,
        class: ContentClass::Fixed,
        trigger: TriggerClass::Connector,
        connector: Some(PresetSource {
            project: project.to_owned(),
            device: proof.device_id.clone(),
        }),
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

pub(crate) fn has_placeholder(skeleton: &str) -> bool {
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

/// Which document backs an approval (`first_send::Backing`).
pub(crate) fn backing(authority: &StepAuthority) -> first_send::Backing {
    match authority.trigger {
        TriggerClass::SlackBadge => first_send::Backing::Settings,
        TriggerClass::Connector => first_send::Backing::Board,
    }
}

/// The source that backs this row's approval, when an automatic send of it
/// relies on one.
pub(crate) fn relied_backing(i: &QueueItem) -> Option<first_send::Backing> {
    i.authority
        .as_ref()
        .filter(|_| relies_on_authority(i))
        .map(backing)
}

/// Decide the fence for an automatic send of `i` under `sources` (read
/// while holding the fence of the source that backs the row:
/// `storage::settings_fence` or `documents::board_fence`).
pub(crate) fn fence(i: &QueueItem, sources: first_send::Sources<'_>) -> Fence {
    let Some(authority) = i.authority.as_ref().filter(|_| relies_on_authority(i)) else {
        return Fence::Clear;
    };
    match standing(sources, authority) {
        None => Fence::Unverified,
        Some(true) => Fence::Clear,
        Some(false) => Fence::Revoked,
    }
}

/// Whether `authority` still stands: `None` when the source that backs it
/// could not be read, which proves nothing either way.
fn standing(sources: first_send::Sources<'_>, authority: &StepAuthority) -> Option<bool> {
    match backing(authority) {
        first_send::Backing::Settings => sources.settings.map(|c| still_granted(c, authority)),
        first_send::Backing::Board => sources.board.map(|t| preset_still_grants(t, authority)),
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

/// Whether `authority` is still backed by its preset's current valid grant
/// and by a device that is still paired.
fn preset_still_grants(tasks: &first_send::PhoneTasks, authority: &StepAuthority) -> bool {
    authority.connector.as_ref().is_some_and(|source| {
        tasks.paired.contains(&source.device)
            && authority.class == ContentClass::Fixed
            && first_send::preset(&tasks.board, &source.project, &authority.rule).is_some_and(
                |preset| {
                    valid_preset_grant(&source.project, preset) == Some(authority.grant.as_str())
                        && preset["steps"][authority.step as usize].is_string()
                },
            )
    })
}

/// Rows the revocation sweep may touch: unsent and not mid-send/ambiguous.
fn revocable(i: &QueueItem) -> bool {
    i.authority.is_some() && matches!(i.state, ItemState::Pending | ItemState::Failed)
}

/// Whether any row carries a revocable authority backed by `source` (the
/// tick reads that source only then).
pub(crate) fn any_authority(q: &QueueState, source: first_send::Backing) -> bool {
    q.items
        .iter()
        .any(|i| revocable(i) && i.authority.as_ref().map(backing) == Some(source))
}

/// Strip authority from every unsent row whose grant is gone or changed;
/// the number of rows that lost it. A row whose source is unreadable is
/// left alone.
pub(crate) fn revoke_stale<'a>(
    q: &mut QueueState,
    sources: impl Into<first_send::Sources<'a>>,
) -> usize {
    let sources = sources.into();
    let mut n = 0;
    for item in q.items.iter_mut().filter(|i| revocable(i)) {
        if item
            .authority
            .as_ref()
            .is_some_and(|a| standing(sources, a) == Some(false))
        {
            item.authority = None;
            item.revision = item.revision.wrapping_add(1);
            n += 1;
        }
    }
    n
}
