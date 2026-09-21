//! Descriptor-relative, no-shell filesystem reads for Deck MCP.
//!
//! Every component is opened below an already opened root with `openat`,
//! `O_NOFOLLOW` and `O_NONBLOCK`; the file type is then checked on that same
//! descriptor with `fstat`, so a FIFO, socket or device can neither block the
//! reader nor be swapped in between a check and the open. Callers never
//! validate one pathname and then reopen another. Symlinks and non-regular
//! files are rejected.
//!
//! Name policy (`allowed_child`): credential, history and key material names
//! are excluded from list, read, identity and search alike. Comparison folds
//! ASCII case plus the two non-ASCII characters (`ſ`, `K`) whose case folding
//! yields ASCII letters, because APFS is case-insensitive; dot-file names that
//! contain any other non-ASCII character are refused outright. This is a
//! defense-in-depth policy for structured reads, not a promise that unknown
//! names or hard links cannot contain secrets.
//!
//! Root policy (`root_policy`): an authorized root may not be `/`, the account
//! home (resolved with `getpwuid_r`, never `$HOME`) or one of its ancestors,
//! and no component of the root may be an excluded name. It is enforced when a
//! root is authorized and again on every open, so a root stored by an older
//! build is refused rather than silently narrowed. The root's device/inode
//! are not recorded at authorization (the stored scope is a path string); a
//! replaced root parent is followed, which is a documented residual.
//!
//! Errors carry a closed `FsErrorKind` so callers can tell argument errors,
//! denials (including absent paths — absence is not distinguished from
//! exclusion), size limits, concurrent changes and cancellation apart without
//! learning anything about out-of-scope files.
//!
//! This is an interface boundary for structured reads, not file isolation: an
//! approved trusted-host job runs with every permission of the Deck account and
//! can read anything this policy excludes. It is also not a sandbox against
//! other code running as the same uid.

use serde::Serialize;
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

pub(crate) const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub(crate) const MAX_READ_BYTES: usize = 16 * 1024;
pub(crate) const MAX_LIST_ENTRIES: usize = 32;
pub(crate) const MAX_SEARCH_FILES: usize = 1_000;
pub(crate) const MAX_SEARCH_RESULTS: usize = 200;
pub(crate) const MAX_SEARCH_DEPTH: usize = 8;
/// Entries one search visits per directory; beyond this the directory is
/// searched partially and the outcome is marked incomplete.
const MAX_SEARCH_DIRECTORY_ENTRIES: usize = 4_096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FsErrorKind {
    /// The request itself is malformed (path shape, query, byte limit, cursor).
    Invalid,
    /// The path is absent, excluded, special, a link, binary or not UTF-8.
    Denied,
    /// The target exceeds a fixed size bound.
    Limit,
    /// The target changed while it was being read.
    Changed,
    /// The caller withdrew authorization during a search.
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FsError {
    pub(crate) kind: FsErrorKind,
    pub(crate) message: &'static str,
}

fn fail(kind: FsErrorKind, message: &'static str) -> FsError {
    FsError { kind, message }
}

fn invalid(message: &'static str) -> FsError {
    fail(FsErrorKind::Invalid, message)
}

fn denied(message: &'static str) -> FsError {
    fail(FsErrorKind::Denied, message)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileVersion {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) kind: &'static str,
    pub(crate) size: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct Listing {
    pub(crate) entries: Vec<Entry>,
    pub(crate) version: FileVersion,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Match {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) preview: String,
}

pub(crate) enum SearchControl {
    Continue,
    Deadline,
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct SearchOutcome {
    pub(crate) matches: Vec<Match>,
    pub(crate) complete: bool,
    pub(crate) stop_reason: Option<&'static str>,
    /// Regular files or directory entries the search should have covered but
    /// could not (too large, unreadable, invalid UTF-8, beyond depth or entry
    /// bounds). Binary files are excluded by policy and are not counted.
    pub(crate) skipped: usize,
}

fn components(path: &str) -> Result<Vec<&OsStr>, FsError> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(invalid("absolute paths are not allowed"));
    }
    let mut values = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => values.push(value),
            Component::CurDir => {}
            _ => return Err(invalid("path traversal is not allowed")),
        }
    }
    Ok(values)
}

