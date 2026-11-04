//! card_title.rs — "Draft title": a title for one card, for the user to
//! confirm in its rename field.
//!
//! # Contract
//! - Runs only on the user's click (`card_title_draft`), for one card, and
//!   writes nothing: the draft goes back to the webview, which opens the
//!   card's rename field with it. Only the user's Enter, or an edit and a
//!   click away, renames the card, through the ordinary Board path.
//! - The pane is the card's Signal target (`agent_status::signal_targets`:
//!   the one active pane of its current window), never the first listed.
//! - First source, nothing leaves the Mac: when that pane's CURRENT
//!   foreground generation has an accepted Claude Code interaction
//!   (`agent_status::generation_evidence`), the draft is the title Claude
//!   Code gave its session. Claude Code sets it as the terminal title and
//!   tmux keeps it as `#{pane_title}` (one display-message read); its
//!   leading status mark (an undocumented glyph such as ✳) and the space
//!   after it are dropped, and tmux's default title (the host name) is not
//!   a title. Claude Code writes it from the session's FIRST prompt and does
//!   not update it later (code.claude.com sessions, "Name your sessions").
//! - Otherwise the model: the end of that pane's text (capture-pane, at most
//!   `MAX_TEXT_BYTES`) goes, on this click only, to the provider whose key
//!   is stored (`model_api::draft_title`). The reply is cut to its first
//!   line without quotes or markdown marks, at most `MAX_TITLE_CHARS`.
//! - Neither the text nor the draft is logged or kept. A refusal is a closed
//!   word: `session` (no running card, or a shell-service restart is under
//!   way), `empty` (the pane shows no text), `no-source` (no Claude title and
//!   no key), or one of `model_api`'s (auth, permission, busy, network,
//!   response, keychain, model, refused). app.log gets the source or the
//!   word.
//! - Presentation only (`tests/signal_census.rs`): the Claude evidence picks
//!   where a draft comes from; it grants nothing and starts nothing.

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::tmux::PaneRow;
use serde_json::{json, Value};

/// The most of a pane's text a model is sent: its end.
const MAX_TEXT_BYTES: usize = 8_000;
/// How far back into the pane's history a capture reaches.
const CAPTURE_START: &str = "-200";
/// A draft is cut to this many characters.
const MAX_TITLE_CHARS: usize = 80;
/// One display-message read; the title last, so a tab inside it stays in it.
const TITLE_FORMAT: &str = "#{host}\t#{host_short}\t#{pane_title}";

/// A draft and where it came from (`claude-code`, `anthropic`, `openai`),
/// or a closed word.
type Drafted = Result<(String, &'static str), &'static str>;

/// What a draft reads, injected so the tests need no Deck server or key.
struct World<'a> {
    rows: &'a dyn Fn() -> Result<Vec<PaneRow>, DeckError>,
    claude_now: &'a dyn Fn(&[PaneRow], &str) -> bool,
    tmux: &'a dyn Fn(&[&str]) -> Result<String, DeckError>,
    model: &'a dyn Fn(&str) -> Drafted,
}

/// Whether `session`'s Signal target has an accepted Claude Code
/// interaction in its current foreground generation.
fn claude_now(rows: &[PaneRow], session: &str) -> bool {
    crate::agent_status::generation_evidence(
        rows,
        crate::agent_status::live_generation,
        crate::agent_status::foreground_name,
    )
    .get(session)
    .is_some_and(|evidence| evidence.claude_interaction)
}

/// A draft for `session`'s card.
fn draft_in(world: &World, session: &str) -> Drafted {
    crate::tmux::validate_session_name(session).map_err(|_| "session")?;
    let rows = (world.rows)().map_err(|_| "session")?;
    let targets = crate::agent_status::signal_targets(&rows);
    let pane = targets.get(session).ok_or("session")?.pane_id.clone();
    if (world.claude_now)(&rows, session) {
        let line = (world.tmux)(&["display-message", "-p", "-t", &pane, TITLE_FORMAT]);
        if let Some(title) = line.ok().as_deref().and_then(claude_title) {
            return Ok((title, "claude-code"));
        }
    }
    let captured = (world.tmux)(&["capture-pane", "-p", "-J", "-t", &pane, "-S", CAPTURE_START])
        .map_err(|_| "session")?;
    let text = tail(&captured).ok_or("empty")?;
    let (reply, provider) = (world.model)(text)?;
    Ok((clean(&reply).ok_or("response")?, provider))
}

