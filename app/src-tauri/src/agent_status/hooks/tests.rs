use super::*;

/// The isolated bundle basename must preserve production ownership, not
/// merely produce entries that the installer can write successfully.
#[test]
fn isolated_project_lifecycle_hooks_round_trip_through_production_ownership() {
    let helper = "/private/tmp/deck-reentry-ab12/deck.app/Contents/MacOS/deck-lifecycle-helper";
    let edit = HookEdit::Install {
        specs: CLAUDE_LIFECYCLE_HOOKS,
        source: CLAUDE_CODE.source,
        style: HookStyle::ExecBrief,
        helper,
    };
    let installed = hooks_file_edited_of(Family::Lifecycle, Some("{}\n"), edit)
        .unwrap()
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&installed).unwrap();
    assert!(hooks_installed_of(
        Family::Lifecycle,
        &value,
        CLAUDE_LIFECYCLE_HOOKS
    ));
    assert!(hooks_are_current_of(
        Family::Lifecycle,
        &value,
        CLAUDE_LIFECYCLE_HOOKS,
        CLAUDE_CODE.source,
        HookStyle::ExecBrief,
        helper
    ));
    assert!(
        hooks_file_edited_of(Family::Lifecycle, Some(&installed), edit)
            .unwrap()
            .is_none()
    );
    let removed = hooks_file_edited_of(Family::Lifecycle, Some(&installed), HookEdit::Uninstall)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&removed).unwrap(),
        serde_json::json!({})
    );
    let wrong = installed.replace("/deck.app/", "/deck-reentry.app/");
    assert!(!hooks_installed_of(
        Family::Lifecycle,
        &serde_json::from_str::<serde_json::Value>(&wrong).unwrap(),
        CLAUDE_LIFECYCLE_HOOKS
    ));
}

/// Not a test of its own: prints, for review, a settings document that
/// holds the Lifecycle family's two hooks for the helper file named in
/// `DECK_M1_HELPER`. Each entry is built by `spec_entry`, the function
/// an install writes its entries with, in the lifecycle style; only the
/// outer `hooks` object is assembled here. Nothing is installed and no
/// settings file is read or written: `agent_lifecycle_hooks_set` and its
/// release-location rule are not involved. Standard error says whether
/// this build would recognise those hooks as its own install — a helper
/// outside a `deck.app` bundle is not, so such a file can never satisfy
/// `lifecycle_observation_installed`.
#[test]
#[ignore = "run only to generate the native certification's project hook file"]
fn lifecycle_hook_file_for_review() {
    let Ok(helper) = std::env::var("DECK_M1_HELPER") else {
        return;
    };
    let mut hooks = serde_json::Map::new();
    for spec in CLAUDE_LIFECYCLE_HOOKS {
        hooks.insert(
            spec.0.to_string(),
            serde_json::json!([spec_entry(
                HookStyle::ExecBrief,
                &helper,
                CLAUDE_CODE.source,
                spec
            )]),
        );
    }
    let document = serde_json::json!({ "hooks": hooks });
    eprintln!(
        "recognised-as-this-builds-install={}",
        hooks_installed_of(Family::Lifecycle, &document, CLAUDE_LIFECYCLE_HOOKS)
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&document).expect("a JSON document")
    );
}

const HELPER: &str = "/Applications/deck.app/Contents/MacOS/deck-status-helper";

/// Protocol v2 changes nothing in the installed hook commands: the
/// helper reads the payload it already received on stdin, so there is
/// no hook migration. The exact pre-v2 command shapes, pinned.
#[test]
fn hook_commands_are_unchanged_by_protocol_v2() {
    assert_eq!(
        hook_value(HookStyle::Exec, HELPER, "claude-code", "turn-done"),
        serde_json::json!({"type": "command", "command": HELPER, "args": ["claude-code", "turn-done"], "timeout": 10})
    );
    assert_eq!(
        hook_value(HookStyle::ShellAsync, HELPER, "codex", "turn-done"),
        serde_json::json!({"type": "command", "command": format!("\"{HELPER}\" codex turn-done"), "timeout": 10, "async": true})
    );
}

#[test]
fn hook_status_command_returns_both_closed_agent_fields() {
    let status = serde_json::to_value(agent_hooks_status()).unwrap();
    let object = status.as_object().unwrap();
    // the two Signal toggles, the separate re-entry observation opt-in
    // (the Lifecycle family), and Pi's two switches with the closed
    // state of its one settings entry: five booleans and one word
    assert_eq!(object.len(), 6);
    assert!(object.get("pi").is_some_and(serde_json::Value::is_boolean));
    assert!(object
        .get("pi_lifecycle")
        .is_some_and(serde_json::Value::is_boolean));
    let installation = object
        .get("pi_installation")
        .and_then(serde_json::Value::as_str)
        .expect("a closed word");
    assert!(
        [
            "absent",
            "current",
            "needs-reload",
            "conflict",
            "unreadable",
            "unsupported-location"
        ]
        .contains(&installation),
        "{installation}"
    );
    // a test binary is no stable install: it may read, never write
    assert_eq!(installation, "unsupported-location");
    assert_eq!(
        agent_hooks_set(crate::agent_pi::SOURCE.into(), true)
            .unwrap_err()
            .message(),
        "pi-config-unsupported-location"
    );
    // the session-observation switch is Pi's own command, under the same rule
    for enable in [true, false] {
        assert_eq!(
            crate::agent_pi::config::agent_pi_lifecycle_set(enable)
                .unwrap_err()
                .message(),
            "pi-config-unsupported-location"
        );
    }
    assert_eq!(
        agent_hooks_set(crate::agent_pi::SOURCE.into(), false)
            .unwrap_err()
            .message(),
        "pi-config-unsupported-location"
    );
    assert!(object
        .get("claude_reentry")
        .is_some_and(serde_json::Value::is_boolean));
    assert!(object
        .get("claude")
        .is_some_and(serde_json::Value::is_boolean));
    assert!(object
        .get("codex")
        .is_some_and(serde_json::Value::is_boolean));
}

