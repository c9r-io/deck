//! Launch arguments. `--debug-logging` is the one flag a release build honours;
//! every `--smoke-*` value is read only by debug bundles (`debug_arg`), so
//! the isolated WKWebView smoke can never redirect a shipped deck.
//! The explicit re-entry experiment additionally binds its project settings,
//! private data/socket and bundle to one temporary root; invalid input exits
//! before any data access. It preserves the real HOME and never uses fixtures.

/// A narrowly scoped, debug-only project hook file. No global config fallback
/// is allowed when this explicit experiment argument is present.
pub(crate) fn reentry_project_config() -> Result<Option<std::path::PathBuf>, &'static str> {
    if !cfg!(debug_assertions) {
        return Ok(None);
    }
    let Some(config) = debug_arg("--smoke-reentry-config") else {
        if command_flag("--smoke-reentry-config") {
            return Err("incomplete reentry isolation");
        }
        return Ok(None);
    };
    let data = debug_arg("--smoke-data-dir").ok_or("missing isolated data")?;
    let socket = debug_arg("--smoke-tmux-socket").ok_or("missing isolated socket")?;
    if debug_arg("--smoke-wkwebview").is_some() {
        return Err("reentry experiment cannot use fixtures");
    }
    let exe = std::env::current_exe().map_err(|_| "missing carrier")?;
    validate_reentry_paths(&data, &socket, &config, &exe).map(Some)
}

fn validate_reentry_paths(
    data: &str,
    socket: &str,
    config: &str,
    exe: &std::path::Path,
) -> Result<std::path::PathBuf, &'static str> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Component, Path};
    let data = Path::new(data);
    let root = data.parent().ok_or("missing experiment root")?;
    let root_name = root
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or("invalid root")?;
    let suffix = root_name
        .strip_prefix("deck-reentry-")
        .ok_or("invalid root")?;
    if suffix.is_empty()
        || !suffix.bytes().all(|b| b.is_ascii_hexdigit())
        || root.parent() != Some(Path::new("/private/tmp"))
        || data != root.join("data")
        || socket != format!("deck-smoke-reentry-{suffix}")
        || socket.len() > 48
        || Path::new(config) != root.join("work/.claude/settings.json")
        || exe != root.join("deck.app/Contents/MacOS/deck")
    {
        return Err("reentry isolation binding mismatch");
    }
    for path in [root, data, Path::new(config), exe] {
        if path.components().any(|c| matches!(c, Component::ParentDir))
            || std::fs::canonicalize(path).ok().as_deref() != Some(path)
        {
            return Err("reentry path must be canonical and unlinked");
        }
    }
    for path in [root, data, &root.join("work"), &root.join("work/.claude")] {
        let m = std::fs::symlink_metadata(path).map_err(|_| "missing isolated directory")?;
        if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
            return Err("isolated directory must be user owned and private");
        }
    }
    let m = std::fs::symlink_metadata(config).map_err(|_| "missing project settings")?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || m.len() > 65536
    {
        return Err("invalid project settings file");
    }
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config).map_err(|_| "unreadable project settings")?)
            .map_err(|_| "invalid project settings JSON")?;
    if !value.is_object() {
        return Err("project settings must be an object");
    }
    let helper = exe
        .parent()
        .ok_or("missing carrier")?
        .join("deck-lifecycle-helper");
    let m = std::fs::symlink_metadata(&helper).map_err(|_| "missing lifecycle helper")?;
    if !m.is_file() || m.file_type().is_symlink() || m.permissions().mode() & 0o111 == 0 {
        return Err("invalid lifecycle helper");
    }
    Ok(Path::new(config).to_path_buf())
}

/// Exact boolean launch flag. Unlike the isolated smoke arguments below,
/// --debug-logging is intentionally available in release builds so a
/// maintainer can reproduce a packaged WKWebView/input problem without
/// exposing a developer control in Settings.
pub(crate) fn command_flag(name: &str) -> bool {
    let expected = std::ffi::OsStr::new(name);
    std::env::args_os().any(|arg| arg.as_os_str() == expected)
}

