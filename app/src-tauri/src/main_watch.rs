//! Main-thread watchdog: which IPC command holds the window's main thread,
//! and how long the main thread went without a turn.
//!
//! # Contract
//! A webview's IPC is answered on the main thread: an ordinary (non-async)
//! Tauri command runs there, inside WebKit's URL-scheme callback, and while
//! it runs nothing else does — no event, no redraw, no other request (the
//! 2026-11-26 and 2026-11-27 freezes, `docs/evidence/deck-freeze-2026112*`).
//! This module only OBSERVES; it changes no command, retries nothing and
//! never decides anything for another module.
//! - `command` is called by the one wrapper around `invoke_handler`
//!   (main.rs) for every request. On the main thread it records the
//!   command's NAME (a fixed Rust identifier or `plugin:x|y`; anything else
//!   is logged as `unknown`, never its text) and when it started; the scope
//!   clears it on return. A command that held the main thread for
//!   `SLOW_COMMAND` or longer is logged once per name every 5 s. Arguments
//!   are never read.
//! - `spawn` starts one watchdog thread. Every `BEAT` it posts one
//!   heartbeat with `run_on_main_thread` (never two outstanding: a stalled
//!   main thread does not collect a queue of them). A heartbeat pending for
//!   `STALL` or longer is one line, `[main] no main-thread turn for Nms
//!   (command=X)`, where X is the command in flight on the main thread at
//!   that moment, or `none`: the main thread is then busy in AppKit, WebKit
//!   or the input method, not in a Deck command. The next turn is one line,
//!   `[main] main thread back after Nms (command=X)`. Time is measured from
//!   when the heartbeat was POSTED, on the monotonic clock (which stops
//!   while the Mac sleeps), so neither a sleeping Mac nor a delayed watchdog
//!   thread reads as a stall.
//! - Once per stall that reaches `SAMPLE_AFTER`, the main thread's stack is
//!   sampled in-process (Apple silicon only): `thread_suspend`, the register
//!   state, a frame-pointer walk whose every read goes through
//!   `mach_vm_read_overwrite` (an unmapped address is an error, never a
//!   fault), `thread_resume`. Between suspend and resume the sampler
//!   allocates nothing and takes no lock (the suspended thread may hold the
//!   allocator's). The addresses are named afterwards on a separate thread
//!   with `dladdr`, which takes the loader's lock and may therefore wait for
//!   the main thread itself; at most one such thread exists, and its line
//!   is written whenever it can be: `[main] main thread stack after Nms
//!   (command=X): image:symbol < image:symbol …`, image BASENAMES and symbol
//!   names only (code identifiers; no path, argument or content). No
//!   process is started (no `sample`, no `spindump`): EDR-quiet.
//! - Nothing here is persisted, and under `cfg(test)` `applog` writes
//!   nothing; the line shapes are pinned by the pure `Stall` state machine.
use crate::applog::applog;
use crate::sync::LockRecover;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often the watchdog asks the main thread for a turn.
const BEAT: Duration = Duration::from_millis(250);
/// A heartbeat pending this long is a stall (one line).
pub(crate) const STALL: Duration = Duration::from_secs(2);
/// A stall this long gets one in-process stack sample.
const SAMPLE_AFTER: Duration = Duration::from_secs(5);
/// A command that held the main thread this long is logged.
pub(crate) const SLOW_COMMAND: Duration = Duration::from_millis(500);

