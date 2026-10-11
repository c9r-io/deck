//! Writing a build output that every cargo context of a checkout shares.
//! Used by `build.rs` only; `tests/shared_output.rs` compiles this same file
//! to pin its behaviour.
//!
//! Two outputs of `build.rs` live at ONE fixed path each, because tauri reads
//! them from its configuration, while debug, release and `cargo llvm-cov`
//! each produce their own version of them:
//!
//! - `binaries/<sidecar>-<triple>` (`externalBin`): the context's own build
//!   of each sidecar — debug, release, or coverage-instrumented;
//! - `ui-dist/` (`frontendDist`): the frontend, with `ui/test` in debug
//!   profiles and without it in release profiles, and in neither any entry
//!   whose name starts with a dot.
//!
//! So each path holds what the last build script wrote, and everything that
//! packages reads it afterwards (`app/run.sh`,
//! `scripts/package-compat-candidate`, the tauri bundler, the compiler
//! embedding the frontend). A context whose build script did not run again
//! keeps using another context's output. Cargo decides whether a build script
//! runs again by modification time only: a path named by `rerun-if-changed`
//! is "changed" for a context when it is newer than the instant that
//! context's script was last started.
//!
//! The defect this module closes: `std::fs::copy` keeps the SOURCE's
//! modification time. A sidecar another context had built days earlier
//! arrived in `binaries/` looking days old, so the context that built next
//! saw nothing changed and did not place its own. After a coverage run an
//! ordinary build went on packaging instrumented helpers, which write a
//! `.profraw` file into the working directory of the agent that runs them;
//! after a debug build a release build packaged debug sidecars and compiled
//! the debug frontend, `ui/test` included, into the release executable
//! (`ui-dist/` was not an input of the build script at all).
//!
//! The rule, the same for a file and for the tree:
//! - an output that already is what this context produces is left alone, so
//!   contexts that produce the same thing (build, test, clippy) never disturb
//!   each other;
//! - otherwise it is replaced, and everything written is stamped with THIS
//!   run's start instant. To every other context that is newer than its own
//!   last start, so its script runs again and writes its own; to this
//!   context it is not newer, so it does not rebuild for its own writing;
//! - when that instant is unknown, or the output in place is already newer
//!   than it (another context wrote while this script was running), the
//!   stamp is the current time: every context then sees a change, this one
//!   included, which costs one more run of this script and is never wrong.
//!
//! `build.rs` names both paths with `rerun-if-changed`; without that the rule
//! has no effect.
//!
//! This keeps each path right after a build of a context. It cannot make a
//! package right when a DIFFERENT context builds between a build and its
//! packaging step (an editor running `cargo check` during a release build):
//! the packaging paths therefore compare what they package with the build
//! tree (`scripts/sidecar_provenance.py`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a write did, for the build log.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Placed {
    /// The output in place already was this context's; nothing was written.
    Unchanged,
    /// It differed (or nothing was there) and was replaced.
    Replaced,
}

/// The instant cargo started this build-script run: the reference its
/// `rerun-if-changed` check uses on the next build. Cargo records it as the
/// modification time of `invoked.timestamp` beside `OUT_DIR`; that file is
/// cargo's own bookkeeping, so its absence is an answer (`None`), not an
/// error.
pub fn started(out_dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(out_dir.parent()?.join("invoked.timestamp"))
        .ok()?
        .modified()
        .ok()
}

/// The stamp for a replacement: this run's start, unless it is unknown or
/// what is in place is already newer than it.
fn stamp(started: Option<SystemTime>, in_place: Option<SystemTime>) -> SystemTime {
    match started {
        Some(start) if in_place.is_none_or(|existing| existing <= start) => start,
        _ => SystemTime::now(),
    }
}

/// Works for a directory too: the owner may set its times through a
/// read-only handle.
fn set_modified(path: &Path, modified: SystemTime) -> std::io::Result<()> {
    std::fs::File::open(path)?.set_modified(modified)
}