/// Lowercases ASCII and maps the non-ASCII code points whose Unicode case
/// folding is an ASCII letter, so `.\u{17f}sh`-style spellings that APFS would
/// resolve to an excluded name compare equal to it.
fn folded(name: &str) -> String {
    name.chars()
        .map(|character| match character {
            '\u{17f}' => 's',
            '\u{212a}' => 'k',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

fn allowed_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    if name.starts_with('.') && !name.is_ascii() {
        return false;
    }
    let lower = folded(name);
    if lower == ".env.example" || lower == ".env.sample" {
        return true;
    }
    const EXCLUDED: &[&str] = &[
        ".git",
        ".ssh",
        ".gnupg",
        ".aws",
        ".kube",
        ".deck",
        ".docker",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".pgpass",
        ".vault-token",
        ".git-credentials",
        ".credentials.json",
        "credentials",
        "credentials.json",
        "application_default_credentials.json",
    ];
    const PRIVATE_KEYS: &[&str] = &["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"];
    const SUFFIXES: &[&str] = &[
        ".pem",
        ".p12",
        ".pfx",
        ".key",
        ".tfstate",
        ".tfstate.backup",
    ];
    !EXCLUDED.contains(&lower.as_str())
        && !lower.starts_with(".env")
        && !(lower.starts_with('.') && lower.ends_with("_history"))
        && !(PRIVATE_KEYS.iter().any(|key| lower.starts_with(key)) && !lower.ends_with(".pub"))
        && !SUFFIXES.iter().any(|suffix| lower.ends_with(suffix))
}

/// Names that are only sensitive below a particular parent directory.
fn allowed_child(parent: Option<&OsStr>, name: &OsStr) -> bool {
    if !allowed_name(name) {
        return false;
    }
    let (Some(parent), Some(name)) = (parent.and_then(OsStr::to_str), name.to_str()) else {
        return true;
    };
    !matches!(
        (folded(parent).as_str(), folded(name).as_str()),
        (".config", "gh") | (".config", "gcloud") | (".codex", "auth.json")
    )
}

/// The account home from the password database, never from `$HOME`.
pub(crate) fn home_directory() -> Option<PathBuf> {
    let mut buffer = vec![0 as libc::c_char; 4096];
    // SAFETY: zero is a valid initialization for passwd.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    // SAFETY: every pointer is live for the call and buffer length is exact.
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into buffer and is NUL terminated.
    let home = unsafe { CStr::from_ptr(entry.pw_dir) };
    let home = PathBuf::from(OsStr::from_bytes(home.to_bytes()));
    Some(std::fs::canonicalize(&home).unwrap_or(home))
}

/// Refuses roots that would expose the whole machine or account, or that sit
/// inside an excluded directory. `root` must already be canonical.
pub(crate) fn root_policy(root: &Path) -> Result<(), FsError> {
    let too_broad = || denied("authorized root is too broad or sensitive");
    if !root.is_absolute() || root.parent().is_none() {
        return Err(too_broad());
    }
    let home = home_directory().ok_or_else(|| denied("account home is unavailable"))?;
    if home.starts_with(root) {
        return Err(too_broad());
    }
    let mut parent = None;
    for component in root.components() {
        if let Component::Normal(name) = component {
            if !allowed_child(parent, name) {
                return Err(too_broad());
            }
            parent = Some(name);
        }
    }
    Ok(())
}

fn cstring(value: &OsStr) -> Result<CString, FsError> {
    CString::new(value.as_bytes()).map_err(|_| invalid("path contains a NUL byte"))
}

fn open_root(root: &Path) -> Result<OwnedFd, FsError> {
    root_policy(root)?;
    let value = cstring(root.as_os_str())?;
    // SAFETY: value is NUL terminated and the returned descriptor is owned.
    let fd = unsafe {
        libc::open(
            value.as_ptr(),
            libc::O_RDONLY
                | libc::O_DIRECTORY
                | libc::O_CLOEXEC
                | libc::O_NOFOLLOW
                | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        Err(denied("authorized root is unavailable or was replaced"))
    } else {
        // SAFETY: open returned a new descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

/// Opens `relative` below `root` without blocking and without following
/// links. The caller checks the type on the returned descriptor.
fn open_relative(root: &Path, relative: &str) -> Result<OwnedFd, FsError> {
    let mut current = open_root(root)?;
    let values = components(relative)?;
    let mut parent = root.file_name();
    for (index, value) in values.iter().enumerate() {
        if !allowed_child(parent, value) {
            return Err(denied("sensitive or private path is excluded"));
        }
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        if index + 1 < values.len() {
            flags |= libc::O_DIRECTORY;
        }
        let name = cstring(value)?;
        // SAFETY: current is live and name is NUL terminated.
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(denied("path is unavailable, excluded, or a symbolic link"));
        }
        // SAFETY: openat returned a new descriptor.
        current = unsafe { OwnedFd::from_raw_fd(fd) };
        parent = Some(value);
    }
    Ok(current)
}

fn stat(fd: &OwnedFd) -> Result<libc::stat, FsError> {
    // SAFETY: zero is valid initialization for stat and fd is live.
    let mut value: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut value) } != 0 {
        Err(denied("file metadata is unavailable"))
    } else {
        Ok(value)
    }
}

fn is_kind(value: &libc::stat, kind: libc::mode_t) -> bool {
    value.st_mode & libc::S_IFMT == kind
}

/// Opens a regular file and restores blocking reads on it. Special files are
/// refused on the non-blocking descriptor before any read.
fn open_regular(root: &Path, relative: &str) -> Result<(File, libc::stat), FsError> {
    let fd = open_relative(root, relative)?;
    let metadata = stat(&fd)?;
    if !is_kind(&metadata, libc::S_IFREG) {
        return Err(denied("only regular files may be read"));
    }
    // SAFETY: fd is live; F_GETFL/F_SETFL only change this descriptor's flags.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) } != 0
    {
        return Err(denied("file could not be prepared for reading"));
    }
    Ok((File::from(fd), metadata))
}

fn version(value: &libc::stat) -> FileVersion {
    FileVersion {
        device: value.st_dev as u64,
        inode: value.st_ino,
        size: value.st_size.max(0) as u64,
        modified_seconds: value.st_mtime,
        modified_nanoseconds: value.st_mtime_nsec,
    }
}

fn has_binary_marker(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|byte| *byte == 0)
}

pub(crate) fn read(
    root: &Path,
    relative: &str,
    offset: u64,
    max_bytes: usize,
) -> Result<(String, u64, bool, FileVersion), FsError> {
    if !(4..=MAX_READ_BYTES).contains(&max_bytes) {
        return Err(invalid("read byte limit is invalid"));
    }
    let (mut file, metadata) = open_regular(root, relative)?;
    let file_version = version(&metadata);
    if file_version.size > MAX_FILE_BYTES {
        return Err(fail(FsErrorKind::Limit, "file exceeds the read limit"));
    }
    if offset > file_version.size {
        return Err(invalid("read cursor is invalid"));
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| fail(FsErrorKind::Changed, "file seek failed"))?;
    let mut bytes = vec![0; max_bytes.min((file_version.size - offset) as usize)];
    file.read_exact(&mut bytes)
        .map_err(|_| fail(FsErrorKind::Changed, "file changed during read"))?;
    if has_binary_marker(&bytes) {
        return Err(denied("binary files are not returned"));
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            String::from_utf8(error.into_bytes()[..valid].to_vec())
                .map_err(|_| denied("file is not valid UTF-8"))?
        }
        Err(_) => return Err(denied("file is not valid UTF-8")),
    };
    let next = offset + text.len() as u64;
    Ok((text, next, next < file_version.size, file_version))
}

