use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub const COLUMNS: [&str; 5] = ["Backlog", "Active", "Waiting", "Review", "Done"];

#[derive(Serialize, Deserialize, Clone)]
pub struct Card {
    pub id: String,
    pub title: String,
    /// Command sent into the session's shell on start (e.g. "claude", "codex").
    pub command: String,
    /// Working directory for the session.
    pub dir: String,
    /// Index into COLUMNS.
    pub column: usize,
    /// tmux session name backing this card.
    pub session: String,
    pub created_at: u64,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Board {
    pub cards: Vec<Card>,
    #[serde(default)]
    pub archived: Vec<Card>,
}

pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck")
}

fn board_path() -> PathBuf {
    data_dir().join("board.json")
}

pub fn notes_path(card_id: &str) -> PathBuf {
    data_dir().join("notes").join(format!("{card_id}.md"))
}

impl Board {
    pub fn load() -> Result<Self> {
        let path = board_path();
        if !path.exists() {
            return Ok(Board::default());
        }
        let raw =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = board_path();
        fs::create_dir_all(data_dir())?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Indices (into self.cards) of cards in the given column, in board order.
    pub fn column_indices(&self, col: usize) -> Vec<usize> {
        self.cards
            .iter()
            .enumerate()
            .filter(|(_, c)| c.column == col)
            .map(|(i, _)| i)
            .collect()
    }
}

pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Compact unique id: epoch millis in base36.
pub fn new_id() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    to_base36(millis)
}

fn to_base36(mut n: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// tmux session name derived from title + id suffix, e.g. "deck-fix-login-abc1".
/// The output alphabet ([a-z0-9-], never leading '-') stays inside what tmux
/// accepts as a target and what can never parse as a CLI flag.
pub fn session_name(title: &str, id: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "card" } else { &slug };
    let slug: String = slug.chars().take(24).collect();
    let tail: String = id
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("deck-{}-{}", slug.trim_matches('-'), tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str, column: usize) -> Card {
        Card {
            id: id.into(),
            title: format!("card {id}"),
            command: String::new(),
            dir: String::new(),
            column,
            session: session_name(&format!("card {id}"), id),
            created_at: 0,
        }
    }

    #[test]
    fn session_names_are_tmux_safe() {
        assert_eq!(session_name("Fix login", "abc123"), "deck-fix-login-c123");
        // punctuation, spaces and unicode collapse to the safe alphabet
        for (title, id) in [
            ("héllo: wörld!!", "zz99"),
            ("  --weird--  ", "1"),
            ("", "abcd"),
            ("日本語だけ", "xy12"),
        ] {
            let s = session_name(title, id);
            assert!(s.starts_with("deck-"), "{s}");
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "unsafe char in {s}"
            );
            assert!(!s.contains("--") || !s.ends_with('-'), "{s}");
        }
        // empty/symbol-only titles still yield a usable name
        assert_eq!(session_name("", "abcd"), "deck-card-abcd");
        // long titles are truncated but keep the unique id tail
        let long = session_name(&"x".repeat(100), "tail9999");
        assert!(long.len() <= 5 + 24 + 1 + 4, "{long}");
        assert!(long.ends_with("-9999"));
    }

    #[test]
    fn column_indices_filter_and_preserve_board_order() {
        let b = Board {
            cards: vec![card("a", 0), card("b", 1), card("c", 0), card("d", 4)],
            archived: Vec::new(),
        };
        assert_eq!(b.column_indices(0), vec![0, 2]);
        assert_eq!(b.column_indices(1), vec![1]);
        assert_eq!(b.column_indices(2), Vec::<usize>::new());
        assert_eq!(b.column_indices(4), vec![3]);
    }

    #[test]
    fn legacy_board_json_without_archived_still_loads() {
        // pre-archive board.json files have no "archived" key
        let raw = r#"{"cards":[{"id":"a","title":"t","command":"","dir":"/tmp",
                       "column":2,"session":"deck-t-a","created_at":5}]}"#;
        let b: Board = serde_json::from_str(raw).expect("legacy shape loads");
        assert_eq!(b.cards.len(), 1);
        assert_eq!(b.cards[0].column, 2);
        assert!(b.archived.is_empty(), "archived defaults to empty");
        // and round-trips with the field present
        let again: Board = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!(again.cards[0].id, "a");
    }

    #[test]
    fn base36_ids_are_compact_and_ordered_by_time() {
        assert_eq!(to_base36(0), "0");
        assert_eq!(to_base36(35), "z");
        assert_eq!(to_base36(36), "10");
        let id = new_id();
        assert!(!id.is_empty());
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()));
    }
}
