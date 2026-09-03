// keychain.rs — the only credential store deck has.
//
// Inbound sources (Slack today) need long-lived API tokens. They never enter
// `~/.deck`: every token lives as a generic password in the user's login
// Keychain under one service name, keyed by a CLOSED account name, so the
// item is readable only by this user and only after macOS's own access
// prompt for a new binary. Callers receive the bytes; nothing here logs,
// returns or interpolates a token into any error string.

use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};

const SERVICE: &str = "io.c9r.deck";
const MAX_LEN: usize = 512;

/// Closed set of credential slots. Adding a source means adding its slots
/// here — never accept an account name from the webview.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    SlackUserToken,
    SlackAppToken,
}

impl Slot {
    pub(crate) fn parse(name: &str) -> Option<Slot> {
        match name {
            "slack-user-token" => Some(Slot::SlackUserToken),
            "slack-app-token" => Some(Slot::SlackAppToken),
            _ => None,
        }
    }
    fn account(self) -> &'static str {
        match self {
            Slot::SlackUserToken => "slack-user-token",
            Slot::SlackAppToken => "slack-app-token",
        }
    }
    /// The shape a stored value must have. Mistyped tokens are refused at
    /// the door so the poller never spends requests on garbage.
    fn accepts(self, value: &str) -> bool {
        let body_ok = value.len() <= MAX_LEN
            && value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        body_ok
            && match self {
                Slot::SlackUserToken => value.starts_with("xoxp-"),
                Slot::SlackAppToken => value.starts_with("xapp-"),
            }
    }
}

pub(crate) fn get(slot: Slot) -> Option<String> {
    let bytes = get_generic_password(SERVICE, slot.account()).ok()?;
    let value = String::from_utf8(bytes).ok()?;
    slot.accepts(&value).then_some(value)
}

pub(crate) fn has(slot: Slot) -> bool {
    get(slot).is_some()
}

/// Store or clear one slot. An empty value clears it. Errors carry only a
/// stable category — never the value, never the Keychain's own message.
pub(crate) fn set(slot: Slot, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return clear(slot);
    }
    if !slot.accepts(value) {
        return Err("shape".into());
    }
    set_generic_password(SERVICE, slot.account(), value.as_bytes()).map_err(|_| "keychain".to_string())
}

pub(crate) fn clear(slot: Slot) -> Result<(), String> {
    match delete_generic_password(SERVICE, slot.account()) {
        Ok(()) => Ok(()),
        // errSecItemNotFound: nothing to clear is success.
        Err(e) if e.code() == -25300 => Ok(()),
        Err(_) => Err("keychain".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_are_a_closed_set() {
        assert_eq!(Slot::parse("slack-user-token"), Some(Slot::SlackUserToken));
        assert_eq!(Slot::parse("slack-app-token"), Some(Slot::SlackAppToken));
        assert_eq!(Slot::parse("anything"), None);
        assert_eq!(Slot::parse(""), None);
    }

    #[test]
    fn token_shapes_are_checked_per_slot() {
        assert!(Slot::SlackUserToken.accepts("xoxp-1-abc_DEF-2"));
        assert!(!Slot::SlackUserToken.accepts("xapp-1-abc"));
        assert!(!Slot::SlackAppToken.accepts("xoxp-1-abc"));
        assert!(Slot::SlackAppToken.accepts("xapp-1-A0-2-deadbeef"));
        assert!(!Slot::SlackUserToken.accepts("xoxp-has space"));
        assert!(!Slot::SlackUserToken.accepts("xoxp-\n"));
        let long = format!("xoxp-{}", "a".repeat(MAX_LEN));
        assert!(!Slot::SlackUserToken.accepts(&long));
    }
}
