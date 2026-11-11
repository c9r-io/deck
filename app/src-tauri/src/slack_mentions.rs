//! Slack mentions: the messages that explicitly mention the user, listed in
//! Deck so the user can start one of their Slack badge automations on a
//! message without touching Slack.
//!
//! # Contract
//! - Collection. Once per dispatcher poll (`extensions::tick_services`), at
//!   most every `SEARCH_EVERY`, the Slack user token searches
//!   `search.messages` for `<@ME> after:<a week>` (`inbound_slack::
//!   search_mentions`, `MENTION_DAYS`: the longest range the list offers, so
//!   nothing is collected that could never be shown; `search:read` is already a granted user scope, so the
//!   Slack app needs no change). A match counts only when its raw text holds
//!   `<@ME>` or `<@ME|…>` and someone else wrote it: `@here`, `@channel`, user
//!   groups and plain-text names are not mentions of the user. It runs
//!   whenever the Slack source is enabled and the user token is present,
//!   with or without badge rules. Opening the list asks for a fresh search
//!   (`slack_mentions_refresh`, at most every `MIN_REFRESH`).
//! - What is kept. The last search result lives only in memory, as events
//!   built by the ONE badge-event constructor (`inbound_slack::badge_event`),
//!   so a routed mention carries the same canonical bytes as the same message
//!   badged in Slack. Nothing about a mention is written to disk until the
//!   user acts: a dismissal is a `(slack, channel/ts)` identifier plus time in
//!   the dispatcher ledger (`inbound.json` `dismissed`, same TTL and cap as
//!   `seen`); a route is the ordinary seen entry of the badge it fired.
//! - What the reader sees. Each open mention's context is read once, a few
//!   per poll (`CONTEXT_PER_TICK`), read-only, with the scopes the user token
//!   already has (`inbound_slack::mention_context`): a thread reply brings its
//!   thread's first message and the replies just before it, any other
//!   mention the conversation's last messages before it, with names in place
//!   of user ids and file names. It lives in memory beside the list and is
//!   only for the reader: the routed event stays the badge event, byte for
//!   byte what a badge in Slack would carry.
//! - What is listed. Mentions sent in the last `MENTION_DAYS` (to the
//!   second; `after:` is a whole day). The view narrows that to 24 hours or
//!   3 days (`slack-mentions-model.js`). A mention is open while the dispatcher has not seen it
//!   under ANY badge, does not hold it pending, and the user has not
//!   dismissed it (`inbound::open_keys`), and no MCP client handed it to an
//!   Agent rule (below). A message the user badged in Slack
//!   therefore disappears once Deck sees that badge; a badge Deck has no rule
//!   for is unknown to Deck and leaves the message listed.
//! - Routing. `slack_mention_route` hands ONE open mention to the dispatcher
//!   as the badge event of the chosen Slack badge rule (`inbound::offer`,
//!   live, so the baseline gate does not apply). From there everything is the
//!   badge path unchanged: the webview's `planInbound`, the channel admission,
//!   the rule's approval and first-send choice (`TriggerClass::SlackBadge`:
//!   the user's own click on this one message is the per-message human act a
//!   reaction is, and revoking the rule's approval revokes it the same way),
//!   the card, the queue and the ack. The ledger's `(source, key, badge)`
//!   dedupe means a later reaction with the same badge in Slack starts
//!   nothing again. Only rules with `source: "slack"` can be chosen.
//! - The MCP reader (`mcp/mentions.rs`). A delegated client with its own
//!   choice reads this same list through `source_list`/`source_mention`:
//!   memory only, never a Slack call, and refused with a closed word
//!   (`slack-off`, `not-searched`, the last search's failure) whenever the
//!   list may be stale. Its cursor (`source_cursor`) moves when a mention
//!   arrives or leaves or its context is read. It may hand ONE open mention
//!   to one of its Agent rules: the agent-inbox entry carries the mention's
//!   key (`inbound_agent::MentionHandoff`), so the mention leaves this list
//!   and is listed apart as `handed` (client id, rule, card); a skipped item
//!   gives it back. A route checks the same openness (`is_open`); a route
//!   and a handoff of one message in the same instant are not serialized
//!   with each other (the inbox serializes handoffs, the ledger routes).
//! - Marks. An MCP orchestrator may mark a listed mention (handling, for-you,
//!   no-action, `mcp/mentions.rs`); the view carries the standing marks by
//!   key (`MentionsView.marks`, names resolved there) and the user clears
//!   one with `slack_mention_clear_mark`. A mark changes no openness here:
//!   it never routes, dismisses or blocks anything.
//! - Silence. Deck never writes to Slack: no reaction, no reply, no read mark.
//!   What the started Agent does is the template's business.
//! - Attention. The list is a count beside its sidebar entry and nothing
//!   else: no notification, no Dock badge, no card.
//! - Privacy. The webview gets the list only by pulling `slack_mentions`; the
//!   change event `slack-mentions-changed` is content-free. Log lines carry
//!   counts and closed codes only.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

