// inbound.rs — "自动响应": external services ask deck to start a session.
//
// Three layers, only the top one knows a service:
//   sources (inbound_slack.rs, later others) → one fixed `Event`
//   → this dispatcher: dedupe (`~/.deck/inbound.json`), rule match
//   (settings.json `inbound.rules`), hand the item to the webview, which
//   creates the card idempotently, enqueues the rule's template, and acks.
//
// Boundaries that hold the privacy contract:
// - credentials live in the Keychain (keychain.rs), never in `~/.deck`;
// - `inbound.json` records only (source, key, badge, time): no text, no
//   author, no channel — the message itself lives exactly once, inside the
//   queue item the webview creates, like any typed prompt;
// - the Tauri event is content-free (`inbound-changed`); the webview pulls
//   pending items with `inbound_pending`;
// - log lines carry counts and closed error codes, never message content,
//   badge names, or ids from the remote service.
//
// The first poll for a (source, badge) BASELINES: every message already
// carrying the badge is recorded as seen and starts nothing. Only badges
// added after that become cards. Live (socket) events are new by definition
// and skip the baseline gate. Losing `inbound.json` therefore degrades to a
// re-baseline, never to a flood of cards.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

use crate::keychain;
use crate::storage::{self, applog};

pub(crate) const SOURCES: &[&str] = &["slack"];
const MAX_RULES: usize = 32;
const MAX_SEEN: usize = 5000;
const SEEN_TTL_SECS: u64 = 45 * 24 * 3600;
/// Search covers this many days back; older badges are invisible to the
/// catch-up path by design (their reaction time is unknowable anyway).
pub(crate) const LOOKBACK_DAYS: u64 = 30;
pub(crate) const POLL_INTERVAL: Duration = Duration::from_secs(30);
const RE_EMIT_AFTER: Duration = Duration::from_secs(20);

/* ---------- settings: sources + rules ---------- */

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Rule {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) badge: String,
    pub(crate) project_id: String,
    pub(crate) column_id: String,
    pub(crate) cmd: String,
    pub(crate) template: String,
    /// Working directory for the new card; empty means the user's home.
    #[serde(default)]
    pub(crate) dir: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Config {
    pub(crate) slack_enabled: bool,
    pub(crate) rules: Vec<Rule>,
}

impl Config {
    pub(crate) fn badges(&self, source: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .rules
            .iter()
            .filter(|r| r.source == source)
            .map(|r| r.badge.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }
    fn rule_for(&self, source: &str, badge: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.source == source && r.badge == badge)
    }
}

fn bounded_id(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Emoji names as Slack spells them: `deck`, `white_check_mark`, `+1`.
pub(crate) fn valid_badge(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'+'))
}

/// Structural validation of `settings.inbound`, run by SettingsDoc on load AND
/// before every save, so a malformed rule can never reach disk or the poller.
/// Referential checks (project/column/template exist) are the webview's,
/// because only it holds the Board; a dangling rule is skipped at dispatch.
pub(crate) fn validate_settings(v: &Value) -> Result<(), String> {
    let obj = v.as_object().ok_or("inbound must be an object")?;
    if let Some(sources) = obj.get("sources") {
        let sources = sources.as_object().ok_or("inbound.sources must be an object")?;
        for (name, cfg) in sources {
            if !SOURCES.contains(&name.as_str()) {
                return Err("inbound.sources names an unknown source".into());
            }
            let cfg = cfg.as_object().ok_or("inbound source config must be an object")?;
            if let Some(e) = cfg.get("enabled") {
                if !e.is_boolean() {
                    return Err("inbound source enabled must be a boolean".into());
                }
            }
        }
    }
    if let Some(rules) = obj.get("rules") {
        let rules = rules.as_array().ok_or("inbound.rules must be an array")?;
        if rules.len() > MAX_RULES {
            return Err("too many inbound rules".into());
        }
        let mut pairs = HashSet::new();
        let mut ids = HashSet::new();
        for r in rules {
            let rule: Rule = serde_json::from_value(r.clone())
                .map_err(|_| "inbound rule has the wrong shape".to_string())?;
            if !bounded_id(&rule.id, 64) {
                return Err("inbound rule id must be a bounded identifier".into());
            }
            if !SOURCES.contains(&rule.source.as_str()) {
                return Err("inbound rule names an unknown source".into());
            }
            if !valid_badge(&rule.badge) {
                return Err("inbound rule badge must be an emoji name".into());
            }
            if !bounded_id(&rule.project_id, 128) || !bounded_id(&rule.column_id, 128) {
                return Err("inbound rule must reference bounded project and column ids".into());
            }
            if rule.cmd.len() > 200 || rule.cmd.contains(['\n', '\r']) {
                return Err("inbound rule command must be one bounded line".into());
            }
            if rule.template.is_empty() || rule.template.len() > 120 {
                return Err("inbound rule template name must be a bounded string".into());
            }
            if rule.dir.len() > 1024 || rule.dir.contains(['\n', '\r', '\0']) {
                return Err("inbound rule directory must be one bounded line".into());
            }
            if !ids.insert(rule.id.clone()) {
                return Err("inbound rule ids must be unique".into());
            }
            if !pairs.insert((rule.source.clone(), rule.badge.clone())) {
                return Err("one badge per source maps to one rule".into());
            }
        }
    }
    Ok(())
}

