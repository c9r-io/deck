fn main() {
    println!("cargo:rerun-if-env-changed=DECK_BUILD_COMMIT");
    let supplied = std::env::var("DECK_BUILD_COMMIT").ok();
    let discovered = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok());
    let commit = supplied
        .or(discovered)
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| {
            (7..=40).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_hexdigit())
        })
        .unwrap_or_else(|| "dev".into());
    println!("cargo:rustc-env=DECK_BUILD_COMMIT={commit}");
    // The frontend is embedded into the binary at compile time; without these,
    // cargo doesn't know about it and UI-only edits silently ship stale.
    println!("cargo:rerun-if-changed=../ui/index.html");
    println!("cargo:rerun-if-changed=../ui/vendor");
    println!("cargo:rerun-if-changed=../ui/js");
    println!("cargo:rerun-if-changed=../ui/test");
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
