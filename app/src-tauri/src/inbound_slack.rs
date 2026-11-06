//! Reaction consumer and search catch-up for the canonical Deck Slack app.
//! The shared transport ACKs routed reaction envelopes before invoking this
//! consumer's Web API fetch. Search remains independent and runs every poll,
//! recovering reactions missed during sleep, disconnect or fetch failure.
//! Only the current user's configured badges become inbound events.
//! Both paths build their event with the ONE constructor `badge_event`, so
//! the same message yields the same canonical bytes (invisible formatting
//! stripped by `admission::strip_invisible` before the event exists) and
//! `{{msg.from}}` names the message's author on both — a user's handle or a
//! bot's name — never the user who added the badge.
//! The mention search (`search_mentions`, for `slack_mentions.rs`) is a
//! third reader on the same token: `<@ME>` over the same window, kept only
//! when the raw text names the user explicitly and someone else wrote it,
//! built by the same constructor with no badge until the user routes it.
//! Its reader (`mention_context`) reads the mention's thread
//! (`conversations.replies`) or the conversation up to it
//! (`conversations.history`) and resolves names (`users.info`): read-only,
//! never part of a routed event.

use serde_json::Value;
use std::collections::HashMap;
use tauri::AppHandle;

use crate::datadir::now_epoch as now_secs;
use crate::inbound::{self, Config, Event, Source, SourceStatus};
use crate::keychain::{self, Slot};
use crate::slack_api::call;
#[cfg(test)]
use crate::slack_api::{encode, last_slack_error, manifest, setup_url, verify, USER_SCOPES};
#[cfg(test)]
use crate::slack_api::{TEST_API, TEST_API_LOCK};

const SEARCH_PAGE: u32 = 100;
const SEARCH_MAX_PAGES: u32 = 3;
const MAX_TEXT: usize = 16 * 1024;

/// `YYYY-MM-DD` for Slack's `after:` modifier, `days` back from now (UTC).
pub(crate) fn after_date(now: u64, days: u64) -> String {
    let secs = now.saturating_sub(days * 86_400);
    let days_since_epoch = (secs / 86_400) as i64;
    // civil-from-days (Howard Hinnant), no chrono dependency
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Slack mrkdwn → plain text a prompt can carry: `<@U1|alice>` → `@alice`,
/// `<#C1|dev>` → `#dev`, `<https://x|label>` → `label (https://x)`,
/// `<https://x>` → `https://x`, HTML entities unescaped. Nothing else is
/// interpreted; formatting marks stay as typed.
pub(crate) fn plain_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let inner = &after[..end];
        let (target, label) = match inner.split_once('|') {
            Some((t, l)) => (t, Some(l)),
            None => (inner, None),
        };
        if let Some(id) = target.strip_prefix('@') {
            out.push('@');
            out.push_str(label.unwrap_or(id));
        } else if let Some(id) = target.strip_prefix('#') {
            out.push('#');
            out.push_str(label.unwrap_or(id));
        } else if let Some(id) = target.strip_prefix('!') {
            out.push('@');
            out.push_str(label.unwrap_or(id));
        } else if target.contains("://") || target.starts_with("mailto:") {
            match label {
                Some(l) if !l.is_empty() && l != target => {
                    out.push_str(l);
                    out.push_str(" (");
                    out.push_str(target);
                    out.push(')');
                }
                _ => out.push_str(target),
            }
        } else {
            out.push('<');
            out.push_str(inner);
            out.push('>');
        }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn clip(s: &str) -> String {
    let s: String = plain_text(s)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    bound(crate::admission::strip_invisible(&s), MAX_TEXT)
}

/// Longest `from` / `where` / `link` label a badge event carries.
const MAX_LABEL: usize = 1024;

/// A one-line display field of a badge event (author, conversation, link):
/// every control character removed, the same invisible-character stripping
/// as the message text, bounded.
fn label(s: &str) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).collect();
    bound(crate::admission::strip_invisible(&s), MAX_LABEL)
}

