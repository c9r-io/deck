//! `build.rs` writes two outputs that every cargo context of a checkout
//! shares: the sidecars in `binaries/` and the staged frontend `ui-dist/`
//! (contract: `build/shared_output.rs`). These tests compile that same file
//! and pin its rule against cargo's freshness check, which is by modification
//! time only: a `rerun-if-changed` path is "changed" for a context when it is
//! NEWER than the instant that context's build script was last started.
//!
//! The regression they exist for: a copy that kept the source's modification
//! time let a coverage-instrumented helper built days earlier sit in
//! `binaries/` looking days old, so the next ordinary build did not replace
//! it; and a release build compiled whatever frontend the last debug build
//! had staged. Whether real cargo contexts behave accordingly is not a unit
//! test: it is the build sequence recorded under `docs/evidence/`.

#[path = "../build/shared_output.rs"]
mod shared_output;

use shared_output::{place_file, stage_tree, started, Placed};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A directory of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("deck-shared-output-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        Scratch(dir)
    }

    /// A file with these bytes, this mode and this modification time.
    fn file(&self, name: &str, bytes: &[u8], mode: u32, modified: SystemTime) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).expect("create a fixture directory");
        std::fs::write(&path, bytes).expect("write a fixture");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("mode");
        set_modified(&path, modified);
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn set_modified(path: &Path, modified: SystemTime) {
    std::fs::File::open(path)
        .and_then(|file| file.set_modified(modified))
        .expect("set a fixture's modification time");
}

fn modified(path: &Path) -> SystemTime {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .expect("read a modification time")
}

/// What cargo reads for a `rerun-if-changed` path: a file's own time, or the
/// newest time of a directory and everything below it, directories included.
fn cargo_mtime(path: &Path) -> SystemTime {
    let mut latest = modified(path);
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            latest = latest.max(cargo_mtime(&entry.unwrap().path()));
        }
    }
    latest
}

/// Cargo's rule for one context: the path is changed when it is newer than
/// the instant the context's build script was last started.
fn changed_for(context_last_started: SystemTime, path: &Path) -> bool {
    cargo_mtime(path) > context_last_started
}

fn ago(seconds: u64) -> SystemTime {
    SystemTime::now() - Duration::from_secs(seconds)
}

