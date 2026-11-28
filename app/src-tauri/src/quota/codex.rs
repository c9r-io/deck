//! The newest Codex rate limits from the Codex home's own rollout files.
//!
//! Codex writes each session to `sessions/YYYY/MM/DD/rollout-*.jsonl`; its
//! `event_msg` / `token_count` lines carry `rate_limits` (`primary`,
//! `secondary`: `used_percent`, `window_minutes`, `resets_at` epoch
//! seconds; `plan_type`), and the first line, `session_meta`, carries
//! `creator_account_id` (checked on Codex 0.172.0, 2026-11-28). This is not
//! a public interface: it is read for presentation only, and anything that
//! does not parse makes the scan `Err(())` (the view says `unreadable`).
//!
//! Bounded: the newest two day folders, the four newest files in them, the
//! first line up to 64 KiB, the last 512 KiB of each file. Only the window
//! numbers, the plan word and the account id leave this module; the rest
//! of each line is dropped where it is parsed.

use super::{minutes_of, plan_ok, Window};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const FIRST_LINE_MAX: usize = 64 << 10;
const TAIL_MAX: u64 = 512 << 10;
const FILES: usize = 4;
const DAY_FOLDERS: usize = 2;

/// What one rollout said last.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rollout {
    pub(crate) account: Option<String>,
    pub(crate) plan: Option<String>,
    pub(crate) windows: Vec<Window>,
    /// epoch milliseconds of the event
    pub(crate) observed_ms: u64,
}

/// Numeric child folders of `dir`, largest first.
fn numbered(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_str()?.parse::<u32>().ok()?;
            e.file_type().ok()?.is_dir().then(|| (n, e.path()))
        })
        .collect();
    found.sort_by_key(|b| std::cmp::Reverse(b.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// The newest rollout files under `sessions`, newest first.
fn newest_files(sessions: &Path) -> Vec<PathBuf> {
    let mut days = Vec::new();
    'outer: for year in numbered(sessions) {
        for month in numbered(&year) {
            for day in numbered(&month) {
                days.push(day);
                if days.len() == DAY_FOLDERS {
                    break 'outer;
                }
            }
        }
    }
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = days
        .iter()
        .flat_map(|day| std::fs::read_dir(day).into_iter().flatten().flatten())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl"))
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|b| std::cmp::Reverse(b.0));
    files.into_iter().take(FILES).map(|(_, p)| p).collect()
}

/// `2026-11-27T23:05:50.866Z` → epoch milliseconds.
pub(crate) fn iso_ms(text: &str) -> Option<u64> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day) = (
        d.next()?.parse::<i32>().ok()?,
        d.next()?.parse::<u32>().ok()?,
        d.next()?.parse::<u32>().ok()?,
    );
    let mut t = time.split(':');
    let (h, min) = (
        t.next()?.parse::<u64>().ok()?,
        t.next()?.parse::<u64>().ok()?,
    );
    let secs: f64 = t.next()?.parse().ok()?;
    if !(1..=12).contains(&m)
        || !(1..=31).contains(&day)
        || h > 23
        || min > 59
        || !(0.0..61.0).contains(&secs)
    {
        return None;
    }
    let days = crate::procinfo::days_from_civil(y, m, day);
    let base = u64::try_from(days).ok()? * 86_400 + h * 3600 + min * 60;
    Some(base * 1000 + (secs * 1000.0) as u64)
}

fn account_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
}

/// The account of a rollout's first line.
pub(crate) fn account_of(first: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(first).ok()?;
    if value.get("type")?.as_str()? != "session_meta" {
        return None;
    }
    value
        .get("payload")?
        .get("creator_account_id")?
        .as_str()
        .filter(|s| account_ok(s))
        .map(str::to_owned)
}

/// One token_count line's windows, plan word and event time.
pub(crate) type Limits = (Vec<Window>, Option<String>, u64);

/// The rate limits of one `token_count` line, `Err` when it has the shape
/// of one but not of what Deck reads.
pub(crate) fn limits_of(line: &str) -> Option<Result<Limits, ()>> {
    if !line.contains("\"token_count\"") || !line.contains("\"rate_limits\"") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let payload = value.get("payload")?;
    if payload.get("type")?.as_str()? != "token_count" {
        return None;
    }
    let limits = payload.get("rate_limits")?;
    if limits.is_null() {
        return None;
    }
    let observed = value
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(iso_ms);
    let parsed = (|| {
        let mut windows = Vec::new();
        for slot in ["primary", "secondary"] {
            let window = limits.get(slot).ok_or(())?;
            if window.is_null() {
                continue;
            }
            let used = window
                .get("used_percent")
                .and_then(|v| v.as_f64())
                .filter(|v| v.is_finite())
                .ok_or(())?
                .clamp(0.0, 100.0);
            let minutes = window
                .get("window_minutes")
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok());
            let resets_at = window.get("resets_at").and_then(|v| v.as_u64()).ok_or(())?;
            let name = match minutes {
                Some(300) => "five_hour".to_string(),
                Some(10_080) => "seven_day".to_string(),
                Some(m) => format!("{m}m"),
                None => slot.to_string(),
            };
            windows.push(Window {
                minutes: minutes.or_else(|| minutes_of(&name)),
                name,
                used,
                resets_at,
            });
        }
        if windows.is_empty() {
            return Err(());
        }
        let plan = limits
            .get("plan_type")
            .and_then(|v| v.as_str())
            .filter(|p| plan_ok(p))
            .map(str::to_owned);
        Ok((windows, plan, observed.ok_or(())?))
    })();
    Some(parsed)
}

/// The last rate limits of one file and its account.
fn read_file(path: &Path) -> Option<Result<Rollout, ()>> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut first = Vec::new();
    (&mut file)
        .take(FIRST_LINE_MAX as u64)
        .read_to_end(&mut first)
        .ok()?;
    let first_line = first.split(|b| *b == b'\n').next().unwrap_or(&[]);
    let account = std::str::from_utf8(first_line).ok().and_then(account_of);
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(TAIL_MAX);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL_MAX).read_to_end(&mut tail).ok()?;
    let text = String::from_utf8_lossy(&tail);
    // the first line of a cut tail may be partial: it is skipped
    let lines = text.lines().skip(usize::from(start > 0));
    let found = lines
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .find_map(limits_of)?;
    Some(found.map(|(windows, plan, observed_ms)| Rollout {
        account,
        plan,
        windows,
        observed_ms,
    }))
}

/// The newest rate limits under a Codex `sessions` folder: `None` when no
/// rollout carries any, `Err` when the newest one that seems to does not
/// parse.
pub(crate) fn latest(sessions: &Path) -> Option<Result<Rollout, ()>> {
    let mut best: Option<Result<Rollout, ()>> = None;
    for path in newest_files(sessions) {
        match (read_file(&path), &best) {
            (None, _) => {}
            (Some(Ok(found)), Some(Ok(kept))) if found.observed_ms <= kept.observed_ms => {}
            (Some(Err(())), Some(_)) => {}
            (Some(found), _) => best = Some(found),
        }
    }
    best
}
