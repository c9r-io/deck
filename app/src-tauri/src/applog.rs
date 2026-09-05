//! `~/.deck/app.log`: structured, sanitized, 0600.
//!
//! Every line passes `redact::sanitize_log` on its way to disk; session
//! names are logged as a per-run tag (`session_tag`), errors as a stable
//! category (`err_code`) — the full error text goes back to the caller's
//! toast, never into the log. One `LOG_LOCK` serializes append, reset,
//! rotation and the one-time migration of logs an older deck wrote
//! (`sanitize_existing_logs`); it recovers from poisoning on its own so
//! `sync.rs` can log a recovered lock without depending on this lock.
//! `applog` is a no-op under `cfg(test)`: unit tests never touch the real log.

use crate::datadir::{atomic_write, create_private_dir, deck_dir, now_epoch, write_private};
use crate::redact::sanitize_log;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOG_LOCK: Mutex<()> = Mutex::new(());

/// Stable, content-free category for an error message. The FULL error text
/// goes back to the current operation's caller (command result → UI toast);
/// the log gets only this code — raw io/tmux/storage/serde Display texts can
/// embed absolute paths, directories or file contents and must never be
/// interpolated into app.log.
pub(crate) fn err_code(e: &str) -> &'static str {
    let l = e.to_ascii_lowercase();
    if l.contains("permission denied") || l.contains("read-only") {
        "perm"
    } else if l.contains("already running") {
        "locked"
    } else if l.contains("not a directory") {
        "not-dir"
    } else if l.contains("tmux not runnable") {
        "tmux-missing"
    } else if l.contains("no such file") || l.contains("not found") || l.contains("no such path") {
        "missing"
    } else if l.contains("no space") || l.contains("quota") {
        "disk-full"
    } else if l.contains("newer deck") {
        "newer-schema"
    } else if l.contains("context identity changed") {
        "context-changed"
    } else if l.contains("invalid json")
        || l.contains("wrong structure")
        || l.contains("refusing to save")
        || l.contains("expected")
    {
        "invalid-doc"
    } else if l.contains("no server")
        || l.contains("can't find session")
        || l.contains("can't find pane")
        || l.contains("no such session")
    {
        "no-session"
    } else if l.contains("tmux") {
        "tmux"
    } else if l.contains("unreadable") || l.contains("backup") || l.contains("corrupt") {
        "recovery"
    } else {
        "other"
    }
}

/// Non-reversible, per-RUN short tag for a session name. Log lines need to
/// correlate events of one session; the NAME itself is user-derived (it is
/// built from a card title) and must not be persisted, so it is hashed with
/// a per-process random seed — the same session reads consistently within a
/// run and is meaningless across runs.
pub(crate) fn session_tag(name: &str) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    static SEED: std::sync::OnceLock<RandomState> = std::sync::OnceLock::new();
    let mut h = SEED.get_or_init(RandomState::new).build_hasher();
    h.write(name.as_bytes());
    format!("sess-{:05x}", h.finish() & 0xf_ffff)
}

pub(crate) fn log_path(dir: &Path) -> PathBuf {
    dir.join("app.log")
}

/// Reset only the active diagnostic log, without creating a backup. The same
/// lock guards append and rotation so no pre-reset file can be written back.
pub(crate) fn reset_logs_at(dir: &Path) -> Result<(), String> {
    let _guard = LOG_LOCK
        .lock()
        .map_err(|_| "log lock unavailable".to_string())?;
    create_private_dir(dir)?;
    atomic_write(&log_path(dir), b"")
}

pub(crate) fn log_size_at(dir: &Path) -> Result<u64, String> {
    match std::fs::metadata(log_path(dir)) {
        Ok(meta) if meta.is_file() => Ok(meta.len()),
        Ok(_) => Err("log is not a file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(format!("could not read log size ({})", e.kind())),
    }
}

/// Append one sanitized line to `path` (0600 from creation). Split out of
/// `applog` so the real writing path — including redaction and permissions —
/// is exercised by tests against a temp directory instead of being stubbed.
pub(crate) fn applog_to(path: &Path, msg: &str) {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        let _ = create_private_dir(dir);
    }
    let ts = now_epoch();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600) // applies at creation; legacy 0644 logs are fixed at boot
        .open(path)
    {
        let _ = writeln!(f, "{ts} {}", sanitize_log(msg));
    }
}

/// Append a line to ~/.deck/app.log (the app may be launched via `open`,
/// where stderr goes nowhere useful). The log is 0600: it must never carry
/// user content, but even metadata stays private to the user.
pub(crate) fn applog(msg: &str) {
    if cfg!(test) {
        return; // unit tests must not write into the user's real app.log
    }
    applog_to(&log_path(&deck_dir()), msg);
}

