//! quota.rs — usage windows per ACCOUNT, for presentation only.
//!
//! What it is: the latest observed usage windows (Claude Code's 5-hour /
//! 7-day `rate_limits`, Codex's rate limits, agy's quota buckets) of each
//! agent account the user's sessions run under, deduplicated by account,
//! so a person sees each account once (the sidebar's Quota area, a card's
//! chip), an orchestrator sees the accounts of its own cards
//! (`deck_worker_list` `quota`), and a client with the quota choice sees
//! every account observed this run, with or without a card under it
//! (`deck_quota_snapshot`, `mcp_snapshot`: redacted like `projection`,
//! with `sources` saying why an agent may be missing — the newest Codex
//! scan's word, `CodexScan`, and the statusLine files' state). Approved 2026-11-28
//! (`docs/design/quota-projection-20261128.md`).
//!
//! Sources, each read only for the windows and the account:
//! - Claude Code and agy: deck's statusLine entry (`quota/install.rs`), a
//!   tee in `deck-status-helper --quota` that passes the payload on to the
//!   user's own statusLine command and sends one `deck-quota` line to the
//!   status socket (`agent_status::dispatch` hands it here).
//! - Codex: the newest rollout of the Codex home Deck itself would use
//!   (`$CODEX_HOME` when Deck sees it, else `~/.codex`), its last
//!   `token_count` event's `rate_limits` (`quota/codex.rs`). Not a public
//!   interface: a format change shows as `unreadable`, never as an error.
//!
//! Account identity: Claude Code — the config folder the helper was started
//! under (its inherited `CLAUDE_CONFIG_DIR`, `~/.claude` when unset) and that
//! folder's `oauthAccount.accountUuid` (`~/.claude.json` for the default
//! folder, `<folder>/.claude.json` otherwise; ONE field read, no email, no
//! credential); agy — the `email` its payload carries; Codex — the
//! rollout's `session_meta.creator_account_id`. Without one the folder is
//! the key and the view says so (`byAccount: false`). Every raw identity
//! is hashed at once with a salt drawn per Deck run (`opaque`) and dropped:
//! views, MCP and app.log see `acct-…` only. Two folders of one account are
//! one entry; their observations are never added or averaged — the newest
//! one is the account's.
//!
//! Admission: the sender must be executing the bundled status helper
//! (`procinfo::runs_file`, kernel). A card is linked to an account only
//! when the line names a pane of this server and that pane's process is in
//! the helper's kernel ancestry, at most once a minute per pane (the
//! statusLine runs every few hundred milliseconds); `retain` drops a link
//! whose pane is gone or back at a shell. A Codex card (the pane's command
//! is `codex`, or its last agent word came from Codex) is linked to the
//! account of the Codex home Deck reads. A same-user
//! process could still feed a wrong number through the helper; that is a
//! presentation error only, because:
//!
//! **Nothing reads quota for a decision.** It is never a hold, a release,
//! readiness, authority, a schedule input or a notification; no card moves
//! or colours by it. Held in memory only (no ledger, no document version);
//! a restart starts empty until the next statusLine run or Codex scan. An
//! account stays once observed: a link ending (`retain`), a scan finding
//! nothing or a scan that does not parse never deletes or zeroes it. Its
//! id is stable within one Deck run only (the salt is drawn per run), so
//! nothing matches accounts across restarts. `observed_ms` is the
//! observation's own time — a statusLine line's arrival, a Codex event's
//! `timestamp` — never the time a file was read; a window past its reset
//! keeps its number and says `expired`.

pub(crate) mod codex;
pub(crate) mod install;

use crate::sync::LockRecover;
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

/// The `protocol` word of the helper's line.
pub(crate) const FAMILY: &str = "deck-quota";
/// An observation older than this is shown as stale.
const STALE_MS: u64 = 30 * 60 * 1000;
/// The least time between two pane attributions of one pane.
const ATTRIBUTE_EVERY_MS: u64 = 60 * 1000;
/// The least time between two Codex scans.
const CODEX_SCAN_EVERY_MS: u64 = 60 * 1000;
/// How long a folder's account read is reused.
const ACCOUNT_READ_EVERY_MS: u64 = 5 * 60 * 1000;
/// The most windows one line may carry (the helper's own cap).
const MAX_WINDOWS: usize = 8;
/// The largest `.claude.json` read for its account id.
const MAX_ACCOUNT_FILE: usize = 32 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Agent {
    Claude,
    Codex,
    Agy,
}