/// The command running on the main thread right now, and since when.
static IN_FLIGHT: Mutex<Option<(&'static str, Instant)>> = Mutex::new(None);
/// The main thread's Mach port, recorded by `init` on the main thread.
static MAIN_PORT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Record which thread is the main one. Called first thing in `main`.
pub(crate) fn init() {
    // SAFETY: plain queries about the calling thread.
    unsafe {
        if libc::pthread_main_np() == 1 {
            MAIN_PORT.store(
                libc::pthread_mach_thread_np(libc::pthread_self()),
                Ordering::Release,
            );
        }
    }
}

fn on_main_thread() -> bool {
    // SAFETY: a plain query about the calling thread.
    unsafe { libc::pthread_main_np() == 1 }
}

/// A command name as it may appear in app.log: a Rust identifier, or a
/// plugin command `plugin:name|command`, short. Anything else is `unknown`
/// (a request names its command itself, so a hostile page could put text
/// there).
pub(crate) fn loggable(name: &str) -> &str {
    let shaped = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_:|-".contains(&b));
    if shaped {
        name
    } else {
        "unknown"
    }
}

/// Interned command names: `IN_FLIGHT` holds a `&'static str` so the
/// watchdog never allocates while reading it. The set is the registered
/// commands (plus `unknown`), so it is bounded.
fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let name = loggable(name);
    let mut names = NAMES.lock_or_recover();
    if let Some(found) = names.iter().find(|n| **n == name) {
        return found;
    }
    if names.len() >= 512 {
        return "unknown";
    }
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    names.push(leaked);
    leaked
}

/// Marks one request's command as in flight on the main thread until drop.
pub(crate) struct CommandScope {
    name: &'static str,
    started: Instant,
}

/// Called by the `invoke_handler` wrapper for every request. Off the main
/// thread (a request answered elsewhere) it records nothing.
pub(crate) fn command(name: &str) -> Option<CommandScope> {
    if !on_main_thread() {
        return None;
    }
    let name = intern(name);
    let started = Instant::now();
    *IN_FLIGHT.lock_or_recover() = Some((name, started));
    Some(CommandScope { name, started })
}

impl Drop for CommandScope {
    fn drop(&mut self) {
        *IN_FLIGHT.lock_or_recover() = None;
        let held = self.started.elapsed();
        if held >= SLOW_COMMAND {
            slow_command_line(self.name, held);
        }
    }
}

/// At most one line per command name every 5 s, carrying how many were
/// folded into it.
fn slow_command_line(name: &'static str, held: Duration) {
    use std::collections::HashMap;
    static SEEN: Mutex<Option<HashMap<&'static str, (Instant, u32)>>> = Mutex::new(None);
    let mut seen = SEEN.lock_or_recover();
    let entry = seen
        .get_or_insert_with(HashMap::new)
        .entry(name)
        .or_insert((Instant::now() - Duration::from_secs(60), 0));
    if entry.0.elapsed() < Duration::from_secs(5) {
        entry.1 += 1;
        return;
    }
    let folded = std::mem::take(&mut entry.1);
    entry.0 = Instant::now();
    drop(seen);
    let more = if folded > 0 {
        format!(" (+{folded} since last)")
    } else {
        String::new()
    };
    applog(&format!(
        "[main] command {name} held the main thread {}ms{more}",
        held.as_millis()
    ));
}

fn in_flight_name() -> &'static str {
    IN_FLIGHT.lock_or_recover().map_or("none", |(name, _)| name)
}

/// One stall episode, as the watchdog sees it: pure, so the lines it writes
/// are pinned by tests without a main thread.
#[derive(Default)]
pub(crate) struct Stall {
    /// the command named in the stall line, once one was written
    reported: Option<&'static str>,
    sampled: bool,
}

/// What the watchdog does after one observation.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Nothing,
    /// write this line
    Line(String),
    /// write this line and sample the main thread's stack
    LineAndSample(String),
    /// sample the stack (the stall line was already written)
    Sample,
}

impl Stall {
    /// `pending`: how long the outstanding heartbeat has waited, or `None`
    /// when the main thread just answered it after `answered` of waiting.
    pub(crate) fn observe(
        &mut self,
        pending: Option<Duration>,
        answered: Option<Duration>,
        command: &'static str,
    ) -> Step {
        match (pending, self.reported) {
            (Some(waited), None) if waited >= STALL => {
                self.reported = Some(command);
                let line = format!(
                    "[main] no main-thread turn for {}ms (command={command})",
                    waited.as_millis()
                );
                if waited >= SAMPLE_AFTER {
                    self.sampled = true;
                    Step::LineAndSample(line)
                } else {
                    Step::Line(line)
                }
            }
            (Some(waited), Some(_)) if waited >= SAMPLE_AFTER && !self.sampled => {
                self.sampled = true;
                Step::Sample
            }
            (None, Some(named)) => {
                let waited = answered.unwrap_or_default();
                *self = Stall::default();
                Step::Line(format!(
                    "[main] main thread back after {}ms (command={named})",
                    waited.as_millis()
                ))
            }
            _ => Step::Nothing,
        }
    }

