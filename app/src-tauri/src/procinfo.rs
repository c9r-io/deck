//! procinfo.rs — metadata-only process observation without spawning `ps`.
//!
//! deck polls the process tree every 2.5s (card memory) and reads a pane
//! tty's foreground process on every scheduler probe. Doing that by forking
//! `ps`/`date` made deck the busiest process-spawner on the machine, which
//! endpoint security tooling reads as process-discovery noise. Everything
//! here is a direct libproc/sysctl query for the same closed facts: pid,
//! parent, physical footprint, controlling tty, foreground process group and
//! argv[0]. No command lines, environments or paths beyond argv[0]'s
//! basename ever leave this module: `orphans_running` compares a full
//! command line here and hands back only pids.
//!
//! One narrow, approved exception serves the re-entry transaction
//! (`agent_reentry`): `qualify_client` and `shell_facts` read ONE exact
//! process (pid + birth instant) at preview, freeze and launch — never on a
//! poll — and `qualify_client` reduces its argv and environment HERE against
//! a static `LaunchGrammar`. What leaves is a closed `LaunchProfile`, one boolean, and
//! two bounded references: the process's real executable file
//! (`ExecutableRef`: path, file identity, SHA-256) and its kernel working
//! directory (`DirRef`). The raw argv, the environment, any token, and any
//! value of a recognized flag are dropped inside the reduction and reach no
//! caller, callback, log or `Debug` output; neither reference prints its
//! path. A profile describes how that client was STARTED, never its current
//! or final permission mode.

use std::collections::HashMap;

/// `e_tdev` of a process without a controlling terminal.
#[cfg(target_os = "macos")]
const NODEV: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProcessInfo {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    pub(crate) pgid: u32,
    /// Controlling terminal device (`st_rdev` of the tty node), 0 if none
    /// (the kernel's NODEV is normalized to 0).
    pub(crate) tty: u32,
    /// The controlling terminal's foreground process group.
    pub(crate) tty_pgid: u32,
    /// Birth instant (`pbi_start_tvsec` / `pbi_start_tvusec`) from the same
    /// `proc_bsdinfo` read: with the pid it tells a process generation
    /// apart from a later process that reused the pid. 0 when unknown.
    pub(crate) start_seconds: u64,
    pub(crate) start_micros: u32,
}

/// Stable process birth identity used by the Connector generation token.
/// Kept separate from the hot process-table scan so existing board polling
/// does not widen its data or cost.
#[cfg(target_os = "macos")]
pub(crate) fn process_start(pid: u32) -> Option<(u64, u32)> {
    use std::ffi::c_void;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::pid_t,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<c_void>(),
            size,
        )
    };
    if got != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    Some((info.pbi_start_tvsec, info.pbi_start_tvusec as u32))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn process_start(_pid: u32) -> Option<(u64, u32)> {
    None
}

#[cfg(target_os = "macos")]
fn list_pids() -> Vec<libc::pid_t> {
    use std::ffi::c_void;
    use std::ptr::null_mut;
    // SAFETY: a null buffer asks libproc for the byte count only.
    let bytes = unsafe { libc::proc_listallpids(null_mut::<c_void>(), 0) };
    if bytes <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0 as libc::pid_t; bytes as usize / std::mem::size_of::<libc::pid_t>() + 64];
    let capacity = (pids.len() * std::mem::size_of::<libc::pid_t>()) as libc::c_int;
    // SAFETY: the buffer is sized from the count above plus slack, and the
    // byte capacity passed matches the allocation.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast::<c_void>(), capacity) };
    if filled <= 0 {
        return Vec::new();
    }
    pids.truncate(filled as usize);
    pids.retain(|pid| *pid > 0);
    pids
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: libc::pid_t) -> Option<ProcessInfo> {
    use std::ffi::c_void;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly the struct libproc fills for this flavor.
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<c_void>(),
            size,
        )
    };
    if got != size {
        return None;
    }
    // SAFETY: libproc reported a full write of the struct.
    let info = unsafe { info.assume_init() };
    Some(ProcessInfo {
        pid: pid as u32,
        ppid: info.pbi_ppid,
        pgid: info.pbi_pgid,
        // the kernel reports "no controlling terminal" as NODEV
        // ((dev_t)-1), not 0 — found by the real detached-hook test
        tty: if info.e_tdev == NODEV { 0 } else { info.e_tdev },
        tty_pgid: info.e_tpgid,
        start_seconds: info.pbi_start_tvsec,
        start_micros: info.pbi_start_tvusec as u32,
    })
}

/// Physical footprint in KiB (`ri_phys_footprint`, the number Activity
/// Monitor shows); 0 for a process we may not inspect. Not the resident set:
/// RSS counts the dyld shared cache and every shared binary's text in each
/// process, so summing it over a tree of a dozen node processes reported
/// roughly double the memory they actually held.
#[cfg(target_os = "macos")]
pub(crate) fn footprint_kib(pid: u32) -> u64 {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: `RUSAGE_INFO_V2` asks the kernel to fill exactly the v2 struct;
    // the buffer pointer is cast the way libproc's own header prescribes.
    let got = unsafe {
        libc::proc_pid_rusage(
            pid as libc::pid_t,
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    if got != 0 {
        return 0;
    }
    // SAFETY: a zero return means the kernel wrote the whole struct.
    unsafe { info.assume_init() }.ri_phys_footprint / 1024
}

/// Every visible process's identity facts, keyed by pid.
#[cfg(target_os = "macos")]
pub(crate) fn processes() -> HashMap<u32, ProcessInfo> {
    list_pids()
        .into_iter()
        .filter_map(bsd_info)
        .map(|info| (info.pid, info))
        .collect()
}

/// One process's identity facts — the same read `processes` makes per pid —
/// for a point check that must not scan the whole table.
#[cfg(target_os = "macos")]
pub(crate) fn process(pid: u32) -> Option<ProcessInfo> {
    bsd_info(libc::pid_t::try_from(pid).ok()?)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn process(_pid: u32) -> Option<ProcessInfo> {
    None
}

/// The pid on the other end of a connected unix socket, from the kernel
/// (`LOCAL_PEERPID`) — never from anything the peer wrote. It stays valid
/// for the life of the socket even after the peer exits.
#[cfg(target_os = "macos")]
pub(crate) fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `len` point to valid storage for LOCAL_PEERPID.
    let result = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(stream),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    (result == 0 && len as usize == std::mem::size_of::<libc::pid_t>() && pid > 0)
        .then_some(pid as u32)
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn peer_pid(_stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    None
}

/// `pid` and its ancestors, nearest first, from one process-table snapshot
/// (so the chain and any other fact taken from `table` describe the same
/// instant): at most `cap` entries, stopping at launchd (pid 1) or at a
/// process missing from the snapshot. A chain that ends early is simply
/// short — callers treat "the pane is not in the chain" as a refusal.
pub(crate) fn ancestry_in(table: &HashMap<u32, ProcessInfo>, pid: u32, cap: usize) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut current = pid;
    while current > 1 && chain.len() < cap {
        let Some(info) = table.get(&current) else {
            break;
        };
        chain.push(current);
        if info.ppid == current {
            break;
        }
        current = info.ppid;
    }
    chain
}

/// argv[0] of a process from `KERN_PROCARGS2` (what `ps -o comm` shows on
/// macOS). The layout is `argc`, the exec path, NUL padding, then argv[0].
#[cfg(target_os = "macos")]
pub(crate) fn argv0(pid: u32) -> Option<String> {
    parse_procargs2_argv0(&procargs2(pid)?)
}

/// The raw `KERN_PROCARGS2` image of `pid`.
#[cfg(target_os = "macos")]
fn procargs2(pid: u32) -> Option<Vec<u8>> {
    use std::ffi::c_void;
    use std::ptr::null_mut;
    let mut argmax: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: KERN_ARGMAX writes one c_int into a buffer of that size.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            (&mut argmax as *mut libc::c_int).cast::<c_void>(),
            &mut len,
            null_mut(),
            0,
        )
    };
    if rc != 0 || argmax <= 0 {
        return None;
    }
    let mut buffer = vec![0u8; argmax as usize];
    let mut size = buffer.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    // SAFETY: the buffer is KERN_ARGMAX bytes, the documented upper bound.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buffer.as_mut_ptr().cast::<c_void>(),
            &mut size,
            null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buffer.truncate(size);
    Some(buffer)
}