/// Relative paths of every file below `root`, sorted.
fn listing(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, root, found);
            } else {
                found.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

#[test]
fn a_sidecar_built_long_ago_is_a_change_for_every_other_context() {
    let scratch = Scratch::new("old-build");
    // the coverage context built its helper days ago; this run only places it
    let built = scratch.file("built", b"instrumented helper", 0o755, ago(5 * 86_400));
    // an ordinary build placed its own an hour ago and last started then
    let ordinary_started = ago(3_600);
    let dest = scratch.file("placed", b"ordinary helper", 0o755, ordinary_started);
    let this_started = ago(20);

    assert_eq!(
        place_file(&built, &dest, Some(this_started)).unwrap(),
        Placed::Replaced
    );

    assert_eq!(std::fs::read(&dest).unwrap(), b"instrumented helper");
    assert!(
        changed_for(ordinary_started, &dest),
        "the ordinary context must run its build script again and place its own sidecar"
    );
    assert!(
        !changed_for(this_started, &dest),
        "the placing context must not rebuild for its own placement"
    );
    assert_eq!(
        modified(&dest),
        this_started,
        "the stamp is this run's start, never the source's old modification time"
    );
}

#[test]
fn identical_bytes_are_left_alone() {
    let scratch = Scratch::new("identical");
    let built = scratch.file("built", b"same helper", 0o755, ago(60));
    let placed_at = ago(7_200);
    let dest = scratch.file("placed", b"same helper", 0o755, placed_at);
    let inode = std::fs::metadata(&dest).unwrap().ino();

    assert_eq!(
        place_file(&built, &dest, Some(ago(5))).unwrap(),
        Placed::Unchanged
    );

    // contexts that build the same sidecar (build, test, clippy) do not
    // invalidate each other
    assert_eq!(modified(&dest), placed_at);
    assert_eq!(std::fs::metadata(&dest).unwrap().ino(), inode);
}

#[test]
fn a_missing_destination_is_created_with_the_products_mode() {
    let scratch = Scratch::new("missing");
    let built = scratch.file("built", b"helper", 0o755, ago(86_400));
    let dest = scratch.0.join("placed");
    let this_started = ago(3);

    assert_eq!(
        place_file(&built, &dest, Some(this_started)).unwrap(),
        Placed::Replaced
    );

    assert_eq!(std::fs::read(&dest).unwrap(), b"helper");
    assert_eq!(
        std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert_eq!(modified(&dest), this_started);
    assert_eq!(
        listing(&scratch.0),
        ["built", "placed"],
        "no staging file is left beside the placed sidecar"
    );
}

#[test]
fn a_replacement_is_a_new_file_so_a_running_copy_keeps_its_image() {
    let scratch = Scratch::new("atomic");
    let built = scratch.file("built", b"new helper", 0o755, ago(86_400));
    let dest = scratch.file("placed", b"old helper", 0o755, ago(3_600));
    let inode = std::fs::metadata(&dest).unwrap().ino();

    assert_eq!(
        place_file(&built, &dest, Some(ago(2))).unwrap(),
        Placed::Replaced
    );

    assert_ne!(std::fs::metadata(&dest).unwrap().ino(), inode);
}

#[test]
fn a_placement_made_while_this_script_ran_is_never_hidden() {
    let scratch = Scratch::new("concurrent");
    let built = scratch.file("built", b"this context", 0o755, ago(86_400));
    let this_started = ago(30);
    // another context started after this one and has already placed its own
    let other_started = ago(10);
    let dest = scratch.file("placed", b"other context", 0o755, other_started);

    assert_eq!(
        place_file(&built, &dest, Some(this_started)).unwrap(),
        Placed::Replaced
    );

    assert!(
        changed_for(other_started, &dest),
        "stamping this run's (earlier) start would tell the other context nothing changed"
    );
}

#[test]
fn an_unknown_start_stamps_the_current_time() {
    let scratch = Scratch::new("unknown-start");
    let built = scratch.file("built", b"new helper", 0o755, ago(86_400));
    let other_started = ago(3_600);
    let dest = scratch.file("placed", b"old helper", 0o755, other_started);
    let before = ago(1);

    assert_eq!(place_file(&built, &dest, None).unwrap(), Placed::Replaced);

    assert!(modified(&dest) >= before);
    assert!(changed_for(other_started, &dest));
}

#[test]
fn a_missing_product_fails_and_leaves_the_placed_sidecar_untouched() {
    let scratch = Scratch::new("no-product");
    let placed_at = ago(3_600);
    let dest = scratch.file("placed", b"old helper", 0o755, placed_at);

    assert!(place_file(&scratch.0.join("absent"), &dest, Some(ago(1))).is_err());

    assert_eq!(std::fs::read(&dest).unwrap(), b"old helper");
    assert_eq!(modified(&dest), placed_at);
    assert_eq!(listing(&scratch.0), ["placed"]);
}

#[test]
fn the_start_instant_is_cargos_invocation_file_beside_out_dir() {
    let scratch = Scratch::new("invocation");
    let out_dir = scratch.0.join("out");
    std::fs::create_dir(&out_dir).unwrap();
    assert_eq!(
        started(&out_dir),
        None,
        "without cargo's file the answer is unknown"
    );

    let invoked = ago(42);
    scratch.file("invoked.timestamp", b"", 0o644, invoked);
    assert_eq!(started(&out_dir), Some(invoked));
}

/// A frontend source tree with a production part and a `test` part.
fn frontend(scratch: &Scratch) -> PathBuf {
    let old = ago(30 * 86_400);
    scratch.file("ui/index.html", b"<html>", 0o644, old);
    scratch.file("ui/js/app.js", b"app", 0o644, old);
    scratch.file(
        "ui/js/test/kept.js",
        b"a nested directory named test",
        0o644,
        old,
    );
    scratch.file("ui/test/wk-smoke.mjs", b"smoke", 0o644, old);
    scratch.file("ui/test/fixtures/board.json", b"{}", 0o644, old);
    scratch.0.join("ui")
}

#[test]
fn a_release_staging_drops_the_test_tree_and_is_a_change_for_the_debug_context() {
    let scratch = Scratch::new("release-staging");
    let source = frontend(&scratch);
    let dist = scratch.0.join("ui-dist");
    // a debug build staged everything an hour ago
    let debug_started = ago(3_600);
    assert_eq!(
        stage_tree(&source, &dist, None, Some(debug_started)).unwrap(),
        Placed::Replaced
    );
    assert_eq!(
        listing(&dist),
        [
            "index.html",
            "js/app.js",
            "js/test/kept.js",
            "test/fixtures/board.json",
            "test/wk-smoke.mjs"
        ]
    );
    assert!(!changed_for(debug_started, &dist));

    let release_started = ago(20);
    assert_eq!(
        stage_tree(&source, &dist, Some("test"), Some(release_started)).unwrap(),
        Placed::Replaced
    );

    assert_eq!(
        listing(&dist),
        ["index.html", "js/app.js", "js/test/kept.js"],
        "only the top-level test tree is a development asset"
    );
    assert!(
        changed_for(debug_started, &dist),
        "the debug context must stage its own frontend again"
    );
    assert!(
        !changed_for(release_started, &dist),
        "the staging context must not rebuild for its own staging"
    );
    assert_eq!(cargo_mtime(&dist), release_started);

    // and back: the debug context puts the test tree back
    let debug_again = ago(5);
    assert_eq!(
        stage_tree(&source, &dist, None, Some(debug_again)).unwrap(),
        Placed::Replaced
    );
    assert!(dist.join("test/wk-smoke.mjs").is_file());
    assert!(changed_for(release_started, &dist));
}

#[test]
fn a_tree_that_is_already_staged_is_left_alone() {
    let scratch = Scratch::new("staged");
    let source = frontend(&scratch);
    let dist = scratch.0.join("ui-dist");
    let first = ago(3_600);
    stage_tree(&source, &dist, Some("test"), Some(first)).unwrap();
    let inode = std::fs::metadata(dist.join("index.html")).unwrap().ino();

    // another context of the same profile (test, clippy) stages the same tree
    assert_eq!(
        stage_tree(&source, &dist, Some("test"), Some(ago(5))).unwrap(),
        Placed::Unchanged
    );

    assert_eq!(cargo_mtime(&dist), first);
    assert_eq!(
        std::fs::metadata(dist.join("index.html")).unwrap().ino(),
        inode
    );
}

#[test]
fn an_edited_removed_or_stray_file_is_staged_again_from_scratch() {
    let scratch = Scratch::new("restage");
    let source = frontend(&scratch);
    let dist = scratch.0.join("ui-dist");
    stage_tree(&source, &dist, None, Some(ago(3_600))).unwrap();

    std::fs::write(source.join("js/app.js"), b"edited").unwrap();
    std::fs::remove_file(source.join("index.html")).unwrap();
    // left by an older build, long before this run started
    let stray = scratch.file("ui-dist/stray.js", b"stray", 0o644, ago(1_800));
    set_modified(stray.parent().unwrap(), ago(1_800));
    let this_started = ago(2);
    assert_eq!(
        stage_tree(&source, &dist, None, Some(this_started)).unwrap(),
        Placed::Replaced
    );

    assert_eq!(std::fs::read(dist.join("js/app.js")).unwrap(), b"edited");
    assert_eq!(
        listing(&dist),
        [
            "js/app.js",
            "js/test/kept.js",
            "test/fixtures/board.json",
            "test/wk-smoke.mjs"
        ],
        "a file deleted from the source, or never in it, does not stay staged"
    );
    assert_eq!(cargo_mtime(&dist), this_started);
}

#[test]
fn a_staging_made_while_this_script_ran_is_never_hidden() {
    let scratch = Scratch::new("concurrent-staging");
    let source = frontend(&scratch);
    let dist = scratch.0.join("ui-dist");
    let this_started = ago(30);
    // another context started after this one and has already staged its own
    let other_started = ago(10);
    stage_tree(&source, &dist, Some("test"), Some(other_started)).unwrap();

    assert_eq!(
        stage_tree(&source, &dist, None, Some(this_started)).unwrap(),
        Placed::Replaced
    );

    assert!(changed_for(other_started, &dist));
}

/// The module only helps if `build.rs` writes both shared outputs through it
/// and names each as an input of its own; a plain copy would bring the
/// defect back without failing any test above.
#[test]
fn build_rs_writes_both_shared_outputs_through_this_module() {
    let source = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs"))
        .expect("read build.rs");
    assert!(source.contains("#[path = \"build/shared_output.rs\"]"));
    assert_eq!(source.matches("shared_output::place_file(").count(), 1);
    assert_eq!(source.matches("shared_output::stage_tree(").count(), 1);
    assert!(source.contains("println!(\"cargo:rerun-if-changed={dest}\");"));
    assert!(source.contains("println!(\"cargo:rerun-if-changed=ui-dist\");"));
    assert!(
        !source.contains("fs::copy("),
        "std::fs::copy keeps the source's modification time"
    );
    assert!(!source.contains("fn copy_tree"));
}
