use super::*;

const HELPER: &str = "/Applications/deck.app/Contents/MacOS/deck-status-helper";
const MOVED: &str = "/Users/u/Applications/deck.app/Contents/MacOS/deck-status-helper";

fn with_entry(text: Option<&str>, helper: &str) -> String {
    let document = document(text).unwrap();
    edited(text, &document, Some(&entry(helper))).unwrap()
}

fn without_entry(text: &str) -> String {
    let document = document(Some(text)).unwrap();
    edited(Some(text), &document, None).unwrap()
}

#[test]
fn the_entry_runs_the_helper_on_the_turns_first_call_and_its_end_only() {
    let value = entry(HELPER);
    let object = value.as_object().unwrap();
    let mut events: Vec<&str> = object.keys().map(String::as_str).collect();
    events.sort_unstable();
    // never PreToolUse: an empty answer there denies the tool
    assert_eq!(events, ["PreInvocation", "Stop"]);
    assert_eq!(
        value["PreInvocation"][0]["command"],
        format!("\"{HELPER}\" agy working --turn-start")
    );
    assert_eq!(
        value["Stop"][0]["command"],
        format!("\"{HELPER}\" agy turn-done")
    );
    for event in ["PreInvocation", "Stop"] {
        assert_eq!(value[event][0]["type"], "command");
        assert_eq!(value[event][0]["timeout"], HOOK_TIMEOUT_SECS);
        assert!(crate::agent_status::status_helper_command(
            value[event][0]["command"].as_str().unwrap()
        ));
    }
}

#[test]
fn no_file_is_written_as_the_one_entry() {
    let text = with_entry(None, HELPER);
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value, json!({ ENTRY: entry(HELPER) }));
    assert!(text.ends_with('\n'));
}

#[test]
fn every_other_byte_of_the_users_file_stays() {
    let user = "{\n    \"lint-checker\": {\n        \"PostToolUse\": [ { \"matcher\": \"run_command\", \"hooks\": [ { \"command\": \"./lint.sh\" } ] } ]\n    },\n    \"reminder\": { \"enabled\": false, \"PreInvocation\": [ { \"command\": \"./r.sh\" } ] }\n}\n";
    let installed = with_entry(Some(user), HELPER);
    // the user's members are byte for byte where they were
    assert!(installed.starts_with(&user[..user.rfind('}').unwrap() - 1]));
    let value: Value = serde_json::from_str(&installed).unwrap();
    assert_eq!(value[ENTRY], entry(HELPER));
    assert_eq!(value["reminder"]["enabled"], false);
    // indented in the file's own unit
    assert!(installed.contains("\n    \"deck-status\": {\n        \"PreInvocation\""));
    // and taking it out gives the user's file back exactly
    assert_eq!(without_entry(&installed), user);
}

#[test]
fn a_moved_bundle_is_rewritten_in_place() {
    let user = "{ \"a\": {}, \"deck-status\": X, \"b\": {} }";
    let old = serde_json::to_string(&entry(MOVED)).unwrap();
    let text = user.replace('X', &old);
    let document = document(Some(&text)).unwrap();
    assert_eq!(
        held(&document, Some(HELPER)),
        Held::Ours {
            current: false,
            disabled: false
        }
    );
    let rewritten = edited(Some(&text), &document, Some(&entry(HELPER))).unwrap();
    let value: Value = serde_json::from_str(&rewritten).unwrap();
    assert_eq!(value[ENTRY], entry(HELPER));
    // the member keeps its place between the user's two
    let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 3);
    assert!(rewritten.starts_with("{ \"a\": {}, \"deck-status\": "));
    assert!(rewritten.ends_with(", \"b\": {} }"));
}

#[test]
fn ownership_is_the_helper_in_the_entry_never_its_name() {
    let foreign = json!({ ENTRY: { "Stop": [ { "command": "./my-own.sh" } ] } });
    assert_eq!(held(&foreign, Some(HELPER)), Held::Foreign);
    assert_eq!(held(&json!({}), Some(HELPER)), Held::Absent);
    let current = json!({ ENTRY: entry(HELPER) });
    assert_eq!(
        held(&current, Some(HELPER)),
        Held::Ours {
            current: true,
            disabled: false
        }
    );
    // the legacy helper copy is Deck's too
    let legacy = json!({ ENTRY: { "Stop": [ { "command": "\"/Users/u/.deck/bin/deck-status-helper\" agy turn-done" } ] } });
    assert!(matches!(
        held(&legacy, Some(HELPER)),
        Held::Ours { current: false, .. }
    ));
    // agy's own switch on Deck's entry: the user's disabling
    let mut disabled = entry(HELPER);
    disabled["enabled"] = json!(false);
    assert_eq!(
        held(&json!({ ENTRY: disabled }), Some(HELPER)),
        Held::Ours {
            current: false,
            disabled: true
        }
    );
}

