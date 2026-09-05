//! One error type for the backend.
//!
//! `DeckError` carries a closed `ErrorKind` plus the human message. The
//! message goes back to the caller (a Tauri command result becomes a UI
//! toast) and is serialized as a plain string, so the webview contract is
//! unchanged; the LOG gets only the kind (`code()`), never the text — raw
//! io/tmux/serde messages can embed absolute paths, directories or file
//! contents.
//!
//! Construction: `DeckError::new(kind, message)` names the kind explicitly;
//! `From<io::Error>` derives it from the io kind; `From<String>` / `From<&str>`
//! classify the text with the same closed rules `err_code` always applied,
//! so an existing `Err("…".into())` keeps working while call sites migrate
//! to explicit kinds; serde and UTF-8 errors convert as `InvalidDoc`. Other
//! library errors go through `.to_string()` at the boundary on purpose: a
//! blanket `From<E: Error>` cannot coexist with `From<String>`.

use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    Perm,
    Locked,
    NotDir,
    TmuxMissing,
    Missing,
    DiskFull,
    NewerSchema,
    ContextChanged,
    InvalidDoc,
    NoSession,
    Tmux,
    Recovery,
    Other,
}

impl ErrorKind {
    /// The stable, content-free code written to app.log.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Perm => "perm",
            ErrorKind::Locked => "locked",
            ErrorKind::NotDir => "not-dir",
            ErrorKind::TmuxMissing => "tmux-missing",
            ErrorKind::Missing => "missing",
            ErrorKind::DiskFull => "disk-full",
            ErrorKind::NewerSchema => "newer-schema",
            ErrorKind::ContextChanged => "context-changed",
            ErrorKind::InvalidDoc => "invalid-doc",
            ErrorKind::NoSession => "no-session",
            ErrorKind::Tmux => "tmux",
            ErrorKind::Recovery => "recovery",
            ErrorKind::Other => "other",
        }
    }

    /// Classify free text: the closed rules every logged error has always
    /// gone through. Only for messages that were not built with a kind.
    pub(crate) fn classify(message: &str) -> ErrorKind {
        let l = message.to_ascii_lowercase();
        if l.contains("permission denied") || l.contains("read-only") {
            ErrorKind::Perm
        } else if l.contains("already running") {
            ErrorKind::Locked
        } else if l.contains("not a directory") {
            ErrorKind::NotDir
        } else if l.contains("tmux not runnable") {
            ErrorKind::TmuxMissing
        } else if l.contains("no such file")
            || l.contains("not found")
            || l.contains("no such path")
        {
            ErrorKind::Missing
        } else if l.contains("no space") || l.contains("quota") {
            ErrorKind::DiskFull
        } else if l.contains("newer deck") {
            ErrorKind::NewerSchema
        } else if l.contains("context identity changed") {
            ErrorKind::ContextChanged
        } else if l.contains("invalid json")
            || l.contains("wrong structure")
            || l.contains("refusing to save")
            || l.contains("expected")
        {
            ErrorKind::InvalidDoc
        } else if l.contains("no server")
            || l.contains("can't find session")
            || l.contains("can't find pane")
            || l.contains("no such session")
        {
            ErrorKind::NoSession
        } else if l.contains("tmux") {
            ErrorKind::Tmux
        } else if l.contains("unreadable") || l.contains("backup") || l.contains("corrupt") {
            ErrorKind::Recovery
        } else {
            ErrorKind::Other
        }
    }

    fn from_io(kind: std::io::ErrorKind) -> Option<ErrorKind> {
        use std::io::ErrorKind as Io;
        Some(match kind {
            Io::PermissionDenied | Io::ReadOnlyFilesystem => ErrorKind::Perm,
            Io::NotFound => ErrorKind::Missing,
            Io::NotADirectory => ErrorKind::NotDir,
            Io::StorageFull | Io::QuotaExceeded => ErrorKind::DiskFull,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeckError {
    kind: ErrorKind,
    message: String,
}

impl DeckError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        DeckError {
            kind,
            message: message.into(),
        }
    }

    /// For library errors with no `From` impl: classify their text.
    pub(crate) fn text(e: impl std::fmt::Display) -> Self {
        DeckError::from(e.to_string())
    }

    pub(crate) fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The content-free code for app.log.
    pub(crate) fn code(&self) -> &'static str {
        self.kind.as_str()
    }

    /// The full text, for the caller's toast or an in-band warning.
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for DeckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Wire format: the message string, exactly what commands returned before.
impl Serialize for DeckError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.message)
    }
}