/// One-time, in-place migration of logs and exports an OLDER deck wrote:
/// absolute paths, URLs, token shapes and raw session names are replaced
/// with `<redacted>`. The files keep their diagnostic structure, are
/// rewritten atomically 0600, and no copy of the original content is left
/// behind (a `.bak` would defeat the whole point). Files that need no change
/// are not touched at all.
pub(crate) fn sanitize_existing_logs(dir: &Path) -> u32 {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut cleaned = 0;
    let mut targets = vec![log_path(dir)];
    if let Ok(rd) = std::fs::read_dir(dir.join("exports")) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_file()).unwrap_or(false) {
                targets.push(e.path());
            }
        }
    }
    for path in targets {
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue; // missing or non-UTF-8: nothing to rewrite safely
        };
        let clean: String = raw.lines().map(sanitize_log).collect::<Vec<_>>().join("\n");
        let clean = if raw.ends_with('\n') && !clean.is_empty() {
            clean + "\n"
        } else {
            clean
        };
        if clean != raw && atomic_write(&path, clean.as_bytes()).is_ok() {
            cleaned += 1;
        }
    }
    cleaned
}

pub(crate) fn rotate_log() {
    let _guard = LOG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = log_path(&deck_dir());
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > 2 * 1024 * 1024 {
            if let Ok(data) = std::fs::read(&path) {
                let keep = &data[data.len().saturating_sub(512 * 1024)..];
                let _ = write_private(&path, keep);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datadir::harden_data_dir;

    fn tdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("deck-applog-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn set_mode(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// Everything that must never reach app.log or an export. Each marker is
    /// distinctive enough that a substring search is a real tripwire.
    const MARKERS: &[&str] = &[
        "/Users/example/private",
        "file:///secret",
        "https://example.com/private",
        "~/Documents/taxes",
        "ghp_AbCdEf0123456789xyz",
        "sk_live_4242424242424242",
        "AKIAIOSFODNN7EXAMPLE",
        "xoxb-9999-8888-distinctivetoken",
        "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
        "deck-quarterly-report-ab12",
        "distinctive0secret0value0here",
    ];

    #[test]
    fn reset_log_preserves_other_data_and_resumes_private_logging() {
        let d = tdir("log-reset");
        assert_eq!(log_size_at(&d).unwrap(), 0);
        reset_logs_at(&d).unwrap();
        applog_to(&log_path(&d), "[test] before-reset");
        assert!(log_size_at(&d).unwrap() > 0);
        create_private_dir(&d.join("exports")).unwrap();
        for name in ["history.json", "settings.json", "exports/previous.txt"] {
            write_private(&d.join(name), b"keep").unwrap();
        }
        reset_logs_at(&d).unwrap();
        assert_eq!(log_size_at(&d).unwrap(), 0);
        assert_eq!(mode_of(&log_path(&d)), 0o600);
        assert!(!d.join("app.log.bak").exists());
        for name in ["history.json", "settings.json", "exports/previous.txt"] {
            assert_eq!(std::fs::read(d.join(name)).unwrap(), b"keep");
        }
        applog_to(&log_path(&d), "[test] after-reset");
        let log = std::fs::read_to_string(log_path(&d)).unwrap();
        assert!(log.contains("after-reset"));
        assert!(!log.contains("before-reset"));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn reset_log_serializes_with_concurrent_writers() {
        let d = tdir("log-reset-concurrent");
        applog_to(&log_path(&d), "[test] old-event");
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let dir = &d;
                scope.spawn(move || {
                    for _ in 0..30 {
                        applog_to(&log_path(dir), "[test] concurrent-event");
                    }
                });
            }
            reset_logs_at(&d).unwrap();
        });
        applog_to(&log_path(&d), "[test] final-event");
        let log = std::fs::read_to_string(log_path(&d)).unwrap();
        assert!(!log.contains("old-event"));
        assert!(log.ends_with("[test] final-event\n"));
        assert!(log.lines().all(|line| line.contains("[test] ")));
        assert!(!log.contains('\0'));
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn reset_log_reports_io_failure_without_removing_other_files() {
        let d = tdir("log-reset-fail");
        std::fs::create_dir(log_path(&d)).unwrap();
        write_private(&log_path(&d).join("keep"), b"keep").unwrap();
        assert!(reset_logs_at(&d).is_err());
        assert!(log_size_at(&d).is_err());
        assert_eq!(std::fs::read(log_path(&d).join("keep")).unwrap(), b"keep");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn the_log_writer_redacts_every_marker_and_stays_private() {
        let d = tdir("log");
        let p = log_path(&d);
        // lines shaped like the ones deck really writes, plus the marker
        for m in MARKERS {
            applog_to(&p, &format!("[pty] attached {m} (80x24)"));
            applog_to(
                &p,
                &format!("[queue] send FAILED for {m} (attempt 1, tmux)"),
            );
            applog_to(&p, &format!("[ui] js-error {m} a=12"));
        }
        // and the shapes of real user content that must never be logged at
        // all: prompt text, a typed command, IME text, a bracketed paste,
        // a PTY marker — the call sites never build these, and if one did,
        // the redactor still has to catch the dangerous parts
        applog_to(&p, "[ui] record a=42");
        applog_to(
            &p,
            "[queue] sent to deck-alpha-9zz1 (17B, mode chain) rm -rf /Users/example/private",
        );
        applog_to(&p, "\x1b[200~pasted https://example.com/private\x1b[201~");
        let text = std::fs::read_to_string(&p).unwrap();
        for m in MARKERS {
            assert!(!text.contains(m), "app.log leaked {m}:\n{text}");
        }
        assert!(text.contains("<redacted>"), "redaction actually happened");
        // the diagnostic structure survives — that is the point of the log
        assert!(text.contains("[pty] attached") && text.contains("(80x24)"));
        assert!(text.contains("[ui] js-error") && text.contains("a=12"));
        assert_eq!(mode_of(&p), 0o600, "app.log is user-only");
        assert_eq!(mode_of(&d), 0o700);
    }

    #[test]
    fn assignment_json_quotes_ansi_and_multiple_values_are_redacted_in_files() {
        let d = tdir("log-shapes");
        let log = log_path(&d);
        let cases = [
            (
                "path=/Users/example/private/file",
                "/Users/example/private/file",
            ),
            (
                "token=ghp_ShapeMarker0123456789",
                "ghp_ShapeMarker0123456789",
            ),
            (
                r#"{"path":"/Users/example/json-private"}"#,
                "/Users/example/json-private",
            ),
            (r#"authorization: "Bearer shortSecret7""#, "shortSecret7"),
            (
                "wrapped=(https://example.com/private?q=1)",
                "https://example.com/private?q=1",
            ),
            ("\x1b[31m~/Documents/private\x1b[0m", "~/Documents/private"),
            ("a=1 token=shortSecret7 path=/opt/private/x", "shortSecret7"),
            (
                "jwt=eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJwcml2YXRlIn0.signature",
                "eyJhbGciOiJIUzI1NiJ9",
            ),
        ];
        for (line, _) in cases {
            applog_to(&log, line);
        }
        let text = std::fs::read_to_string(&log).unwrap();
        for (index, (_, secret)) in cases.iter().enumerate() {
            assert!(
                !text.contains(secret),
                "sensitive marker survived case {index}"
            );
        }
        assert_eq!(text.matches("<redacted>").count(), cases.len() + 1);

        // The migration path uses the exact same sanitizer on whole files.
        let exports = d.join("exports");
        std::fs::create_dir_all(&exports).unwrap();
        let old = exports.join("old.txt");
        std::fs::write(
            &old,
            cases.iter().map(|x| x.0).collect::<Vec<_>>().join("\n"),
        )
        .unwrap();
        assert_eq!(sanitize_existing_logs(&d), 1);
        let migrated = std::fs::read_to_string(old).unwrap();
        for (index, (_, secret)) in cases.iter().enumerate() {
            assert!(
                !migrated.contains(secret),
                "migration retained case {index}"
            );
        }
    }

    #[test]
    fn session_tags_identify_without_revealing() {
        let a = session_tag("deck-quarterly-report-ab12");
        assert_eq!(a, session_tag("deck-quarterly-report-ab12"), "stable");
        assert_ne!(a, session_tag("deck-other-card-cd34"), "distinguishes");
        assert!(!a.contains("quarterly") && !a.contains("report"));
        assert_eq!(sanitize_log(&a), a, "a tag is safe to log");
    }

    #[test]
    fn old_logs_and_exports_are_migrated_in_place() {
        let d = tdir("logmig");
        create_private_dir(&d).unwrap();
        let exports = d.join("exports");
        std::fs::create_dir_all(&exports).unwrap();
        let log = log_path(&d);
        let old_export = exports.join("deck-log-1787814782.txt");
        // what a pre-0.4.29 deck really left behind (the real-world case:
        // the absolute path of the bundled tmux binary), world-readable
        let body = "1787814000 [boot] deck started\n\
             1787814001 [tmux] using /Users/example/private/deck.app/Contents/MacOS/tmux\n\
             1787814002 [pty] attached deck-quarterly-report-ab12 (80x24)\n\
             1787814003 [queue] token ghp_AbCdEf0123456789xyz seen\n\
             1787814004 [poll] session listing recovered\n";
        std::fs::write(&log, body).unwrap();
        std::fs::write(&old_export, format!("deck 0.4.28\n{body}")).unwrap();
        set_mode(&log, 0o644);
        set_mode(&old_export, 0o644);

        assert_eq!(sanitize_existing_logs(&d), 2, "both files rewritten");
        harden_data_dir(&d).unwrap();
        for p in [&log, &old_export] {
            let t = std::fs::read_to_string(p).unwrap();
            for m in [
                "/Users/example/private",
                "deck-quarterly-report-ab12",
                "ghp_AbCdEf0123456789xyz",
            ] {
                assert!(!t.contains(m), "{} still leaks {m}", p.display());
            }
            assert!(t.contains("[boot] deck started"), "structure kept");
            assert!(t.contains("[poll] session listing recovered"));
            assert_eq!(
                t.lines().count(),
                body.lines().count() + usize::from(p == &old_export)
            );
            assert_eq!(mode_of(p), 0o600);
        }
        // no raw copy left anywhere in the tree
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            assert!(!name.contains(".bak"), "migration must not keep a raw copy");
        }
        // idempotent: a second run finds nothing left to clean
        assert_eq!(sanitize_existing_logs(&d), 0);
    }
}
