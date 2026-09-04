//! procinfo.rs — metadata-only process observation without spawning `ps`.
//!
//! deck polls the process tree every 2.5s (card memory) and reads a pane
//! tty's foreground process on every scheduler probe. Doing that by forking
//! `ps`/`date` made deck the busiest process-spawner on the machine, which
//! endpoint security tooling reads as process-discovery noise. Everything
//! here is a direct libproc/sysctl query for the same closed facts: pid,
//! parent, resident size, controlling tty, foreground process group and
//! argv[0]. No command lines, environments or paths beyond argv[0]'s
//! basename ever leave this module.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProcessInfo {
    pub(crate) pid: u32,
    pub(crate) ppid: u32,
    pub(crate) pgid: u32,
    /// Controlling terminal device (`st_rdev` of the tty node), 0 if none.
    pub(crate) tty: u32,
    /// The controlling terminal's foreground process group.
    pub(crate) tty_pgid: u32,
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
        tty: info.e_tdev,
        tty_pgid: info.e_tpgid,
    })
}

/// Resident set size in KiB; 0 for a process we may not inspect.
#[cfg(target_os = "macos")]
pub(crate) fn resident_kib(pid: u32) -> u64 {
    use std::ffi::c_void;
    let mut info = std::mem::MaybeUninit::<libc::proc_taskinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: the buffer is exactly the struct libproc fills for this flavor.
    let got = unsafe {
        libc::proc_pidinfo(
            pid as libc::pid_t,
            libc::PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast::<c_void>(),
            size,
        )
    };
    if got != size {
        return 0;
    }
    // SAFETY: libproc reported a full write of the struct.
    unsafe { info.assume_init() }.pti_resident_size / 1024
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

/// argv[0] of a process from `KERN_PROCARGS2` (what `ps -o comm` shows on
/// macOS). The layout is `argc`, the exec path, NUL padding, then argv[0].
#[cfg(target_os = "macos")]
pub(crate) fn argv0(pid: u32) -> Option<String> {
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
    parse_procargs2_argv0(&buffer)
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

/// Sum of resident memory over `roots` and all their descendants, in MiB,
/// keyed exactly like `roots`.
#[cfg(target_os = "macos")]
pub(crate) fn tree_memory(roots: &HashMap<String, u32>) -> HashMap<String, f64> {
    let table = processes();
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
            sum += resident_kib(pid);
            if let Some(kids) = children.get(&pid) {
                stack.extend(kids);
            }
        }
        result.insert(session.clone(), sum as f64 / 1024.0);
    }
    result
}

/// Local wall-clock minutes since midnight, from the C library's timezone
/// database; 12:00 if the platform cannot answer.
pub(crate) fn local_minutes() -> u32 {
    extern "C" {
        fn tzset();
    }
    // SAFETY: plain libc time calls on stack-owned values; tzset refreshes
    // the zone before every read so a timezone change is honored.
    unsafe {
        tzset();
        let now = libc::time(std::ptr::null_mut());
        let mut tm = std::mem::MaybeUninit::<libc::tm>::zeroed();
        if libc::localtime_r(&now, tm.as_mut_ptr()).is_null() {
            return 720;
        }
        let tm = tm.assume_init();
        (tm.tm_hour.clamp(0, 23) * 60 + tm.tm_min.clamp(0, 59)) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(pid: u32, ppid: u32, pgid: u32, tty: u32, tty_pgid: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            ppid,
            pgid,
            tty,
            tty_pgid,
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
        assert!(resident_kib(me) > 0);
        let own_argv0 = argv0(me).expect("own argv0 readable");
        let expected = std::env::current_exe().unwrap();
        assert_eq!(
            std::path::Path::new(&own_argv0).file_name(),
            expected.file_name()
        );
        assert_eq!(resident_kib(u32::MAX), 0);
        assert_eq!(argv0(u32::MAX), None);
    }

    #[test]
    fn local_minutes_is_a_wall_clock_minute_of_day() {
        assert!(local_minutes() < 24 * 60);
    }
}