/// Tests compare messages; production code compares kinds.
impl PartialEq<str> for DeckError {
    fn eq(&self, other: &str) -> bool {
        self.message == other
    }
}

impl PartialEq<&str> for DeckError {
    fn eq(&self, other: &&str) -> bool {
        self.message == *other
    }
}

impl From<String> for DeckError {
    fn from(message: String) -> Self {
        DeckError {
            kind: ErrorKind::classify(&message),
            message,
        }
    }
}

impl From<&str> for DeckError {
    fn from(message: &str) -> Self {
        DeckError::from(message.to_string())
    }
}

impl From<DeckError> for String {
    fn from(e: DeckError) -> String {
        e.message
    }
}

impl From<std::io::Error> for DeckError {
    fn from(e: std::io::Error) -> Self {
        let message = e.to_string();
        DeckError {
            kind: ErrorKind::from_io(e.kind()).unwrap_or_else(|| ErrorKind::classify(&message)),
            message,
        }
    }
}

impl From<serde_json::Error> for DeckError {
    fn from(e: serde_json::Error) -> Self {
        DeckError::new(ErrorKind::InvalidDoc, e.to_string())
    }
}

impl From<std::string::FromUtf8Error> for DeckError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        DeckError::new(ErrorKind::InvalidDoc, e.to_string())
    }
}

/// Stable, content-free category for an error message. The FULL error text
/// goes back to the current operation's caller (command result → UI toast);
/// the log gets only this code — raw io/tmux/storage/serde Display texts can
/// embed absolute paths, directories or file contents and must never be
/// interpolated into app.log.
pub(crate) fn err_code(e: &str) -> &'static str {
    ErrorKind::classify(e).as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_kinds_win_and_free_text_is_classified_by_the_old_rules() {
        let typed = DeckError::new(ErrorKind::NoSession, "permission denied inside the text");
        assert_eq!(
            typed.code(),
            "no-session",
            "a named kind is never re-classified"
        );
        assert_eq!(typed.to_string(), "permission denied inside the text");
        let text: DeckError = "tmux: no server running".into();
        assert_eq!(text.code(), "no-session");
        assert_eq!(err_code("refusing to save wrong structure"), "invalid-doc");
        assert_eq!(err_code("plain"), "other");
    }

    #[test]
    fn io_errors_carry_their_kind_without_text_matching() {
        let e: DeckError = std::io::Error::from(std::io::ErrorKind::PermissionDenied).into();
        assert_eq!(e.kind(), ErrorKind::Perm);
        let e: DeckError = std::io::Error::from(std::io::ErrorKind::NotFound).into();
        assert_eq!(e.kind(), ErrorKind::Missing);
        let e: DeckError = std::io::Error::other("something odd").into();
        assert_eq!(
            e.kind(),
            ErrorKind::Other,
            "an unmapped io kind falls back to the text rules"
        );
        let e: DeckError = serde_json::from_str::<u8>("x").unwrap_err().into();
        assert_eq!(
            e.kind(),
            ErrorKind::InvalidDoc,
            "serde's `expected …` text classifies"
        );
    }

    #[test]
    fn the_wire_format_is_the_message_string() {
        let e = DeckError::new(
            ErrorKind::Locked,
            "another deck instance is already running",
        );
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            "\"another deck instance is already running\""
        );
        let s: String = e.into();
        assert_eq!(s, "another deck instance is already running");
    }
}
