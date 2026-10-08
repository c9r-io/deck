//! The Slack first-party service integration: ONE descriptor for the static
//! extension tables (`extensions.rs`) over the modules that already do the
//! work.
//!
//! # Contract
//! - One connection. `slack_transport.rs` stays the sole Socket Mode owner;
//!   the badge source (`inbound_slack.rs`) and the channel monitor
//!   (`inbound_channel.rs`) are its two consumers on the same credentials
//!   and the same reconnect loop. `INTEGRATION.start` is the only caller of
//!   `slack_transport::spawn`, which refuses a second owner.
//! - What this side owns: the Slack Web API and Socket Mode protocol, event
//!   parsing, the search/reaction catch-up, the channel inbox staging and
//!   its ACK order (a reaction envelope is ACKed before its recoverable
//!   fetch, a channel message only after durable staging), and proving a
//!   pasted token with Slack before it reaches the Keychain
//!   (`store_secret`).
//! - What it never gets: the Board, the queue, a pane, or a say in
//!   authority. Badge events enter through the dispatcher's `Source` trait
//!   and `inbound::offer`; channel events through the durable inbox and the
//!   webview's ordinary Board transaction. The approval grant, first-send
//!   permission, target identity and readiness checks of a Slack rule stay
//!   in `scheduler/authority.rs`, `scheduler/first_send.rs`,
//!   `scheduler/channel_first_send.rs` and `documents.rs`: Slack-specific
//!   code that the CORE owns, on purpose, so the integration can never
//!   vouch for itself.
//! - The Keychain slots are the host's closed vocabulary (`keychain.rs`).
//!   `settable` names the three the user may store; the two legacy channel
//!   slots are reported for presence only and cleared by the transport's
//!   own command.
//! - Disconnecting stops collection. It does not revoke an approved
//!   delivery or drop durably staged pending work; those follow the
//!   dispatcher's and the scheduler's contracts unchanged.

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::extensions::ServiceIntegration;
use crate::inbound::Source;
use crate::keychain::{self, Slot};
use crate::{inbound_channel, inbound_slack, slack_api, slack_transport};
use serde_json::Value;

pub(crate) const INTEGRATION: ServiceIntegration = ServiceIntegration {
    id: "slack",
    sources,
    start: slack_transport::spawn,
    wake: slack_transport::wake,
    live: slack_transport::connected,
    secrets: &[
        ("slack-user-token", Slot::SlackUserToken),
        ("slack-app-token", Slot::SlackAppToken),
        ("slack-bot-token", Slot::SlackBotToken),
        ("slack-channel-bot-token", Slot::SlackChannelBotToken),
        ("slack-channel-app-token", Slot::SlackChannelAppToken),
    ],
    settable: &[
        Slot::SlackUserToken,
        Slot::SlackBotToken,
        Slot::SlackAppToken,
    ],
    store_secret,
    setup_url: slack_api::setup_url,
    reannounce: inbound_channel::reannounce_pending,
};

/// The badge source. The channel monitor is not a dispatcher source: it
/// stages into its own durable inbox from the shared socket.
fn sources() -> Vec<Box<dyn Source>> {
    vec![Box::new(inbound_slack::Slack::default())]
}

/// Store or clear one Slack credential. A candidate is proven with Slack
/// first; the error is a closed code for the toast and never carries the
/// token or Slack's text.
fn store_secret(slot: Slot, value: &str) -> Result<(), DeckError> {
    let clearing = value.trim().is_empty();
    if !clearing {
        let trimmed = value.trim();
        if !keychain::accepts(slot, trimmed) {
            return Err(DeckError::new(ErrorKind::Invalid, "shape"));
        }
        let verified = (if slot == Slot::SlackAppToken {
            slack_transport::open_url(trimmed).map(|_| Value::Null)
        } else {
            slack_api::verify(slot, trimmed)
        })
        .map_err(|code| {
            let slack_error = slack_api::last_slack_error();
            applog(&format!(
                "[inbound] credential verify FAILED ({code}:{slack_error})"
            ));
            // closed codes the frontend maps to messages, never Slack's text
            DeckError::new(
                ErrorKind::Other,
                match code {
                    "auth" => "auth".to_string(),
                    "scope" => "scope".to_string(),
                    "network" | "timeout" | "http" => "network".to_string(),
                    _ => format!("slack:{slack_error}"),
                },
            )
        })?;
        if slot == Slot::SlackBotToken || slot == Slot::SlackUserToken {
            let other = if slot == Slot::SlackBotToken {
                Slot::SlackUserToken
            } else {
                Slot::SlackBotToken
            };
            if let Some(other_token) = keychain::get(other) {
                let other_identity = slack_api::verify(other, &other_token)
                    .map_err(|_| DeckError::new(ErrorKind::Other, "other-credential"))?;
                let team = verified.get("team_id").and_then(Value::as_str);
                let other_team = other_identity.get("team_id").and_then(Value::as_str);
                let enterprise = verified.get("enterprise_id").and_then(Value::as_str);
                let other_enterprise = other_identity.get("enterprise_id").and_then(Value::as_str);
                if team.is_none()
                    || team != other_team
                    || (enterprise.is_some()
                        && other_enterprise.is_some()
                        && enterprise != other_enterprise)
                {
                    return Err(DeckError::new(ErrorKind::Other, "workspace"));
                }
            }
        }
    }
    keychain::set(slot, value).map_err(|code| {
        DeckError::new(
            ErrorKind::Other,
            match code.message() {
                "shape" => "shape",
                _ => "keychain",
            },
        )
    })?;
    if slot == Slot::SlackBotToken {
        inbound_channel::set_current_identity(None);
    }
    if slot == Slot::SlackAppToken {
        slack_transport::app_credential_saved(!clearing);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_candidate_is_verified_before_canonical_keychain_write() {
        let source = include_str!("slack.rs");
        let body = source
            .split("fn store_secret(slot:")
            .nth(1)
            .unwrap()
            .split("#[cfg(test)]")
            .next()
            .unwrap();
        let verify = body.find("slack_transport::open_url(trimmed)").unwrap();
        let write = body.find("keychain::set(slot, value)").unwrap();
        assert!(verify < write);
        assert!(
            body[verify..write].contains("})?;"),
            "verification errors return before write"
        );
    }

    #[test]
    fn the_descriptor_names_slack_and_only_its_own_slots() {
        assert_eq!(INTEGRATION.id, "slack");
        let ids: Vec<&str> = sources().iter().map(|s| s.id()).collect();
        assert_eq!(ids, ["slack"]);
        for (name, slot) in INTEGRATION.secrets {
            assert_eq!(Slot::parse(name), Some(*slot));
        }
        assert!(!INTEGRATION.settable.contains(&Slot::ConnectorIdentity));
        assert!(!INTEGRATION.settable.contains(&Slot::SlackChannelBotToken));
        assert!(!INTEGRATION.settable.contains(&Slot::SlackChannelAppToken));
        assert_eq!(
            store_secret(Slot::SlackUserToken, "not-a-token")
                .unwrap_err()
                .message(),
            "shape",
            "shape is refused before any network or Keychain call"
        );
    }
}