    /// The command the stall line named (for the stack line).
    pub(crate) fn command(&self) -> &'static str {
        self.reported.unwrap_or("none")
    }
}

/// Start the watchdog thread. `post` hands one closure to the main thread.
pub(crate) fn spawn(app: tauri::AppHandle) {
    let _ = std::thread::Builder::new()
        .name("deck-main-watch".into())
        .spawn(move || watch(move |turn| app.run_on_main_thread(turn).is_ok()));
}

/// The outstanding heartbeat, read and written under one lock so an answer
/// is never separated from the post it answers.
#[derive(Default)]
struct Beat {
    /// when the outstanding heartbeat was posted; `None` = none outstanding
    posted: Option<Instant>,
    /// how long the last answered heartbeat waited, until observed
    answered: Option<Duration>,
}

fn watch(post: impl Fn(Box<dyn FnOnce() + Send>) -> bool) {
    use std::sync::Arc;
    let beat: Arc<Mutex<Beat>> = Arc::default();
    let mut stall = Stall::default();
    loop {
        std::thread::sleep(BEAT);
        let (pending, last_answer) = {
            let mut b = beat.lock_or_recover();
            (b.posted.map(|at| at.elapsed()), b.answered.take())
        };
        let step = stall.observe(pending, last_answer, in_flight_name());
        let waited = pending.unwrap_or_default();
        match step {
            Step::Nothing => {}
            Step::Line(line) => applog(&line),
            Step::LineAndSample(line) => {
                applog(&line);
                sample_main_stack(waited, stall.command());
            }
            Step::Sample => sample_main_stack(waited, stall.command()),
        }
        if pending.is_none() {
            let at = Instant::now();
            beat.lock_or_recover().posted = Some(at);
            let answer = beat.clone();
            let delivered = post(Box::new(move || {
                let mut b = answer.lock_or_recover();
                b.answered = Some(at.elapsed());
                b.posted = None;
            }));
            if !delivered {
                // the event loop is gone (shutdown): stop watching
                return;
            }
        }
    }
}

/// At most one symbolizer thread at a time (it may wait for the loader lock
/// the stalled main thread holds).
static SYMBOLIZING: AtomicBool = AtomicBool::new(false);

