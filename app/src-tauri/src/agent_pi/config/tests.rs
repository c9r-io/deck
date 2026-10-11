use super::*;
use std::os::unix::fs::PermissionsExt;

/// One throwaway layout: Pi's agent directory and two bundles that hold the
/// observer's files, the first being "this install".
struct Site {
    root: PathBuf,
    place: Place,
}

impl Site {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("deck-pi-config-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bundles = [
            root.join("Applications/deck.app"),
            root.join("home/Applications/deck.app"),
        ];
        for bundle in &bundles {
            let dir = bundle.join(RESOURCE_DIR);
            std::fs::create_dir_all(&dir).unwrap();
            for (name, content) in RESOURCES {
                std::fs::write(dir.join(name), content).unwrap();
            }
        }
        std::fs::create_dir_all(root.join("agent")).unwrap();
        Self {
            place: Place {
                settings: root.join("agent/settings.json"),
                bundle: Some(bundles[0].clone()),
                owned: bundles.to_vec(),
            },
            root,
        }
    }

    fn entry(&self) -> String {
        entry_path(&self.place.owned[0], Variant::Signal).unwrap()
    }

    fn other_entry(&self) -> String {
        entry_path(&self.place.owned[1], Variant::Signal).unwrap()
    }

    fn write(&self, text: &str) {
        std::fs::write(&self.place.settings, text).unwrap();
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.place.settings).unwrap()
    }

    fn value(&self) -> Value {
        serde_json::from_str(&self.text()).unwrap()
    }

    fn set(&self, enable: bool) -> Result<bool, DeckError> {
        apply(&self.place, Edit::Set(Capability::Signal, enable))
    }

    fn refused(&self, edit: Edit) -> String {
        apply(&self.place, edit).unwrap_err().message().to_string()
    }

    fn status(&self) -> Status {
        status_in(&self.place, |_| false)
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Documents as their owners might write them.
fn layouts() -> Vec<(&'static str, String)> {
    vec![
        ("compact", r#"{"theme":"dark","retry":{"enabled":true}}"#.into()),
        ("pretty", "{\n  \"theme\": \"dark\",\n  \"quietStartup\": true\n}\n".into()),
        ("tabs", "{\n\t\"theme\": \"dark\",\n\t\"n\": 1.0e3\n}\n".into()),
        ("crlf", "{\r\n  \"theme\": \"dark\",\r\n  \"n\": 2\r\n}\r\n".into()),
        ("empty", "{}".into()),
        (
            "array",
            "{\n  \"extensions\": [\n    \"-builtin:mcp\",\n    \"~/mine/ext.ts\"\n  ],\n  \"theme\": \"dark\"\n}\n"
                .into(),
        ),
        (
            "array-inline",
            r#"{ "extensions": ["+./a.ts", "!**/skip/*.ts"], "packages": ["npm:x"] }"#.into(),
        ),
        (
            "escapes",
            "{\n    \"note\": \"caf\\u00e9 \\\" \\\\ \\n\",\n    \"extensions\": [ \"x y.ts\" ],\n    \"deep\": {\"a\": [1, {\"b\": null}]}\n}"
                .into(),
        ),
    ]
}

#[test]
fn enabling_and_disabling_returns_every_layout_to_its_bytes() {
    for (name, original) in layouts() {
        let site = Site::new(&format!("round-{name}"));
        site.write(&original);
        let before: Value = serde_json::from_str(&original).unwrap();
        assert_eq!(site.status().installation, Installation::Absent, "{name}");

        assert!(site.set(true).unwrap(), "{name}");
        let enabled = site.text();
        let value = site.value();
        // the decided document: the same, with Deck's one entry at the end
        let mut expected = before.clone();
        let list = expected
            .as_object_mut()
            .unwrap()
            .entry("extensions")
            .or_insert_with(|| serde_json::json!([]));
        list.as_array_mut().unwrap().push(site.entry().into());
        assert_eq!(value, expected, "{name}");
        assert_eq!(enabled.matches(&site.entry()).count(), 1, "{name}");
        assert_eq!(
            site.status(),
            Status {
                signal: true,
                installation: Installation::Current
            },
            "{name}"
        );
        assert_eq!(
            enabled_in(&site.place),
            Capabilities {
                signal: true,
                lifecycle: false
            },
            "{name}"
        );
        // asked again: nothing is written
        assert!(!site.set(true).unwrap(), "{name}");
        assert_eq!(site.text(), enabled, "{name}");

        assert!(site.set(false).unwrap(), "{name}");
        assert_eq!(site.text(), original, "{name}: back to the owner's bytes");
        assert!(!site.set(false).unwrap(), "{name}");
        assert_eq!(enabled_in(&site.place), Capabilities::NONE, "{name}");
    }
}

#[test]
fn everything_outside_the_array_keeps_its_bytes_while_enabled() {
    let site = Site::new("outside");
    let original = "{\n  \"packages\": [ \"npm:@me/pkg\", {\"source\": \"git:x\", \"extensions\": [\"a.ts\"]} ],\n  \"extensions\": [\n    \"-builtin:mcp\",\n    \"/Users/me/ext/mine.ts\"\n  ],\n  \"défaut\": \"\\u00e9\",\n  \"n\": 1.50\n}\n";
    site.write(original);
    site.set(true).unwrap();
    let enabled = site.text();
    let open = original.find("\"extensions\": [\n").unwrap();
    let close = original[open..].find(']').unwrap() + open;
    assert!(enabled.starts_with(&original[..open]));
    assert!(enabled.ends_with(&original[close..]));
    // the user's own entries are where they were, as they were written
    assert!(enabled.contains("    \"-builtin:mcp\",\n    \"/Users/me/ext/mine.ts\",\n"));
    // a nested `extensions` of a package is not the settings' array
    assert!(enabled.contains("{\"source\": \"git:x\", \"extensions\": [\"a.ts\"]}"));
    // an empty array the user wrote is theirs until Deck's entry empties it
    let site = Site::new("empty-array");
    site.write("{\n  \"extensions\": []\n}\n");
    site.set(true).unwrap();
    assert_eq!(
        site.value()["extensions"],
        serde_json::json!([site.entry()])
    );
    site.set(false).unwrap();
    // what Deck's entry alone populated leaves no husk; an object that this
    // empties is written `{}`, whatever stood between its braces before
    assert_eq!(site.text(), "{}\n");
}

#[test]
fn a_file_that_is_not_deck_s_to_repair_is_never_written() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("not-json", b"{ theme: dark }".to_vec()),
        ("truncated", b"{\"extensions\": [".to_vec()),
        ("bom", b"\xef\xbb\xbf{}".to_vec()),
        ("not-utf8", b"{\"a\": \"\xff\"}".to_vec()),
        ("array-root", b"[]".to_vec()),
        (
            "repeated-key",
            b"{\"extensions\": [], \"extensions\": []}".to_vec(),
        ),
        (
            "repeated-nested",
            b"{\"a\": {\"k\": 1, \"k\": 2}, \"extensions\": []}".to_vec(),
        ),
        (
            "repeated-in-list",
            b"{\"a\": [{\"k\": 1, \"k\": 2}]}".to_vec(),
        ),
        ("not-a-list", b"{\"extensions\": \"x.ts\"}".to_vec()),
        (
            "object-list",
            b"{\"extensions\": {\"0\": \"x.ts\"}}".to_vec(),
        ),
        ("non-string", b"{\"extensions\": [\"x.ts\", 7]}".to_vec()),
        ("null-entry", b"{\"extensions\": [null]}".to_vec()),
        ("trailing", b"{} {}".to_vec()),
    ];
    for (name, bytes) in cases {
        let site = Site::new(&format!("bad-{name}"));
        std::fs::write(&site.place.settings, &bytes).unwrap();
        for edit in [
            Edit::Set(Capability::Signal, true),
            Edit::Set(Capability::Signal, false),
            Edit::Migrate,
        ] {
            assert_eq!(site.refused(edit), "pi-config-unreadable", "{name}");
        }
        assert_eq!(
            std::fs::read(&site.place.settings).unwrap(),
            bytes,
            "{name}"
        );
        assert_eq!(
            site.status().installation,
            Installation::Unreadable,
            "{name}"
        );
        assert!(!site.status().signal, "{name}");
        assert_eq!(enabled_in(&site.place), Capabilities::NONE, "{name}");
        // nothing was left beside it either
        let left: Vec<_> = std::fs::read_dir(site.root.join("agent"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["settings.json"], "{name}");
    }
}

#[test]
fn the_file_keeps_its_mode_and_its_link_and_a_new_one_is_private() {
    // an existing file keeps the mode its owner gave it
    let site = Site::new("mode");
    site.write("{}");
    std::fs::set_permissions(&site.place.settings, std::fs::Permissions::from_mode(0o644)).unwrap();
    site.set(true).unwrap();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&site.place.settings), 0o644);

    // a symlinked settings file stays a link; its target gets the bytes
    let site = Site::new("link");
    let dotfiles = site.root.join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("pi.json"), "{\n  \"theme\": \"x\"\n}\n").unwrap();
    std::os::unix::fs::symlink(dotfiles.join("pi.json"), &site.place.settings).unwrap();
    site.set(true).unwrap();
    assert!(std::fs::symlink_metadata(&site.place.settings)
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(std::fs::read_to_string(dotfiles.join("pi.json"))
        .unwrap()
        .contains(&site.entry()));
    site.set(false).unwrap();
    assert_eq!(
        std::fs::read_to_string(dotfiles.join("pi.json")).unwrap(),
        "{\n  \"theme\": \"x\"\n}\n"
    );

    // no file yet: a private one is made, holding the entry and nothing else
    let site = Site::new("new");
    assert_eq!(site.status().installation, Installation::Absent);
    assert!(!site.set(false).unwrap(), "nothing to remove, nothing made");
    assert!(!site.place.settings.exists());
    site.set(true).unwrap();
    assert_eq!(mode(&site.place.settings), 0o600);
    assert_eq!(
        site.value(),
        serde_json::json!({ "extensions": [site.entry()] })
    );

    // no agent directory: Pi never ran here, and nothing is created
    let site = Site::new("never");
    std::fs::remove_dir_all(site.root.join("agent")).unwrap();
    let error = site.set(true).unwrap_err();
    assert_eq!(
        (error.kind(), error.message()),
        (ErrorKind::Missing, "pi-never-ran")
    );
    assert!(!site.root.join("agent").exists());
    assert_eq!(site.status().installation, Installation::Absent);
}