impl Agent {
    fn word(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Agy => "agy",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Source {
    Statusline,
    Rollout,
}

/// One usage window: its name, its length when known, the percent used and
/// when it resets (epoch seconds).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Window {
    pub(crate) name: String,
    pub(crate) minutes: Option<u32>,
    pub(crate) used: f64,
    pub(crate) resets_at: u64,
}

/// A window's length from its name when the source gives none.
pub(crate) fn minutes_of(name: &str) -> Option<u32> {
    if name.starts_with("five_hour") {
        Some(300)
    } else if name.starts_with("seven_day") || name.contains("weekly") {
        Some(10_080)
    } else if name.contains("daily") {
        Some(1_440)
    } else {
        None
    }
}

/// `[a-z0-9_-]{1,40}`.
fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// A plan word as a source names it: `[A-Za-z0-9 _-]{1,40}`.
pub(crate) fn plan_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'_' | b'-'))
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct WireWindow {
    name: String,
    used: f64,
    resets_at: u64,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    protocol: String,
    v: u32,
    source: String,
    windows: Vec<WireWindow>,
    #[serde(default)]
    root: Option<String>,
    #[serde(default)]
    account: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    socket: Option<String>,
    #[serde(default)]
    server_pid: Option<u32>,
    #[serde(default)]
    pane: Option<String>,
}

/// One admitted line, closed and checked.
#[derive(Debug, PartialEq)]
pub(crate) struct Line {
    kind: Agent,
    windows: Vec<Window>,
    root: Option<String>,
    account: Option<String>,
    plan: Option<String>,
    pane: Option<(u32, String)>,
}

/// Whether a status-socket line is this family's (a cheap test first).
pub(crate) fn is_family(line: &str) -> bool {
    line.contains(FAMILY)
        && serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .is_some_and(|v| v.get("protocol").and_then(|p| p.as_str()) == Some(FAMILY))
}

fn root_ok(s: &str) -> bool {
    s.starts_with('/')
        && s.len() <= 512
        && !s.split('/').any(|part| part == "..")
        && !s.bytes().any(|b| b < 0x20 || b == 0x7f)
}

fn email_ok(s: &str) -> bool {
    s.len() <= 254
        && s.bytes().filter(|b| *b == b'@').count() == 1
        && s.bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
}

/// The closed parse of one line; `socket` is this Deck's tmux socket name.
pub(crate) fn parse(line: &str, socket: &str) -> Result<Line, &'static str> {
    let wire: Wire = serde_json::from_str(line).map_err(|_| "quota-malformed")?;
    if wire.protocol != FAMILY || wire.v != 1 {
        return Err("quota-malformed");
    }
    let agent = match wire.source.as_str() {
        "claude-code" => Agent::Claude,
        "agy" => Agent::Agy,
        _ => return Err("quota-malformed"),
    };
    if wire.windows.is_empty() || wire.windows.len() > MAX_WINDOWS {
        return Err("quota-malformed");
    }
    let mut windows = Vec::new();
    for w in wire.windows {
        if !name_ok(&w.name) || !w.used.is_finite() || !(0.0..=100.0).contains(&w.used) {
            return Err("quota-malformed");
        }
        windows.push(Window {
            minutes: minutes_of(&w.name),
            name: w.name,
            used: w.used,
            resets_at: w.resets_at,
        });
    }
    let root = match wire.root {
        Some(root) if agent == Agent::Claude && root_ok(&root) => Some(root),
        Some(_) => return Err("quota-malformed"),
        None => None,
    };
    let account = match wire.account {
        Some(account) if agent == Agent::Agy && email_ok(&account) => Some(account),
        Some(_) => return Err("quota-malformed"),
        None => None,
    };
    let plan = wire.plan.filter(|p| plan_ok(p));
    let pane = match (wire.socket, wire.server_pid, wire.pane) {
        (Some(s), Some(pid), Some(pane)) if s == socket => Some((pid, pane)),
        _ => None,
    };
    Ok(Line {
        kind: agent,
        windows,
        root,
        account,
        plan,
        pane,
    })
}

