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
//! (`ExecutableRef`: path, file identity, SHA-256, validated code identity)
//! and its kernel working directory (`DirRef`). The raw argv, the
//! environment, any token, and any value of a recognized flag are dropped
//! inside the reduction and reach no caller, callback, log or `Debug`
//! output; neither reference prints its path. A profile describes how that
//! client was STARTED, never its current or final permission mode.
//!
//! Whose code a process is, and that it executes the referenced file, is
//! asked of Security.framework in this process (`code_identity`, a private
//! child module: its header is the contract of those checks). Three kinds of
//! fact are kept apart everywhere below: what the KERNEL says of a process
//! (generation, image path, launch image, directory), what the DISK holds
//! (file state, content digest, signature) and what Security proves of the
//! RUNNING image (`binding`). A file name, a version, a path, a signature
//! that validates on disk or a digest never stands in for the third; a
//! digest is compared only with itself, inside one transaction.
//!
//! The third kind is asked by a call that cannot be interrupted. It is made
//! only for a caller that owns a deadline, and its answer exists only when
//! it returned inside that deadline (`validated`): a late "yes" is no
//! binding and no product, and it is not "another program" either. The
//! deadline bounds the answer, never the call — a caller that must stay
//! responsive asks from a worker it can stop waiting for.
//!
//! A fourth fact stands beside those three and is none of them: whether a
//! process is still the EXECUTION INSTANCE it was (`InstanceWatch`). A pid and
//! a birth instant survive an exec, so a caller that bound something to "the
//! program that runs in this process now" — a Pi observer's lease
//! (`agent_pi::transport`), what was proven of that observer
//! (`agent_status`) — also holds a watch of the process and asks it whenever
//! it reads or writes: the kernel's own exec and exit reports for that pid
//! (kqueue `EVFILT_PROC`), taken without a task port and without starting
//! anything. It says "the same instance" or "not", and nothing about which
//! program or whose: it is no code identity and never stands in for one.
//! The same watch counts the kernel's fork reports for the pid: whether the
//! process made a child between two questions, which is what decides whether
//! anything it made in that time can be held by another process as well.

mod code_identity;

use std::collections::HashMap;

pub(crate) use code_identity::ProductIdentity;
use code_identity::{CodeIdentity, OnDisk, Running};

/// Test support of the code-identity checks: stand-in products and signed
/// fixture programs, and what the tests count and inject on their thread.
#[cfg(test)]
pub(crate) use code_identity::fixture as identity_fixture;
#[cfg(test)]
pub(crate) use code_identity::hold as identity_hold;
#[cfg(test)]
pub(crate) use code_identity::{DYNAMIC_CHECKS, STATIC_VALIDATIONS};

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

// ---------- execution instances -----------------------------------------------------
//
// A pid and a birth instant name a PROCESS, not what it executes: `execve`
// and the spawn call's exec-in-place form replace the image and keep both
// (measured on macOS 27.0.1; the kernel's own instance counter, the
// `pidversion` of the process's audit token, changes at that moment). So a
// caller that bound something to "this program, as it runs now" needs a
// third fact: that the process has not executed another image since.
//
// The kernel reports that through kqueue(2): `EVFILT_PROC` with `NOTE_EXEC`
// and `NOTE_EXIT`, registered for a pid. Any process may ask this of any
// other without a task port, a privilege or an entitlement, and it starts
// nothing. It is asked HERE, of ONE queue for the whole application (one
// descriptor, however many processes are watched). What it says is exactly
// "an exec or the exit happened after the watch was taken": a `fork` is not
// an exec (the child is another process, with its own pid and birth), an
// exec in a child is the child's, and a file replaced on disk under a
// running program changes nothing about the image in memory, so none of the
// three is reported as one. Which program a process is, or whose, is NOT
// answered by this and is never inferred from it (`code_identity`).
//
// The same registration also asks for `NOTE_FORK`, and a watch counts those
// reports (`forks`). A fork leaves the watched process what it was; what it
// changes is who ELSE may hold what that process held: a child starts with a
// copy of its parent's memory and of its open descriptors. The kernel posts
// the report before it lets the child run — for `fork` and for the spawn
// call alike (xnu `kern_fork.c`, `kern_exec.c`; measured: 20000 children of
// each kind whose first act was one byte on an inherited connection, and the
// report was there every time that byte had been read). So "no fork was
// counted between two readings of the queue" means that no process came to
// hold a copy of anything the watched process made in between, and a caller
// that needs a connection or a secret to be that process's ALONE asks exactly
// this (`agent_pi::transport`). It is a fact about copies made by forking; a
// descriptor a process hands to another on purpose is that process's own act
// and is not reported by anything here.
//
// Registering a pid that is already watched changes nothing that was
// reported and not yet taken (xnu `filt_proctouch` leaves it; measured), so
// a second watch of a process never costs the first one an exec.

/// What the kernel reported of a watched process since its watch was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Continuity {
    /// Neither: it is still the execution instance that was watched.
    Same,
    /// It executed another image: the same pid and birth instant, and
    /// nothing that was true of the program before is known of this one.
    Executed,
    /// It is gone.
    Exited,
    /// The kernel could not be asked. Nothing is known; a caller fails
    /// closed and never treats this as `Same`.
    Unreadable,
}

const INSTANCE_SAME: u8 = 0;
const INSTANCE_EXECUTED: u8 = 1;
const INSTANCE_EXITED: u8 = 2;
const INSTANCE_UNREADABLE: u8 = 3;

/// What one watch was told so far.
struct InstanceSeen {
    /// One of the `INSTANCE_` words; the first that is not `SAME` stands.
    state: std::sync::atomic::AtomicU8,
    /// How many fork reports were taken for the process.
    forks: std::sync::atomic::AtomicU64,
}

impl InstanceSeen {
    fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            state: std::sync::atomic::AtomicU8::new(INSTANCE_SAME),
            forks: std::sync::atomic::AtomicU64::new(0),
        })
    }
}

/// One process as the execution instance it was when `of` was called. The
/// first thing reported of it stands for as long as the watch lives; a
/// process is never watched again under the same watch.
pub(crate) struct InstanceWatch {
    pid: u32,
    seen: std::sync::Arc<InstanceSeen>,
    /// Whether the kernel's queue holds a registration this watch shares
    /// (a test's stand-in has none).
    registered: bool,
}

/// The one queue, and every watch that is alive, by pid. A leaf lock:
/// nothing else is taken while it is held.
#[cfg(target_os = "macos")]
struct InstanceQueue {
    queue: std::os::fd::OwnedFd,
    watched: HashMap<u32, Vec<std::sync::Arc<InstanceSeen>>>,
}

#[cfg(target_os = "macos")]
static INSTANCES: std::sync::Mutex<Option<InstanceQueue>> = std::sync::Mutex::new(None);

