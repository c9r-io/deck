//! The one place a visible pane's tmux client is started: the bundled tmux
//! on a pty slave, as the leader of its own session with that slave as its
//! controlling terminal.
//!
//! # Contract
//! - What starts is exactly what portable-pty started before (2026-11-27):
//!   `tmux_program()` with `tmux_clients::pane_client_args`, the process
//!   environment plus `TERM`/`LANG`, the home directory as its cwd, the
//!   slave as stdin/stdout/stderr, SIGCHLD, SIGHUP, SIGINT, SIGQUIT, SIGTERM
//!   and SIGALRM at their default. No new process kind, no shell, no argv
//!   change (`tests/edr_quiet.rs` pins this file's one `Command::new`, and
//!   its only caller `pty.rs` `attach_now` with `tmux_program()`).
//! - Why not portable-pty's `spawn_command` any more: its child closes every
//!   inherited descriptor above 2 (`close_random_fds`, which also reads
//!   `/dev/fd` — allocating code in the child of a multithreaded process),
//!   std's exec-status pipe included, so its `spawn` returned BEFORE exec:
//!   `client started` did not mean tmux was running, and an exec failure was
//!   never reported (2026-11-27, gen=261 never drew). Here the child only
//!   MARKS those descriptors close-on-exec (`fcntl`, async-signal-safe, no
//!   allocation), so the status pipe lives until exec: `spawn` returns once
//!   tmux's image runs, or with exec's error.
//! - Why not the POSIX spawn call (tried 2026-11-27): macOS gives a controlling
//!   terminal only through `TIOCSCTTY`, never by opening the slave, and
//!   that call has no file action for it; a tmux client started that way has
//!   no controlling tty and never receives SIGWINCH, so a resized pane kept
//!   its old size (WK `run`/`ux-layers` on air2; `a_real_tmux_client_follows_a_resize`).
//! - It still forks Deck. Process creation that the kernel holds holds this
//!   worker, never the window: it runs on the attach worker (`pty.rs`).
//! - `PaneClient` ends the client as portable-pty did: SIGHUP, up to 200 ms
//!   for it to go, then SIGKILL; `wait` reaps it. The master is never handed
//!   to a writer (`pty.rs` `master_input`).
use crate::error::{DeckError, ErrorKind};
use std::ffi::CString;
use std::os::unix::process::CommandExt;
use std::sync::Mutex;

/// `<sys/ttycom.h>` `TIOCPTYGNAME`: the slave's device name of a master.
const TIOCPTYGNAME: libc::c_ulong = 0x4080_7453;

/// The slave device of `master` (`/dev/ttysNNN`).
pub(crate) fn slave_path(master: std::os::fd::RawFd) -> Result<CString, DeckError> {
    let mut name = [0u8; 128];
    // SAFETY: TIOCPTYGNAME writes at most 128 bytes into `name`.
    if unsafe { libc::ioctl(master, TIOCPTYGNAME, name.as_mut_ptr()) } != 0 {
        return Err(DeckError::new(ErrorKind::Other, "pty unavailable"));
    }
    let end = name.iter().position(|b| *b == 0).unwrap_or(0);
    if end == 0 || !name.starts_with(b"/dev/") {
        return Err(DeckError::new(ErrorKind::Other, "pty unavailable"));
    }
    CString::new(&name[..end]).map_err(|_| DeckError::new(ErrorKind::Other, "pty unavailable"))
}

