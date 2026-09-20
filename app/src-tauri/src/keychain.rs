// keychain.rs — the only credential store deck has.
//
// Inbound sources (Slack today) need long-lived API tokens. They never enter
// `~/.deck`: every token lives as a generic password in the user's login
// Keychain under one service name, keyed by a CLOSED account name, so the
// item is readable only by this user and only after macOS's own access
// prompt for a new binary. Callers receive the bytes; nothing here logs,
// returns or interpolates a token into any error string.

use crate::error::{DeckError, ErrorKind};
use security_framework::item::{ItemClass, ItemSearchOptions};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use std::sync::Mutex;

const SERVICE: &str = "io.c9r.deck";
pub(crate) const MCP_SERVICE: &str = "io.c9r.deck.mcp";
const MAX_LEN: usize = 512;

/// Process-local copy of the closed slots after their first successful read. macOS
/// asks the user before an app may read a Keychain item's DATA (and asks
/// again after every rebuild of an unsigned development binary), so the
/// pollers read each slot once per process instead of every 30 seconds.
/// Presence checks never touch the data at all (`has`).
static CACHE: Mutex<[Option<String>; 5]> = Mutex::new([None, None, None, None, None]);

/// Closed set of credential slots. Adding a source means adding its slots
/// here — never accept an account name from the webview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // the service-qualified names keep closed slots unmistakable
pub(crate) enum Slot {
    SlackUserToken,
    SlackAppToken,
    SlackChannelBotToken,
    SlackChannelAppToken,
    ConnectorIdentity,
}

impl Slot {
    pub(crate) fn parse(name: &str) -> Option<Slot> {
        match name {
            "slack-user-token" => Some(Slot::SlackUserToken),
            "slack-app-token" => Some(Slot::SlackAppToken),
            "slack-channel-bot-token" => Some(Slot::SlackChannelBotToken),
            "slack-channel-app-token" => Some(Slot::SlackChannelAppToken),
            "connector-identity" => Some(Slot::ConnectorIdentity),
            _ => None,
        }
    }
    fn account(self) -> &'static str {
        match self {
            Slot::SlackUserToken => "slack-user-token",
            Slot::SlackAppToken => "slack-app-token",
            Slot::SlackChannelBotToken => "slack-channel-bot-token",
            Slot::SlackChannelAppToken => "slack-channel-app-token",
            Slot::ConnectorIdentity => "connector-identity",
        }
    }
    /// The shape a stored value must have. Mistyped tokens are refused at
    /// the door so the poller never spends requests on garbage.
    fn accepts(self, value: &str) -> bool {
        let max_len = if self == Slot::ConnectorIdentity {
            16 * 1024
        } else {
            MAX_LEN
        };
        let body_ok = value.len() <= max_len
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        body_ok
            && match self {
                Slot::SlackUserToken => value.starts_with("xoxp-"),
                Slot::SlackAppToken => value.starts_with("xapp-"),
                Slot::SlackChannelBotToken => value.starts_with("xoxb-"),
                Slot::SlackChannelAppToken => value.starts_with("xapp-"),
                Slot::ConnectorIdentity => value.starts_with("v1_"),
            }
    }
}

pub(crate) fn accepts(slot: Slot, value: &str) -> bool {
    slot.accepts(value)
}

fn cache_slot(slot: Slot) -> usize {
    match slot {
        Slot::SlackUserToken => 0,
        Slot::SlackAppToken => 1,
        Slot::SlackChannelBotToken => 2,
        Slot::SlackChannelAppToken => 3,
        Slot::ConnectorIdentity => 4,
    }
}

fn cache_put(slot: Slot, value: Option<String>) {
    if let Ok(mut c) = CACHE.lock() {
        c[cache_slot(slot)] = value;
    }
}

pub(crate) fn get(slot: Slot) -> Option<String> {
    get_checked(slot).ok().flatten()
}

