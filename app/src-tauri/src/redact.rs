//! Log redaction: the net UNDER every log call site.
//!
//! Log lines are written by deck itself, so the call sites are the primary
//! guarantee: no prompt, command, PTY byte, clipboard/IME content, raw error
//! Display text or raw session name is ever formatted into one. `sanitize_log`
//! runs on every line on its way to disk (and again on its way into an
//! export), so a future call site — or a log written by an older deck —
//! cannot leak an absolute path, a URL or a token shape. `redact_credentials`
//! is the narrower policy shell recovery uses: paths and links survive,
//! obvious secret values do not. Pure string scanning, no dependencies.
//! Quoted assignments consume escaped delimiters and spaces; Authorization
//! consumes the complete header value. Cached token/scheme lookahead keeps
//! both policies linear even on long terminal lines without whitespace.

const SECRET_PREFIXES: &[&str] = &[
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "sk_live_",
    "sk_test_",
    "sk-",
    "pk_live_",
    "rk_live_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xoxs-",
    "xapp-",
    "AKIA",
    "ASIA",
    "AIza",
    "eyJ", // JWT header
];

fn value_end(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len()
        && !bytes[i].is_ascii_whitespace()
        && !matches!(bytes[i], b'"' | b'\'' | b')' | b']' | b'}' | b',' | b';')
    {
        i += 1;
    }
    i
}

fn token_end(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'_' | b'-')) {
        i += 1;
    }
    i
}

/// Look ahead at most once per token/scheme run, while still visiting each
/// position to recognize embedded secret prefixes (e.g. `prefix_sk-...`).
#[derive(Default)]
struct Scan {
    end: usize,
    last_digit: Option<usize>,
    last_alpha: Option<usize>,
    scheme_end: usize,
    previous: usize,
    escape: Option<usize>,
    #[cfg(test)]
    inspected: usize,
}

impl Scan {
    fn at(&mut self, line: &str, start: usize) {
        if let Some(offset) = line[self.previous..start].rfind('\x1b') {
            self.escape = Some(self.previous + offset);
        }
        self.previous = start;
        if start < self.end {
            return;
        }
        self.end = start;
        self.last_digit = None;
        self.last_alpha = None;
        for b in line[start..].bytes() {
            #[cfg(test)]
            {
                self.inspected += 1;
            }
            if !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) {
                break;
            }
            if b.is_ascii_digit() {
                self.last_digit = Some(self.end);
            }
            if b.is_ascii_alphabetic() {
                self.last_alpha = Some(self.end);
            }
            self.end += 1;
        }
    }

    fn opaque(&self, start: usize) -> bool {
        self.end.saturating_sub(start) >= 24
            && self.last_digit.is_some_and(|i| i >= start)
            && self.last_alpha.is_some_and(|i| i >= start)
    }

    fn scheme_end(&mut self, line: &str, start: usize) -> usize {
        if start >= self.scheme_end {
            self.scheme_end = start;
            for b in line[start..].bytes() {
                #[cfg(test)]
                {
                    self.inspected += 1;
                }
                if !(b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
                    break;
                }
                self.scheme_end += 1;
            }
        }
        self.scheme_end
    }
}

fn quoted_end(bytes: &[u8], mut i: usize, close: u8) -> usize {
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b if b == close => break,
            _ => i += 1,
        }
    }
    i // an unfinished quote is sensitive through the end of the input
}

fn credential_assignment(line: &str, start: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    if start > 0 {
        let prev = bytes[start - 1];
        if prev.is_ascii_alphanumeric() || matches!(prev, b'_' | b'-') {
            return None;
        }
    }
    let key_end = token_end(bytes, start);
    if key_end == start {
        return None;
    }
    let key = line[start..key_end].to_ascii_lowercase();
    let sensitive_key = matches!(
        key.as_str(),
        "token"
            | "password"
            | "passwd"
            | "secret"
            | "api_key"
            | "apikey"
            | "access_key"
            | "authorization"
            | "proxy-authorization"
            | "credential"
            | "cookie"
            | "private_key"
            | "database_url"
    ) || [
        "_token",
        "_password",
        "_passwd",
        "_secret",
        "_api_key",
        "_apikey",
        "_access_key",
        "_private_key",
        "_credential",
        "_cookie",
    ]
    .iter()
    .any(|suffix| key.ends_with(suffix));
    if !sensitive_key {
        return None;
    }
    let mut i = key_end;
    if bytes.get(i).is_some_and(|b| matches!(b, b'"' | b'\'')) {
        i += 1; // closing quote around a JSON key
    }
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if !bytes.get(i).is_some_and(|b| matches!(b, b'=' | b':')) {
        return None;
    }
    i += 1;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    if let Some(quote @ (b'"' | b'\'')) = bytes.get(i).copied() {
        i += 1;
        let end = quoted_end(bytes, i, quote);
        return (end > i).then_some((i, end));
    }
    if key == "authorization" || key == "proxy-authorization" {
        // Basic, Bearer, Digest and future schemes all carry credentials.
        // An outer shell quote ends the header; quotes INSIDE a raw Digest
        // header do not. Otherwise redact through the header line boundary.
        let outer = start
            .checked_sub(1)
            .and_then(|n| bytes.get(n))
            .copied()
            .filter(|b| matches!(b, b'"' | b'\''));
        let limit = outer.map_or(bytes.len(), |quote| quoted_end(bytes, i, quote));
        let end = bytes[i..limit]
            .iter()
            .position(|b| matches!(b, b'\r' | b'\n'))
            .map_or(limit, |n| i + n);
        return (end > i).then_some((i, end));
    }
    if bytes.get(i).is_some_and(|b| matches!(b, b'(' | b'[')) {
        let close = if bytes[i] == b'(' { b')' } else { b']' };
        i += 1;
        let end = quoted_end(bytes, i, close);
        return (end > i).then_some((i, end));
    }
    let mut end = value_end(bytes, i);
    if line[i..end].eq_ignore_ascii_case("bearer") {
        let mut j = end;
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        end = value_end(bytes, j);
    }
    (end > i).then_some((i, end))
}

