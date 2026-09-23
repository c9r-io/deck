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

    #[test]
    fn helper_has_no_shell_or_persistence_registration_vocabulary() {
        let sources = [
            include_str!("cli.rs"),
            include_str!("identity.rs"),
            include_str!("process.rs"),
            include_str!("tunnel_client.rs"),
        ]
        .join("\n");
        for forbidden in [
            "Command::new(\"sh\")",
            "Command::new(\"bash\")",
            "Command::new(\"zsh\")",
            "/bin/sh",
            "launchctl",
            "LaunchAgent",
            "LaunchDaemon",
            "SMAppService",
            "osascript",
            "nohup",
            "disown",
            "cron",
        ] {
            assert!(!sources.contains(forbidden), "helper contains {forbidden}");
        }
    }
}
