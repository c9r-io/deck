use super::*;
use std::path::Path;

/// The store is process-global: tests that use it take this lock.
static LOCK: Mutex<()> = Mutex::new(());

fn locked() -> std::sync::MutexGuard<'static, ()> {
    let guard = LOCK.lock_or_recover();
    reset_for_tests();
    guard
}

const SOCKET: &str = "deck-dev";

fn claude_line(root: Option<&str>, used: f64, pane: Option<&str>) -> String {
    let mut value = serde_json::json!({
        "protocol": FAMILY, "v": 1, "source": "claude-code",
        "windows": [{"name": "five_hour", "used": used, "resets_at": 4_000_000_000u64},
                    {"name": "seven_day", "used": 10.0, "resets_at": 4_000_000_000u64}],
    });
    if let Some(root) = root {
        value["root"] = root.into();
    }
    if let Some(pane) = pane {
        value["socket"] = SOCKET.into();
        value["server_pid"] = 42.into();
        value["pane"] = pane.into();
    }
    value.to_string()
}

fn row(pane: &str, pane_pid: u32, session: &str, command: &str) -> crate::tmux::PaneRow {
    crate::tmux::PaneRow {
        server_pid: 42,
        session_id: "$1".into(),
        session_name: session.into(),
        window_id: "@1".into(),
        pane_id: pane.into(),
        pane_pid,
        window_activity: 0,
        in_mode: false,
        window_active: true,
        pane_active: true,
        command: command.into(),
        tty: "/dev/ttys001".into(),
        path: "/tmp".into(),
    }
}

#[test]
fn the_closed_parse_takes_only_what_the_helper_sends() {
    assert!(is_family(&claude_line(None, 1.0, None)));
    assert!(!is_family("{\"v\":1,\"source\":\"claude-code\"}"));
    let line = parse(
        &claude_line(Some("/Users/u/.claude-work"), 62.0, Some("%3")),
        SOCKET,
    )
    .unwrap();
    assert_eq!(line.kind, Agent::Claude);
    assert_eq!(line.windows[0].minutes, Some(300));
    assert_eq!(line.windows[1].minutes, Some(10_080));
    assert_eq!(line.pane, Some((42, "%3".into())));
    // another Deck's socket: no pane claim
    let other = parse(&claude_line(None, 1.0, Some("%3")), "deck").unwrap();
    assert_eq!(other.pane, None);
    for bad in [
        claude_line(None, 101.0, None),
        claude_line(Some("relative"), 1.0, None),
        r#"{"protocol":"deck-quota","v":1,"source":"codex","windows":[{"name":"a","used":1,"resets_at":1}]}"#.into(),
        r#"{"protocol":"deck-quota","v":1,"source":"agy","windows":[]}"#.into(),
        r#"{"protocol":"deck-quota","v":1,"source":"agy","windows":[{"name":"a","used":1,"resets_at":1}],"extra":1}"#.into(),
        r#"{"protocol":"deck-quota","v":1,"source":"agy","root":"/x","windows":[{"name":"a","used":1,"resets_at":1}]}"#.into(),
    ] {
        assert_eq!(parse(&bad, SOCKET), Err("quota-malformed"), "{bad}");
    }
}

#[test]
fn two_folders_of_one_account_are_one_entry_with_the_newest_numbers() {
    let _guard = locked();
    let home = Path::new("/Users/u");
    let read = |path: &Path| -> Option<String> {
        match path.to_str()? {
            "/Users/u/.claude.json" => Some("aaaa-1".into()),
            "/Users/u/.claude-twin/.claude.json" => Some("aaaa-1".into()),
            "/Users/u/.claude-work/.claude.json" => Some("bbbb-2".into()),
            _ => None,
        }
    };
    let a = record(
        parse(&claude_line(None, 20.0, None), SOCKET).unwrap(),
        home,
        None,
        1_000,
        read,
    );
    let b = record(
        parse(
            &claude_line(Some("/Users/u/.claude-twin"), 30.0, None),
            SOCKET,
        )
        .unwrap(),
        home,
        None,
        2_000,
        read,
    );
    let c = record(
        parse(
            &claude_line(Some("/Users/u/.claude-work"), 70.0, None),
            SOCKET,
        )
        .unwrap(),
        home,
        None,
        3_000,
        read,
    );
    assert_eq!(a, b, "one account behind two folders");
    assert_ne!(a, c);
    assert!(a.starts_with("acct-") && !a.contains("aaaa"));
    let snapshot = snapshot_at(4_000);
    assert_eq!(snapshot.accounts.len(), 2);
    let first = snapshot.accounts.iter().find(|x| x.id == a).unwrap();
    assert_eq!(
        first.windows[0].used_percent, 30.0,
        "the newest observation, never a sum"
    );
    assert_eq!(first.folders.len(), 2);
    assert!(first.by_account);
    // the tightest account first
    assert_eq!(snapshot.accounts[0].id, c);
}