/// Pure parser for a `KERN_PROCARGS2` image.
pub(crate) fn parse_procargs2_argv0(image: &[u8]) -> Option<String> {
    let (count, rest) = image.split_at_checked(std::mem::size_of::<libc::c_int>())?;
    let argc = i32::from_ne_bytes(count.try_into().ok()?);
    if argc < 1 {
        return None;
    }
    let exec_end = rest.iter().position(|b| *b == 0)?;
    let after_exec = &rest[exec_end..];
    let argv_start = after_exec.iter().position(|b| *b != 0)?;
    let argv = &after_exec[argv_start..];
    let argv0_end = argv.iter().position(|b| *b == 0).unwrap_or(argv.len());
    let argv0 = std::str::from_utf8(&argv[..argv0_end]).ok()?;
    (!argv0.is_empty()).then(|| argv0.to_string())
}

/// Exec path and the complete argv (exactly `argc` NUL-terminated strings)
/// of a `KERN_PROCARGS2` image; `None` for a truncated or non-UTF-8 image.
fn parse_procargs2_command(image: &[u8]) -> Option<(&str, Vec<&str>)> {
    let (count, rest) = image.split_at_checked(std::mem::size_of::<libc::c_int>())?;
    let argc = usize::try_from(i32::from_ne_bytes(count.try_into().ok()?)).ok()?;
    let exec_end = rest.iter().position(|b| *b == 0)?;
    let exec = std::str::from_utf8(&rest[..exec_end]).ok()?;
    let after_exec = &rest[exec_end..];
    let mut tail = &after_exec[after_exec.iter().position(|b| *b != 0)?..];
    let mut argv = Vec::with_capacity(argc);
    for _ in 0..argc {
        let end = tail.iter().position(|b| *b == 0)?;
        argv.push(std::str::from_utf8(&tail[..end]).ok()?);
        tail = &tail[end + 1..];
    }
    Some((exec, argv))
}

/// Whether a command line is `exec` run with exactly `prefix` plus one final
/// argument that starts with `last_prefix`.
fn command_matches(
    (path, argv): (&str, Vec<&str>),
    exec: &str,
    prefix: &[&str],
    last_prefix: &str,
) -> bool {
    path == exec
        && argv.len() == prefix.len() + 1
        && argv[..prefix.len()] == *prefix
        && argv[prefix.len()].starts_with(last_prefix)
}

/// Pids of this user's orphaned processes (reparented to launchd) whose exec
/// path is `exec` and whose argv is `prefix` plus one argument starting with
/// `last_prefix`. The command lines are compared here and never leave this
/// module; only the pids do.
#[cfg(target_os = "macos")]
pub(crate) fn orphans_running(exec: &str, prefix: &[&str], last_prefix: &str) -> Vec<u32> {
    use std::ffi::c_void;
    // SAFETY: getuid(2) cannot fail.
    let uid = unsafe { libc::getuid() };
    list_pids()
        .into_iter()
        .filter(|&pid| {
            let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
            // SAFETY: the buffer is exactly the struct libproc fills for this flavor.
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    info.as_mut_ptr().cast::<c_void>(),
                    size,
                )
            };
            // SAFETY: only read after libproc reported a full write.
            got == size && {
                let info = unsafe { info.assume_init() };
                info.pbi_ppid == 1 && info.pbi_uid == uid
            }
        })
        .filter(|&pid| {
            procargs2(pid as u32).is_some_and(|image| {
                parse_procargs2_command(&image)
                    .is_some_and(|command| command_matches(command, exec, prefix, last_prefix))
            })
        })
        .map(|pid| pid as u32)
        .collect()
}

// ---------- re-entry qualification (exact target, low frequency) -------------

/// The launch shapes one certified client supports, stated by its adapter as
/// static data. The reduction of a live argv/environment against it happens
/// in this module only; nothing here is a callback that could carry a raw
/// argument out.
pub(crate) struct LaunchGrammar {
    /// The one flag that classifies a launch as `Bypass`.
    pub(crate) bypass: &'static str,
    /// `<flag> <id>` / `<flag>=<id>`: an exact-id resume; the id must have
    /// this shape and is dropped (it never stands in for a final session id).
    pub(crate) resume: &'static str,
    pub(crate) resume_value: fn(&str) -> bool,
    /// A permission-mode flag and the values that still classify `Ordinary`.
    pub(crate) mode: &'static str,
    pub(crate) ordinary_modes: &'static [&'static str],
    /// Flags taking exactly one value that are recognized and dropped.
    pub(crate) dropped: &'static [&'static str],
    /// A flag taking one or more directories; seeing it sets one boolean.
    pub(crate) directories: &'static str,
    /// Environment names, and name prefixes, whose PRESENCE makes the
    /// necessary context unprovable (their values are never read).
    pub(crate) refused_env: &'static [&'static str],
    pub(crate) refused_env_prefixes: &'static [&'static str],
}

/// How an exact client was started. Never its current or final permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchProfile {
    /// The supported launch shape with the bypass flag.
    Bypass,
    /// The supported launch shape without it.
    Ordinary,
    /// The launch image could not be read completely: no proof either way.
    Unknown,
    /// An unknown flag, a positional argument, a refused environment name or
    /// another home: the necessary context cannot be rebuilt.
    Unsupported,
}

/// Exec path, argv and environment strings of a `KERN_PROCARGS2` image.
fn parse_procargs2_image(image: &[u8]) -> Option<(&str, Vec<&str>, Vec<&str>)> {
    let (exec, argv) = parse_procargs2_command(image)?;
    // locate the byte after the last argv string
    let (count, rest) = image.split_at_checked(std::mem::size_of::<libc::c_int>())?;
    let argc = usize::try_from(i32::from_ne_bytes(count.try_into().ok()?)).ok()?;
    let exec_end = rest.iter().position(|b| *b == 0)?;
    let after_exec = &rest[exec_end..];
    let mut tail = &after_exec[after_exec.iter().position(|b| *b != 0)?..];
    for _ in 0..argc {
        let end = tail.iter().position(|b| *b == 0)?;
        tail = &tail[end + 1..];
    }
    let mut env = Vec::new();
    while let Some(end) = tail.iter().position(|b| *b == 0) {
        if end == 0 {
            break;
        }
        env.push(std::str::from_utf8(&tail[..end]).ok()?);
        tail = &tail[end + 1..];
    }
    Some((exec, argv, env))
}

/// Whether an environment carries exactly `home` as HOME and none of the
/// names the grammar refuses. Values other than HOME's are never compared.
fn environment_supported(env: &[&str], home: &str, grammar: &LaunchGrammar) -> bool {
    let mut home_seen = false;
    for entry in env {
        let Some((name, value)) = entry.split_once('=') else {
            continue;
        };
        if name == "HOME" {
            if value != home || home_seen {
                return false;
            }
            home_seen = true;
        }
        if grammar.refused_env.contains(&name)
            || grammar
                .refused_env_prefixes
                .iter()
                .any(|prefix| name.starts_with(prefix))
        {
            return false;
        }
    }
    home_seen
}

/// The closed reduction of one launch image: its profile and whether a
/// directory flag was present. Pure; every argument is dropped here.
fn reduce_launch(
    argv: &[&str],
    env: &[&str],
    home: &str,
    grammar: &LaunchGrammar,
) -> (LaunchProfile, bool) {
    let unsupported = (LaunchProfile::Unsupported, false);
    let (mut bypass, mut mode, mut directories, mut resumed) = (false, false, false, false);
    let mut rest = argv.iter().skip(1).copied().peekable();
    while let Some(token) = rest.next() {
        let (flag, inline) = match token.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (token, None),
        };
        let value = |rest: &mut std::iter::Peekable<_>| -> Option<&str> {
            inline
                .or_else(|| rest.next())
                .filter(|v: &&str| !v.starts_with('-'))
        };
        if flag == grammar.bypass && inline.is_none() {
            bypass = true;
        } else if flag == grammar.resume {
            if resumed || !value(&mut rest).is_some_and(|id| (grammar.resume_value)(id)) {
                return unsupported;
            }
            resumed = true;
        } else if flag == grammar.mode {
            if mode || !value(&mut rest).is_some_and(|m| grammar.ordinary_modes.contains(&m)) {
                return unsupported;
            }
            mode = true;
        } else if grammar.dropped.contains(&flag) {
            if value(&mut rest).is_none() {
                return unsupported;
            }
        } else if flag == grammar.directories {
            if value(&mut rest).is_none() {
                return unsupported;
            }
            while rest.next_if(|next| !next.starts_with('-')).is_some() {}
            directories = true;
        } else {
            return unsupported;
        }
    }
    if (bypass && mode) || !environment_supported(env, home, grammar) {
        return unsupported;
    }
    let profile = if bypass {
        LaunchProfile::Bypass
    } else {
        LaunchProfile::Ordinary
    };
    (profile, directories)
}