#[test]
fn only_an_exact_path_of_a_stable_bundle_is_deck_s() {
    let site = Site::new("owned");
    let own = site.entry();
    let lookalikes = [
        format!("{own}.bak"),
        own.replace("deck.app", "deck.app.evil"),
        own.replace("/Contents/", "/Contents/../Contents/"),
        "signal.ts".to_string(),
        "./Contents/Resources/pi/signal.ts".to_string(),
        format!("+{own}"),
        format!("!{own}"),
        "/opt/other/deck.app/Contents/Resources/pi/signal.ts".to_string(),
        own.replace("signal.ts", "observer.ts"),
    ];
    for entry in &lookalikes {
        assert_eq!(owned_variant(entry, &site.place), None, "{entry}");
    }
    assert_eq!(owned_variant(&own, &site.place), Some(Variant::Signal));
    assert_eq!(
        owned_variant(&site.other_entry(), &site.place),
        Some(Variant::Signal)
    );
    // none of them is counted, touched or removed
    let document = serde_json::json!({ "extensions": lookalikes });
    let original = format!("{}\n", serde_json::to_string_pretty(&document).unwrap());
    site.write(&original);
    assert_eq!(site.status().installation, Installation::Absent);
    assert!(!site.set(false).unwrap());
    site.set(true).unwrap();
    site.set(false).unwrap();
    assert_eq!(site.text(), original);
}

