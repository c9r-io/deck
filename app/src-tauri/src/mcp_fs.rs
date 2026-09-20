//! Descriptor-relative, no-shell filesystem reads for Deck MCP.
//!
//! Every component is opened below an already opened root with `openat` and
//! `O_NOFOLLOW`. Callers never validate one pathname and then reopen another.
//! Symlinks and non-regular files are rejected. This is an interface boundary,
//! not a sandbox against other code running as the same uid.

use serde::Serialize;
use std::ffi::{CStr, CString, OsStr};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path};

pub(crate) const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
pub(crate) const MAX_READ_BYTES: usize = 64 * 1024;
pub(crate) const MAX_LIST_ENTRIES: usize = 256;
pub(crate) const MAX_SEARCH_FILES: usize = 1_000;
pub(crate) const MAX_SEARCH_RESULTS: usize = 200;
pub(crate) const MAX_SEARCH_DEPTH: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileVersion {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) kind: &'static str,
    pub(crate) size: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Match {
    pub(crate) path: String,
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) preview: String,
}

fn invalid(message: &'static str) -> String {
    message.into()
}

fn components(path: &str) -> Result<Vec<&OsStr>, String> {
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

fn allowed_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    if lower == ".env.example" || lower == ".env.sample" {
        return true;
    }
    !matches!(
        lower.as_str(),
        ".git" | ".ssh" | ".gnupg" | ".aws" | ".kube" | ".deck"
    ) && !lower.starts_with(".env")
        && !matches!(
            lower.as_str(),
            "id_rsa" | "id_dsa" | "id_ecdsa" | "id_ed25519" | "credentials" | "credentials.json"
        )
        && !lower.ends_with(".pem")
        && !lower.ends_with(".p12")
        && !lower.ends_with(".key")
}

fn cstring(value: &OsStr) -> Result<CString, String> {
    CString::new(value.as_bytes()).map_err(|_| invalid("path contains a NUL byte"))
}

fn open_root(root: &Path) -> Result<OwnedFd, String> {
    let value = cstring(root.as_os_str())?;
    // SAFETY: value is NUL terminated and the returned descriptor is owned.
    let fd = unsafe {
        libc::open(
            value.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        Err(invalid("authorized root is unavailable or was replaced"))
    } else {
        // SAFETY: open returned a new descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }
}

fn open_relative(root: &Path, relative: &str, directory: bool) -> Result<OwnedFd, String> {
    let mut current = open_root(root)?;
    let values = components(relative)?;
    for (index, value) in values.iter().enumerate() {
        if !allowed_name(value) {
            return Err(invalid("sensitive or private path is excluded"));
        }
        let final_component = index + 1 == values.len();
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        if !final_component || directory {
            flags |= libc::O_DIRECTORY;
        }
        let value = cstring(value)?;
        // SAFETY: current is live and value is NUL terminated.
        let fd = unsafe { libc::openat(current.as_raw_fd(), value.as_ptr(), flags) };
        if fd < 0 {
            return Err(invalid("path is unavailable, excluded, or a symbolic link"));
        }
        // SAFETY: openat returned a new descriptor.
        current = unsafe { OwnedFd::from_raw_fd(fd) };
    }
    Ok(current)
}

fn stat(fd: &OwnedFd) -> Result<libc::stat, String> {
    // SAFETY: zero is valid initialization for stat and fd is live.
    let mut value: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut value) } != 0 {
        Err(invalid("file metadata is unavailable"))
    } else {
        Ok(value)
    }
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

pub(crate) fn read(
    root: &Path,
    relative: &str,
    offset: u64,
    max_bytes: usize,
) -> Result<(String, u64, bool, FileVersion), String> {
    if !(4..=MAX_READ_BYTES).contains(&max_bytes) {
        return Err(invalid("read byte limit is invalid"));
    }
    let fd = open_relative(root, relative, false)?;
    let metadata = stat(&fd)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid("only regular files may be read"));
    }
    let file_version = version(&metadata);
    if file_version.size > MAX_FILE_BYTES || offset > file_version.size {
        return Err(invalid("file exceeds the read limit or cursor is invalid"));
    }
    let mut file = File::from(fd);
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| invalid("file seek failed"))?;
    let mut bytes = vec![0; max_bytes.min((file_version.size - offset) as usize)];
    file.read_exact(&mut bytes)
        .map_err(|_| invalid("file changed during read"))?;
    if bytes.iter().take(8192).any(|byte| *byte == 0) {
        return Err(invalid("binary files are not returned"));
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if error.utf8_error().error_len().is_none() => {
            let valid = error.utf8_error().valid_up_to();
            String::from_utf8(error.into_bytes()[..valid].to_vec())
                .map_err(|_| invalid("file is not valid UTF-8"))?
        }
        Err(_) => return Err(invalid("file is not valid UTF-8")),
    };
    let next = offset + text.len() as u64;
    Ok((text, next, next < file_version.size, file_version))
}