#[cfg(target_os = "macos")]
impl InstanceQueue {
    fn open() -> Option<Self> {
        use std::os::fd::FromRawFd;
        // SAFETY: `kqueue` takes no argument and returns a new descriptor or -1.
        let fd = unsafe { libc::kqueue() };
        if fd < 0 {
            return None;
        }
        // SAFETY: `fd` is a descriptor this call just created and owns.
        let queue = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        // SAFETY: a flag set on a descriptor this function owns.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        Some(Self {
            queue,
            watched: HashMap::new(),
        })
    }

    fn change(&self, pid: u32, flags: u16) -> bool {
        use std::os::fd::AsRawFd;
        let change = libc::kevent {
            ident: pid as libc::uintptr_t,
            filter: libc::EVFILT_PROC,
            flags,
            fflags: libc::NOTE_EXEC | libc::NOTE_EXIT | libc::NOTE_FORK,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: one change is read from valid storage, and no event is
        // asked for, so nothing is written.
        let done = unsafe {
            libc::kevent(
                self.queue.as_raw_fd(),
                &change,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        };
        done == 0
    }

    fn mark(&self, pid: Option<u32>, word: u8) {
        use std::sync::atomic::Ordering;
        let lists = self
            .watched
            .iter()
            .filter(|(watched, _)| pid.is_none_or(|pid| pid == **watched));
        for (_, watches) in lists {
            for seen in watches {
                // the first report stands
                let _ = seen.state.compare_exchange(
                    INSTANCE_SAME,
                    word,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
    }

    /// Take what the kernel has reported so far, without waiting, and mark
    /// the watches it concerns.
    fn drain(&mut self) {
        use std::os::fd::AsRawFd;
        let none = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        loop {
            // SAFETY: zeroed storage for the events the kernel may write.
            let mut events: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
            // SAFETY: no change is read; at most 16 events are written into
            // storage of exactly that size; the zero timeout never waits.
            let got = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    events.len() as libc::c_int,
                    &none,
                )
            };
            if got < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                // the queue cannot be asked: no watch is known to hold
                self.mark(None, INSTANCE_UNREADABLE);
                return;
            }
            for event in &events[..got as usize] {
                let pid = event.ident as u32;
                // one report may stand for several forks: what a caller asks
                // is whether there was any, so each report counts once
                if event.fflags & libc::NOTE_FORK != 0 {
                    for seen in self.watched.get(&pid).into_iter().flatten() {
                        seen.forks.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    }
                }
                // an exec that was followed by the exit is still an exec first
                if event.fflags & libc::NOTE_EXEC != 0 {
                    self.mark(Some(pid), INSTANCE_EXECUTED);
                }
                if event.fflags & libc::NOTE_EXIT != 0 {
                    self.mark(Some(pid), INSTANCE_EXITED);
                    // the kernel dropped that registration with the process:
                    // a later process of this pid is watched afresh
                    self.watched.remove(&pid);
                }
            }
            if (got as usize) < events.len() {
                return;
            }
        }
    }
}

impl InstanceWatch {
    /// Watch `pid` from now on. `None` when the kernel refused — no such
    /// process, or no queue: a caller then has no continuity to rely on.
    #[cfg(target_os = "macos")]
    pub(crate) fn of(pid: u32) -> Option<std::sync::Arc<Self>> {
        use crate::sync::LockRecover;
        let mut instances = INSTANCES.lock_or_recover();
        if instances.is_none() {
            *instances = InstanceQueue::open();
        }
        let queue = instances.as_mut()?;
        // what was reported before this moment belongs to earlier watches
        queue.drain();
        if !queue.change(pid, libc::EV_ADD | libc::EV_CLEAR) {
            return None;
        }
        let seen = InstanceSeen::new();
        queue.watched.entry(pid).or_default().push(seen.clone());
        Some(std::sync::Arc::new(Self {
            pid,
            seen,
            registered: true,
        }))
    }

    #[cfg(not(target_os = "macos"))]
    pub(crate) fn of(_pid: u32) -> Option<std::sync::Arc<Self>> {
        None
    }

    /// Take what the kernel reported until now.
    fn asked(&self) {
        if !self.registered {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            use crate::sync::LockRecover;
            match INSTANCES.lock_or_recover().as_mut() {
                Some(queue) => queue.drain(),
                None => {
                    let _ = self.seen.state.compare_exchange(
                        INSTANCE_SAME,
                        INSTANCE_UNREADABLE,
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    );
                }
            }
        }
    }

    /// Whether the watched process is still the instance it was, asked of
    /// the kernel now. Once it is not, the answer never changes back.
    pub(crate) fn continuity(&self) -> Continuity {
        let word = |state: u8| match state {
            INSTANCE_SAME => Continuity::Same,
            INSTANCE_EXECUTED => Continuity::Executed,
            INSTANCE_EXITED => Continuity::Exited,
            _ => Continuity::Unreadable,
        };
        let known = self.seen.state.load(std::sync::atomic::Ordering::Acquire);
        if known != INSTANCE_SAME {
            return word(known);
        }
        self.asked();
        word(self.seen.state.load(std::sync::atomic::Ordering::Acquire))
    }

    /// How many fork reports the kernel has made of the watched process
    /// since this watch was taken, asked now. A caller keeps the number and
    /// asks again later: the same number means that the process made no
    /// child in between (section header). It only ever grows.
    pub(crate) fn forks(&self) -> u64 {
        self.asked();
        self.seen.forks.load(std::sync::atomic::Ordering::Acquire)
    }

    /// A watch no kernel answers, and the switch that says what happened to
    /// its process: a test describes an exec, an exit, a fork or a failed
    /// read exactly, without starting a program.
    #[cfg(test)]
    pub(crate) fn stand_in() -> (std::sync::Arc<Self>, InstanceStandIn) {
        let seen = InstanceSeen::new();
        (
            std::sync::Arc::new(Self {
                pid: 0,
                seen: seen.clone(),
                registered: false,
            }),
            InstanceStandIn(seen),
        )
    }
}

impl Drop for InstanceWatch {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            use crate::sync::LockRecover;
            let mut instances = INSTANCES.lock_or_recover();
            let Some(queue) = instances.as_mut() else {
                return;
            };
            let Some(watches) = queue.watched.get_mut(&self.pid) else {
                return;
            };
            watches.retain(|seen| !std::sync::Arc::ptr_eq(seen, &self.seen));
            if watches.is_empty() {
                queue.watched.remove(&self.pid);
                // nobody watches this process any more; a registration the
                // kernel already dropped is simply not there
                let _ = queue.change(self.pid, libc::EV_DELETE);
            }
        }
    }
}

/// What a test says happened to the process of a stand-in watch.
#[cfg(test)]
pub(crate) struct InstanceStandIn(std::sync::Arc<InstanceSeen>);

#[cfg(test)]
impl InstanceStandIn {
    fn put(&self, word: u8) {
        self.0
            .state
            .store(word, std::sync::atomic::Ordering::Release);
    }
    pub(crate) fn executed(&self) {
        self.put(INSTANCE_EXECUTED);
    }
    pub(crate) fn exited(&self) {
        self.put(INSTANCE_EXITED);
    }
    pub(crate) fn unreadable(&self) {
        self.put(INSTANCE_UNREADABLE);
    }
    /// The process made a child.
    pub(crate) fn forked(&self) {
        self.0
            .forks
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }
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

/// The launch shapes a client is supported in, stated by its adapter as
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
    /// The resume word may stand alone (a native picker): still a supported
    /// launch, with no id.
    pub(crate) resume_optional: bool,
    /// Flags without a value that MUST be present, each exactly once, for
    /// the launch to be supported at all (a topology the adapter requires).
    pub(crate) required: &'static [&'static str],
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
    // a bare resume word still waits for its id, which may follow the
    // flags as a positional word (`resume --flag <id>`)
    let mut id_pending = false;
    let mut required_seen = vec![false; grammar.required.len()];
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
            if resumed {
                return unsupported;
            }
            // a bare resume word is a picker: supported only where the
            // adapter says so, and the next word must not be its value
            let bare = inline.is_none() && rest.peek().is_none_or(|next| next.starts_with('-'));
            if bare {
                if !grammar.resume_optional {
                    return unsupported;
                }
                id_pending = true;
            } else if !value(&mut rest).is_some_and(|id| (grammar.resume_value)(id)) {
                return unsupported;
            }
            resumed = true;
        } else if let Some(at) = grammar.required.iter().position(|name| *name == flag) {
            if inline.is_some() || required_seen[at] {
                return unsupported;
            }
            required_seen[at] = true;
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
        } else if id_pending && inline.is_none() && (grammar.resume_value)(token) {
            // the bare resume word's id, after the flags
            id_pending = false;
        } else {
            return unsupported;
        }
    }
    if (bypass && mode)
        || required_seen.iter().any(|seen| !seen)
        || !environment_supported(env, home, grammar)
    {
        return unsupported;
    }
    let profile = if bypass {
        LaunchProfile::Bypass
    } else {
        LaunchProfile::Ordinary
    };
    (profile, directories)
}

// How many full content reads this thread started (the cost tests), and how
// long each of this thread's next full reads stalls before its first byte
// (the tests of a read that cannot be interrupted and returns late).
#[cfg(test)]
thread_local! {
    pub(crate) static FULL_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static READ_STALL: std::cell::Cell<Option<std::time::Duration>> = const { std::cell::Cell::new(None) };
}

/// The largest executable the qualification will hash.
const MAX_EXECUTABLE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Why an executable reference could not be made, or no longer holds. The
/// two are different facts and are worded differently for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unavailable {
    /// Positively not the referenced program: the path names no regular
    /// file of its own, the file's state or content is another one, what it
    /// holds is not valid code of the product, or the process does not
    /// execute it.
    Changed,
    /// The read or the validation did not finish — its phase ended, or the
    /// file, the kernel or Security gave no answer. No proof either way.
    Unverified,
}

/// One file state: where the file is, which file it is, and what it
/// contains.
#[derive(Clone, PartialEq, Eq)]
struct FileState {
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

impl std::fmt::Debug for FileState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileState(..)")
    }
}