/// Lenient read for the poller: an unreadable or invalid settings file
/// yields the empty config (nothing enabled), never a panic or a guess.
pub(crate) fn read_config() -> Config {
    let raw = match storage::load_typed::<crate::commands::SettingsDoc>(&crate::commands::settings_path()) {
        Ok(Some(doc)) => doc.payload,
        _ => return Config::default(),
    };
    let v: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Config::default(),
    };
    config_from_value(v.get("inbound"))
}

pub(crate) fn config_from_value(v: Option<&Value>) -> Config {
    let Some(v) = v else { return Config::default() };
    if validate_settings(v).is_err() {
        return Config::default();
    }
    let slack_enabled = v
        .pointer("/sources/slack/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let rules = v
        .get("rules")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|r| serde_json::from_value(r.clone()).ok()).collect())
        .unwrap_or_default();
    Config { slack_enabled, rules }
}

/* ---------- the event ---------- */

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Event {
    pub(crate) source: String,
    /// Stable per-source identity of the item (Slack: `channel/ts`).
    pub(crate) key: String,
    pub(crate) badge: String,
    pub(crate) text: String,
    pub(crate) from: String,
    #[serde(rename = "where")]
    pub(crate) where_: String,
    pub(crate) link: String,
}

/// What a source implements. `poll` is the catch-up path (bounded, may be
/// slow to notice); `set_live` turns the instant path on or off. Sources
/// only produce events — they never see rules, cards or the Board.
pub(crate) trait Source: Send {
    fn id(&self) -> &'static str;
    fn enabled(&self, cfg: &Config) -> bool;
    fn poll(&mut self, badges: &[String]) -> Result<Vec<Event>, &'static str>;
    fn set_live(&mut self, app: &AppHandle, wanted: bool, badges: &[String]);
    fn status(&self) -> SourceStatus;
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceStatus {
    pub(crate) live: bool,
    pub(crate) last_poll: Option<u64>,
    pub(crate) last_error: Option<&'static str>,
}

/* ---------- the seen ledger: ~/.deck/inbound.json ---------- */

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Seen {
    source: String,
    key: String,
    badge: String,
    at: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct InboundDoc {
    #[serde(default)]
    seen: Vec<Seen>,
    #[serde(default)]
    baselined: Vec<String>,
}

impl InboundDoc {
    fn has(&self, source: &str, key: &str, badge: &str) -> bool {
        self.seen
            .iter()
            .any(|s| s.source == source && s.key == key && s.badge == badge)
    }
    fn mark(&mut self, source: &str, key: &str, badge: &str, now: u64) -> bool {
        if self.has(source, key, badge) {
            return false;
        }
        self.seen.push(Seen { source: source.into(), key: key.into(), badge: badge.into(), at: now });
        true
    }
    fn is_baselined(&self, source: &str, badge: &str) -> bool {
        self.baselined.iter().any(|b| b == &format!("{source}/{badge}"))
    }
    fn baseline(&mut self, source: &str, badge: &str) {
        let tag = format!("{source}/{badge}");
        if !self.baselined.contains(&tag) {
            self.baselined.push(tag);
        }
    }
    fn prune(&mut self, now: u64) {
        self.seen.retain(|s| now.saturating_sub(s.at) <= SEEN_TTL_SECS);
        if self.seen.len() > MAX_SEEN {
            let drop = self.seen.len() - MAX_SEEN;
            self.seen.drain(..drop);
        }
    }
}

fn doc_path() -> PathBuf {
    storage::deck_dir().join("inbound.json")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/* ---------- runtime ---------- */

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingView {
    pub(crate) id: u64,
    pub(crate) event: Event,
    pub(crate) rule: Rule,
}

struct Pending {
    view: PendingView,
    emitted_at: Instant,
}

struct Runtime {
    doc: InboundDoc,
    dirty: bool,
    pending: Vec<Pending>,
    next_id: u64,
    poll_wanted: bool,
    statuses: Vec<(&'static str, SourceStatus)>,
}

static RT: Mutex<Option<Runtime>> = Mutex::new(None);
static WAKE: OnceLock<(Mutex<bool>, Condvar)> = OnceLock::new();

fn wake() -> &'static (Mutex<bool>, Condvar) {
    WAKE.get_or_init(|| (Mutex::new(false), Condvar::new()))
}

fn with_rt<R>(f: impl FnOnce(&mut Runtime) -> R) -> R {
    let mut guard = RT.lock().unwrap_or_else(|p| p.into_inner());
    let rt = guard.get_or_insert_with(|| Runtime {
        doc: load_doc(),
        dirty: false,
        pending: Vec::new(),
        next_id: 1,
        poll_wanted: false,
        statuses: Vec::new(),
    });
    f(rt)
}

fn load_doc() -> InboundDoc {
    match storage::load_typed::<InboundDoc>(&doc_path()) {
        Ok(Some(outcome)) => {
            if outcome.warning.is_some() {
                applog("[inbound] ledger recovered from backup");
            }
            serde_json::from_str(&outcome.payload).unwrap_or_default()
        }
        Ok(None) => InboundDoc::default(),
        Err(e) => {
            // Not a first run — but for a dedupe ledger the safe degradation
            // is "re-baseline everything", which an empty doc does.
            applog(&format!("[inbound] ledger unreadable ({}) — re-baselining", storage::err_code(&e)));
            InboundDoc::default()
        }
    }
}

fn persist(rt: &mut Runtime) {
    let payload = match serde_json::to_string(&rt.doc) {
        Ok(p) => p,
        Err(_) => return,
    };
    match storage::save_typed_ephemeral::<InboundDoc>(&doc_path(), &payload) {
        Ok(()) => rt.dirty = false,
        Err(e) => {
            rt.dirty = true;
            applog(&format!("[inbound] ledger save FAILED ({}) — will retry", storage::err_code(&e)));
        }
    }
}

/// Sources hand events here. Returns how many became pending. `live` events
/// bypass the baseline gate; polled events for a badge seen for the first
/// time are swallowed into the baseline.
pub(crate) fn offer(app: &AppHandle, cfg: &Config, events: Vec<Event>, live: bool) -> usize {
    let now = now_secs();
    let mut fresh = 0usize;
    let mut baselined = 0usize;
    with_rt(|rt| {
        let mut newly_baselined: Vec<(String, String)> = Vec::new();
        for ev in events {
            let Some(rule) = cfg.rule_for(&ev.source, &ev.badge) else { continue };
            if rt.doc.has(&ev.source, &ev.key, &ev.badge) {
                continue;
            }
            if rt
                .pending
                .iter()
                .any(|p| p.view.event.source == ev.source && p.view.event.key == ev.key && p.view.event.badge == ev.badge)
            {
                continue;
            }
            if !live && !rt.doc.is_baselined(&ev.source, &ev.badge) {
                rt.doc.mark(&ev.source, &ev.key, &ev.badge, now);
                newly_baselined.push((ev.source.clone(), ev.badge.clone()));
                baselined += 1;
                continue;
            }
            let id = rt.next_id;
            rt.next_id += 1;
            rt.pending.push(Pending {
                view: PendingView { id, event: ev, rule: rule.clone() },
                emitted_at: Instant::now(),
            });
            fresh += 1;
        }
        for (s, b) in newly_baselined {
            rt.doc.baseline(&s, &b);
        }
        if baselined > 0 {
            persist(rt);
        }
    });
    if fresh > 0 {
        applog(&format!("[inbound] {fresh} new item(s) pending"));
        let _ = app.emit("inbound-changed", ());
    }
    if baselined > 0 {
        applog(&format!("[inbound] baselined {baselined} existing item(s)"));
    }
    fresh
}

/// Polled badges with NO current matches still need their baseline recorded,
/// otherwise the first badge ever added would be swallowed as "existing".
pub(crate) fn note_baselined(source: &str, badges: &[String]) {
    with_rt(|rt| {
        let mut changed = false;
        for b in badges {
            if !rt.doc.is_baselined(source, b) {
                rt.doc.baseline(source, b);
                changed = true;
            }
        }
        if changed {
            persist(rt);
        }
    });
}

/* ---------- commands ---------- */

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InboundStatus {
    sources: Vec<SourceStatusView>,
    pending: usize,
    seen: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SourceStatusView {
    id: &'static str,
    live: bool,
    last_poll: Option<u64>,
    last_error: Option<&'static str>,
    secrets: Vec<SecretView>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretView {
    slot: &'static str,
    present: bool,
}

#[tauri::command]
pub(crate) fn inbound_status() -> InboundStatus {
    with_rt(|rt| {
        let mut sources = Vec::new();
        for id in SOURCES {
            let st = rt
                .statuses
                .iter()
                .find(|(s, _)| s == id)
                .map(|(_, st)| st.clone())
                .unwrap_or_default();
            let secrets = match *id {
                "slack" => vec![
                    SecretView { slot: "slack-user-token", present: keychain::has(keychain::Slot::SlackUserToken) },
                    SecretView { slot: "slack-app-token", present: keychain::has(keychain::Slot::SlackAppToken) },
                ],
                _ => Vec::new(),
            };
            sources.push(SourceStatusView {
                id,
                live: st.live,
                last_poll: st.last_poll,
                last_error: st.last_error,
                secrets,
            });
        }
        InboundStatus { sources, pending: rt.pending.len(), seen: rt.doc.seen.len() }
    })
}

#[tauri::command]
pub(crate) fn inbound_pending() -> Vec<PendingView> {
    with_rt(|rt| rt.pending.iter().map(|p| p.view.clone()).collect())
}

/// The webview has created the card (or decided it cannot). Both outcomes
/// retire the item for good: a badge the user must fix a rule for is
/// re-armed by removing and re-adding the badge, never by deck retrying.
#[tauri::command]
pub(crate) fn inbound_ack(id: u64, outcome: String) -> Result<(), String> {
    if !matches!(outcome.as_str(), "done" | "skipped") {
        return Err("outcome must be done or skipped".into());
    }
    with_rt(|rt| {
        let Some(pos) = rt.pending.iter().position(|p| p.view.id == id) else { return };
        let p = rt.pending.remove(pos);
        let ev = &p.view.event;
        rt.doc.mark(&ev.source, &ev.key, &ev.badge, now_secs());
        rt.doc.prune(now_secs());
        persist(rt);
        applog(&format!("[inbound] item {}", if outcome == "done" { "created" } else { "skipped" }));
    });
    Ok(())
}

/// Open the source's prefilled "create an app" page in the browser. The URL
/// is compiled into deck (never user input) and carries no credential.
#[tauri::command]
pub(crate) fn inbound_setup(source: String) -> Result<(), String> {
    let url = match source.as_str() {
        "slack" => crate::inbound_slack::setup_url(),
        _ => return Err("unknown source".into()),
    };
    let status = std::process::Command::new("open").arg(&url).status().map_err(|_| "could not open the browser".to_string())?;
    if !status.success() {
        return Err("could not open the browser".into());
    }
    applog("[inbound] setup page opened");
    Ok(())
}

/// Store a credential after proving it is the right kind and alive. The
/// error is a short sentence for the toast; the token never appears in it.
#[tauri::command]
pub(crate) fn inbound_set_secret(slot: String, value: String) -> Result<(), String> {
    let slot = keychain::Slot::parse(&slot).ok_or("unknown credential slot")?;
    let clearing = value.trim().is_empty();
    if !clearing {
        let trimmed = value.trim();
        if !keychain::accepts(slot, trimmed) {
            return Err("shape".into());
        }
        crate::inbound_slack::verify(slot, trimmed).map_err(|code| match code {
            "auth" => "auth".to_string(),
            "network" | "timeout" | "http" => "network".to_string(),
            _ => "slack".to_string(),
        })?;
    }
    keychain::set(slot, &value).map_err(|code| match code.as_str() {
        "shape" => "shape".to_string(),
        _ => "keychain".to_string(),
    })?;
    applog(&format!("[inbound] credential {}", if clearing { "cleared" } else { "stored" }));
    inbound_check_now();
    Ok(())
}

#[tauri::command]
pub(crate) fn inbound_check_now() {
    let (flag, cv) = wake();
    if let Ok(mut f) = flag.lock() {
        *f = true;
    }
    cv.notify_all();
}

/* ---------- the poller ---------- */

fn wait_for_tick(d: Duration) {
    let (flag, cv) = wake();
    let Ok(mut f) = flag.lock() else {
        std::thread::sleep(d);
        return;
    };
    let deadline = Instant::now() + d;
    while !*f {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let Ok((g, _)) = cv.wait_timeout(f, left) else { return };
        f = g;
    }
    *f = false;
}

pub(crate) fn spawn_inbound(app: AppHandle) {
    std::thread::spawn(move || {
        let mut sources: Vec<Box<dyn Source>> = vec![Box::new(crate::inbound_slack::Slack::default())];
        let mut failures = 0u32;
        wait_for_tick(Duration::from_secs(5));
        loop {
            let cfg = read_config();
            for src in sources.iter_mut() {
                let badges = cfg.badges(src.id());
                let wanted = src.enabled(&cfg) && !badges.is_empty();
                src.set_live(&app, wanted, &badges);
                if wanted {
                    match src.poll(&badges) {
                        Ok(events) => {
                            failures = 0;
                            offer(&app, &cfg, events, false);
                            note_baselined(src.id(), &badges);
                        }
                        Err(code) => {
                            failures = failures.saturating_add(1);
                            if failures <= 20 || failures % 120 == 0 {
                                applog(&format!("[inbound] poll FAILED ({code}) ×{failures}"));
                            }
                        }
                    }
                }
                let st = src.status();
                with_rt(|rt| {
                    rt.statuses.retain(|(id, _)| *id != src.id());
                    rt.statuses.push((src.id(), st));
                });
            }
            let re_emit = with_rt(|rt| {
                if rt.dirty {
                    persist(rt);
                }
                let stale = rt.pending.iter().any(|p| p.emitted_at.elapsed() >= RE_EMIT_AFTER);
                if stale {
                    for p in rt.pending.iter_mut() {
                        p.emitted_at = Instant::now();
                    }
                }
                stale
            });
            if re_emit {
                let _ = app.emit("inbound-changed", ());
            }
            let _ = with_rt(|rt| std::mem::replace(&mut rt.poll_wanted, false));
            wait_for_tick(POLL_INTERVAL);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(badge: &str) -> Value {
        json!({"id": format!("R-{badge}"), "source": "slack", "badge": badge,
               "projectId": "P1", "columnId": "C1", "cmd": "claude", "template": "triage"})
    }

    #[test]
    fn settings_shape_is_closed() {
        assert!(validate_settings(&json!({})).is_ok());
        assert!(validate_settings(&json!({"sources": {"slack": {"enabled": true}}, "rules": [rule("deck")]})).is_ok());
        assert!(validate_settings(&json!([])).is_err());
        assert!(validate_settings(&json!({"sources": {"notion": {}}})).is_err());
        assert!(validate_settings(&json!({"sources": {"slack": {"enabled": "yes"}}})).is_err());
        assert!(validate_settings(&json!({"rules": {}})).is_err());
        assert!(validate_settings(&json!({"rules": [{"id": "x"}]})).is_err());
        let mut bad = rule("deck");
        bad["badge"] = json!("Deck Badge");
        assert!(validate_settings(&json!({"rules": [bad]})).is_err());
        let mut bad = rule("deck");
        bad["cmd"] = json!("claude\nrm -rf");
        assert!(validate_settings(&json!({"rules": [bad]})).is_err());
        let mut bad = rule("deck");
        bad["dir"] = json!("/tmp\n/etc");
        assert!(validate_settings(&json!({"rules": [bad]})).is_err());
        let mut with_dir = rule("deck");
        with_dir["dir"] = json!("~/work/web");
        assert!(validate_settings(&json!({"rules": [with_dir]})).is_ok());
        let mut bad = rule("deck");
        bad["source"] = json!("notion");
        assert!(validate_settings(&json!({"rules": [bad]})).is_err());
        let mut bad = rule("deck");
        bad["projectId"] = json!("../x");
        assert!(validate_settings(&json!({"rules": [bad]})).is_err());
        assert!(validate_settings(&json!({"rules": [rule("deck"), rule("deck")]})).is_err());
        let mut dup = rule("bug");
        dup["id"] = json!("R-deck");
        assert!(validate_settings(&json!({"rules": [rule("deck"), dup]})).is_err());
        let many: Vec<Value> = (0..MAX_RULES + 1).map(|i| rule(&format!("b{i}"))).collect();
        assert!(validate_settings(&json!({"rules": many})).is_err());
    }

    #[test]
    fn badge_names_follow_slack_spelling() {
        for ok in ["deck", "white_check_mark", "+1", "bug-fix", "a1"] {
            assert!(valid_badge(ok), "{ok}");
        }
        for bad in ["", "Deck", ":deck:", "deck badge", "deck::skin-tone-2", &"a".repeat(65)] {
            assert!(!valid_badge(bad), "{bad}");
        }
    }

    #[test]
    fn config_reads_enabled_flag_and_rules_and_rejects_invalid_wholesale() {
        let v = json!({"sources": {"slack": {"enabled": true}}, "rules": [rule("deck"), rule("bug")]});
        let cfg = config_from_value(Some(&v));
        assert!(cfg.slack_enabled);
        assert_eq!(cfg.badges("slack"), vec!["bug".to_string(), "deck".to_string()]);
        assert_eq!(cfg.rule_for("slack", "bug").map(|r| r.id.as_str()), Some("R-bug"));
        assert!(cfg.rule_for("slack", "nope").is_none());
        assert_eq!(config_from_value(None), Config::default());
        let invalid = json!({"sources": {"slack": {"enabled": true}}, "rules": [rule("deck"), rule("deck")]});
        assert_eq!(config_from_value(Some(&invalid)), Config::default());
    }

    #[test]
    fn ledger_dedupes_baselines_and_prunes() {
        let mut d = InboundDoc::default();
        assert!(d.mark("slack", "C1/1.0", "deck", 100));
        assert!(!d.mark("slack", "C1/1.0", "deck", 101));
        assert!(d.mark("slack", "C1/1.0", "bug", 101));
        assert!(!d.is_baselined("slack", "deck"));
        d.baseline("slack", "deck");
        d.baseline("slack", "deck");
        assert!(d.is_baselined("slack", "deck"));
        assert_eq!(d.baselined.len(), 1);
        d.prune(100 + SEEN_TTL_SECS + 1);
        assert_eq!(d.seen.len(), 1, "only the older entry expired");
        for i in 0..(MAX_SEEN + 10) {
            d.mark("slack", &format!("k{i}"), "deck", 200);
        }
        d.prune(200);
        assert_eq!(d.seen.len(), MAX_SEEN);
    }

    #[test]
    fn ledger_document_round_trips_and_tolerates_missing_fields() {
        let d: InboundDoc = serde_json::from_str("{}").unwrap();
        assert_eq!(d, InboundDoc::default());
        let mut d = InboundDoc::default();
        d.mark("slack", "C/1", "deck", 5);
        d.baseline("slack", "deck");
        let s = serde_json::to_string(&d).unwrap();
        assert!(!s.contains("text"), "ledger carries no content fields");
        assert_eq!(serde_json::from_str::<InboundDoc>(&s).unwrap(), d);
    }
}
