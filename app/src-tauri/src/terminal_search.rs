//! Find in a card's terminal (⌘F): tmux copy-mode's own literal search over
//! the pane's history and screen.
//!
//! # Contract
//! - tmux is the search authority, as it is the scrollback authority
//!   (`terminal_scroll.rs`). Deck never reads pane text for a search: a
//!   query goes in, and only integers come back — in copy-mode, a match
//!   present, the match count, whether that count hit tmux's limit, and
//!   whether the pane shows the alternate screen (a full-screen program,
//!   whose history tmux does not hold). The matches are painted by tmux's
//!   own `copy-mode-match-style` / `copy-mode-current-match-style`.
//! - Matching is tmux's `search-*-text`: literal, case-insensitive unless
//!   the query holds an upper-case letter, across soft-wrapped rows.
//! - A query is one argv element, never part of a command string, and is
//!   handed over with `send-keys -F`: every `#` is doubled and the value
//!   always ends in the empty literal `#{l:}`. tmux's argv parser treats a
//!   trailing `;` as a command separator and has its own `\;` escape, so the
//!   suffix guarantees the element never ends in `;`, and `--` keeps a query
//!   that starts with `-` from being read as a flag. The query is bounded
//!   (`QUERY_MAX_BYTES`, mirrored in `ui/test/fixtures/limits.json`) and
//!   holds no control character; it never reaches app.log.
//! - `Start` enters copy-mode (`-e`, as a wheel scroll does, recording the
//!   live cursor row the same way), goes to the bottom and searches
//!   backwards: the newest match. `Older` / `Newer` continue tmux's stored
//!   search (`search-again` / `search-reverse`) while the pane is still in
//!   copy-mode with a match; otherwise they start again. tmux leaves the
//!   cursor at a match's start after a backward step and at its end after a
//!   forward one, so the first step after a change of direction lands on
//!   the same match; `turn` makes that step two.
//! - A search is a view operation like wheel history: it sends no bytes to
//!   the pane's program and so is not behind the MCP terminal-input fence.
//!   Leaving the find bar keeps the view where it is; the scrollback chip,
//!   typing, or scrolling to the bottom returns to the live view.

use crate::error::{DeckError, ErrorKind};
use serde::Serialize;

/// Longest query, in UTF-8 bytes.
pub(crate) const QUERY_MAX_BYTES: usize = 256;

const PROBE_FORMAT: &str = "#{pane_in_mode}\t#{search_present}";
const REPORT_FORMAT: &str =
    "#{pane_in_mode}\t#{search_present}\t#{search_count}\t#{search_count_partial}\t#{alternate_on}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FindStep {
    Start,
    Older,
    Newer,
}

impl FindStep {
    pub(crate) fn parse(word: &str) -> Result<Self, DeckError> {
        match word {
            "start" => Ok(Self::Start),
            "older" => Ok(Self::Older),
            "newer" => Ok(Self::Newer),
            _ => Err(DeckError::new(ErrorKind::Other, "find-step-invalid")),
        }
    }
}

/// The pane's state just before a step, read under the same operation lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FindProbe {
    pub(crate) in_mode: bool,
    pub(crate) search_present: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TerminalSearchResult {
    pub(crate) active: bool,
    pub(crate) present: bool,
    pub(crate) count: u32,
    pub(crate) partial: bool,
    pub(crate) alternate: bool,
}

pub(crate) fn validate_query(query: &str) -> Result<(), DeckError> {
    if query.is_empty() {
        return Err(DeckError::new(ErrorKind::Other, "find-query-empty"));
    }
    if query.len() > QUERY_MAX_BYTES {
        return Err(DeckError::new(ErrorKind::Other, "find-query-long"));
    }
    if query.chars().any(char::is_control) {
        return Err(DeckError::new(ErrorKind::Other, "find-query-invalid"));
    }
    Ok(())
}

/// The `send-keys -F` form of a literal query (see the contract).
fn encode_query(query: &str) -> String {
    format!("{}#{{l:}}", query.replace('#', "##"))
}