/// Start `program` with `args` on the slave `tty` (see the module contract).
/// `env` entries override the inherited environment.
pub(crate) fn spawn(
    program: &std::path::Path,
    args: &[&str],
    env: &[(&str, &str)],
    tty: &CString,
) -> Result<PaneClient, DeckError> {
    let open = || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(std::ffi::OsStr::new(
                std::str::from_utf8(tty.as_bytes()).unwrap_or_default(),
            ))
            .map_err(|_| DeckError::new(ErrorKind::Other, "pty unavailable"))
    };
    let home = std::env::var_os("HOME").unwrap_or_else(|| "/".into());
    // descriptors above this cannot exist (the soft limit), computed before
    // the fork so the child only calls fcntl
    // SAFETY: a plain query.
    let limit = unsafe { libc::getdtablesize() }.max(3);
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .envs(env.iter().copied())
        .current_dir(&home)
        .stdin(open()?)
        .stdout(open()?)
        .stderr(open()?);
    // SAFETY: the closure runs in the forked child before exec and calls
    // only async-signal-safe functions (signal, setsid, ioctl, fcntl); it
    // allocates nothing and takes no lock.
    unsafe {
        command.pre_exec(move || {
            for signal in [
                libc::SIGCHLD,
                libc::SIGHUP,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGALRM,
            ] {
                libc::signal(signal, libc::SIG_DFL);
            }
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // the slave is fd 0 now: make it the controlling terminal
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            for fd in 3..limit {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
                }
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .map_err(|e| DeckError::classified(format!("pane client spawn failed: {e}")))?;
    Ok(PaneClient {
        pid: child.id() as libc::pid_t,
        status: Mutex::new(None),
    })
}

/// A started pane client: its pid until reaped, then its exit status.
#[derive(Debug)]
pub(crate) struct PaneClient {
    pid: libc::pid_t,
    status: Mutex<Option<portable_pty::ExitStatus>>,
}

impl PaneClient {
    fn reap(&self, flags: libc::c_int) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        use crate::sync::LockRecover;
        let mut status = self.status.lock_or_recover();
        if let Some(done) = status.clone() {
            return Ok(Some(done));
        }
        loop {
            let mut raw: libc::c_int = 0;
            // SAFETY: waitpid on this client's own pid.
            let got = unsafe { libc::waitpid(self.pid, &mut raw, flags) };
            if got == self.pid {
                let code = if libc::WIFEXITED(raw) {
                    libc::WEXITSTATUS(raw) as u32
                } else {
                    1
                };
                let done = portable_pty::ExitStatus::with_exit_code(code);
                *status = Some(done.clone());
                return Ok(Some(done));
            }
            if got == 0 {
                return Ok(None);
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

impl portable_pty::ChildKiller for PaneClient {
    fn kill(&mut self) -> std::io::Result<()> {
        if self.reap(libc::WNOHANG)?.is_some() {
            return Ok(());
        }
        // SAFETY: signals to this client's own, not yet reaped, pid.
        unsafe { libc::kill(self.pid, libc::SIGHUP) };
        for _ in 0..4 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            if self.reap(libc::WNOHANG)?.is_some() {
                return Ok(());
            }
        }
        // SAFETY: as above; the pid is still unreaped, so it is still ours.
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        // never used by Deck: one owner kills and reaps
        Box::new(NoKiller)
    }
}

#[derive(Debug)]
struct NoKiller;
impl portable_pty::ChildKiller for NoKiller {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(NoKiller)
    }
}

impl portable_pty::Child for PaneClient {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        self.reap(libc::WNOHANG)
    }
    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        loop {
            if let Some(done) = self.reap(0)? {
                return Ok(done);
            }
        }
    }
    fn process_id(&self) -> Option<u32> {
        u32::try_from(self.pid).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portable_pty::{Child, ChildKiller};
    use std::io::Read;
    use std::os::fd::AsRawFd;

    fn read_until(master: &mut std::fs::File, marker: &str) -> String {
        let fd = master.as_raw_fd();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut out = Vec::new();
        while std::time::Instant::now() < until {
            let mut p = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd on the test's own master.
            if unsafe { libc::poll(&mut p, 1, 100) } > 0 {
                let mut buf = [0u8; 1024];
                match master.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => out.extend_from_slice(&buf[..n]),
                }
            }
            if String::from_utf8_lossy(&out).contains(marker) {
                break;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }

    /// The client gets the slave as its controlling terminal, is its
    /// foreground group, receives SIGWINCH on a resize of the master, sees
    /// the overridden environment, and is killed and reaped. The program
    /// here is `/bin/sh` printing fixed words (a test harness, not a pane).
    #[test]
    fn the_client_owns_its_tty_receives_resizes_and_is_reaped() {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .expect("pty");
        let fd = pair.master.as_raw_fd().expect("master fd");
        let tty = slave_path(fd).expect("slave path");
        let script = "if exec 3<>/dev/tty; then echo CTTY-$TERM_PROBE; fi; \
                      trap 'echo WINCH-$(stty size)' WINCH; echo READY; \
                      i=0; while [ $i -lt 50 ]; do sleep 0.1; i=$((i+1)); done";
        let mut client = spawn(
            std::path::Path::new("/bin/sh"),
            &["-c", script],
            &[("TERM_PROBE", "yes")],
            &tty,
        )
        .expect("spawned");
        // SAFETY: a dup of the test's own open master, owned by this File.
        let mut master =
            unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(libc::dup(fd)) };
        let early = read_until(&mut master, "READY");
        assert!(early.contains("CTTY-yes"), "{early}");
        pair.master
            .resize(portable_pty::PtySize {
                rows: 40,
                cols: 132,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let later = read_until(&mut master, "WINCH-40 132");
        assert!(later.contains("WINCH-40 132"), "{later}");
        assert!(client.process_id().is_some());
        client.kill().expect("killed");
        client.wait().expect("reaped");
        assert!(client.try_wait().unwrap().is_some(), "remembered");
        drop(pair.slave);
    }

    /// The bundled tmux as a real pane client on a throwaway server: after
    /// a resize of the master, the pane takes the new size (SIGWINCH reached
    /// the client through its controlling tty).
    #[test]
    fn a_real_tmux_client_follows_a_resize() {
        let tmux = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("binaries/tmux-aarch64-apple-darwin");
        let dir = crate::test_dir::TestDir::new("pane-client-tmux");
        let sock = dir.join("s");
        let sock = sock.to_str().unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new(&tmux)
                .args(["-f", "/dev/null", "-S", sock])
                .args(args)
                .output()
                .unwrap()
        };
        assert!(run(&["new-session", "-d", "-s", "t"]).status.success());
        assert!(run(&["set", "-g", "status", "off"]).status.success());
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let tty = slave_path(pair.master.as_raw_fd().unwrap()).unwrap();
        let mut client = spawn(
            &tmux,
            &[
                "-u",
                "-f",
                "/dev/null",
                "-S",
                sock,
                "attach-session",
                "-t",
                "=t",
            ],
            &[("TERM", "xterm-256color")],
            &tty,
        )
        .unwrap();
        let size = || {
            let out = run(&[
                "list-windows",
                "-t",
                "=t",
                "-F",
                "#{window_width}x#{window_height}",
            ]);
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout).trim(),
                String::from_utf8_lossy(&out.stderr).trim()
            )
        };
        let until = |want: &str| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while std::time::Instant::now() < end && size() != want {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            size()
        };
        assert_eq!(until("80x24"), "80x24");
        pair.master
            .resize(portable_pty::PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let after = until("100x30");
        client.kill().unwrap();
        // nothing read the tty's output: a session leader exits only once it
        // drained, so close the master as the reaper does
        drop(pair);
        client.wait().unwrap();
        let _ = run(&["kill-server"]);
        assert_eq!(after, "100x30");
    }

    /// A program that cannot be executed is reported by the spawn itself.
    #[test]
    fn an_exec_failure_is_the_spawns_error() {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize::default())
            .expect("pty");
        let tty = slave_path(pair.master.as_raw_fd().unwrap()).unwrap();
        let error = spawn(std::path::Path::new("/does-not-exist"), &[], &[], &tty).unwrap_err();
        assert!(
            error.message().contains("pane client spawn failed"),
            "{}",
            error.message()
        );
    }
}