impl FileState {
    /// The file at `path` if it is a regular file that names itself (no
    /// link on the way), read in full and hashed now.
    fn read(path: &std::path::Path) -> Result<Self, Unavailable> {
        use sha2::Digest;
        use std::io::Read;
        use std::os::unix::fs::MetadataExt;
        let named = path.is_absolute()
            && std::fs::canonicalize(path).is_ok_and(|real| real.as_path() == path);
        if !named {
            return Err(Unavailable::Changed);
        }
        #[cfg(test)]
        {
            FULL_READS.with(|reads| reads.set(reads.get() + 1));
            if let Some(stall) = READ_STALL.with(std::cell::Cell::get) {
                std::thread::sleep(stall);
            }
        }
        let mut file = std::fs::File::open(path).map_err(|_| Unavailable::Changed)?;
        let before = file.metadata().map_err(|_| Unavailable::Unverified)?;
        if !before.is_file() || before.size() > MAX_EXECUTABLE_BYTES {
            return Err(Unavailable::Changed);
        }
        let mut hasher = sha2::Sha256::new();
        let mut buffer = vec![0u8; 1 << 20];
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buffer[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(Unavailable::Unverified),
            }
            // Bounded by size above and by the caller's deadline here: a
            // read that outlives its phase yields no reference at all.
            if crate::session_runtime::check_deadline().is_err() {
                return Err(Unavailable::Unverified);
            }
        }
        let state = Self {
            path: path.to_path_buf(),
            device: before.dev(),
            inode: before.ino(),
            size: before.size(),
            modified: (before.mtime(), before.mtime_nsec()),
            changed: (before.ctime(), before.ctime_nsec()),
            sha256: hasher.finalize().into(),
        };
        // the path must still name the file that was hashed
        if state.at(path) {
            Ok(state)
        } else {
            Err(Unavailable::Changed)
        }
    }

    /// `path` is this state's path and names the very file state that was
    /// hashed: same device and inode, same size, same modification AND
    /// change instants. The content is not read.
    fn at(&self, path: &std::path::Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        path == self.path
            && std::fs::symlink_metadata(path).is_ok_and(|meta| {
                meta.is_file()
                    && (meta.dev(), meta.ino(), meta.size()) == (self.device, self.inode, self.size)
                    && (meta.mtime(), meta.mtime_nsec()) == self.modified
                    && (meta.ctime(), meta.ctime_nsec()) == self.changed
            })
    }
}

/// One real executable file as it was when read: where it is, which file it
/// is, what it contains (SHA-256 of the whole file) and what its signature
/// validated as — the code identity of one product, validated in full
/// (`code_identity::on_disk`). The digest is this transaction's proof that
/// the content is still the same; it is never looked up anywhere, and no
/// digest is "known" or "unknown". The signature says whose code it is and
/// lets a process be bound to it; it is never taken for the content. `Debug`
/// prints none of it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExecutableRef {
    file: FileState,
    code: CodeIdentity,
}

impl std::fmt::Debug for ExecutableRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecutableRef(..)")
    }
}

impl ExecutableRef {
    /// The file at `path` as code of `product`: read and hashed in full,
    /// then its signature validated in full against the product's
    /// requirement, for one and the same file state. Both are uncancellable
    /// once started; the caller's deadline is asked after each, and an
    /// answer that arrived late is discarded, not used.
    pub(crate) fn read(
        path: &std::path::Path,
        product: &ProductIdentity,
    ) -> Result<Self, Unavailable> {
        let file = FileState::read(path)?;
        let code = match code_identity::on_disk(path, product) {
            OnDisk::Valid(code) => code,
            OnDisk::Invalid => return Err(Unavailable::Changed),
            OnDisk::Unreadable => return Err(Unavailable::Unverified),
        };
        if crate::session_runtime::check_deadline().is_err() {
            return Err(Unavailable::Unverified);
        }
        // the signature was validated for the file state that was hashed
        if !file.at(path) {
            return Err(Unavailable::Changed);
        }
        Ok(Self { file, code })
    }

    /// Lowercase hex SHA-256 of the file's content (tests: production
    /// compares a digest only with itself, through `unchanged`).
    #[cfg(test)]
    pub(crate) fn digest_hex(&self) -> String {
        self.file
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The path still names the same file with the same content: the whole
    /// file is read and hashed again. Equal content is the same signed
    /// code, so the signature is not validated a second time.
    pub(crate) fn unchanged(&self) -> Result<(), Unavailable> {
        if FileState::read(&self.file.path)? == self.file {
            Ok(())
        } else {
            Err(Unavailable::Changed)
        }
    }

    /// The path still names the very file state that was hashed: same device
    /// and inode, same size, same modification AND change instants. The
    /// content is not re-read; this is valid only as a follow-up to a full
    /// read of this reference in the same operation, and any write, rename,
    /// replacement or metadata change since then fails it.
    pub(crate) fn same_state(&self) -> bool {
        self.file.at(&self.file.path)
    }

    /// `path` is this reference's path and still names the same file state
    /// (the content is not re-read).
    fn same_file_at(&self, path: &std::path::Path) -> bool {
        self.file.at(path)
    }

    /// The launch target: this exact absolute path, for the launch sink only.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.file.path
    }

    /// The reference of `path` under the product "exactly this file's own
    /// signing identifier" (tests: any signed file can stand in).
    #[cfg(test)]
    pub(crate) fn for_tests(path: &std::path::Path) -> Self {
        Self::read(path, code_identity::fixture::product_of(path)).expect("a signed test file")
    }

    /// The reference of the running test executable, read once per process
    /// (tests that only need SOME real reference).
    #[cfg(test)]
    pub(crate) fn of_this_test() -> Self {
        static OWN: std::sync::OnceLock<ExecutableRef> = std::sync::OnceLock::new();
        OWN.get_or_init(|| {
            Self::for_tests(&std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap())
        })
        .clone()
    }
}

/// Whether one exact process generation executes one referenced file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Binding {
    /// The generation executes exactly the referenced code, and its path
    /// names the referenced file state.
    Bound,
    /// No such process generation (exited, or the pid was reused).
    Gone,
    /// It executes something else, or its path no longer holds the
    /// referenced file state.
    Other,
    /// The kernel or Security gave no answer, or none inside the caller's
    /// deadline: no proof either way.
    Unreadable,
}

/// One dynamic validation, for a caller that owns a deadline.
///
/// The call cannot be interrupted, so three things decide whether its answer
/// exists at all: the calling thread has a deadline scope (a validation
/// nobody bounded is not made — `None`, and nothing was asked), that
/// deadline has not passed when the call would start (`None`, nothing was
/// asked), and it has not passed when the call returned. An answer that
/// arrived late is dropped whatever it was: a late "this is the product" is
/// not a product, and it is not a different program either — the caller
/// hears "no answer". The scope does not shorten the call; who stops waiting
/// for it, and what the stalled thread goes on holding, is each caller's own
/// worker (`session_runtime::bounded_worker`).
fn validated(check: impl FnOnce() -> Running) -> Option<Running> {
    use crate::session_runtime::{check_deadline, deadline_active};
    if !deadline_active() || check_deadline().is_err() {
        return None;
    }
    let answer = check();
    check_deadline().is_ok().then_some(answer)
}

/// Whether the process `pid` born at `birth` executes, right now, exactly
/// the code `executable` refers to, from the file state it refers to.
///
/// Four fences around one running-code fact. The generation (pid and birth
/// instant), the kernel's path of the running image and the file's state at
/// that path are read BEFORE and AFTER; in between Security validates the
/// process dynamically against the reference's own code directory
/// (`code_identity::running`). The generation and the path are what the
/// kernel says of the process; the file state is a disk fact; only the
/// validation in the middle is a fact about the image in memory. It is the
/// one that fails for an old image whose path already holds another validly
/// signed file, and for an exec into another image under the same pid and
/// birth instant. No file content is read.
///
/// The validation is made only under the caller's deadline and its answer
/// counts only when it returned inside it (`validated`); otherwise this is
/// `Unreadable` — never `Bound`, and never `Other` either.
#[cfg(target_os = "macos")]
pub(crate) fn binding(pid: u32, birth: (u64, u32), executable: &ExecutableRef) -> Binding {
    let alive = || process_start(pid) == Some(birth);
    let here = || image_path(pid).is_some_and(|image| executable.same_file_at(&image));
    if !alive() {
        return Binding::Gone;
    }
    if !here() {
        return if alive() {
            Binding::Other
        } else {
            Binding::Gone
        };
    }
    let Some(running) = validated(|| code_identity::running(pid, &executable.code)) else {
        return Binding::Unreadable;
    };
    if !alive() {
        return Binding::Gone;
    }
    if !here() {
        return Binding::Other;
    }
    match running {
        Running::Is => Binding::Bound,
        Running::IsNot | Running::Diverged => Binding::Other,
        Running::Unreadable => Binding::Unreadable,
    }
}

/// Whether the process `pid` born at `birth` is, right now, at the
/// referenced file: the generation exists and the kernel's path of its image
/// names the referenced file state. Kernel reads and one `lstat` — no
/// validation, nothing that can stall. It proves nothing about the image in
/// memory by itself; it is what a caller asks AGAIN after a `binding` whose
/// validation took time, so that the generation and the file are facts of
/// the instant it acts on.
#[cfg(target_os = "macos")]
pub(crate) fn at_executable(pid: u32, birth: (u64, u32), executable: &ExecutableRef) -> bool {
    process_start(pid) == Some(birth)
        && image_path(pid).is_some_and(|image| executable.same_file_at(&image))
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

/// What a process must be to qualify as a client: its adapter's static
/// statements and the home it must have been started under.
pub(crate) struct ClientSpec<'a> {
    /// Whose code it must be. This, validated against the running image, is
    /// the ONLY thing that admits a process: no file name, version, path or
    /// digest does, and none refuses one.
    pub(crate) product: &'a ProductIdentity,
    /// The launcher's own file name. A HINT for wording a refusal — "this
    /// looks like a client that could not be identified" — never a ground
    /// for admitting a process and never one for refusing it.
    pub(crate) launcher: &'a str,
    pub(crate) grammar: &'a LaunchGrammar,
    pub(crate) home: &'a std::path::Path,
}

