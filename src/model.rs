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
