//! The Runtime-key transport file and in-memory key hygiene.
//!
//! The key is written to `$TMPDIR/deck-tunnelctl-<pid>-<random>/runtime-key`
//! (create-new, no-follow, 0600 inside a 0700 directory owned by the current
//! user) and removed on every exit this process can observe:
//!
//! - `Drop` on the normal and error paths;
//! - SIGINT, SIGTERM and SIGHUP: an async-signal-safe handler unlinks the
//!   armed file and directory, then re-raises the signal with its default
//!   action;
//! - SIGKILL (e.g. Deck's hard timeout) cannot be caught, so each command
//!   first sweeps `deck-tunnelctl-*` directories whose owning pid is gone,
//!   accepting only real 0700 directories owned by the current user and
//!   removing only a regular `runtime-key` file inside them.
//!
//! The helper's own command budget (`cli.rs`) stays below Deck's kill timeout,
//! so SIGKILL is the exceptional path. `KeyBytes` zeroes key buffers on drop.

use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::Once;

const DIRECTORY_PREFIX: &str = "deck-tunnelctl-";
const FILE_NAME: &str = "runtime-key";

/// Key bytes that are zeroed when dropped.
pub struct KeyBytes(pub Vec<u8>);

impl Drop for KeyBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

static ARMED_FILE: AtomicPtr<libc::c_char> = AtomicPtr::new(std::ptr::null_mut());
static ARMED_DIRECTORY: AtomicPtr<libc::c_char> = AtomicPtr::new(std::ptr::null_mut());
static HANDLERS: Once = Once::new();

extern "C" fn remove_on_signal(signal: libc::c_int) {
    // Only async-signal-safe calls: unlink(2), rmdir(2), signal(3), raise(3).
    let file = ARMED_FILE.swap(std::ptr::null_mut(), Ordering::SeqCst);
    let directory = ARMED_DIRECTORY.swap(std::ptr::null_mut(), Ordering::SeqCst);
    unsafe {
        if !file.is_null() {
            libc::unlink(file);
        }
        if !directory.is_null() {
            libc::rmdir(directory);
        }
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

fn install_handlers() {
    HANDLERS.call_once(|| {
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            let handler = remove_on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            unsafe { libc::signal(signal, handler) };
        }
    });
}

fn arm(slot: &AtomicPtr<libc::c_char>, path: &Path) -> *mut libc::c_char {
    let raw = CString::new(path.as_os_str().as_bytes())
        .map(CString::into_raw)
        .unwrap_or(std::ptr::null_mut());
    slot.store(raw, Ordering::SeqCst);
    raw
}

fn disarm(slot: &AtomicPtr<libc::c_char>, raw: *mut libc::c_char) {
    if raw.is_null() {
        return;
    }
    // Free only if the handler did not take it; otherwise the process is
    // already terminating (and a concurrent test file may have replaced it).
    if slot
        .compare_exchange(
            raw,
            std::ptr::null_mut(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        drop(unsafe { CString::from_raw(raw) });
    }
}

pub struct SecretFile {
    path: PathBuf,
    directory: PathBuf,
    armed_file: *mut libc::c_char,
    armed_directory: *mut libc::c_char,
}

impl SecretFile {
    pub fn create(secret: &[u8]) -> Result<Self, &'static str> {
        install_handlers();
        let base = std::env::temp_dir();
        let nonce = format!("{}-{}", std::process::id(), random_suffix()?);
        let directory = base.join(format!("{DIRECTORY_PREFIX}{nonce}"));
        let path = directory.join(FILE_NAME);
        // Armed before anything exists: an early signal unlinks nothing.
        // Every error return below drops `secret_file`, whose Drop removes
        // whatever was created and disarms the handler.
        let secret_file = Self {
            armed_directory: arm(&ARMED_DIRECTORY, &directory),
            armed_file: arm(&ARMED_FILE, &path),
            path,
            directory,
        };
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&secret_file.directory)
            .map_err(|_| "secret_create_failed")?;
        if !private_directory(&secret_file.directory) {
            return Err("secret_validate_failed");
        }
        let mut file = open_new(&secret_file.path)?;
        file.write_all(secret).map_err(|_| "secret_write_failed")?;
        file.sync_all().map_err(|_| "secret_write_failed")?;
        let metadata = file.metadata().map_err(|_| "secret_validate_failed")?;
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o777 != 0o600
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err("secret_validate_failed");
        }
        Ok(secret_file)
    }

    pub fn reference(&self) -> String {
        format!("file:{}", self.path.to_string_lossy())
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn private_directory(directory: &Path) -> bool {
    fs::symlink_metadata(directory).is_ok_and(|metadata| {
        metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.permissions().mode() & 0o777 == 0o700
            && metadata.uid() == unsafe { libc::geteuid() }
    })
}

/// Removes only a regular file (never through a symlink) and then the
/// directory if it is empty.
fn remove_secret(directory: &Path, path: &Path) {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_file() && !metadata.file_type().is_symlink() {
            let _ = fs::remove_file(path);
        }
    }
    let _ = fs::remove_dir(directory);
}

