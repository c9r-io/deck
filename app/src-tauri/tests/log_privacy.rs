//! Static tripwire: sensitive content must never flow into app.log again.
//! These patterns each correspond to a logging call that once shipped user
//! keystrokes, IME text, command lines or prompt contents (purged in the
//! privacy pass). If one reappears, this test names the offending pattern.

use std::path::PathBuf;

fn read(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

/// All frontend module sources concatenated (the ES-module split moved the
/// script out of index.html).
fn frontend_js() -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/js");
    let mut out = String::new();
    for e in std::fs::read_dir(&dir).expect("ui/js") {
        let p = e.unwrap().path();
        if p.extension().is_some_and(|x| x == "js") {
            out.push_str(&std::fs::read_to_string(&p).unwrap());
            out.push('\n');
        }
    }
    assert!(!out.is_empty(), "no frontend modules found");
    out
}

#[test]
fn frontend_logs_carry_no_user_content() {
    let ui = frontend_js();
    let forbidden: &[(&str, &str)] = &[
        (
            "${e.key}",
            "keydown logging must not include the pressed key text",
        ),
        (
            "${e.data}",
            "IME composition logging must not include composed text",
        ),
        (
            "lineBuf.slice",
            "input-mirror logging must not include the typed line",
        ),
        (
            "cmd.slice(0",
            "command recording must not log the command text",
        ),
    ];
    for (pat, why) in forbidden {
        for line in ui.lines() {
            if (line.contains("ulog(") || line.contains("dlog(")) && line.contains(pat) {
                panic!("sensitive log pattern {pat:?} in ui/index.html: {why}\n  {line}");
            }
        }
    }
}

#[test]
fn backend_logs_carry_no_user_content() {
    let rs = read("src/main.rs");
    for line in rs.lines() {
        if line.contains("applog") && (line.contains("item.text)") || line.contains("&item.text")) {
            panic!("backend log includes prompt text: {line}");
        }
    }
}

/// The verbose diagnostics gate must stay default-off.
#[test]
fn debug_logging_defaults_off() {
    let ui = frontend_js();
    assert!(
        ui.contains("window.__DECK_DEBUG = !!settings.debug"),
        "debug flag must derive from settings.debug"
    );
    assert!(
        ui.contains("debug: false"),
        "settings.debug must default to false"
    );
}