/// Whole-file text for search. `Ok(None)` is a binary file, which search
/// excludes by policy rather than counting as skipped.
fn read_text(root: &Path, relative: &str) -> Result<Option<String>, FsError> {
    let (file, metadata) = open_regular(root, relative)?;
    let size = version(&metadata).size;
    if size > MAX_FILE_BYTES {
        return Err(fail(FsErrorKind::Limit, "file exceeds the read limit"));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| fail(FsErrorKind::Changed, "file changed during read"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(fail(FsErrorKind::Limit, "file exceeds the read limit"));
    }
    if has_binary_marker(&bytes) {
        return Ok(None);
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| denied("file is not valid UTF-8"))
}

pub(crate) fn identity(root: &Path, relative: &str) -> Result<FileVersion, FsError> {
    let (_, metadata) = open_regular(root, relative)?;
    let file_version = version(&metadata);
    if file_version.size > MAX_FILE_BYTES {
        return Err(fail(FsErrorKind::Limit, "file exceeds the read limit"));
    }
    Ok(file_version)
}

/// Reads up to `limit` allowed entries (sorted by name, so truncation is
/// deterministic) and reports whether more allowed entries existed.
fn read_directory(
    root: &Path,
    relative: &str,
    limit: usize,
) -> Result<(Vec<Entry>, FileVersion, bool), FsError> {
    let fd = open_relative(root, relative)?;
    let metadata = stat(&fd)?;
    if !is_kind(&metadata, libc::S_IFDIR) {
        return Err(denied("only directories may be listed"));
    }
    let directory_version = version(&metadata);
    let parent = components(relative)?
        .last()
        .copied()
        .or_else(|| root.file_name());
    // fdopendir owns the descriptor, so duplicate it first.
    let duplicate = unsafe { libc::dup(fd.as_raw_fd()) };
    if duplicate < 0 {
        return Err(denied("directory could not be read"));
    }
    let directory = unsafe { libc::fdopendir(duplicate) };
    if directory.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(denied("directory could not be read"));
    }
    let mut names = Vec::new();
    loop {
        let item = unsafe { libc::readdir(directory) };
        if item.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." || !allowed_child(parent, OsStr::from_bytes(name)) {
            continue;
        }
        names.push(name.to_vec());
    }
    unsafe { libc::closedir(directory) };
    names.sort();
    let truncated = names.len() > limit;
    names.truncate(limit);
    let mut entries = Vec::with_capacity(names.len());
    for name in names {
        let name_os = OsStr::from_bytes(&name);
        let value = cstring(name_os)?;
        // SAFETY: fd is live and value is NUL terminated.
        let child = unsafe {
            libc::openat(
                fd.as_raw_fd(),
                value.as_ptr(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            )
        };
        if child < 0 {
            continue;
        }
        // SAFETY: openat returned a new descriptor.
        let child = unsafe { OwnedFd::from_raw_fd(child) };
        let child_stat = stat(&child)?;
        let kind = match child_stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => "file",
            libc::S_IFDIR => "directory",
            _ => "excluded-special",
        };
        entries.push(Entry {
            name: String::from_utf8_lossy(&name).into_owned(),
            kind,
            size: (kind == "file").then_some(child_stat.st_size.max(0) as u64),
        });
    }
    Ok((entries, directory_version, truncated))
}