#[test]
fn without_an_account_id_the_folder_is_the_key_and_says_so() {
    let _guard = locked();
    let id = record(
        parse(
            &claude_line(Some("/Users/u/.claude-api"), 5.0, None),
            SOCKET,
        )
        .unwrap(),
        Path::new("/Users/u"),
        None,
        1,
        |_| None,
    );
    let view = snapshot_at(2)
        .accounts
        .into_iter()
        .find(|x| x.id == id)
        .unwrap();
    assert!(!view.by_account);
    assert_eq!(view.folders[0].name, ".claude-api");
}

#[test]
fn agy_is_keyed_by_its_email_which_never_leaves() {
    let _guard = locked();
    let line = r#"{"protocol":"deck-quota","v":1,"source":"agy","account":"dev@example.com","plan":"Pro","windows":[{"name":"gemini-weekly","used":25,"resets_at":4000000000}]}"#;
    let id = record(
        parse(line, SOCKET).unwrap(),
        Path::new("/Users/u"),
        None,
        1,
        |_| None,
    );
    let snapshot = snapshot_at(2);
    let text = serde_json::to_string(&snapshot).unwrap();
    assert!(!text.contains("example.com"));
    let view = &snapshot.accounts[0];
    assert_eq!(view.id, id);
    assert_eq!(view.plan.as_deref(), Some("Pro"));
    assert_eq!(view.windows[0].minutes, Some(10_080));
}

#[test]
fn stale_and_expired_are_shown_never_guessed() {
    let _guard = locked();
    let line = r#"{"protocol":"deck-quota","v":1,"source":"claude-code","windows":[{"name":"five_hour","used":90,"resets_at":100}]}"#;
    record(
        parse(line, SOCKET).unwrap(),
        Path::new("/Users/u"),
        None,
        50_000,
        |_| None,
    );
    let view = &snapshot_at(50_000 + STALE_MS + 1).accounts[0];
    assert!(view.stale);
    assert!(view.windows[0].expired);
    assert_eq!(
        view.windows[0].used_percent, 90.0,
        "the number stays as observed"
    );
}

#[test]
fn a_pane_link_needs_the_pane_in_the_helpers_ancestry_and_ends_with_the_agent() {
    let _guard = locked();
    let rows = vec![
        row("%3", 500, "deck-card-1", "claude"),
        row("%4", 600, "deck-card-2", "zsh"),
    ];
    let key = (42, "%3".to_string());
    assert_eq!(
        proven_session(&rows, &key, &[900, 700, 500, 1]),
        Some("deck-card-1".into())
    );
    assert_eq!(
        proven_session(&rows, &key, &[900, 700, 1]),
        None,
        "a foreign pane"
    );
    assert_eq!(
        proven_session(&rows, &(42, "%4".into()), &[600]),
        None,
        "a shell foreground"
    );
    let id = record(
        parse(&claude_line(None, 1.0, Some("%3")), SOCKET).unwrap(),
        Path::new("/Users/u"),
        Some("deck-card-1".into()),
        1,
        |_| None,
    );
    assert_eq!(
        snapshot_at(2).accounts[0].sessions,
        vec!["deck-card-1".to_string()]
    );
    assert!(attribution_due(&key, 10));
    assert!(!attribution_due(&key, 20), "at most once a minute");
    retain_with(&[row("%3", 500, "deck-card-1", "zsh")], &[]);
    assert!(
        snapshot_at(3).accounts[0].sessions.is_empty(),
        "the agent left the pane"
    );
    let projection_input: HashMap<String, String> =
        [("deck-card-1".to_string(), "card-1".to_string())].into();
    record(
        parse(&claude_line(None, 1.0, Some("%3")), SOCKET).unwrap(),
        Path::new("/Users/u"),
        Some("deck-card-1".into()),
        4,
        |_| None,
    );
    let seen = projection_at(&projection_input, 5);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0]["account"], id);
    assert_eq!(seen[0]["cards"], serde_json::json!(["card-1"]));
    assert!(
        seen[0].get("folders").is_none(),
        "no folder path toward MCP"
    );
    let other: HashMap<String, String> = [("other".to_string(), "c".to_string())].into();
    assert!(projection_at(&other, 5).is_empty());
}