// How many full content reads this thread started (the cost tests).
#[cfg(test)]
thread_local! {
    pub(crate) static FULL_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The largest executable the qualification will hash.
const MAX_EXECUTABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// One real executable file as it was when read: where it is, which file it
/// is, and what it contains. `Debug` prints none of it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExecutableRef {
    path: std::path::PathBuf,
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    /// The inode's change instant. Unlike the modification instant it cannot
    /// be set back by the file's owner, so a same-size rewrite that restores
    /// the modification time still reads as another file state.
    changed: (i64, i64),
    sha256: [u8; 32],
}

impl std::fmt::Debug for ExecutableRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecutableRef(..)")
    }
}

impl ExecutableRef {
    /// The file at `path` if it is a regular file that names itself (no
    /// link on the way), read and hashed now.
    pub(crate) fn read(path: &std::path::Path) -> Option<Self> {
        use sha2::Digest;
        use std::io::Read;
        use std::os::unix::fs::MetadataExt;
        if !path.is_absolute() || std::fs::canonicalize(path).ok()?.as_path() != path {
            return None;
        }
        #[cfg(test)]
        FULL_READS.with(|reads| reads.set(reads.get() + 1));
        let mut file = std::fs::File::open(path).ok()?;
        let before = file.metadata().ok()?;
        if !before.is_file() || before.size() > MAX_EXECUTABLE_BYTES {
            return None;
        }
        let mut hasher = sha2::Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            }
            // Bounded by size above and by the caller's deadline here: a
            // read that outlives its phase yields no reference at all.
            if crate::session_runtime::check_deadline().is_err() {
                return None;
            }
        }
        // the path must still name the file that was hashed
        let after = std::fs::symlink_metadata(path).ok()?;
        let identity = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.size(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        if !after.is_file() || identity(&after) != identity(&before) {
            return None;
        }
        Some(Self {
            path: path.to_path_buf(),
            device: before.dev(),
            inode: before.ino(),
            size: before.size(),
            modified: (before.mtime(), before.mtime_nsec()),
            changed: (before.ctime(), before.ctime_nsec()),
            sha256: hasher.finalize().into(),
        })
    }

    /// Lowercase hex SHA-256 of the file's content.
    pub(crate) fn digest_hex(&self) -> String {
        self.sha256.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// The path still names the same file with the same content (re-read).
    pub(crate) fn unchanged(&self) -> bool {
        Self::read(&self.path).is_some_and(|now| now == *self)
    }

    /// The path still names the very file state that was hashed: same device
    /// and inode, same size, same modification AND change instants. The
    /// content is not re-read; this is valid only as a follow-up to a full
    /// read of this reference in the same operation, and any write, rename,
    /// replacement or metadata change since then fails it.
    pub(crate) fn same_state(&self) -> bool {
        self.same_file_at(&self.path)
    }

    /// `path` is this reference's path and still names the same file state
    /// (the content is not re-read).
    fn same_file_at(&self, path: &std::path::Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        path == self.path
            && std::fs::symlink_metadata(path).is_ok_and(|meta| {
                meta.is_file()
                    && (meta.dev(), meta.ino(), meta.size()) == (self.device, self.inode, self.size)
                    && (meta.mtime(), meta.mtime_nsec()) == self.modified
                    && (meta.ctime(), meta.ctime_nsec()) == self.changed
            })
    }

    /// The launch target: this exact absolute path, for the launch sink only.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// A directory reference read from the kernel. `Debug` prints none of it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct DirRef(std::path::PathBuf);

impl std::fmt::Debug for DirRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DirRef(..)")
    }
}

impl DirRef {
    #[cfg(test)]
    pub(crate) fn for_tests(path: &std::path::Path) -> Self {
        Self(path.to_path_buf())
    }
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
    pub(crate) fn is_dir(&self) -> bool {
        self.0.is_absolute() && self.0.is_dir()
    }
}

/// The real path of the file `pid` is executing.
#[cfg(target_os = "macos")]
fn image_path(pid: u32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is the documented maximum for this call.
    let len = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
        )
    };
    if len <= 0 {
        return None;
    }
    buffer.truncate(len as usize);
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(
        buffer,
    )))
}

/// The kernel's current working directory of `pid`.
#[cfg(target_os = "macos")]
fn working_dir(pid: u32) -> Option<DirRef> {
    use std::os::unix::ffi::OsStrExt;
    let mut info = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly the struct libproc fills for this flavor.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if got != size {
        return None;
    }
    // SAFETY: libproc reported a full write of the struct.
    let info = unsafe { info.assume_init() };
    let raw = &info.pvi_cdir.vip_path;
    let bytes: Vec<u8> = raw
        .iter()
        .flatten()
        .map(|c| *c as u8)
        .take_while(|b| *b != 0)
        .collect();
    let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&bytes));
    path.is_absolute().then_some(DirRef(path))
}

/// What one exact client process qualifies as.
#[derive(Debug)]
pub(crate) struct ClientQualification {
    /// The real file it runs, read and hashed; `None` when it cannot be
    /// established (gone, replaced, not a regular file).
    pub(crate) executable: Option<ExecutableRef>,
    pub(crate) profile: LaunchProfile,
    /// A directory flag was present at launch (an observation, not a list).
    pub(crate) additional_dirs: bool,
    pub(crate) cwd: Option<DirRef>,
}

/// One exact process, asked whether it is a named client and how it started.
#[derive(Debug)]
pub(crate) enum ClientRead {
    /// No such process generation (exited, or the pid was reused).
    Gone,
    /// Its executable carries none of the wanted names; nothing was hashed.
    NotNamed,
    Named(ClientQualification),
}

fn name_of(path: &std::path::Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// Qualify the process `pid` born at `birth`: is its executable (by file
/// name, or by argv[0]'s basename) one of `names`, which file is it, how was
/// it started and where does it run. The generation is checked before and
/// after every read; a process that changed in between reads as `Gone`.
#[cfg(target_os = "macos")]
pub(crate) fn qualify_client(
    pid: u32,
    birth: (u64, u32),
    names: &[&str],
    home: &std::path::Path,
    grammar: &LaunchGrammar,
    known: Option<&ExecutableRef>,
) -> ClientRead {
    let alive = || process_start(pid) == Some(birth);
    if !alive() {
        return ClientRead::Gone;
    }
    let Some(image) = image_path(pid) else {
        return ClientRead::Gone;
    };
    let args = procargs2(pid);
    let parsed = args.as_deref().and_then(parse_procargs2_image);
    let argv_name = parsed
        .as_ref()
        .and_then(|(_, argv, _)| argv.first())
        .and_then(|argv0| name_of(std::path::Path::new(argv0)));
    let named = |name: Option<&str>| name.is_some_and(|name| names.contains(&name));
    if !named(name_of(&image)) && !named(argv_name) {
        return ClientRead::NotNamed;
    }
    let (profile, additional_dirs) = match (&parsed, home.to_str()) {
        (Some((_, argv, env)), Some(home)) => reduce_launch(argv, env, home, grammar),
        _ => (LaunchProfile::Unknown, false),
    };
    // A reference read moments ago for this very file (same path, device,
    // inode, size and modification instant) is reused instead of hashing
    // the file again inside the restart's preparation budget; the launch
    // re-reads the content in full (`ExecutableRef::unchanged`).
    let executable = match known.filter(|known| known.same_file_at(&image)) {
        Some(known) => Some(known.clone()),
        None => ExecutableRef::read(&image),
    };
    let cwd = working_dir(pid);
    if !alive() || image_path(pid).as_deref() != Some(image.as_path()) {
        return ClientRead::Gone;
    }
    ClientRead::Named(ClientQualification {
        executable,
        profile,
        additional_dirs,
        cwd,
    })
}

/// What one re-entry operation expects of the client it started. Every
/// field is Deck's own data — the frozen executable, the request words this
/// operation built, the expected home, the frozen directory; nothing here is
/// read from the process, and nothing read from the process is returned.
pub(crate) struct LaunchExpectation<'a> {
    pub(crate) executable: &'a ExecutableRef,
    /// The argument words of the one request this operation typed, in order.
    pub(crate) words: &'a [String],
    pub(crate) home: &'a std::path::Path,
    /// For the environment names whose presence is refused.
    pub(crate) grammar: &'a LaunchGrammar,
    pub(crate) cwd: &'a DirRef,
}