/// Credential read that preserves the difference between an absent item and
/// a denied/locked/malformed Keychain value. Security identities must use
/// this path so a read failure can never be mistaken for permission to
/// generate and overwrite a new identity.
pub(crate) fn get_checked(slot: Slot) -> Result<Option<String>, DeckError> {
    if let Ok(c) = CACHE.lock() {
        if let Some(v) = &c[cache_slot(slot)] {
            return Ok(Some(v.clone()));
        }
    }
    if crate::smoke_faults::enabled() {
        return Ok(None);
    }
    let bytes = match get_generic_password(SERVICE, slot.account()) {
        Ok(bytes) => bytes,
        Err(error) if error.code() == -25300 => return Ok(None),
        Err(_) => return Err(DeckError::new(ErrorKind::Perm, "keychain unavailable")),
    };
    let value = String::from_utf8(bytes)
        .map_err(|_| DeckError::new(ErrorKind::Recovery, "keychain value is invalid"))?;
    if !slot.accepts(&value) {
        return Err(DeckError::new(
            ErrorKind::Recovery,
            "keychain value is invalid",
        ));
    }
    cache_put(slot, Some(value.clone()));
    Ok(Some(value))
}

/// Attribute-only lookup: answers "is there an item?" without reading its
/// data, so it never triggers the Keychain access prompt.
pub(crate) fn has(slot: Slot) -> bool {
    if let Ok(c) = CACHE.lock() {
        if c[cache_slot(slot)].is_some() {
            return true;
        }
    }
    if crate::smoke_faults::enabled() {
        return false;
    }
    ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(SERVICE)
        .account(slot.account())
        .limit(1)
        .load_attributes(true)
        .search()
        .map(|items| !items.is_empty())
        .unwrap_or(false)
}

/// Store or clear one slot. An empty value clears it. Errors carry only a
/// stable category — never the value, never the Keychain's own message.
pub(crate) fn set(slot: Slot, value: &str) -> Result<(), DeckError> {
    let value = value.trim();
    if value.is_empty() {
        return clear(slot);
    }
    if !slot.accepts(value) {
        return Err(DeckError::new(ErrorKind::Other, "shape"));
    }
    if crate::smoke_faults::enabled() {
        cache_put(slot, Some(value.to_string()));
        return Ok(());
    }
    set_generic_password(SERVICE, slot.account(), value.as_bytes())
        .map_err(|_| DeckError::new(ErrorKind::Other, "keychain"))?;
    cache_put(slot, Some(value.to_string()));
    Ok(())
}

pub(crate) fn clear(slot: Slot) -> Result<(), DeckError> {
    cache_put(slot, None);
    if crate::smoke_faults::enabled() {
        return Ok(());
    }
    match delete_generic_password(SERVICE, slot.account()) {
        Ok(()) => Ok(()),
        // errSecItemNotFound: nothing to clear is success.
        Err(e) if e.code() == -25300 => Ok(()),
        Err(_) => Err(DeckError::new(ErrorKind::Other, "keychain")),
    }
}

fn valid_mcp_account(account: &str) -> bool {
    account.starts_with("client_")
        && account.len() <= 128
        && account
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub(crate) fn set_mcp_credential(account: &str, value: &str) -> Result<(), DeckError> {
    if !valid_mcp_account(account)
        || !value.starts_with("mcp_")
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid MCP credential"));
    }
    if crate::smoke_faults::enabled() {
        return Ok(());
    }
    set_generic_password(MCP_SERVICE, account, value.as_bytes())
        .map_err(|_| DeckError::new(ErrorKind::Other, "keychain"))
}

