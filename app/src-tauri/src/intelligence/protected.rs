//! Deterministic lexical protection and bounded segmentation. No Agent
//! messages, prompts or task semantics are inspected here.
use regex::Regex;
use std::sync::OnceLock;
use unicode_segmentation::UnicodeSegmentation;

pub(super) const SEGMENT_BYTES: usize = 400;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: &'static str,
}

fn patterns() -> &'static Vec<(&'static str, Regex, usize)> {
    static PATTERNS: OnceLock<Vec<(&'static str, Regex, usize)>> = OnceLock::new();
    PATTERNS.get_or_init(|| [
        ("fenced-code", r"(?ms)^```[^\n]*\n.*?^```[ \t]*", 0),
        ("inline-code", r"`[^`\n]+`", 0),
        ("inline-json", r#"\{[^{}\n]*"[^{}\n]*\}"#, 0),
        ("inline-xml", r"<[A-Za-z][\w:-]*\b[^>]*>[^<\n]*</[A-Za-z][\w:-]*>", 0),
        ("shell-line", r"(?m)^(?:\$\s*)?(?:cargo|git|npm|kubectl)\s+[^\n]+$", 0),
        ("url", r"https?://[^\s<>\])]+", 0),
        ("windows-path", r"\b[A-Za-z]:\\(?:[^\\\s]+\\)*[^\\\s]+", 0),
        ("path", r"(?:^|[^\w])((?:/|\./|\.\./|~/)[A-Za-z0-9_./~-]+)", 1),
        ("xml-tag", r"</?[A-Za-z][\w:-]*(?:\s+[^<>]*)?/?>", 0),
        ("yaml-key", r"(?m)^[ \t]*-?[ \t]*([A-Za-z_][\w.-]*):[ \t]", 1),
        ("identifier", r"\b(?:[A-Z][A-Za-z]*[A-Z][A-Za-z0-9]*\d+[A-Za-z0-9]*|[A-Z][A-Z0-9_]*_[A-Z0-9_]+|[A-Za-z][\w]*_[A-Za-z0-9_]+|[A-Za-z]+(?:-[A-Za-z0-9]+){2,}|[A-Z][A-Z0-9]*-[A-Z0-9]+|v\d+(?:\.\d+)+|ticket-\d+)\b", 0),
        ("numeric-id", r"(?:#\d{3,}|\b\d{5,}\b|\b\d{4}-\d{2}-\d{2}\b|\b\d+(?:\.\d+){2,}\b)", 0),
    ].into_iter().map(|(kind, pattern, group)| (kind, Regex::new(pattern).expect("fixed scanner pattern"), group)).collect())
}

pub(super) fn scan(source: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for (kind, pattern, group) in patterns() {
        for captures in pattern.captures_iter(source) {
            let Some(matched) = captures.get(*group) else {
                continue;
            };
            let (start, mut end) = (matched.start(), matched.end());
            if matches!(*kind, "url" | "path" | "windows-path") {
                while end > start
                    && matches!(
                        source.as_bytes()[end - 1],
                        b'.' | b',' | b';' | b':' | b'!' | b'?'
                    )
                {
                    end -= 1;
                }
            }
            if start < end && !spans.iter().any(|old| start < old.end && end > old.start) {
                spans.push(Span { start, end, kind });
            }
        }
    }
    spans.sort_by_key(|span| span.start);
    spans
}

fn escape_html(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    for ch in source.chars() {
        match ch {
            '&' => result.push_str("&amp;"),
            '<' => result.push_str("&lt;"),
            '>' => result.push_str("&gt;"),
            '"' => result.push_str("&quot;"),
            '\'' => result.push_str("&#39;"),
            _ => result.push(ch),
        }
    }
    result
}
fn unescape_html(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find('&') {
        result.push_str(&rest[..at]);
        rest = &rest[at..];
        let entity = [
            (&rest[..rest.len().min(5)], "&amp;", '&'),
            (&rest[..rest.len().min(4)], "&lt;", '<'),
            (&rest[..rest.len().min(4)], "&gt;", '>'),
            (&rest[..rest.len().min(6)], "&quot;", '"'),
            (&rest[..rest.len().min(5)], "&#39;", '\''),
        ];
        if let Some((_, token, ch)) = entity
            .into_iter()
            .find(|(_, token, _)| rest.starts_with(token))
        {
            result.push(ch);
            rest = &rest[token.len()..];
        } else {
            result.push('&');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}

pub(super) fn carrier(source: &str) -> (String, Vec<String>) {
    let spans = scan(source);
    let mut payload = String::new();
    let mut originals = Vec::with_capacity(spans.len());
    let mut cursor = 0;
    for span in spans {
        payload.push_str(&escape_html(&source[cursor..span.start]));
        payload.push_str(&format!(
            "<code data-deck-id=\"{}\">x</code>",
            originals.len()
        ));
        originals.push(source[span.start..span.end].to_owned());
        cursor = span.end;
    }
    payload.push_str(&escape_html(&source[cursor..]));
    (payload, originals)
}

fn marker() -> &'static Regex {
    static MARKER: OnceLock<Regex> = OnceLock::new();
    MARKER.get_or_init(|| Regex::new(r#"<code data-deck-id="(\d+)">x</code>"#).unwrap())
}
pub(super) fn restore(target: &str, originals: &[String]) -> Result<String, &'static str> {
    let matches: Vec<_> = marker().captures_iter(target).collect();
    if matches.len() != originals.len() {
        return Err("protected-restoration-failed");
    }
    let mut seen = vec![false; originals.len()];
    let mut result = String::new();
    let mut cursor = 0;
    for captures in matches {
        let matched = captures.get(0).ok_or("protected-restoration-failed")?;
        let id: usize = captures
            .get(1)
            .ok_or("protected-restoration-failed")?
            .as_str()
            .parse()
            .map_err(|_| "protected-restoration-failed")?;
        if id >= originals.len() || seen[id] {
            return Err("protected-restoration-failed");
        }
        seen[id] = true;
        let prose = &target[cursor..matched.start()];
        if prose.contains("data-deck-id") || prose.contains('<') || prose.contains('>') {
            return Err("protected-restoration-failed");
        }
        result.push_str(&unescape_html(prose));
        result.push_str(&originals[id]);
        cursor = matched.end();
    }
    let tail = &target[cursor..];
    if tail.contains("data-deck-id")
        || tail.contains('<')
        || tail.contains('>')
        || seen.iter().any(|seen| !seen)
    {
        return Err("protected-restoration-failed");
    }
    result.push_str(&unescape_html(tail));
    Ok(result)
}

fn split_unit<'a>(source: &'a str, level: usize, output: &mut Vec<&'a str>) {
    if source.len() <= SEGMENT_BYTES {
        output.push(source);
        return;
    }
    let delimiters = [r"\n\n+", r"\n", r"[.!?][ \t]+"];
    if let Some(pattern) = delimiters.get(level) {
        let regex = Regex::new(pattern).expect("fixed split pattern");
        let mut cursor = 0;
        let mut found = false;
        for m in regex.find_iter(source) {
            found = true;
            let cut = if level == 2 { m.start() + 1 } else { m.start() };
            if cursor < cut {
                split_unit(&source[cursor..cut], level + 1, output);
            }
            if level == 2 {
                split_unit(&source[cut..m.end()], level + 1, output);
            } else {
                split_unit(m.as_str(), level + 1, output);
            }
            cursor = m.end();
        }
        if found {
            if cursor < source.len() {
                split_unit(&source[cursor..], level + 1, output);
            }
            return;
        }
        split_unit(source, level + 1, output);
        return;
    }
    let mut start = 0;
    let mut end = 0;
    for (at, grapheme) in source.grapheme_indices(true) {
        if at + grapheme.len() - start > SEGMENT_BYTES && start < at {
            output.push(&source[start..at]);
            start = at;
        }
        end = at + grapheme.len();
    }
    if start < end {
        output.push(&source[start..end]);
    }
}

pub(super) fn split_document(source: &str) -> Vec<&str> {
    if source.is_empty() {
        return Vec::new();
    }
    let mut base = Vec::new();
    split_unit(source, 0, &mut base);
    let protected = scan(source);
    let mut cuts = Vec::new();
    let mut offset = 0;
    for piece in base.iter().take(base.len().saturating_sub(1)) {
        offset += piece.len();
        if !protected
            .iter()
            .any(|span| span.start < offset && offset < span.end)
        {
            cuts.push(offset);
        }
    }
    let mut result = Vec::new();
    let mut start = 0;
    for cut in cuts {
        if start < cut {
            result.push(&source[start..cut]);
            start = cut;
        }
    }
    if start < source.len() {
        result.push(&source[start..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_literals_and_fake_html_are_exact() {
        let source = "Run `cargo test --workspace` for AcmeWidgetQ7 at /usr/local/example-q17.\n```bash\ngit diff --check\n```\n<span data-deck-protected=\"1\">bad</span>\n<code data-deck-id=\"0\">x</code>";
        let (html, originals) = carrier(source);
        assert!(!html.contains("AcmeWidgetQ7"));
        assert_eq!(restore(&html, &originals).unwrap(), source);
        assert!(restore(
            &html.replace("data-deck-id=\"0\"", "data-deck-id=\"1\""),
            &originals
        )
        .is_err());
    }
    #[test]
    fn segmentation_is_byte_identical() {
        for source in [
            "paragraph\n\nline. Sentence! More text.",
            "é中文🙂".repeat(400).as_str(),
        ] {
            assert_eq!(split_document(source).concat(), source);
        }
    }
}
