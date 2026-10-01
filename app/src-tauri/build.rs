/// Build signed bundle sidecars before tauri validates `externalBin`.
/// A separate target directory avoids the cargo-in-cargo workspace lock.
/// None is installed under the user's home directory: status-helper is used
/// by explicit agent hooks, deck-mcp is a user-launched STDIO adapter, and
/// deck-mcp-runner exists only as an MCP-managed tmux pane process.
fn build_sidecars() {
    let triple = std::env::var("TARGET").expect("cargo sets TARGET");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    // Sidecars follow the outer profile. tauri-build copies each sidecar
    // over the workspace binary of the same name in the outer target dir, so
    // a debug build must place debug sidecars there: debug-only test seams
    // (the adapter's --socket) exist only in debug binaries, and a release
    // sidecar would silently replace the binary the adapter tests execute.
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let profile = if release { "release" } else { "debug" };
    // A `cargo llvm-cov` run instruments the sidecars too (its RUSTC_WRAPPER
    // is inherited below) and keeps them in their own target dir. With one
    // shared dir tauri-build's copy handed the adapter/runner integration tests
    // whichever sidecar was built last — usually uninstrumented — and their
    // coverage silently read 0%.
    println!("cargo:rerun-if-env-changed=CARGO_LLVM_COV");
    let coverage = std::env::var_os("CARGO_LLVM_COV").is_some();
    for (package, manifest, binary, target_dir) in [
        (
            "status-helper",
            "status-helper/Cargo.toml",
            "deck-status-helper",
            "status-helper/target",
        ),
        (
            "mcp-runner",
            "mcp-runner/Cargo.toml",
            "deck-mcp-runner",
            "mcp-runner/target",
        ),
        (
            "mcp-adapter",
            "mcp-adapter/Cargo.toml",
            "deck-mcp",
            "mcp-adapter/target",
        ),
    ] {
        println!("cargo:rerun-if-changed={package}/src");
        println!("cargo:rerun-if-changed={manifest}");
        let target_dir = if coverage {
            format!("{target_dir}/llvm-cov")
        } else {
            target_dir.to_string()
        };
        let started = std::time::Instant::now();
        let mut command = std::process::Command::new(&cargo);
        command.arg("build");
        if release {
            command.arg("--release");
        }
        let output = command
            .args([
                "--locked",
                "--manifest-path",
                manifest,
                "--target-dir",
                &target_dir,
                "--target",
                &triple,
            ])
            // Outer RUSTFLAGS and a lint (clippy) workspace wrapper must not
            // leak into the sidecar build; cargo-llvm-cov's RUSTC_WRAPPER is
            // kept on purpose (above).
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTC_WORKSPACE_WRAPPER")
            .output()
            .unwrap_or_else(|_| panic!("failed to run cargo for {package}"));
        // passed through, so `cargo -vv` and a failure still show the nested build
        let log = String::from_utf8_lossy(&output.stderr);
        eprint!("{log}");
        assert!(output.status.success(), "{package} build failed");
        // the bare word: CARGO_TERM_COLOR=always (CI) wraps the status word
        // in ANSI escapes, so neither a prefix nor a trailing space matches
        let compiled = log
            .lines()
            .filter(|line| line.contains("Compiling"))
            .count();
        timing(
            &format!("sidecar {package}"),
            started,
            &format!("cargo build ran, {compiled} crates compiled ({target_dir})"),
        );
        let built = format!("{target_dir}/{triple}/{profile}/{binary}");
        let dest = format!("binaries/{binary}-{triple}");
        std::fs::copy(&built, &dest).unwrap_or_else(|_| panic!("failed to place {binary} sidecar"));
    }
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
    let started = std::time::Instant::now();
    let release = std::env::var("PROFILE").as_deref() == Ok("release");
    let dist = std::path::Path::new("ui-dist");
    let _ = std::fs::remove_dir_all(dist);
    copy_tree(
        std::path::Path::new("../ui"),
        dist,
        if release { Some("test") } else { None },
    );
    timing("frontend staging", started, "ui-dist staged from scratch");
}

