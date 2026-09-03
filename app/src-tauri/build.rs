/// Build the status-helper sidecar (a standalone zero-dependency crate) and
/// place it where tauri's externalBin expects it. tauri_build validates that
/// path on EVERY cargo build — not just at bundle time — so the helper must
/// exist before `tauri_build::build()` runs. Always release profile: the
/// artifact ships inside every bundle, debug ones included. A separate
/// target dir avoids the cargo-in-cargo file lock.
fn build_status_helper() {
    println!("cargo:rerun-if-changed=../status-helper/src");
    println!("cargo:rerun-if-changed=../status-helper/Cargo.toml");
    let triple = std::env::var("TARGET").expect("cargo sets TARGET");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = std::process::Command::new(cargo)
        .args([
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            "../status-helper/Cargo.toml",
            "--target-dir",
            "../status-helper/target",
            "--target",
            &triple,
        ])
        // a coverage/lint wrapper around the OUTER build must not leak into
        // the sidecar build (llvm-cov's flags would corrupt the artifact)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .status()
        .expect("failed to run cargo for status-helper");
    assert!(status.success(), "status-helper build failed");
    let built = format!("../status-helper/target/{triple}/release/deck-status-helper");
    let dest = format!("binaries/deck-status-helper-{triple}");
    std::fs::copy(&built, &dest).expect("failed to place status-helper sidecar");
}

fn main() {
    build_status_helper();
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