use crate::applog::applog;
use crate::datadir::now_epoch as now_secs;
use crate::error::{DeckError, ErrorKind};
use crate::inbound::{self, Config, Event};
use crate::inbound_slack::{self, MentionContext};
use crate::keychain::{self, Slot};
use crate::sync::LockRecover;

const SOURCE: &str = "slack";
pub(crate) const CHANGED: &str = "slack-mentions-changed";
/// How often the list is searched for while nobody asks for it.
const SEARCH_EVERY: u64 = 60;
/// The soonest an explicit refresh searches again.
const MIN_REFRESH: u64 = 10;
/// Longest key a route or dismissal names (`channel/ts`).
const KEY_MAX: usize = 128;
/// Most keys one dismissal names (the "dismiss all" of a full list).
const DISMISS_MAX: usize = 400;
/// Most mentions whose context one poll reads from Slack.
const CONTEXT_PER_TICK: usize = 10;

#[derive(Default)]
struct State {
    enabled: bool,
    /// The last search's mentions, newest first.
    items: Vec<Event>,
    last_poll: Option<u64>,
    last_error: Option<&'static str>,
    failures: u32,
    refresh: bool,
    /// The open keys last announced, to tell when the list changed.
    shown: Vec<String>,
    /// What each listed mention's reader sees, read once per mention.
    contexts: HashMap<String, MentionContext>,
    /// User id → name, for the readers' text.
    names: HashMap<String, String>,
    /// A debug smoke seeded the list (`smoke_mentions_seed`): the poll
    /// neither searches nor clears it.
    seeded: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let mut guard = STATE.lock_or_recover();
    f(guard.get_or_insert_with(State::default))
}

/// Whether a search is due at `now`.
fn due(state: &State, now: u64) -> bool {
    match state.last_poll {
        None => true,
        Some(last) => {
            let since = now.saturating_sub(last);
            since >= SEARCH_EVERY || (state.refresh && since >= MIN_REFRESH)
        }
    }
}

