//! Slack channel first-step authorization.
//!
//! This is content authority for one frozen channel run head. It is separate
//! from `StepAuthority` and from the first-interaction readiness override.
//! Every head from the native staged-grant path carries a permanent
//! constraint: loss of proof can take automatic permission
//! away but can never turn the row into an ordinary owner row. Admission binds
//! current settings, the native inbox proof, the current Board's unique card
//! and frozen run, and the exact normalized queue request. Automatic delivery
//! rechecks current settings and Board under the normal settings -> Board ->
//! queue persist fence. Manual send-now remains the user's explicit action.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{ops::QueueAddArgs, QueueItem};
use crate::error::{DeckError, ErrorKind};

pub(crate) const MAX_FIRST_ADMISSION_AGE_SECS: u64 = 900;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ChannelFirstSendClaim {
    pub(crate) inbox_id: String,
    pub(crate) grant_id: String,
    pub(crate) grant_digest: String,
    pub(crate) skeleton: String,
}

/// Content-free facts fixed at firing intent, including uncertain outcomes.
/// This records the attempt's policy dependency, not readiness or success.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ChannelAttemptAudit {
    pub(crate) automatic: bool,
    pub(crate) readiness_overridden: bool,
}

/// Durable row fact. `authorized` is revocable; the constraint itself is not.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ChannelFirstSendConstraint {
    pub(crate) rule: String,
    pub(crate) inbox: String,
    pub(crate) grant_id: String,
    pub(crate) grant_digest: String,
    pub(crate) operation: String,
    pub(crate) workspace: String,
    pub(crate) authorized: bool,
}

pub(crate) fn operation_id(inbox_id: &str) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(format!("channel:{inbox_id}/step/0").as_bytes())
    );
    format!("B{}", &digest[..32])
}

pub(crate) fn rollback_uncertain(
    args: &QueueAddArgs,
    recovery_before: Option<u64>,
) -> Result<bool, DeckError> {
    if let (Some(boundary), Some(claim)) = (recovery_before, args.channel_first_send.as_ref()) {
        let event = crate::inbound_channel::channel_proof(&claim.inbox_id)?;
        if event.is_none_or(|event| event.staged_at <= boundary) {
            return Ok(true);
        }
    }
    let Some(board) = crate::documents::board_authority() else {
        return Ok(false);
    };
    Ok(board["cards"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|card| card["id"] == args.card_id && card["session"] == args.session)
        .is_some_and(|card| card["channelRun"]["firstSendUncertain"] == true))
}

pub(crate) fn reject_reviewed_list(args: &QueueAddArgs) -> Result<(), DeckError> {
    if args.channel_first_send.is_some()
        || crate::inbound_channel::channel_constraint_for_card(&args.card_id)?
            .is_some_and(|(_, granted)| granted)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "channel first-send does not support reviewed lists",
        ));
    }
    Ok(())
}

fn flatten(value: &str) -> String {
    let lines = regex::Regex::new(r"\s*[\r\n\t]+\s*").expect("fixed whitespace regex");
    let spaces = regex::Regex::new(r" {2,}").expect("fixed ASCII space regex");
    spaces
        .replace_all(&lines.replace_all(value, " "), " ")
        .trim()
        .to_owned()
}

fn expand(skeleton: &str, event: &crate::inbound_channel::PendingChannelEvent) -> String {
    let placeholders =
        regex::Regex::new(r"\{\{\s*msg\.([a-z]+)\s*\}\}").expect("fixed placeholder regex");
    let out = placeholders.replace_all(skeleton, |caps: &regex::Captures<'_>| {
        let value = match &caps[1] {
            "text" => Some(event.body.as_str()),
            "from" => Some(
                event
                    .sender_user_id
                    .as_deref()
                    .or(event.sender_bot_id.as_deref())
                    .unwrap_or(""),
            ),
            "where" => Some(event.channel_id.as_str()),
            "link" => Some(""),
            _ => None,
        };
        value.map(flatten).unwrap_or_else(|| caps[0].to_owned())
    });
    super::normalize_prompt(&out)
}

fn matching_card<'a>(
    board: &'a serde_json::Value,
    event: &crate::inbound_channel::PendingChannelEvent,
    args: &QueueAddArgs,
) -> Result<Option<&'a serde_json::Value>, DeckError> {
    let Some(cards) = board["cards"].as_array() else {
        return Ok(None);
    };
    let matches: Vec<_> = cards
        .iter()
        .filter(|card| {
            card["origin"]["source"] == "channel"
                && card["origin"]["key"] == event.operation_key
                && card["origin"]["badge"] == event.rule_id
        })
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [card] if card["id"] == args.card_id => Ok(Some(*card)),
        [_] => Err(DeckError::new(
            ErrorKind::Invalid,
            "channel card identity does not match",
        )),
        _ => Err(DeckError::new(
            ErrorKind::Invalid,
            "channel origin is not unique",
        )),
    }
}