/// The title Claude Code gave its session, from `host \t host_short \t
/// title`; None while the pane still has tmux's default title.
fn claude_title(line: &str) -> Option<String> {
    let mut fields = line.trim_end_matches(['\r', '\n']).splitn(3, '\t');
    let (host, short, title) = (fields.next()?, fields.next()?, fields.next()?);
    if title.trim().is_empty() || title == host || title == short {
        return None;
    }
    clean(without_status_mark(title))
}

/// Claude Code puts a status mark (an undocumented glyph such as ✳) and a
/// space before its title; a title without that shape is kept whole.
fn without_status_mark(title: &str) -> &str {
    let title = title.trim();
    let mut chars = title.char_indices();
    match (chars.next(), chars.next()) {
        (Some((_, mark)), Some((space, ' '))) if !mark.is_alphanumeric() => {
            title[space + 1..].trim_start()
        }
        _ => title,
    }
}

/// A draft as the rename field gets it: the first non-empty line, without
/// control characters, wrapping quotes or markdown marks, spaces collapsed,
/// cut to `MAX_TITLE_CHARS`; None when nothing is left.
fn clean(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let line: String = line
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let line = line.trim_matches(|c: char| c.is_whitespace() || "\"'`*#「」『』“”‘’".contains(c));
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = line.chars().take(MAX_TITLE_CHARS).collect();
    (!cut.is_empty()).then_some(cut)
}

/// The end of a capture, at most `MAX_TEXT_BYTES` on a character
/// boundary; None when the pane shows no text.
fn tail(captured: &str) -> Option<&str> {
    let text = captured.trim_end();
    if text.trim().is_empty() {
        return None;
    }
    let mut start = text.len().saturating_sub(MAX_TEXT_BYTES);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    Some(&text[start..])
}

