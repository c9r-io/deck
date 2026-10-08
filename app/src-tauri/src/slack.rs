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

    /// Real-service certification, off by default. It runs only when
    /// `DECK_SLACK_CERT_DIR` names a private directory holding `bot-token`,
    /// `app-token`, `user-token`, `channel` (a channel id the bot is in),
    /// `sender` (the user id that will post) and `marker` (digits); an
    /// operator or driver posts `DECKEXT-<marker>` to that channel once the
    /// test has written `connected` there. Run it alone: it writes this test
    /// process's settings.json and never ends the socket loop it starts.
    /// Nothing here prints a credential or a message body.
    #[test]
    fn real_slack_certification_with_explicit_sandbox_credentials() {
        use crate::sync::LockRecover;
        use std::time::{Duration, Instant};
        let Ok(dir) = std::env::var("DECK_SLACK_CERT_DIR") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .unwrap()
                .trim()
                .to_string()
        };
        let (bot, app, user) = (read("bot-token"), read("app-token"), read("user-token"));
        let (channel, sender, marker) = (read("channel"), read("sender"), read("marker"));
        let until = |seconds: u64, done: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < deadline {
                if done() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            done()
        };
        // no other test may point the Web API at a fixture meanwhile
        let _api = slack_api::TEST_API_LOCK.lock_or_recover();
        let slack = crate::extensions::service("slack").expect("the registered integration");

        // 1. credential adaptation against the real service
        let mut forged = bot.clone();
        let last = if forged.ends_with('a') { 'b' } else { 'a' };
        forged.pop();
        forged.push(last);
        assert_eq!(
            (slack.store_secret)(Slot::SlackBotToken, &forged)
                .unwrap_err()
                .message(),
            "auth",
            "a well-shaped token Slack does not know is refused"
        );
        assert!(!keychain::has(Slot::SlackBotToken), "and never stored");
        (slack.store_secret)(Slot::SlackBotToken, &bot).unwrap();
        (slack.store_secret)(Slot::SlackUserToken, &user).unwrap();
        (slack.store_secret)(Slot::SlackAppToken, &app).unwrap();
        for slot in slack.settable {
            assert!(keychain::has(*slot));
        }
        assert!(!(slack.live)(), "storing credentials connects nothing");
        eprintln!("slack-cert credentials-verified");

        // 2. one saved channel rule; no badge rule
        let settings = serde_json::json!({"inbound":{
            "channelConnection":{"enabled":true,"connectionId":"default"},
            "channelRules":[{"id":"cert","enabled":true,"connectionId":"default",
                "channelIds":[channel],"senderUserIds":[sender],"senderBotIds":[],
                "match":{"kind":"regex","value":"DECKEXT-(?<run>[0-9]+)","groupCapture":"run"},
                "includeThreads":false,"projectId":"P1","columnId":"C1","dir":"~",
                "cmd":"claude","template":"cert","idleMinutes":0}]}});
        crate::storage::save_typed::<crate::documents::SettingsDoc>(
            &crate::documents::settings_path(),
            &settings.to_string(),
        )
        .unwrap();

        // 3. the production socket loop, with the durable stage as the
        // channel consumer (the half of `handle_message` that needs no window)
        let consumers = slack_transport::Consumers {
            channel: Box::new(|identity, text| {
                inbound_channel::stage_message(identity, text).map(|_| ())
            }),
            reaction: Box::new(|_, _, _| {}),
        };
        std::thread::spawn(move || slack_transport::socket_loop(consumers));
        (slack.wake)();
        assert!(
            until(90, &|| (slack.live)()
                && inbound_channel::current_identity().is_some()),
            "the transport connects and publishes the channel identity"
        );
        let identity = inbound_channel::current_identity().unwrap();
        assert!(identity.team_id.starts_with('T') && identity.app_id.starts_with('A'));
        eprintln!("slack-cert connected");
        std::fs::write(dir.join("connected"), "1").unwrap();

        // 4. a real message is staged durably, once
        let staged = || {
            inbound_channel::channel_pending()
                .unwrap()
                .into_iter()
                .filter(|event| event.body.contains(&format!("DECKEXT-{marker}")))
                .collect::<Vec<_>>()
        };
        assert!(until(180, &|| !staged().is_empty()), "the message arrives");
        std::thread::sleep(Duration::from_secs(8));
        let events = staged();
        assert_eq!(
            events.len(),
            1,
            "ACKed after staging: no redelivery, no duplicate"
        );
        let event = &events[0];
        assert_eq!(event.rule_id, "cert");
        assert_eq!(event.channel_id, channel);
        assert_eq!(event.workspace_id, identity.team_id);
        assert_eq!(event.sender_user_id.as_deref(), Some(sender.as_str()));
        assert!(event.first_send_grant.is_none(), "staging grants nothing");
        assert!(crate::datadir::deck_dir()
            .join("channel-inbox.json")
            .is_file());
        eprintln!("slack-cert staged-once");

        // 5. clearing the app credential stops collection and keeps what
        // was already durably received
        (slack.store_secret)(Slot::SlackAppToken, "").unwrap();
        (slack.wake)();
        assert!(until(60, &|| !(slack.live)()), "the connection ends");
        assert!(inbound_channel::current_identity().is_none());
        assert_eq!(staged().len(), 1, "pending work survives the disconnect");
        eprintln!("slack-cert disconnected-pending-kept");
        for slot in slack.settable {
            keychain::clear(*slot).unwrap();
        }
    }
}