/// The closed answer of `check_launch`. It names which expectation failed
/// and carries no argument, environment entry, path or id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LaunchCheck {
    /// Image, request, launch environment and directory are the expected ones.
    Matches,
    /// No such process generation (exited, or the pid was reused).
    Gone,
    /// The kernel did not hand out the launch image or the directory: no
    /// proof either way.
    Unreadable,
    /// It executes another file than the frozen executable.
    OtherImage,
    /// Its arguments are not exactly the request this operation built.
    OtherRequest,
    /// Its launch environment has another home or a refused name.
    OtherEnvironment,
    /// Its working directory is not the frozen one.
    OtherDirectory,
}

// Makes the next launch-image reads on this thread fail (the tests of an
// unreadable launch image).
#[cfg(test)]
thread_local! {
    pub(crate) static LAUNCH_IMAGE_UNREADABLE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The pure part of `check_launch`: one launch image against the expected
/// request and environment. The arguments after argv[0] must be EXACTLY the
/// expected words — same count, same order, same spelling — so another id,
/// an added or missing flag and an inline `flag=value` spelling all differ.
fn judge_launch(argv: &[&str], env: &[&str], expect: &LaunchExpectation<'_>) -> LaunchCheck {
    let Some(home) = expect.home.to_str() else {
        return LaunchCheck::OtherEnvironment;
    };
    let words = argv.get(1..).unwrap_or_default();
    if words.len() != expect.words.len()
        || words
            .iter()
            .zip(expect.words)
            .any(|(seen, wanted)| seen != wanted)
    {
        return LaunchCheck::OtherRequest;
    }
    // An image with arguments and no environment at all is one the kernel
    // withheld the environment of, not a client started with none.
    if env.is_empty() {
        return LaunchCheck::Unreadable;
    }
    if !environment_supported(env, home, expect.grammar) {
        return LaunchCheck::OtherEnvironment;
    }
    LaunchCheck::Matches
}

/// Whether the process `pid` born at `birth` is the client this operation
/// asked for, as far as the kernel shows it — and what that evidence is:
///
/// - **image** and **working directory** are read NOW (the file the process
///   executes at this instant, the directory it is in at this instant);
/// - **arguments** and **environment** are the launch image the kernel kept
///   from the exec: what the process was started with. They are not its
///   current configuration, and a process can overwrite that area itself.
///
/// So `Matches` means "started by exactly this request, in the expected
/// home, with no refused name, executing the frozen file in the frozen
/// directory when looked at". It does not mean any file the client read
/// afterwards, any setting it loaded, or its effective permission.
///
/// The raw arguments and environment are parsed and dropped here; only the
/// closed `LaunchCheck` leaves, and no callback sees them.
#[cfg(target_os = "macos")]
pub(crate) fn check_launch(
    pid: u32,
    birth: (u64, u32),
    expect: &LaunchExpectation<'_>,
) -> LaunchCheck {
    let alive = || process_start(pid) == Some(birth);
    if !alive() {
        return LaunchCheck::Gone;
    }
    if !runs_executable(pid, birth, expect.executable) {
        return if alive() {
            LaunchCheck::OtherImage
        } else {
            LaunchCheck::Gone
        };
    }
    #[cfg(test)]
    if LAUNCH_IMAGE_UNREADABLE.with(std::cell::Cell::get) {
        return LaunchCheck::Unreadable;
    }
    let image = procargs2(pid);
    let Some((_, argv, env)) = image.as_deref().and_then(parse_procargs2_image) else {
        return if alive() {
            LaunchCheck::Unreadable
        } else {
            LaunchCheck::Gone
        };
    };
    let judged = judge_launch(&argv, &env, expect);
    let cwd = working_dir(pid);
    // every read above was of this generation, still executing this file
    if !alive() || !runs_executable(pid, birth, expect.executable) {
        return LaunchCheck::Gone;
    }
    match (judged, cwd) {
        (LaunchCheck::Matches, None) => LaunchCheck::Unreadable,
        (LaunchCheck::Matches, Some(cwd)) if cwd != *expect.cwd => LaunchCheck::OtherDirectory,
        (judged, _) => judged,
    }
}

/// How much of a launch image the kernel hands out for `pid`: the number of
/// arguments and of environment entries, or `None` when the image cannot be
/// read or parsed. Counts only — for the native certification probe, which
/// must tell "nothing readable" from "readable and different" without any
/// argument or environment entry leaving this module.
#[cfg(all(test, target_os = "macos"))]
pub(crate) fn launch_image_shape_for_tests(pid: u32) -> Option<(usize, usize)> {
    let image = procargs2(pid)?;
    let (_, argv, env) = parse_procargs2_image(&image)?;
    Some((argv.len(), env.len()))
}

/// Whether the process `pid` born at `birth` is executing exactly the file
/// `executable` refers to (same path, same file identity).
#[cfg(target_os = "macos")]
pub(crate) fn runs_executable(pid: u32, birth: (u64, u32), executable: &ExecutableRef) -> bool {
    use std::os::unix::fs::MetadataExt;
    process_start(pid) == Some(birth)
        && image_path(pid).is_some_and(|image| {
            image == executable.path
                && std::fs::symlink_metadata(&image).is_ok_and(|meta| {
                    meta.is_file()
                        && meta.dev() == executable.device
                        && meta.ino() == executable.inode
                })
        })
}

/// Whether `pid` is executing the very file at `path` (file identity).
#[cfg(target_os = "macos")]
pub(crate) fn runs_file(pid: u32, path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let identity = |p: &std::path::Path| {
        std::fs::metadata(p)
            .ok()
            .filter(std::fs::Metadata::is_file)
            .map(|meta| (meta.dev(), meta.ino()))
    };
    image_path(pid).is_some_and(|image| {
        let (ours, theirs) = (identity(path), identity(&image));
        ours.is_some() && ours == theirs
    })
}

/// Whether one exact process generation still exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Liveness {
    /// The pid exists with this birth instant.
    Alive,
    /// No such pid, or the pid belongs to a later process.
    Gone,
    /// The pid exists but its birth instant cannot be read: no proof of an
    /// exit. Callers must not treat this as gone.
    Unknown,
}

/// The liveness of the process `pid` born at `birth`, from two kernel reads.
pub(crate) fn generation_liveness(pid: u32, birth: (u64, u32)) -> Liveness {
    match process_start(pid) {
        Some(now) if now == birth => Liveness::Alive,
        Some(_) => Liveness::Gone,
        None => {
            // SAFETY: signal 0 delivers nothing; it only asks whether the
            // pid names a process.
            let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
            let missing =
                rc != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            if missing {
                Liveness::Gone
            } else {
                Liveness::Unknown
            }
        }
    }
}

/// Whether THIS process's environment — the baseline every tmux server and
/// pane Deck starts inherits — carries exactly `home` and none of the names
/// the grammar refuses. Names only, and HOME's value; nothing is returned
/// or kept. A baseline is not the context a shell's startup files leave
/// behind: it proves what Deck handed over, not what a client inherits.
pub(crate) fn own_baseline_supported(home: &std::path::Path, grammar: &LaunchGrammar) -> bool {
    let Some(home) = home.to_str() else {
        return false;
    };
    let entries: Vec<String> = std::env::vars_os()
        .filter_map(|(name, value)| {
            let name = name.into_string().ok()?;
            Some(if name == "HOME" {
                format!("HOME={}", value.to_str()?)
            } else {
                format!("{name}=")
            })
        })
        .collect();
    let entries: Vec<&str> = entries.iter().map(String::as_str).collect();
    environment_supported(&entries, home, grammar)
}

/// The user's login shell as the account database names it (what tmux
/// starts in a new pane by default). Read, never constructed: deck holds no
/// shell path of its own.
pub(crate) fn login_shell() -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::zeroed();
    let mut buffer = vec![0u8; 4096];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwuid_r fills the caller's entry and string buffer; the
    // strings are copied out before the buffer is dropped.
    let shell = unsafe {
        let rc = libc::getpwuid_r(
            libc::getuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        );
        if rc != 0 || found.is_null() || (*found).pw_shell.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*found).pw_shell)
            .to_bytes()
            .to_vec()
    };
    let path = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&shell));
    path.is_absolute().then_some(path)
}