/// Draft a title for the card whose tmux session is `session`.
#[tauri::command]
pub(crate) async fn card_title_draft(session: String) -> Result<Value, DeckError> {
    tauri::async_runtime::spawn_blocking(move || {
        let outcome = match crate::session_runtime::activity_guard() {
            Err(_) => Err("session"),
            Ok(_activity) => draft_in(
                &World {
                    rows: &crate::tmux::list_panes,
                    claude_now: &claude_now,
                    tmux: &crate::tmux::tmux,
                    model: &crate::model_api::draft_title,
                },
                &session,
            ),
        };
        match outcome {
            Ok((title, source)) => {
                applog(&format!("[title] drafted ({source})"));
                Ok(json!({ "title": title, "source": source }))
            }
            Err(code) => {
                applog(&format!("[title] draft refused ({code})"));
                Err(DeckError::new(ErrorKind::Other, code))
            }
        }
    })
    .await
    .map_err(|_| DeckError::new(ErrorKind::Other, "worker"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A pane row as tmux lists it: session `name`, pane `%id`, active
    /// flags as given.
    fn row(name: &str, pane: u32, window_active: bool, pane_active: bool) -> PaneRow {
        let flag = |on: bool| if on { "1" } else { "0" };
        crate::tmux::parse_pane_row(&format!(
            "100\t$1\t{name}\t@1\t%{pane}\t{}\t0\t0\t{}\t{}\tzsh\t/dev/ttys00{pane}\t/tmp",
            200 + pane,
            flag(window_active),
            flag(pane_active)
        ))
        .unwrap()
    }

    #[test]
    fn the_status_mark_goes_and_a_draft_is_one_clean_bounded_line() {
        assert_eq!(without_status_mark("✳ 修复登录流程"), "修复登录流程");
        assert_eq!(
            without_status_mark("⠂ Fix the login flow"),
            "Fix the login flow"
        );
        assert_eq!(
            without_status_mark("Fix it"),
            "Fix it",
            "no mark, kept whole"
        );
        assert_eq!(
            without_status_mark("修 复"),
            "修 复",
            "a letter is never a mark"
        );
        assert_eq!(
            clean("\n  \"Fix the login flow\"  \nsecond line"),
            Some("Fix the login flow".into())
        );
        assert_eq!(clean("# **「修复登录」**"), Some("修复登录".into()));
        assert_eq!(clean("a\u{7}b   c"), Some("ab c".into()));
        assert_eq!(clean("  \n \"\" \n"), None);
        assert_eq!(
            clean(&"字".repeat(200)).unwrap().chars().count(),
            MAX_TITLE_CHARS
        );
        // tmux's default title is the host, not a title
        assert_eq!(
            claude_title("mac.local\tmac\t✳ 修复登录流程\n"),
            Some("修复登录流程".into())
        );
        assert_eq!(claude_title("mac.local\tmac\tmac.local"), None);
        assert_eq!(claude_title("mac.local\tmac\tmac"), None);
        assert_eq!(claude_title("mac.local\tmac\t"), None);
        assert_eq!(
            claude_title("mac.local\tmac\t✳ a\tb"),
            Some("a b".into()),
            "a tab stays in the title"
        );
        assert_eq!(claude_title("garbage"), None);
    }

    #[test]
    fn a_tail_is_the_end_of_the_pane_on_a_character_boundary() {
        assert_eq!(tail("  \n\n  "), None);
        assert_eq!(tail("$ make\nok\n\n\n"), Some("$ make\nok"));
        let long = format!("{}{}", "字".repeat(4_000), "end");
        let cut = tail(&long).unwrap();
        assert!(cut.len() <= MAX_TEXT_BYTES && cut.ends_with("end"));
        assert!(cut.starts_with('字'), "never inside a character");
    }

    /// `draft_in` over canned rows, evidence, tmux answers and model.
    struct Fake {
        rows: Vec<PaneRow>,
        claude: bool,
        title_line: Result<String, ()>,
        capture: String,
        model: Drafted,
        asked: RefCell<Vec<Vec<String>>>,
        sent: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                // the session's first pane is not its Signal target
                rows: vec![row("deck-a", 1, true, false), row("deck-a", 2, true, true)],
                claude: false,
                title_line: Ok("mac.local\tmac\t✳ 修复登录流程".into()),
                capture: "$ cargo test\nfailed: login\n".into(),
                model: Ok(("\"Fix the login tests\"".into(), "anthropic")),
                asked: RefCell::new(Vec::new()),
                sent: RefCell::new(Vec::new()),
            }
        }

        fn draft(&self, session: &str) -> Drafted {
            let rows = || Ok(self.rows.clone());
            let claude_now = |_: &[PaneRow], _: &str| self.claude;
            let tmux = |args: &[&str]| {
                self.asked
                    .borrow_mut()
                    .push(args.iter().map(|arg| arg.to_string()).collect());
                match args[0] {
                    "display-message" => self
                        .title_line
                        .clone()
                        .map_err(|_| DeckError::new(ErrorKind::Tmux, "x")),
                    _ => Ok(self.capture.clone()),
                }
            };
            let model = |text: &str| {
                self.sent.borrow_mut().push(text.to_string());
                self.model.clone()
            };
            draft_in(
                &World {
                    rows: &rows,
                    claude_now: &claude_now,
                    tmux: &tmux,
                    model: &model,
                },
                session,
            )
        }
    }

    #[test]
    fn a_claude_card_gets_its_own_title_and_nothing_is_sent() {
        let fake = Fake {
            claude: true,
            ..Fake::new()
        };
        assert_eq!(
            fake.draft("deck-a"),
            Ok(("修复登录流程".into(), "claude-code"))
        );
        assert!(
            fake.sent.borrow().is_empty(),
            "no model, nothing leaves the Mac"
        );
        assert_eq!(
            fake.asked.borrow()[0],
            ["display-message", "-p", "-t", "%2", TITLE_FORMAT],
            "the Signal target pane, not the first one"
        );
    }

    #[test]
    fn any_other_card_sends_the_end_of_its_pane_to_the_model() {
        // not Claude: the title is never read, the model drafts from the text
        let fake = Fake::new();
        assert_eq!(
            fake.draft("deck-a"),
            Ok(("Fix the login tests".into(), "anthropic"))
        );
        assert_eq!(fake.asked.borrow().len(), 1);
        assert_eq!(
            fake.asked.borrow()[0],
            ["capture-pane", "-p", "-J", "-t", "%2", "-S", "-200"]
        );
        assert_eq!(
            fake.sent.borrow().as_slice(),
            ["$ cargo test\nfailed: login"]
        );
        // a Claude card whose pane still has the default title falls back too
        let fake = Fake {
            claude: true,
            title_line: Ok("mac.local\tmac\tmac.local".into()),
            ..Fake::new()
        };
        assert_eq!(fake.draft("deck-a").unwrap().1, "anthropic");
        let fake = Fake {
            claude: true,
            title_line: Err(()),
            ..Fake::new()
        };
        assert_eq!(fake.draft("deck-a").unwrap().1, "anthropic");
    }

    #[test]
    fn a_refusal_is_a_closed_word_and_sends_nothing_it_need_not() {
        // an empty pane: nothing to summarise, nothing sent
        let fake = Fake {
            capture: " \n\n".into(),
            ..Fake::new()
        };
        assert_eq!(fake.draft("deck-a"), Err("empty"));
        assert!(fake.sent.borrow().is_empty());
        // no such card, a malformed name, an ambiguous target
        assert_eq!(Fake::new().draft("deck-b"), Err("session"));
        assert_eq!(Fake::new().draft("bad name;"), Err("session"));
        let fake = Fake {
            rows: vec![row("deck-a", 1, true, true), row("deck-a", 2, true, true)],
            ..Fake::new()
        };
        assert_eq!(
            fake.draft("deck-a"),
            Err("session"),
            "two marked panes: no target"
        );
        // the model's own words pass through; an empty reply is no draft
        let fake = Fake {
            model: Err("no-source"),
            ..Fake::new()
        };
        assert_eq!(fake.draft("deck-a"), Err("no-source"));
        let fake = Fake {
            model: Ok(("  \n".into(), "openai")),
            ..Fake::new()
        };
        assert_eq!(fake.draft("deck-a"), Err("response"));
    }

    /// The format strings against a real tmux: a pane title set the way an
    /// agent sets it comes back through `TITLE_FORMAT`, and the capture
    /// arguments return the pane's text.
    #[test]
    fn the_title_and_capture_reads_work_on_a_real_tmux() {
        let label = format!("deck-test-title-{}", std::process::id());
        let tmux = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries/tmux-aarch64-apple-darwin");
        let run = |args: &[&str]| -> Result<String, DeckError> {
            crate::tmux::captured_output(
                std::process::Command::new(&tmux).args(["-f", "/dev/null", "-L", &label]),
                args,
            )
        };
        struct Gone<'a>(
            &'a dyn Fn(&[&str]) -> Result<String, DeckError>,
            std::path::PathBuf,
        );
        impl Drop for Gone<'_> {
            fn drop(&mut self) {
                let _ = (self.0)(&["kill-server"]);
                let _ = std::fs::remove_file(&self.1);
            }
        }
        run(&[
            "new-session",
            "-d",
            "-s",
            "deck-t",
            "-x",
            "80",
            "-y",
            "10",
            "printf 'hello title\\n'; sleep 30",
        ])
        .unwrap();
        let socket = run(&["display-message", "-p", "#{socket_path}"]).unwrap();
        let _gone = Gone(&run, std::path::PathBuf::from(socket.trim()));
        run(&["select-pane", "-t", "deck-t", "-T", "✳ 修复登录流程"]).unwrap();
        let rows = crate::tmux::list_panes_with(&run).unwrap();
        let world = World {
            rows: &|| Ok(rows.clone()),
            claude_now: &|_, _| true,
            tmux: &run,
            model: &|_| Err("no-source"),
        };
        assert_eq!(
            draft_in(&world, "deck-t"),
            Ok(("修复登录流程".into(), "claude-code"))
        );
        // the default title is the host: the capture is what the model would get
        run(&["select-pane", "-t", "deck-t", "-T", ""]).unwrap();
        let host = run(&["display-message", "-p", "#{host}"]).unwrap();
        run(&["select-pane", "-t", "deck-t", "-T", host.trim()]).unwrap();
        let sent = RefCell::new(String::new());
        let model = |text: &str| {
            *sent.borrow_mut() = text.to_string();
            Ok(("Hello".to_string(), "openai"))
        };
        let world = World {
            model: &model,
            ..world
        };
        assert_eq!(draft_in(&world, "deck-t"), Ok(("Hello".into(), "openai")));
        assert!(sent.borrow().contains("hello title"), "{}", sent.borrow());
    }
}