/// What one exact client process qualifies as.
#[derive(Debug)]
pub(crate) struct ClientQualification {
    /// The real file it runs — read and hashed in full, its signature
    /// validated as the product, and THIS generation bound to it — or why
    /// there is no such reference.
    pub(crate) executable: Result<ExecutableRef, Unavailable>,
    pub(crate) profile: LaunchProfile,
    /// A directory flag was present at launch (an observation, not a list).
    pub(crate) additional_dirs: bool,
    pub(crate) cwd: Option<DirRef>,
}

/// Why a process is not taken for the product.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotProduct {
    /// Its code is another signer's or another product's.
    Other,
    /// It does not run what its path holds now: no file's identity is its own.
    Diverged,
    /// The kernel or Security gave no answer, or none inside the caller's
    /// deadline. Nothing is known about the process.
    Unreadable,
}

/// One exact process, asked whether it is the product and how it started.
#[derive(Debug)]
pub(crate) enum ClientRead {
    /// No such process generation (exited, or the pid was reused).
    Gone,
    /// Its running code was not shown to be the product's; nothing was
    /// hashed. `named`: its image or argv[0] carries the launcher's name —
    /// said only so that a refusal can be worded, and proof of nothing.
    NotProduct {
        named: bool,
        why: NotProduct,
    },
    Product(ClientQualification),
}

