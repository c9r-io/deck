//! Temporary directories for unit tests (a test-only module).
//!
//! A test that writes files takes a [`TestDir`]: a fresh, empty directory
//! under the system temporary directory, named `deck-<tag>-<n>-<pid>` so a
//! leftover is attributable, and removed when the value drops, also when an
//! assertion fails. A path that must stay the same for the whole process
//! comes from [`temp_path`] and needs its own removal (the per-process data
//! directory in `datadir.rs` is removed at exit).
//!
//! The gate runs the tests with `TMPDIR` set to an empty directory and
//! `scripts/check-test-tmpdir` fails on anything left there, so a test that
//! keeps a file after it ends is a red gate, not a slow leak. A test that
//! removes its own directory only at its end still leaves it when it fails.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// `deck-<name>-<pid>` under the system temporary directory.
pub(crate) fn temp_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("deck-{name}-{}", std::process::id()))
}

/// One test's private directory, removed when the value drops.
pub(crate) struct TestDir(PathBuf);

impl TestDir {
    pub(crate) fn new(tag: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = temp_path(&format!("{tag}-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a test directory");
        Self(dir)
    }
}

impl std::ops::Deref for TestDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TestDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_dir_is_fresh_unique_and_gone_after_it_drops() {
        let first = TestDir::new("test-dir");
        let second = TestDir::new("test-dir");
        assert_ne!(first.to_path_buf(), second.to_path_buf());
        assert!(first.starts_with(std::env::temp_dir()));
        assert_eq!(std::fs::read_dir(&*first).unwrap().count(), 0, "empty");
        std::fs::create_dir_all(first.join("nested")).unwrap();
        std::fs::write(first.join("nested/file"), b"x").unwrap();
        let kept = first.to_path_buf();
        drop(first);
        assert!(!kept.exists(), "removed with everything in it");
        assert!(second.exists(), "only its own directory");
    }
}