/// The dispatcher poll's Slack work (`ServiceIntegration::tick`).
pub(crate) fn tick(cfg: &Config, announce: &dyn Fn(&'static str)) {
    if with_state(|s| s.seeded) {
        if announce_if_changed() {
            announce(CHANGED);
        }
        return;
    }
    let enabled = cfg.slack_enabled && keychain::has(Slot::SlackUserToken);
    let now = now_secs();
    let (search, changed) = with_state(|s| {
        if !enabled {
            s.items.clear();
            s.last_error = None;
        }
        let changed = s.enabled != enabled;
        s.enabled = enabled;
        (enabled && due(s, now), changed)
    });
    if search {
        let found = keychain::get(Slot::SlackUserToken)
            .ok_or("no-token")
            .and_then(|token| {
                let me = crate::slack_api::verify(Slot::SlackUserToken, &token)?
                    .get("user_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
                    .ok_or("parse")?;
                crate::inbound_slack::search_mentions(&token, &me)
            });
        record(found, now);
    }
    let read = if enabled { read_contexts() } else { 0 };
    if announce_if_changed() || changed || read > 0 {
        announce(CHANGED);
    }
}

/// Read the context of open mentions that have none yet, a few per poll.
/// A failed read is tried again at a later poll; the list never waits for it.
fn read_contexts() -> usize {
    let open = open_now();
    let wanted: Vec<Event> = with_state(|s| {
        s.contexts
            .retain(|key, _| s.items.iter().any(|e| &e.key == key));
        s.items
            .iter()
            .filter(|e| open.contains(&e.key) && !s.contexts.contains_key(&e.key))
            .take(CONTEXT_PER_TICK)
            .cloned()
            .collect()
    });
    if wanted.is_empty() {
        return 0;
    }
    let Some(token) = keychain::get(Slot::SlackUserToken) else {
        return 0;
    };
    let mut names = with_state(|s| std::mem::take(&mut s.names));
    let mut read = Vec::new();
    for e in &wanted {
        let Some((channel, ts)) = e.key.split_once('/') else {
            continue;
        };
        match crate::inbound_slack::mention_context(&token, channel, ts, &e.link, &mut names) {
            Ok(context) => read.push((e.key.clone(), context)),
            Err(code) => {
                applog(&format!("[mentions] context FAILED ({code})"));
                break;
            }
        }
    }
    let n = read.len();
    with_state(|s| {
        s.names = names;
        s.contexts.extend(read);
    });
    n
}

/// Keep a search's outcome: a failure keeps the last list and says why.
fn record(found: Result<Vec<Event>, &'static str>, now: u64) {
    let line = with_state(|s| {
        s.last_poll = Some(now);
        s.refresh = false;
        match found {
            Ok(items) => {
                s.items = items;
                s.last_error = None;
                let ended = std::mem::take(&mut s.failures);
                (ended > 0).then(|| format!("[mentions] search recovered after {ended} failure(s)"))
            }
            Err(code) => {
                s.last_error = Some(code);
                s.failures = s.failures.saturating_add(1);
                let n = s.failures;
                (n <= 3 || n.is_multiple_of(120))
                    .then(|| format!("[mentions] search FAILED ({code}) ×{n}"))
            }
        }
    });
    if let Some(line) = line {
        applog(&line);
    }
}

/// The keys open now, in list order: the dispatcher's open keys less the
/// mentions an MCP client handed to one of its Agent rules.
fn open_now() -> Vec<String> {
    let keys: Vec<String> = with_state(|s| s.items.iter().map(|e| e.key.clone()).collect());
    let handed = crate::inbound_agent::mention_handoffs(now_secs());
    inbound::open_keys(SOURCE, &keys)
        .into_iter()
        .filter(|k| !handed.iter().any(|h| &h.mention == k))
        .collect()
}

/// Whether mention `key` is still open (listed, untouched by the user, a
/// badge or a handoff).
fn is_open(key: &str) -> bool {
    !inbound::open_keys(SOURCE, &[key.to_string()]).is_empty()
        && !crate::inbound_agent::mention_handoffs(now_secs())
            .iter()
            .any(|h| h.mention == key)
}

/// Whether the open list differs from the one last announced.
fn announce_if_changed() -> bool {
    let open = open_now();
    with_state(|s| {
        if s.shown == open {
            return false;
        }
        s.shown = open;
        true
    })
}

/* ---------- commands ---------- */

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MentionView {
    pub(crate) key: String,
    pub(crate) text: String,
    pub(crate) from: String,
    #[serde(rename = "where")]
    pub(crate) where_: String,
    pub(crate) link: String,
    /// The message's own time, epoch seconds (from its Slack `ts`).
    pub(crate) at: u64,
    /// What the reader needs to judge it, once read (`None` until then).
    pub(crate) context: Option<MentionContext>,
}

/// A listed mention an MCP client handed to one of its Agent rules: the
/// user sees what went where (the client's public id and the rule's
/// automation id; the webview shows their names).
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HandedView {
    #[serde(flatten)]
    mention: MentionView,
    client: String,
    rule: String,
    card: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MentionsView {
    enabled: bool,
    last_poll: Option<u64>,
    last_error: Option<&'static str>,
    items: Vec<MentionView>,
    handed: Vec<HandedView>,
    /// The marks MCP orchestrators set on listed mentions, by key
    /// (`mcp/mentions.rs`): presentation only.
    marks: HashMap<String, crate::mcp::MarkView>,
}

fn message_time(key: &str) -> u64 {
    key.rsplit('/')
        .next()
        .and_then(|ts| ts.split('.').next())
        .and_then(|secs| secs.parse().ok())
        .unwrap_or(0)
}

fn view(
    items: &[Event],
    open: &[String],
    contexts: &HashMap<String, MentionContext>,
    since: u64,
) -> Vec<MentionView> {
    items
        .iter()
        .filter(|e| open.contains(&e.key) && message_time(&e.key) >= since)
        .map(|e| {
            let context = contexts.get(&e.key).cloned();
            // once read, the reader sees names where Slack wrote user ids
            let from = context
                .as_ref()
                .map(|c| c.from.clone())
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| e.from.clone());
            let text = context
                .as_ref()
                .map(|c| c.text.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| e.text.clone());
            MentionView {
                key: e.key.clone(),
                text,
                from,
                where_: e.where_.clone(),
                link: e.link.clone(),
                at: message_time(&e.key),
                context,
            }
        })
        .collect()
}

/// The oldest message time listed: `MENTION_DAYS` back. Slack's `after:`
/// is a whole day, so the search returns a little more than is listed.
fn list_since(now: u64) -> u64 {
    now.saturating_sub(inbound_slack::MENTION_DAYS * 86_400)
}

/// The open mentions of the last `MENTION_DAYS`, newest first, with the
/// state of the last search.
#[tauri::command]
pub(crate) fn slack_mentions() -> MentionsView {
    let open = open_now();
    let now = now_secs();
    let handoffs = crate::inbound_agent::mention_handoffs(now);
    let mut marks = crate::mcp::mention_marks_ui();
    with_state(|s| {
        s.shown = open.clone();
        let handed_keys: Vec<String> = handoffs.iter().map(|h| h.mention.clone()).collect();
        let handed = view(&s.items, &handed_keys, &s.contexts, list_since(now))
            .into_iter()
            .filter_map(|mention| {
                let h = handoffs.iter().find(|h| h.mention == mention.key)?;
                Some(HandedView {
                    mention,
                    client: h.client.clone(),
                    rule: h.rule.clone(),
                    card: h.card.clone(),
                })
            })
            .collect();
        let items = view(&s.items, &open, &s.contexts, list_since(now));
        marks.retain(|key, _| items.iter().any(|m| &m.key == key));
        MentionsView {
            enabled: s.enabled,
            last_poll: s.last_poll,
            last_error: s.last_error,
            items,
            handed,
            marks,
        }
    })
}

/* ---------- the mention source MCP reads (`mcp/mentions.rs`) ---------- */

/// What MCP may read: the open mentions of the last `MENTION_DAYS`, newest
/// first, exactly as the user's list shows them, or a closed word saying why
/// there is no current list: `slack-off` (the Slack source is off or has no
/// user token), `not-searched` (no search has finished yet), or the last
/// search's own failure code (a list that may be stale is never handed out
/// as current). Memory only: no Slack call is made for a reader.
pub(crate) fn source_list() -> Result<Vec<MentionView>, &'static str> {
    let open = open_now();
    with_state(|s| {
        if !s.enabled {
            return Err("slack-off");
        }
        if s.last_poll.is_none() {
            return Err("not-searched");
        }
        if let Some(code) = s.last_error {
            return Err(code);
        }
        Ok(view(&s.items, &open, &s.contexts, list_since(now_secs())))
    })
}

/// One open mention, or why not: a `source_list` word, or `not-found` (not
/// listed, or no longer open).
pub(crate) fn source_mention(key: &str) -> Result<MentionView, &'static str> {
    if !key_shape(key) {
        return Err("not-found");
    }
    source_list()?
        .into_iter()
        .find(|m| m.key == key)
        .ok_or("not-found")
}