pub(crate) fn probe_args(target: &str) -> Vec<String> {
    vec![
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        target.into(),
        PROBE_FORMAT.into(),
    ]
}

pub(crate) fn parse_probe(raw: &str) -> Result<FindProbe, DeckError> {
    let mut fields = raw.trim_end_matches(['\r', '\n']).split('\t');
    let (Some(in_mode), Some(present), None) = (fields.next(), fields.next(), fields.next()) else {
        return Err(DeckError::new(ErrorKind::Other, "find-status-invalid"));
    };
    Ok(FindProbe {
        in_mode: in_mode == "1",
        search_present: present == "1",
    })
}

/// One tmux command list: the step, then the report. `query` must have
/// passed `validate_query`; `target` comes from strict session validation.
pub(crate) fn step_args(
    target: &str,
    query: &str,
    step: FindStep,
    probe: FindProbe,
    turn: bool,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    let mut command = |words: Vec<String>| {
        if !args.is_empty() {
            args.push(";".into());
        }
        args.extend(words);
    };
    let resume = step != FindStep::Start && probe.in_mode && probe.search_present;
    if resume {
        let repeat = if turn { "2" } else { "1" };
        let verb = if step == FindStep::Older {
            "search-again"
        } else {
            "search-reverse"
        };
        command(vec![
            "send-keys".into(),
            "-t".into(),
            target.into(),
            "-X".into(),
            "-N".into(),
            repeat.into(),
            verb.into(),
        ]);
    } else {
        if !probe.in_mode {
            command(vec![
                "set-option".into(),
                "-p".into(),
                "-F".into(),
                "-t".into(),
                target.into(),
                crate::terminal_scroll::CURSOR_ROW_OPTION.into(),
                "#{cursor_y}".into(),
            ]);
            command(vec![
                "copy-mode".into(),
                "-e".into(),
                "-t".into(),
                target.into(),
            ]);
        }
        command(vec![
            "send-keys".into(),
            "-t".into(),
            target.into(),
            "-X".into(),
            "history-bottom".into(),
        ]);
        command(vec![
            "send-keys".into(),
            "-F".into(),
            "-t".into(),
            target.into(),
            "-X".into(),
            "search-backward-text".into(),
            "--".into(),
            encode_query(query),
        ]);
    }
    command(vec![
        "display-message".into(),
        "-p".into(),
        "-t".into(),
        target.into(),
        REPORT_FORMAT.into(),
    ]);
    args
}