#[test]
fn the_user_s_own_disabling_entry_wins() {
    let site = Site::new("disabled");
    let original = format!(
        "{{\n  \"extensions\": [\n    \"-{}\"\n  ]\n}}\n",
        site.entry()
    );
    site.write(&original);
    assert_eq!(
        site.refused(Edit::Set(Capability::Signal, true)),
        "pi-config-conflict"
    );
    assert!(!apply(&site.place, Edit::Migrate).unwrap());
    assert_eq!(site.text(), original);
    assert_eq!(
        site.status(),
        Status::default(),
        "off, and nothing to resolve"
    );
    assert_eq!(enabled_in(&site.place), Capabilities::NONE);

    // beside an entry of Deck's: a conflict, nothing admitted, nothing re-enabled
    let both = format!(
        "{{\n  \"extensions\": [\n    \"{}\",\n    \"-{}\"\n  ]\n}}\n",
        site.entry(),
        site.other_entry()
    );
    site.write(&both);
    assert_eq!(
        site.status(),
        Status {
            signal: true,
            installation: Installation::Conflict
        }
    );
    assert_eq!(enabled_in(&site.place), Capabilities::NONE);
    assert!(!apply(&site.place, Edit::Migrate).unwrap());
    assert_eq!(site.text(), both);
    // turning the switch off takes Deck's entry out and leaves the user's
    site.set(false).unwrap();
    assert_eq!(
        site.value(),
        serde_json::json!({ "extensions": [format!("-{}", site.other_entry())] })
    );
}