/// A short digest of what is listed, for a reader's cursor: it moves when a
/// mention arrives or leaves, or when a mention's context has been read.
pub(crate) fn source_cursor(items: &[MentionView]) -> String {
    let mut bytes = Vec::new();
    for m in items {
        bytes.extend(m.key.as_bytes());
        bytes.push(if m.context.is_some() { 1 } else { 0 });
    }
    crate::ledger::sha(&bytes)[..16].to_string()
}

/// The user cleared a mention's mark (`mcp/mentions.rs`), whoever set it.
#[tauri::command]
pub(crate) fn slack_mention_clear_mark(app: AppHandle, key: String) -> Result<bool, DeckError> {
    if !key_shape(&key) {
        return Err(DeckError::new(ErrorKind::Invalid, "invalid mention key"));
    }
    let cleared = crate::mcp::clear_mention_mark(&key)?;
    if cleared {
        let _ = app.emit(CHANGED, ());
    }
    Ok(cleared)
}

/// A mention left the list through MCP: the webview pulls the list again.
pub(crate) fn note_handed<R: tauri::Runtime>(app: &AppHandle<R>) {
    let _ = app.emit(CHANGED, ());
}

/// Search again soon (the list was opened). Never sooner than `MIN_REFRESH`
/// after the last search.
#[tauri::command]
pub(crate) fn slack_mentions_refresh() {
    with_state(|s| s.refresh = true);
    inbound::wake_poll();
}