#[test]
fn what_is_not_one_object_without_repeats_is_left_alone() {
    for text in [
        "",
        "not json",
        "[]",
        "\"x\"",
        "{\"a\": 1, \"a\": 2}",
        "{\"deck-status\": {}, \"deck-status\": {}}",
    ] {
        assert!(document(Some(text)).is_err(), "{text:?}");
    }
    assert!(document(Some("{}")).is_ok());
    assert!(document(Some("  {\n}\n")).is_ok());
}

/// The switch and the migration through one file, as `set` and
/// `migrate_on_boot` run them (the helper path given in).
#[test]
fn the_edits_install_refuse_migrate_and_remove_only_decks_entry() {
    let dir = crate::test_dir::TestDir::new("agy-hooks");
    let config = dir.join("config");
    let path = config.join("hooks.json");
    // agy never ran: nothing is created
    assert_eq!(
        apply(&path, Edit::Install(HELPER)).unwrap_err().kind(),
        ErrorKind::Missing
    );
    assert!(!config.exists());
    std::fs::create_dir(&config).unwrap();
    // nothing to remove, nothing to migrate
    assert!(!apply(&path, Edit::Uninstall).unwrap());
    assert!(!apply(&path, Edit::Migrate(HELPER)).unwrap());
    assert!(!path.exists());
    // install, then once more: already current
    assert!(apply(&path, Edit::Install(MOVED)).unwrap());
    assert!(!apply(&path, Edit::Install(MOVED)).unwrap());
    // the migration moves it to this bundle
    assert!(apply(&path, Edit::Migrate(HELPER)).unwrap());
    let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(value, json!({ ENTRY: entry(HELPER) }));
    // removed
    assert!(apply(&path, Edit::Uninstall).unwrap());
    let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(value, json!({}));

    // somebody else's deck-status: never replaced, never removed
    let foreign = "{\"deck-status\": {\"Stop\": [{\"command\": \"./mine.sh\"}]}}";
    std::fs::write(&path, foreign).unwrap();
    assert_eq!(
        apply(&path, Edit::Install(HELPER))
            .unwrap_err()
            .message()
            .to_string(),
        "agy-entry-foreign"
    );
    assert!(!apply(&path, Edit::Uninstall).unwrap());
    assert!(!apply(&path, Edit::Migrate(HELPER)).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), foreign);

    // the user disabled Deck's entry in agy: turning it on is refused, the
    // migration leaves it, turning it off takes it out
    let mut disabled = entry(MOVED);
    disabled["enabled"] = json!(false);
    let text = serde_json::to_string(&json!({ ENTRY: disabled })).unwrap();
    std::fs::write(&path, &text).unwrap();
    assert_eq!(
        apply(&path, Edit::Install(HELPER))
            .unwrap_err()
            .message()
            .to_string(),
        "agy-disabled-by-user"
    );
    assert!(!apply(&path, Edit::Migrate(HELPER)).unwrap());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert!(apply(&path, Edit::Uninstall).unwrap());

    // a file that is not JSON is never touched
    std::fs::write(&path, "{ broken").unwrap();
    for edit in [
        Edit::Install(HELPER),
        Edit::Uninstall,
        Edit::Migrate(HELPER),
    ] {
        assert_eq!(
            apply(&path, edit).unwrap_err().message().to_string(),
            "agy-hooks-unreadable"
        );
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ broken");
}

/// agy's trust list (`trusted_folders`): only `trustedWorkspaces` of the
/// shape agy writes; anything else trusts nothing. A directory is trusted
/// only by its exact entry, symlinks resolved (`/tmp` is `/private/tmp`).
#[test]
fn agy_trusts_only_the_exact_directories_its_settings_list() {
    let root = crate::test_dir::TestDir::new("agy-trusted");
    let canonical = |name: &str| {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::canonicalize(&path).unwrap()
    };
    let (project, child) = (canonical("p"), canonical("p/child"));
    let text = json!({"trustedWorkspaces": [project.display().to_string(), "relative/dir", 7], "other": true})
        .to_string();
    let list = trusted_from(&text).unwrap();
    assert_eq!(
        list,
        vec![project.clone()],
        "relative and non-string entries dropped"
    );
    assert!(trusts(&list, &project.display().to_string()));
    assert!(
        !trusts(&list, &child.display().to_string()),
        "a parent covers no child"
    );
    assert!(!trusts(&list, "p"), "a relative path is never trusted");
    // the same directory through a link
    let link = root.join("link");
    std::os::unix::fs::symlink(&project, &link).unwrap();
    assert!(trusts(&list, &link.display().to_string()));
    // not agy's shape: nothing is trusted
    for bad in ["", "[]", "{\"trustedWorkspaces\": \"/\"}", "{}", "not json"] {
        assert!(trusted_from(bad).is_none(), "{bad}");
    }
}
