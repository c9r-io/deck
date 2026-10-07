//! Launch arguments. `--debug-logging` is the one flag a release build honours;
//! every `--smoke-*` value is read only by debug bundles (`debug_arg`), so
//! the isolated WKWebView smoke can never redirect a shipped deck.

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
    fn release_or_test_process_has_no_isolated_carrier_arguments() {
        assert!(super::debug_arg("--smoke-data-dir").is_none());
        assert!(super::debug_arg("--smoke-tmux-socket").is_none());
    }
}