/// The file belongs to the system: a regular file owned by root that
/// nobody else may write.
pub(crate) fn system_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.uid() == 0 && meta.mode() & 0o022 == 0)
}

/// What the kernel says about a freshly created shell. Its launch
/// environment is NOT among these facts: the kernel does not hand out a
/// system shell's environment, so the launch sink proves the environment
/// from the tmux server Deck itself started instead.
#[derive(Debug)]
pub(crate) struct ShellFacts {
    /// It executes exactly `shell`.
    pub(crate) image: bool,
    /// That file belongs to the system: owned by root and writable by
    /// nobody else (a user-installed shell build is another program).
    pub(crate) system: bool,
    pub(crate) cwd: Option<DirRef>,
}

#[cfg(target_os = "macos")]
pub(crate) fn shell_facts(
    pid: u32,
    birth: (u64, u32),
    shell: &std::path::Path,
) -> Option<ShellFacts> {
    if process_start(pid) != Some(birth) {
        return None;
    }
    let facts = ShellFacts {
        image: runs_file(pid, shell),
        system: system_file(shell),
        cwd: working_dir(pid),
    };
    (process_start(pid) == Some(birth)).then_some(facts)
}

/// The pid leading the foreground process group of the terminal whose
/// device number is `tty`, from a process table snapshot. This is the
/// process `ps -t` marks `+` and lists as its own group leader.
pub(crate) fn foreground_leader(table: &HashMap<u32, ProcessInfo>, tty: u32) -> Option<u32> {
    if tty == 0 {
        return None;
    }
    let foreground = table
        .values()
        .find(|info| info.tty == tty && info.tty_pgid > 0)
        .map(|info| info.tty_pgid)?;
    table
        .values()
        .find(|info| info.tty == tty && info.pgid == foreground && info.pid == foreground)
        .map(|info| info.pid)
}

/// Device number of a tty node such as `/dev/ttys004`.
#[cfg(target_os = "macos")]
pub(crate) fn tty_device(path: &str) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    u32::try_from(meta.rdev()).ok().filter(|dev| *dev != 0)
}

/// Sum of physical footprint over `roots` and all their descendants, in
/// MiB, keyed exactly like `roots`, over one process-table snapshot (the
/// poll shares it with agent-status reconciliation).
#[cfg(target_os = "macos")]
pub(crate) fn tree_memory(
    table: &HashMap<u32, ProcessInfo>,
    roots: &HashMap<String, u32>,
) -> HashMap<String, f64> {
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for info in table.values() {
        children.entry(info.ppid).or_default().push(info.pid);
    }
    let mut result = HashMap::new();
    for (session, root) in roots {
        let mut sum = 0u64;
        let mut stack = vec![*root];
        let mut seen = std::collections::HashSet::new();
        while let Some(pid) = stack.pop() {
            if !seen.insert(pid) || !table.contains_key(&pid) {
                continue;
            }
            sum += footprint_kib(pid);
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids);
            }
        }
        result.insert(session.clone(), sum as f64 / 1024.0);
    }
    result
}

/// Local wall-clock facts read once per tick from the C library's timezone
/// database (never a spawned `date`): the minute of the day for the
/// scheduler's daily windows, and the local calendar day for clock rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LocalClock {
    /// epoch seconds the clock was read at
    pub(crate) now: u64,
    /// minutes since local midnight (0..1440)
    pub(crate) min: u32,
    /// epoch second this local day started at — `mktime` of 00:00 local,
    /// so it is one value for the whole day even across a DST switch (a
    /// `now - seconds since midnight` reading would move by the shift and
    /// hand the same slot two keys)
    pub(crate) day_start: u64,
    /// ISO weekday: 1 = Monday … 7 = Sunday
    pub(crate) wday: u32,
    /// day of the month, 1-based
    pub(crate) mday: u32,
    /// how many days this local month has
    pub(crate) mdays: u32,
}

impl LocalClock {
    /// A clock at `min` on Monday the 1st of a 31-day month: the platform
    /// fallback, and what unit tests use for a deterministic day.
    pub(crate) fn synthetic(now: u64, min: u32) -> Self {
        LocalClock {
            now,
            min,
            day_start: now.saturating_sub(u64::from(min) * 60),
            wday: 1,
            mday: 1,
            mdays: 31,
        }
    }
}

fn days_in_month(year: i32, month0: i32) -> u32 {
    match month0 {
        1 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        3 | 5 | 8 | 10 => 30,
        _ => 31,
    }
}

/// The local wall clock; noon on a synthetic day if the platform cannot
/// answer.
pub(crate) fn local_clock() -> LocalClock {
    extern "C" {
        fn tzset();
    }
    // SAFETY: tzset refreshes the zone before every read so a timezone
    // change is honored; `time` with a null pointer only returns.
    unsafe {
        tzset();
        clock_at(libc::time(std::ptr::null_mut()))
    }
}

/// The local wall clock at epoch second `now` (the caller has refreshed the
/// zone).
fn clock_at(now: libc::time_t) -> LocalClock {
    let epoch = u64::try_from(now).unwrap_or(0);
    let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: plain libc time calls on stack-owned values.
    let tm = unsafe {
        if libc::localtime_r(&now, tm.as_mut_ptr()).is_null() {
            return LocalClock::synthetic(epoch, 720);
        }
        tm.assume_init()
    };
    let hour = tm.tm_hour.clamp(0, 23) as u32;
    let min = tm.tm_min.clamp(0, 59) as u32;
    let sec = tm.tm_sec.clamp(0, 59) as u32;
    let since_midnight = u64::from(hour * 3600 + min * 60 + sec);
    let mut midnight = tm;
    midnight.tm_hour = 0;
    midnight.tm_min = 0;
    midnight.tm_sec = 0;
    midnight.tm_isdst = -1;
    // SAFETY: mktime only reads and normalizes the stack-owned tm.
    let day_start = u64::try_from(unsafe { libc::mktime(&mut midnight) })
        .ok()
        .filter(|&start| start <= epoch && epoch - start < 26 * 3600)
        .unwrap_or_else(|| epoch.saturating_sub(since_midnight));
    LocalClock {
        now: epoch,
        min: hour * 60 + min,
        day_start,
        // tm_wday counts from Sunday = 0; ISO counts from Monday = 1
        wday: ((tm.tm_wday.clamp(0, 6) + 6) % 7 + 1) as u32,
        mday: tm.tm_mday.clamp(1, 31) as u32,
        mdays: days_in_month(tm.tm_year + 1900, tm.tm_mon.clamp(0, 11)),
    }
}

/// Local wall-clock minutes since midnight.
pub(crate) fn local_minutes() -> u32 {
    local_clock().min
}

#[cfg(test)]
mod tests {
    use crate::sync::LockRecover;

    #[test]
    fn ancestry_walks_parents_nearest_first_and_respects_its_cap() {
        let table = super::processes();
        let me = std::process::id();
        let chain = super::ancestry_in(&table, me, 32);
        assert_eq!(chain.first(), Some(&me));
        assert_eq!(chain.get(1), table.get(&me).map(|info| &info.ppid));
        assert!(chain.len() >= 2, "a test process has a parent: {chain:?}");
        assert!(!chain.contains(&1), "launchd is never listed");
        assert_eq!(super::ancestry_in(&table, me, 1), vec![me]);
        assert!(super::ancestry_in(&table, 0, 32).is_empty());
        assert!(super::ancestry_in(&table, u32::MAX - 1, 32).is_empty());
    }

    /// A process without a controlling terminal (any user agent daemon)
    /// reads 0, never the kernel's NODEV — terminal continuity compares
    /// against 0.
    #[test]
    fn a_process_without_a_terminal_reads_tty_zero() {
        let table = super::processes();
        assert!(table.values().all(|info| info.tty != super::NODEV));
        assert!(table.values().any(|info| info.tty == 0));
    }

    use super::*;