fn sample_main_stack(waited: Duration, command: &'static str) {
    let port = MAIN_PORT.load(Ordering::Acquire);
    if port == 0 || SYMBOLIZING.swap(true, Ordering::AcqRel) {
        return;
    }
    let frames = stack::sample(port);
    let spawned = std::thread::Builder::new()
        .name("deck-main-symbols".into())
        .spawn(move || {
            let line = match frames {
                Some(frames) if !frames.is_empty() => {
                    let names: Vec<String> = frames.iter().map(|pc| stack::name(*pc)).collect();
                    format!(
                        "[main] main thread stack after {}ms (command={command}): {}",
                        waited.as_millis(),
                        names.join(" < ")
                    )
                }
                _ => format!(
                    "[main] main thread stack after {}ms (command={command}) unavailable",
                    waited.as_millis()
                ),
            };
            applog(&line);
            SYMBOLIZING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        SYMBOLIZING.store(false, Ordering::Release);
    }
}

/// The in-process stack sample (see the module contract). Apple silicon
/// only; elsewhere it samples nothing.
mod stack {
    /// Frames kept from one sample.
    const MAX_FRAMES: usize = 32;

    #[cfg(target_arch = "aarch64")]
    pub(super) fn sample(port: libc::mach_port_t) -> Option<Vec<u64>> {
        type KernReturn = libc::c_int;
        extern "C" {
            fn thread_suspend(thread: libc::mach_port_t) -> KernReturn;
            fn thread_resume(thread: libc::mach_port_t) -> KernReturn;
            fn thread_get_state(
                thread: libc::mach_port_t,
                flavor: libc::c_int,
                state: *mut u32,
                count: *mut u32,
            ) -> KernReturn;
            static mach_task_self_: libc::mach_port_t;
            fn mach_vm_read_overwrite(
                task: libc::mach_port_t,
                address: u64,
                size: u64,
                data: u64,
                out: *mut u64,
            ) -> KernReturn;
        }
        const ARM_THREAD_STATE64: libc::c_int = 6;
        const STATE_WORDS: u32 = 68; // x0..x28, fp, lr, sp, pc, cpsr+pad
                                     // user addresses are below 2^47; the bits above carry pointer auth
        const ADDRESS: u64 = (1 << 47) - 1;
        // Everything below is preallocated: between suspend and resume the
        // suspended thread may hold the allocator's lock.
        let mut frames = [0u64; MAX_FRAMES];
        let mut found = 0usize;
        let mut state = [0u64; 34];
        let mut count = STATE_WORDS;
        // SAFETY: Mach calls on this process's own main thread port; every
        // memory read goes through the kernel and fails instead of faulting;
        // resume follows suspend on every path.
        unsafe {
            if thread_suspend(port) != 0 {
                return None;
            }
            let got = thread_get_state(
                port,
                ARM_THREAD_STATE64,
                state.as_mut_ptr().cast(),
                &mut count,
            );
            if got == 0 {
                let (mut fp, lr, pc) = (state[29], state[30], state[32]);
                for address in [pc, lr] {
                    let address = address & ADDRESS;
                    if address != 0 && found < MAX_FRAMES {
                        frames[found] = address;
                        found += 1;
                    }
                }
                let task = mach_task_self_;
                while found < MAX_FRAMES && fp != 0 && fp & 7 == 0 {
                    let mut pair = [0u64; 2];
                    let mut read = 0u64;
                    if mach_vm_read_overwrite(task, fp, 16, pair.as_mut_ptr() as u64, &mut read)
                        != 0
                        || read != 16
                    {
                        break;
                    }
                    let (next, ret) = (pair[0], pair[1] & ADDRESS);
                    if ret == 0 {
                        break;
                    }
                    frames[found] = ret;
                    found += 1;
                    if next <= fp {
                        break; // frames grow toward higher addresses
                    }
                    fp = next;
                }
            }
            thread_resume(port);
            if got != 0 {
                return None;
            }
        }
        Some(frames[..found].to_vec())
    }

    #[cfg(not(target_arch = "aarch64"))]
    pub(super) fn sample(_port: libc::mach_port_t) -> Option<Vec<u64>> {
        None
    }

    /// `image:symbol` for one address: the image's file NAME (never its
    /// directory) and the nearest exported symbol, a Rust one demangled
    /// without its hash.
    pub(super) fn name(address: u64) -> String {
        let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
        // SAFETY: dladdr fills `info` with pointers into loader-owned
        // strings, valid while the image stays loaded (Deck unloads none).
        let found = unsafe { libc::dladdr(address as *const libc::c_void, &mut info) } != 0;
        if !found {
            return "?".into();
        }
        let text = |ptr: *const libc::c_char| {
            if ptr.is_null() {
                None
            } else {
                // SAFETY: a NUL-terminated loader string (above).
                Some(
                    unsafe { std::ffi::CStr::from_ptr(ptr) }
                        .to_string_lossy()
                        .into_owned(),
                )
            }
        };
        let image = text(info.dli_fname)
            .and_then(|path| path.rsplit('/').next().map(str::to_string))
            .unwrap_or_else(|| "?".into());
        let symbol = text(info.dli_sname).map_or_else(|| "?".into(), |s| demangle(&s));
        format!("{image}:{symbol}")
    }

    /// Rust's legacy mangling (`_ZN3std2io5Write9write_all17h0123…E`) as
    /// `std::io::Write::write_all`; anything else unchanged but the leading
    /// underscore. At most 80 characters.
    pub(super) fn demangle(symbol: &str) -> String {
        let plain = symbol.strip_prefix('_').unwrap_or(symbol);
        let mut out = String::new();
        if let Some(mut rest) = plain.strip_prefix("ZN") {
            let mut parts = Vec::new();
            while let Some(digits) = rest.find(|c: char| !c.is_ascii_digit()).filter(|&n| n > 0) {
                let Ok(len) = rest[..digits].parse::<usize>() else {
                    break;
                };
                let Some(part) = rest.get(digits..digits + len) else {
                    break;
                };
                let hash = part.len() == 17
                    && part.starts_with('h')
                    && part[1..].bytes().all(|b| b.is_ascii_hexdigit());
                if !hash {
                    parts.push(
                        part.replace("$LT$", "<")
                            .replace("$GT$", ">")
                            .replace("..", "::"),
                    );
                }
                rest = &rest[digits + len..];
            }
            if !parts.is_empty() {
                out = parts.join("::");
            }
        }
        if out.is_empty() {
            if let Some(v0) = plain.strip_prefix('R') {
                out = v0_path(v0);
            }
        }
        if out.is_empty() {
            out = plain.to_string();
        }
        out.chars().take(80).collect()
    }

    /// The identifiers of a v0-mangled path (`RNvMs_NtNtCs…_3std6thread…`)
    /// joined with `::`: disambiguators (`s…_`) and back-references (`B…_`)
    /// are skipped, generic arguments are not reconstructed. Enough to read
    /// a stack, not a full demangler.
    fn v0_path(mangled: &str) -> String {
        let b = mangled.as_bytes();
        let mut idents: Vec<&str> = Vec::new();
        let mut i = 0;
        while i < b.len() {
            if matches!(b[i], b's' | b'B') {
                let mut j = i + 1;
                while j < b.len() && b[j].is_ascii_alphanumeric() {
                    j += 1;
                }
                if j < b.len()
                    && b[j] == b'_'
                    && (b[i] == b'B' || b.get(j + 1).is_some_and(u8::is_ascii_digit))
                {
                    i = j + 1;
                    continue;
                }
            }
            if b[i].is_ascii_digit() {
                let mut j = i;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                let len: usize = mangled[i..j].parse().unwrap_or(0);
                if b.get(j) == Some(&b'_') {
                    j += 1;
                }
                match mangled.get(j..j + len) {
                    Some(ident)
                        if !ident.is_empty()
                            && ident
                                .bytes()
                                .all(|c| c.is_ascii_alphanumeric() || c == b'_') =>
                    {
                        idents.push(ident);
                        i = j + len;
                    }
                    _ => i = j.max(i + 1),
                }
                continue;
            }
            i += 1;
        }
        idents.join("::")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stall is one line naming the command in flight, then one line when
    /// the main thread answers; a long one asks for one stack sample.
    #[test]
    fn a_stall_and_its_end_are_one_line_each_with_the_command() {
        let mut stall = Stall::default();
        let ms = Duration::from_millis;
        assert_eq!(stall.observe(Some(ms(250)), None, "none"), Step::Nothing);
        assert_eq!(
            stall.observe(Some(ms(2100)), None, "attach_session"),
            Step::Line("[main] no main-thread turn for 2100ms (command=attach_session)".into())
        );
        // still stalled: nothing more until the sample
        assert_eq!(stall.observe(Some(ms(3000)), None, "none"), Step::Nothing);
        assert_eq!(stall.observe(Some(ms(5200)), None, "none"), Step::Sample);
        assert_eq!(stall.command(), "attach_session");
        assert_eq!(stall.observe(Some(ms(9000)), None, "none"), Step::Nothing);
        assert_eq!(
            stall.observe(None, Some(ms(9100)), "none"),
            Step::Line("[main] main thread back after 9100ms (command=attach_session)".into())
        );
        // a new episode starts clean
        assert_eq!(stall.observe(None, Some(ms(3)), "none"), Step::Nothing);
        assert_eq!(
            stall.observe(Some(ms(6000)), None, "none"),
            Step::LineAndSample("[main] no main-thread turn for 6000ms (command=none)".into())
        );
        assert_eq!(stall.observe(Some(ms(7000)), None, "none"), Step::Nothing);
    }

    /// The command in flight is recorded only on the main thread, and only
    /// by name: a request naming arbitrary text is logged as `unknown`.
    #[test]
    fn only_command_names_are_recorded() {
        assert_eq!(loggable("attach_session"), "attach_session");
        assert_eq!(loggable("plugin:updater|check"), "plugin:updater|check");
        assert_eq!(loggable("rm -rf /Users/x"), "unknown");
        assert_eq!(loggable(""), "unknown");
        assert_eq!(loggable(&"a".repeat(65)), "unknown");
        // the test harness's thread is not the process's main thread
        assert!(command("attach_session").is_none());
        assert_eq!(in_flight_name(), "none");
    }

    /// The watchdog loop posts one heartbeat at a time and writes the stall
    /// and recovery lines from what the posted turns report.
    #[test]
    fn the_loop_posts_one_heartbeat_at_a_time() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Arc;
        let posted = Arc::new(AtomicUsize::new(0));
        type Turns = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;
        let held: Turns = Arc::default();
        let (p, h) = (posted.clone(), held.clone());
        std::thread::spawn(move || {
            watch(move |turn| {
                let n = p.fetch_add(1, Ordering::SeqCst);
                if n >= 3 {
                    return false; // stop the loop
                }
                h.lock_or_recover().push(turn);
                true
            })
        });
        std::thread::sleep(BEAT * 4);
        assert_eq!(
            posted.load(Ordering::SeqCst),
            1,
            "one outstanding heartbeat"
        );
        // the "main thread" answers: the next beat posts again
        let turn = held.lock_or_recover().pop().expect("posted turn");
        turn();
        std::thread::sleep(BEAT * 3);
        assert_eq!(posted.load(Ordering::SeqCst), 2);
        // answering again lets the next post end the loop
        let turn = held.lock_or_recover().pop().expect("posted turn");
        turn();
    }

    #[test]
    fn rust_symbols_are_demangled_without_their_hash() {
        assert_eq!(
            stack::demangle("_ZN3std2io5Write9write_all17h0123456789abcdefE"),
            "std::io::Write::write_all"
        );
        assert_eq!(
            stack::demangle(
                "_ZN4deck3pty14attach_session28_$u7b$$u7b$closure$u7d$$u7d$17h0123456789abcdefE"
            ),
            "deck::pty::attach_session::_$u7b$$u7b$closure$u7d$$u7d$"
        );
        assert_eq!(
            stack::demangle("_RNvMs_NtNtCsa9BKTri5B3M_3std6thread6threadNtB4_6Thread4park"),
            "std::thread::thread::Thread::park"
        );
        assert_eq!(stack::demangle("_fork"), "fork");
        assert_eq!(stack::demangle("__posix_spawn"), "_posix_spawn");
    }

    /// The sampler reads a real suspended thread's frames and names them,
    /// and leaves the thread running.
    #[cfg(target_arch = "aarch64")]
    #[test]
    fn a_suspended_thread_is_sampled_and_resumed() {
        let (ready_tx, ready) = std::sync::mpsc::channel();
        let (stop_tx, stop) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            // SAFETY: a plain query about the calling thread.
            let port = unsafe { libc::pthread_mach_thread_np(libc::pthread_self()) };
            ready_tx.send(port).unwrap();
            let _ = stop.recv();
        });
        let port = ready.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let frames = stack::sample(port).expect("sampled");
        assert!(frames.len() >= 2, "{frames:?}");
        let names: Vec<String> = frames.iter().map(|pc| stack::name(*pc)).collect();
        println!("sampled: {}", names.join(" < "));
        assert!(names.iter().all(|n| !n.contains('/')), "{names:?}");
        stop_tx.send(()).unwrap();
        worker
            .join()
            .expect("the sampled thread ran on after resume");
    }
}