/// A canonical UUID (8-4-4-4-12 hex) standing as a WHOLE token. Shell
/// recovery keeps these: an agent session identifier is exactly what a
/// restored transcript exists to hand back (`claude --resume <id>`, a
/// `.../<id>.jsonl` path), and the opaque-token rule below — 24+ chars with
/// digits and letters — swallowed every one of them. A UUID-shaped secret
/// assigned to a credential key is still redacted by
/// `credential_assignment`, which runs first.
fn uuid_token_end(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    if start > 0
        && (bytes[start - 1].is_ascii_alphanumeric() || matches!(bytes[start - 1], b'_' | b'-'))
    {
        return None;
    }
    let end = token_end(bytes, start);
    let run = &line[start..end];
    let groups: Vec<&str> = run.split('-').collect();
    let shaped = groups.len() == 5
        && groups.iter().map(|g| g.len()).eq([8usize, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.bytes().all(|c| c.is_ascii_hexdigit()));
    shaped.then_some(end)
}

fn credential_token_end(line: &str, start: usize, scan: &Scan) -> Option<usize> {
    let bytes = line.as_bytes();
    let rest = &line[start..];
    if SECRET_PREFIXES
        .iter()
        .any(|prefix| rest.starts_with(prefix))
    {
        return Some(value_end(bytes, start));
    }
    scan.opaque(start).then_some(scan.end)
}

/// Redact likely credentials without removing ordinary paths and URLs. Shell
/// recovery uses this narrower policy because its output remains useful only
/// if directories and links survive, while obvious secret values must not.
pub(crate) fn redact_credentials(line: &str) -> String {
    redact(line, true, &mut Scan::default())
}

fn redact(line: &str, credentials_only: bool, scan: &mut Scan) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        scan.at(line, i);
        if let Some((value_start, end)) = credential_assignment(line, i) {
            out.push_str(&line[i..value_start]);
            out.push_str("<redacted>");
            i = end;
            continue;
        }
        if credentials_only {
            if let Some(end) = uuid_token_end(line, i) {
                out.push_str(&line[i..end]); // an identifier, not a credential
                i = end;
                continue;
            }
        }
        let end = if credentials_only {
            credential_token_end(line, i, scan)
        } else {
            sensitive_end(line, i, scan)
        };
        if let Some(end) = end {
            out.push_str("<redacted>");
            i = end;
            continue;
        }
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Return the end of a sensitive value beginning exactly at `start`.
fn sensitive_end(line: &str, start: usize, scan: &mut Scan) -> Option<usize> {
    let bytes = line.as_bytes();
    let rest = &line[start..];
    if rest.starts_with("~/") || rest.starts_with('/') {
        let ansi_boundary = scan.escape.is_some_and(|esc| {
            line[esc..start].starts_with("\x1b[") && line[esc..start].ends_with('m')
        });
        let boundary = start == 0
            || bytes[start - 1].is_ascii_whitespace()
            || matches!(
                bytes[start - 1],
                b'=' | b':' | b'"' | b'\'' | b'(' | b'[' | b'{'
            )
            || ansi_boundary;
        if boundary {
            return Some(value_end(bytes, start));
        }
    }
    if SECRET_PREFIXES.iter().any(|p| rest.starts_with(p)) {
        return Some(value_end(bytes, start));
    }
    if rest.starts_with("deck-") {
        let end = scan.end;
        if line[start + 5..end].contains('-') {
            return Some(end);
        }
    }

    // Any RFC-style scheme:// URL, even when attached to JSON/assignment
    // punctuation. Detection starts at the scheme rather than splitting on
    // whitespace, so quotes, colons, equals and ANSI wrappers cannot hide it.
    if bytes[start].is_ascii_alphabetic() {
        let j = scan.scheme_end(line, start);
        if line[j..].starts_with("://") {
            return Some(value_end(bytes, j + 3));
        }
    }

    scan.opaque(start).then_some(scan.end)
}

/// Replace sensitive spans wherever they occur while preserving surrounding
/// diagnostic punctuation and ANSI control sequences.
pub(crate) fn sanitize_log(line: &str) -> String {
    redact(line, false, &mut Scan::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_quoted_and_authorization_values_are_private() {
        for (input, expected) in [
            (
                r#"Authorization: Digest username="alice", response="secret""#,
                "Authorization: <redacted>",
            ),
            (
                r#"curl -H 'Authorization: Digest username="alice", response="secret"' next"#,
                "curl -H 'Authorization: <redacted>' next",
            ),
            (
                r#"PASSWORD="correct horse battery staple""#,
                r#"PASSWORD="<redacted>""#,
            ),
            (
                r#"{"password":"hello, world; [secret]"}"#,
                r#"{"password":"<redacted>"}"#,
            ),
            (
                r#"PASSWORD="escaped \"quote\" and tail" ok"#,
                r#"PASSWORD="<redacted>" ok"#,
            ),
            ("TOKEN='多字节 密码' ok", "TOKEN='<redacted>' ok"),
            ("SECRET='unfinished value", "SECRET='<redacted>"),
            (
                "Authorization: Basic dXNlcjpwYXNz",
                "Authorization: <redacted>",
            ),
            (
                "authorization: Bearer short-token",
                "authorization: <redacted>",
            ),
            (
                "Proxy-Authorization: Basic dXNlcjpwYXNz",
                "Proxy-Authorization: <redacted>",
            ),
            (
                "Authorization: Digest username=alice, response=secret",
                "Authorization: <redacted>",
            ),
            (
                "curl -H 'Authorization: Basic dXNlcjpwYXNz' next",
                "curl -H 'Authorization: <redacted>' next",
            ),
            (
                "Authorization: Basic dXNlcjpwYXNz\nnext",
                "Authorization: <redacted>\nnext",
            ),
        ] {
            for redact in [redact_credentials, sanitize_log] {
                assert_eq!(redact(input), expected, "{input}");
            }
        }
    }

    #[test]
    fn lookahead_work_is_linear_for_long_non_secret_runs() {
        for input in ["a".repeat(128 * 1024), "a.+a/".repeat(32 * 1024)] {
            for credentials_only in [true, false] {
                let mut scan = Scan::default();
                assert_eq!(redact(&input, credentials_only, &mut scan), input);
                assert!(
                    scan.inspected <= input.len() * 4,
                    "{} visits",
                    scan.inspected
                );
            }
        }
        // Skipping a non-secret run wholesale would miss embedded prefixes.
        for redact in [redact_credentials, sanitize_log] {
            let input = format!("{}sk-short-secret", "a".repeat(65536));
            assert_eq!(redact(&input), format!("{}<redacted>", "a".repeat(65536)));
        }
    }

    #[test]
    fn ordinary_diagnostics_survive_redaction_unchanged() {
        // false positives would make the log useless, so pin the negative
        for line in [
            "[queue] sent to sess-1a2b3 (17B, mode chain)",
            "[poll] session listing FAILED (tmux-missing)",
            "[storage] warning (invalid-doc)",
            "[ui] keydown arrow a=0",
            "[ui] update-avail 0.4.29",
            "[tmux] using sidecar binary",
            "[boot] instance lock unavailable (locked) — exiting",
            "[pty] emit #3 4096B to sess-0f1e2",
            "[queue] step skipped by user — group unblocked",
            "ratios at/every/chain and count=24 are safe",
            "relative/path and version 0.4.30 stay useful",
        ] {
            assert_eq!(sanitize_log(line), line, "over-redacted: {line}");
        }
    }

    #[test]
    fn shell_recovery_keeps_agent_session_ids_but_not_credentials() {
        // A restored transcript exists so the user can pick their work back
        // up; `claude --resume <uuid>` is the single most valuable line in
        // it and the opaque-token rule used to erase the id.
        let id = "0f3ab19c-4d2e-4a71-9b8c-1d2e3f4a5b6c";
        for line in [
            format!("$ claude --resume {id}"),
            format!("$ codex resume {id}"),
            format!("~/.claude/projects/deck/{id}.jsonl"),
            format!("({id})"),
        ] {
            assert_eq!(redact_credentials(&line), line, "over-redacted: {line}");
        }
        // shape, boundary and credential rules still hold
        for (line, keep) in [
            (format!("API_TOKEN={id}"), false),
            (format!("Authorization: Bearer {id}"), false),
            (format!("sk-live-{id}"), false),
            ("0f3ab19c-4d2e-4a71-9b8c-1d2e3f4a5b6c7d".to_string(), false),
        ] {
            assert_eq!(
                redact_credentials(&line).contains(id),
                keep,
                "wrong verdict: {line}"
            );
        }
        // app.log keeps the strict policy: it carries no user content at all
        assert!(!sanitize_log(&format!("[ui] {id}")).contains(id));
    }
}