fn key_shape(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= KEY_MAX
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'.')
}

/// The badge event a route hands to the dispatcher: mention `key` of
/// `items`, under the Slack badge rule `rule_id` of `cfg`. A closed word
/// says why there is none.
fn route_event(
    items: &[Event],
    cfg: &Config,
    key: &str,
    rule_id: &str,
) -> Result<Event, &'static str> {
    if !cfg.slack_enabled {
        return Err("slack-off");
    }
    let rule = cfg
        .rules
        .iter()
        .find(|r| r.id == rule_id && r.source == SOURCE)
        .ok_or("rule")?;
    let mention = items.iter().find(|e| e.key == key).ok_or("not-found")?;
    Ok(Event {
        badge: rule.badge.clone(),
        ..mention.clone()
    })
}

/// Start the Slack badge rule `rule` on mention `key`, exactly as if the user
/// had added that rule's badge to the message in Slack. Refused with a
/// closed word: `shape`, `slack-off`, `rule`, `not-found`, `settled` (it was
/// started or dismissed already).
#[tauri::command]
pub(crate) fn slack_mention_route(
    app: AppHandle,
    key: String,
    rule: String,
) -> Result<(), DeckError> {
    route(&app, &key, &rule, &inbound::read_config())?;
    let _ = app.emit(CHANGED, ());
    Ok(())
}

fn route<R: tauri::Runtime>(
    app: &AppHandle<R>,
    key: &str,
    rule: &str,
    cfg: &Config,
) -> Result<(), DeckError> {
    let refuse = |word: &'static str| DeckError::new(ErrorKind::Invalid, word);
    if !key_shape(key) || !inbound::bounded_id(rule, inbound::RULE_ID_MAX) {
        return Err(refuse("shape"));
    }
    let event = with_state(|s| route_event(&s.items, cfg, key, rule)).map_err(refuse)?;
    if !is_open(key) {
        return Err(refuse("settled"));
    }
    if inbound::offer(app, cfg, vec![event], true) == 0 {
        return Err(refuse("settled"));
    }
    applog("[mentions] routed");
    Ok(())
}