fn bound(s: String, max: usize) -> String {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The ONE constructor of a Slack badge event, shared by the live reaction
/// path and the search catch-up, so both hand the dispatcher the same
/// canonical bytes for the same message: the text is plain text (mrkdwn
/// resolved), control-filtered, stripped of bidi controls, directional
/// marks, zero-width characters, word joiners, BOM and Unicode tags
/// (`admission::strip_invisible`, which keeps a single ZWJ/ZWNJ between
/// visible characters) and bounded; the author, conversation and link
/// labels get the same treatment on one line. This happens BEFORE the
/// event exists, so the pending event, the webview's frozen plan, the
/// native bounded-step proof and the pasted prompt all carry these bytes.
/// It hides nothing from the person reading the message; it is not a
/// prompt-injection defence.
pub(crate) fn badge_event(
    channel: &str,
    ts: &str,
    badge: &str,
    text: &str,
    from: &str,
    where_: &str,
    link: &str,
) -> Event {
    Event {
        source: "slack".into(),
        key: format!("{channel}/{ts}"),
        badge: badge.to_string(),
        text: clip(text),
        from: label(from),
        where_: label(where_),
        link: label(link),
    }
}

/// Who wrote a Slack message, as both the search match and the message
/// object name it: its `username` (a user's handle in a search match; a
/// bot's or webhook's display name), else the author's user id, else a
/// bot's profile name or id. `{{msg.from}}` is always the AUTHOR of the
/// message that was reacted to — never the person who added the badge.
enum Author<'a> {
    Name(&'a str),
    User(&'a str),
    Unknown,
}

fn author(m: &Value) -> Author<'_> {
    let field = |p: &str| {
        m.pointer(p)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    if let Some(name) = field("/username") {
        Author::Name(name)
    } else if let Some(user) = field("/user") {
        Author::User(user)
    } else if let Some(bot) = field("/bot_profile/name").or_else(|| field("/bot_id")) {
        Author::Name(bot)
    } else {
        Author::Unknown
    }
}

/// A reaction name as the API spells it, minus any skin-tone suffix.
pub(crate) fn plain_badge(reaction: &str) -> &str {
    reaction.split("::").next().unwrap_or(reaction)
}

fn where_label(channel: &Value) -> String {
    let is_im = channel
        .get("is_im")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_mpim = channel
        .get("is_mpim")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let name = channel.get("name").and_then(Value::as_str).unwrap_or("");
    if is_im
        || name.starts_with('D')
            && name.len() > 8
            && name.bytes().all(|b| b.is_ascii_alphanumeric())
    {
        "DM".to_string()
    } else if is_mpim || name.starts_with("mpdm-") {
        "group DM".to_string()
    } else if name.is_empty() {
        channel
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string()
    } else {
        format!("#{name}")
    }
}

/// Build the events one `search.messages` response page yields.
pub(crate) fn events_from_search(body: &Value, badge: &str) -> Vec<Event> {
    let mut out = Vec::new();
    let Some(matches) = body.pointer("/messages/matches").and_then(Value::as_array) else {
        return out;
    };
    for m in matches {
        let (Some(ts), Some(channel)) = (
            m.get("ts").and_then(Value::as_str),
            m.pointer("/channel/id").and_then(Value::as_str),
        ) else {
            continue;
        };
        let text = m.get("text").and_then(Value::as_str).unwrap_or("");
        // a search match names a user author by handle (`username`)
        let from = match author(m) {
            Author::Name(name) | Author::User(name) => name,
            Author::Unknown => "?",
        };
        out.push(badge_event(
            channel,
            ts,
            badge,
            text,
            from,
            &where_label(m.get("channel").unwrap_or(&Value::Null)),
            m.get("permalink").and_then(Value::as_str).unwrap_or(""),
        ));
    }
    out
}

fn search_badge(token: &str, badge: &str) -> Result<Vec<Event>, &'static str> {
    let query = format!(
        "hasmy::{badge}: after:{}",
        after_date(now_secs(), inbound::LOOKBACK_DAYS)
    );
    let count = SEARCH_PAGE.to_string();
    let mut events = Vec::new();
    let mut page = 1u32;
    loop {
        let p = page.to_string();
        let body = call(
            "search.messages",
            token,
            &[
                ("query", &query),
                ("sort", "timestamp"),
                ("sort_dir", "desc"),
                ("count", &count),
                ("page", &p),
            ],
        )?;
        events.extend(events_from_search(&body, badge));
        let pages = body
            .pointer("/messages/pagination/page_count")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        page += 1;
        if page > pages || page > SEARCH_MAX_PAGES {
            break;
        }
    }
    Ok(events)
}

/// Whether `text` (raw mrkdwn, as Slack stores it) names user `me` by an
/// explicit user mention: `<@ME>` or `<@ME|label>`. `@here`, `@channel`,
/// user groups and a plain-text name are not a mention of `me`.
pub(crate) fn mentions_user(text: &str, me: &str) -> bool {
    let plain = format!("<@{me}>");
    let labelled = format!("<@{me}|");
    !me.is_empty() && (text.contains(&plain) || text.contains(&labelled))
}

/// The messages one `search.messages` page yields that explicitly mention
/// `me` and that someone else wrote, as badge-less events built by the ONE
/// constructor (`badge_event`), so a mention routed later carries the same
/// canonical bytes as the same message badged in Slack.
pub(crate) fn mentions_from_search(body: &Value, me: &str) -> Vec<Event> {
    let Some(matches) = body.pointer("/messages/matches").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mine: Vec<&Value> = matches
        .iter()
        .filter(|m| {
            let text = m.get("text").and_then(Value::as_str).unwrap_or("");
            mentions_user(text, me) && m.get("user").and_then(Value::as_str) != Some(me)
        })
        .collect();
    let page = serde_json::json!({"messages": {"matches": mine}});
    events_from_search(&page, "")
}

/// How far back mentions are searched and listed: the longest of the list's
/// three ranges (24 hours, 3 days, a week), so nothing is collected that the
/// list could never show.
pub(crate) const MENTION_DAYS: u64 = 7;

/// Every message of the last `MENTION_DAYS` that explicitly mentions `me`,
/// newest first, bounded by the same page limits as a badge search.
pub(crate) fn search_mentions(token: &str, me: &str) -> Result<Vec<Event>, &'static str> {
    let query = format!("<@{me}> after:{}", after_date(now_secs(), MENTION_DAYS));
    let count = SEARCH_PAGE.to_string();
    let mut events = Vec::new();
    let mut page = 1u32;
    loop {
        let p = page.to_string();
        let body = call(
            "search.messages",
            token,
            &[
                ("query", &query),
                ("sort", "timestamp"),
                ("sort_dir", "desc"),
                ("count", &count),
                ("page", &p),
            ],
        )?;
        events.extend(mentions_from_search(&body, me));
        let pages = body
            .pointer("/messages/pagination/page_count")
            .and_then(Value::as_u64)
            .unwrap_or(1) as u32;
        page += 1;
        if page > pages || page > SEARCH_MAX_PAGES {
            break;
        }
    }
    Ok(events)
}

/* ---------- what a mention's reader sees ---------- */

/// How many earlier messages a mention brings along (besides a thread's
/// first message), and how long each may be.
pub(crate) const CONTEXT_BEFORE: usize = 3;
const CONTEXT_TEXT: usize = 4 * 1024;
const CONTEXT_FILES: usize = 8;

/// One message around a mention, as the reader sees it.
#[derive(Clone, Debug, Default, serde::Serialize, PartialEq)]
pub(crate) struct ContextMessage {
    pub(crate) from: String,
    pub(crate) text: String,
    pub(crate) at: u64,
    pub(crate) files: Vec<String>,
}

/// What the reader of a mention needs to judge it without opening Slack:
/// the mention itself with names in place of user ids and its files, and
/// the messages before it — a thread's first message and the replies just
/// before, or the conversation's last messages before it.
#[derive(Clone, Debug, Default, serde::Serialize, PartialEq)]
pub(crate) struct MentionContext {
    pub(crate) from: String,
    pub(crate) thread: bool,
    pub(crate) before: Vec<ContextMessage>,
    pub(crate) text: String,
    pub(crate) files: Vec<String>,
}

