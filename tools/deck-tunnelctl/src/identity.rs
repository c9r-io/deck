//! Executable identity for the helper's two external programs.
//!
//! `tunnel-client` is accepted only at fixed install candidates (never PATH)
//! and only when its SHA-256 matches the pinned official release. The pin
//! already fixes the version and CLI surface, so no `--version` or `--help`
//! probe runs. Verification records the identity (device, inode, size, mtime,
//! ctime) of the hashed bytes; every later run first re-compares that
//! identity, and the one run that receives the Runtime-key reference re-hashes
//! the file. This narrows, but cannot eliminate, path-based validate-to-exec
//! replacement by the same macOS user (see docs/mcp-tunnel-helper.md).

use sha2::{Digest, Sha256};
use std::fs::{self, Metadata, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Official macOS arm64 tunnel-client 0.0.14.
const TUNNEL_CLIENT_SHA256: &str =
    "309fd85da5a8c2ca8dae920deea8ac10a4d7934ed18ac46e7df0c200139cc9c5";
const TUNNEL_CLIENT_CANDIDATES: [&str; 4] = [
    "/opt/homebrew/opt/tunnel-client/libexec/tunnel-client",
    "/usr/local/opt/tunnel-client/libexec/tunnel-client",
    "/opt/homebrew/bin/tunnel-client",
    "/usr/local/bin/tunnel-client",
];
const PRODUCTION_ADAPTER: &str = "/Applications/deck.app/Contents/MacOS/deck-mcp";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileIdentity {
    fn of(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

/// A validated tunnel-client, bound to the file identity seen at validation.
#[derive(Debug)]
pub struct Executable {
    path: PathBuf,
    identity: FileIdentity,
    pinned: bool,
}

impl Executable {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Before every run: the path still names the validated file.
    pub fn recheck(&self) -> Result<(), &'static str> {
        validate_direct_executable(&self.path)?;
        let metadata = fs::symlink_metadata(&self.path).map_err(|_| "tunnel_client_replaced")?;
        if FileIdentity::of(&metadata) != self.identity {
            return Err("tunnel_client_replaced");
        }
        Ok(())
    }

    /// Immediately before the run that is handed the Runtime-key reference:
    /// the identity recheck plus a fresh hash of the pinned contents.
    pub fn recheck_contents(&self) -> Result<(), &'static str> {
        self.recheck()?;
        if self.pinned {
            let (digest, identity) = hash_file(&self.path)?;
            if digest != TUNNEL_CLIENT_SHA256 || identity != self.identity {
                return Err("tunnel_client_replaced");
            }
        }
        Ok(())
    }
}

pub fn tunnel_client() -> Result<Executable, &'static str> {
    if let Some(path) = development_tunnel_client_path() {
        return development_executable(&path);
    }
    tunnel_client_from_candidates(&TUNNEL_CLIENT_CANDIDATES)
}

fn tunnel_client_from_candidates(candidates: &[&str]) -> Result<Executable, &'static str> {
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

fn validate_tunnel_client(candidate: &Path) -> Result<Executable, &'static str> {
    let canonical = candidate
        .canonicalize()
        .map_err(|_| "tunnel_client_missing")?;
    validate_direct_executable(&canonical)?;
    let (digest, identity) = hash_file(&canonical)?;
    if digest != TUNNEL_CLIENT_SHA256 {
        return Err("tunnel_client_untrusted");
    }
    let executable = Executable {
        path: canonical,
        identity,
        pinned: true,
    };
    executable.recheck()?;
    Ok(executable)
}

/// SHA-256 of one regular file read through a no-follow descriptor, with the
/// identity of that same descriptor; a change while hashing is refused.
fn hash_file(path: &Path) -> Result<(String, FileIdentity), &'static str> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "tunnel_client_untrusted")?;
    let before = file.metadata().map_err(|_| "tunnel_client_untrusted")?;
    if !before.is_file() {
        return Err("tunnel_client_untrusted");
    }
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|_| "tunnel_client_untrusted")?;
    let after = file.metadata().map_err(|_| "tunnel_client_untrusted")?;
    let identity = FileIdentity::of(&before);
    if FileIdentity::of(&after) != identity {
        return Err("tunnel_client_replaced");
    }
    let digest = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    Ok((digest, identity))
}

/// Debug-only override and unit-test fixtures: identity-bound, not pinned.
pub(crate) fn development_executable(path: &Path) -> Result<Executable, &'static str> {
    let path = validate_development_executable(path)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| "executable_missing")?;
    Ok(Executable {
        identity: FileIdentity::of(&metadata),
        path,
        pinned: false,
    })
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
            validate_tunnel_client(&target).unwrap_err(),
            "tunnel_client_untrusted"
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
            let executable = tunnel_client().unwrap();
            assert!(executable.path().ends_with("libexec/tunnel-client"));
            assert_eq!(executable.recheck_contents(), Ok(()));
        }
    }

    #[test]
    fn absent_candidates_report_missing_without_path_lookup() {
        assert_eq!(
            tunnel_client_from_candidates(&["/definitely/not/a/tunnel-client"]).unwrap_err(),
            "tunnel_client_missing"
        );
    }

    #[test]
    fn same_size_replacement_with_restored_mtime_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("client");
        fs::write(&target, b"fixture-a").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        let executable = development_executable(&target).unwrap();
        assert_eq!(executable.recheck(), Ok(()));
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        fs::write(&target, b"fixture-b").unwrap();
        fs::File::options()
            .write(true)
            .open(&target)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
        assert_eq!(executable.recheck(), Err("tunnel_client_replaced"));
        assert_eq!(executable.recheck_contents(), Err("tunnel_client_replaced"));
    }

    #[test]
    fn pinned_recheck_rehashes_the_contents() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("client");
        fs::write(&target, b"fixture").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        let (_, identity) = hash_file(&target).unwrap();
        let executable = Executable {
            path: target.canonicalize().unwrap(),
            identity,
            pinned: true,
        };
        assert_eq!(executable.recheck(), Ok(()));
        assert_eq!(executable.recheck_contents(), Err("tunnel_client_replaced"));
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