/// Set mentions aside: they leave the list and start nothing. Keys are
/// identifiers only; an unknown one is recorded like any other and simply
/// never matches a listed mention.
#[tauri::command]
pub(crate) fn slack_mention_dismiss(app: AppHandle, keys: Vec<String>) -> Result<usize, DeckError> {
    let added = dismiss(&keys)?;
    let _ = app.emit(CHANGED, ());
    Ok(added)
}

fn dismiss(keys: &[String]) -> Result<usize, DeckError> {
    if keys.is_empty() || keys.len() > DISMISS_MAX || !keys.iter().all(|k| key_shape(k)) {
        return Err(DeckError::new(ErrorKind::Invalid, "shape"));
    }
    let added = inbound::dismiss(SOURCE, keys);
    applog(&format!("[mentions] dismissed {added}"));
    Ok(added)
}

/// One mention a debug smoke seeds (`smoke_mentions_seed`).
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeedMention {
    key: String,
    text: String,
    from: String,
    #[serde(rename = "where")]
    where_: String,
    link: String,
}

/// Debug smoke only (`smoke_faults::enabled`, the isolated carrier): the
/// list as if a search had found `items` just now, each with its context
/// read (the sender's name, no earlier messages), so the view and the MCP
/// reader run without a Slack workspace. From then on the poll keeps it.
#[tauri::command]
pub(crate) fn smoke_mentions_seed(
    app: AppHandle,
    items: Vec<SeedMention>,
) -> Result<usize, DeckError> {
    if !crate::smoke_faults::enabled() {
        return Err(DeckError::new(
            ErrorKind::Other,
            "smoke seam is unavailable",
        ));
    }
    if items.len() > 20
        || !items
            .iter()
            .all(|m| key_shape(&m.key) && m.key.contains('/'))
    {
        return Err(DeckError::new(ErrorKind::Invalid, "shape"));
    }
    let n = items.len();
    with_state(|s| {
        s.contexts.clear();
        s.items = items
            .iter()
            .map(|m| {
                let (channel, ts) = m.key.split_once('/').unwrap_or_default();
                s.contexts.insert(
                    m.key.clone(),
                    MentionContext {
                        from: m.from.clone(),
                        text: m.text.clone(),
                        ..Default::default()
                    },
                );
                inbound_slack::badge_event(channel, ts, "", &m.text, &m.from, &m.where_, &m.link)
            })
            .collect();
        s.enabled = true;
        s.seeded = true;
        s.last_poll = Some(now_secs());
        s.last_error = None;
    });
    let _ = app.emit(CHANGED, ());
    Ok(n)
}