/// One account as the store keeps it.
#[derive(Clone, Debug)]
struct Account {
    kind: Agent,
    by_account: bool,
    plan: Option<String>,
    folders: BTreeSet<String>,
    windows: Vec<Window>,
    observed_ms: u64,
    source: Source,
}

/// The pane a card shows, linked to one account.
#[derive(Clone, Debug, PartialEq)]
struct Link {
    account: String,
    session: String,
}

#[derive(Default)]
struct Store {
    salt: Vec<u8>,
    accounts: HashMap<String, Account>,
    links: HashMap<(u32, String), Link>,
    attributed: HashMap<(u32, String), u64>,
    /// folder → (account uuid, read at)
    folder_accounts: HashMap<String, (Option<String>, u64)>,
    codex_scanned_ms: u64,
    /// The default Codex home's account and whether its rollouts parsed.
    codex: Option<CodexState>,
    /// What the newest Codex scan found and when it ran (epoch ms): the
    /// one place a scan that found nothing is remembered.
    codex_scan: Option<(CodexScan, u64)>,
}

/// What the newest Codex scan found (`deck_quota_snapshot` `sources.codex`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CodexScan {
    /// A rollout in the bounded scan carried rate limits that parsed.
    Found,
    /// No rollout in the bounded scan carried any rate limits.
    NotFound,
    /// The newest rollout that seemed to carry them did not parse.
    Unreadable,
    /// Deck has no Codex home to read (no home folder).
    NoHome,
}

#[derive(Clone, Debug, PartialEq)]
enum CodexState {
    Account(String),
    Unreadable,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut guard = STORE.lock_or_recover();
    let store = guard.get_or_insert_with(|| Store {
        salt: crate::ledger::random(32).unwrap_or_else(|_| {
            // no secure random: a per-process value still keeps ids opaque
            // to anything outside this process
            format!("{:?}{}", std::time::Instant::now(), std::process::id()).into_bytes()
        }),
        ..Store::default()
    });
    f(store)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `acct-` and 10 hex of SHA-256(salt, agent, raw): the only form an
/// identity leaves this module in.
fn opaque(salt: &[u8], agent: Agent, raw: &str) -> String {
    let mut bytes = salt.to_vec();
    bytes.extend_from_slice(agent.word().as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(raw.as_bytes());
    format!("acct-{}", &crate::ledger::sha(&bytes)[..10])
}

/// The default Claude Code config folder and the file its account lives in.
fn claude_default(home: &std::path::Path) -> (String, std::path::PathBuf) {
    (
        home.join(".claude").to_string_lossy().into_owned(),
        home.join(".claude.json"),
    )
}

/// `oauthAccount.accountUuid` of one `.claude.json`; nothing else is kept.
pub(crate) fn claude_account_in(text: &[u8]) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(rename = "oauthAccount")]
        oauth_account: Option<Oauth>,
    }
    #[derive(serde::Deserialize)]
    struct Oauth {
        #[serde(rename = "accountUuid")]
        account_uuid: Option<String>,
    }
    let file: File = serde_json::from_slice(text).ok()?;
    file.oauth_account?.account_uuid.filter(|id| {
        !id.is_empty()
            && id.len() <= 64
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    })
}

fn read_claude_account(path: &std::path::Path) -> Option<String> {
    let bytes = crate::ledger::read_bounded(path, MAX_ACCOUNT_FILE, "claude account")
        .ok()
        .flatten()?;
    claude_account_in(&bytes)
}