#[test]
fn the_boot_migration_moves_an_entry_and_enrolls_nobody() {
    // an entry of the other stable location moves to this install, in place
    let site = Site::new("moved");
    site.write(&format!(
        "{{\n  \"extensions\": [\n    \"first.ts\",\n    \"{}\",\n    \"last.ts\"\n  ]\n}}\n",
        site.other_entry()
    ));
    assert_eq!(site.status().installation, Installation::Conflict);
    assert_eq!(
        enabled_in(&site.place),
        Capabilities {
            signal: true,
            lifecycle: false
        },
        "one entry of Deck's is one observer, wherever it points"
    );
    assert!(apply(&site.place, Edit::Migrate).unwrap());
    assert_eq!(
        site.value(),
        serde_json::json!({ "extensions": ["first.ts", site.entry(), "last.ts"] })
    );
    assert_eq!(site.status().installation, Installation::Current);
    assert!(
        !apply(&site.place, Edit::Migrate).unwrap(),
        "already current"
    );

    // two entries would be two observers: they become the one
    let site = Site::new("merged");
    site.write(
        &serde_json::json!({ "extensions": [site.other_entry(), "x.ts", site.entry()] })
            .to_string(),
    );
    assert_eq!(site.status().installation, Installation::Conflict);
    assert_eq!(
        enabled_in(&site.place),
        Capabilities::NONE,
        "no observer of two"
    );
    assert!(apply(&site.place, Edit::Migrate).unwrap());
    assert_eq!(
        site.value(),
        serde_json::json!({ "extensions": [site.entry(), "x.ts"] })
    );
    // a switch resolves the same conflict
    let site = Site::new("merged-by-switch");
    site.write(
        &serde_json::json!({ "extensions": [site.other_entry(), site.entry()] }).to_string(),
    );
    site.set(true).unwrap();
    assert_eq!(
        site.value(),
        serde_json::json!({ "extensions": [site.entry()] })
    );

    // nobody is enrolled: no entry, no file, an unrelated file
    for (name, original) in [
        ("none", Some("{\n  \"extensions\": [\"x.ts\"]\n}\n")),
        ("no-file", None),
    ] {
        let site = Site::new(&format!("enroll-{name}"));
        if let Some(original) = original {
            site.write(original);
        }
        assert!(!apply(&site.place, Edit::Migrate).unwrap());
        assert_eq!(
            std::fs::read_to_string(&site.place.settings)
                .ok()
                .as_deref(),
            original
        );
    }
}