pub(crate) fn list(root: &Path, relative: &str) -> Result<Listing, FsError> {
    let (entries, version, truncated) = read_directory(root, relative, MAX_LIST_ENTRIES)?;
    Ok(Listing {
        entries,
        version,
        truncated,
    })
}

fn search_text(path: &str, text: &str, needle: &str, results: &mut Vec<Match>) -> bool {
    for (line_index, line) in text.lines().enumerate() {
        if let Some(column) = line.find(needle) {
            results.push(Match {
                path: path.to_owned(),
                line: line_index + 1,
                column: column + 1,
                preview: line.chars().take(300).collect(),
            });
            if results.len() >= MAX_SEARCH_RESULTS {
                return false;
            }
        }
    }
    true
}

pub(crate) fn search_controlled(
    root: &Path,
    relative: &str,
    needle: &str,
    mut control: impl FnMut() -> SearchControl,
) -> Result<SearchOutcome, FsError> {
    if needle.is_empty() || needle.len() > 256 || needle.contains('\0') {
        return Err(invalid("search query is invalid"));
    }
    let partial = |matches, stop_reason, skipped| SearchOutcome {
        matches,
        complete: false,
        stop_reason: Some(stop_reason),
        skipped,
    };
    let mut results = Vec::new();
    let mut skipped = 0usize;
    // A regular-file target searches exactly that file.
    let target = open_relative(root, relative)?;
    let target_stat = stat(&target)?;
    drop(target);
    if is_kind(&target_stat, libc::S_IFREG) {
        match read_text(root, relative) {
            Ok(Some(text)) => {
                if !search_text(relative, &text, needle, &mut results) {
                    return Ok(partial(results, "result-limit", 0));
                }
            }
            Ok(None) => {}
            Err(error) if error.kind == FsErrorKind::Denied => return Err(error),
            Err(_) => skipped += 1,
        }
    } else if !is_kind(&target_stat, libc::S_IFDIR) {
        return Err(denied("only regular files and directories may be searched"));
    } else {
        let mut pending = vec![(relative.to_owned(), 0usize)];
        let mut files = 0usize;
        while let Some((directory, depth)) = pending.pop() {
            match control() {
                SearchControl::Continue => {}
                SearchControl::Deadline => return Ok(partial(results, "deadline", skipped)),
                SearchControl::Cancelled => {
                    return Err(fail(FsErrorKind::Cancelled, "search authorization changed"))
                }
            }
            let Ok((entries, _, truncated)) =
                read_directory(root, &directory, MAX_SEARCH_DIRECTORY_ENTRIES)
            else {
                skipped += 1;
                continue;
            };
            if truncated {
                skipped += 1;
            }
            for entry in entries {
                match control() {
                    SearchControl::Continue => {}
                    SearchControl::Deadline => return Ok(partial(results, "deadline", skipped)),
                    SearchControl::Cancelled => {
                        return Err(fail(FsErrorKind::Cancelled, "search authorization changed"))
                    }
                }
                let path = if directory.is_empty() {
                    entry.name.clone()
                } else {
                    format!("{directory}/{}", entry.name)
                };
                if entry.kind == "directory" {
                    if depth < MAX_SEARCH_DEPTH {
                        pending.push((path, depth + 1));
                    } else {
                        skipped += 1;
                    }
                    continue;
                }
                if entry.kind != "file" {
                    continue;
                }
                files += 1;
                if files > MAX_SEARCH_FILES {
                    return Ok(partial(results, "file-limit", skipped));
                }
                match read_text(root, &path) {
                    Ok(Some(text)) => {
                        if !search_text(&path, &text, needle, &mut results) {
                            return Ok(partial(results, "result-limit", skipped));
                        }
                    }
                    Ok(None) => {}
                    Err(_) => skipped += 1,
                }
            }
        }
    }
    if skipped > 0 {
        return Ok(partial(results, "skipped-entries", skipped));
    }
    Ok(SearchOutcome {
        matches: results,
        complete: true,
        stop_reason: None,
        skipped: 0,
    })
}