fn frozen_matches(
    card: &serde_json::Value,
    event: &crate::inbound_channel::PendingChannelEvent,
    args: &QueueAddArgs,
    claim: &ChannelFirstSendClaim,
) -> bool {
    let run = &card["channelRun"];
    let head = &run["initialSteps"][0];
    let target = &run["target"];
    let frozen = &run["firstSend"];
    card["session"] == args.session
        && card["projectId"] == event.target.project_id
        && run["connectionId"] == event.connection_id
        && run["workspaceId"] == event.workspace_id
        && run["channelId"] == event.channel_id
        && run["ruleId"] == event.rule_id
        && run["firstEventId"] == event.event_id
        && run["initialQueued"] == false
        && target["projectId"] == event.target.project_id
        && target["dir"]
            .as_str()
            .and_then(crate::inbound_channel::normalized_dir)
            .as_deref()
            == Some(args.dir.as_str())
        && target["cmd"] == args.cmd
        && target["session"] == args.session
        && args.dir == event.target.dir
        && args.cmd == event.target.cmd
        && frozen["inboxId"] == claim.inbox_id
        && frozen["grantId"] == claim.grant_id
        && frozen["grantDigest"] == claim.grant_digest
        && frozen["skeleton"] == claim.skeleton
        && head["operationId"].as_str() == args.operation_id.as_deref()
        && args.operation_id.as_deref() == Some(operation_id(&claim.inbox_id).as_str())
        && head["text"]
            .as_str()
            .map(super::normalize_prompt)
            .as_deref()
            == Some(super::normalize_prompt(&args.text).as_str())
        && head["mode"] == "at"
        && args.mode == "at"
        && head["at"].as_u64() == args.at
        && head["tpl"].as_str() == args.tpl.as_deref()
        && head["tplIdx"] == 1
        && args.tpl_idx == Some(1)
        && head["tplTotal"].as_u64() == args.tpl_total.map(u64::from)
        && !args.review_each
        && args.group.is_none()
        && !args.external_text
        && args.quiet_secs.is_none()
        && args.every.is_none()
        && args.not_before.is_none()
        && args.win_from.is_none()
        && args.win_to.is_none()
        && args.until_n.is_none()
        && args.until_at.is_none()
        && args.steps.as_ref().is_none_or(Vec::is_empty)
}