/// Record one admitted line: its account's windows, and the pane link when
/// `pane_session` (already proven) names one. `read` reads a folder's
/// account file (injected for tests).
fn record(
    line: Line,
    home: &std::path::Path,
    pane_session: Option<String>,
    now: u64,
    read: impl Fn(&std::path::Path) -> Option<String>,
) -> String {
    with_store(|store| {
        let (folder, raw) = match line.kind {
            Agent::Claude => {
                let (default_folder, default_file) = claude_default(home);
                let (folder, file) = match &line.root {
                    Some(root) if root.trim_end_matches('/') != default_folder => (
                        root.trim_end_matches('/').to_string(),
                        std::path::Path::new(root).join(".claude.json"),
                    ),
                    _ => (default_folder, default_file),
                };
                let cached = store
                    .folder_accounts
                    .get(&folder)
                    .filter(|(_, at)| now.saturating_sub(*at) < ACCOUNT_READ_EVERY_MS)
                    .map(|(id, _)| id.clone());
                let id = cached.unwrap_or_else(|| {
                    let id = read(&file);
                    store
                        .folder_accounts
                        .insert(folder.clone(), (id.clone(), now));
                    id
                });
                let raw = match id {
                    Some(id) => (true, format!("uuid:{id}")),
                    None => (false, format!("folder:{folder}")),
                };
                (folder, raw)
            }
            Agent::Agy => {
                let folder = home.join(".gemini").to_string_lossy().into_owned();
                let raw = match &line.account {
                    Some(email) => (true, format!("email:{email}")),
                    None => (false, format!("folder:{folder}")),
                };
                (folder, raw)
            }
            Agent::Codex => unreachable!("Codex has no statusLine line"),
        };
        let (by_account, raw) = raw;
        let id = opaque(&store.salt, line.kind, &raw);
        let account = store.accounts.entry(id.clone()).or_insert_with(|| Account {
            kind: line.kind,
            by_account,
            plan: None,
            folders: BTreeSet::new(),
            windows: Vec::new(),
            observed_ms: 0,
            source: Source::Statusline,
        });
        account.folders.insert(folder);
        if now >= account.observed_ms {
            account.windows = line.windows;
            account.observed_ms = now;
            account.source = Source::Statusline;
            if line.plan.is_some() {
                account.plan = line.plan;
            }
        }
        if let (Some(key), Some(session)) = (line.pane, pane_session) {
            store.links.insert(
                key,
                Link {
                    account: id.clone(),
                    session,
                },
            );
        }
        id
    })
}

/// Whether a pane attribution is due for this pane now (and note it).
fn attribution_due(key: &(u32, String), now: u64) -> bool {
    with_store(|store| {
        let due = store
            .attributed
            .get(key)
            .is_none_or(|at| now.saturating_sub(*at) >= ATTRIBUTE_EVERY_MS);
        if due {
            store.attributed.insert(key.clone(), now);
        }
        due
    })
}

/// The session of the pane a proven line names: the pane of this server
/// whose process is in the helper's ancestry and whose foreground is not a
/// shell.
fn proven_session(
    rows: &[crate::tmux::PaneRow],
    (server_pid, pane): &(u32, String),
    chain: &[u32],
) -> Option<String> {
    rows.iter()
        .find(|row| row.server_pid == *server_pid && row.pane_id == *pane)
        .filter(|row| chain.contains(&row.pane_pid))
        .filter(|row| !crate::context::shell_process(Some(&row.command)))
        .filter(|row| crate::tmux::validate_session_name(&row.session_name).is_ok())
        .map(|row| row.session_name.clone())
}

/// One job for the quota worker: a parsed line and, when an attribution
/// is due, the helper's kernel ancestry read while it was connected.
struct Job {
    line: Line,
    chain: Option<Vec<u32>>,
}

static WORKER: Mutex<Option<std::sync::mpsc::SyncSender<Job>>> = Mutex::new(None);

