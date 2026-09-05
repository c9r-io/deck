//! One deck per user: an exclusive advisory `flock` on `~/.deck/deck.lock`
//! held for the process lifetime. A second instance would double-fire
//! scheduled prompts and race every data file, so it logs and exits
//! (`main.rs`) — no dialog, because the only way to show one without a
//! dialog plugin is `osascript`, an EDR signature.

use crate::datadir::{create_private_dir, open_private};
use crate::error::{DeckError, ErrorKind};
use std::path::Path;

/// Hold an exclusive advisory lock for the app's lifetime. A second deck
/// instance would double-fire scheduled prompts and race every data file,
/// so it must not start.
pub fn acquire_instance_lock(dir: &Path) -> Result<(), DeckError> {
    use std::os::fd::AsRawFd;
    create_private_dir(dir)?;
    let f = open_private(&dir.join("deck.lock")).map_err(DeckError::from)?;
    // LOCK_EX | LOCK_NB
    let rc = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(DeckError::new(
            ErrorKind::Locked,
            "another deck instance is already running",
        ));
    }
    std::mem::forget(f); // keep the fd (and the lock) until the process exits
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tdir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("deck-instance-lock-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn second_instance_lock_is_refused() {
        let d = tdir("lock");
        acquire_instance_lock(&d).unwrap();
        assert_eq!(mode_of(&d.join("deck.lock")), 0o600, "lock file private");
        // same-process flock on a fresh fd of the same file: macOS grants it
        // (locks are per-open-file but merge per process), so exercise the
        // failure path from a child process instead.
        let out = std::process::Command::new("python3")
            .arg("-c")
            .arg(format!(
                "import fcntl,sys;f=open('{}','w')\ntry:\n fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB);print('got')\nexcept OSError:\n print('refused')",
                d.join("deck.lock").display()
            ))
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("refused"),
            "child process must not obtain the lock"
        );
    }
}