/// `<@ID>` → `<@ID|name>` for every user id `name` knows, so the plain text
/// reads as names. Labelled mentions are left as written.
pub(crate) fn with_names(text: &str, name: &mut dyn FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('>') {
            Some(end)
                if !after[..end].contains('|')
                    && after[..end].bytes().all(|b| b.is_ascii_alphanumeric()) =>
            {
                let id = &after[..end];
                out.push_str(&format!("<@{id}|{}>", name(id)));
                rest = &after[end + 1..];
            }
            _ => {
                out.push_str("<@");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn ts_secs(ts: &str) -> u64 {
    ts.split('.')
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn ts_order(ts: &str) -> (u64, u64) {
    let (a, b) = ts.split_once('.').unwrap_or((ts, "0"));
    (a.parse().unwrap_or(0), b.parse().unwrap_or(0))
}

fn file_names(m: &Value) -> Vec<String> {
    m.get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|f| {
                    f.get("name")
                        .or_else(|| f.get("title"))
                        .and_then(Value::as_str)
                })
                .take(CONTEXT_FILES)
                .map(label)
                .collect()
        })
        .unwrap_or_default()
}

fn reader_text(m: &Value, name: &mut dyn FnMut(&str) -> String, max: usize) -> String {
    let raw = m.get("text").and_then(Value::as_str).unwrap_or("");
    bound(clip(&with_names(raw, name)), max)
}

fn reader_from(m: &Value, name: &mut dyn FnMut(&str) -> String) -> String {
    match author(m) {
        Author::Name(n) => label(n),
        Author::User(id) => label(&name(id)),
        Author::Unknown => "?".to_string(),
    }
}

/// The context of the mention at `ts` from the messages of its thread
/// (`thread`) or of its conversation around it, in any order.
pub(crate) fn context_from(
    messages: &[Value],
    ts: &str,
    thread: bool,
    name: &mut dyn FnMut(&str) -> String,
) -> MentionContext {
    let mut sorted: Vec<&Value> = messages
        .iter()
        .filter(|m| m.get("ts").and_then(Value::as_str).is_some())
        .collect();
    sorted.sort_by_key(|m| ts_order(m["ts"].as_str().unwrap_or("")));
    let own = sorted.iter().find(|m| m["ts"].as_str() == Some(ts));
    let earlier: Vec<&Value> = sorted
        .iter()
        .copied()
        .filter(|m| ts_order(m["ts"].as_str().unwrap_or("")) < ts_order(ts))
        .collect();
    let mut picked: Vec<&Value> = Vec::new();
    if thread {
        if let Some(root) = earlier.first() {
            picked.push(root);
        }
        let replies = &earlier[earlier.len().min(1)..];
        picked.extend(&replies[replies.len().saturating_sub(CONTEXT_BEFORE)..]);
    } else {
        picked.extend(&earlier[earlier.len().saturating_sub(CONTEXT_BEFORE)..]);
    }
    MentionContext {
        from: own.map(|m| reader_from(m, name)).unwrap_or_default(),
        thread,
        before: picked
            .iter()
            .map(|m| ContextMessage {
                from: reader_from(m, name),
                text: reader_text(m, name, CONTEXT_TEXT),
                at: ts_secs(m["ts"].as_str().unwrap_or("")),
                files: file_names(m),
            })
            .collect(),
        text: own
            .map(|m| reader_text(m, name, MAX_TEXT))
            .unwrap_or_default(),
        files: own.map(|m| file_names(m)).unwrap_or_default(),
    }
}

/// The thread a permalink names (`thread_ts=`), when it is not the message itself.
pub(crate) fn thread_of(link: &str, ts: &str) -> Option<String> {
    let at = link.find("thread_ts=")? + "thread_ts=".len();
    let thread: String = link[at..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!thread.is_empty() && thread != ts).then_some(thread)
}

/// Read a mention's context from Slack: its thread, or the conversation's
/// messages up to it. Names are resolved through `names` (a cache the
/// caller keeps). Read-only.
pub(crate) fn mention_context(
    token: &str,
    channel: &str,
    ts: &str,
    link: &str,
    names: &mut HashMap<String, String>,
) -> Result<MentionContext, &'static str> {
    let thread = thread_of(link, ts);
    let body = match &thread {
        Some(root) => call(
            "conversations.replies",
            token,
            &[
                ("channel", channel),
                ("ts", root),
                ("latest", ts),
                ("inclusive", "true"),
                ("limit", "200"),
            ],
        )?,
        None => {
            let limit = (CONTEXT_BEFORE + 1).to_string();
            call(
                "conversations.history",
                token,
                &[
                    ("channel", channel),
                    ("latest", ts),
                    ("inclusive", "true"),
                    ("limit", &limit),
                ],
            )?
        }
    };
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut cache = Names {
        users: std::mem::take(names),
        channels: HashMap::new(),
    };
    let mut name = |id: &str| user_name(token, &mut cache, id);
    let context = context_from(&messages, ts, thread.is_some(), &mut name);
    *names = cache.users;
    Ok(context)
}

/* ---------- live path helpers (need the user token) ---------- */

#[derive(Default)]
struct Names {
    users: HashMap<String, String>,
    channels: HashMap<String, String>,
}

/// A user's handle (`users.info` `name`) — what a search match reports as
/// the author's `username` — else the id itself.
fn user_name(token: &str, names: &mut Names, id: &str) -> String {
    if let Some(n) = names.users.get(id) {
        return n.clone();
    }
    let name = call("users.info", token, &[("user", id)])
        .ok()
        .and_then(|b| {
            b.pointer("/user/name")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| id.to_string());
    names.users.insert(id.to_string(), name.clone());
    name
}

fn channel_label(token: &str, names: &mut Names, id: &str) -> String {
    if let Some(n) = names.channels.get(id) {
        return n.clone();
    }
    let label = call("conversations.info", token, &[("channel", id)])
        .ok()
        .and_then(|b| b.get("channel").map(where_label))
        .unwrap_or_else(|| id.to_string());
    names.channels.insert(id.to_string(), label.clone());
    label
}

#[cfg(test)]
fn message_text(token: &str, channel: &str, ts: &str) -> Result<String, &'static str> {
    message(token, channel, ts).map(|m| {
        m.get("text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    })
}

/// The reacted-to message object (text and author fields).
fn message(token: &str, channel: &str, ts: &str) -> Result<Value, &'static str> {
    let body = call(
        "conversations.history",
        token,
        &[
            ("channel", channel),
            ("latest", ts),
            ("oldest", ts),
            ("inclusive", "true"),
            ("limit", "1"),
        ],
    )?;
    if let Some(m) = body
        .get("messages")
        .and_then(Value::as_array)
        .and_then(|a| {
            a.iter()
                .find(|m| m.get("ts").and_then(Value::as_str) == Some(ts))
        })
    {
        return Ok(m.clone());
    }
    // A thread reply is only reachable through the thread.
    let mut cursor = String::new();
    for _ in 0..3 {
        let mut params: Vec<(&str, &str)> =
            vec![("channel", channel), ("ts", ts), ("limit", "200")];
        if !cursor.is_empty() {
            params.push(("cursor", &cursor));
        }
        let body = call("conversations.replies", token, &params)?;
        if let Some(m) = body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter()
                    .find(|m| m.get("ts").and_then(Value::as_str) == Some(ts))
            })
        {
            return Ok(m.clone());
        }
        cursor = body
            .pointer("/response_metadata/next_cursor")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if cursor.is_empty() {
            break;
        }
    }
    Err("slack")
}