fn worker() -> std::sync::mpsc::SyncSender<Job> {
    let mut guard = WORKER.lock_or_recover();
    if let Some(sender) = guard.as_ref() {
        return sender.clone();
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel::<Job>(64);
    let spawned = std::thread::Builder::new()
        .name("deck-quota".into())
        .spawn(move || {
            while let Ok(job) = receiver.recv() {
                let Some(home) = dirs::home_dir() else {
                    continue;
                };
                let session = match (&job.line.pane, &job.chain) {
                    (Some(key), Some(chain)) => crate::tmux::list_panes()
                        .ok()
                        .and_then(|rows| proven_session(&rows, key, chain)),
                    _ => None,
                };
                record(job.line, &home, session, now_ms(), read_claude_account);
            }
        });
    if spawned.is_ok() {
        *guard = Some(sender.clone());
    }
    sender
}

/// One `deck-quota` line from the status socket, while its sender is
/// still connected: the closed parse, the kernel check that the sender
/// executes the bundled status helper, and — when a pane link is due — its
/// ancestry; the rest happens on the quota worker. The stream is closed
/// before this returns (the helper's ACK).
pub(crate) fn hand(line: &str, stream: std::os::unix::net::UnixStream) -> Result<(), &'static str> {
    let line = parse(line, crate::tmux::socket())?;
    let peer = crate::procinfo::peer_pid(&stream).ok_or("quota-no-peer")?;
    let helper = crate::agent_status::bundled_status_helper().ok_or("quota-no-helper")?;
    if !crate::procinfo::runs_file(peer, &helper) {
        return Err("quota-foreign-sender");
    }
    let chain = line
        .pane
        .as_ref()
        .filter(|key| attribution_due(key, now_ms()))
        .map(|_| {
            crate::procinfo::ancestry_in(
                &crate::procinfo::processes(),
                peer,
                crate::agent_origin::ORIGIN_HOPS,
            )
        });
    drop(stream);
    worker()
        .try_send(Job { line, chain })
        .map_err(|_| "quota-busy")
}

/// The poll's pane listing: links of panes that are gone or back at a shell
/// end; a pane whose foreground is Codex is linked to the default Codex
/// home's account.
pub(crate) fn retain(rows: &[crate::tmux::PaneRow]) {
    retain_with(rows, &crate::agent_status::codex_panes());
}

/// `retain` with the panes whose agent word came from Codex.
fn retain_with(rows: &[crate::tmux::PaneRow], codex_panes: &[(u32, String)]) {
    with_store(|store| {
        store.links.retain(|(server_pid, pane), link| {
            rows.iter().any(|row| {
                row.server_pid == *server_pid
                    && row.pane_id == *pane
                    && row.session_name == link.session
                    && !crate::context::shell_process(Some(&row.command))
            })
        });
        let live: BTreeSet<(u32, String)> = rows
            .iter()
            .map(|row| (row.server_pid, row.pane_id.clone()))
            .collect();
        store.attributed.retain(|key, _| live.contains(key));
        if let Some(CodexState::Account(id)) = store.codex.clone() {
            for row in rows {
                let codex = row.command == "codex"
                    || codex_panes.contains(&(row.server_pid, row.pane_id.clone()));
                if codex && row.window_active && row.pane_active {
                    store.links.insert(
                        (row.server_pid, row.pane_id.clone()),
                        Link {
                            account: id.clone(),
                            session: row.session_name.clone(),
                        },
                    );
                }
            }
        }
    });
}

/// The Codex home Deck reads: `$CODEX_HOME` when Deck's own environment
/// has an absolute one, else `~/.codex` (the hooks file's rule).
fn codex_home() -> Option<std::path::PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".codex")))
}

/// Scan the Codex home when the last scan is older than a minute.
pub(crate) fn refresh_codex() {
    // tests never read the real Codex home: they record their own scans
    if cfg!(test) {
        return;
    }
    let now = now_ms();
    let due = with_store(|store| {
        let due = now.saturating_sub(store.codex_scanned_ms) >= CODEX_SCAN_EVERY_MS;
        if due {
            store.codex_scanned_ms = now;
        }
        due
    });
    if !due {
        return;
    }
    let Some(home) = codex_home() else {
        with_store(|store| store.codex_scan = Some((CodexScan::NoHome, now)));
        return;
    };
    record_codex(&home, codex::latest(&home.join("sessions")));
}