/// A test's list in place of a search: `items` as found now, their
/// contexts, and the last search's failure (`enabled: false` = source off).
#[cfg(test)]
pub(crate) fn load_for_tests(
    enabled: bool,
    items: Vec<Event>,
    contexts: Vec<(String, MentionContext)>,
    last_error: Option<&'static str>,
) {
    with_state(|s| {
        *s = State {
            enabled,
            items,
            contexts: contexts.into_iter().collect(),
            last_poll: Some(now_secs()),
            last_error,
            ..State::default()
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mention(key: &str, text: &str) -> Event {
        let (channel, ts) = key.split_once('/').unwrap();
        crate::inbound_slack::badge_event(channel, ts, "", text, "alice", "#dev", "https://x/p1")
    }

    /// A key for a message sent `ago` seconds before now.
    fn recent(ago: u64, micros: &str) -> String {
        format!("C1/{}.{micros}", now_secs() - ago)
    }

    fn cfg(enabled: bool) -> Config {
        inbound::config_from_value(Some(&json!({
            "sources": {"slack": {"enabled": enabled}},
            "rules": [
                {"id": "r-eyes", "source": "slack", "badge": "eyes", "projectId": "p", "columnId": "c", "cmd": "claude", "template": "T"},
                {"id": "r-clock", "source": "clock", "badge": "r-clock", "projectId": "p", "columnId": "c", "cmd": "claude", "template": "T",
                 "schedule": {"unit": "day", "minute": 0}}
            ]
        })))
    }

    #[test]
    fn a_route_is_the_rule_s_badge_event_with_the_mention_s_own_bytes() {
        let items = vec![mention(
            "C1/1793942517.831339",
            "<@U1|me> please look \u{200b}here",
        )];
        let ev = route_event(&items, &cfg(true), "C1/1793942517.831339", "r-eyes").unwrap();
        let badged = crate::inbound_slack::badge_event(
            "C1",
            "1793942517.831339",
            "eyes",
            "<@U1|me> please look \u{200b}here",
            "alice",
            "#dev",
            "https://x/p1",
        );
        assert_eq!(
            ev, badged,
            "the same message badged in Slack carries the same bytes"
        );
        assert_eq!(ev.badge, "eyes");
        assert_eq!(ev.source, "slack");
    }

    #[test]
    fn a_route_names_a_listed_mention_and_a_slack_badge_rule_or_says_why_not() {
        let items = vec![mention("C1/1.5", "<@U1> hi")];
        assert_eq!(
            route_event(&items, &cfg(false), "C1/1.5", "r-eyes"),
            Err("slack-off")
        );
        assert_eq!(
            route_event(&items, &cfg(true), "C1/1.5", "r-clock"),
            Err("rule"),
            "only Slack badge rules"
        );
        assert_eq!(
            route_event(&items, &cfg(true), "C1/1.5", "nope"),
            Err("rule")
        );
        assert_eq!(
            route_event(&items, &cfg(true), "C1/9.9", "r-eyes"),
            Err("not-found")
        );
    }

    #[test]
    fn keys_are_identifier_shaped() {
        assert!(key_shape("C0123/1793942517.831339"));
        assert!(!key_shape(""));
        assert!(!key_shape("C1/1.5 x"));
        assert!(!key_shape(&"C".repeat(KEY_MAX + 1)));
        assert!(dismiss(&[]).is_err());
        assert!(dismiss(&["bad key".to_string()]).is_err());
        assert!(dismiss(&vec!["C1/1.5".to_string(); DISMISS_MAX + 1]).is_err());
    }

    #[test]
    fn the_view_lists_open_mentions_with_their_message_time() {
        let items = vec![
            mention("C1/1793942527.287979", "<@U1> b"),
            mention("C1/1793942517.831339", "<@U1> a"),
        ];
        let open = vec!["C1/1793942517.831339".to_string()];
        let v = view(&items, &open, &HashMap::new(), 0);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].at, 1_793_942_517);
        assert_eq!(
            v[0].text, "@U1 a",
            "the text is the badge event's plain text"
        );
        assert_eq!(v[0].context, None, "no context read yet");
        let mut contexts = HashMap::new();
        contexts.insert(
            "C1/1793942517.831339".to_string(),
            MentionContext {
                from: "Alice".into(),
                text: "@Me a".into(),
                ..Default::default()
            },
        );
        let read = view(&items, &open, &contexts, 0);
        assert_eq!(
            (read[0].from.as_str(), read[0].text.as_str()),
            ("Alice", "@Me a"),
            "names once read"
        );
        let json = serde_json::to_value(&v[0]).unwrap();
        assert!(json.get("where").is_some() && json.get("where_").is_none());
        assert_eq!(message_time("bad"), 0);
    }

    #[test]
    fn the_view_lists_only_the_last_week() {
        let now = 1_793_942_527;
        let week = inbound_slack::MENTION_DAYS * 86_400;
        let fresh = format!("C1/{}.000001", now - week);
        let old = format!("C1/{}.000001", now - week - 1);
        let items = vec![mention(&fresh, "<@U1> a"), mention(&old, "<@U1> b")];
        let open = vec![fresh.clone(), old];
        let v = view(&items, &open, &HashMap::new(), list_since(now));
        assert_eq!(
            v.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(),
            [fresh.as_str()],
            "what is collected is what can be shown: one week"
        );
        assert_eq!(list_since(5), 0);
    }

    #[test]
    fn a_routed_mention_becomes_the_badge_item_once_and_leaves_the_list() {
        crate::inbound::with_fresh_runtime(|_| {
            let app = tauri::test::mock_app();
            // the list holds only the last week: the fixtures are minutes old
            let (a, b) = (recent(20, "831339"), recent(10, "287979"));
            let (a, b) = (a.as_str(), b.as_str());
            with_state(|s| s.items = vec![mention(b, "<@U1> two"), mention(a, "<@U1> one")]);
            assert_eq!(slack_mentions().items.len(), 2);
            route(app.handle(), a, "r-eyes", &cfg(true)).unwrap();
            let pending = inbound::inbound_pending();
            assert_eq!(
                pending.len(),
                1,
                "one badge item, the ordinary dispatcher's"
            );
            assert_eq!(pending[0].event.key, a);
            assert_eq!(pending[0].event.badge, "eyes");
            assert_eq!(pending[0].rule.id, "r-eyes");
            assert_eq!(
                route(app.handle(), a, "r-eyes", &cfg(true)).unwrap_err(),
                "settled",
                "never twice"
            );
            let listed: Vec<String> = slack_mentions().items.into_iter().map(|m| m.key).collect();
            assert_eq!(
                listed,
                vec![b.to_string()],
                "a pending route already leaves the list"
            );
            inbound::inbound_ack(pending[0].id, "done".into(), Some("S1".into()), None).unwrap();
            assert_eq!(
                route(app.handle(), a, "r-eyes", &cfg(true)).unwrap_err(),
                "settled"
            );
            assert_eq!(
                route(app.handle(), "C1/x y", "r-eyes", &cfg(true)).unwrap_err(),
                "shape"
            );
            assert_eq!(dismiss(&[b.to_string()]).unwrap(), 1);
            assert!(slack_mentions().items.is_empty());
            assert_eq!(
                route(app.handle(), b, "r-eyes", &cfg(true)).unwrap_err(),
                "settled"
            );
            with_state(|s| *s = State::default());
        });
    }

    #[test]
    fn a_failed_search_keeps_the_last_list_and_says_why() {
        crate::inbound::with_fresh_runtime(|_| {
            with_state(|s| *s = State::default());
            record(Ok(vec![mention(&recent(5, "000001"), "<@U1> hi")]), 100);
            record(Err("ratelimited"), 200);
            let v = slack_mentions();
            assert_eq!(v.items.len(), 1);
            assert_eq!(v.last_error, Some("ratelimited"));
            assert_eq!(v.last_poll, Some(200));
            announce_if_changed();
            assert!(
                !announce_if_changed(),
                "an unchanged list is not announced again"
            );
            record(Ok(Vec::new()), 300);
            assert!(announce_if_changed(), "an emptied list is");
            assert_eq!(slack_mentions().last_error, None);
            with_state(|s| *s = State::default());
        });
    }

    #[test]
    fn searches_are_paced_and_an_open_list_asks_sooner() {
        let mut s = State::default();
        assert!(due(&s, 100), "never searched");
        s.last_poll = Some(100);
        assert!(!due(&s, 105));
        assert!(due(&s, 100 + SEARCH_EVERY));
        s.refresh = true;
        assert!(
            !due(&s, 100 + MIN_REFRESH - 1),
            "a refresh still waits its minimum"
        );
        assert!(due(&s, 100 + MIN_REFRESH));
    }
}