fn permalink(token: &str, channel: &str, ts: &str) -> String {
    call(
        "chat.getPermalink",
        token,
        &[("channel", channel), ("message_ts", ts)],
    )
    .ok()
    .and_then(|b| {
        b.get("permalink")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
    .unwrap_or_default()
}

/// Parse one Socket Mode envelope. Returns (envelope_id, own reaction hit).
pub(crate) fn parse_envelope(
    text: &str,
    self_id: &str,
    badges: &[String],
) -> (Option<String>, Option<(String, String, String)>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return (None, None);
    };
    let envelope = v
        .get("envelope_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if v.get("type").and_then(Value::as_str) != Some("events_api") {
        return (envelope, None);
    }
    let Some(ev) = v.pointer("/payload/event") else {
        return (envelope, None);
    };
    if ev.get("type").and_then(Value::as_str) != Some("reaction_added") {
        return (envelope, None);
    }
    if ev.get("user").and_then(Value::as_str) != Some(self_id) {
        return (envelope, None);
    }
    if ev.pointer("/item/type").and_then(Value::as_str) != Some("message") {
        return (envelope, None);
    }
    let badge = plain_badge(ev.get("reaction").and_then(Value::as_str).unwrap_or(""));
    if !badges.iter().any(|b| b == badge) {
        return (envelope, None);
    }
    let (Some(channel), Some(ts)) = (
        ev.pointer("/item/channel").and_then(Value::as_str),
        ev.pointer("/item/ts").and_then(Value::as_str),
    ) else {
        return (envelope, None);
    };
    (
        envelope,
        Some((channel.to_string(), ts.to_string(), badge.to_string())),
    )
}

/* ---------- routed live Reaction consumer ---------- */

/// Platform ACK is emitted by slack_transport before this fetch. A failure
/// here is recovered by the unchanged search.messages catch-up path.
pub(crate) fn handle_reaction(app: &AppHandle, value: &Value, self_id: &str, badges: &[String]) {
    let (.., hit) = parse_envelope(&value.to_string(), self_id, badges);
    let Some((channel, ts, badge)) = hit else {
        return;
    };
    let Some(user) = keychain::get(Slot::SlackUserToken) else {
        return;
    };
    let Some(ev) = live_event(&user, &channel, &ts, &badge) else {
        return;
    };
    let cfg = inbound::read_config();
    inbound::offer(app, &cfg, vec![ev], true);
}

/// The live path's event for the reacted-to message: the same facts the
/// search catch-up reads, through `badge_event`. The author is the
/// message's (a user id resolved to the handle a search match carries),
/// never the reaction's user.
fn live_event(token: &str, channel: &str, ts: &str, badge: &str) -> Option<Event> {
    let m = message(token, channel, ts).ok()?;
    let text = m.get("text").and_then(Value::as_str).unwrap_or("");
    let mut names = Names::default();
    let from = match author(&m) {
        Author::Name(name) => name.to_string(),
        Author::User(id) => user_name(token, &mut names, id),
        Author::Unknown => "?".to_string(),
    };
    let where_ = channel_label(token, &mut names, channel);
    let link = permalink(token, channel, ts);
    Some(badge_event(channel, ts, badge, text, &from, &where_, &link))
}

/* ---------- Source impl ---------- */

#[derive(Default)]
pub(crate) struct Slack {
    last_poll: Option<u64>,
    last_error: Option<&'static str>,
}

impl Source for Slack {
    fn id(&self) -> &'static str {
        "slack"
    }

    fn enabled(&self, cfg: &Config) -> bool {
        cfg.slack_enabled && keychain::has(Slot::SlackUserToken)
    }

    fn poll(&mut self, _cfg: &Config, badges: &[String]) -> Result<Vec<Event>, &'static str> {
        let token = keychain::get(Slot::SlackUserToken).ok_or("no-token")?;
        // Search is a separate recovery path, but it shares the same installed
        // scope gate as live delivery. A token with partial scopes is not ready.
        crate::slack_api::verify(Slot::SlackUserToken, &token)?;
        let mut all = Vec::new();
        let mut result = Ok(());
        for badge in badges {
            match search_badge(&token, badge) {
                Ok(events) => all.extend(events),
                Err(code) => {
                    result = Err(code);
                    if code == "auth" || code == "ratelimited" {
                        break;
                    }
                }
            }
        }
        self.last_poll = Some(now_secs());
        self.last_error = result.err();
        result.map(|_| all)
    }

    fn set_live(&mut self, _app: &AppHandle, _wanted: bool, _badges: &[String]) {}

    fn status(&self) -> SourceStatus {
        SourceStatus {
            live: crate::slack_transport::connected(),
            last_poll: self.last_poll,
            last_error: self.last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::LockRecover;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    /// Point `call` at a port nothing listens on for the duration of `f`.
    fn with_offline<T>(f: impl FnOnce() -> T) -> T {
        let _serial = TEST_API_LOCK.lock_or_recover();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let unused = listener.local_addr().unwrap();
        drop(listener);
        *TEST_API.lock_or_recover() = Some(format!("http://{unused}/"));
        let value = f();
        *TEST_API.lock_or_recover() = None;
        value
    }

    /// Answer exactly `responses.len()` requests from a local fake Web API
    /// while `f` runs; returns `f`'s value and the raw requests received.
    fn with_responses<T>(responses: Vec<(u16, &str)>, f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let _serial = TEST_API_LOCK.lock_or_recover();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let responses: Vec<(u16, String)> = responses
            .into_iter()
            .map(|(status, body)| (status, body.to_string()))
            .collect();
        let worker = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut raw = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = stream.read(&mut chunk).unwrap();
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&chunk[..n]);
                    let Some(header_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&raw[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    if raw.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
                requests.push(String::from_utf8(raw).unwrap());
                let reason = match status {
                    200 => "OK",
                    429 => "Too Many Requests",
                    _ => "Server Error",
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nx-oauth-scopes: search:read,reactions:read,channels:history,groups:history,im:history,mpim:history,users:read,channels:read,groups:read,im:read,mpim:read\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        *TEST_API.lock_or_recover() = Some(base);
        let value = f();
        *TEST_API.lock_or_recover() = None;
        (value, worker.join().unwrap())
    }

    #[test]
    fn after_date_is_civil_utc() {
        // 2026-09-04 07:00 UTC
        assert_eq!(after_date(1_788_505_200, 0), "2026-09-04");
        assert_eq!(after_date(1_788_505_200, 30), "2026-08-05");
        assert_eq!(after_date(0, 0), "1970-01-01");
        assert_eq!(after_date(951_782_400, 0), "2000-02-29");
    }

    #[test]
    fn query_components_are_percent_encoded() {
        assert_eq!(
            encode("hasmy::deck: after:2026-08-05"),
            "hasmy%3A%3Adeck%3A%20after%3A2026-08-05"
        );
        assert_eq!(encode("+1"), "%2B1");
        assert_eq!(encode("a_b-c.d~e"), "a_b-c.d~e");
    }

    #[test]
    fn setup_link_carries_the_whole_manifest_and_nothing_secret() {
        let url = setup_url();
        assert!(url.starts_with("https://api.slack.com/apps?new_app=1&manifest_json=%7B"));
        let encoded = url.split("manifest_json=").nth(1).unwrap();
        let decoded: String = {
            let bytes = encoded.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    out.push(
                        u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap(), 16)
                            .unwrap(),
                    );
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).unwrap()
        };
        let m: Value = serde_json::from_str(&decoded).unwrap();
        assert_eq!(m, manifest());
        assert_eq!(
            m.pointer("/settings/socket_mode_enabled"),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            m.pointer("/settings/event_subscriptions/user_events/0")
                .and_then(Value::as_str),
            Some("reaction_added")
        );
        let scopes = m
            .pointer("/oauth_config/scopes/user")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(scopes.len(), USER_SCOPES.len());
        assert!(m.get("features").is_some(), "one app includes the bot");
        assert_eq!(
            m.pointer("/oauth_config/scopes/bot"),
            Some(&json!(["channels:history", "groups:history"]))
        );
        assert!(m.pointer("/oauth_config/redirect_urls").is_none());
    }

    #[test]
    fn badge_names_drop_skin_tone() {
        assert_eq!(plain_badge("+1::skin-tone-2"), "+1");
        assert_eq!(plain_badge("deck"), "deck");
    }

    #[test]
    fn search_matches_become_events_with_source_neutral_fields() {
        let body = json!({"ok": true, "messages": {"matches": [
            {"ts": "1.5", "text": "fix the login flicker\nplease", "user": "U1", "username": "alice",
             "channel": {"id": "C1", "name": "frontend"}, "permalink": "https://x.slack.com/archives/C1/p15"},
            {"ts": "2.5", "text": "hi", "user": "U2", "channel": {"id": "D9", "name": "D9ABCDEFGH", "is_im": true}},
            {"text": "no ts"},
            {"ts": "3.5", "text": "grp", "channel": {"id": "G1", "name": "mpdm-a--b--c-1", "is_mpim": true}}
        ]}});
        let ev = events_from_search(&body, "deck");
        assert_eq!(ev.len(), 3);
        assert_eq!(ev[0].key, "C1/1.5");
        assert_eq!(ev[0].badge, "deck");
        assert_eq!(ev[0].from, "alice");
        assert_eq!(ev[0].where_, "#frontend");
        assert_eq!(ev[0].link, "https://x.slack.com/archives/C1/p15");
        assert_eq!(ev[0].text, "fix the login flicker\nplease");
        assert_eq!(ev[1].from, "U2", "falls back to the user id");
        assert_eq!(ev[1].where_, "DM");
        assert_eq!(ev[2].where_, "group DM");
        let json = serde_json::to_value(&ev[0]).unwrap();
        assert!(json.get("where").is_some() && json.get("where_").is_none());
    }

    #[test]
    fn only_explicit_mentions_of_the_user_by_someone_else_are_mentions() {
        assert!(mentions_user("hi <@U_ME>", "U_ME"));
        assert!(mentions_user("hi <@U_ME|me>", "U_ME"));
        assert!(!mentions_user("hi <@U_ME2>", "U_ME"));
        assert!(!mentions_user(
            "<!here> <!channel> <!subteam^S1|devs> @me",
            "U_ME"
        ));
        assert!(!mentions_user("<@U_ME>", ""));
        let body = json!({"ok": true, "messages": {"matches": [
            {"ts": "1.5", "text": "<@U_ME> please look", "user": "U2", "username": "bob",
             "channel": {"id": "C1", "name": "dev"}, "permalink": "https://x/p1"},
            {"ts": "2.5", "text": "<@U_ME> note to self", "user": "U_ME", "username": "me",
             "channel": {"id": "D1", "name": "D1ABCDEFGH", "is_im": true}},
            {"ts": "3.5", "text": "<!here> standup", "user": "U2", "channel": {"id": "C1", "name": "dev"}},
            {"ts": "4.5", "text": "thanks <@U_ME|me>", "user": "U3", "username": "carol",
             "channel": {"id": "C2", "name": "ops"}, "permalink": "https://x/p4?thread_ts=4.0"}
        ]}});
        let ev = mentions_from_search(&body, "U_ME");
        let keys: Vec<&str> = ev.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["C1/1.5", "C2/4.5"]);
        assert_eq!(ev[0].badge, "");
        assert_eq!(
            ev[0],
            badge_event(
                "C1",
                "1.5",
                "",
                "<@U_ME> please look",
                "bob",
                "#dev",
                "https://x/p1"
            )
        );
        assert_eq!(ev[1].text, "thanks @me");
        assert!(mentions_from_search(&json!({"ok": true}), "U_ME").is_empty());
    }

    #[test]
    fn the_mention_search_asks_for_the_user_s_id_over_the_lookback_window() {
        let page = r#"{"ok":true,"messages":{"matches":[{"ts":"1.5","text":"<@U_ME> hi","user":"U2","username":"bob","channel":{"id":"C1","name":"dev"}}],"pagination":{"page_count":1}}}"#;
        let (found, requests) =
            with_responses(vec![(200, page)], || search_mentions("xoxp-t", "U_ME"));
        let found = found.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert!(request.contains("search.messages"));
        assert!(
            request.contains(&format!("query={}", encode("<@U_ME> after:"))),
            "{request}"
        );
        assert!(request.contains("sort=timestamp"));
        let (failed, _) =
            with_responses(vec![(429, r#"{"ok":false,"error":"ratelimited"}"#)], || {
                search_mentions("xoxp-t", "U_ME")
            });
        assert!(failed.is_err());
    }

    #[test]
    fn a_mention_s_reader_gets_its_thread_or_the_messages_before_it_with_names() {
        let mut name = |id: &str| match id {
            "UME" => "me".to_string(),
            "U2" => "bob".to_string(),
            other => other.to_lowercase(),
        };
        let thread = json!([
            {"ts": "10.0", "user": "U3", "text": "migration fails on staging"},
            {"ts": "11.0", "user": "U4", "text": "rolled back once"},
            {"ts": "12.0", "user": "U5", "text": "still broken"},
            {"ts": "13.0", "user": "U6", "text": "logs attached", "files": [{"name": "a.log"}]},
            {"ts": "14.0", "user": "U7", "text": "any idea?"},
            {"ts": "15.0", "user": "U2", "text": "<@UME> can you look?", "files": [{"name": "migration.log"}]},
            {"ts": "16.0", "user": "U3", "text": "after the mention"}
        ]);
        let c = context_from(thread.as_array().unwrap(), "15.0", true, &mut name);
        assert!(c.thread);
        let texts: Vec<&str> = c.before.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "migration fails on staging",
                "still broken",
                "logs attached",
                "any idea?"
            ],
            "the thread's first message and the replies just before"
        );
        assert_eq!(c.before[2].files, ["a.log"]);
        assert_eq!(c.before[0].from, "u3");
        assert_eq!(c.before[0].at, 10);
        assert_eq!(
            (c.from.as_str(), c.text.as_str()),
            ("bob", "@me can you look?")
        );
        assert_eq!(c.files, ["migration.log"]);

        let channel = json!([
            {"ts": "5.0", "user": "U3", "text": "one"},
            {"ts": "3.0", "username": "deploy-bot", "text": "two"},
            {"ts": "6.0", "user": "U2", "text": "hey <@UME|me>"}
        ]);
        let c = context_from(channel.as_array().unwrap(), "6.0", false, &mut name);
        assert!(!c.thread);
        let order: Vec<(&str, &str)> = c
            .before
            .iter()
            .map(|m| (m.from.as_str(), m.text.as_str()))
            .collect();
        assert_eq!(
            order,
            [("deploy-bot", "two"), ("u3", "one")],
            "oldest first"
        );
        assert_eq!(c.text, "hey @me", "a labelled mention is left as written");
        assert_eq!(
            context_from(&[], "6.0", false, &mut name),
            MentionContext::default()
        );
    }

    #[test]
    fn names_replace_ids_and_a_permalink_names_its_thread() {
        let mut name = |id: &str| format!("name-{id}");
        assert_eq!(
            with_names("hi <@U1> and <@U2|bo> <@", &mut name),
            "hi <@U1|name-U1> and <@U2|bo> <@"
        );
        assert_eq!(with_names("<@U1 x> <!here>", &mut name), "<@U1 x> <!here>");
        assert_eq!(
            thread_of("https://x/p1?thread_ts=10.0&cid=C1", "15.0").as_deref(),
            Some("10.0")
        );
        assert_eq!(
            thread_of("https://x/p1?thread_ts=15.0&cid=C1", "15.0"),
            None
        );
        assert_eq!(thread_of("https://x/p1", "15.0"), None);
    }

    #[test]
    fn a_mention_s_context_is_one_read_of_its_thread_or_conversation() {
        let replies = r#"{"ok":true,"messages":[{"ts":"10.0","user":"U3","text":"root"},{"ts":"15.0","user":"U2","text":"<@UME> look"}]}"#;
        let carol = r#"{"ok":true,"user":{"name":"carol"}}"#;
        let bob = r#"{"ok":true,"user":{"name":"bob"}}"#;
        let me = r#"{"ok":true,"user":{"name":"me"}}"#;
        let mut names = HashMap::new();
        let (c, requests) = with_responses(
            vec![(200, replies), (200, bob), (200, carol), (200, me)],
            || {
                mention_context(
                    "xoxp-t",
                    "C1",
                    "15.0",
                    "https://x/p?thread_ts=10.0",
                    &mut names,
                )
            },
        );
        let c = c.unwrap();
        assert!(requests[0].contains("conversations.replies") && requests[0].contains("ts=10.0"));
        assert_eq!(c.before[0].from, "carol");
        assert_eq!(c.text, "@me look");
        assert_eq!(names.len(), 3, "names are kept for the next read");
        let history = r#"{"ok":true,"messages":[{"ts":"15.0","user":"U2","text":"hi"}]}"#;
        let (c, requests) = with_responses(vec![(200, history)], || {
            mention_context("xoxp-t", "C1", "15.0", "https://x/p", &mut names)
        });
        assert!(
            requests[0].contains("conversations.history") && requests[0].contains("latest=15.0")
        );
        assert_eq!(
            c.unwrap().from,
            "bob",
            "a known name is not asked for again"
        );
    }

    #[test]
    fn mrkdwn_becomes_plain_text() {
        assert_eq!(
            plain_text("<@U1|alice> see <#C1|dev> and <https://x.y/z|the doc> or <https://a.b>"),
            "@alice see #dev and the doc (https://x.y/z) or https://a.b"
        );
        assert_eq!(plain_text("<@U1>"), "@U1");
        assert_eq!(plain_text("<!here> a &lt;b&gt; &amp; c"), "@here a <b> & c");
        assert_eq!(plain_text("x < y > z"), "x < y > z");
        assert_eq!(plain_text("unterminated <@U1"), "unterminated <@U1");
    }

    #[test]
    fn text_is_clipped_and_stripped_of_control_bytes() {
        assert_eq!(clip("a\u{7}b\nc\td"), "ab\nc\td");
        let long = "é".repeat(MAX_TEXT);
        let c = clip(&long);
        assert!(c.len() <= MAX_TEXT && c.chars().all(|ch| ch == 'é'));
    }

    #[test]
    fn envelopes_are_filtered_to_own_ruled_message_reactions() {
        let badges = vec!["deck".to_string()];
        let mk = |user: &str, reaction: &str, item_type: &str| {
            json!({"envelope_id": "E1", "type": "events_api", "payload": {"event": {
                "type": "reaction_added", "user": user, "reaction": reaction,
                "item": {"type": item_type, "channel": "C1", "ts": "1.2"}}}})
            .to_string()
        };
        assert_eq!(
            parse_envelope(&mk("U_ME", "deck", "message"), "U_ME", &badges),
            (
                Some("E1".into()),
                Some(("C1".into(), "1.2".into(), "deck".into()))
            )
        );
        assert_eq!(
            parse_envelope(&mk("U_ME", "deck::skin-tone-3", "message"), "U_ME", &badges)
                .1
                .map(|h| h.2),
            Some("deck".into())
        );
        assert_eq!(
            parse_envelope(&mk("U_OTHER", "deck", "message"), "U_ME", &badges),
            (Some("E1".into()), None)
        );
        assert_eq!(
            parse_envelope(&mk("U_ME", "eyes", "message"), "U_ME", &badges),
            (Some("E1".into()), None)
        );
        assert_eq!(
            parse_envelope(&mk("U_ME", "deck", "file"), "U_ME", &badges),
            (Some("E1".into()), None)
        );
        assert_eq!(
            parse_envelope(r#"{"type":"hello"}"#, "U_ME", &badges),
            (None, None)
        );
        assert_eq!(
            parse_envelope(
                r#"{"envelope_id":"E2","type":"slash_commands"}"#,
                "U_ME",
                &badges
            ),
            (Some("E2".into()), None)
        );
        assert_eq!(parse_envelope("not json", "U_ME", &badges), (None, None));
    }

    #[test]
    fn web_api_transport_errors_pagination_and_live_helpers_are_closed() {
        // Exercise the async IPC entry through a real HTTP failure. Verification
        // must run off the async executor and never reach the real Keychain.
        let (error, requests) = with_responses(
            vec![(200, r#"{"ok":false,"error":"invalid_auth"}"#)],
            || {
                tauri::async_runtime::block_on(inbound::inbound_set_secret(
                    "slack-user-token".into(),
                    "xoxp-test-invalid".into(),
                ))
                .unwrap_err()
            },
        );
        assert_eq!(error.message(), "auth");
        assert!(requests[0].starts_with("POST /auth.test HTTP/1.1\r\n"));

        let (body, requests) = with_responses(vec![(200, r#"{"ok":true,"value":7}"#)], || {
            call("auth.test", "secret", &[("query", "a b"), ("badge", "+1")]).unwrap()
        });
        assert_eq!(body["value"], 7);
        assert!(requests[0].starts_with("POST /auth.test HTTP/1.1\r\n"));
        assert!(requests[0].contains("authorization: Bearer secret\r\n"));
        assert!(requests[0].ends_with("query=a%20b&badge=%2B1"));

        let (errors, _) = with_responses(
            vec![
                (200, r#"{"ok":false,"error":"invalid_auth"}"#),
                (200, r#"{"ok":false,"error":"missing_scope"}"#),
                (200, r#"{"ok":false,"error":"ratelimited"}"#),
                (200, r#"{"ok":false,"error":"strange.error-42"}"#),
                (429, "{}"),
                (500, "{}"),
                (200, "not json"),
            ],
            || {
                [
                    call("one", "t", &[]).unwrap_err(),
                    call("two", "t", &[]).unwrap_err(),
                    call("three", "t", &[]).unwrap_err(),
                    call("four", "t", &[]).unwrap_err(),
                    call("five", "t", &[]).unwrap_err(),
                    call("six", "t", &[]).unwrap_err(),
                    call("seven", "t", &[]).unwrap_err(),
                ]
            },
        );
        assert_eq!(
            errors,
            [
                "auth",
                "scope",
                "ratelimited",
                "slack",
                "ratelimited",
                "http",
                "parse"
            ]
        );
        assert_eq!(last_slack_error(), "strangeerror");

        assert_eq!(with_offline(|| call("offline", "t", &[])), Err("network"));

        let (verified, _) = with_responses(
            vec![
                (200, r#"{"ok":true,"team_id":"T1"}"#),
                (200, r#"{"ok":true,"team_id":"T1"}"#),
            ],
            || {
                (
                    verify(Slot::SlackUserToken, "xoxp-test"),
                    verify(Slot::SlackBotToken, "xoxb-test"),
                )
            },
        );
        assert!(verified.0.is_ok() && verified.1.is_ok());

        let page_one = r#"{"ok":true,"messages":{"matches":[{"ts":"1.0","text":"first","username":"alice","channel":{"id":"C1","name":"dev"}}],"pagination":{"page_count":2}}}"#;
        let page_two = r#"{"ok":true,"messages":{"matches":[{"ts":"2.0","text":"second","user":"U2","channel":{"id":"C2","name":"ops"}}],"pagination":{"page_count":2}}}"#;
        let (events, requests) = with_responses(vec![(200, page_one), (200, page_two)], || {
            search_badge("xoxp-test", "deck").unwrap()
        });
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].key, "C1/1.0");
        assert_eq!(events[1].key, "C2/2.0");
        assert!(requests[0].contains("page=1"));
        assert!(requests[1].contains("page=2"));
        assert!(requests[0].contains("query=hasmy%3A%3Adeck%3A%20after%3A"));

        let responses = vec![
            (
                200,
                r#"{"ok":true,"user":{"profile":{"display_name":""},"real_name":"Alice","name":"alice"}}"#,
            ),
            (200, r#"{"ok":true,"channel":{"id":"C1","name":"dev"}}"#),
            (
                200,
                r#"{"ok":true,"messages":[{"ts":"1.0","text":"root"}]}"#,
            ),
            (200, r#"{"ok":true,"messages":[]}"#),
            (
                200,
                r#"{"ok":true,"messages":[{"ts":"2.0","text":"reply"}],"response_metadata":{"next_cursor":""}}"#,
            ),
            (
                200,
                r#"{"ok":true,"permalink":"https://example.slack.com/p2"}"#,
            ),
        ];
        let (resolved, requests) = with_responses(responses, || {
            let mut names = Names::default();
            let user = user_name("xoxp-test", &mut names, "U1");
            let cached_user = user_name("no-network", &mut names, "U1");
            let channel = channel_label("xoxp-test", &mut names, "C1");
            let cached_channel = channel_label("no-network", &mut names, "C1");
            (
                user,
                cached_user,
                channel,
                cached_channel,
                message_text("xoxp-test", "C1", "1.0"),
                message_text("xoxp-test", "C1", "2.0"),
                permalink("xoxp-test", "C1", "2.0"),
            )
        });
        assert_eq!(resolved.0, "alice", "the handle a search match reports");
        assert_eq!(resolved.1, "alice");
        assert_eq!(resolved.2, "#dev");
        assert_eq!(resolved.3, "#dev");
        assert_eq!(resolved.4, Ok("root".into()));
        assert_eq!(resolved.5, Ok("reply".into()));
        assert_eq!(resolved.6, "https://example.slack.com/p2");
        assert_eq!(requests.len(), 6, "cached names issue no second request");

        assert_eq!(where_label(&json!({"is_im": true})), "DM");
        assert_eq!(where_label(&json!({"is_mpim": true})), "group DM");
        assert_eq!(where_label(&json!({"id": "C9"})), "C9");
        assert_eq!(where_label(&Value::Null), "?");

        let slack = Slack::default();
        assert_eq!(slack.id(), "slack");
        assert!(!slack.enabled(&Config::default()));
        let status = slack.status();
        assert!(!status.live);
        assert!(status.last_poll.is_none());
        assert!(status.last_error.is_none());
        assert!(now_secs() > 0);
    }

    #[test]
    fn verify_probes_each_slot_with_the_call_it_can_make() {
        let (verified, requests) = with_responses(
            vec![
                (200, r#"{"ok":true,"team_id":"T1"}"#),
                (200, r#"{"ok":true,"team_id":"T1"}"#),
            ],
            || {
                (
                    verify(Slot::SlackBotToken, "xoxb-test"),
                    verify(Slot::SlackUserToken, "xoxp-test"),
                )
            },
        );
        assert!(verified.0.is_ok() && verified.1.is_ok());
        assert!(requests[0].starts_with("POST /auth.test HTTP/1.1\r\n"));
        assert!(requests[0].contains("authorization: Bearer xoxb-test\r\n"));
        assert!(requests[1].starts_with("POST /auth.test HTTP/1.1\r\n"));
        // the Connector identity is not a Slack token: no request is made
        assert_eq!(
            with_offline(|| verify(Slot::ConnectorIdentity, "v1_identity")),
            Err("slot")
        );
    }

    #[test]
    fn live_helpers_fall_back_to_identifiers_and_bound_thread_paging() {
        // a reply that is in none of the first three thread pages is given up
        // on, with the cursor threaded through every follow-up request
        let page = |cursor: &str| {
            format!(
                r#"{{"ok":true,"messages":[{{"ts":"9.9","text":"other"}}],"response_metadata":{{"next_cursor":"{cursor}"}}}}"#
            )
        };
        let (pages, requests) = with_responses(
            vec![
                (200, r#"{"ok":true,"messages":[]}"#),
                (200, &page("c1")),
                (200, &page("c2")),
                (200, &page("c3")),
            ],
            || message_text("xoxp-test", "C1", "5.0"),
        );
        assert_eq!(pages, Err("slack"));
        assert_eq!(requests.len(), 4);
        assert!(requests[0].starts_with("POST /conversations.history "));
        assert!(requests[1].starts_with("POST /conversations.replies "));
        assert!(
            !requests[1].contains("cursor="),
            "the first page has no cursor"
        );
        assert!(requests[2].ends_with("cursor=c1"));
        assert!(requests[3].ends_with("cursor=c2"));
        // an empty cursor ends the walk early, and a Slack error propagates
        let (short, requests) = with_responses(
            vec![
                (200, r#"{"ok":true,"messages":[]}"#),
                (200, r#"{"ok":true,"messages":[]}"#),
            ],
            || message_text("xoxp-test", "C1", "5.0"),
        );
        assert_eq!(short, Err("slack"));
        assert_eq!(requests.len(), 2);
        let (denied, _) = with_responses(
            vec![(200, r#"{"ok":false,"error":"missing_scope"}"#)],
            || message_text("xoxp-test", "C1", "5.0"),
        );
        assert_eq!(denied, Err("scope"));

        // name lookups degrade to the raw identifier, and are cached as such
        let (fallbacks, requests) = with_responses(
            vec![
                (500, "{}"),
                (200, r#"{"ok":true,"user":{"profile":{"display_name":""}}}"#),
                (200, r#"{"ok":false,"error":"channel_not_found"}"#),
                (200, r#"{"ok":false,"error":"invalid_auth"}"#),
            ],
            || {
                let mut names = Names::default();
                (
                    user_name("xoxp-test", &mut names, "U404"),
                    user_name("xoxp-test", &mut names, "U_NAMELESS"),
                    channel_label("xoxp-test", &mut names, "C404"),
                    permalink("xoxp-test", "C404", "1.0"),
                    user_name("xoxp-test", &mut names, "U404"),
                    channel_label("xoxp-test", &mut names, "C404"),
                )
            },
        );
        assert_eq!(fallbacks.0, "U404");
        assert_eq!(fallbacks.1, "U_NAMELESS", "no usable name field");
        assert_eq!(fallbacks.2, "C404");
        assert_eq!(
            fallbacks.3, "",
            "no permalink is an empty link, never an error"
        );
        assert_eq!(
            (fallbacks.4.as_str(), fallbacks.5.as_str()),
            ("U404", "C404")
        );
        assert_eq!(requests.len(), 4, "the fallback is cached like a hit");

        assert!(events_from_search(&json!({"ok": true}), "deck").is_empty());
        assert!(
            events_from_search(&json!({"ok": true, "messages": {"matches": []}}), "deck")
                .is_empty()
        );
        let no_item = json!({"envelope_id": "E9", "type": "events_api", "payload": {"event": {
            "type": "reaction_added", "user": "U_ME", "reaction": "deck",
            "item": {"type": "message", "ts": "1.2"}}}})
        .to_string();
        assert_eq!(
            parse_envelope(&no_item, "U_ME", &["deck".to_string()]),
            (Some("E9".into()), None),
            "a hit without a channel is acknowledged and dropped"
        );
        let no_event =
            json!({"envelope_id": "E8", "type": "events_api", "payload": {}}).to_string();
        assert_eq!(
            parse_envelope(&no_event, "U_ME", &["deck".to_string()]),
            (Some("E8".into()), None)
        );
        assert!(
            !Slack::default().polled_events_are_live(),
            "polled badges go through the first-poll baseline"
        );
    }

    /// B.2: every Slack badge event is canonical before it exists — hidden
    /// formatting characters are stripped from the text AND the author,
    /// conversation and link labels, with `admission::strip_invisible`'s own
    /// rules (a single ZWJ/ZWNJ between visible characters survives).
    #[test]
    fn badge_events_carry_no_invisible_formatting() {
        let hidden =
            "a\u{202A}b\u{202B}c\u{202C}d\u{202D}e\u{202E}f\u{2066}g\u{2067}h\u{2068}i\u{2069}\
                      j\u{200E}k\u{200F}l\u{200B}m\u{2060}n\u{FEFF}o\u{E0041}\u{E007F}p";
        let body = json!({"ok": true, "messages": {"matches": [
            {"ts": "1.5", "text": format!("{hidden} 👨\u{200D}👩 می\u{200C}خواهم \u{200D}x"),
             "user": "U1", "username": format!("al\u{202E}ice\u{200B}"),
             "channel": {"id": "C1", "name": format!("front\u{2066}end")},
             "permalink": "https://x.slack.com/archives/C1/p15\u{FEFF}"}
        ]}});
        let ev = &events_from_search(&body, "deck")[0];
        assert_eq!(ev.text, "abcdefghijklmnop 👨\u{200D}👩 می\u{200C}خواهم x");
        assert_eq!(ev.from, "alice");
        assert_eq!(ev.where_, "#frontend");
        assert_eq!(ev.link, "https://x.slack.com/archives/C1/p15");
        for field in [&ev.text, &ev.from, &ev.where_, &ev.link] {
            assert_eq!(
                &crate::admission::strip_invisible(field),
                field,
                "already canonical"
            );
        }
        // labels are one bounded line; text keeps its lines
        let long = label(&format!("x\ny\t{}", "z".repeat(4000)));
        assert!(long.starts_with("xyz") && long.len() <= MAX_LABEL);
    }

    /// B.2: the live reaction path and the search catch-up produce the SAME
    /// event for the same message, and `{{msg.from}}` is the message's
    /// author — never the user who added the badge (`U_ME` is not asked for).
    #[test]
    fn live_and_catch_up_agree_on_the_same_message() {
        let text = "please <@U2|bob> look \u{202E}here\u{200B}";
        let search = json!({"ok": true, "messages": {"matches": [
            {"ts": "7.0", "text": text, "user": "U1", "username": "alice",
             "channel": {"id": "C1", "name": "dev"}, "permalink": "https://x.slack.com/p7"}
        ]}});
        let caught_up = events_from_search(&search, "deck").remove(0);
        let history = json!({"ok": true, "messages": [{"ts": "7.0", "text": text, "user": "U1"}]})
            .to_string();
        let (live, requests) = with_responses(
            vec![
                (200, &history),
                (
                    200,
                    r#"{"ok":true,"user":{"name":"alice","real_name":"Alice A","profile":{"display_name":"Al"}}}"#,
                ),
                (200, r#"{"ok":true,"channel":{"id":"C1","name":"dev"}}"#),
                (200, r#"{"ok":true,"permalink":"https://x.slack.com/p7"}"#),
            ],
            || live_event("xoxp-test", "C1", "7.0", "deck").unwrap(),
        );
        assert_eq!(live, caught_up);
        assert_eq!(live.from, "alice");
        assert_eq!(live.text, "please @bob look here");
        assert!(requests[1].starts_with("POST /users.info ") && requests[1].contains("user=U1"));
        assert!(requests.iter().all(|r| !r.contains("U_ME")));
        // a bot-authored message: the same bot name on both paths
        let bot_search = json!({"ok": true, "messages": {"matches": [
            {"ts": "8.0", "text": "deploy failed", "username": "deploy-bot", "bot_id": "B1",
             "channel": {"id": "C1", "name": "dev"}, "permalink": "https://x.slack.com/p8"}
        ]}});
        let bot_history = json!({"ok": true, "messages": [
            {"ts": "8.0", "text": "deploy failed", "bot_id": "B1", "username": "deploy-bot",
             "bot_profile": {"name": "Deploy"}}]})
        .to_string();
        let (bot_live, requests) = with_responses(
            vec![
                (200, &bot_history),
                (200, r#"{"ok":true,"channel":{"id":"C1","name":"dev"}}"#),
                (200, r#"{"ok":true,"permalink":"https://x.slack.com/p8"}"#),
            ],
            || live_event("xoxp-test", "C1", "8.0", "deck").unwrap(),
        );
        assert_eq!(bot_live, events_from_search(&bot_search, "deck").remove(0));
        assert_eq!(bot_live.from, "deploy-bot");
        assert!(
            requests.iter().all(|r| !r.contains("users.info")),
            "no user lookup for a bot"
        );
        // a bot message without a username falls back to its profile name;
        // an author Slack does not name is "?" — never invented
        assert!(matches!(
            author(&json!({"bot_id": "B1", "bot_profile": {"name": "CI"}})),
            Author::Name("CI")
        ));
        assert!(matches!(
            author(&json!({"bot_id": "B1"})),
            Author::Name("B1")
        ));
        assert!(matches!(author(&json!({})), Author::Unknown));
    }

    /// B.2: the native bounded-step proof validates exactly the canonical
    /// bytes: the expansion over the backend's event carries no hidden
    /// character, and a row still holding the raw message fails the proof.
    #[test]
    fn the_bounded_proof_sees_the_canonical_message() {
        let body = json!({"ok": true, "messages": {"matches": [
            {"ts": "9.0", "text": "rm\u{202E}fr- mr\u{200B} now", "user": "U1", "username": "al\u{2066}ice",
             "channel": {"id": "C1", "name": "ops"}}
        ]}});
        let ev = events_from_search(&body, "eyes").remove(0);
        let skeleton = "Investigate: {{msg.text}} (from {{msg.from}})";
        let expanded = crate::scheduler::expand_bounded(skeleton, &ev);
        assert_eq!(
            expanded,
            format!(
                "Investigate: {} (from {})",
                crate::scheduler::external_block("rmfr- mr now"),
                crate::scheduler::external_block("alice")
            )
        );
        assert_eq!(crate::admission::strip_invisible(&expanded), expanded);
        let raw = "Investigate: rm\u{202E}fr- mr\u{200B} now (from al\u{2066}ice)";
        assert_ne!(
            crate::scheduler::normalize_prompt(raw),
            expanded,
            "the raw bytes never match the proof"
        );
    }
}
