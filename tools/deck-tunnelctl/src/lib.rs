//! Closed implementation surface for the optional Deck Tunnel helper.

pub mod cli;
mod identity;
mod keychain;
mod process;
mod protocol;
mod secret_file;
mod tunnel_client;

#[cfg(test)]
mod real_file_secret_lifecycle;

pub const RUNTIME_KEY_SERVICE: &str = "io.c9r.deck-tunnelctl.runtime";

pub fn valid_client_id(value: &str) -> bool {
    value.starts_with("client_")
        && value.len() > "client_".len()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

pub fn runtime_alias(client_id: &str) -> Result<String, &'static str> {
    use sha2::{Digest, Sha256};
    if !valid_client_id(client_id) {
        return Err("invalid_client_id");
    }
    let mut hash = Sha256::new();
    hash.update(b"deck-tunnelctl:runtime-alias:v1\0");
    hash.update(client_id.as_bytes());
    let digest = hash.finalize();
    Ok(format!("deck-{}", hex(&digest[..16])))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_ids_are_closed_and_bounded() {
        assert!(valid_client_id("client_abc-DEF_012"));
        assert!(!valid_client_id("client_"));
        assert!(!valid_client_id("client_a b"));
        assert!(!valid_client_id("other_abc"));
        assert!(!valid_client_id(&format!("client_{}", "a".repeat(122))));
    }

    #[test]
    fn aliases_are_deterministic_legal_and_do_not_contain_user_text() {
        let first = runtime_alias("client_alpha").unwrap();
        assert_eq!(first, runtime_alias("client_alpha").unwrap());
        assert_ne!(first, runtime_alias("client_beta").unwrap());
        assert_eq!(first.len(), 37);
        assert!(first.starts_with("deck-"));
        assert!(first
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        assert!(!first.contains("alpha"));
    }

    #[test]
    fn sampled_aliases_do_not_collide() {
        let mut aliases = std::collections::HashSet::new();
        for index in 0..20_000 {
            assert!(aliases.insert(runtime_alias(&format!("client_{index}")).unwrap()));
        }
    }

    /// Production source of every module: a file declared `#[cfg(test)] mod`
    /// in lib.rs is left out, as is only a TRAILING `#[cfg(test)] mod tests`
    /// (nothing but indented lines and its closing brace after it).
    fn production_sources() -> Vec<(String, String)> {
        const TEST_MODULE: &str = "#[cfg(test)]\nmod tests";
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let lib = std::fs::read_to_string(dir.join("lib.rs")).unwrap();
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            assert!(path.is_file(), "unscanned source directory {path:?}");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let module = name.strip_suffix(".rs").expect("only .rs sources");
            if lib.contains(&format!("#[cfg(test)]\nmod {module};")) {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let production =
                match source.rfind(TEST_MODULE) {
                    Some(at)
                        if source[at..].lines().skip(2).all(|line| {
                            line.is_empty() || line == "}" || line.starts_with(' ')
                        }) && source.trim_end().ends_with('}') =>
                    {
                        source[..at].to_string()
                    }
                    _ => source,
                };
            out.push((name, production));
        }
        assert!(out.len() >= 9, "all helper modules found: {}", out.len());
        out
    }

    /// (enclosing fn, argument) of every `Command :: new (` call, tolerant of
    /// whitespace; `Command` must not be the tail of a longer identifier.
    fn command_sites(source: &str) -> Vec<(String, String)> {
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let mut sites = Vec::new();
        for (at, _) in source.match_indices("Command") {
            if source[..at].chars().next_back().is_some_and(ident) {
                continue;
            }
            let rest = source[at + "Command".len()..].trim_start();
            let Some(rest) = rest.strip_prefix("::").map(str::trim_start) else {
                continue;
            };
            let Some(rest) = rest.strip_prefix("new") else {
                continue;
            };
            if rest.chars().next().is_some_and(ident) {
                continue;
            }
            let Some(rest) = rest.trim_start().strip_prefix('(') else {
                continue;
            };
            let mut depth = 0usize;
            let end = rest
                .char_indices()
                .find(|(_, c)| match c {
                    '(' => {
                        depth += 1;
                        false
                    }
                    ')' if depth == 0 => true,
                    ')' => {
                        depth -= 1;
                        false
                    }
                    _ => false,
                })
                .expect("balanced constructor argument")
                .0;
            let argument = rest[..end].trim().to_string();
            let before = &source[..at];
            let function = before
                .match_indices("fn ")
                .filter(|(index, _)| !before[..*index].chars().next_back().is_some_and(ident))
                .map(|(index, _)| {
                    before[index + 3..]
                        .chars()
                        .take_while(|c| ident(*c))
                        .collect::<String>()
                })
                .last()
                .unwrap_or_default();
            sites.push((function, argument));
        }
        sites
    }

    /// EDR spawn census (the helper's counterpart of Deck's
    /// `app/src-tauri/tests/edr_quiet.rs`): the ONE production process spawn
    /// is the verified tunnel-client in `Client::spawn`; no shell, no bare
    /// program name (PATH lookup), no lower-level spawn, no persistence.
    #[test]
    fn production_spawn_census_is_one_verified_tunnel_client_site() {
        let mut sites = Vec::new();
        for (name, source) in production_sources() {
            for site in command_sites(&source) {
                sites.push((name.clone(), site.0, site.1));
            }
            for forbidden in [
                "Command as",
                "= Command;",
                "= std::process::Command;",
                "= process::Command;",
                "CommandBuilder",
                "posix_spawn",
                "libc::exec",
                "libc::system",
                "libc::fork",
                "libc::popen",
                "NSTask",
                "/bin/sh",
                "/bin/bash",
                "/bin/zsh",
                "\"sh\"",
                "\"bash\"",
                "\"zsh\"",
                "launchctl",
                "LaunchAgent",
                "LaunchDaemon",
                "SMAppService",
                "osascript",
                "nohup",
                "disown",
                "setsid",
                "cron",
                "\"PATH\"",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{name}: helper contains {forbidden}"
                );
            }
        }
        assert_eq!(
            sites,
            vec![(
                "tunnel_client.rs".to_string(),
                "spawn".to_string(),
                "self.executable.path()".to_string()
            )],
            "the helper's process surface changed and needs EDR review"
        );
    }

    #[test]
    fn census_scanner_sees_aliases_whitespace_and_bare_names() {
        assert_eq!(
            command_sites("fn f() { Command :: new (\"curl\").status(); }"),
            vec![("f".to_string(), "\"curl\"".to_string())]
        );
        assert!(command_sites("fn f() { CommandBuilder::new(x); MyCommand::new(y); }").is_empty());
    }
}
