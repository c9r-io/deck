use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::process;

const TUNNEL_CLIENT_VERSION: &str = "0.0.14";
const TUNNEL_CLIENT_SHA256: &str =
    "309fd85da5a8c2ca8dae920deea8ac10a4d7934ed18ac46e7df0c200139cc9c5";
const TUNNEL_CLIENT_CANDIDATES: [&str; 4] = [
    "/opt/homebrew/opt/tunnel-client/libexec/tunnel-client",
    "/usr/local/opt/tunnel-client/libexec/tunnel-client",
    "/opt/homebrew/bin/tunnel-client",
    "/usr/local/bin/tunnel-client",
];
const PRODUCTION_ADAPTER: &str = "/Applications/deck.app/Contents/MacOS/deck-mcp";

pub fn tunnel_client() -> Result<PathBuf, &'static str> {
    if let Some(path) = development_tunnel_client_path() {
        return validate_development_executable(&path);
    }
    tunnel_client_from_candidates(&TUNNEL_CLIENT_CANDIDATES)
}

fn tunnel_client_from_candidates(candidates: &[&str]) -> Result<PathBuf, &'static str> {
    for candidate in candidates {
        if Path::new(candidate).exists() {
            return validate_tunnel_client(Path::new(candidate));
        }
    }
    Err("tunnel_client_missing")
}

#[cfg(debug_assertions)]
fn development_tunnel_client_path() -> Option<PathBuf> {
    std::env::var_os("DECK_TUNNELCTL_TUNNEL_CLIENT").map(PathBuf::from)
}

#[cfg(not(debug_assertions))]
fn development_tunnel_client_path() -> Option<PathBuf> {
    None
}

pub fn adapter() -> Result<PathBuf, &'static str> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("DECK_TUNNELCTL_ADAPTER_PATH") {
        return validate_development_executable(Path::new(&path));
    }
    let path = Path::new(PRODUCTION_ADAPTER);
    validate_direct_executable(path)?;
    validate_adapter_signature(path)?;
    Ok(path.to_path_buf())
}

fn validate_tunnel_client(candidate: &Path) -> Result<PathBuf, &'static str> {
    let canonical = candidate
        .canonicalize()
        .map_err(|_| "tunnel_client_missing")?;
    validate_direct_executable(&canonical)?;
    let bytes = fs::read(&canonical).map_err(|_| "tunnel_client_untrusted")?;
    let digest = Sha256::digest(bytes);
    let actual = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if actual != TUNNEL_CLIENT_SHA256 {
        return Err("tunnel_client_untrusted");
    }
    let version = process::output(
        Command::new(&canonical).arg("--version"),
        Duration::from_secs(3),
    )
    .map_err(|_| "tunnel_client_probe_failed")?;
    if !version.status.success()
        || !String::from_utf8_lossy(&version.stdout).contains(TUNNEL_CLIENT_VERSION)
    {
        return Err("tunnel_client_incompatible");
    }
    let help = process::output(
        Command::new(&canonical).args(["runtimes", "--help"]),
        Duration::from_secs(3),
    )
    .map_err(|_| "tunnel_client_probe_failed")?;
    let text = String::from_utf8_lossy(&help.stdout);
    if !help.status.success()
        || !["connect", "list", "status", "stop", "rm"]
            .iter()
            .all(|command| text.contains(command))
    {
        return Err("tunnel_client_incompatible");
    }
    Ok(canonical)
}

fn validate_development_executable(path: &Path) -> Result<PathBuf, &'static str> {
    let canonical = path
        .canonicalize()
        .map_err(|_| "development_executable_missing")?;
    validate_direct_executable(&canonical)?;
    Ok(canonical)
}

fn validate_direct_executable(path: &Path) -> Result<(), &'static str> {
    let symlink = fs::symlink_metadata(path).map_err(|_| "executable_missing")?;
    if symlink.file_type().is_symlink() || !symlink.is_file() {
        return Err("executable_untrusted");
    }
    let owner = symlink.uid();
    if symlink.permissions().mode() & 0o111 == 0
        || (owner != 0 && owner != unsafe { libc::geteuid() })
    {
        return Err("executable_untrusted");
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn validate_adapter_signature(path: &Path) -> Result<(), &'static str> {
    use core_foundation::url::CFURL;
    use security_framework::os::macos::code_signing::{Flags, SecRequirement, SecStaticCode};
    use std::str::FromStr;

    let url = CFURL::from_path(path, false).ok_or("adapter_untrusted")?;
    let code = SecStaticCode::from_path(&url, Flags::NONE).map_err(|_| "adapter_untrusted")?;
    let requirement = SecRequirement::from_str(
        "identifier \"deck-mcp\" and anchor apple generic and certificate leaf[subject.OU] = \"Y8ZG3D692W\"",
    )
    .map_err(|_| "adapter_untrusted")?;
    code.check_validity(
        Flags::STRICT_VALIDATE | Flags::CHECK_ALL_ARCHITECTURES,
        &requirement,
    )
    .map_err(|_| "adapter_untrusted")
}

#[cfg(not(target_os = "macos"))]
fn validate_adapter_signature(_: &Path) -> Result<(), &'static str> {
    Err("adapter_unsupported_platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn direct_validator_rejects_symlinks_and_non_executables() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, b"fixture").unwrap();
        assert_eq!(
            validate_direct_executable(&target),
            Err("executable_untrusted")
        );
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(validate_direct_executable(&target).is_ok());
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        assert_eq!(
            validate_direct_executable(&link),
            Err("executable_untrusted")
        );
        assert_eq!(
            validate_tunnel_client(&target),
            Err("tunnel_client_untrusted")
        );
    }

    #[test]
    fn installed_deck_adapter_has_the_expected_signed_identity_when_present() {
        if Path::new(PRODUCTION_ADAPTER).exists() {
            assert_eq!(adapter().unwrap(), PathBuf::from(PRODUCTION_ADAPTER));
        }
    }

    #[test]
    fn installed_tunnel_client_matches_the_pinned_release_when_present() {
        if TUNNEL_CLIENT_CANDIDATES
            .iter()
            .any(|candidate| Path::new(candidate).exists())
        {
            let path = tunnel_client().unwrap();
            assert!(path.ends_with("libexec/tunnel-client"));
        }
    }

    #[test]
    fn absent_candidates_report_missing_without_path_lookup() {
        assert_eq!(
            tunnel_client_from_candidates(&["/definitely/not/a/tunnel-client"]),
            Err("tunnel_client_missing")
        );
    }

    #[cfg(not(debug_assertions))]
    #[test]
    fn release_build_ignores_tunnel_client_override() {
        std::env::set_var(
            "DECK_TUNNELCTL_TUNNEL_CLIENT",
            "/definitely/not/a/tunnel-client",
        );
        assert!(development_tunnel_client_path().is_none());
        std::env::remove_var("DECK_TUNNELCTL_TUNNEL_CLIENT");
    }
}