#[test]
fn a_bundle_that_is_not_a_stable_install_writes_nothing() {
    let mut site = Site::new("location");
    let configured = format!(
        "{{\n  \"extensions\": [\n    \"{}\"\n  ]\n}}\n",
        site.entry()
    );
    site.write(&configured);
    site.place.bundle = None;
    for edit in [
        Edit::Set(Capability::Signal, true),
        Edit::Set(Capability::Signal, false),
        Edit::Migrate,
    ] {
        assert_eq!(site.refused(edit), "pi-config-unsupported-location");
    }
    assert_eq!(site.text(), configured);
    // the file is still read: the switch shows what Pi will load
    assert_eq!(
        site.status(),
        Status {
            signal: true,
            installation: Installation::UnsupportedLocation
        }
    );
    assert_eq!(
        enabled_in(&site.place),
        Capabilities {
            signal: true,
            lifecycle: false
        }
    );
    site.write("{}");
    assert_eq!(
        site.status(),
        Status {
            signal: false,
            installation: Installation::UnsupportedLocation
        }
    );
}

#[test]
fn a_path_is_written_only_for_this_build_s_own_files() {
    let site = Site::new("resources");
    site.write("{}");
    let file = site.place.owned[0].join(RESOURCE_DIR).join("observer.ts");
    let original = std::fs::read(&file).unwrap();
    // another build's file, a missing file, a link in its place
    std::fs::write(&file, [&original[..], b"\n// changed"].concat()).unwrap();
    assert_eq!(
        site.refused(Edit::Set(Capability::Signal, true)),
        "pi-resource-missing"
    );
    std::fs::remove_file(&file).unwrap();
    assert_eq!(
        site.refused(Edit::Set(Capability::Signal, true)),
        "pi-resource-missing"
    );
    let elsewhere = site.root.join("observer.ts");
    std::fs::write(&elsewhere, &original).unwrap();
    std::os::unix::fs::symlink(&elsewhere, &file).unwrap();
    assert_eq!(
        site.refused(Edit::Set(Capability::Signal, true)),
        "pi-resource-missing"
    );
    assert_eq!(site.text(), "{}");
    std::fs::remove_file(&file).unwrap();
    std::fs::write(&file, &original).unwrap();
    assert!(site.set(true).unwrap());
    // turning it off needs no file to be there
    std::fs::remove_file(&file).unwrap();
    assert!(site.set(false).unwrap());
    assert_eq!(site.text(), "{}");
    // what the build carries is what the checkout holds, entry and all
    for (name, content) in RESOURCES {
        assert!(content.contains("Deck"), "{name}");
        assert!(!content.is_empty());
    }
    assert_eq!(
        RESOURCES.map(|(name, _)| name),
        ["observer.ts", "signal.ts"],
        "the closed list of bundled observer files"
    );
}