pub(crate) fn parse_report(raw: &str) -> Result<TerminalSearchResult, DeckError> {
    let invalid = || DeckError::new(ErrorKind::Other, "find-status-invalid");
    let fields: Vec<&str> = raw.trim_end_matches(['\r', '\n']).split('\t').collect();
    let [in_mode, present, count, partial, alternate] = fields[..] else {
        return Err(invalid());
    };
    let active = in_mode == "1";
    let present = active && present == "1";
    let count = if present {
        count.parse::<u32>().map_err(|_| invalid())?
    } else {
        0
    };
    Ok(TerminalSearchResult {
        active,
        present,
        count,
        partial: present && partial == "1",
        alternate: alternate == "1",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    const OUT: FindProbe = FindProbe {
        in_mode: false,
        search_present: false,
    };
    const FOUND: FindProbe = FindProbe {
        in_mode: true,
        search_present: true,
    };

    #[test]
    fn queries_are_bounded_and_free_of_control_characters() {
        assert!(validate_query("needle").is_ok());
        assert!(validate_query("中文 ; -x #{pane_id}").is_ok());
        assert!(validate_query(&"a".repeat(QUERY_MAX_BYTES)).is_ok());
        assert_eq!(
            validate_query(&"a".repeat(QUERY_MAX_BYTES + 1)).unwrap_err(),
            "find-query-long"
        );
        assert_eq!(validate_query("").unwrap_err(), "find-query-empty");
        for bad in ["a\nb", "a\tb", "\u{1b}[31m", "a\u{7f}"] {
            assert_eq!(validate_query(bad).unwrap_err(), "find-query-invalid");
        }
        assert_eq!(FindStep::parse("older").unwrap(), FindStep::Older);
        assert_eq!(
            FindStep::parse("sideways").unwrap_err(),
            "find-step-invalid"
        );
    }

    #[test]
    fn a_query_is_one_element_that_never_ends_in_a_separator() {
        for query in ["a;", ";", "a\\;", "-x", "#{pane_id}", "--"] {
            let args = step_args("=deck-c:", query, FindStep::Start, OUT, false);
            let at = args.iter().position(|arg| arg == "--").unwrap();
            let element = &args[at + 1];
            assert!(element.ends_with("#{l:}"), "{query}");
            assert!(!element.ends_with(';'));
            assert_eq!(args[at + 2], ";", "the report follows as its own command");
            let separators = args.iter().filter(|arg| arg.as_str() == ";").count();
            assert_eq!(
                separators, 4,
                "{query}: set, copy-mode, bottom, search, report"
            );
        }
        assert_eq!(encode_query("a#b"), "a##b#{l:}");
    }

    #[test]
    fn start_enters_copy_mode_like_a_wheel_scroll_and_steps_resume_tmux_search() {
        let start = step_args("=deck-c:", "q", FindStep::Start, OUT, false).join(" ");
        assert!(start.starts_with(&format!(
            "set-option -p -F -t =deck-c: {} #{{cursor_y}} ; copy-mode -e -t =deck-c:",
            crate::terminal_scroll::CURSOR_ROW_OPTION
        )));
        assert!(start.contains(
            "history-bottom ; send-keys -F -t =deck-c: -X search-backward-text -- q#{l:}"
        ));

        let again = step_args("=deck-c:", "q", FindStep::Start, FOUND, true).join(" ");
        assert!(again.starts_with("send-keys -t =deck-c: -X history-bottom"));

        let older = step_args("=deck-c:", "q", FindStep::Older, FOUND, false);
        assert_eq!(
            older[..7].join(" "),
            "send-keys -t =deck-c: -X -N 1 search-again"
        );
        assert!(
            !older.iter().any(|arg| arg.contains("q#")),
            "a resumed step carries no query"
        );
        let newer = step_args("=deck-c:", "q", FindStep::Newer, FOUND, true);
        assert_eq!(
            newer[..7].join(" "),
            "send-keys -t =deck-c: -X -N 2 search-reverse"
        );

        let lost = FindProbe {
            in_mode: true,
            search_present: false,
        };
        for probe in [OUT, lost] {
            let fresh = step_args("=deck-c:", "q", FindStep::Newer, probe, true).join(" ");
            assert!(fresh.contains("search-backward-text -- q#{l:}"));
        }
        assert!(step_args("=deck-c:", "q", FindStep::Older, FOUND, false)
            .last()
            .unwrap()
            .contains("#{search_count}"));
    }

    #[test]
    fn reports_parse_strictly_and_carry_no_count_outside_a_match() {
        assert_eq!(
            parse_report("1\t1\t17\t0\t0\n").unwrap(),
            TerminalSearchResult {
                active: true,
                present: true,
                count: 17,
                partial: false,
                alternate: false
            }
        );
        let none = parse_report("1\t0\t\t\t1").unwrap();
        assert!(none.active && !none.present && none.alternate);
        assert_eq!(none.count, 0);
        let gone = parse_report("0\t\t\t\t0").unwrap();
        assert!(!gone.active && !gone.present);
        assert!(parse_report("1\t1\t1000\t1\t0").unwrap().partial);
        for bad in ["", "1\t1\t3\t0", "1\t1\t3\t0\t0\textra", "1\t1\tmany\t0\t0"] {
            assert_eq!(parse_report(bad).unwrap_err(), "find-status-invalid");
        }
        assert_eq!(
            parse_probe("1\t0\n").unwrap(),
            FindProbe {
                in_mode: true,
                search_present: false
            }
        );
        assert!(parse_probe("1").is_err());
    }

    /// A private tmux server on its own socket name, never Deck's.
    struct Server(String);
    impl Server {
        fn new(lines: &str) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let server = Self(format!(
                "deck-test-find-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let command = format!("printf '%s\\n' {lines}; exec sleep 30");
            server.run(&[
                "new-session",
                "-d",
                "-s",
                "find",
                "-x",
                "40",
                "-y",
                "8",
                &command,
            ]);
            let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !server
                .run(&["capture-pane", "-p", "-t", "find:"])
                .contains("END")
            {
                assert!(std::time::Instant::now() < until, "fixture printed");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            server
        }
        fn raw<S: AsRef<str>>(&self, args: &[S]) -> std::io::Result<std::process::Output> {
            Command::new(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("binaries/tmux-aarch64-apple-darwin"),
            )
            .env("LANG", "en_US.UTF-8")
            .args(["-u", "-f", "/dev/null", "-L", &self.0])
            .args(args.iter().map(AsRef::as_ref))
            .output()
        }
        fn run<S: AsRef<str>>(&self, args: &[S]) -> String {
            let out = self.raw(args).unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).into_owned()
        }
        fn step(&self, query: &str, step: FindStep, turn: bool) -> (TerminalSearchResult, String) {
            let probe = parse_probe(&self.run(&probe_args("find:"))).unwrap();
            let report =
                parse_report(&self.run(&step_args("find:", query, step, probe, turn))).unwrap();
            let line = self.run(&[
                "display-message",
                "-p",
                "-t",
                "find:",
                "#{copy_cursor_line}",
            ]);
            (report, line.trim_end().to_string())
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            let socket = self
                .raw(&["display-message", "-p", "#{socket_path}"])
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_default();
            let _ = self.raw(&["kill-server"]);
            let path = std::path::PathBuf::from(socket);
            if path.file_name().and_then(|name| name.to_str()) == Some(&self.0) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    #[test]
    fn real_tmux_finds_literal_queries_and_steps_through_matches() {
        let filler: String = (1..=40).map(|n| format!("'row {n}' ")).collect();
        let server = Server::new(&format!(
            "'n1 needle' {filler} 'n2 needle' {filler} 'n3 needle' \
             'semi a; z' 'dash -x opt' 'fmt #{{pane_id}} lit' '中文搜索' 'END'"
        ));

        let (first, line) = server.step("needle", FindStep::Start, false);
        assert!(first.active && first.present && !first.alternate);
        assert_eq!((first.count, line.as_str()), (3, "n3 needle"));
        assert_eq!(server.step("needle", FindStep::Older, false).1, "n2 needle");
        assert_eq!(server.step("needle", FindStep::Older, false).1, "n1 needle");
        assert_eq!(
            server.step("needle", FindStep::Newer, true).1,
            "n2 needle",
            "a turn moves"
        );
        assert_eq!(server.step("needle", FindStep::Newer, false).1, "n3 needle");
        assert_eq!(
            server.step("NEEDLE", FindStep::Start, false).0.count,
            0,
            "upper case is exact"
        );

        for (query, expected) in [
            ("a;", "semi a; z"),
            ("-x", "dash -x opt"),
            ("#{pane_id}", "fmt #{pane_id} lit"),
            ("搜索", "中文搜索"),
        ] {
            let (found, line) = server.step(query, FindStep::Start, false);
            assert_eq!((found.count, line.as_str()), (1, expected), "{query}");
        }
        let (missing, _) = server.step("a; kill-server", FindStep::Start, false);
        assert!(
            missing.active && !missing.present,
            "a literal, never a command"
        );

        server.run(&["send-keys", "-t", "find:", "-X", "cancel"]);
        let (lost, line) = server.step("needle", FindStep::Older, false);
        assert_eq!(
            (lost.count, line.as_str()),
            (3, "n3 needle"),
            "a lost search starts again"
        );
    }
}
