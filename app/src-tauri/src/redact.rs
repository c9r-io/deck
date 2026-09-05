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
    if bytes
        .get(i)
        .is_some_and(|b| matches!(b, b'"' | b'\'' | b'(' | b'['))
    {
        i += 1; // keep the user's syntax, redact only its value
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

fn credential_token_end(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let rest = &line[start..];
    if SECRET_PREFIXES
        .iter()
        .any(|prefix| rest.starts_with(prefix))
    {
        return Some(value_end(bytes, start));
    }
    let end = token_end(bytes, start);
    if end > start {
        let run = &line[start..end];
        let opaque = run.len() >= 24
            && run.bytes().any(|c| c.is_ascii_digit())
            && run.bytes().any(|c| c.is_ascii_alphabetic());
        if opaque {
            return Some(end);
        }
    }
    None
}

/// Redact likely credentials without removing ordinary paths and URLs. Shell
/// recovery uses this narrower policy because its output remains useful only
/// if directories and links survive, while obvious secret values must not.
pub(crate) fn redact_credentials(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if let Some((value_start, end)) = credential_assignment(line, i) {
            out.push_str(&line[i..value_start]);
            out.push_str("<redacted>");
            i = end;
            continue;
        }
        if let Some(end) = uuid_token_end(line, i) {
            out.push_str(&line[i..end]); // an identifier, not a credential
            i = end;
            continue;
        }
        if let Some(end) = credential_token_end(line, i) {
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
fn sensitive_end(line: &str, start: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let rest = &line[start..];
    if rest.starts_with("~/") || rest.starts_with('/') {
        let ansi_boundary = line[..start].rfind('\x1b').is_some_and(|esc| {
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
        let end = token_end(bytes, start);
        if line[start + 5..end].contains('-') {
            return Some(end);
        }
    }

    // Any RFC-style scheme:// URL, even when attached to JSON/assignment
    // punctuation. Detection starts at the scheme rather than splitting on
    // whitespace, so quotes, colons, equals and ANSI wrappers cannot hide it.
    if bytes[start].is_ascii_alphabetic() {
        let mut j = start + 1;
        while j < bytes.len()
            && (bytes[j].is_ascii_alphanumeric() || matches!(bytes[j], b'+' | b'-' | b'.'))
        {
            j += 1;
        }
        if line[j..].starts_with("://") {
            return Some(value_end(bytes, j + 3));
        }
    }

    let end = token_end(bytes, start);
    if end > start {
        let run = &line[start..end];
        let opaque = run.len() >= 24
            && run.bytes().any(|c| c.is_ascii_digit())
            && run.bytes().any(|c| c.is_ascii_alphabetic());
        if opaque {
            return Some(end);
        }
    }
    None
}

/// Replace sensitive spans wherever they occur while preserving surrounding
/// diagnostic punctuation and ANSI control sequences.
pub(crate) fn sanitize_log(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if let Some((value_start, end)) = credential_assignment(line, i) {
            out.push_str(&line[i..value_start]);
            out.push_str("<redacted>");
            i = end;
            continue;
        }
        if let Some(end) = sensitive_end(line, i) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