pub(crate) fn clear_mcp_credential(account: &str) -> Result<(), DeckError> {
    if !valid_mcp_account(account) {
        return Err(DeckError::new(
            ErrorKind::Invalid,
            "invalid MCP credential account",
        ));
    }
    if crate::smoke_faults::enabled() {
        return Ok(());
    }
    match delete_generic_password(MCP_SERVICE, account) {
        Ok(()) => Ok(()),
        Err(error) if error.code() == -25300 => Ok(()),
        Err(_) => Err(DeckError::new(ErrorKind::Other, "keychain")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_a_closed_set() {
        assert_eq!(Slot::parse("slack-user-token"), Some(Slot::SlackUserToken));
        assert_eq!(Slot::parse("slack-app-token"), Some(Slot::SlackAppToken));
        assert_eq!(
            Slot::parse("slack-channel-bot-token"),
            Some(Slot::SlackChannelBotToken)
        );
        assert_eq!(
            Slot::parse("slack-channel-app-token"),
            Some(Slot::SlackChannelAppToken)
        );
        assert_eq!(
            Slot::parse("connector-identity"),
            Some(Slot::ConnectorIdentity)
        );
        assert_eq!(Slot::parse("anything"), None);
        assert_eq!(Slot::parse(""), None);
    }

    #[test]
    fn token_shapes_are_checked_per_slot() {
        assert!(Slot::SlackUserToken.accepts("xoxp-1-abc_DEF-2"));
        assert!(!Slot::SlackUserToken.accepts("xapp-1-abc"));
        assert!(!Slot::SlackAppToken.accepts("xoxp-1-abc"));
        assert!(Slot::SlackAppToken.accepts("xapp-1-A0-2-deadbeef"));
        assert!(Slot::SlackChannelBotToken.accepts("xoxb-1-abc_DEF-2"));
        assert!(!Slot::SlackChannelBotToken.accepts("xoxp-1-abc"));
        assert!(Slot::SlackChannelAppToken.accepts("xapp-1-A0-2-deadbeef"));
        assert!(Slot::ConnectorIdentity.accepts("v1_eyJrZXkiOiJhYmMifQ"));
        assert!(!Slot::ConnectorIdentity.accepts("xapp-1-A0-2-deadbeef"));
        assert!(!Slot::SlackUserToken.accepts("xoxp-has space"));
        assert!(!Slot::SlackUserToken.accepts("xoxp-\n"));
        let long = format!("xoxp-{}", "a".repeat(MAX_LEN));
        assert!(!Slot::SlackUserToken.accepts(&long));
    }

    #[test]
    fn cached_credentials_serve_reads_and_presence_without_keychain_io() {
        cache_put(Slot::SlackUserToken, Some("xoxp-cached".into()));
        cache_put(Slot::SlackAppToken, Some("xapp-cached".into()));
        cache_put(Slot::SlackChannelBotToken, Some("xoxb-cached".into()));
        cache_put(
            Slot::SlackChannelAppToken,
            Some("xapp-channel-cached".into()),
        );
        assert_eq!(Slot::SlackUserToken.account(), "slack-user-token");
        assert_eq!(Slot::SlackAppToken.account(), "slack-app-token");
        assert_eq!(
            Slot::SlackChannelBotToken.account(),
            "slack-channel-bot-token"
        );
        assert_eq!(
            Slot::SlackChannelAppToken.account(),
            "slack-channel-app-token"
        );
        assert!(accepts(Slot::SlackUserToken, "xoxp-valid"));
        assert_eq!(get(Slot::SlackUserToken).as_deref(), Some("xoxp-cached"));
        assert_eq!(get(Slot::SlackAppToken).as_deref(), Some("xapp-cached"));
        assert!(has(Slot::SlackUserToken));
        assert!(has(Slot::SlackAppToken));
        assert!(has(Slot::SlackChannelBotToken));
        assert!(has(Slot::SlackChannelAppToken));
        cache_put(Slot::SlackUserToken, None);
        cache_put(Slot::SlackAppToken, None);
        cache_put(Slot::SlackChannelBotToken, None);
        cache_put(Slot::SlackChannelAppToken, None);
    }
}