#[cfg(test)]
pub(crate) fn search(root: &Path, relative: &str, needle: &str) -> Result<Vec<Match>, FsError> {
    search_controlled(root, relative, needle, || SearchControl::Continue)
        .map(|outcome| outcome.matches)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn descriptor_reads_reject_links_special_files_and_sensitive_names() {
        let root = std::env::temp_dir().join(format!("deck-mcp-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn bounded() {}\n").unwrap();
        std::fs::write(root.join(".env.example"), "EXAMPLE=yes\n").unwrap();
        std::fs::write(root.join(".env"), "SECRET=no\n").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", root.join("link")).unwrap();
        let _socket = UnixListener::bind(root.join("socket")).unwrap();
        assert_eq!(
            read(&root, "src/lib.rs", 0, 64).unwrap().0,
            "fn bounded() {}\n"
        );
        assert!(read(&root, "../outside", 0, 64).is_err());
        assert!(read(&root, "link", 0, 64).is_err());
        assert!(read(&root, "socket", 0, 64).is_err());
        assert!(read(&root, ".env", 0, 64).is_err());
        assert!(read(&root, ".env.example", 0, 64).is_ok());
        assert_eq!(search(&root, "", "bounded").unwrap().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn fixture(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "deck-mcp-fs-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Runs `work` on a helper thread and fails instead of hanging when a
    /// blocking open would otherwise wedge the test process.
    fn bounded<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(work());
        });
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("filesystem call blocked instead of returning")
    }

    #[test]
    fn search_reads_whole_files_beyond_one_read_page() {
        let root = fixture("large-file");
        let mut body = "filler line without the marker\n".repeat(700);
        body.push_str("DECK_SYNTHETIC_TAIL_MARKER\n");
        assert!(body.len() > 20 * 1024);
        std::fs::write(root.join("large.txt"), body).unwrap();
        let outcome = search_controlled(&root, "", "DECK_SYNTHETIC_TAIL_MARKER", || {
            SearchControl::Continue
        })
        .unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].line, 701);
        assert!(outcome.complete);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn large_directories_list_truncated_and_remain_searchable() {
        let root = fixture("wide-dir");
        for index in 0..40 {
            std::fs::write(
                root.join(format!("file{index:02}.txt")),
                format!("DECK_SYNTHETIC_WIDE_{index:02}\n"),
            )
            .unwrap();
        }
        let listing = list(&root, "").unwrap();
        assert_eq!(listing.entries.len(), MAX_LIST_ENTRIES);
        assert!(listing.truncated);
        let outcome = search_controlled(&root, "", "DECK_SYNTHETIC_WIDE_39", || {
            SearchControl::Continue
        })
        .unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert!(outcome.complete);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn search_target_may_be_one_regular_file() {
        let root = fixture("file-target");
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/one.rs"),
            "fn marker() {} // DECK_SYNTHETIC_ONE\n",
        )
        .unwrap();
        std::fs::write(root.join("src/two.rs"), "// DECK_SYNTHETIC_ONE elsewhere\n").unwrap();
        let outcome = search_controlled(&root, "src/one.rs", "DECK_SYNTHETIC_ONE", || {
            SearchControl::Continue
        })
        .unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].path, "src/one.rs");
        assert!(outcome.complete);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_files_make_search_incomplete() {
        let root = fixture("skipped");
        std::fs::write(root.join("text.txt"), "DECK_SYNTHETIC_SKIP\n").unwrap();
        // Binary files are excluded by policy and do not make a search partial.
        std::fs::write(root.join("binary.bin"), b"DECK_SYNTHETIC_SKIP\0\0binary").unwrap();
        let outcome =
            search_controlled(&root, "", "DECK_SYNTHETIC_SKIP", || SearchControl::Continue)
                .unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert!(outcome.complete);
        // Undecodable text is a file the search could not cover.
        std::fs::write(root.join("latin1.txt"), b"DECK_SYNTHETIC_SKIP \xff\xfe\n").unwrap();
        let outcome =
            search_controlled(&root, "", "DECK_SYNTHETIC_SKIP", || SearchControl::Continue)
                .unwrap();
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.skipped, 1);
        assert!(!outcome.complete);
        assert_eq!(outcome.stop_reason, Some("skipped-entries"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn errors_distinguish_invalid_denied_and_limit() {
        let root = fixture("error-kinds");
        std::fs::write(root.join(".env"), "DECK_SYNTHETIC_SECRET=1\n").unwrap();
        std::fs::write(root.join("ok.txt"), "DECK_SYNTHETIC_OK\n").unwrap();
        assert_eq!(
            search_controlled(&root, "", "", || SearchControl::Continue)
                .unwrap_err()
                .kind,
            FsErrorKind::Invalid
        );
        assert_eq!(
            read(&root, "../x", 0, 64).unwrap_err().kind,
            FsErrorKind::Invalid
        );
        assert_eq!(
            read(&root, ".env", 0, 64).unwrap_err().kind,
            FsErrorKind::Denied
        );
        assert_eq!(
            read(&root, "missing.txt", 0, 64).unwrap_err().kind,
            FsErrorKind::Denied
        );
        assert_eq!(
            read(&root, "ok.txt", 0, 2).unwrap_err().kind,
            FsErrorKind::Invalid
        );
        assert_eq!(
            search_controlled(&root, "", "x", || SearchControl::Cancelled)
                .unwrap_err()
                .kind,
            FsErrorKind::Cancelled
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sensitive_names_are_excluded_from_every_entry_point() {
        let root = fixture("sensitive");
        let files = [
            ".netrc",
            ".npmrc",
            ".pypirc",
            ".pgpass",
            ".vault-token",
            ".git-credentials",
            ".zsh_history",
            ".bash_history",
            ".credentials.json",
            "application_default_credentials.json",
            "prod.tfstate",
            "prod.tfstate.backup",
            "id_ed25519_work",
            "id_rsa_legacy",
            ".ENV",
            ".Env.local",
            ".\u{17f}sh-like",
            "id_r\u{17f}a",
        ];
        for name in files {
            std::fs::write(root.join(name), "DECK_SYNTHETIC_SENSITIVE\n").unwrap();
        }
        let nested = [
            (".docker", "config.json"),
            (".config/gh", "hosts.yml"),
            (".codex", "auth.json"),
            (".claude", ".credentials.json"),
            (".SSH", "config"),
        ];
        for (directory, file) in nested {
            std::fs::create_dir_all(root.join(directory)).unwrap();
            std::fs::write(
                root.join(directory).join(file),
                "DECK_SYNTHETIC_SENSITIVE\n",
            )
            .unwrap();
        }
        std::fs::write(root.join("id_ed25519_work.pub"), "DECK_SYNTHETIC_PUBLIC\n").unwrap();
        std::fs::write(root.join("id_generator.rs"), "DECK_SYNTHETIC_ORDINARY\n").unwrap();
        std::fs::create_dir_all(root.join(".claude/commands")).unwrap();
        std::fs::write(
            root.join(".claude/commands/review.md"),
            "DECK_SYNTHETIC_PROJECT_COMMAND\n",
        )
        .unwrap();
        assert!(read(&root, "id_generator.rs", 0, 64).is_ok());
        assert!(read(&root, ".claude/commands/review.md", 0, 64).is_ok());
        std::fs::write(root.join(".env.example"), "DECK_SYNTHETIC_EXAMPLE\n").unwrap();
        let blocked = files
            .iter()
            .map(|name| name.to_string())
            .chain(
                nested
                    .iter()
                    .map(|(directory, file)| format!("{directory}/{file}")),
            )
            .collect::<Vec<_>>();
        for path in &blocked {
            assert_eq!(
                read(&root, path, 0, 64).unwrap_err().kind,
                FsErrorKind::Denied,
                "read {path}"
            );
            assert_eq!(
                identity(&root, path).unwrap_err().kind,
                FsErrorKind::Denied,
                "identity {path}"
            );
        }
        let names = list(&root, "")
            .unwrap()
            .entries
            .into_iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"id_ed25519_work.pub".to_string()));
        assert!(names.contains(&".env.example".to_string()));
        assert!(names.contains(&".config".to_string()));
        for name in files.iter().chain(&[".docker", ".SSH"]) {
            assert!(!names.contains(&name.to_string()), "list exposed {name}");
        }
        // Project-local agent configuration stays readable; only the parent-
        // specific credential names below it are excluded.
        for (directory, hidden) in [(".config", "gh"), (".codex", "auth.json")] {
            assert!(names.contains(&directory.to_string()));
            assert!(list(&root, directory)
                .unwrap()
                .entries
                .iter()
                .all(|entry| entry.name != hidden));
        }
        let outcome = search_controlled(&root, "", "DECK_SYNTHETIC_SENSITIVE", || {
            SearchControl::Continue
        })
        .unwrap();
        assert!(outcome.matches.is_empty(), "{:?}", outcome.matches);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn special_files_return_promptly_from_every_entry_point() {
        let root = fixture("fifo");
        let fifo = CString::new(root.join("pipe").as_os_str().as_bytes()).unwrap();
        // SAFETY: fifo is NUL terminated; mkfifo only creates a node.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let _socket = UnixListener::bind(root.join("socket")).unwrap();
        let shared = root.clone();
        let read_error = bounded(move || read(&shared, "pipe", 0, 64).unwrap_err().kind);
        assert_eq!(read_error, FsErrorKind::Denied);
        let shared = root.clone();
        let identity_error = bounded(move || identity(&shared, "pipe").unwrap_err().kind);
        assert_eq!(identity_error, FsErrorKind::Denied);
        let shared = root.clone();
        let outcome = bounded(move || {
            search_controlled(&shared, "", "x", || SearchControl::Continue).unwrap()
        });
        assert!(outcome.matches.is_empty());
        let shared = root.clone();
        let search_target = bounded(move || {
            search_controlled(&shared, "pipe", "x", || SearchControl::Continue)
                .unwrap_err()
                .kind
        });
        assert_eq!(search_target, FsErrorKind::Denied);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broad_or_sensitive_roots_are_rejected() {
        let home = home_directory().expect("account home");
        assert!(root_policy(Path::new("/")).is_err());
        assert!(root_policy(&home).is_err());
        if let Some(parent) = home.parent() {
            assert!(root_policy(parent).is_err());
        }
        assert!(root_policy(&home.join(".ssh")).is_err());
        assert!(root_policy(&home.join(".ssh/keys")).is_err());
        assert!(root_policy(&home.join("work/.git")).is_err());
        assert!(root_policy(&home.join("work/project")).is_ok());
        let root = fixture("root-policy");
        assert!(root_policy(&std::fs::canonicalize(&root).unwrap()).is_ok());
        assert_eq!(
            read(Path::new("/"), "etc/hosts", 0, 64).unwrap_err().kind,
            FsErrorKind::Denied
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn controlled_search_reports_deadline_and_cancellation() {
        let root =
            std::env::temp_dir().join(format!("deck-mcp-fs-deadline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("one.rs"), "needle\n").unwrap();

        let partial = search_controlled(&root, "", "needle", || SearchControl::Deadline).unwrap();
        assert!(!partial.complete);
        assert_eq!(partial.stop_reason, Some("deadline"));
        assert!(partial.matches.is_empty());
        assert_eq!(
            search_controlled(&root, "", "needle", || SearchControl::Cancelled)
                .unwrap_err()
                .message,
            "search authorization changed"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