/// Removes key files left by a helper that was killed uncatchably. A
/// directory whose pid is still alive belongs to a concurrent helper and is
/// left alone.
pub fn sweep_stale() {
    sweep_stale_in(&std::env::temp_dir());
}

fn sweep_stale_in(base: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|name| name.strip_prefix(DIRECTORY_PREFIX))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        let directory = entry.path();
        if pid <= 0 || process_alive(pid) || !private_directory(&directory) {
            continue;
        }
        remove_secret(&directory, &directory.join(FILE_NAME));
    }
}

fn process_alive(pid: libc::pid_t) -> bool {
    let signalled = unsafe { libc::kill(pid, 0) };
    signalled == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn open_new(path: &Path) -> Result<File, &'static str> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "secret_create_failed")
}

fn random_suffix() -> Result<String, &'static str> {
    let mut bytes = [0u8; 16];
    let result = unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) };
    if result != 0 {
        return Err("random_unavailable");
    }
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

impl Drop for SecretFile {
    fn drop(&mut self) {
        remove_secret(&self.directory, &self.path);
        disarm(&ARMED_FILE, self.armed_file);
        disarm(&ARMED_DIRECTORY, self.armed_directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    #[test]
    fn secret_is_private_and_removed_on_drop() {
        let path;
        let directory;
        {
            let secret = SecretFile::create(b"do-not-log-this").unwrap();
            path = secret.path.clone();
            directory = secret.directory.clone();
            let metadata = fs::symlink_metadata(&path).unwrap();
            assert!(metadata.is_file());
            assert_eq!(metadata.mode() & 0o777, 0o600);
            assert_eq!(
                fs::symlink_metadata(&directory).unwrap().mode() & 0o777,
                0o700
            );
            assert_eq!(secret.reference(), format!("file:{}", path.display()));
        }
        assert!(!path.exists());
        assert!(!directory.exists());
    }

    #[test]
    fn stale_directories_of_dead_helpers_are_swept_and_live_ones_kept() {
        let base = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let dead = child.id();
        child.wait().unwrap();
        let make = |name: String, mode: u32| {
            let directory = base.path().join(name);
            fs::DirBuilder::new().mode(mode).create(&directory).unwrap();
            fs::write(directory.join(FILE_NAME), b"stale").unwrap();
            directory
        };
        let stale = make(format!("{DIRECTORY_PREFIX}{dead}-a"), 0o700);
        let live = make(format!("{DIRECTORY_PREFIX}{}-b", std::process::id()), 0o700);
        let loose = make(format!("{DIRECTORY_PREFIX}{dead}-c"), 0o755);
        let target = make("elsewhere".into(), 0o700);
        let link = base.path().join(format!("{DIRECTORY_PREFIX}{dead}-d"));
        std::os::unix::fs::symlink(&target, &link).unwrap();

        sweep_stale_in(base.path());
        assert!(!stale.exists());
        assert!(live.join(FILE_NAME).exists());
        assert!(loose.join(FILE_NAME).exists());
        assert!(target.join(FILE_NAME).exists());
        assert!(fs::symlink_metadata(&link).is_ok());
    }

    #[test]
    fn signal_handler_removes_the_armed_secret() {
        let secret = SecretFile::create(b"do-not-log-this").unwrap();
        let (path, directory) = (secret.path.clone(), secret.directory.clone());
        // Run only the cleanup half of the handler: the armed pointers are
        // this file's unless a parallel test re-armed them.
        let file = ARMED_FILE.swap(std::ptr::null_mut(), Ordering::SeqCst);
        let dir = ARMED_DIRECTORY.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if file == secret.armed_file && dir == secret.armed_directory {
            unsafe {
                libc::unlink(file);
                libc::rmdir(dir);
            }
            assert!(!path.exists());
            assert!(!directory.exists());
        }
        drop(secret);
        assert!(!path.exists());
    }
}