pub(crate) fn identity(root: &Path, relative: &str) -> Result<FileVersion, String> {
    let fd = open_relative(root, relative, false)?;
    let metadata = stat(&fd)?;
    if metadata.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid("only regular files may be read"));
    }
    let file_version = version(&metadata);
    if file_version.size > MAX_FILE_BYTES {
        return Err(invalid("file exceeds the read limit"));
    }
    Ok(file_version)
}

pub(crate) fn list(root: &Path, relative: &str) -> Result<(Vec<Entry>, FileVersion), String> {
    let fd = open_relative(root, relative, true)?;
    let metadata = stat(&fd)?;
    let directory_version = version(&metadata);
    // fdopendir owns the descriptor, so duplicate it first.
    let duplicate = unsafe { libc::dup(fd.as_raw_fd()) };
    if duplicate < 0 {
        return Err(invalid("directory could not be read"));
    }
    let directory = unsafe { libc::fdopendir(duplicate) };
    if directory.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(invalid("directory could not be read"));
    }
    let mut entries = Vec::new();
    loop {
        let item = unsafe { libc::readdir(directory) };
        if item.is_null() {
            break;
        }
        let name = unsafe { CStr::from_ptr((*item).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        let name_os = OsStr::from_bytes(name);
        if !allowed_name(name_os) {
            continue;
        }
        let name_string = String::from_utf8_lossy(name).into_owned();
        let value = cstring(name_os)?;
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
        let child = unsafe { OwnedFd::from_raw_fd(child) };
        let child_stat = stat(&child)?;
        let kind = match child_stat.st_mode & libc::S_IFMT {
            libc::S_IFREG => "file",
            libc::S_IFDIR => "directory",
            _ => "excluded-special",
        };
        entries.push(Entry {
            name: name_string,
            kind,
            size: (kind == "file").then_some(child_stat.st_size.max(0) as u64),
        });
        if entries.len() > MAX_LIST_ENTRIES {
            unsafe { libc::closedir(directory) };
            return Err(invalid("directory entry limit exceeded"));
        }
    }
    unsafe { libc::closedir(directory) };
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok((entries, directory_version))
}

pub(crate) fn search(root: &Path, relative: &str, needle: &str) -> Result<Vec<Match>, String> {
    if needle.is_empty() || needle.len() > 256 || needle.contains('\0') {
        return Err(invalid("search query is invalid"));
    }
    let mut pending = vec![(relative.to_owned(), 0usize)];
    let mut files = 0usize;
    let mut results = Vec::new();
    while let Some((directory, depth)) = pending.pop() {
        if depth > MAX_SEARCH_DEPTH {
            return Err(invalid("search depth limit exceeded"));
        }
        for entry in list(root, &directory)?.0 {
            let path = if directory.is_empty() {
                entry.name.clone()
            } else {
                format!("{directory}/{}", entry.name)
            };
            if entry.kind == "directory" {
                pending.push((path, depth + 1));
                continue;
            }
            if entry.kind != "file" || entry.size.is_some_and(|size| size > MAX_FILE_BYTES) {
                continue;
            }
            files += 1;
            if files > MAX_SEARCH_FILES {
                return Err(invalid("search file limit exceeded"));
            }
            let Ok((text, _, _, _)) =
                read(root, &path, 0, (entry.size.unwrap_or(0) as usize).max(4))
            else {
                continue;
            };
            for (line_index, line) in text.lines().enumerate() {
                if let Some(column) = line.find(needle) {
                    results.push(Match {
                        path: path.clone(),
                        line: line_index + 1,
                        column: column + 1,
                        preview: line.chars().take(300).collect(),
                    });
                    if results.len() >= MAX_SEARCH_RESULTS {
                        return Ok(results);
                    }
                }
            }
        }
    }
    Ok(results)
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
}