// Swift is compiled and statically linked at build time, never spawned by Deck.
// New Speech APIs remain availability-guarded; older macOS uses local-only SF.
// The notification bridge (UNUserNotificationCenter) is compiled into the same
// library: one object and archive for voice, notifications, input source and translation.
fn build_native_bridges() {
    println!("cargo:rerun-if-changed=native/SpeechBridge.swift");
    println!("cargo:rerun-if-changed=native/NotificationBridge.swift");
    println!("cargo:rerun-if-changed=native/InputSourceBridge.swift");
    println!("cargo:rerun-if-changed=native/PasteboardBridge.swift");
    println!("cargo:rerun-if-changed=native/SmokeBridge.swift");
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rerun-if-env-changed=DECK_REQUIRE_MODERN_SPEECH");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let arch = if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        "arm64"
    } else {
        "x86_64"
    };
    let started = std::time::Instant::now();
    let object = out.join("NativeBridges.o");
    let mut compiler = std::process::Command::new("xcrun");
    compiler.arg("swiftc");
    // A release must include the same modern path exercised by local device
    // testing. Older developer toolchains may still build the legacy debug app.
    if std::env::var("PROFILE").as_deref() == Ok("release")
        || std::env::var("DECK_REQUIRE_MODERN_SPEECH").as_deref() == Ok("1")
    {
        compiler.args(["-D", "DECK_REQUIRE_MODERN_SPEECH"]);
    }
    // The smoke driver (own-window events, own-webview snapshots, pasteboard
    // guard) exists only in debug builds; a release object never contains it.
    let smoke = std::env::var("PROFILE").as_deref() != Ok("release");
    compiler
        .args(["-parse-as-library", "-swift-version", "5", "-O", "-target"])
        .arg(format!("{arch}-apple-macosx11.0"))
        .args([
            "-emit-object",
            "-whole-module-optimization",
            "native/SpeechBridge.swift",
            "native/NotificationBridge.swift",
            "native/InputSourceBridge.swift",
            "native/PasteboardBridge.swift",
        ]);
    if smoke {
        compiler.arg("native/SmokeBridge.swift");
    }
    let status = compiler
        .arg("-o")
        .arg(&object)
        .status()
        .expect("Swift compiler is required (use the macOS 26 SDK for SpeechAnalyzer)");
    assert!(status.success(), "failed to compile the native bridges");
    let status = std::process::Command::new("xcrun")
        .args(["libtool", "-static", "-o"])
        .arg(out.join("libdeck_native.a"))
        .arg(object)
        .status()
        .expect("failed to archive the native bridges");
    assert!(status.success(), "failed to archive the native bridges");
    timing(
        "native bridges",
        started,
        "swiftc compiled (per OUT_DIR, not shared)",
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=deck_native");
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
        "UserNotifications",
        "Carbon",
        "ImageIO",
        "NaturalLanguage",
        "AppKit",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
    if smoke {
        println!("cargo:rustc-link-lib=framework=WebKit");
    }
}

// Bergamot is repository-owned C++ source. CMake only compiles local inputs;
// it must never fetch source or a model. The resulting archives are merged
// into one static archive so Deck ships no executable translation sidecar.
//
// The CMake build directory is SHARED by every cargo context of this checkout
// (check/clippy, test, `cargo llvm-cov`, release) instead of living in each
// context's OUT_DIR: the C++ compile does not depend on the cargo profile —
// it is always CMake `Release` with the arguments below, and nothing cargo or
// cargo-llvm-cov sets for a build script is read by CMake or this tree (see
// `BERGAMOT_ENV`). One directory per configuration identity, under
// `target/native/`: the configure arguments, the target arch, the resolved
// compiler, SDK and CMake, and every environment variable CMake would bake in
// at configure time. A different identity gets a different directory and
// never inherits objects from another. Within one directory make's own
// dependency tracking (including the system and Homebrew headers it
// depfiles) decides what is stale, so an edit to `vendor/bergamot` or the
// bridge recompiles exactly the affected objects. A lock serializes
// concurrent cargo processes on the same directory. The merged archive is
// still produced per OUT_DIR, so each context links its own copy.
const BERGAMOT_ENV: [&str; 17] = [
    "CMAKE",
    "CC",
    "CXX",
    "CFLAGS",
    "CXXFLAGS",
    "CPPFLAGS",
    "LDFLAGS",
    "SDKROOT",
    "MACOSX_DEPLOYMENT_TARGET",
    "DEVELOPER_DIR",
    "CMAKE_GENERATOR",
    "CMAKE_TOOLCHAIN_FILE",
    // read by clang itself on every compile
    "CPATH",
    "C_INCLUDE_PATH",
    "CPLUS_INCLUDE_PATH",
    "LIBRARY_PATH",
    "CCC_OVERRIDE_OPTIONS",
];

fn tool_output(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".into())
}

