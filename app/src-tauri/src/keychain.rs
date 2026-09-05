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
const MAX_LEN: usize = 512;

/// Process-local copy of each slot after its first successful read. macOS
/// asks the user before an app may read a Keychain item's DATA (and asks
/// again after every rebuild of an unsigned development binary), so the
/// pollers read each slot once per process instead of every 30 seconds.
/// Presence checks never touch the data at all (`has`).
static CACHE: Mutex<[Option<String>; 2]> = Mutex::new([None, None]);

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
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        body_ok
            && match self {
                Slot::SlackUserToken => value.starts_with("xoxp-"),
                Slot::SlackAppToken => value.starts_with("xapp-"),
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
    }
}

fn cache_put(slot: Slot, value: Option<String>) {
    if let Ok(mut c) = CACHE.lock() {
        c[cache_slot(slot)] = value;
    }
}

pub(crate) fn get(slot: Slot) -> Option<String> {
    if let Ok(c) = CACHE.lock() {
        if let Some(v) = &c[cache_slot(slot)] {
            return Some(v.clone());
        }
    }
    let bytes = get_generic_password(SERVICE, slot.account()).ok()?;
    let value = String::from_utf8(bytes).ok()?;
    let value = slot.accepts(&value).then_some(value)?;
    cache_put(slot, Some(value.clone()));
    Some(value)
}

/// Attribute-only lookup: answers "is there an item?" without reading its
/// data, so it never triggers the Keychain access prompt.
pub(crate) fn has(slot: Slot) -> bool {
    if let Ok(c) = CACHE.lock() {
        if c[cache_slot(slot)].is_some() {
            return true;
        }
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
    set_generic_password(SERVICE, slot.account(), value.as_bytes())
        .map_err(|_| DeckError::new(ErrorKind::Other, "keychain"))?;
    cache_put(slot, Some(value.to_string()));
    Ok(())
}

pub(crate) fn clear(slot: Slot) -> Result<(), DeckError> {
    cache_put(slot, None);
    match delete_generic_password(SERVICE, slot.account()) {
        Ok(()) => Ok(()),
        // errSecItemNotFound: nothing to clear is success.
        Err(e) if e.code() == -25300 => Ok(()),
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

    #[test]
    fn cached_credentials_serve_reads_and_presence_without_keychain_io() {
        cache_put(Slot::SlackUserToken, Some("xoxp-cached".into()));
        cache_put(Slot::SlackAppToken, Some("xapp-cached".into()));
        assert_eq!(Slot::SlackUserToken.account(), "slack-user-token");
        assert_eq!(Slot::SlackAppToken.account(), "slack-app-token");
        assert!(accepts(Slot::SlackUserToken, "xoxp-valid"));
        assert_eq!(get(Slot::SlackUserToken).as_deref(), Some("xoxp-cached"));
        assert_eq!(get(Slot::SlackAppToken).as_deref(), Some("xapp-cached"));
        assert!(has(Slot::SlackUserToken));
        assert!(has(Slot::SlackAppToken));
        cache_put(Slot::SlackUserToken, None);
        cache_put(Slot::SlackAppToken, None);
    }
}