/// Record what a Codex scan found (`None`: no rollout carries any limits).
/// The accounts already observed stay: a scan that finds nothing or does
/// not parse never deletes or zeroes an earlier observation.
fn record_codex(home: &std::path::Path, found: Option<Result<codex::Rollout, ()>>) {
    let folder = home.to_string_lossy().into_owned();
    let scanned = now_ms();
    with_store(|store| match found {
        None => store.codex_scan = Some((CodexScan::NotFound, scanned)),
        Some(Err(())) => {
            store.codex = Some(CodexState::Unreadable);
            store.codex_scan = Some((CodexScan::Unreadable, scanned));
        }
        Some(Ok(rollout)) => {
            store.codex_scan = Some((CodexScan::Found, scanned));
            let (by_account, raw) = match &rollout.account {
                Some(id) => (true, format!("account:{id}")),
                None => (false, format!("folder:{folder}")),
            };
            let id = opaque(&store.salt, Agent::Codex, &raw);
            let account = store.accounts.entry(id.clone()).or_insert_with(|| Account {
                kind: Agent::Codex,
                by_account,
                plan: None,
                folders: BTreeSet::new(),
                windows: Vec::new(),
                observed_ms: 0,
                source: Source::Rollout,
            });
            account.folders.insert(folder.clone());
            if rollout.observed_ms >= account.observed_ms {
                account.windows = rollout.windows;
                account.observed_ms = rollout.observed_ms;
                account.source = Source::Rollout;
                account.plan = rollout.plan.or(account.plan.take());
            }
            store.codex = Some(CodexState::Account(id));
        }
    });
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WindowView {
    pub(crate) name: String,
    pub(crate) minutes: Option<u32>,
    pub(crate) used_percent: f64,
    /// epoch milliseconds
    pub(crate) resets_at: u64,
    pub(crate) expired: bool,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FolderView {
    pub(crate) name: String,
    pub(crate) path: String,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AccountView {
    pub(crate) id: String,
    pub(crate) kind: Agent,
    pub(crate) by_account: bool,
    pub(crate) plan: Option<String>,
    pub(crate) folders: Vec<FolderView>,
    pub(crate) windows: Vec<WindowView>,
    /// epoch milliseconds
    pub(crate) observed_at: u64,
    pub(crate) source: Source,
    pub(crate) stale: bool,
    /// tmux session names of the cards linked to this account
    pub(crate) sessions: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub(crate) accounts: Vec<AccountView>,
    /// `unreadable` when the newest Codex rollout did not parse.
    pub(crate) codex: Option<&'static str>,
}

fn folder_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map_or_else(|| path.to_string(), |n| n.to_string_lossy().into_owned())
}

/// The accounts as views, tightest window first within each agent.
pub(crate) fn snapshot_at(now: u64) -> Snapshot {
    with_store(|store| {
        let mut accounts: Vec<AccountView> = store
            .accounts
            .iter()
            .map(|(id, account)| {
                let mut sessions: Vec<String> = store
                    .links
                    .values()
                    .filter(|link| link.account == *id)
                    .map(|link| link.session.clone())
                    .collect();
                sessions.sort();
                sessions.dedup();
                AccountView {
                    id: id.clone(),
                    kind: account.kind,
                    by_account: account.by_account,
                    plan: account.plan.clone(),
                    folders: account
                        .folders
                        .iter()
                        .map(|path| FolderView {
                            name: folder_name(path),
                            path: path.clone(),
                        })
                        .collect(),
                    windows: account
                        .windows
                        .iter()
                        .map(|w| WindowView {
                            name: w.name.clone(),
                            minutes: w.minutes,
                            used_percent: w.used,
                            resets_at: w.resets_at.saturating_mul(1000),
                            expired: w.resets_at.saturating_mul(1000) <= now,
                        })
                        .collect(),
                    observed_at: account.observed_ms,
                    source: account.source,
                    stale: now.saturating_sub(account.observed_ms) > STALE_MS,
                    sessions,
                }
            })
            .collect();
        let tightest = |a: &AccountView| {
            a.windows
                .iter()
                .filter(|w| !w.expired)
                .map(|w| w.used_percent)
                .fold(0.0_f64, f64::max)
        };
        accounts.sort_by(|a, b| {
            a.kind
                .cmp(&b.kind)
                .then(tightest(b).total_cmp(&tightest(a)))
                .then(a.id.cmp(&b.id))
        });
        Snapshot {
            accounts,
            codex: matches!(store.codex, Some(CodexState::Unreadable)).then_some("unreadable"),
        }
    })
}

/// What the Quota area and a card's chip read (`quota_snapshot`).
pub(crate) fn snapshot() -> Snapshot {
    refresh_codex();
    snapshot_at(now_ms())
}

#[tauri::command]
pub(crate) async fn quota_snapshot() -> Result<Snapshot, crate::error::DeckError> {
    crate::session_runtime::off_ipc_thread(
        "quota_snapshot",
        std::time::Duration::from_secs(5),
        move || Ok(snapshot()),
    )
    .await
}

/// The accounts of the given sessions only, as an orchestrator sees them
/// (`deck_worker_list` `quota`): no folder paths, only the sessions asked
/// about.
pub(crate) fn projection(cards: &HashMap<String, String>) -> Vec<serde_json::Value> {
    refresh_codex();
    projection_at(cards, now_ms())
}

/// `cards`: session name → card id, the cards the caller may see.
fn projection_at(cards: &HashMap<String, String>, now: u64) -> Vec<serde_json::Value> {
    snapshot_at(now)
        .accounts
        .into_iter()
        .filter_map(|account| {
            let mut mine: Vec<&String> = account
                .sessions
                .iter()
                .filter_map(|s| cards.get(s))
                .collect();
            mine.sort();
            mine.dedup();
            if mine.is_empty() {
                return None;
            }
            Some(serde_json::json!({
                "account": account.id,
                "agent": account.kind,
                "byAccount": account.by_account,
                "label": account.folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                "plan": account.plan,
                "windows": account.windows.iter().map(|w| serde_json::json!({
                    "window": w.name,
                    "minutes": w.minutes,
                    "usedPercent": w.used_percent,
                    "resetsAt": w.resets_at,
                    "expired": w.expired,
                })).collect::<Vec<_>>(),
                "observedAt": account.observed_at,
                "stale": account.stale,
                "source": account.source,
                "cards": mine,
            }))
        })
        .collect()
}

/// The 7-day window an alert reads: of the account's windows of 10 080
/// minutes (Claude Code may report more than one, e.g. a model's own), the
/// one closest to its limit among those not yet reset, else the first.
fn seven_day(windows: &[WindowView]) -> Option<&WindowView> {
    let weekly = || windows.iter().filter(|w| w.minutes == Some(10_080));
    weekly()
        .filter(|w| !w.expired)
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
        .or_else(|| weekly().next())
}

/// What a statusLine source says in `deck_quota_snapshot` `sources`:
/// `observed` (an account of it was observed this run; each account's own
/// `stale` says how old), `waiting` (Deck's statusLine entry is on but no
/// line came since Deck started), `not-connected` (the entry is off, the
/// slot holds another command, or the agent never ran here), `unreadable`
/// (its settings file is not JSON).
fn statusline_source(observed: usize, state: install::State) -> serde_json::Value {
    let word = match state {
        _ if observed > 0 => "observed",
        install::State::On => "waiting",
        install::State::Unreadable => "unreadable",
        install::State::Off | install::State::User | install::State::Absent => "not-connected",
    };
    serde_json::json!({"state": word, "statusLine": state, "accounts": observed})
}

/// `deck_quota_snapshot` (`mcp/quota.rs`): every account observed in this
/// Deck run, whether or not a card runs under it now, after the throttled
/// Codex scan. `cards`: session name → card id of the cards the caller may
/// see (`None`: it may see none, and each account's `cards` is empty).
/// Redacted like `projection`: opaque ids, folder names, never a path.
pub(crate) fn mcp_snapshot(cards: Option<&HashMap<String, String>>) -> serde_json::Value {
    refresh_codex();
    #[cfg(not(test))]
    let (claude, agy) = install::status_now().states();
    // tests never read the real home: the statusLine states are theirs
    #[cfg(test)]
    let (claude, agy) = test_installs();
    mcp_snapshot_at(cards, claude, agy, now_ms())
}

fn mcp_snapshot_at(
    cards: Option<&HashMap<String, String>>,
    claude: install::State,
    agy: install::State,
    now: u64,
) -> serde_json::Value {
    let codex_scan = with_store(|store| store.codex_scan);
    let snapshot = snapshot_at(now);
    let count = |agent: Agent| snapshot.accounts.iter().filter(|a| a.kind == agent).count();
    let window = |w: &WindowView| {
        serde_json::json!({
            "window": w.name,
            "minutes": w.minutes,
            "usedPercent": w.used_percent,
            "resetsAt": w.resets_at,
            "expired": w.expired,
        })
    };
    let accounts: Vec<serde_json::Value> = snapshot
        .accounts
        .iter()
        .map(|account| {
            let mut mine: Vec<&String> = cards
                .map(|cards| {
                    account
                        .sessions
                        .iter()
                        .filter_map(|s| cards.get(s))
                        .collect()
                })
                .unwrap_or_default();
            mine.sort();
            mine.dedup();
            serde_json::json!({
                "account": account.id,
                "agent": account.kind,
                "byAccount": account.by_account,
                "label": account.folders.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
                "plan": account.plan,
                "source": account.source,
                "observedAt": account.observed_at,
                "ageMs": now.saturating_sub(account.observed_at),
                "stale": account.stale,
                "windows": account.windows.iter().map(window).collect::<Vec<_>>(),
                "sevenDay": seven_day(&account.windows).map(window),
                "cards": mine,
            })
        })
        .collect();
    let codex_state = codex_scan.map_or(serde_json::json!("not-scanned"), |(scan, _)| {
        serde_json::json!(scan)
    });
    serde_json::json!({
        "ok": true,
        "now": now,
        "staleAfterMs": STALE_MS,
        "accounts": accounts,
        "sources": {
            "codex": {
                "state": codex_state,
                "scannedAt": codex_scan.map(|(_, at)| at),
                "accounts": count(Agent::Codex),
            },
            "claude": statusline_source(count(Agent::Claude), claude),
            "agy": statusline_source(count(Agent::Agy), agy),
        },
        "cardScope": if cards.is_some() { "workers" } else { "none" },
        "accountIds": "this-run",
    })
}

#[cfg(test)]
static TEST_INSTALLS: Mutex<(install::State, install::State)> =
    Mutex::new((install::State::Absent, install::State::Absent));

#[cfg(test)]
fn test_installs() -> (install::State, install::State) {
    *TEST_INSTALLS.lock_or_recover()
}

/// Tests outside this module (`mcp/tests.rs`): the store's lock, a reset
/// store, and the statusLine states `mcp_snapshot` reports.
#[cfg(test)]
pub(crate) fn lock_for_tests(
    claude: install::State,
    agy: install::State,
) -> std::sync::MutexGuard<'static, ()> {
    let guard = tests::LOCK.lock_or_recover();
    reset_for_tests();
    *TEST_INSTALLS.lock_or_recover() = (claude, agy);
    guard
}

/// Tests outside this module: record a Codex scan of `sessions` as if it
/// were the default home `home`.
#[cfg(test)]
pub(crate) fn record_codex_for_tests(home: &std::path::Path, sessions: &std::path::Path) {
    record_codex(home, codex::latest(sessions));
}

/// Tests outside this module: the poll sees a Codex pane in each of
/// `sessions` (and no other pane), linking them to the default Codex home's
/// account.
#[cfg(test)]
pub(crate) fn link_codex_sessions_for_tests(sessions: &[&str]) {
    let rows: Vec<crate::tmux::PaneRow> = sessions
        .iter()
        .map(|session| crate::tmux::PaneRow {
            server_pid: 42,
            session_id: "$1".into(),
            session_name: (*session).into(),
            window_id: "@1".into(),
            pane_id: format!("%{session}"),
            pane_pid: 1,
            window_activity: 0,
            in_mode: false,
            window_active: true,
            pane_active: true,
            command: "codex".into(),
            tty: "/dev/ttys001".into(),
            path: "/tmp".into(),
        })
        .collect();
    retain_with(&rows, &[]);
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *STORE.lock_or_recover() = None;
}

#[cfg(test)]
mod tests;