/// Recognize a channel run from native inbox + Board facts. A recognized head
/// always returns a constraint. Stale/revoked/freshness failures return a
/// denied constraint; forged request material returns an error.
pub(crate) fn admit(
    args: &QueueAddArgs,
    now: u64,
) -> Result<Option<ChannelFirstSendConstraint>, DeckError> {
    let claim = args.channel_first_send.as_ref();
    let native_binding = if claim.is_none() {
        crate::inbound_channel::channel_constraint_for_card(&args.card_id)?
    } else {
        None
    };
    if native_binding
        .as_ref()
        .is_some_and(|(session, _)| session != &args.session)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "channel session identity changed",
        ));
    }
    let board = match crate::documents::board_authority() {
        Some(board) => board,
        None if claim.is_none() => {
            if native_binding.as_ref().is_some_and(|(_, granted)| *granted) {
                return Err(DeckError::new(
                    ErrorKind::Other,
                    "channel Board authority unavailable",
                ));
            }
            return Ok(None);
        }
        None => {
            return Err(DeckError::new(
                ErrorKind::Other,
                "board authority unavailable",
            ))
        }
    };
    let event = if let Some(claim) = claim {
        crate::inbound_channel::channel_proof(&claim.inbox_id)?
            .ok_or_else(|| DeckError::new(ErrorKind::Invalid, "channel event proof is missing"))?
    } else {
        if native_binding.as_ref().is_some_and(|(_, granted)| *granted) {
            crate::inbound_channel::channel_proof_for_card(&args.card_id)?.ok_or_else(|| {
                DeckError::new(ErrorKind::Invalid, "channel event proof was retired")
            })?
        } else {
            // Missing claims cannot silently downgrade a known channel origin.
            let mut cards = board["cards"].as_array().into_iter().flatten();
            let matching_card = cards
                .find(|card| card["id"] == args.card_id && card["origin"]["source"] == "channel");
            let Some(card) = matching_card else {
                return Ok(None);
            };
            let origin_key = card["origin"]["key"].as_str().ok_or_else(|| {
                DeckError::new(ErrorKind::Invalid, "channel origin key is missing")
            })?;
            let key = origin_key.strip_prefix("channel:").ok_or_else(|| {
                DeckError::new(ErrorKind::Invalid, "channel origin key is malformed")
            })?;
            crate::inbound_channel::channel_proof(key)?.ok_or_else(|| {
                DeckError::new(ErrorKind::Invalid, "channel event proof is missing")
            })?
        }
    };
    let Some(card) = matching_card(&board, &event, args)? else {
        if claim.is_some() || native_binding.as_ref().is_some_and(|(_, granted)| *granted) {
            return Err(DeckError::new(
                ErrorKind::Invalid,
                "channel Board proof is missing",
            ));
        }
        return Ok(None);
    };
    let base = |authorized| ChannelFirstSendConstraint {
        rule: event.rule_id.clone(),
        inbox: event.id.clone(),
        grant_id: claim.map(|c| c.grant_id.clone()).unwrap_or_default(),
        grant_digest: claim.map(|c| c.grant_digest.clone()).unwrap_or_default(),
        operation: args.operation_id.clone().unwrap_or_default(),
        workspace: event.workspace_id.clone(),
        authorized,
    };
    let Some(claim) = claim else {
        let head = &card["channelRun"]["initialSteps"][0];
        let expected = operation_id(&event.id);
        let head_like = args.operation_id.as_deref() == Some(expected.as_str())
            || args.tpl_idx == Some(1)
            || head["text"]
                .as_str()
                .map(super::normalize_prompt)
                .as_deref()
                == Some(super::normalize_prompt(&args.text).as_str());
        if event.first_send_grant.is_some() {
            let exact_later = card["channelRun"]["initialSteps"]
                .as_array()
                .into_iter()
                .flatten()
                .skip(1)
                .any(|step| {
                    step["operationId"].as_str() == args.operation_id.as_deref()
                        && step["text"]
                            .as_str()
                            .map(super::normalize_prompt)
                            .as_deref()
                            == Some(super::normalize_prompt(&args.text).as_str())
                        && step["mode"] == args.mode
                        && step["at"].as_u64() == args.at
                        && step["tpl"].as_str() == args.tpl.as_deref()
                        && step["tplIdx"].as_u64() == args.tpl_idx.map(u64::from)
                        && step["tplTotal"].as_u64() == args.tpl_total.map(u64::from)
                })
                && card["session"] == args.session
                && card["channelRun"]["target"]["session"] == args.session
                && card["channelRun"]["target"]["dir"] == args.dir
                && card["channelRun"]["target"]["cmd"] == args.cmd
                && !args.review_each
                && args.group.is_none()
                && !args.external_text
                && args.quiet_secs.is_none()
                && args.every.is_none()
                && args.not_before.is_none()
                && args.win_from.is_none()
                && args.win_to.is_none()
                && args.until_n.is_none()
                && args.until_at.is_none()
                && args.steps.as_ref().is_none_or(Vec::is_empty);
            if !exact_later || head_like {
                return Err(DeckError::new(
                    ErrorKind::Invalid,
                    "channel first-send claim is missing",
                ));
            }
        }
        return Ok(None);
    };
    // An exact operation replay was handled before admission. A consumed
    // event cannot mint a new operation merely because its proof is retained.
    if !crate::inbound_channel::pending_exact(&event.id)? {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "channel event was already consumed",
        ));
    }
    let frozen = event.first_send_grant.as_ref().ok_or_else(|| {
        DeckError::new(ErrorKind::Invalid, "channel event did not freeze a grant")
    })?;
    if claim.inbox_id != event.id
        || claim.grant_id != frozen.id
        || claim.grant_digest != frozen.digest
        || claim.skeleton != frozen.skeleton
        || !frozen_matches(card, &event, args, claim)
    {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "channel first-send claim is forged",
        ));
    }
    let effective = if frozen.skeleton.contains("{{") {
        expand(&claim.skeleton, &event)
    } else {
        super::normalize_prompt(&claim.skeleton)
    };
    if effective != super::normalize_prompt(&args.text) {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "channel first-send text is forged",
        ));
    }
    let config = crate::inbound_channel::read_config_strict_result()
        .map_err(|_| DeckError::new(ErrorKind::Other, "settings authority unavailable"))?;
    let identity = crate::inbound_channel::current_identity()
        .ok_or_else(|| DeckError::new(ErrorKind::Other, "verified Slack identity unavailable"))?;
    let Some(rule) = config.rules.iter().find(|rule| rule.id == event.rule_id) else {
        return Ok(Some(base(false)));
    };
    let Some(grant) = rule.first_send_grant.as_ref() else {
        return Ok(Some(base(false)));
    };
    // Admission observes whole seconds. Use the upper end of that second,
    // so an imprecise 15-minute boundary never upgrades an already-old message.
    let upper_now_micros = now.saturating_add(1).saturating_mul(1_000_000);
    let fresh = event.staged_at > 0
        && now.saturating_sub(event.staged_at) < MAX_FIRST_ADMISSION_AGE_SECS
        && crate::inbound_channel::message_micros(&event.message_ts).is_some_and(|message| {
            upper_now_micros.saturating_sub(message) <= MAX_FIRST_ADMISSION_AGE_SECS * 1_000_000
        });
    let after_activation =
        crate::inbound_channel::grant_activation(&grant.id).is_some_and(|activation| {
            activation == grant.issued_at_micros
                && crate::inbound_channel::message_micros(&event.message_ts)
                    .is_some_and(|message| message > activation)
        });
    let current = config.connection.enabled
        && card["channelRun"]["firstSendUncertain"] != true
        && after_activation
        && grant.id == claim.grant_id
        && grant.digest == claim.grant_digest
        && identity.team_id == event.workspace_id
        && crate::inbound_channel::grant_valid(rule, grant, &board, &identity).as_deref()
            == Some(claim.skeleton.as_str());
    Ok(Some(base(fresh && current)))
}