    fn info(pid: u32, ppid: u32, pgid: u32, tty: u32, tty_pgid: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            pgid,
            tty,
            tty_pgid,
            ..ProcessInfo::default()
        }
    }

    #[test]
    fn procargs2_yields_argv0_after_the_exec_path_padding() {
        let mut image = 2i32.to_ne_bytes().to_vec();
        image.extend_from_slice(b"/Users/x/.claude/versions/2.1.259\0\0\0");
        image.extend_from_slice(b"claude\0--resume\0HOME=/Users/x\0");
        assert_eq!(parse_procargs2_argv0(&image).as_deref(), Some("claude"));

        let mut login = 1i32.to_ne_bytes().to_vec();
        login.extend_from_slice(b"/bin/zsh\0-zsh\0");
        assert_eq!(parse_procargs2_argv0(&login).as_deref(), Some("-zsh"));

        assert_eq!(parse_procargs2_argv0(b""), None);
        assert_eq!(parse_procargs2_argv0(&0i32.to_ne_bytes()), None);
        let mut truncated = 1i32.to_ne_bytes().to_vec();
        truncated.extend_from_slice(b"/bin/zsh");
        assert_eq!(parse_procargs2_argv0(&truncated), None);
    }

    #[test]
    fn a_command_matches_only_its_exact_exec_argv_and_final_shape() {
        let mut image = 4i32.to_ne_bytes().to_vec();
        image.extend_from_slice(b"/Applications/Deck.app/Contents/MacOS/tmux\0\0\0");
        image.extend_from_slice(
            b"/Applications/Deck.app/Contents/MacOS/tmux\0-L\0deck\0=main\0HOME=/x\0",
        );
        let command = || parse_procargs2_command(&image).expect("parsed");
        let exec = "/Applications/Deck.app/Contents/MacOS/tmux";
        let prefix = [exec, "-L", "deck"];
        assert_eq!(command().1, vec![exec, "-L", "deck", "=main"]);
        assert!(command_matches(command(), exec, &prefix, "="));
        assert!(!command_matches(
            command(),
            "/opt/homebrew/bin/tmux",
            &prefix,
            "="
        ));
        assert!(!command_matches(
            command(),
            exec,
            &[exec, "-L", "deck-dev"],
            "="
        ));
        assert!(!command_matches(command(), exec, &prefix, "=other"));
        assert!(!command_matches(command(), exec, &[exec, "-L"], "="));
        let mut truncated = 3i32.to_ne_bytes().to_vec();
        truncated.extend_from_slice(b"/bin/tmux\0tmux\0-L");
        assert_eq!(parse_procargs2_command(&truncated), None);
    }

    #[test]
    fn a_real_orphan_is_found_by_its_exact_command_and_nothing_else() {
        // A unique duration makes the argv ours alone; the shell exits at
        // once so the sleep is reparented to launchd like a dead Deck's child.
        let unique = format!("600.{}", std::process::id());
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("/bin/sleep {unique} >/dev/null 2>&1 &")])
            .status()
            .unwrap();
        assert!(status.success());
        let mut found = Vec::new();
        for _ in 0..100 {
            found = orphans_running("/bin/sleep", &["/bin/sleep"], &unique);
            if !found.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(found.len(), 1, "exactly the one orphan");
        assert!(orphans_running("/bin/sleep", &["/bin/sleep"], "600.x").is_empty());
        assert!(orphans_running("/bin/cat", &["/bin/sleep"], &unique).is_empty());
        // SAFETY: kill(2) on the pid matched above.
        unsafe { libc::kill(found[0] as libc::pid_t, libc::SIGKILL) };
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(orphans_running("/bin/sleep", &["/bin/sleep"], &unique).is_empty());
    }

    fn id_shape(value: &str) -> bool {
        value.len() == 36 && value.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
    }

    const GRAMMAR: LaunchGrammar = LaunchGrammar {
        bypass: "--dangerously-skip-permissions",
        resume: "--resume",
        resume_value: id_shape,
        mode: "--permission-mode",
        ordinary_modes: &["default", "plan", "acceptEdits"],
        dropped: &["--model"],
        directories: "--add-dir",
        refused_env: &["CLAUDE_CONFIG_DIR"],
        refused_env_prefixes: &["ANTHROPIC_"],
    };
    const UUID: &str = "731c5908-251a-4388-b3a3-4719ad442bce";
    const HOME: &[&str] = &["PATH=/usr/bin", "HOME=/Users/u", "TERM=xterm"];

    fn reduced(args: &[&str]) -> (LaunchProfile, bool) {
        let mut argv = vec!["claude"];
        argv.extend_from_slice(args);
        reduce_launch(&argv, HOME, "/Users/u", &GRAMMAR)
    }

    /// The live launch image is reduced to a closed profile. Only the exact
    /// bypass flag classifies as bypass; an unknown word never does.
    #[test]
    fn a_launch_image_reduces_to_a_closed_profile() {
        use LaunchProfile::*;
        assert_eq!(reduced(&[]), (Ordinary, false));
        assert_eq!(
            reduced(&["--dangerously-skip-permissions"]),
            (Bypass, false)
        );
        assert_eq!(reduced(&["--permission-mode", "plan"]), (Ordinary, false));
        assert_eq!(
            reduced(&["--permission-mode=acceptEdits"]),
            (Ordinary, false)
        );
        // both forms this feature generates classify again; the id is dropped
        assert_eq!(reduced(&["--resume", UUID]), (Ordinary, false));
        assert_eq!(
            reduced(&["--dangerously-skip-permissions", "--resume", UUID]),
            (Bypass, false)
        );
        assert_eq!(reduced(&[&format!("--resume={UUID}")]), (Ordinary, false));
        // recognized and dropped: a model, and directories as one boolean
        assert_eq!(reduced(&["--model", "opus"]), (Ordinary, false));
        assert_eq!(
            reduced(&["--add-dir", "/a", "/b", "--dangerously-skip-permissions"]),
            (Bypass, true)
        );
        for unsupported in [
            vec!["--allow-dangerously-skip-permissions"],
            vec!["--dangerously-skip-permissions=1"],
            vec!["--permission-mode", "bypassPermissions"],
            vec!["--permission-mode"],
            vec![
                "--dangerously-skip-permissions",
                "--permission-mode",
                "plan",
            ],
            vec!["--resume"],
            vec!["--resume", "latest"],
            vec!["--resume", UUID, "--resume", UUID],
            vec!["--continue"],
            vec!["-c"],
            vec!["--fork-session"],
            vec!["--settings", "/x.json"],
            vec!["--model"],
            vec!["--add-dir"],
            vec!["fix the bug"],
            vec!["-p", "hello"],
        ] {
            assert_eq!(
                reduced(&unsupported),
                (Unsupported, false),
                "{unsupported:?}"
            );
        }
    }

    #[test]
    fn an_environment_is_supported_only_with_the_expected_home_and_no_refused_name() {
        let argv = ["claude", "--dangerously-skip-permissions"];
        let profile = |env: &[&str]| reduce_launch(&argv, env, "/Users/u", &GRAMMAR).0;
        assert_eq!(profile(HOME), LaunchProfile::Bypass);
        for env in [
            &["PATH=/usr/bin"][..],
            &["HOME=/Users/other"],
            &["HOME=/Users/u", "HOME=/Users/u"],
            &["HOME=/Users/u", "CLAUDE_CONFIG_DIR=/x"],
            &["HOME=/Users/u", "ANTHROPIC_API_KEY=SENTINEL-SECRET"],
            &["HOME=/Users/u", "ANTHROPIC_BASE_URL="],
        ] {
            assert_eq!(profile(env), LaunchProfile::Unsupported, "{env:?}");
        }
        // a name that merely resembles a refused one is not refused
        assert_eq!(
            profile(&[
                "HOME=/Users/u",
                "MY_ANTHROPIC_NOTE=1",
                "CLAUDE_CONFIG_DIRS=1"
            ]),
            LaunchProfile::Bypass
        );
    }

    /// The post-launch check is the exact request, not a profile: the same
    /// words in the same order, nothing added, nothing missing.
    #[test]
    fn a_launch_matches_only_as_exactly_the_built_request_in_the_expected_environment() {
        const ID: &str = "731c5908-251a-4388-b3a3-4719ad442bce";
        const OTHER: &str = "831c5908-251a-4388-b3a3-4719ad442bce";
        const FLAG: &str = "--dangerously-skip-permissions";
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let executable = ExecutableRef::read(&exe).unwrap();
        let cwd = DirRef::for_tests(std::path::Path::new("/"));
        let judge = |argv: &[&str], env: &[&str], wanted: &[&str]| {
            let wanted: Vec<String> = wanted.iter().map(|word| word.to_string()).collect();
            judge_launch(
                argv,
                env,
                &LaunchExpectation {
                    executable: &executable,
                    words: &wanted,
                    home: std::path::Path::new("/Users/u"),
                    grammar: &GRAMMAR,
                    cwd: &cwd,
                },
            )
        };
        let plain = ["--resume", ID];
        let bypass = [FLAG, "--resume", ID];
        assert_eq!(
            judge(&["/x/claude", "--resume", ID], HOME, &plain),
            LaunchCheck::Matches
        );
        assert_eq!(
            judge(&["/x/claude", FLAG, "--resume", ID], HOME, &bypass),
            LaunchCheck::Matches
        );
        for (argv, wanted) in [
            // another native session
            (&["c", "--resume", OTHER][..], &plain[..]),
            // bypass that this operation did not ask for, and the reverse
            (&["c", FLAG, "--resume", ID], &plain),
            (&["c", "--resume", ID], &bypass),
            // the same words in another order, or spelled inline
            (&["c", "--resume", ID, FLAG], &bypass),
            (
                &["c", "--resume=731c5908-251a-4388-b3a3-4719ad442bce"],
                &plain,
            ),
            // the same launch PROFILE, another request shape
            (&["c", "--resume", ID, "--permission-mode", "plan"], &plain),
            (&["c", "--resume", ID, "--model", "opus"], &plain),
            (&["c", "--resume", ID, "continue"], &plain),
            (&["c", "--resume"], &plain),
            (&["c"], &plain),
            (&[], &plain),
        ] {
            assert_eq!(
                judge(argv, HOME, wanted),
                LaunchCheck::OtherRequest,
                "{argv:?}"
            );
        }
        for env in [
            &["HOME=/Users/other"][..],
            &["PATH=/usr/bin"],
            &["HOME=/Users/u", "CLAUDE_CONFIG_DIR=/x"],
            &["HOME=/Users/u", "ANTHROPIC_BASE_URL=SENTINEL"],
        ] {
            assert_eq!(
                judge(&["c", "--resume", ID], env, &plain),
                LaunchCheck::OtherEnvironment,
                "{env:?}"
            );
        }
        // an image without any environment was withheld, not proven
        assert_eq!(
            judge(&["c", "--resume", ID], &[], &plain),
            LaunchCheck::Unreadable
        );
        // the closed answer names no argument, environment entry or path
        for check in [LaunchCheck::OtherRequest, LaunchCheck::OtherEnvironment] {
            let text = format!("{check:?}");
            assert!(!text.contains(ID) && !text.contains("SENTINEL") && !text.contains('/'));
        }
    }

    /// The same check against a real process (this one): every expectation
    /// is read for one exact generation, and each mismatch has its own answer.
    #[test]
    fn a_real_process_is_checked_by_generation_image_request_and_directory() {
        let me = std::process::id();
        let birth = process_start(me).expect("own birth");
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let executable = ExecutableRef::read(&exe).unwrap();
        let other = ExecutableRef::read(&std::fs::canonicalize("/bin/ls").unwrap()).unwrap();
        let own_words: Vec<String> = std::env::args().skip(1).collect();
        let home = std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let here = working_dir(me).expect("own directory");
        let elsewhere = DirRef::for_tests(std::path::Path::new("/nonexistent/elsewhere"));
        // this process's own environment is not the subject: refuse no name
        let open = LaunchGrammar {
            refused_env: &[],
            refused_env_prefixes: &[],
            ..GRAMMAR
        };
        let check = |birth, executable: &ExecutableRef, words: &[String], cwd: &DirRef| {
            check_launch(
                me,
                birth,
                &LaunchExpectation {
                    executable,
                    words,
                    home: &home,
                    grammar: &open,
                    cwd,
                },
            )
        };
        assert_eq!(
            check(birth, &executable, &own_words, &here),
            LaunchCheck::Matches
        );
        assert_eq!(
            check((birth.0 + 1, birth.1), &executable, &own_words, &here),
            LaunchCheck::Gone
        );
        assert_eq!(
            check(birth, &other, &own_words, &here),
            LaunchCheck::OtherImage
        );
        let mut more = own_words.clone();
        more.push("--resume".into());
        assert_eq!(
            check(birth, &executable, &more, &here),
            LaunchCheck::OtherRequest
        );
        assert_eq!(
            check(birth, &executable, &own_words, &elsewhere),
            LaunchCheck::OtherDirectory
        );
        LAUNCH_IMAGE_UNREADABLE.with(|fault| fault.set(true));
        let unreadable = check(birth, &executable, &own_words, &here);
        LAUNCH_IMAGE_UNREADABLE.with(|fault| fault.set(false));
        assert_eq!(unreadable, LaunchCheck::Unreadable);
    }

    #[test]
    fn a_procargs_image_yields_exactly_argc_arguments_and_then_the_environment() {
        let mut image = 3i32.to_ne_bytes().to_vec();
        image.extend_from_slice(b"/opt/claude/versions/2.1.294\0\0\0\0");
        image.extend_from_slice(b"/opt/claude/versions/2.1.294\0--resume\0x\0");
        image.extend_from_slice(b"HOME=/Users/u\0TERM=xterm\0\0\0garbage");
        let (exec, argv, env) = parse_procargs2_image(&image).expect("parsed");
        assert_eq!(exec, "/opt/claude/versions/2.1.294");
        assert_eq!(argv, ["/opt/claude/versions/2.1.294", "--resume", "x"]);
        assert_eq!(env, ["HOME=/Users/u", "TERM=xterm"]);
        // a truncated or non-UTF-8 image has no profile to read
        assert!(parse_procargs2_image(&image[..40]).is_none());
        let mut bad = 1i32.to_ne_bytes().to_vec();
        bad.extend_from_slice(b"/bin/x\0\0x\0HOME=\xff\xfe\0");
        assert!(parse_procargs2_image(&bad).is_none());
    }

    /// The executable reference follows no link, notices a replaced file and
    /// prints nothing about where or what it is.
    #[test]
    fn an_executable_reference_is_one_real_file_and_notices_any_change() {
        let dir = std::env::temp_dir().join(format!("deck-exe-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let real = dir.join("2.1.294");
        std::fs::write(&real, b"first build").unwrap();
        let link = dir.join("claude");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let reference = ExecutableRef::read(&real).expect("a regular file");
        // SHA-256 of the bytes just written
        assert_eq!(reference.digest_hex(), {
            use sha2::Digest;
            sha2::Sha256::digest(b"first build")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        });
        assert_eq!(reference.digest_hex().len(), 64);
        assert!(reference.unchanged());
        assert_eq!(reference.path(), real);
        // a launcher link is never the reference, and a relative path never
        assert!(ExecutableRef::read(&link).is_none());
        assert!(ExecutableRef::read(std::path::Path::new("2.1.294")).is_none());
        assert!(ExecutableRef::read(&dir).is_none());
        assert_eq!(format!("{reference:?}"), "ExecutableRef(..)");
        assert_eq!(format!("{:?}", DirRef(dir.clone())), "DirRef(..)");
        // the link is moved to another release: the frozen file is unchanged
        let other = dir.join("2.1.295");
        std::fs::write(&other, b"second build").unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(reference.unchanged());
        // the same version rebuilt in place: another file, another content
        std::fs::write(dir.join("staged"), b"other bytes").unwrap();
        std::fs::rename(dir.join("staged"), &real).unwrap();
        assert!(!reference.unchanged());
        std::fs::remove_file(&real).unwrap();
        assert!(!reference.unchanged());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A rewrite of the same size that puts the modification time back is
    /// still another file state (the change instant cannot be set back) and
    /// another content; a deadline that has passed yields no reference.
    #[test]
    fn a_same_size_rewrite_and_an_expired_deadline_never_pass_as_the_reference() {
        let dir = std::env::temp_dir().join(format!("deck-exe-same-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let file = dir.join("2.1.294");
        std::fs::write(&file, b"build-AAAA").unwrap();
        let reference = ExecutableRef::read(&file).expect("a regular file");
        assert!(reference.same_state() && reference.unchanged());
        let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
        // the change instant has a coarse clock on some volumes
        std::thread::sleep(std::time::Duration::from_millis(20));
        {
            use std::io::Write;
            let mut handle = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
            handle.write_all(b"build-BBBB").unwrap();
            handle.set_modified(modified).unwrap();
        }
        let now = std::fs::metadata(&file).unwrap();
        assert_eq!(now.len(), 10, "same size");
        assert_eq!(now.modified().unwrap(), modified, "same modification time");
        assert!(!reference.same_state(), "the cheap check sees the rewrite");
        assert!(!reference.unchanged(), "the full read sees the rewrite");
        // a fresh reference of the rewritten file is another reference
        let rewritten = ExecutableRef::read(&file).expect("still a regular file");
        assert_ne!(rewritten.digest_hex(), reference.digest_hex());
        assert!(rewritten != reference);
        // a read under an expired deadline gives nothing, never a partial hash
        {
            let _expired = crate::session_runtime::Deadline::until(std::time::Instant::now());
            assert!(ExecutableRef::read(&file).is_none());
            assert!(!rewritten.unchanged());
            // the cheap state check reads no content and is not a hash
            assert!(rewritten.same_state());
        }
        assert!(rewritten.unchanged());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Read-only carrier preflight: the real certified image must finish its
    /// complete re-read inside the unchanged production post-phase budget.
    /// An unoptimized SHA-256 dependency can exhaust that budget in debug.
    #[test]
    #[ignore = "requires an explicitly supplied certified executable; no client is started"]
    fn certified_image_reread_fits_the_production_post_budget() {
        let path = std::path::PathBuf::from(
            std::env::var_os("DECK_REENTRY_EXECUTABLE_PROBE")
                .expect("explicit read-only executable probe"),
        );
        let reference = ExecutableRef::read(&path).expect("complete certified image read");
        assert!(crate::agent_reentry::claude::CAPABILITY
            .certified(&reference.digest_hex())
            .is_some());
        let started = std::time::Instant::now();
        let _deadline =
            crate::session_runtime::Deadline::until(started + crate::agent_reentry::POST_BUDGET);
        assert!(
            reference.unchanged(),
            "full image re-read inside the deadline"
        );
        assert!(started.elapsed() < crate::agent_reentry::POST_BUDGET);
    }

    /// This very process, qualified: its generation, file and working
    /// directory are read; another birth instant is another process.
    #[test]
    fn qualifying_reads_one_exact_generation_and_nothing_by_name_alone() {
        let me = std::process::id();
        let birth = process_start(me).expect("own birth");
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap().to_string();
        assert!(matches!(
            qualify_client(me, (birth.0 + 1, birth.1), &[&name], &home, &GRAMMAR, None),
            ClientRead::Gone
        ));
        assert!(matches!(
            qualify_client(me, birth, &["claude", "2.1.294"], &home, &GRAMMAR, None),
            ClientRead::NotNamed
        ));
        let ClientRead::Named(read) = qualify_client(me, birth, &[&name], &home, &GRAMMAR, None)
        else {
            panic!("named by its own file name");
        };
        let executable = read.executable.expect("own executable is a regular file");
        assert_eq!(executable.path(), exe);
        assert!(runs_executable(me, birth, &executable));
        assert!(!runs_executable(me, (birth.0 + 1, birth.1), &executable));
        assert!(runs_file(me, &exe) && !runs_file(me, std::path::Path::new("/bin/zsh")));
        assert_eq!(
            read.cwd.expect("own cwd").path(),
            std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap()
        );
        // the test harness is started with arguments this grammar does not know
        assert!(matches!(
            read.profile,
            LaunchProfile::Unsupported | LaunchProfile::Ordinary
        ));
        assert!(!format!("{:?}", executable).contains(&name));
    }

    #[test]
    fn foreground_leader_is_the_tty_foreground_group_leader() {
        let mut table = HashMap::new();
        // login shell (group leader, background), agent leader in the
        // foreground group and a child of that agent in the same group
        table.insert(100, info(100, 1, 100, 7, 200));
        table.insert(200, info(200, 100, 200, 7, 200));
        table.insert(201, info(201, 200, 200, 7, 200));
        // an unrelated tty
        table.insert(300, info(300, 1, 300, 8, 300));
        assert_eq!(foreground_leader(&table, 7), Some(200));
        assert_eq!(foreground_leader(&table, 8), Some(300));
        assert_eq!(foreground_leader(&table, 9), None);
        assert_eq!(foreground_leader(&table, 0), None);
        // a foreground group whose leader already exited names nobody
        table.remove(&200);
        assert_eq!(foreground_leader(&table, 7), None);
    }

    #[test]
    fn live_table_contains_this_process_and_its_argv0() {
        let table = processes();
        let me = std::process::id();
        let mine = table.get(&me).expect("own process listed");
        assert_eq!(mine.pid, me);
        assert!(mine.ppid > 0);
        assert!(footprint_kib(me) > 0);
        let own_argv0 = argv0(me).expect("own argv0 readable");
        let expected = std::env::current_exe().unwrap();
        assert_eq!(
            std::path::Path::new(&own_argv0).file_name(),
            expected.file_name()
        );
        assert_eq!(footprint_kib(u32::MAX), 0);
        assert_eq!(argv0(u32::MAX), None);
        // the snapshot's birth instant is the one process_start reads
        let (seconds, micros) = process_start(me).expect("own start");
        assert_eq!((mine.start_seconds, mine.start_micros), (seconds, micros));
    }

    #[test]
    fn snapshot_ancestry_stops_at_launchd_a_gap_or_the_cap() {
        let mut table = HashMap::new();
        table.insert(10, info(10, 1, 10, 0, 0));
        table.insert(20, info(20, 10, 20, 0, 0));
        table.insert(30, info(30, 20, 30, 0, 0));
        assert_eq!(ancestry_in(&table, 30, 32), vec![30, 20, 10]);
        assert_eq!(ancestry_in(&table, 30, 2), vec![30, 20]);
        table.remove(&20);
        assert_eq!(
            ancestry_in(&table, 30, 32),
            vec![30],
            "a gap ends the chain"
        );
        assert_eq!(ancestry_in(&table, 99, 32), Vec::<u32>::new());
    }

    /// `TZ` is process-wide: the one test that changes it and every test
    /// that reads the real local clock hold this lock.
    static TZ_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    extern "C" {
        fn tzset();
    }

    /// Sets `TZ` for one test and restores the previous value (and the libc
    /// zone) when dropped, a failed assertion included.
    struct TzOverride(Option<std::ffi::OsString>);

    impl TzOverride {
        fn set(zone: &str) -> Self {
            let before = std::env::var_os("TZ");
            std::env::set_var("TZ", zone);
            // SAFETY: tzset only re-reads the environment.
            unsafe { tzset() };
            TzOverride(before)
        }
    }

    impl Drop for TzOverride {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => std::env::set_var("TZ", value),
                None => std::env::remove_var("TZ"),
            }
            // SAFETY: as above.
            unsafe { tzset() };
        }
    }

    #[test]
    fn local_clock_is_a_consistent_local_day() {
        let _tz = TZ_TESTS.lock_or_recover();
        let c = local_clock();
        assert!(c.min < 1440);
        assert!((1..=7).contains(&c.wday));
        assert!((1..=c.mdays).contains(&c.mday));
        assert!((28..=31).contains(&c.mdays));
        assert!(c.day_start <= c.now);
        assert!(c.now - c.day_start < 26 * 3600);
        assert!(local_minutes() < 24 * 60);
        assert_eq!(days_in_month(2024, 1), 29);
        assert_eq!(days_in_month(2100, 1), 28);
        assert_eq!(days_in_month(2000, 1), 29);
        assert_eq!(days_in_month(2026, 8), 30);
        assert_eq!(days_in_month(2026, 11), 31);
        let s = LocalClock::synthetic(1_000_000, 600);
        assert_eq!(s.day_start, 1_000_000 - 36_000);
    }

    /// The whole local day shares one `day_start`, so a clock rule's slot
    /// keeps one key across a DST switch (both directions) — the bug was a
    /// 00:30 slot firing twice on the switch day.
    #[test]
    fn day_start_is_stable_across_a_dst_switch() {
        let _tz = TZ_TESTS.lock_or_recover();
        let _zone = TzOverride::set("America/New_York");
        // 2026-03-08: clocks jump 02:00 → 03:00; 2026-11-01: 02:00 → 01:00
        let spring_midnight = 1_772_946_000;
        let spring = [
            (1_772_947_800, 30),  // 00:30 EST
            (1_772_978_400, 600), // 10:00 EDT
        ];
        let fall_midnight = 1_793_505_600;
        let fall = [
            (1_793_507_400, 30),  // 00:30 EDT
            (1_793_545_200, 600), // 10:00 EST
        ];
        for (midnight, day) in [(spring_midnight, spring), (fall_midnight, fall)] {
            for (at, min) in day {
                let c = clock_at(at);
                assert_eq!(c.min, min, "wall minute at {at}");
                assert_eq!(c.day_start, midnight, "day start at {at}");
                assert_eq!(c.wday, 7, "both switch days are Sundays");
            }
        }
    }
}