fn rollout(dir: &Path, day: &str, name: &str, lines: &[&str]) {
    let folder = dir.join(day);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(name), lines.join("\n") + "\n").unwrap();
}

const META: &str = r#"{"timestamp":"2026-11-27T20:00:00.000Z","type":"session_meta","payload":{"id":"x","cwd":"/secret","creator_account_id":"user-abc_1","account_scoped":true}}"#;
const COUNT: &str = r#"{"timestamp":"2026-11-27T23:05:50.866Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":{"used_percent":53.0,"window_minutes":10080,"resets_at":1796342926},"secondary":{"used_percent":12.5,"window_minutes":300,"resets_at":1796000000},"credits":null,"plan_type":"prolite"}}}"#;

#[test]
fn codex_rollouts_give_the_last_limits_and_the_account() {
    let dir = crate::test_dir::TestDir::new("quota-codex");
    let sessions: &Path = &dir;
    assert_eq!(codex::latest(sessions), None);
    rollout(
        sessions,
        "2026/11/27",
        "rollout-a.jsonl",
        &[
            META,
            COUNT,
            r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#,
        ],
    );
    let found = codex::latest(sessions).unwrap().unwrap();
    assert_eq!(found.account.as_deref(), Some("user-abc_1"));
    assert_eq!(found.plan.as_deref(), Some("prolite"));
    assert_eq!(found.windows[0].name, "seven_day");
    assert_eq!(found.windows[1].minutes, Some(300));
    assert_eq!(
        found.observed_ms,
        codex::iso_ms("2026-11-27T23:05:50.866Z").unwrap()
    );
    assert_eq!(codex::iso_ms("1970-01-01T00:00:01.5Z"), Some(1500));
    // a newer file whose limits do not parse
    rollout(
        sessions,
        "2026/11/28",
        "rollout-b.jsonl",
        &[
            META,
            r#"{"timestamp":"2026-11-28T01:00:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":"high"}}}}"#,
        ],
    );
    assert!(
        codex::latest(sessions).unwrap().is_ok(),
        "a parsed older file still answers"
    );
}

#[test]
fn a_codex_format_change_shows_as_unreadable() {
    let _guard = locked();
    let dir = crate::test_dir::TestDir::new("quota-codex-bad");
    let sessions: &Path = &dir;
    rollout(
        sessions,
        "2026/11/28",
        "rollout-b.jsonl",
        &[
            META,
            r#"{"timestamp":"2026-11-28T01:00:00.000Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":"high"}}}}"#,
        ],
    );
    let found = codex::latest(sessions);
    assert_eq!(found, Some(Err(())));
    record_codex(Path::new("/Users/u/.codex"), found);
    assert_eq!(snapshot_at(1).codex, Some("unreadable"));
}

#[test]
fn a_codex_pane_is_linked_to_the_default_homes_account() {
    let _guard = locked();
    let dir = crate::test_dir::TestDir::new("quota-codex-link");
    let sessions: &Path = &dir;
    rollout(sessions, "2026/11/27", "rollout-a.jsonl", &[META, COUNT]);
    record_codex(Path::new("/Users/u/.codex"), codex::latest(sessions));
    retain_with(&[row("%5", 700, "deck-card-9", "codex")], &[]);
    retain_with(
        &[
            row("%5", 700, "deck-card-9", "codex"),
            row("%6", 701, "deck-card-10", "node"),
        ],
        &[(42, "%6".into())],
    );
    let snapshot = snapshot_at(1);
    let view = &snapshot.accounts[0];
    assert_eq!(view.kind, Agent::Codex);
    assert_eq!(view.source, Source::Rollout);
    assert_eq!(
        view.sessions,
        vec!["deck-card-10".to_string(), "deck-card-9".to_string()]
    );
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("user-abc"));
}

#[test]
fn a_claude_account_file_gives_up_only_its_uuid() {
    assert_eq!(
        claude_account_in(
            br#"{"oauthAccount":{"accountUuid":"9f1c-22","emailAddress":"x@y"},"projects":{}}"#
        ),
        Some("9f1c-22".into())
    );
    assert_eq!(
        claude_account_in(br#"{"oauthAccount":{"accountUuid":"bad id"}}"#),
        None
    );
    assert_eq!(claude_account_in(b"{}"), None);
    assert_eq!(claude_account_in(b"garbage"), None);
}
