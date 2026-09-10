/// Build the status-helper sidecar (a standalone zero-dependency crate) and
/// place it where tauri's externalBin expects it. tauri_build validates that
/// path on EVERY cargo build — not just at bundle time — so the helper must
/// exist before `tauri_build::build()` runs. Always release profile: the
/// artifact ships inside every bundle, debug ones included. A separate
/// target dir avoids the cargo-in-cargo file lock.
fn build_status_helper() {
    println!("cargo:rerun-if-changed=status-helper/src");
    println!("cargo:rerun-if-changed=status-helper/Cargo.toml");
    let triple = std::env::var("TARGET").expect("cargo sets TARGET");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = std::process::Command::new(cargo)
        .args([
            "build",
            "--release",
            "--locked",
            "--manifest-path",
            "status-helper/Cargo.toml",
            "--target-dir",
            "status-helper/target",
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
    let built = format!("status-helper/target/{triple}/release/deck-status-helper");
    let dest = format!("binaries/deck-status-helper-{triple}");
    std::fs::copy(&built, &dest).expect("failed to place status-helper sidecar");
}

/// Stage the no-build frontend for `tauri::generate_context!`: a fresh copy
/// of `../ui` under `ui-dist/` (gitignored). Release profiles drop `ui/test`
/// — the node:test carriers and the WKWebView smoke are development assets
/// and must not ship inside the signed bundle. Debug profiles keep it because
/// `main.rs` imports `./test/wk-smoke.mjs` from the served bundle for the
/// isolated smoke run. Staging from scratch also drops files deleted from
/// `ui/`, which an in-place copy would leave in the bundle.
/// The tauri CLI checks that `frontendDist` EXISTS before it runs cargo, so
/// `tauri build` on a clean checkout dies before this script can stage
/// anything (the 0.5.15 nightly build did). `beforeBuildCommand` in
/// tauri.conf.json therefore only `mkdir -p`s the directory; the content
/// is owned here, and only here.
fn stage_frontend() {
    fn copy_tree(src: &std::path::Path, dst: &std::path::Path, skip: Option<&str>) {
        std::fs::create_dir_all(dst).expect("create ui-dist directory");
        for entry in std::fs::read_dir(src).expect("read ui directory") {
            let entry = entry.expect("ui entry");
            let name = entry.file_name();
            if skip.is_some_and(|s| name == s) {
                continue;
            }
            let from = entry.path();
            let to = dst.join(&name);
            if from.is_dir() {
                copy_tree(&from, &to, None);
            } else {
                std::fs::copy(&from, &to).expect("copy frontend file");
            }
        }
    }
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let dist = std::path::Path::new("ui-dist");
    let _ = std::fs::remove_dir_all(dist);
    copy_tree(
        std::path::Path::new("../ui"),
        dist,
        if release { Some("test") } else { None },
    );
}

// Swift is compiled and statically linked at build time, never spawned by Deck.
// New Speech APIs remain availability-guarded; older macOS uses local-only SF.
fn build_speech_bridge() {
    println!("cargo:rerun-if-changed=native/SpeechBridge.swift");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let object = out.join("SpeechBridge.o");
    let status = std::process::Command::new("xcrun")
        .args([
            "swiftc",
            "-parse-as-library",
            "-swift-version",
            "5",
            "-O",
            "-target",
        ])
        .arg(format!("{arch}-apple-macosx11.0"))
        .args([
            "-emit-object",
            "-whole-module-optimization",
            "native/SpeechBridge.swift",
            "-o",
        ])
        .arg(&object)
        .status()
        .expect("Swift compiler is required (use the macOS 26 SDK for SpeechAnalyzer)");
    assert!(status.success(), "failed to compile native Speech bridge");
    let status = std::process::Command::new("xcrun")
        .args(["libtool", "-static", "-o"])
        .arg(out.join("libdeck_speech.a"))
        .arg(object)
        .status()
        .expect("failed to archive Speech bridge");
    assert!(status.success(), "failed to archive Speech bridge");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=deck_speech");
    let compiler = std::process::Command::new("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .expect("find Swift runtime libraries");
    let compiler = std::path::PathBuf::from(String::from_utf8(compiler.stdout).unwrap().trim());
    let lib = compiler
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("lib/swift/macosx");
    println!("cargo:rustc-link-search=native={}", lib.display());
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    for framework in [
        "Foundation",
        "AVFoundation",
        "Speech",
        "CoreMedia",
        "AudioToolbox",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}

fn main() {
    build_speech_bridge();
    build_status_helper();
    stage_frontend();
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
    // The frontend is staged into ui-dist/ and embedded into the binary at
    // compile time; without these, cargo doesn't know about it and UI-only
    // edits silently ship stale.
    println!("cargo:rerun-if-changed=../ui/index.html");
    println!("cargo:rerun-if-changed=../ui/style.css");
    println!("cargo:rerun-if-changed=../ui/vendor");
    println!("cargo:rerun-if-changed=../ui/js");
    println!("cargo:rerun-if-changed=../ui/test");
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