/// Debug bundles accept an isolated data root for packaged WKWebView smoke
/// tests. Release builds ignore this argument completely.
pub(crate) fn debug_arg(name: &str) -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let args: Vec<String> = std::env::args().collect();
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].clone())
        .or_else(|| isolated_carrier_arg(name))
}

/// LaunchServices notification cold starts have no command-line arguments.
/// A debug-only carrier can retain its isolated launch identity in its signed
/// Resources directory. Release builds never read this test configuration.
fn isolated_carrier_arg(name: &str) -> Option<String> {
    if !cfg!(debug_assertions)
        || !matches!(
            name,
            "--smoke-data-dir" | "--smoke-tmux-socket" | "--smoke-wkwebview"
        )
    {
        return None;
    }
    let executable = std::env::current_exe().ok()?;
    let contents = executable.parent()?.parent()?;
    if contents.file_name()? != "Contents" {
        return None;
    }
    let bytes = std::fs::read(contents.join("Resources/deck-smoke-launch.json")).ok()?;
    let config: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let root = config.get("--smoke-data-dir")?.as_str()?;
    let socket = config.get("--smoke-tmux-socket")?.as_str()?;
    if !root.starts_with("/tmp/deck-reminder-")
        || !socket.starts_with("deck-smoke-reminder-")
        || socket.len() > 64
        || !socket
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return None;
    }
    config.get(name)?.as_str().map(str::to_owned)
}

/// Establish private process configuration before starting any threads, also
/// on a LaunchServices cold start without argv. Never changes the login
/// environment, normal Agent trees, launchd or the caller's environment.
///
/// The `channel-first-send` debug carrier additionally gets a closed PATH
/// containing its signed, bundle-local harmless `claude` fixture plus system
/// utilities, and a private TMPDIR. Direct argv is validated independently of
/// the Reminder cold-start resource above; neither route exists in a release
/// build.
pub(crate) fn channel_fixture_bin() -> Option<std::path::PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let executable = std::fs::canonicalize(std::env::current_exe().ok()?).ok()?;
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    let bundle_name = bundle.file_name()?.to_str()?;
    if executable.file_name()?.to_str()? != "deck"
        || macos.file_name()?.to_str()? != "MacOS"
        || contents.file_name()?.to_str()? != "Contents"
        || !bundle_name.starts_with("deck-channel-smoke-")
        || !bundle_name.ends_with(".app")
        || bundle_name.len() <= "deck-channel-smoke-.app".len()
    {
        return None;
    }
    let bin = std::fs::canonicalize(macos.join("channel-fixture-bin")).ok()?;
    let fixture = bin.join("claude");
    let metadata = std::fs::symlink_metadata(&fixture).ok()?;
    use std::os::unix::fs::PermissionsExt;
    if bin.parent() != Some(macos)
        || !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return None;
    }
    Some(bin)
}