/// Put the file `built` at `dest`.
pub fn place_file(
    built: &Path,
    dest: &Path,
    started: Option<SystemTime>,
) -> std::io::Result<Placed> {
    let bytes = std::fs::read(built)?;
    if std::fs::read(dest).ok().as_deref() == Some(bytes.as_slice()) {
        return Ok(Placed::Unchanged);
    }
    let in_place = std::fs::metadata(dest)
        .and_then(|meta| meta.modified())
        .ok();
    let name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("the destination has no file name"))?;
    // A new file renamed over the old one: a running copy keeps its image,
    // and no reader sees half a file. The staging name still matches the
    // `binaries/<sidecar>-*` ignore patterns if a run dies before the rename.
    let staged = dest.with_file_name(format!("{name}.placing-{}", std::process::id()));
    let result = (|| {
        std::fs::write(&staged, &bytes)?;
        std::fs::set_permissions(&staged, std::fs::metadata(built)?.permissions())?;
        set_modified(&staged, stamp(started, in_place))?;
        std::fs::rename(&staged, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result.map(|()| Placed::Replaced)
}

/// Every file below `root` by its path relative to `root`, without the
/// top-level entry named `skip`. `dot_entries` says whether an entry whose
/// name starts with a dot counts, at any depth: it never does for a SOURCE
/// tree (see `stage_tree`) and always does for the tree in place, so one
/// that was staged there earlier makes the two differ.
fn files(
    root: &Path,
    skip: Option<&str>,
    dot_entries: bool,
) -> std::io::Result<BTreeMap<PathBuf, PathBuf>> {
    fn walk(
        dir: &Path,
        relative: &Path,
        skip: Option<&str>,
        dot_entries: bool,
        found: &mut BTreeMap<PathBuf, PathBuf>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            if skip.is_some_and(|skipped| name == skipped)
                || (!dot_entries && name.as_encoded_bytes().first() == Some(&b'.'))
            {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                walk(&path, &relative.join(&name), None, dot_entries, found)?;
            } else {
                found.insert(relative.join(&name), path);
            }
        }
        Ok(())
    }
    let mut found = BTreeMap::new();
    walk(root, Path::new(""), skip, dot_entries, &mut found)?;
    Ok(found)
}

/// The newest modification time of `root` and everything below it,
/// directories included: what cargo compares for a `rerun-if-changed`
/// directory. `None` when `root` is absent.
fn newest(root: &Path) -> Option<SystemTime> {
    let mut latest = std::fs::metadata(root).ok()?.modified().ok()?;
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        let modified = if path.is_dir() {
            newest(&path)
        } else {
            entry.metadata().ok().and_then(|meta| meta.modified().ok())
        };
        if let Some(modified) = modified {
            latest = latest.max(modified);
        }
    }
    Some(latest)
}

/// Make `dest` the tree `source` without its top-level entry `skip`: the
/// same files with the same bytes, and nothing else. An entry of `source`
/// whose name starts with a dot is not part of it at any depth: what a tool,
/// an editor or Finder leaves in the source directory (`.playwright-cli/`,
/// `.DS_Store`) is local state, usually ignored by git, and staging it would
/// compile it into the executable of every build made on that machine.
pub fn stage_tree(
    source: &Path,
    dest: &Path,
    skip: Option<&str>,
    started: Option<SystemTime>,
) -> std::io::Result<Placed> {
    let wanted = files(source, skip, false)?;
    let same = |staged: &BTreeMap<PathBuf, PathBuf>| {
        staged.len() == wanted.len()
            && wanted.iter().all(|(relative, from)| {
                staged.get(relative).is_some_and(|to| {
                    matches!((std::fs::read(from), std::fs::read(to)), (Ok(a), Ok(b)) if a == b)
                })
            })
    };
    if files(dest, None, true).is_ok_and(|staged| same(&staged)) {
        return Ok(Placed::Unchanged);
    }
    let stamp = stamp(started, newest(dest));
    // From scratch: a file deleted from the source must not stay staged.
    match std::fs::remove_dir_all(dest) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::fs::create_dir_all(dest)?;
    let mut directories = vec![dest.to_path_buf()];
    for (relative, from) in &wanted {
        let to = dest.join(relative);
        if let Some(parent) = to.parent() {
            if !parent.is_dir() {
                std::fs::create_dir_all(parent)?;
                directories.extend(
                    parent
                        .ancestors()
                        .take_while(|ancestor| *ancestor != dest)
                        .map(Path::to_path_buf),
                );
            }
        }
        std::fs::copy(from, &to)?;
        set_modified(&to, stamp)?;
    }
    // last: creating an entry moves its directory's time
    for directory in directories {
        set_modified(&directory, stamp)?;
    }
    Ok(Placed::Replaced)
}
