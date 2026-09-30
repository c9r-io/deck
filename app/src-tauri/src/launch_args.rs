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
pub(crate) fn configure_isolated_carrier() {
    if !cfg!(debug_assertions) {
        return;
    }
    let Some(root) = isolated_carrier_arg("--smoke-data-dir") else {
        return;
    };
    let home = std::path::Path::new(&root).join("home");
    if !home.is_dir() {
        return;
    }
    std::env::set_var("HOME", &home);
    std::env::set_var("CLAUDE_CONFIG_DIR", home.join(".claude"));
    std::env::set_var("CODEX_HOME", home.join(".codex"));
}