#[test]
fn another_writer_s_change_is_taken_once_and_never_overwritten_twice() {
    // one change while the edit was computed: the edit is made again from it
    let site = Site::new("race-once");
    site.write("{\n  \"theme\": \"dark\"\n}\n");
    let calls = std::cell::Cell::new(0);
    let written = apply_between(&site.place, Edit::Set(Capability::Signal, true), &|| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            site.write("{\n  \"theme\": \"light\",\n  \"lastChangelogVersion\": \"1.1.0\"\n}\n");
        }
    });
    assert!(written.unwrap());
    assert_eq!(calls.get(), 2);
    assert_eq!(
        site.value(),
        serde_json::json!({
            "theme": "light", "lastChangelogVersion": "1.1.0", "extensions": [site.entry()]
        }),
        "the other writer's change is in the result"
    );

    // a second change: the write is refused and the other writer's file stands
    let site = Site::new("race-twice");
    site.write("{}");
    let turn = std::cell::Cell::new(0);
    let refused = apply_between(&site.place, Edit::Set(Capability::Signal, true), &|| {
        turn.set(turn.get() + 1);
        site.write(&format!("{{\"turn\": {}}}", turn.get()));
    });
    assert_eq!(refused.unwrap_err().message(), "pi-config-busy");
    assert_eq!(site.text(), "{\"turn\": 2}");

    // a file that appears, or goes, meanwhile is a change like any other
    let site = Site::new("race-appears");
    let appeared = std::cell::Cell::new(false);
    apply_between(&site.place, Edit::Set(Capability::Signal, true), &|| {
        if !appeared.replace(true) {
            site.write("{\"mine\": 1}");
        }
    })
    .unwrap();
    assert_eq!(
        site.value(),
        serde_json::json!({ "mine": 1, "extensions": [site.entry()] })
    );
}

#[test]
fn the_status_names_what_the_one_entry_is() {
    let site = Site::new("status");
    assert_eq!(site.status(), Status::default());
    site.set(true).unwrap();
    assert_eq!(site.status().installation, Installation::Current);
    // a connected observer with other capabilities: that Pi must reload
    assert_eq!(
        status_in(&site.place, |configured| {
            assert_eq!(
                configured,
                Capabilities {
                    signal: true,
                    lifecycle: false
                }
            );
            true
        })
        .installation,
        Installation::NeedsReload
    );
    // the closed words the Settings page is told
    for (state, word) in [
        (Installation::Absent, "absent"),
        (Installation::Current, "current"),
        (Installation::NeedsReload, "needs-reload"),
        (Installation::Conflict, "conflict"),
        (Installation::Unreadable, "unreadable"),
        (Installation::UnsupportedLocation, "unsupported-location"),
    ] {
        assert_eq!(serde_json::to_value(state).unwrap(), word);
    }
}

#[test]
fn the_agent_directory_follows_pi_s_own_rule() {
    let home = Path::new("/Users/u");
    let dir = |value: Option<&str>| agent_directory(home, value.map(std::ffi::OsString::from));
    assert_eq!(dir(None), Some(PathBuf::from("/Users/u/.pi/agent")));
    assert_eq!(dir(Some("")), Some(PathBuf::from("/Users/u/.pi/agent")));
    assert_eq!(dir(Some("/opt/pi")), Some(PathBuf::from("/opt/pi")));
    assert_eq!(dir(Some("~")), Some(PathBuf::from("/Users/u")));
    assert_eq!(
        dir(Some("~/cfg/pi")),
        Some(PathBuf::from("/Users/u/cfg/pi"))
    );
    // a relative directory is each Pi's own: Deck names none
    assert_eq!(dir(Some("cfg/pi")), None);
    assert_eq!(dir(Some("~other/pi")), None);
}

#[test]
fn a_repeated_key_is_refused_wherever_it_is() {
    assert!(strict(r#"{"a": 1, "b": {"c": [1, 2, {"d": null}]}, "e": "x"}"#).is_some());
    assert!(strict(r#"{"a": 1, "a": 2}"#).is_none());
    assert!(strict(r#"{"a": {"b": 1, "b": 1}}"#).is_none());
    assert!(strict(r#"[{"a": 1, "a": 2}]"#).is_none());
    assert!(strict(r#"{"a": 1} x"#).is_none());
    // the same value serde reads, for every document it accepts
    for (_, text) in layouts() {
        assert_eq!(strict(&text), serde_json::from_str::<Value>(&text).ok());
    }
}