#[test]
fn codex_hooks_install_covers_all_events_async_and_coexists() {
    // a user hooks.json with its own Stop hook and a description key
    let user = serde_json::json!({
        "description": "my hooks",
        "hooks": {
            "Stop": [
                { "hooks": [{ "type": "command", "command": "terminal-notifier -message done" }] }
            ]
        }
    });
    let installed = hooks_with_install(
        user.clone(),
        CODEX_HOOKS,
        "codex",
        HookStyle::ShellAsync,
        HELPER,
    )
    .unwrap();
    assert!(hooks_installed(&installed, CODEX_HOOKS));
    assert_eq!(installed["description"], "my hooks");
    // the user's own Stop hook coexists with ours — no notify-style conflict
    assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 2);
    let permission = &installed["hooks"]["PermissionRequest"][0]["hooks"][0];
    assert_eq!(
        permission["command"].as_str().unwrap(),
        "\"/Applications/deck.app/Contents/MacOS/deck-status-helper\" codex needs-input"
    );
    // fire-and-forget: every deck entry is async so it can never block a turn
    assert_eq!(permission["async"], serde_json::json!(true));
    assert_eq!(
        installed["hooks"]["Interrupt"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap(),
        "\"/Applications/deck.app/Contents/MacOS/deck-status-helper\" codex turn-done"
    );

    // idempotent install, exact uninstall
    let twice = hooks_with_install(
        installed,
        CODEX_HOOKS,
        "codex",
        HookStyle::ShellAsync,
        HELPER,
    )
    .unwrap();
    assert_eq!(twice["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(hooks_with_uninstall(twice), user);

    // claude's spec set does not read as installed for codex and vice versa
    let claude_only = hooks_with_install(
        serde_json::json!({}),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert!(!hooks_installed(&claude_only, CODEX_HOOKS));
}

#[test]
fn hook_install_is_idempotent_and_preserves_foreign_config() {
    let user = serde_json::json!({
        "model": "opus",
        "hooks": {
            "Stop": [
                { "hooks": [{ "type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff" }] }
            ],
            "PreToolUse": [
                { "matcher": "Bash", "hooks": [{ "type": "command", "command": "my-guard" }] }
            ]
        }
    });
    let installed = hooks_with_install(
        user.clone(),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert!(hooks_installed(&installed, CLAUDE_HOOKS));
    assert_eq!(installed["model"], "opus");
    // the user's own Stop hook and PreToolUse guard survive
    assert_eq!(installed["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert_eq!(
        installed["hooks"]["PreToolUse"].as_array().unwrap().len(),
        1
    );
    // exec form: the helper path alone in `command`, words in `args`,
    // so Claude Code spawns the helper without a shell
    let submit = &installed["hooks"]["UserPromptSubmit"][0]["hooks"][0];
    assert_eq!(submit["command"], HELPER);
    assert_eq!(
        submit["args"],
        serde_json::json!(["claude-code", "working"])
    );
    assert_eq!(submit["type"], "command");
    assert!(submit.get("async").is_none());
    // only a mid-turn permission question is "needs-input"; an idle
    // prompt after a finished turn stays turn-done
    assert_eq!(
        installed["hooks"]["Notification"][0]["matcher"],
        "permission_prompt"
    );

    // installing twice does not duplicate
    let twice = hooks_with_install(
        installed.clone(),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert_eq!(twice["hooks"]["Stop"].as_array().unwrap().len(), 2);

    // uninstall restores the user's document exactly
    let removed = hooks_with_uninstall(twice);
    assert!(!hooks_installed(&removed, CLAUDE_HOOKS));
    assert_eq!(removed, user);

    // uninstalling a never-installed file is a no-op
    let empty = hooks_with_uninstall(serde_json::json!({ "model": "opus" }));
    assert_eq!(empty, serde_json::json!({ "model": "opus" }));

    // a hooks-only file empties back to no hooks key at all
    let only_ours = hooks_with_install(
        serde_json::json!({}),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert_eq!(hooks_with_uninstall(only_ours), serde_json::json!({}));
}

/// The agent's settings file belongs to the agent CLI and to whoever
/// manages it. A symlinked one (a dotfiles checkout) stays a link and its
/// TARGET gets the bytes and keeps its mode; a link to nothing and a file
/// deck may not write are refused and left exactly as they were; and no
/// temp file is ever left beside any of them.
#[test]
fn the_config_write_goes_through_a_link_and_refuses_what_it_may_not_write() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let root = std::env::temp_dir().join(format!("deck-agentcfg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (home, repo) = (root.join("claude"), root.join("dotfiles"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    let names = |dir: &Path| {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let chmod = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
    };
    let is_link = |path: &Path| {
        std::fs::symlink_metadata(path)
            .unwrap()
            .file_type()
            .is_symlink()
    };
    let read = |path: &Path| std::fs::read_to_string(path).unwrap();

    // a link into another directory
    let target = repo.join("settings.json");
    std::fs::write(&target, "old").unwrap();
    chmod(&target, 0o644);
    let link = home.join("settings.json");
    symlink(&target, &link).unwrap();
    write_agent_config(&link, b"new", "never ran").unwrap();
    assert!(is_link(&link), "the link is still a link");
    assert_eq!(read(&target), "new", "its target got the bytes");
    assert_eq!(mode(&target), 0o644, "and kept its mode");

    // a relative link to that link
    let chained = home.join("chained.json");
    symlink("settings.json", &chained).unwrap();
    write_agent_config(&chained, b"newer", "never ran").unwrap();
    assert!(is_link(&chained) && is_link(&link));
    assert_eq!(read(&target), "newer");

    // a plain file keeps its mode; a file deck creates is private
    let plain = home.join("plain.json");
    std::fs::write(&plain, "old").unwrap();
    chmod(&plain, 0o640);
    write_agent_config(&plain, b"new", "never ran").unwrap();
    assert_eq!((read(&plain).as_str(), mode(&plain)), ("new", 0o640));
    let fresh = home.join("fresh.json");
    write_agent_config(&fresh, b"new", "never ran").unwrap();
    assert_eq!((read(&fresh).as_str(), mode(&fresh)), ("new", 0o600));

    // a link to nothing: refused; it stays a link and nothing is created
    let dangling = home.join("dangling.json");
    symlink(repo.join("missing.json"), &dangling).unwrap();
    assert!(write_agent_config(&dangling, b"new", "never ran").is_err());
    assert!(
        is_link(&dangling),
        "a dangling link is not replaced by a file"
    );
    assert!(!repo.join("missing.json").exists());

    // a file deck may not write: refused, although a rename would pass
    let readonly = home.join("readonly.json");
    std::fs::write(&readonly, "old").unwrap();
    chmod(&readonly, 0o444);
    assert!(write_agent_config(&readonly, b"new", "never ran").is_err());
    assert_eq!((read(&readonly).as_str(), mode(&readonly)), ("old", 0o444));

    // a directory deck may not write into: refused
    let locked = root.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("settings.json"), "old").unwrap();
    chmod(&locked, 0o500);
    let refused = write_agent_config(&locked.join("settings.json"), b"new", "never ran");
    chmod(&locked, 0o700);
    assert!(refused.is_err());
    assert_eq!(read(&locked.join("settings.json")), "old");
    assert_eq!(names(&locked), ["settings.json"]);

    // the replace itself fails (the path names a directory): the temp
    // file it had written is removed
    let occupied = home.join("occupied.json");
    std::fs::create_dir_all(occupied.join("inside")).unwrap();
    assert!(write_agent_config(&occupied, b"new", "never ran").is_err());
    assert!(occupied.join("inside").is_dir());

    // the agent never ran here: its directory is not created
    let absent = root.join("absent").join("settings.json");
    let error = write_agent_config(&absent, b"new", "never ran").unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Missing);
    assert_eq!(
        error.message(),
        "never ran",
        "said as the agent's own sentence"
    );
    assert!(!root.join("absent").exists());

    // nothing but the files themselves, beside the links and the target
    assert_eq!(
        names(&home),
        [
            "chained.json",
            "dangling.json",
            "fresh.json",
            "occupied.json",
            "plain.json",
            "readonly.json",
            "settings.json"
        ]
    );
    assert_eq!(names(&repo), ["settings.json"]);
    let _ = std::fs::remove_dir_all(&root);
}

/// A hook the user keeps in the SAME group as deck's is theirs. It does
/// not make deck's hook stale (so the boot migration leaves the file
/// alone), and install and uninstall take deck's hook out from beside it:
/// their hook stays in its group, under that group's matcher.
#[test]
fn a_foreign_hook_in_decks_own_group_is_never_removed() {
    let install = |doc: serde_json::Value| {
        hooks_with_install(doc, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER).unwrap()
    };
    let is_current = |doc: &serde_json::Value| {
        hooks_are_current(doc, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER)
    };
    let theirs = serde_json::json!({ "type": "command", "command": "say done" });
    let guard = serde_json::json!({ "type": "command", "command": "my-notifier" });
    let mut mixed = install(serde_json::json!({ "model": "opus" }));
    mixed["hooks"]["Stop"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(theirs.clone());
    mixed["hooks"]["Notification"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .insert(0, guard.clone());
    let their_stop = serde_json::json!({ "hooks": [theirs] });
    let their_notification =
        serde_json::json!({ "matcher": "permission_prompt", "hooks": [guard] });

    assert!(hooks_installed(&mixed, CLAUDE_HOOKS));
    assert!(
        is_current(&mixed),
        "a user's hook beside deck's does not make deck's stale"
    );

    // an install keeps both groups, each without deck's hook, and gives
    // deck a group of its own again
    let reinstalled = install(mixed.clone());
    assert_eq!(reinstalled["hooks"]["Stop"][0], their_stop);
    assert_eq!(reinstalled["hooks"]["Notification"][0], their_notification);
    assert_eq!(reinstalled["hooks"]["Stop"].as_array().unwrap().len(), 2);
    assert!(is_current(&reinstalled));
    assert_eq!(install(reinstalled.clone()), reinstalled);

    // an uninstall leaves exactly their hooks
    let removed = hooks_with_uninstall(mixed.clone());
    assert!(!hooks_installed(&removed, CLAUDE_HOOKS));
    assert_eq!(
        removed,
        serde_json::json!({ "model": "opus", "hooks": {
                "Stop": [their_stop],
                "Notification": [their_notification]
            } })
    );
    assert_eq!(hooks_with_uninstall(reinstalled), removed);

    // deck's hook in a shared group is held to the spec like any other:
    // a moved helper or a widened matcher is stale, and the repair keeps
    // the user's hook
    let mut moved = mixed.clone();
    moved["hooks"]["Stop"][0]["hooks"][0]["command"] =
        serde_json::json!("/Users/x/Applications/deck.app/Contents/MacOS/deck-status-helper");
    assert!(!is_current(&moved));
    let repaired = install(moved);
    assert!(is_current(&repaired));
    assert_eq!(repaired["hooks"]["Stop"][0], their_stop);
    let mut wide = mixed.clone();
    wide["hooks"]["Notification"][0]["matcher"] =
        serde_json::json!("permission_prompt|idle_prompt");
    assert!(!is_current(&wide));
}

/// Uninstall prunes what taking deck's hooks out emptied, and nothing
/// the user wrote empty.
#[test]
fn uninstall_leaves_what_the_user_wrote_empty() {
    let user = serde_json::json!({ "hooks": { "PreToolUse": [], "Stop": [{ "hooks": [] }] } });
    assert_eq!(hooks_with_uninstall(user.clone()), user);
    let bare = serde_json::json!({ "hooks": {} });
    assert_eq!(hooks_with_uninstall(bare.clone()), bare);
    let installed = hooks_with_install(
        user.clone(),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert!(hooks_installed(&installed, CLAUDE_HOOKS));
    assert_eq!(hooks_with_uninstall(installed), user);
}

const CLAUDE_INSTALL: HookEdit<'static> = HookEdit::Install {
    specs: CLAUDE_HOOKS,
    source: "claude-code",
    style: HookStyle::Exec,
    helper: HELPER,
};

/// The text a file gets from an edit that changes it.
fn edited(text: &str, edit: HookEdit) -> String {
    hooks_file_edited(Some(text), edit)
        .unwrap()
        .expect("the edit changes the file")
}

fn parsed(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

fn value_installed(text: &str) -> serde_json::Value {
    hooks_with_install(
        parsed(text),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap()
}

/// The file is its owner's: the order of its keys, a compact array, an
/// escape and a number spelled their way all stay BYTE for byte. Deck's
/// key goes after the last one, in the document's indentation; nothing
/// is written a second time; and turning the hooks off again gives the
/// original file back exactly.
#[test]
fn the_file_keeps_every_byte_that_is_not_decks() {
    let original = concat!(
        "{\n",
        "  \"model\": \"opus\",\n",
        "  \"zeta\": true,\n",
        "  \"env\": {\n",
        "    \"ZED\": \"1\",\n",
        "    \"ALPHA\": \"2\"\n",
        "  },\n",
        "  \"alpha\": [1, 2],\n",
        "  \"note\": \"caf\\u00e9\",\n",
        "  \"big\": 1e3\n",
        "}\n",
    );
    let installed = edited(original, CLAUDE_INSTALL);
    let kept = original.strip_suffix("\n}\n").unwrap();
    // deck's own part is what serde writes for it, one step in
    let hooks = serde_json::to_string_pretty(&value_installed(original)["hooks"])
        .unwrap()
        .replace('\n', "\n  ");
    assert_eq!(installed, format!("{kept},\n  \"hooks\": {hooks}\n}}\n"));
    assert_eq!(parsed(&installed), value_installed(original));

    assert_eq!(
        hooks_file_edited(Some(&installed), CLAUDE_INSTALL).unwrap(),
        None,
        "already what the toggle asks for: nothing to write"
    );
    assert_eq!(edited(&installed, HookEdit::Uninstall), original);
    assert_eq!(
        hooks_file_edited(Some(original), HookEdit::Uninstall).unwrap(),
        None
    );
}

/// Foreign groups keep their own layout — a compact one, an oddly spaced
/// one, an empty list — and deck's lines follow the document's
/// indentation, here four spaces.
#[test]
fn foreign_groups_keep_their_text_and_decks_follow_the_document() {
    let stop = "            {\"hooks\": [{\"type\": \"command\", \"command\": \"say done\"}]}";
    let guard = "        \"PreToolUse\": [ { \"matcher\" : \"Bash\", \"hooks\" : [ { \"type\":\"command\", \"command\":\"my-guard\" } ] } ],\n";
    let original = format!(
            "{{\n    \"hooks\": {{\n        \"Stop\": [\n{stop}\n        ],\n{guard}        \"SessionEnd\": []\n    }},\n    \"model\": \"opus\"\n}}\n"
        );
    let installed = edited(&original, CLAUDE_INSTALL);
    assert!(
            installed.starts_with(&format!(
                "{{\n    \"hooks\": {{\n        \"Stop\": [\n{stop},\n            {{\n                \"hooks\": [\n"
            )),
            "{installed}"
        );
    assert!(installed.contains(guard));
    assert!(installed.contains("        \"SessionEnd\": [],\n        \"UserPromptSubmit\": [\n"));
    assert!(installed.ends_with("\n        ]\n    },\n    \"model\": \"opus\"\n}\n"));
    assert!(
        installed
            .lines()
            .all(|line| (line.len() - line.trim_start_matches(' ').len()) % 4 == 0),
        "{installed}"
    );
    assert_eq!(parsed(&installed), value_installed(&original));
    assert_eq!(edited(&installed, HookEdit::Uninstall), original);
}

/// Tabs and CRLF line ends, and a file written on one line, stay what
/// they are.
#[test]
fn tabs_line_ends_and_a_one_line_file_stay_what_they_are() {
    let theirs = "{ \"hooks\": [{ \"type\": \"command\", \"command\": \"say done\" }] }";
    let original = format!(
            "{{\r\n\t\"model\": \"opus\",\r\n\t\"hooks\": {{\r\n\t\t\"Stop\": [\r\n\t\t\t{theirs}\r\n\t\t]\r\n\t}}\r\n}}\r\n"
        );
    let installed = edited(&original, CLAUDE_INSTALL);
    assert!(installed.contains(&format!(
        "\r\n\t\t\t{theirs},\r\n\t\t\t{{\r\n\t\t\t\t\"hooks\": [\r\n\t\t\t\t\t{{\r\n"
    )));
    assert!(
        !installed.replace("\r\n", "").contains(['\r', '\n']),
        "every line ends the way the file's do"
    );
    assert!(
        installed.lines().all(|line| !line.starts_with(' ')),
        "indented with tabs alone: {installed}"
    );
    assert_eq!(parsed(&installed), value_installed(&original));
    assert_eq!(edited(&installed, HookEdit::Uninstall), original);

    let one_line = "{\"model\":\"opus\",\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"say done\"}]}]}}";
    let installed = edited(one_line, CLAUDE_INSTALL);
    assert!(
        !installed.contains('\n'),
        "still one line, still no final newline"
    );
    assert!(installed.starts_with(
            "{\"model\":\"opus\",\"hooks\":{\"Stop\":[{\"hooks\":[{\"type\":\"command\",\"command\":\"say done\"}]},{"
        ));
    assert_eq!(parsed(&installed), value_installed(one_line));
    assert_eq!(edited(&installed, HookEdit::Uninstall), one_line);
}

/// A hook the user keeps in deck's own group: taking deck's hook out
/// leaves theirs, and the group around it, exactly as written.
#[test]
fn a_shared_group_loses_decks_hook_and_nothing_else() {
    let theirs = "          { \"type\": \"command\", \"command\": \"say done\" }\n";
    let before = "{\n  \"hooks\": {\n    \"Stop\": [\n      {\n        \"hooks\": [\n";
    let after = "        ]\n      }\n    ]\n  }\n}\n";
    let original = format!(
            "{before}          {{ \"type\": \"command\", \"command\": \"{HELPER}\", \"args\": [\"claude-code\", \"turn-done\"], \"timeout\": 10 }},\n{theirs}{after}"
        );
    assert_eq!(
        edited(&original, HookEdit::Uninstall),
        format!("{before}{theirs}{after}")
    );
    // an install (two events are missing) moves deck's hook to a group
    // of its own and leaves theirs where it was
    let installed = edited(&original, CLAUDE_INSTALL);
    assert!(
        installed.starts_with(&format!(
            "{before}{}\n        ]\n      }},\n      {{\n        \"hooks\": [\n",
            theirs.trim_end()
        )),
        "{installed}"
    );
    assert_eq!(parsed(&installed), value_installed(&original));
    assert_eq!(
        edited(&installed, HookEdit::Uninstall),
        format!("{before}{theirs}{after}")
    );
    // a file whose deck hooks are the spec's is not written at all, a
    // shared group or not: neither the toggle nor the boot migration
    // rearranges what the user put together
    let mut shared = value_installed("{}");
    shared["hooks"]["Stop"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            serde_json::json!({ "type": "command", "command": "say done" }),
        );
    let shared = serde_json::to_string_pretty(&shared).unwrap();
    assert_eq!(
        hooks_file_edited(Some(&shared), CLAUDE_INSTALL).unwrap(),
        None
    );
}

/// No file, an empty object, a text that is not JSON, and one the
/// editor cannot follow.
#[test]
fn a_new_file_an_empty_one_and_what_is_refused() {
    // no file yet: deck's own document, as it always wrote it
    let fresh = hooks_file_edited(None, CLAUDE_INSTALL).unwrap().unwrap();
    assert_eq!(
        fresh,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&value_installed("{}")).unwrap()
        )
    );
    assert_eq!(hooks_file_edited(None, HookEdit::Uninstall).unwrap(), None);
    // an empty object, with and without a final newline: laid out in
    // two spaces, and the final newline is the owner's choice
    for original in ["{}", "{}\n"] {
        let installed = edited(original, CLAUDE_INSTALL);
        let (head, tail) = original.split_once("{}").unwrap();
        assert!(installed.starts_with(&format!("{head}{{\n  \"hooks\": {{\n")));
        assert!(installed.ends_with(&format!("\n  }}\n}}{tail}")));
        assert_eq!(parsed(&installed), value_installed(original));
        assert_eq!(edited(&installed, HookEdit::Uninstall), original);
    }
    // the last of two equal keys is the one serde_json reads; the
    // shadowed one is the owner's text like everything else
    let twice = "{\"hooks\": {\"Stop\": []}, \"hooks\": {}}";
    let installed = edited(twice, CLAUDE_INSTALL);
    assert!(installed.starts_with("{\"hooks\": {\"Stop\": []}, \"hooks\": {"));
    assert_eq!(parsed(&installed), value_installed(twice));
    // refused, with a reason and nothing to write
    for broken in ["", "{ \"model\": ", "[]", "{\"hooks\": []}"] {
        assert!(
            hooks_file_edited(Some(broken), CLAUDE_INSTALL).is_err(),
            "{broken:?}"
        );
    }
    assert!(hooks_file_edited(Some("{ nope"), HookEdit::Uninstall).is_err());
    // the editor itself gives up on what it cannot follow
    assert_eq!(hooks_text_edited("[]", CLAUDE_INSTALL), None);
    assert_eq!(
        hooks_text_edited("{\"hooks\": {\"Stop\": [}}", CLAUDE_INSTALL),
        None
    );
    assert_eq!(hooks_text_edited("{\"hooks\": 1}", CLAUDE_INSTALL), None);
}

/// The whole write on disk: through a symlink, byte for byte, and no
/// write at all when the file already is what was asked for.
#[test]
fn the_toggle_writes_the_owners_file_in_place_or_not_at_all() {
    use std::os::unix::fs::MetadataExt;
    let root = std::env::temp_dir().join(format!("deck-hooktext-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (home, repo) = (root.join("claude"), root.join("dotfiles"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    let original = "{\n  \"zeta\": 1,\n  \"alpha\": [1, 2]\n}\n";
    let target = repo.join("settings.json");
    std::fs::write(&target, original).unwrap();
    let link = home.join("settings.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let read = |path: &Path| std::fs::read_to_string(path).unwrap();
    let inode = |path: &Path| std::fs::metadata(path).unwrap().ino();

    assert!(hooks_set(&link, CLAUDE_INSTALL, "never ran").unwrap());
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(read(&target), edited(original, CLAUDE_INSTALL));
    let written = inode(&target);
    assert!(!hooks_set(&link, CLAUDE_INSTALL, "never ran").unwrap());
    assert_eq!(inode(&target), written, "the file was not replaced again");
    assert!(hooks_set(&link, HookEdit::Uninstall, "never ran").unwrap());
    assert_eq!(read(&target), original);
    assert!(!hooks_set(&link, HookEdit::Uninstall, "never ran").unwrap());

    // no file: turning hooks off creates none, turning them on does
    let fresh = home.join("hooks.json");
    assert!(!hooks_set(&fresh, HookEdit::Uninstall, "never ran").unwrap());
    assert!(!fresh.exists());
    assert!(hooks_set(&fresh, CLAUDE_INSTALL, "never ran").unwrap());
    assert_eq!(parsed(&read(&fresh)), value_installed("{}"));

    // not JSON: refused, not a byte changed
    let broken = home.join("broken.json");
    std::fs::write(&broken, "{ nope").unwrap();
    assert!(hooks_set(&broken, CLAUDE_INSTALL, "never ran").is_err());
    assert_eq!(read(&broken), "{ nope");
    let _ = std::fs::remove_dir_all(&root);
}

/// Awkward but valid documents: brackets and quotes inside strings, a
/// `hooks` key that is not the document's, numbers and escapes in their
/// own spelling, uneven spacing, several groups. The edited text always
/// parses to what the value engine decided, and turning the hooks off
/// again gives the original bytes back.
#[test]
fn awkward_documents_are_followed_byte_for_byte() {
    let documents = [
            "{\"a\":\"] } \\\" [ {\",\"b\":[{\"hooks\":\"not ours\"}]}",
            "{\n  \"env\": { \"hooks\": 1 },\n  \"n\": [-0.0, 1E+2, 123456789012345678901234567890],\n  \"s\": \"\\u4f60\\ud83d\\ude00\\\\\"\n}",
            "{\n\t\"hooks\": {\n\t\t\"PreToolUse\": [\n\t\t\t{ \"matcher\": \"Bash\", \"hooks\": [] },\n\n\t\t\t{ \"hooks\": [ { \"type\": \"command\", \"command\": \"a\" } ,{ \"type\": \"command\", \"command\": \"b\" } ] }\n\t\t]   ,\n\t\t\"Stop\"   :   [ { \"hooks\": [ { \"type\": \"command\", \"command\": \"c\" } ] } ]\n\t}\n}",
            "  {\"model\": \"opus\"}  \n\n",
            "{\"hooks\": {\"PreCompact\": \"left alone\", \"Other\": [1, \"x\", null]}, \"hooks2\": {}}",
        ];
    for original in documents {
        let installed = edited(original, CLAUDE_INSTALL);
        assert_eq!(
            parsed(&installed),
            value_installed_or_none(original).expect("installable"),
            "{original}"
        );
        assert_eq!(
            hooks_file_edited(Some(&installed), CLAUDE_INSTALL).unwrap(),
            None,
            "{original}"
        );
        assert_eq!(
            edited(&installed, HookEdit::Uninstall),
            original,
            "{original}"
        );
    }
    // deck's group in the middle of a list goes, and its neighbours keep
    // the separators they had
    let deck = serde_json::to_string(&spec_entry(
        HookStyle::Exec,
        HELPER,
        "claude-code",
        &("Stop", None, "turn-done"),
    ))
    .unwrap();
    let middle = format!(
            "{{\"hooks\": {{\"Stop\": [ {{\"hooks\": [{{\"command\": \"a\"}}]}} ,\t{deck},\n  {{\"hooks\": [{{\"command\": \"b\"}}]}} ]}}}}"
        );
    assert_eq!(
            edited(&middle, HookEdit::Uninstall),
            "{\"hooks\": {\"Stop\": [ {\"hooks\": [{\"command\": \"a\"}]},\n  {\"hooks\": [{\"command\": \"b\"}]} ]}}"
        );
    // what the user wrote empty cannot be told from what deck emptied
    // once deck's hooks have been in it: the round trip keeps the value
    // of everything else, not these empty containers
    for (original, without) in [
        ("{ }", "{}"),
        ("{\n}\n", "{}\n"),
        ("{\"hooks\": { }, \"z\": 1}", "{\"z\": 1}"),
    ] {
        let installed = edited(original, CLAUDE_INSTALL);
        assert_eq!(edited(&installed, HookEdit::Uninstall), without);
    }
}

fn value_installed_or_none(text: &str) -> Option<serde_json::Value> {
    hooks_with_install(
        parsed(text),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .ok()
}

#[test]
fn hook_install_refuses_malformed_documents() {
    let install = |v| hooks_with_install(v, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER);
    assert!(install(serde_json::json!([1, 2])).is_err());
    assert!(install(serde_json::json!({ "hooks": "nope" })).is_err());
    assert!(install(serde_json::json!({ "hooks": { "Stop": "nope" } })).is_err());
    // partial installs read as OFF so re-enabling repairs them
    let partial = serde_json::json!({
        "hooks": { "Stop": [{ "hooks": [{ "type": "command",
            "command": "\"$HOME/.deck/bin/deck-status-helper\" claude-code turn-done" }] }] }
    });
    assert!(!hooks_installed(&partial, CLAUDE_HOOKS));
}

#[test]
fn stale_entries_are_ours_and_get_rewritten_to_the_current_spec() {
    let legacy_command =
        |state: &str| format!("\"$HOME/.deck/bin/deck-status-helper\" claude-code {state}");
    let legacy = serde_json::json!({
        "model": "opus",
        "hooks": {
            "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": legacy_command("working"), "timeout": 10 }] }],
            "Notification": [{ "matcher": "permission_prompt|idle_prompt", "hooks": [{ "type": "command", "command": legacy_command("needs-input"), "timeout": 10 }] }],
            "Stop": [
                { "hooks": [{ "type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff" }] },
                { "hooks": [{ "type": "command", "command": legacy_command("turn-done"), "timeout": 10 }] }
            ]
        }
    });
    // a legacy install still reads as installed, but not as current:
    // the helper path is stale AND the matcher predates the narrowing
    assert!(hooks_installed(&legacy, CLAUDE_HOOKS));
    assert!(!hooks_are_current(
        &legacy,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER
    ));

    // migration = the ordinary install over the legacy document
    let migrated = hooks_with_install(
        legacy.clone(),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert!(hooks_are_current(
        &migrated,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER
    ));
    assert_eq!(migrated["model"], "opus");
    assert_eq!(migrated["hooks"]["Stop"].as_array().unwrap().len(), 2);
    let text = serde_json::to_string(&migrated).unwrap();
    assert!(!text.contains(".deck/bin"), "no legacy path survives");
    assert!(text.contains(HELPER));
    // the stale wide matcher is gone — an idle prompt no longer decays a
    // finished card into "needs-input"
    assert_eq!(
        migrated["hooks"]["Notification"][0]["matcher"],
        "permission_prompt"
    );

    // a bundle that moved is equally not current
    let moved = "/Users/x/Applications/deck.app/Contents/MacOS/deck-status-helper";
    assert!(!hooks_are_current(
        &migrated,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        moved
    ));
    assert!(hooks_are_current(
        &hooks_with_install(
            migrated.clone(),
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            moved
        )
        .unwrap(),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        moved
    ));

    // a shell-form entry with the right path is not current for Claude
    // Code (exec form expected) but is for Codex
    let shell_form = hooks_with_install(
        serde_json::json!({}),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::ShellAsync,
        HELPER,
    )
    .unwrap();
    assert!(!hooks_are_current(
        &shell_form,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER
    ));
    assert!(hooks_are_current(
        &shell_form,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::ShellAsync,
        HELPER
    ));
    let exec_form = hooks_with_install(
        shell_form,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert!(hooks_are_current(
        &exec_form,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER
    ));
    assert!(!serde_json::to_string(&exec_form).unwrap().contains("\\\""));

    // uninstall removes legacy and current entries alike
    let removed = hooks_with_uninstall(legacy);
    assert_eq!(removed["hooks"]["Stop"].as_array().unwrap().len(), 1);
    assert!(!hooks_installed(&removed, CLAUDE_HOOKS));
}

/// Boot migration rewrites whenever the document is not current, so
/// install's OWN output must be current for every module — otherwise
/// deck would rewrite the user's agent config on every launch. One
/// spec event carrying two specs is exactly how that would happen.
#[test]
fn installing_any_module_leaves_a_document_the_migration_will_not_touch() {
    for (specs, source, style) in [
        (CLAUDE_HOOKS, "claude-code", HookStyle::Exec),
        (CODEX_HOOKS, "codex", HookStyle::ShellAsync),
    ] {
        let mut events: Vec<&str> = specs.iter().map(|(e, _, _)| *e).collect();
        events.sort_unstable();
        let unique = events.len();
        events.dedup();
        assert_eq!(events.len(), unique, "{source}: one spec per event");

        let doc = hooks_with_install(
            serde_json::json!({ "model": "opus" }),
            specs,
            source,
            style,
            HELPER,
        )
        .unwrap();
        assert!(hooks_installed(&doc, specs), "{source}");
        assert!(
            hooks_are_current(&doc, specs, source, style, HELPER),
            "{source}: a fresh install must not be stale"
        );
    }
}

/// The migration's whole point after 0.5.13: a user who enabled the
/// toggle under an older spec gets the narrowed matcher without touching
/// the Settings switch, and a retired event leaves nothing behind.
#[test]
fn a_spec_change_alone_makes_installed_hooks_stale() {
    let current = hooks_with_install(
        serde_json::json!({}),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    let is_current = |doc: &serde_json::Value| {
        hooks_are_current(doc, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER)
    };
    assert!(is_current(&current));

    // same helper, same events, only the matcher predates the narrowing
    let mut wide = current.clone();
    wide["hooks"]["Notification"][0]["matcher"] =
        serde_json::json!("permission_prompt|idle_prompt");
    assert!(hooks_installed(&wide, CLAUDE_HOOKS), "still installed");
    assert!(!is_current(&wide), "a matcher change is a stale install");
    assert!(is_current(
        &hooks_with_install(wide, CLAUDE_HOOKS, "claude-code", HookStyle::Exec, HELPER).unwrap()
    ));

    // an event this deck version no longer registers: not current, and
    // install drops the husk instead of leaving a dead hook behind
    let mut retired = current.clone();
    retired["hooks"]["SessionStart"] = serde_json::json!([spec_entry(
        HookStyle::Exec,
        HELPER,
        "claude-code",
        &("SessionStart", None, "working")
    )]);
    assert!(!is_current(&retired));
    let repaired = hooks_with_install(
        retired,
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert!(is_current(&repaired));
    assert!(repaired["hooks"].get("SessionStart").is_none());
    assert_eq!(repaired, current);

    // a duplicate deck entry (a hand-edited file) is stale, and install
    // collapses it back to exactly one
    let mut doubled = current.clone();
    let entry = doubled["hooks"]["Stop"][0].clone();
    doubled["hooks"]["Stop"].as_array_mut().unwrap().push(entry);
    assert!(!is_current(&doubled));
    assert_eq!(
        hooks_with_install(
            doubled,
            CLAUDE_HOOKS,
            "claude-code",
            HookStyle::Exec,
            HELPER
        )
        .unwrap(),
        current
    );

    // an EMPTY array the user wrote is theirs — install never prunes it
    let user_empty = hooks_with_install(
        serde_json::json!({ "hooks": { "PreToolUse": [] } }),
        CLAUDE_HOOKS,
        "claude-code",
        HookStyle::Exec,
        HELPER,
    )
    .unwrap();
    assert_eq!(user_empty["hooks"]["PreToolUse"], serde_json::json!([]));
}

#[test]
fn helper_path_requires_a_present_quotable_sidecar_in_a_release_location() {
    let dir = std::env::temp_dir().join(format!("deck-hooks-{}", std::process::id()));
    let bundle = dir.join("deck.app");
    let macos = bundle.join("Contents").join("MacOS");
    std::fs::create_dir_all(&macos).unwrap();
    assert!(helper_path_in(&bundle).is_err(), "missing sidecar");
    std::fs::write(macos.join(HELPER_NAME), "binary").unwrap();
    let helper = helper_path_in(&bundle).unwrap();
    assert!(helper.ends_with("/deck.app/Contents/MacOS/deck-status-helper"));
    assert!(helper.contains(HELPER_MARKER));
    assert_eq!(
        hook_value(HookStyle::ShellAsync, &helper, "codex", "working")["command"],
        format!("\"{helper}\" codex working")
    );
    assert_eq!(
        hook_value(HookStyle::Exec, &helper, "claude-code", "working")["command"],
        helper
    );

    let quoted = dir.join("we\"ird").join("deck.app");
    let quoted_macos = quoted.join("Contents").join("MacOS");
    std::fs::create_dir_all(&quoted_macos).unwrap();
    std::fs::write(quoted_macos.join(HELPER_NAME), "binary").unwrap();
    assert!(helper_path_in(&quoted).is_err(), "unquotable path");
    let _ = std::fs::remove_dir_all(&dir);

    // the test binary is not a release install: no hook may name it
    let error = installed_helper_path().unwrap_err();
    assert!(error.message().contains("/Applications/deck.app"));
}

// ---------- two ownership families: Signal and Lifecycle ----------------

const LIFECYCLE_HELPER: &str = "/Applications/deck.app/Contents/MacOS/deck-lifecycle-helper";
const LIFECYCLE_INSTALL: HookEdit = HookEdit::Install {
    specs: CLAUDE_LIFECYCLE_HOOKS,
    source: "claude-code",
    style: HookStyle::ExecBrief,
    helper: LIFECYCLE_HELPER,
};

fn family_edit(family: Family, text: &str, edit: HookEdit) -> Option<String> {
    hooks_file_edited_of(family, Some(text), edit).expect("an editable document")
}

fn document(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("a JSON document")
}

/// The user's own hooks: one under an event the lifecycle family also
/// uses, one under an event the Signal family also uses.
const USER_HOOKS: &str = r#"{
    "model": "opus",
    "hooks": {
        "SessionStart": [
            { "hooks": [ { "type": "command", "command": "/usr/local/bin/mine start" } ] }
        ],
        "Stop": [ { "hooks": [ { "type": "command", "command": "/usr/local/bin/mine stop", "timeout": 5 } ] } ]
    }
 }"#;

#[test]
fn each_hook_family_installs_and_removes_only_its_own_hooks() {
    let signal = family_edit(Family::Signal, USER_HOOKS, CLAUDE_INSTALL).unwrap();
    let both = family_edit(Family::Lifecycle, &signal, LIFECYCLE_INSTALL).unwrap();
    let value = document(&both);
    assert!(hooks_installed_of(Family::Signal, &value, CLAUDE_HOOKS));
    assert!(hooks_installed_of(
        Family::Lifecycle,
        &value,
        CLAUDE_LIFECYCLE_HOOKS
    ));
    // the lifecycle hook: exec form, the closed event word, one second
    assert_eq!(
        value["hooks"]["SessionEnd"],
        serde_json::json!([{ "hooks": [{
                "type": "command", "command": LIFECYCLE_HELPER,
                "args": ["claude-code", "session-end"], "timeout": 1
            }] }])
    );
    // beside the user's own SessionStart group, not instead of it
    let starts = value["hooks"]["SessionStart"].as_array().unwrap();
    assert_eq!(starts.len(), 2);
    assert_eq!(
        starts[0]["hooks"][0]["command"],
        "/usr/local/bin/mine start"
    );
    assert_eq!(starts[1]["hooks"][0]["args"][1], "session-start");
    // asking again writes nothing
    assert_eq!(
        family_edit(Family::Lifecycle, &both, LIFECYCLE_INSTALL),
        None
    );
    assert_eq!(family_edit(Family::Signal, &both, CLAUDE_INSTALL), None);

    // Turning the status hooks off keeps the lifecycle hooks and the
    // user's: the result is what installing lifecycle alone gives.
    let without_signal = family_edit(Family::Signal, &both, HookEdit::Uninstall).unwrap();
    let lifecycle_only = family_edit(Family::Lifecycle, USER_HOOKS, LIFECYCLE_INSTALL).unwrap();
    assert_eq!(document(&without_signal), document(&lifecycle_only));
    assert!(hooks_installed_of(
        Family::Lifecycle,
        &document(&without_signal),
        CLAUDE_LIFECYCLE_HOOKS
    ));
    assert!(!without_signal.contains("deck-status-helper"));
    assert!(without_signal.contains("/usr/local/bin/mine stop"));

    // Turning re-entry observation off keeps the status hooks and the
    // user's, byte for byte.
    assert_eq!(
        family_edit(Family::Lifecycle, &both, HookEdit::Uninstall).unwrap(),
        signal
    );
    // with both families gone the owner's bytes are back
    assert_eq!(
        family_edit(Family::Lifecycle, &without_signal, HookEdit::Uninstall).unwrap(),
        USER_HOOKS
    );
    assert_eq!(
        family_edit(Family::Signal, &signal, HookEdit::Uninstall).unwrap(),
        USER_HOOKS
    );
}

#[test]
fn a_status_hook_user_is_never_enrolled_in_lifecycle_observation() {
    let signal = family_edit(Family::Signal, USER_HOOKS, CLAUDE_INSTALL).unwrap();
    let value = document(&signal);
    assert!(!hooks_installed_of(
        Family::Lifecycle,
        &value,
        CLAUDE_LIFECYCLE_HOOKS
    ));
    assert!(value["hooks"].get("SessionEnd").is_none());
    assert_eq!(value["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
    // the Signal family's own repair (what boot migration applies) of a
    // moved bundle adds no lifecycle hook either
    let moved = HookEdit::Install {
        specs: CLAUDE_HOOKS,
        source: "claude-code",
        style: HookStyle::Exec,
        helper: "/Users/u/Applications/deck.app/Contents/MacOS/deck-status-helper",
    };
    let repaired = document(&family_edit(Family::Signal, &signal, moved).unwrap());
    assert!(!hooks_installed_of(
        Family::Lifecycle,
        &repaired,
        CLAUDE_LIFECYCLE_HOOKS
    ));
    assert!(!repaired.to_string().contains("deck-lifecycle-helper"));
    // removing a family that is not there writes nothing
    assert_eq!(
        family_edit(Family::Lifecycle, &signal, HookEdit::Uninstall),
        None
    );
    assert_eq!(
        family_edit(Family::Lifecycle, USER_HOOKS, HookEdit::Uninstall),
        None
    );
    // neither family touches a file it cannot read as JSON
    for family in [Family::Signal, Family::Lifecycle] {
        assert!(hooks_file_edited_of(family, Some("{ not json"), HookEdit::Uninstall).is_err());
        assert!(hooks_file_edited_of(family, Some("{ not json"), LIFECYCLE_INSTALL).is_err());
    }
}

#[test]
fn a_moved_bundle_updates_the_lifecycle_hooks_and_keeps_a_user_hook_in_their_group() {
    // the user put a hook of their own into deck's lifecycle group
    let installed = family_edit(Family::Lifecycle, USER_HOOKS, LIFECYCLE_INSTALL).unwrap();
    let mut value = document(&installed);
    let end = value["hooks"]["SessionEnd"][0]["hooks"]
        .as_array_mut()
        .unwrap();
    end.push(serde_json::json!({ "type": "command", "command": "/usr/local/bin/mine end" }));
    let mixed = serde_json::to_string_pretty(&value).unwrap();
    let stale = HookEdit::Install {
        specs: CLAUDE_LIFECYCLE_HOOKS,
        source: "claude-code",
        style: HookStyle::ExecBrief,
        helper: "/Users/u/Applications/deck.app/Contents/MacOS/deck-lifecycle-helper",
    };
    assert!(hooks_are_current_of(
        Family::Lifecycle,
        &document(&mixed),
        CLAUDE_LIFECYCLE_HOOKS,
        "claude-code",
        HookStyle::ExecBrief,
        LIFECYCLE_HELPER
    ));
    let moved = document(&family_edit(Family::Lifecycle, &mixed, stale).unwrap());
    let text = moved.to_string();
    for event in ["SessionStart", "SessionEnd"] {
        let ours: Vec<&str> = moved["hooks"][event]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["hooks"].as_array().unwrap())
            .filter_map(|hook| hook["command"].as_str())
            .filter(|command| command.contains("deck-lifecycle-helper"))
            .collect();
        assert_eq!(
            ours,
            ["/Users/u/Applications/deck.app/Contents/MacOS/deck-lifecycle-helper"],
            "{event}"
        );
    }
    assert!(
        text.contains("/usr/local/bin/mine end"),
        "the user's hook stays"
    );
    // and removal leaves that hook, in its group, behind
    let removed = document(&family_edit(Family::Lifecycle, &mixed, HookEdit::Uninstall).unwrap());
    assert_eq!(
        removed["hooks"]["SessionEnd"][0]["hooks"],
        serde_json::json!([{ "type": "command", "command": "/usr/local/bin/mine end" }])
    );
    assert!(!removed.to_string().contains("deck-lifecycle-helper"));
}

#[test]
fn only_claude_declares_a_reentry_capability_and_lifecycle_hooks() {
    let claude = crate::extensions::agent("claude-code").expect("registered");
    assert!(claude.reentry.is_some());
    assert_eq!(
        claude.hook_install().expect("hooks").lifecycle_hooks.len(),
        2
    );
    let codex = crate::extensions::agent("codex").expect("registered");
    assert!(codex.reentry.is_none(), "Codex is never re-entered");
    assert!(codex
        .hook_install()
        .expect("hooks")
        .lifecycle_hooks
        .is_empty());
    // Pi: registered, no hook family at all and no re-entry capability
    let pi = crate::extensions::agent("pi").expect("registered");
    assert!(pi.reentry.is_none(), "Pi is never re-entered");
    assert!(pi.hook_install().is_none());
    assert!(matches!(pi.install, Install::PiExtension));
    assert!(crate::extensions::agent("fake-agent").is_none());
    // the two helpers are two markers: neither family owns the other's
    for (family, command, owns) in [
        (Family::Signal, HELPER, true),
        (Family::Signal, LIFECYCLE_HELPER, false),
        (Family::Lifecycle, LIFECYCLE_HELPER, true),
        (Family::Lifecycle, HELPER, false),
        (
            Family::Lifecycle,
            "/Users/u/.deck/bin/deck-status-helper",
            false,
        ),
        (
            Family::Signal,
            "/Users/u/.deck/bin/deck-status-helper",
            true,
        ),
    ] {
        assert_eq!(family.owns(command), owns, "{family:?} {command}");
    }
    // a development build cannot install either family
    assert!(agent_lifecycle_hooks_set(true).is_err());
}

/// Not a test of its own: the child of
/// `the_production_read_entries_answer_from_a_real_home_and_environment`.
/// It calls the three production read entries of the re-entry assembly
/// exactly as production does — nothing is handed in — in whatever home
/// and environment it was started with, runs the production assembly
/// over their answers, and prints closed words.
#[test]
#[ignore = "run only as a child process, in a home and an environment its parent made"]
fn production_read_entries_child() {
    if std::env::var_os("DECK_READ_ENTRIES_CHILD").is_none() {
        return;
    }
    let (Some(home), Some(capability)) = (
        dirs::home_dir(),
        crate::extensions::agent(CLAUDE_CODE.source).and_then(|adapter| adapter.reentry),
    ) else {
        return println!("ENTRIES unavailable");
    };
    let helper = bundled_lifecycle_helper();
    let installed = lifecycle_observation_installed();
    let baseline = crate::procinfo::own_baseline_supported(&home, capability.grammar);
    // the assembly reads no pane here: its tmux is never called
    let tmux = |_: &[&str]| Err(DeckError::new(ErrorKind::Tmux, "not called"));
    let tmux_stdin = |_: &[&str], _: &[u8]| Err(DeckError::new(ErrorKind::Tmux, "not called"));
    let (observes, assembled_baseline) = crate::tmux_lifecycle::assemble_reentry_env(
        &crate::tmux_lifecycle::ReentryParts {
            tmux: &tmux,
            tmux_stdin: &tmux_stdin,
            socket: "unused",
            capability,
            helper: helper.clone(),
            hooks_installed: installed,
            home: &home,
            baseline,
        },
        |env| (env.observation, env.baseline),
    );
    println!(
            "ENTRIES settings-say-installed={installed} helper-beside-the-executable={} baseline-supported={baseline} assembly-observes={observes} assembly-baseline={assembled_baseline}",
            helper.is_some()
        );
}

/// The three read entries the re-entry assembly takes its facts from —
/// the user-level opt-in, the helper beside the executable, the baseline
/// of this process's environment — were handed to every earlier test and
/// to every native stage as explicit values. Here each one answers by
/// itself, in a child process whose home, environment and directory this
/// test made; the real home and the real Claude settings are never read.
///
/// What the missing-helper cells show is the layer they call: the
/// assembly does not observe. Nothing here runs a restart.
#[test]
fn the_production_read_entries_answer_from_a_real_home_and_environment() {
    use std::path::Path;
    let root = std::env::temp_dir().join(format!("deck-read-entries-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let exe = std::env::current_exe().expect("test executable");
    // this test binary under two directories: one with a file of the
    // helper's name beside it, one without
    let (with, without) = (root.join("with-helper"), root.join("without-helper"));
    for dir in [&with, &without] {
        std::fs::create_dir_all(dir).expect("directory");
        std::fs::hard_link(&exe, dir.join("entries"))
            .or_else(|_| std::fs::copy(&exe, dir.join("entries")).map(|_| ()))
            .expect("a copy of the test binary");
    }
    std::fs::write(with.join(LIFECYCLE_HELPER_NAME), b"").expect("helper name");
    // settings documents built by the production entry builder
    let document = |helper: &str, specs: &[HookSpec]| {
        let mut hooks = serde_json::Map::new();
        for spec in specs {
            hooks.insert(
                spec.0.to_string(),
                serde_json::json!([spec_entry(
                    HookStyle::ExecBrief,
                    helper,
                    CLAUDE_CODE.source,
                    spec
                )]),
            );
        }
        serde_json::json!({ "hooks": hooks })
    };
    let signal = document(
        "/Applications/deck.app/Contents/MacOS/deck-status-helper",
        CLAUDE_HOOKS,
    );
    let lifecycle = document(
        "/Applications/deck.app/Contents/MacOS/deck-lifecycle-helper",
        CLAUDE_LIFECYCLE_HOOKS,
    );
    assert!(hooks_installed(&signal, CLAUDE_HOOKS));
    assert!(!hooks_installed_of(
        Family::Lifecycle,
        &signal,
        CLAUDE_LIFECYCLE_HOOKS
    ));
    assert!(hooks_installed_of(
        Family::Lifecycle,
        &lifecycle,
        CLAUDE_LIFECYCLE_HOOKS
    ));
    let home = |name: &str, settings: Option<&serde_json::Value>| {
        let home = root.join(name);
        std::fs::create_dir_all(home.join(".claude")).expect("home");
        if let Some(settings) = settings {
            std::fs::write(
                home.join(".claude").join("settings.json"),
                serde_json::to_string_pretty(settings).expect("document"),
            )
            .expect("settings");
        }
        home
    };
    let (none, only_signal, opted_in) = (
        home("home-no-settings", None),
        home("home-signal-only", Some(&signal)),
        home("home-lifecycle", Some(&lifecycle)),
    );
    let answer = |dir: &Path, home: &Path, extra: &[(&str, &str)]| -> String {
        let out = std::process::Command::new(dir.join("entries"))
            .args([
                "--exact",
                "agent_status::hooks::tests::production_read_entries_child",
                "--ignored",
                "--nocapture",
            ])
            .env_clear()
            // where this instrumented child's counters belong (`cargo llvm-cov`)
            .envs(std::env::var_os("LLVM_PROFILE_FILE").map(|file| ("LLVM_PROFILE_FILE", file)))
            .env("HOME", home)
            .env("PATH", "/usr/bin:/bin")
            .env("DECK_READ_ENTRIES_CHILD", "1")
            .envs(extra.iter().copied())
            .output()
            .expect("the child process");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("ENTRIES "))
            .unwrap_or("no answer")
            .to_string()
    };
    let line = |installed: bool, helper: bool, baseline: bool, observes: bool| {
        format!(
                "settings-say-installed={installed} helper-beside-the-executable={helper} baseline-supported={baseline} assembly-observes={observes} assembly-baseline={baseline}"
            )
    };
    // no settings file: nothing is installed, with or without a helper
    assert_eq!(
        answer(&without, &none, &[]),
        line(false, false, true, false)
    );
    assert_eq!(answer(&with, &none, &[]), line(false, true, true, false));
    // only the Signal family's hooks: not an opt-in to observation
    assert_eq!(
        answer(&with, &only_signal, &[]),
        line(false, true, true, false)
    );
    // the Lifecycle family's hooks and the helper: the assembly observes
    assert_eq!(answer(&with, &opted_in, &[]), line(true, true, true, true));
    // the same opt-in without a helper beside the executable: it does not
    assert_eq!(
        answer(&without, &opted_in, &[]),
        line(true, false, true, false)
    );
    // an environment that carries a refused name is not a supported baseline
    for refused in [
        ("CLAUDE_CONFIG_DIR", "/elsewhere"),
        ("ANTHROPIC_BASE_URL", "x"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("AWS_BEARER_TOKEN_BEDROCK", "x"),
    ] {
        assert_eq!(
            answer(&with, &opted_in, &[refused]),
            line(true, true, false, true),
            "{}",
            refused.0
        );
    }
    // a name that is not refused changes nothing
    assert_eq!(
        answer(&with, &opted_in, &[("DISABLE_AUTOUPDATER", "1")]),
        line(true, true, true, true)
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Not a test of its own: for a tool outside this crate that must agree
/// with the lifecycle writer. In every directory under the one
/// `DECK_HOOK_WRITER_OUT` names it applies the writer the Settings toggle
/// calls (`hooks_set_of` with the registered adapter's lifecycle specs)
/// to that directory's `settings.json` — which may be absent — once to
/// install and once to uninstall, and keeps a copy of the file after
/// each step. It reads and writes nothing outside that directory.
#[test]
#[ignore = "run only by a tool that names an output directory"]
fn lifecycle_hook_writer_steps() {
    let (Some(out), Ok(helper)) = (
        std::env::var_os("DECK_HOOK_WRITER_OUT").map(std::path::PathBuf::from),
        std::env::var("DECK_HOOK_WRITER_HELPER"),
    ) else {
        return;
    };
    let adapter = crate::extensions::agent(CLAUDE_CODE.source).expect("the Claude adapter");
    let hooks = adapter.hook_install().expect("the Claude hooks");
    let install = HookEdit::Install {
        specs: hooks.lifecycle_hooks,
        source: adapter.source,
        style: HookStyle::ExecBrief,
        helper: &helper,
    };
    let mut cases: Vec<std::path::PathBuf> = std::fs::read_dir(&out)
        .expect("the output directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    cases.sort();
    for case in cases {
        let path = case.join("settings.json");
        let step = |edit: HookEdit, keep: &str| {
            let wrote = hooks_set_of(Family::Lifecycle, &path, edit, hooks.never_ran);
            if path.exists() {
                std::fs::copy(&path, case.join(keep)).expect("keep a copy");
            }
            wrote.map_or_else(|e| e.code().to_string(), |wrote| wrote.to_string())
        };
        let installed = step(install, "after-install.json");
        let again = step(install, "after-second-install.json");
        let removed = step(HookEdit::Uninstall, "after-uninstall.json");
        println!(
                "WRITER case={} install-wrote={installed} second-install-wrote={again} uninstall-wrote={removed}",
                case.file_name().unwrap_or_default().to_string_lossy()
            );
    }
}