fn name_of(path: &std::path::Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// Qualify the process `pid` born at `birth`: is it the product, which file
/// does it run, how was it started and where.
///
/// The order is cost and proof. The cheap question comes first — does the
/// running image, validated dynamically, satisfy the product's requirement
/// (`code_identity::running_product`)? Only then is its file read and hashed
/// in full and its signature validated in full (`ExecutableRef::read`), and
/// only a generation BOUND to that reference (`binding`) keeps it. A
/// reference in `known` that was validated for this very file state, in this
/// same operation, is reused instead of reading the file again; the binding
/// of this generation to it is still checked on its own, every time. The
/// generation is checked before and after; a process that changed in between
/// reads as `Gone`.
///
/// Both dynamic validations here — the first question and the binding — are
/// made under the caller's deadline and count only inside it (`validated`).
/// One that did not answer in time is `NotProduct { why: Unreadable }` or an
/// `Unverified` reference: the process is never taken for the product on a
/// late answer, and nothing is read or hashed after one.
#[cfg(target_os = "macos")]
pub(crate) fn qualify_client(
    pid: u32,
    birth: (u64, u32),
    spec: &ClientSpec<'_>,
    known: &[ExecutableRef],
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
    let named = [name_of(&image), argv_name].contains(&Some(spec.launcher));
    let reused = known
        .iter()
        .find(|known| known.code.is_of(spec.product) && known.same_file_at(&image));
    let executable = match reused {
        Some(known) => Ok(known.clone()),
        None => {
            let not = match validated(|| code_identity::running_product(pid, spec.product)) {
                Some(Running::Is) => None,
                Some(Running::IsNot) => Some(NotProduct::Other),
                Some(Running::Diverged) => Some(NotProduct::Diverged),
                Some(Running::Unreadable) | None => Some(NotProduct::Unreadable),
            };
            if let Some(why) = not {
                return if alive() {
                    ClientRead::NotProduct { named, why }
                } else {
                    ClientRead::Gone
                };
            }
            ExecutableRef::read(&image, spec.product)
        }
    };
    let executable = executable.and_then(|executable| match binding(pid, birth, &executable) {
        Binding::Bound => Ok(executable),
        Binding::Unreadable => Err(Unavailable::Unverified),
        Binding::Other | Binding::Gone => Err(Unavailable::Changed),
    });
    let (profile, additional_dirs) = match (&parsed, spec.home.to_str()) {
        (Some((_, argv, env)), Some(home)) => reduce_launch(argv, env, home, spec.grammar),
        _ => (LaunchProfile::Unknown, false),
    };
    let cwd = working_dir(pid);
    if !alive() || image_path(pid).as_deref() != Some(image.as_path()) {
        return ClientRead::Gone;
    }
    ClientRead::Product(ClientQualification {
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
/// - **image** and **working directory** are read NOW: the code the process
///   executes at this instant, bound to the frozen file by a dynamic
///   validation (`binding`), and the directory it is in at this instant;
/// - **arguments** and **environment** are the launch image the kernel kept
///   from the exec: what the process was started with. They are not its
///   current configuration, and a process can overwrite that area itself.
///
/// So `Matches` means "started by exactly this request, in the expected
/// home, with no refused name, executing the frozen file in the frozen
/// directory when looked at". It does not mean any file the client read
/// afterwards, any setting it loaded, or its effective permission.
///
/// The image is validated twice, before and after the other reads, each
/// under the caller's deadline (`binding`): a validation that did not answer
/// inside it makes the whole answer `Unreadable` — not `Matches`, and not
/// `OtherImage`.
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
    match binding(pid, birth, expect.executable) {
        Binding::Bound => {}
        Binding::Gone => return LaunchCheck::Gone,
        Binding::Other => return LaunchCheck::OtherImage,
        Binding::Unreadable => return LaunchCheck::Unreadable,
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
    // every read above was of this generation, still executing this code
    match binding(pid, birth, expect.executable) {
        Binding::Bound => {}
        Binding::Unreadable => return LaunchCheck::Unreadable,
        Binding::Gone | Binding::Other => return LaunchCheck::Gone,
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

/// Whether the process `pid` born at `birth` executes exactly the code
/// `executable` refers to, from the file state it refers to (`binding`).
/// Anything short of a positive binding is `false`, an answer that came
/// after the caller's deadline included. Kernel reads and one dynamic
/// validation; no file content is read. The validation cannot be
/// interrupted: a caller that must stay responsive asks from a worker it
/// does not wait for (the lifecycle receipts do).
#[cfg(target_os = "macos")]
pub(crate) fn runs_executable(pid: u32, birth: (u64, u32), executable: &ExecutableRef) -> bool {
    binding(pid, birth, executable) == Binding::Bound
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
        resume_optional: false,
        required: &[],
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

    /// A grammar with a required flag and a bare resume word (Codex's
    /// shape): the flag must be there exactly once and without a value; the
    /// resume word stands alone or carries one id of the native shape.
    #[test]
    fn a_required_flag_and_a_bare_resume_word_classify_as_the_grammar_says() {
        use LaunchProfile::*;
        const CODEX_LIKE: LaunchGrammar = LaunchGrammar {
            bypass: "--yolo",
            resume: "resume",
            resume_optional: true,
            required: &["--no-daemon"],
            mode: "--sandbox",
            ordinary_modes: &["workspace-write", "read-only"],
            dropped: &["--model", "-m"],
            directories: "--add-dir",
            ..GRAMMAR
        };
        let reduced = |args: &[&str]| {
            let mut argv = vec!["codex"];
            argv.extend_from_slice(args);
            reduce_launch(&argv, HOME, "/Users/u", &CODEX_LIKE)
        };
        assert_eq!(reduced(&["--no-daemon"]), (Ordinary, false));
        assert_eq!(reduced(&["--no-daemon", "--yolo"]), (Bypass, false));
        assert_eq!(
            reduced(&["--yolo", "--no-daemon", "resume"]),
            (Bypass, false)
        );
        assert_eq!(reduced(&["--no-daemon", "resume", UUID]), (Ordinary, false));
        assert_eq!(
            reduced(&["resume", "--no-daemon", "--yolo", UUID]),
            (Bypass, false)
        );
        assert_eq!(
            reduced(&["resume", "--no-daemon", "-m", "gpt-5", UUID]),
            (Ordinary, false)
        );
        assert_eq!(
            reduced(&["--no-daemon", "-m", "gpt-5", "--add-dir", "/a"]),
            (Ordinary, true)
        );
        assert_eq!(
            reduced(&["--no-daemon", "--sandbox", "read-only", "resume"]),
            (Ordinary, false)
        );
        for unsupported in [
            vec![],
            vec!["--yolo"],
            vec!["resume", UUID],
            vec!["--no-daemon", "--no-daemon"],
            vec!["--no-daemon=1"],
            vec!["--no-daemon", "resume", "latest"],
            vec!["--no-daemon", "resume", "--last"],
            vec!["--no-daemon", "resume", UUID, "resume"],
            vec!["resume", "--no-daemon", UUID, UUID],
            vec!["--no-daemon", UUID],
            vec!["--no-daemon", "--sandbox", "danger-full-access"],
            vec!["--no-daemon", "--yolo", "--sandbox", "read-only"],
            vec!["--no-daemon", "--ask-for-approval", "never"],
            vec!["--no-daemon", "--dangerously-bypass-hook-trust"],
            vec!["--no-daemon", "fix the bug"],
            vec!["--no-daemon", "exec", "hi"],
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
        let executable = ExecutableRef::for_tests(&exe);
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
        // these checks ask for a deadline owner; the deadline is not the subject
        let _phase = crate::session_runtime::test_phase();
        let me = std::process::id();
        let birth = process_start(me).expect("own birth");
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let executable = ExecutableRef::for_tests(&exe);
        let other = ExecutableRef::for_tests(&std::fs::canonicalize("/bin/ls").unwrap());
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

    /// A file state follows no link, notices a replaced file and is one
    /// content: the half of an executable reference that is about the disk.
    #[test]
    fn a_file_state_is_one_real_file_and_notices_any_change() {
        let dir = std::env::temp_dir().join(format!("deck-exe-ref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let real = dir.join("2.1.294");
        std::fs::write(&real, b"first build").unwrap();
        let link = dir.join("claude");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let state = FileState::read(&real).expect("a regular file");
        let same = |state: &FileState| FileState::read(&state.path).as_ref() == Ok(state);
        // SHA-256 of the bytes just written
        assert_eq!(state.sha256.as_slice(), {
            use sha2::Digest;
            sha2::Sha256::digest(b"first build").as_slice().to_vec()
        });
        assert!(same(&state) && state.at(&real));
        // a launcher link is never the file, and a relative path never
        for other in [link.as_path(), std::path::Path::new("2.1.294"), &dir] {
            assert_eq!(FileState::read(other).err(), Some(Unavailable::Changed));
        }
        assert!(!state.at(&link));
        assert_eq!(format!("{:?}", DirRef(dir.clone())), "DirRef(..)");
        // the link is moved to another release: the frozen file is unchanged
        let other = dir.join("2.1.295");
        std::fs::write(&other, b"second build").unwrap();
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&other, &link).unwrap();
        assert!(same(&state));
        // the same version rebuilt in place: another file, another content
        std::fs::write(dir.join("staged"), b"other bytes").unwrap();
        std::fs::rename(dir.join("staged"), &real).unwrap();
        assert!(!same(&state) && !state.at(&real));
        std::fs::remove_file(&real).unwrap();
        assert_eq!(FileState::read(&real).err(), Some(Unavailable::Changed));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A rewrite of the same size that puts the modification time back is
    /// still another file state (the change instant cannot be set back) and
    /// another content; a deadline that has passed yields no state, and that
    /// is "not finished", never "changed".
    #[test]
    fn a_same_size_rewrite_and_an_expired_deadline_never_pass_as_the_file_state() {
        let dir = std::env::temp_dir().join(format!("deck-exe-same-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(&dir).unwrap();
        let file = dir.join("2.1.294");
        std::fs::write(&file, b"build-AAAA").unwrap();
        let state = FileState::read(&file).expect("a regular file");
        assert!(state.at(&file));
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
        assert!(!state.at(&file), "the cheap check sees the rewrite");
        // a fresh state of the rewritten file is another state
        let rewritten = FileState::read(&file).expect("still a regular file");
        assert_ne!(rewritten.sha256, state.sha256);
        assert!(rewritten != state);
        // a read under an expired deadline gives nothing, never a partial hash
        {
            let _expired = crate::session_runtime::Deadline::until(std::time::Instant::now());
            assert_eq!(FileState::read(&file).err(), Some(Unavailable::Unverified));
            // the cheap state check reads no content and is not a hash
            assert!(rewritten.at(&file));
        }
        assert_eq!(FileState::read(&file).as_ref(), Ok(&rewritten));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    use super::identity_fixture::{self as fixture, Process};

    /// An executable reference is a file state AND the product's validated
    /// code: text, another product's code, a link and a missing file make
    /// none, and a reference notices a change of content the way its file
    /// state does. Nothing about it is printed.
    #[test]
    fn an_executable_reference_is_valid_product_code_in_one_file_state() {
        let dir = fixture::directory("reference");
        let product = fixture::product(fixture::IDENTIFIER);
        let program = dir.join("2.1.300");
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        let reference = ExecutableRef::read(&program, product).expect("valid product code");
        assert_eq!(reference.path(), program);
        assert_eq!(reference.digest_hex().len(), 64);
        assert!(reference.same_state());
        assert_eq!(reference.unchanged(), Ok(()));
        assert_eq!(format!("{reference:?}"), "ExecutableRef(..)");
        // a file that is not code of the product is no reference, whatever it is named
        std::fs::write(dir.join("claude"), b"#!/bin/sh\n").unwrap();
        let foreign = dir.join("foreign");
        fixture::build(&foreign, "build-A", &[], fixture::OTHER_IDENTIFIER);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&program, &link).unwrap();
        for other in [dir.join("claude"), foreign, link, dir.join("missing")] {
            assert_eq!(
                ExecutableRef::read(&other, product).err(),
                Some(Unavailable::Changed),
                "{other:?}"
            );
        }
        // the same product rebuilt at the same path: another reference
        let staged = dir.join("staged");
        fixture::build(&staged, "build-B", &[], fixture::IDENTIFIER);
        std::fs::rename(&staged, &program).unwrap();
        assert!(!reference.same_state());
        assert_eq!(reference.unchanged(), Err(Unavailable::Changed));
        let rebuilt = ExecutableRef::read(&program, product).expect("the new build");
        assert!(rebuilt != reference);
        assert_ne!(rebuilt.digest_hex(), reference.digest_hex());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A read or a validation that outlives its phase yields no reference,
    /// and that is "not finished" — never "changed". A validation that
    /// answers late is discarded even when its answer was "valid".
    #[test]
    fn a_reference_that_is_not_finished_inside_its_phase_is_unverified_not_changed() {
        use super::code_identity::{Fault, FAULT, STATIC_VALIDATIONS};
        let dir = fixture::directory("deadline");
        let product = fixture::product(fixture::IDENTIFIER);
        let program = dir.join("client");
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        let reference = ExecutableRef::read(&program, product).expect("valid product code");
        {
            // the phase ended before the read
            let _expired = crate::session_runtime::Deadline::until(std::time::Instant::now());
            assert_eq!(
                ExecutableRef::read(&program, product).err(),
                Some(Unavailable::Unverified)
            );
            assert_eq!(reference.unchanged(), Err(Unavailable::Unverified));
            // the cheap state check reads no content and is not a hash
            assert!(reference.same_state());
        }
        {
            // the read fits, the uncancellable validation returns after the end
            let validated = STATIC_VALIDATIONS.with(std::cell::Cell::get);
            let end = std::time::Instant::now() + std::time::Duration::from_millis(150);
            let _phase = crate::session_runtime::Deadline::until(end);
            FAULT.with(|fault| fault.set(Some(Fault::Slow(std::time::Duration::from_millis(400)))));
            let late = ExecutableRef::read(&program, product);
            FAULT.with(|fault| fault.set(None));
            assert!(
                std::time::Instant::now() >= end,
                "the validation ran past the end"
            );
            assert_eq!(STATIC_VALIDATIONS.with(std::cell::Cell::get), validated + 1);
            assert_eq!(late.err(), Some(Unavailable::Unverified));
        }
        // Security answers nothing: unverified as well
        FAULT.with(|fault| fault.set(Some(Fault::Unreadable)));
        let unanswered = ExecutableRef::read(&program, product);
        FAULT.with(|fault| fault.set(None));
        assert_eq!(unanswered.err(), Some(Unavailable::Unverified));
        assert_eq!(reference.unchanged(), Ok(()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The binding of ONE generation to ONE reference: its pid and birth
    /// instant, the kernel's path of its image, that file's state and the
    /// code it executes. Each can be the one that does not hold.
    #[test]
    fn a_binding_is_one_generation_one_path_one_file_state_and_one_running_code() {
        // these checks ask for a deadline owner; the deadline is not the subject
        let _phase = crate::session_runtime::test_phase();
        let dir = fixture::directory("binding");
        let product = fixture::product(fixture::IDENTIFIER);
        let program = dir.join("client");
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        let reference = ExecutableRef::read(&program, product).expect("valid product code");
        let (mut process, _) = Process::start(&program, None).expect("the fixture runs");
        let pid = process.pid();
        let birth = process_start(pid).expect("its birth instant");
        assert_eq!(binding(pid, birth, &reference), Binding::Bound);
        assert!(runs_executable(pid, birth, &reference));
        // another generation under the same pid: what a reused pid looks like
        assert_eq!(
            binding(pid, (birth.0 + 1, birth.1), &reference),
            Binding::Gone
        );
        // the same content at another path is another file
        let copy = dir.join("copy");
        std::fs::copy(&program, &copy).unwrap();
        let elsewhere = ExecutableRef::read(&copy, product).expect("the copy validates");
        assert_eq!(elsewhere.digest_hex(), reference.digest_hex());
        assert_eq!(binding(pid, birth, &elsewhere), Binding::Other);
        // started through a link, the kernel still names the real file
        let link = dir.join("link");
        std::os::unix::fs::symlink(&program, &link).unwrap();
        let (linked, _) = Process::start(&link, None).expect("the link runs");
        let linked_birth = process_start(linked.pid()).unwrap();
        assert_eq!(
            binding(linked.pid(), linked_birth, &reference),
            Binding::Bound
        );
        drop(linked);
        // Security answers nothing: no binding, and not "another image"
        super::code_identity::FAULT
            .with(|fault| fault.set(Some(super::code_identity::Fault::Unreadable)));
        let unanswered = binding(pid, birth, &reference);
        super::code_identity::FAULT.with(|fault| fault.set(None));
        assert_eq!(unanswered, Binding::Unreadable);
        // an exec into another signed image, under the same pid and birth
        let next = dir.join("next");
        fixture::build(&next, "build-B", &[], fixture::IDENTIFIER);
        let next_reference = ExecutableRef::read(&next, product).expect("the next build");
        assert_eq!(process.exec(&next).as_deref(), Some("build-B"));
        let now = process_start(pid).expect("still that pid");
        assert_eq!(binding(pid, now, &reference), Binding::Other);
        assert_eq!(binding(pid, now, &next_reference), Binding::Bound);
        assert!(!runs_executable(pid, birth, &reference));
        // gone: its generation names nothing
        drop(process);
        assert_eq!(binding(pid, now, &next_reference), Binding::Gone);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A dynamic validation cannot be interrupted, so it is made only for a
    /// caller that owns a deadline. Without one, or with one that has
    /// already passed, nothing is asked of Security and the answer is "no
    /// answer": never a binding, and never another image.
    #[test]
    fn a_dynamic_validation_is_made_only_for_a_caller_that_owns_a_deadline() {
        use super::code_identity::DYNAMIC_CHECKS;
        let dir = fixture::directory("owner");
        let product = fixture::product(fixture::IDENTIFIER);
        let program = dir.join("client");
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        let reference = ExecutableRef::read(&program, product).expect("valid product code");
        let (process, _) = Process::start(&program, None).expect("the fixture runs");
        let pid = process.pid();
        let birth = process_start(pid).expect("its birth instant");
        let spec = ClientSpec {
            product,
            launcher: "client",
            grammar: &GRAMMAR,
            home: std::path::Path::new("/nonexistent"),
        };
        let asked = || DYNAMIC_CHECKS.with(std::cell::Cell::get);
        let unowned = |label: &str| {
            let before = asked();
            assert_eq!(
                binding(pid, birth, &reference),
                Binding::Unreadable,
                "{label}"
            );
            assert!(!runs_executable(pid, birth, &reference), "{label}");
            assert!(
                matches!(
                    qualify_client(pid, birth, &spec, &[]),
                    ClientRead::NotProduct {
                        why: NotProduct::Unreadable,
                        ..
                    }
                ),
                "{label}"
            );
            assert!(
                matches!(
                    qualify_client(pid, birth, &spec, std::slice::from_ref(&reference)),
                    ClientRead::Product(ClientQualification {
                        executable: Err(Unavailable::Unverified),
                        ..
                    })
                ),
                "{label}"
            );
            assert_eq!(asked(), before, "{label}: nothing was asked of Security");
        };
        assert!(!crate::session_runtime::deadline_active());
        unowned("no deadline");
        {
            let _expired = crate::session_runtime::Deadline::until(std::time::Instant::now());
            unowned("an expired deadline");
        }
        // the kernel-only question needs no owner, and is not a binding
        assert!(at_executable(pid, birth, &reference));
        assert!(!at_executable(pid, (birth.0 + 1, birth.1), &reference));
        let _phase = crate::session_runtime::test_phase();
        let before = asked();
        assert_eq!(binding(pid, birth, &reference), Binding::Bound);
        assert_eq!(asked(), before + 1, "one validation for one binding");
        drop(process);
        assert!(!at_executable(pid, birth, &reference), "gone");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The two running-code questions are asked nowhere but through
    /// `validated`: a call added beside it would be a validation whose
    /// answer nobody bounds.
    #[test]
    fn every_dynamic_validation_is_asked_through_its_deadline_owner() {
        let source = include_str!("procinfo.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests {")
            .next()
            .expect("the production part");
        for question in ["code_identity::running(", "code_identity::running_product("] {
            let asked = production.matches(question).count();
            let owned = production
                .matches(&format!("validated(|| {question}"))
                .count();
            assert!(
                asked == 1 && owned == 1,
                "{question}: asked {asked} time(s), {owned} through `validated`"
            );
        }
    }

    /// The validation answers "yes, exactly this code" — after the caller's
    /// deadline. A late yes is no answer: no binding, no product, no match;
    /// and it is not "another image" or "changed" either. Nothing is read or
    /// hashed after it, and nothing is asked a second time.
    #[test]
    fn a_validation_that_answers_after_its_deadline_is_no_answer_and_not_another_image() {
        use super::code_identity::{hold, DYNAMIC_CHECKS};
        use std::time::{Duration, Instant};
        let dir = fixture::directory("late");
        let product = fixture::product(fixture::IDENTIFIER);
        let program = dir.join("client");
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        let reference = ExecutableRef::read(&program, product).expect("valid product code");
        let (process, _) = Process::start(&program, None).expect("the fixture runs");
        let pid = process.pid();
        let birth = process_start(pid).expect("its birth instant");
        let cwd = working_dir(pid).expect("its directory");
        let spec = ClientSpec {
            product,
            launcher: "client",
            grammar: &GRAMMAR,
            home: std::path::Path::new("/nonexistent"),
        };
        // every validation of this process, on this thread, answers
        // truthfully and returns only when the thread's deadline has passed
        let me = std::thread::current().id();
        let held = hold::install(
            move |target| target == pid && std::thread::current().id() == me,
            hold::Until::DeadlinePassed,
        );
        let asked = || DYNAMIC_CHECKS.with(std::cell::Cell::get);
        let late = |what: &str, work: &dyn Fn()| {
            let (before, entered) = (asked(), held.entered());
            let end = Instant::now() + Duration::from_millis(120);
            let _phase = crate::session_runtime::Deadline::until(end);
            work();
            assert!(Instant::now() >= end, "{what}: waited for past the end");
            assert_eq!(
                (asked() - before, held.entered() - entered),
                (1, 1),
                "{what}: one validation, answered once, never asked again"
            );
        };
        late("binding", &|| {
            assert_eq!(binding(pid, birth, &reference), Binding::Unreadable)
        });
        late("runs", &|| {
            assert!(!runs_executable(pid, birth, &reference))
        });
        // the first question of a qualification: nothing is read after it
        let reads = FULL_READS.with(std::cell::Cell::get);
        late("product", &|| {
            assert!(matches!(
                qualify_client(pid, birth, &spec, &[]),
                ClientRead::NotProduct {
                    why: NotProduct::Unreadable,
                    ..
                }
            ))
        });
        assert_eq!(FULL_READS.with(std::cell::Cell::get), reads);
        // a reference that is reused: its binding is unverified, not changed
        late("reused", &|| {
            assert!(matches!(
                qualify_client(pid, birth, &spec, std::slice::from_ref(&reference)),
                ClientRead::Product(ClientQualification {
                    executable: Err(Unavailable::Unverified),
                    ..
                })
            ))
        });
        // the started client's check: neither a match nor another image
        let words: Vec<String> = Vec::new();
        late("launch", &|| {
            assert_eq!(
                check_launch(
                    pid,
                    birth,
                    &LaunchExpectation {
                        executable: &reference,
                        words: &words,
                        home: std::path::Path::new("/nonexistent"),
                        grammar: &GRAMMAR,
                        cwd: &cwd,
                    },
                ),
                LaunchCheck::Unreadable
            )
        });
        // in time, the same questions are answered
        drop(held);
        let _phase = crate::session_runtime::test_phase();
        assert_eq!(binding(pid, birth, &reference), Binding::Bound);
        assert!(matches!(
            qualify_client(pid, birth, &spec, std::slice::from_ref(&reference)),
            ClientRead::Product(ClientQualification {
                executable: Ok(_),
                ..
            })
        ));
        drop(process);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The old process still runs build A; its path already holds build B,
    /// another validly signed file of the same product. The reference made
    /// before the replacement no longer names the file at that path, and the
    /// one made after it names a file the process does not execute.
    #[test]
    fn an_old_process_is_bound_to_neither_reference_of_a_replaced_path() {
        // these checks ask for a deadline owner; the deadline is not the subject
        let _phase = crate::session_runtime::test_phase();
        let dir = fixture::directory("replaced-path");
        let product = fixture::product(fixture::IDENTIFIER);
        let program = dir.join("2.1.300");
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        let before = ExecutableRef::read(&program, product).expect("build A");
        let (process, _) = Process::start(&program, None).expect("the fixture runs");
        let pid = process.pid();
        let birth = process_start(pid).unwrap();
        assert_eq!(binding(pid, birth, &before), Binding::Bound);
        let staged = dir.join("staged");
        fixture::build(&staged, "build-B", &[], fixture::IDENTIFIER);
        std::fs::rename(&staged, &program).unwrap();
        // the file alone is valid product code, and the kernel still reports
        // the old process at this very path
        let after = ExecutableRef::read(&program, product).expect("build B validates");
        assert_eq!(image_path(pid).as_deref(), Some(program.as_path()));
        assert_eq!(binding(pid, birth, &before), Binding::Other);
        assert_eq!(binding(pid, birth, &after), Binding::Other);
        // and the qualification of that process finds no reference for it
        let spec = ClientSpec {
            product,
            launcher: "claude",
            grammar: &GRAMMAR,
            home: std::path::Path::new("/nonexistent"),
        };
        assert!(matches!(
            qualify_client(pid, birth, &spec, &[]),
            ClientRead::NotProduct {
                named: false,
                why: NotProduct::Diverged
            }
        ));
        // a reference handed in as known is not a way around the binding
        assert!(matches!(
            qualify_client(pid, birth, &spec, std::slice::from_ref(&after)),
            ClientRead::Product(ClientQualification {
                executable: Err(Unavailable::Changed),
                ..
            })
        ));
        drop(process);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Read-only cost of qualifying one real executable, against the
    /// unchanged production budgets: the whole-file digest, the full
    /// signature validation, and a complete reference followed by the post
    /// phase's full re-read under ONE post-phase deadline. Names no client
    /// and starts none; with `DECK_REENTRY_PROCESS_PROBE` it also times the
    /// dynamic binding of that one already running process. Every measure
    /// is printed before anything is judged.
    #[test]
    #[ignore = "requires an explicitly supplied executable; no client is started"]
    fn executable_qualification_cost_fits_the_production_budgets() {
        // these checks ask for a deadline owner; the deadline is not the subject
        let _phase = crate::session_runtime::test_phase();
        use super::code_identity::{on_disk, OnDisk};
        let path = std::path::PathBuf::from(
            std::env::var_os("DECK_REENTRY_EXECUTABLE_PROBE")
                .expect("explicit read-only executable probe"),
        );
        let product = fixture::product_of(&path);
        let timed = |work: &mut dyn FnMut()| {
            let started = std::time::Instant::now();
            work();
            started.elapsed()
        };
        let mut state = None;
        let digest = timed(&mut || state = FileState::read(&path).ok());
        let mut valid = false;
        let signature = timed(&mut || valid = matches!(on_disk(&path, product), OnDisk::Valid(_)));
        println!(
            "COST bytes={} digest_ms={} digest_complete={} signature_ms={} signature_valid={valid}",
            std::fs::metadata(&path).unwrap().len(),
            digest.as_millis(),
            state.is_some(),
            signature.as_millis(),
        );
        // what one post phase asks of one distinct binary, under its deadline
        let started = std::time::Instant::now();
        let (made, reread) = {
            let _deadline = crate::session_runtime::Deadline::until(
                started + crate::agent_reentry::POST_BUDGET,
            );
            let made = ExecutableRef::read(&path, product);
            let reread = made.as_ref().ok().map(ExecutableRef::unchanged);
            (made.map(|_| ()), reread)
        };
        let total = started.elapsed();
        println!(
            "COST reference_and_reread_ms={} reference={made:?} reread={reread:?} post_budget_ms={}",
            total.as_millis(),
            crate::agent_reentry::POST_BUDGET.as_millis()
        );
        let bound = std::env::var("DECK_REENTRY_PROCESS_PROBE")
            .ok()
            .and_then(|pid| pid.parse::<u32>().ok())
            .map(|pid| {
                let reference = ExecutableRef::read(&path, product).expect("a reference");
                let birth = process_start(pid).expect("the named process exists");
                let mut answers = Vec::new();
                let first = timed(&mut || answers.push(binding(pid, birth, &reference)));
                let rest = timed(&mut || {
                    for _ in 0..20 {
                        answers.push(binding(pid, birth, &reference));
                    }
                });
                println!(
                    "COST binding_first_us={} binding_next20_us={} bound={}/{}",
                    first.as_micros(),
                    rest.as_micros(),
                    answers.iter().filter(|a| **a == Binding::Bound).count(),
                    answers.len()
                );
                answers.iter().all(|answer| *answer == Binding::Bound)
            });
        assert!(state.is_some() && valid, "the file reads and validates");
        assert_eq!(made, Ok(()), "a complete reference");
        assert_eq!(
            reread,
            Some(Ok(())),
            "the full re-read fits the same deadline"
        );
        assert!(total < crate::agent_reentry::POST_BUDGET);
        assert_ne!(bound, Some(false), "the named process is bound to the file");
    }

    /// This very process, qualified: its generation, file and working
    /// directory are read; another birth instant is another process. Its
    /// name admits nothing and refuses nothing: the product's requirement
    /// decides, under whatever name the file has.
    #[test]
    fn qualifying_reads_one_exact_generation_and_nothing_by_name_alone() {
        // these checks ask for a deadline owner; the deadline is not the subject
        let _phase = crate::session_runtime::test_phase();
        let me = std::process::id();
        let birth = process_start(me).expect("own birth");
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap().to_string();
        let own = fixture::product_of(&exe);
        let spec = |product, launcher| ClientSpec {
            product,
            launcher,
            grammar: &GRAMMAR,
            home: &home,
        };
        let names = name.as_str();
        assert!(matches!(
            qualify_client(me, (birth.0 + 1, birth.1), &spec(own, names), &[]),
            ClientRead::Gone
        ));
        // carrying the launcher's name is not being the product, and nothing
        // is hashed to find that out; the name is reported as a hint only
        let reads = FULL_READS.with(std::cell::Cell::get);
        let other = fixture::product(fixture::IDENTIFIER);
        for (launcher, named) in [(names, true), ("claude", false)] {
            assert!(matches!(
                qualify_client(me, birth, &spec(other, launcher), &[]),
                ClientRead::NotProduct { named: hint, why: NotProduct::Other } if hint == named
            ));
        }
        assert_eq!(FULL_READS.with(std::cell::Cell::get), reads);
        // being the product under ANOTHER name than the launcher's: admitted
        let ClientRead::Product(unnamed) = qualify_client(me, birth, &spec(own, "claude"), &[])
        else {
            panic!("the product, whatever its file is called");
        };
        assert!(unnamed.executable.is_ok());
        let reads = FULL_READS.with(std::cell::Cell::get);
        let ClientRead::Product(read) = qualify_client(me, birth, &spec(own, names), &[]) else {
            panic!("the product by its own signature");
        };
        assert_eq!(FULL_READS.with(std::cell::Cell::get), reads + 1);
        let executable = read.executable.expect("own executable is bound");
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
        // a reference validated for this very file state is reused: no second
        // read of the content, and the binding is still asked for
        let checks = super::code_identity::DYNAMIC_CHECKS.with(std::cell::Cell::get);
        let known = std::slice::from_ref(&executable);
        let ClientRead::Product(again) = qualify_client(me, birth, &spec(own, names), known) else {
            panic!("qualified again");
        };
        assert_eq!(again.executable.as_ref(), Ok(&executable));
        assert_eq!(FULL_READS.with(std::cell::Cell::get), reads + 1);
        assert_eq!(
            super::code_identity::DYNAMIC_CHECKS.with(std::cell::Cell::get),
            checks + 1
        );
        // a reference of another product is not reused for this one
        let foreign = [ExecutableRef::for_tests(
            &std::fs::canonicalize("/bin/ls").unwrap(),
        )];
        let reads = FULL_READS.with(std::cell::Cell::get);
        let ClientRead::Product(fresh) = qualify_client(me, birth, &spec(own, names), &foreign)
        else {
            panic!("qualified with a foreign reference at hand");
        };
        assert_eq!(fresh.executable.as_ref(), Ok(&executable));
        assert_eq!(FULL_READS.with(std::cell::Cell::get), reads + 1);
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

    // ---------- execution instances: the kernel's own reports ---------------------

    /// A real child that does one step each time it is sent a line and says
    /// one word when the step is done.
    struct Stepper {
        child: std::process::Child,
        steps: std::process::ChildStdin,
        said: std::io::BufReader<std::process::ChildStdout>,
    }

    impl Stepper {
        fn start(program: &std::path::Path, script: &str) -> Self {
            let mut child = std::process::Command::new(program)
                .args(["-c", script])
                .env_clear()
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .expect("a child");
            let steps = child.stdin.take().unwrap();
            let said = std::io::BufReader::new(child.stdout.take().unwrap());
            Self { child, steps, said }
        }

        fn step(&mut self) {
            use std::io::Write;
            self.steps.write_all(b"\n").expect("the step is asked for");
        }

        fn word(&mut self) -> String {
            use std::io::BufRead;
            let mut line = String::new();
            self.said.read_line(&mut line).expect("the child's word");
            line.trim().to_string()
        }

        fn end(mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn until_instance(what: &str, done: impl Fn() -> bool) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(
                std::time::Instant::now() < end,
                "timed out waiting for {what}"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// The fact everything bound to "this program, as it runs now" rests on,
    /// asked of the real kernel with real processes. An exec inside one pid
    /// keeps the pid AND the birth instant and is reported; the program's
    /// file being replaced on disk is not an exec and is not reported; the
    /// exit is its own word.
    #[test]
    fn an_exec_inside_one_pid_ends_the_instance_and_a_replaced_file_does_not() {
        let dir = fixture::directory("instance");
        let (program, next) = (dir.join("peer"), dir.join("next"));
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        fixture::build(&next, "build-B", &[], fixture::IDENTIFIER);
        let (mut process, tag) = Process::start(&program, None).expect("the fixture runs");
        assert_eq!(tag, "build-A");
        let pid = process.pid();
        let birth = process_start(pid).expect("a birth instant");
        let watch = InstanceWatch::of(pid).expect("a watch of a live process");
        assert_eq!(watch.continuity(), Continuity::Same);

        // the file at the program's path is another file now
        let staged = dir.join("staged");
        fixture::build(&staged, "build-C", &[], fixture::IDENTIFIER);
        std::fs::rename(&staged, &program).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(
            watch.continuity(),
            Continuity::Same,
            "a file replaced on disk changes nothing that runs"
        );

        // a second watch of the same process, taken before its exec
        let second = InstanceWatch::of(pid).expect("a second watch");
        // the exec: the same pid, the same birth instant, another image
        assert_eq!(process.exec(&next).as_deref(), Some("build-B"));
        assert_eq!(watch.continuity(), Continuity::Executed);
        assert_eq!(second.continuity(), Continuity::Executed);
        assert_eq!(
            process_start(pid),
            Some(birth),
            "pid and birth instant survive an exec: they do not tell it"
        );
        // a watch taken now watches the program that runs now ...
        let after = InstanceWatch::of(pid).expect("a watch of the new image");
        assert_eq!(after.continuity(), Continuity::Same);
        assert_eq!(watch.continuity(), Continuity::Executed, "and stays said");
        // ... and a second exec, of the SAME file, is an exec again
        assert_eq!(process.exec(&next).as_deref(), Some("build-B"));
        assert_eq!(after.continuity(), Continuity::Executed);

        // the exit is another word, and never replaces the first report
        let last = InstanceWatch::of(pid).expect("a watch before the exit");
        drop(process);
        until_instance("the exit", || last.continuity() == Continuity::Exited);
        assert_eq!(watch.continuity(), Continuity::Executed);
        assert_eq!(after.continuity(), Continuity::Executed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second watch of a process that is already watched costs the first
    /// nothing, whenever it is taken — before the exec, after it, or WHILE
    /// the process executes. Registering a pid again leaves what the kernel
    /// reported and nobody took yet (xnu `filt_proctouch`), so an exec that
    /// falls into another watch's registration is still the first watch's
    /// report. Here one thread registers watches of a process as fast as it
    /// can while the process executes another image, again and again: no
    /// exec is ever missed by the watch that was there before it, and a
    /// watch taken after the exec was reported is of the new image.
    #[test]
    fn a_watch_taken_while_a_process_executes_never_costs_an_earlier_one_its_report() {
        let dir = fixture::directory("rewatch");
        let (program, next) = (dir.join("peer"), dir.join("next"));
        fixture::build(&program, "build-A", &[], fixture::IDENTIFIER);
        fixture::build(&next, "build-B", &[], fixture::IDENTIFIER);
        let (mut process, _) = Process::start(&program, None).expect("the fixture runs");
        let pid = process.pid();
        let (mut taken, mut reported_to_late_ones) = (0u64, 0u64);
        for round in 0..60 {
            let first = InstanceWatch::of(pid).expect("a watch before the exec");
            assert_eq!(first.continuity(), Continuity::Same, "round {round}");
            let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let registering = std::thread::spawn({
                let busy = busy.clone();
                move || {
                    let mut watches = Vec::new();
                    while busy.load(std::sync::atomic::Ordering::Acquire) {
                        watches.push(InstanceWatch::of(pid).expect("a watch meanwhile"));
                    }
                    watches
                }
            });
            // let the other thread get going, then execute another image
            std::thread::sleep(std::time::Duration::from_millis(1));
            assert_eq!(process.exec(&next).as_deref(), Some("build-B"));
            busy.store(false, std::sync::atomic::Ordering::Release);
            let meanwhile = registering.join().unwrap();
            assert_eq!(
                first.continuity(),
                Continuity::Executed,
                "round {round}: the exec was reported to the watch that was there"
            );
            // each of the others was taken before, during or after the exec:
            // it says "executed" or "the same", and never anything else
            for watch in &meanwhile {
                let said = watch.continuity();
                assert!(
                    matches!(said, Continuity::Same | Continuity::Executed),
                    "round {round}: {said:?}"
                );
                reported_to_late_ones += u64::from(said == Continuity::Executed);
            }
            taken += meanwhile.len() as u64;
            // and one taken now is of the image that runs now
            let after = InstanceWatch::of(pid).expect("a watch after the exec");
            assert_eq!(after.continuity(), Continuity::Same, "round {round}");
        }
        assert!(taken > 60, "{taken} watches were taken while it executed");
        assert!(
            reported_to_late_ones > 0,
            "some of them were there before an exec: {reported_to_late_ones} of {taken}"
        );
        drop(process);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A fork is not an exec, and neither is an exec in a child: the
    /// process that was watched is still the program it was. What a fork
    /// does change is counted: a watch says whether its process made a
    /// child between two questions, each watch from the moment it was taken.
    #[test]
    fn a_fork_and_a_child_s_exec_are_not_this_process_s_exec() {
        let mut peer = Stepper::start(
            std::path::Path::new("/bin/bash"),
            "echo ready; read a; /usr/bin/true; (exec /usr/bin/true); echo forked; read b; \
             /usr/bin/true; echo again; read c",
        );
        assert_eq!(peer.word(), "ready");
        let watch = InstanceWatch::of(peer.child.id()).expect("a watch");
        assert_eq!(watch.forks(), 0, "no child was made since the watch");
        assert_eq!(watch.forks(), 0, "and asking does not make one");
        peer.step();
        assert_eq!(peer.word(), "forked");
        assert_eq!(watch.continuity(), Continuity::Same);
        let counted = watch.forks();
        assert!(counted > 0, "two children were made: that is reported");
        assert_eq!(watch.forks(), counted, "and nothing more until the next");
        // a watch taken now counts from now; the first one keeps its count
        let later = InstanceWatch::of(peer.child.id()).expect("a second watch");
        assert_eq!((later.forks(), watch.forks()), (0, counted));
        peer.step();
        assert_eq!(peer.word(), "again");
        assert!(watch.forks() > counted && later.forks() > 0);
        assert_eq!(
            (watch.continuity(), later.continuity()),
            (Continuity::Same, Continuity::Same)
        );
        peer.step();
        until_instance("the exit", || watch.continuity() == Continuity::Exited);
        peer.end();
    }

    /// A watch is of ONE process: what happens to another is not its
    /// report. No process, no watch — and no watch is never "unchanged".
    #[test]
    fn a_watch_is_of_one_process_and_none_without_one() {
        let script = "echo ready; read a";
        let mut one = Stepper::start(std::path::Path::new("/bin/bash"), script);
        let mut two = Stepper::start(std::path::Path::new("/bin/bash"), script);
        assert_eq!(
            (one.word().as_str(), two.word().as_str()),
            ("ready", "ready")
        );
        let first = InstanceWatch::of(one.child.id()).expect("a watch");
        let second = InstanceWatch::of(two.child.id()).expect("a watch");
        // the first ends by itself
        one.step();
        until_instance("the exit", || first.continuity() == Continuity::Exited);
        assert_eq!(second.continuity(), Continuity::Same);
        one.end();
        // dropping every watch of a process and watching it again works
        drop(second);
        let again = InstanceWatch::of(two.child.id()).expect("watched again");
        assert_eq!(again.continuity(), Continuity::Same);
        two.end();
        until_instance("the second exit", || {
            again.continuity() == Continuity::Exited
        });
        // a pid no process can have
        assert!(InstanceWatch::of(4_000_000).is_none());
        // a test's stand-in says exactly what the test says
        let (watch, switch) = InstanceWatch::stand_in();
        assert_eq!(watch.continuity(), Continuity::Same);
        switch.unreadable();
        assert_eq!(watch.continuity(), Continuity::Unreadable);
        let (watch, switch) = InstanceWatch::stand_in();
        switch.exited();
        assert_eq!(watch.continuity(), Continuity::Exited);
        let (watch, switch) = InstanceWatch::stand_in();
        switch.executed();
        assert_eq!(watch.continuity(), Continuity::Executed);
        let (watch, switch) = InstanceWatch::stand_in();
        assert_eq!(watch.forks(), 0);
        switch.forked();
        assert_eq!((watch.forks(), watch.continuity()), (1, Continuity::Same));
    }
}