pub(crate) enum Standing {
    Clear,
    Revoked,
    Unverified,
}

pub(crate) fn standing(
    item: &QueueItem,
    config: Option<&crate::inbound_channel::ChannelConfig>,
    board: Option<&serde_json::Value>,
) -> Standing {
    let Some(constraint) = item.channel_first_send.as_ref() else {
        return Standing::Clear;
    };
    if !constraint.authorized {
        return Standing::Revoked;
    }
    let (Some(config), Some(board)) = (config, board) else {
        return Standing::Unverified;
    };
    if !config.connection.enabled {
        return Standing::Revoked;
    }
    let Some(rule) = config.rules.iter().find(|rule| rule.id == constraint.rule) else {
        return Standing::Revoked;
    };
    let Some(grant) = rule.first_send_grant.as_ref() else {
        return Standing::Revoked;
    };
    if grant.id == constraint.grant_id
        && grant.digest == constraint.grant_digest
        && crate::inbound_channel::grant_semantically_valid(rule, grant, board)
    {
        Standing::Clear
    } else {
        Standing::Revoked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Vector {
        body: String,
        sender_user_id: Option<String>,
        sender_bot_id: Option<String>,
        channel_id: String,
        skeleton: String,
        expected: String,
        eligible: bool,
    }

    #[derive(Deserialize)]
    struct Vectors {
        vectors: Vec<Vector>,
    }

    fn event(vector: &Vector) -> crate::inbound_channel::PendingChannelEvent {
        crate::inbound_channel::PendingChannelEvent {
            id: "default/T1/E1/R1".into(),
            operation_key: "channel:default/T1/E1/R1".into(),
            group_key: "default/T1/C1/R1".into(),
            connection_id: "default".into(),
            workspace_id: "T1".into(),
            event_id: "E1".into(),
            rule_id: "R1".into(),
            channel_id: vector.channel_id.clone(),
            message_ts: "2000000000.000001".into(),
            thread_ts: None,
            sender_user_id: vector.sender_user_id.clone(),
            sender_bot_id: vector.sender_bot_id.clone(),
            occurred_at: 2_000_000_000,
            staged_at: 2_000_000_000,
            body: vector.body.clone(),
            target: crate::inbound_channel::ChannelTarget {
                project_id: "P1".into(),
                column_id: "C1".into(),
                dir: "/tmp".into(),
                cmd: "claude".into(),
                template: "T".into(),
                idle_minutes: 0,
            },
            board_card_id: None,
            board_session: None,
            first_send_grant: None,
        }
    }

    #[test]
    fn expansion_matches_shared_frontend_vectors_without_recursive_replacement() {
        let vectors: Vectors = serde_json::from_str(include_str!(
            "../../../ui/test/fixtures/channel-first-send.json"
        ))
        .unwrap();
        for vector in vectors.vectors.iter().filter(|vector| vector.eligible) {
            assert_eq!(expand(&vector.skeleton, &event(vector)), vector.expected);
        }
    }

    #[test]
    fn operation_ids_are_closed_and_deterministic() {
        let id = operation_id("default/T1/E1/R1");
        assert_eq!(id.len(), 33);
        assert!(id.starts_with('B'));
        assert_eq!(id, operation_id("default/T1/E1/R1"));
        assert_ne!(id, operation_id("default/T1/E2/R1"));
    }
}
