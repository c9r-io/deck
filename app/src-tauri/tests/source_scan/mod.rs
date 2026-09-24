//! Production-source scanning shared by the census tests (`edr_quiet.rs`,
//! `external_admission.rs`, `ipc_contract.rs`): which files count as
//! production, which part of a file is its trailing test module, and which
//! `fn` (Rust) or JS function encloses a site.
// Each test crate uses a subset of these helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

pub fn manifest(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// (relative path, contents) of every backend module, recursively.
pub fn all_sources() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("src") {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().is_some_and(|x| x == "rs") {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&manifest("src"), &manifest("src"), &mut out);
    assert!(out.len() >= 20, "all backend modules found: {}", out.len());
    out.sort();
    out
}

pub const TEST_MODULE: &str = "#[cfg(test)]\nmod tests";

/// The production part of one file: only a TRAILING `#[cfg(test)] mod tests`
/// block is removed, i.e. one after which nothing at column 0 follows except
/// its closing brace. An early or empty test module never hides the
/// production code after it.
pub fn production_region(source: &str) -> &str {
    let Some(at) = source.rfind(TEST_MODULE) else {
        return source;
    };
    let mut closed = false;
    for line in source[at..].lines().skip(2).filter(|line| !line.is_empty()) {
        if closed {
            return source;
        }
        if line == "}" {
            closed = true;
        } else if !line.starts_with(char::is_whitespace) {
            return source;
        }
    }
    if closed {
        &source[..at]
    } else {
        source
    }
}

/// A dedicated `dir/tests.rs` is test-only when its parent module declares
/// it under `#[cfg(test)]`.
pub fn is_declared_test_file(name: &str, sources: &[(String, String)]) -> bool {
    let Some(dir) = name.strip_suffix("/tests.rs") else {
        return false;
    };
    let parents = [format!("{dir}.rs"), format!("{dir}/mod.rs")];
    sources.iter().any(|(parent, text)| {
        parents.contains(parent)
            && (text.contains("#[cfg(test)]\nmod tests;")
                || text.contains("#[cfg(test)]\npub(crate) mod tests;"))
    })
}

pub fn production_sources() -> Vec<(String, String)> {
    let sources = all_sources();
    sources
        .iter()
        .filter(|(name, _)| !is_declared_test_file(name, &sources))
        .map(|(name, text)| (name.clone(), production_region(text).to_string()))
        .collect()
}

pub fn is_ident(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// The name of the last `fn` declared before `at` (empty at file scope).
pub fn enclosing_function(source: &str, at: usize) -> String {
    let before = &source[..at];
    let mut found = String::new();
    for (index, _) in before.match_indices("fn ") {
        if before[..index].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let name: String = before[index + 3..]
            .chars()
            .take_while(|character| is_ident(*character))
            .collect();
        if !name.is_empty() {
            found = name;
        }
    }
    found
}

/// (file name, contents) of every frontend module in `ui/js`.
pub fn js_sources() -> Vec<(String, String)> {
    let root = manifest("../ui/js");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&root).expect("ui/js") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|x| x == "js") {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            out.push((name, std::fs::read_to_string(&path).unwrap()));
        }
    }
    assert!(out.len() >= 20, "frontend modules found: {}", out.len());
    out.sort();
    out
}

pub fn js_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The function a JS line declares: `function name(`, an object method
/// `async name(a, b) {` (a plain parameter list, so `listen('x', () => {` is
/// a call, not a method), or a top-level `const name = (...) =>` /
/// `= async` / `= function`. Anything nested deeper than one closure level
/// belongs to the function around it.
pub fn js_declaration(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start().len();
    if indent > 4 {
        return None;
    }
    let t = line.trim_start();
    let t = t.strip_prefix("export ").unwrap_or(t);
    let t = t.strip_prefix("default ").unwrap_or(t);
    let t = t.strip_prefix("async ").unwrap_or(t);
    let head = |s: &str| -> String { s.chars().take_while(|c| js_ident(*c)).collect() };
    if let Some(rest) = t.strip_prefix("function") {
        let name = head(rest.trim_start_matches(['*', ' ']));
        return (!name.is_empty()).then_some(name);
    }
    if let Some(rest) = t
        .strip_prefix("const ")
        .or_else(|| t.strip_prefix("let "))
        .filter(|_| indent == 0)
    {
        let name = head(rest);
        let rhs = rest[name.len()..]
            .trim_start()
            .strip_prefix('=')?
            .trim_start();
        let callee = head(rhs);
        let arrow = rhs.starts_with("async")
            || rhs.starts_with("function")
            || (rhs.starts_with('(') && line.contains("=>"))
            || (!callee.is_empty() && rhs[callee.len()..].trim_start().starts_with("=>"));
        return (arrow && !name.is_empty()).then_some(name);
    }
    let name = head(t);
    let keyword = [
        "if", "for", "while", "switch", "catch", "return", "else", "do", "try",
    ];
    let params = t[name.len()..]
        .strip_prefix('(')
        .and_then(|rest| rest.trim_end().strip_suffix('{'))
        .and_then(|rest| rest.trim_end().strip_suffix(')'))?;
    (!name.is_empty()
        && !keyword.contains(&name.as_str())
        && !params.contains(['\'', '"', '`', '(', ')', '>']))
    .then_some(name)
}

pub fn js_enclosing(source: &str, at: usize) -> String {
    source[..at]
        .lines()
        .rev()
        .find_map(js_declaration)
        .unwrap_or_default()
}