pub(crate) fn configure_isolated_carrier() {
    if let Err(reason) = reentry_project_config() {
        eprintln!("isolated reentry refused: {reason}");
        std::process::exit(2);
    }
    if !cfg!(debug_assertions) {
        return;
    }
    let direct_mode = debug_arg("--smoke-wkwebview");
    let direct_root = debug_arg("--smoke-data-dir");
    let direct_socket = debug_arg("--smoke-tmux-socket");
    let channel = direct_mode.as_deref() == Some("channel-first-send")
        && direct_root
            .as_deref()
            .is_some_and(|v| v.starts_with("/tmp/deck-channel-"))
        && direct_socket.as_deref().is_some_and(|v| {
            v.starts_with("deck-smoke-channel-")
                && v.len() > "deck-smoke-channel-".len()
                && v.len() <= 64
                && v.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
    let Some(root) = (if channel {
        direct_root
    } else {
        isolated_carrier_arg("--smoke-data-dir")
    }) else {
        return;
    };
    let home = std::path::Path::new(&root).join("home");
    let private_dir = |path: &std::path::Path| {
        use std::os::unix::fs::PermissionsExt;
        std::fs::symlink_metadata(path)
            .ok()
            .is_some_and(|metadata| {
                metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && metadata.permissions().mode() & 0o077 == 0
            })
    };
    let canonical_root = std::fs::canonicalize(&root).ok();
    let canonical_home = std::fs::canonicalize(&home).ok();
    if std::path::Path::new(&root)
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
        || !private_dir(std::path::Path::new(&root))
        || !private_dir(&home)
        || channel
            && (canonical_root.as_ref().is_none_or(|path| {
                !path
                    .to_str()
                    .is_some_and(|value| value.starts_with("/private/tmp/deck-channel-"))
            }) || canonical_home
                .as_ref()
                .zip(canonical_root.as_ref())
                .is_none_or(|(actual, root)| actual != &root.join("home")))
    {
        return;
    }
    let channel_paths = channel.then(|| {
        (
            std::path::Path::new(&root).join("tmp"),
            channel_fixture_bin(),
        )
    });
    if channel_paths
        .as_ref()
        .is_some_and(|(tmp, bin)| !private_dir(tmp) || bin.is_none())
    {
        return;
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("ZDOTDIR", &home);
    std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude"));
    std::env::set_var("CODEX_HOME", home.join(".codex"));
    if let Some((tmp, Some(bin))) = channel_paths {
        for name in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "OPENAI_API_KEY",
            "AZURE_OPENAI_API_KEY",
            "CODEX_API_KEY",
            "OPENAI_ORG_ID",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_SESSION_TOKEN",
            "GOOGLE_API_KEY",
            "GEMINI_API_KEY",
        ] {
            std::env::remove_var(name);
        }
        std::env::set_var("TMPDIR", tmp);
        std::env::set_var("PATH", format!("{}:/usr/bin:/bin", bin.display()));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reentry_paths_require_one_private_root_and_real_helper() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::path::PathBuf::from(format!(
            "/private/tmp/deck-reentry-{:x}",
            std::process::id()
        ));
        assert!(!root.exists());
        for dir in [
            "",
            "data",
            "work",
            "work/.claude",
            "deck.app/Contents/MacOS",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
            std::fs::set_permissions(root.join(dir), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let exe = root.join("deck.app/Contents/MacOS/deck");
        let config = root.join("work/.claude/settings.json");
        let helper = exe.parent().unwrap().join("deck-lifecycle-helper");
        std::fs::write(&exe, b"inert carrier").unwrap();
        std::fs::write(&config, b"{}").unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        let data = root.join("data").to_string_lossy().into_owned();
        let socket = format!("deck-smoke-reentry-{:x}", std::process::id());
        let config_text = config.to_string_lossy();
        let check = || super::validate_reentry_paths(&data, &socket, &config_text, &exe);
        assert!(check().is_err(), "missing helper refuses");
        std::fs::write(&helper, b"inert helper").unwrap();
        assert!(check().is_err(), "non-executable helper refuses");
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(check().unwrap(), config);
        assert!(super::validate_reentry_paths(&data, "deck", &config_text, &exe).is_err());
        assert!(
            super::validate_reentry_paths("/Users/u/.deck", &socket, &config_text, &exe).is_err()
        );
        assert!(super::validate_reentry_paths(
            &data,
            &socket,
            "/Users/u/.claude/settings.json",
            &exe
        )
        .is_err());
        std::fs::write(&config, b"not JSON").unwrap();
        assert!(check().is_err(), "malformed configuration refuses");
        std::fs::remove_file(&config).unwrap();
        assert!(check().is_err(), "missing configuration refuses");
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn release_or_test_process_has_no_isolated_carrier_arguments() {
        assert!(super::debug_arg("--smoke-data-dir").is_none());
        assert!(super::debug_arg("--smoke-tmux-socket").is_none());
        assert!(super::reentry_project_config().unwrap().is_none());
    }
}
