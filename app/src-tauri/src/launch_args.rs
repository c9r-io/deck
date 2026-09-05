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
}