fn build_bergamot() {
    println!("cargo:rerun-if-changed=vendor/bergamot");
    println!("cargo:rerun-if-changed=native/BergamotBridge.cpp");
    for name in BERGAMOT_ENV {
        println!("cargo:rerun-if-env-changed={name}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => ("arm64", "armv8-a"),
        Ok("x86_64") => ("x86_64", "core2"),
        other => panic!("unsupported Bergamot target arch: {other:?}"),
    };
    let cmake = std::env::var("CMAKE").unwrap_or_else(|_| "cmake".into());
    let configure = [
        "-DCMAKE_BUILD_TYPE=Release".to_string(),
        "-DCMAKE_POLICY_VERSION_MINIMUM=3.5".into(),
        "-DCMAKE_OSX_DEPLOYMENT_TARGET=11.0".into(),
        "-DUSE_STATIC_LIBS=ON".into(),
        "-DCOMPILE_CPU=ON".into(),
        "-DCOMPILE_CUDA=OFF".into(),
        format!("-DCMAKE_OSX_ARCHITECTURES={}", arch.0),
        format!("-DBUILD_ARCH={}", arch.1),
    ];
    let mut identity = format!("deck-bergamot-build/1\nconfigure {}\n", configure.join(" "));
    for name in BERGAMOT_ENV {
        identity += &format!("env {name}={:?}\n", std::env::var_os(name));
    }
    identity += &format!("cmake {}\n", tool_output(&cmake, &["--version"]));
    for (label, args) in [
        ("c++", &["--find", "c++"][..]),
        ("clang++", &["clang++", "--version"][..]),
        ("sdk", &["--show-sdk-path"][..]),
        ("sdk-version", &["--show-sdk-version"][..]),
    ] {
        identity += &format!("{label} {}\n", tool_output("xcrun", args));
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        identity.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    };
    let root = std::path::Path::new("target/native");
    std::fs::create_dir_all(root).expect("create the native build directory");
    let lock = std::fs::File::create(root.join(format!("bergamot-{key}.lock")))
        .expect("create the Bergamot build lock");
    lock.lock().expect("lock the Bergamot build directory");
    let build = std::env::current_dir()
        .expect("build script directory")
        .join(root)
        .join(format!("bergamot-{key}"));
    // The identity is recorded in full: a directory whose record differs (a
    // key collision, an interrupted configure) is discarded, never reused.
    let record = build.join("deck-build-identity.txt");
    let started = std::time::Instant::now();
    let configured = std::fs::read_to_string(&record).ok().as_deref() == Some(identity.as_str());
    if !configured {
        let _ = std::fs::remove_dir_all(&build);
        let status = std::process::Command::new(&cmake)
            .args(["-S", "vendor/bergamot", "-B"])
            .arg(&build)
            .args(&configure)
            .status()
            .expect("CMake is required to build vendored Bergamot");
        assert!(status.success(), "Bergamot configure failed");
        std::fs::write(&record, &identity).expect("record the Bergamot build identity");
    }
    let configure_time = started.elapsed();
    let started = std::time::Instant::now();
    let output = std::process::Command::new(&cmake)
        .args(["--build"])
        .arg(&build)
        .args(["--target", "deck_bergamot_bridge", "--parallel", "8"])
        .output()
        .expect("failed to run Bergamot build");
    let log = String::from_utf8_lossy(&output.stdout);
    print!("{log}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "Bergamot static build failed");
    // make prints one "Building C/CXX object" line per object it compiles;
    // zero means the shared directory was already up to date.
    let compiled = log
        .lines()
        .filter(|line| line.contains("Building C"))
        .count();
    timing(
        "bergamot",
        started,
        &format!(
            "{} (configure {:.1}s), make ran, {compiled} objects compiled in target/native/bergamot-{key}",
            if configured { "configured directory reused" } else { "fresh configure" },
            configure_time.as_secs_f64()
        ),
    );
    fn archives(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("read Bergamot build directory") {
            let path = entry.expect("Bergamot build entry").path();
            if path.is_dir() {
                archives(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "a") {
                found.push(path);
            }
        }
    }
    let mut inputs = Vec::new();
    archives(&build, &mut inputs);
    inputs.sort();
    assert!(
        inputs.len() >= 5,
        "Bergamot static dependency closure missing"
    );
    let combined = out.join("libdeck_bergamot.a");
    let status = std::process::Command::new("xcrun")
        .args(["libtool", "-static", "-o"])
        .arg(&combined)
        .args(&inputs)
        .status()
        .expect("failed to merge Bergamot archives");
    assert!(status.success(), "Bergamot archive merge failed");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=deck_bergamot");
    println!("cargo:rustc-link-lib=framework=Accelerate");
    println!("cargo:rustc-link-lib=iconv");
    println!("cargo:rustc-link-lib=pcre2-8");
    println!("cargo:rustc-link-lib=c++");
}

/// One line per build-script stage in the file named by `DECK_BUILD_TIMINGS`
/// (CI sets it; unset, nothing is written): the cargo context, the stage's
/// wall time and what it actually did. Running a build command is reported
/// apart from how much it compiled — make or cargo finding everything up to
/// date is not a rebuild. Best effort: a write failure never fails the build.
fn timing(stage: &str, started: std::time::Instant, detail: &str) {
    use std::io::Write;
    let Some(path) = std::env::var_os("DECK_BUILD_TIMINGS") else {
        return;
    };
    let profile = std::env::var("PROFILE").unwrap_or_default();
    let context = if std::env::var_os("CARGO_LLVM_COV").is_some() {
        "coverage"
    } else if std::env::var_os("RUSTC_WORKSPACE_WRAPPER").is_some() {
        "lint"
    } else {
        "build"
    };
    let line = format!(
        "build.rs [{profile}/{context}] {stage}: {:.1}s, {detail}\n",
        started.elapsed().as_secs_f64()
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

fn main() {
    build_native_bridges();
    build_bergamot();
    build_sidecars();
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
    // compile time; without this, cargo doesn't know about it and UI-only
    // edits silently ship stale. stage_frontend copies ALL of ../ui, so the
    // guard is the whole directory (cargo scans a directory recursively),
    // never a list of its entries: a new top-level file would be bundled
    // without triggering a rebuild.
    println!("cargo:rerun-if-changed=../ui");
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
