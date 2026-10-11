//! Code identity for the re-entry transaction: which product a process is,
//! and that it executes the code a validated file holds. Asked of the kernel
//! and of Security.framework inside this process — no `codesign`, `spctl` or
//! client program is ever started to find out, and nothing here opens a
//! network connection or a dialog (`kSecCSNoNetworkAccess` on every call).
//!
//! # Contract
//! Three questions, each answered with ONE closed word. No status code,
//! certificate, signing identifier, path or hash leaves this module, and
//! `CodeIdentity` prints nothing.
//!
//! - **`on_disk(path, product)` is a DISK fact.** The file's signature is
//!   validated in full against the product's requirement
//!   (`SecStaticCodeCheckValidity` with `kSecCSStrictValidate |
//!   kSecCSCheckAllArchitectures | kSecCSNoNetworkAccess`): the signature and
//!   its certificate chain, the sealed components, and every page of the
//!   executable against the code directory. The answer carries the unique
//!   identifier of that code directory (`kSecCodeInfoUnique`), read from the
//!   SAME static code object after it validated. It says nothing about any
//!   process.
//! - **`running_product(pid, product)` and `running(pid, code)` are
//!   RUNNING-CODE facts.** A code object is looked up for the pid at the
//!   kernel (`SecCodeCopyGuestWithAttributes`, `kSecGuestAttributePid`),
//!   fresh for every question and never kept, and validated dynamically
//!   (`SecCodeCheckValidity` with `kSecCSStrictValidate |
//!   kSecCSNoNetworkAccess`). That call asks the kernel whether the process's
//!   code is still valid and compares the code directory hash the KERNEL
//!   holds for the running image with the one of the file at the process's
//!   path — the documented property of the call ("secure against attempts to
//!   modify the file system source"). A process that still runs an old image
//!   while its path already holds another validly signed file fails it,
//!   although that file alone validates. `running` adds `cdhash H"…"` of one
//!   validated reference to the requirement, so `Is` means: at the instant
//!   Security asked the kernel, this pid executed exactly that code, and the
//!   file at its path held it.
//!
//! What is NOT evidence and is not used as such: the static code, the
//! signing information and the path that Security derives from a running
//! code all answer from the file at the process's path — after a replacement
//! they describe the NEW file. A requirement that matched, a Team string or
//! a signing identifier read that way proves nothing about the image in
//! memory; only the dynamic validity call above does.
//!
//! **A pid is a number.** The lookup names no generation: an exec inside the
//! same pid keeps the pid and (as observed) the birth instant. A code object
//! held across an exec would go on answering for the image it first saw,
//! which is why none is kept. The pinned code directory hash is what notices
//! an exec — the kernel then holds another hash — and the caller fences every
//! answer with the process generation, the kernel's image path and the
//! file's state, read before and after (`procinfo::binding`). An audit-token
//! lookup would bind the exec generation too, but needs the target's task
//! name port; this feature takes no task port of any kind.
//!
//! **Universal files.** `on_disk` validates every slice and identifies the
//! one Security selects for this host. A process executing another slice of
//! the same file has another code directory: it is `IsNot` for that
//! reference, never bound by file name or signer alone.
//!
//! **Bounded by the caller.** These calls cannot be interrupted, and
//! `on_disk` reads the whole file. Nothing here retries, caches or watches a
//! clock: the caller asks its phase's deadline before it calls and again
//! after the call returned, and an answer that arrived late is discarded
//! whatever it says — `procinfo::ExecutableRef::read` for the file,
//! `procinfo::validated` for the two running-code questions, which are asked
//! only for a caller that owns a deadline at all. A late `Is` is therefore
//! never a binding; it is not a `Diverged` or an `IsNot` either.
//!
//! Revocation is the system's: Deck makes no online check, and a process is
//! running only because the system let it start.

use std::ffi::c_void;
use std::path::Path;
use std::str::FromStr;

use core_foundation::base::{CFType, TCFType};
use core_foundation::data::CFData;
use core_foundation::dictionary::{CFDictionaryGetValue, CFDictionaryRef};
use core_foundation::string::CFStringRef;
use core_foundation::url::CFURL;
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement, SecStaticCode,
};

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecCodeInfoUnique: CFStringRef;
    #[cfg(test)]
    static kSecCodeInfoIdentifier: CFStringRef;
    fn SecCodeCopySigningInformation(
        code: *const c_void,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> i32;
}

/// The product a client must be, as its adapter states it: one requirement
/// in Apple's code requirement language. Static data; it grants nothing.
pub(crate) struct ProductIdentity {
    pub(crate) requirement: &'static str,
}

/// The identity of one validated code: the unique identifier of its code
/// directory, and the requirement it validated against. `Debug` prints none
/// of it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CodeIdentity {
    unique: [u8; 20],
    product: &'static str,
}

impl std::fmt::Debug for CodeIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CodeIdentity(..)")
    }
}

impl CodeIdentity {
    /// Whether this identity was validated against `product`'s requirement.
    pub(crate) fn is_of(&self, product: &ProductIdentity) -> bool {
        self.product == product.requirement
    }
}

/// What a file holds, as code of one product.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum OnDisk {
    /// Validly signed code of the product, validated in full.
    Valid(CodeIdentity),
    /// Not that: unsigned, another signer or product, a broken signature or
    /// a page that does not match it.
    Invalid,
    /// Security gave no answer: no proof either way.
    Unreadable,
}

/// What a process executes right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Running {
    /// Valid code that satisfies the requirement, and the file at the
    /// process's path holds exactly that code.
    Is,
    /// Valid code of another signer, product or code directory.
    IsNot,
    /// The process does not run what its path holds now (replaced, rewritten,
    /// removed), or the kernel no longer vouches for its code.
    Diverged,
    /// The kernel or Security gave no answer: no proof either way.
    Unreadable,
}

const STATIC_FLAGS: Flags = Flags::STRICT_VALIDATE
    .union(Flags::CHECK_ALL_ARCHITECTURES)
    .union(Flags::NO_NETWORK_ACCESS);
const DYNAMIC_FLAGS: Flags = Flags::STRICT_VALIDATE.union(Flags::NO_NETWORK_ACCESS);

// Security's documented answers (CSCommon.h) that are a statement about the
// code. Every other failure is "no answer".
const REQ_FAILED: i32 = -67050;
const UNSIGNED: i32 = -67062;
const SIGNATURE_FAILED: i32 = -67061;
const SIGNATURE_INVALID: i32 = -67045;
const SIGNATURE_UNTRUSTED: i32 = -66996;
const BAD_MAIN_EXECUTABLE: i32 = -67010;
const BAD_OBJECT_FORMAT: i32 = -67049;
const RESOURCES_INVALID: i32 = -67055;
const BAD_RESOURCE: i32 = -67054;
const DETRITUS: i32 = -66999;
const REVOKED: i32 = -66992;
const STATIC_CODE_NOT_FOUND: i32 = -67068;
const STATIC_CODE_CHANGED: i32 = -67034;
const GUEST_INVALID: i32 = -67063;
/// Security's wrapping of `ENOENT`: the process's path names nothing.
const NO_SUCH_FILE: i32 = 100_000 + libc::ENOENT;

/// Whether a status says the code is not valid code of the product.
fn not_the_product(status: i32) -> bool {
    matches!(
        status,
        REQ_FAILED
            | UNSIGNED
            | SIGNATURE_FAILED
            | SIGNATURE_INVALID
            | SIGNATURE_UNTRUSTED
            | BAD_MAIN_EXECUTABLE
            | BAD_OBJECT_FORMAT
            | RESOURCES_INVALID
            | BAD_RESOURCE
            | DETRITUS
            | REVOKED
    )
}

// What the tests count and inject on their own thread: how many validations
// ran, and one fault for the calls that follow.
#[cfg(test)]
thread_local! {
    pub(crate) static STATIC_VALIDATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static DYNAMIC_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static FAULT: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
}

/// A fault for this thread's next calls (tests only).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Security answers nothing: every question is `Unreadable`.
    Unreadable,
    /// A full validation takes this long before it answers truthfully.
    Slow(std::time::Duration),
}

/// The unique identifier of a static code's code directory.
fn unique_of(code: &SecStaticCode) -> Option<[u8; 20]> {
    let mut information: CFDictionaryRef = std::ptr::null();
    // SAFETY: `code` is a live static code object and `information` is valid
    // storage for the one dictionary the call hands back on success.
    let status = unsafe {
        SecCodeCopySigningInformation(code.as_concrete_TypeRef().cast(), 0, &mut information)
    };
    if status != 0 || information.is_null() {
        return None;
    }
    // SAFETY: the call above created this dictionary (create rule); the
    // wrapper releases it.
    let information = unsafe { CFType::wrap_under_create_rule(information.cast()) };
    // SAFETY: a live dictionary and one of Security's own constant keys.
    let value = unsafe {
        CFDictionaryGetValue(information.as_CFTypeRef().cast(), kSecCodeInfoUnique.cast())
    };
    if value.is_null() {
        return None;
    }
    // SAFETY: a value owned by that live dictionary, retained while it is
    // read (get rule).
    let value = unsafe { CFType::wrap_under_get_rule(value) };
    value.downcast::<CFData>()?.bytes().try_into().ok()
}

/// The file at `path` as code of `product`: validated in full. A disk fact.
pub(crate) fn on_disk(path: &Path, product: &ProductIdentity) -> OnDisk {
    #[cfg(test)]
    {
        STATIC_VALIDATIONS.with(|count| count.set(count.get() + 1));
        match FAULT.with(std::cell::Cell::get) {
            Some(Fault::Unreadable) => return OnDisk::Unreadable,
            Some(Fault::Slow(time)) => std::thread::sleep(time),
            None => {}
        }
    }
    let (Some(url), Ok(requirement)) = (
        CFURL::from_path(path, false),
        SecRequirement::from_str(product.requirement),
    ) else {
        return OnDisk::Unreadable;
    };
    let answer = SecStaticCode::from_path(&url, Flags::NONE).and_then(|code| {
        code.check_validity(STATIC_FLAGS, &requirement)?;
        Ok(unique_of(&code))
    });
    match answer {
        Ok(Some(unique)) => OnDisk::Valid(CodeIdentity {
            unique,
            product: product.requirement,
        }),
        Ok(None) => OnDisk::Unreadable,
        Err(error) if not_the_product(error.code()) || error.code() == STATIC_CODE_NOT_FOUND => {
            OnDisk::Invalid
        }
        Err(_) => OnDisk::Unreadable,
    }
}

/// One dynamic validation of the process `pid` against `requirement`, on a
/// code object looked up now and dropped at once.
fn validate_running(pid: u32, requirement: &str) -> Running {
    #[cfg(test)]
    {
        DYNAMIC_CHECKS.with(|count| count.set(count.get() + 1));
        if FAULT.with(std::cell::Cell::get) == Some(Fault::Unreadable) {
            return Running::Unreadable;
        }
    }
    let (Ok(pid), Ok(requirement)) = (
        libc::pid_t::try_from(pid),
        SecRequirement::from_str(requirement),
    ) else {
        return Running::Unreadable;
    };
    let mut attributes = GuestAttributes::new();
    attributes.set_pid(pid);
    let answer = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .and_then(|guest| guest.check_validity(DYNAMIC_FLAGS, &requirement));
    let answer = match answer {
        Ok(()) => Running::Is,
        Err(error) => match error.code() {
            STATIC_CODE_CHANGED | GUEST_INVALID | STATIC_CODE_NOT_FOUND | NO_SUCH_FILE => {
                Running::Diverged
            }
            status if not_the_product(status) => Running::IsNot,
            _ => Running::Unreadable,
        },
    };
    // A test may keep this truthful answer back: the call that returns late.
    #[cfg(test)]
    hold::keep_back(pid as u32);
    answer
}

/// Whether the process `pid` executes, right now, valid code of `product`
/// that the file at its path holds. A running-code fact about a pid, not a
/// generation: the caller fences it.
pub(crate) fn running_product(pid: u32, product: &ProductIdentity) -> Running {
    validate_running(pid, product.requirement)
}

/// Whether the process `pid` executes, right now, exactly `code`. A
/// running-code fact about a pid, not a generation: the caller fences it.
pub(crate) fn running(pid: u32, code: &CodeIdentity) -> Running {
    let unique: String = code.unique.iter().map(|b| format!("{b:02x}")).collect();
    validate_running(pid, &format!("({}) and cdhash H\"{unique}\"", code.product))
}

/// The signing identifier a file's signature names, unvalidated (tests
/// only: how a test names "the product that is this very file").
#[cfg(test)]
fn identifier_of(path: &Path) -> Option<String> {
    use core_foundation::string::CFString;
    let code = SecStaticCode::from_path(&CFURL::from_path(path, false)?, Flags::NONE).ok()?;
    let mut information: CFDictionaryRef = std::ptr::null();
    // SAFETY: as in `unique_of`.
    let status = unsafe {
        SecCodeCopySigningInformation(code.as_concrete_TypeRef().cast(), 0, &mut information)
    };
    if status != 0 || information.is_null() {
        return None;
    }
    // SAFETY: as in `unique_of`.
    let information = unsafe { CFType::wrap_under_create_rule(information.cast()) };
    // SAFETY: as in `unique_of`.
    let value = unsafe {
        CFDictionaryGetValue(
            information.as_CFTypeRef().cast(),
            kSecCodeInfoIdentifier.cast(),
        )
    };
    if value.is_null() {
        return None;
    }
    // SAFETY: as in `unique_of`.
    let value = unsafe { CFType::wrap_under_get_rule(value) };
    Some(value.downcast::<CFString>()?.to_string())
}

/// Test support shared with the modules above this one: stand-in products,
/// fixture programs and their ad-hoc signatures. The body is in the test
/// module below, where `cc` and `codesign` — test-time tools, never run by
/// the app — may be started.
#[cfg(test)]
pub(crate) use tests::fixture;
/// Test support: a dynamic validation whose answer is kept back (`hold`).
#[cfg(test)]
pub(crate) use tests::hold;

#[cfg(test)]
mod tests {
    //! The mechanism against real processes and real signatures.
    //!
    //! The fixtures are tiny C programs built and signed AD HOC here, and
    //! judged by a TEST requirement (`identifier "…"`). They prove how a
    //! running image is bound to a file. They carry no certificate: nothing
    //! below is a validation of Anthropic's signature. What the production
    //! requirement accepts and rejects is tested where it is stated
    //! (`agent_reentry/claude.rs`).

    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};

    pub(crate) mod fixture {
        use super::*;
        use std::sync::atomic::{AtomicU64, Ordering};

        /// The signing identifier of every fixture of "the test product".
        pub(crate) const IDENTIFIER: &str = "io.c9r.deck.test.identity-fixture";
        /// Another product of the same (absent) signer.
        pub(crate) const OTHER_IDENTIFIER: &str = "io.c9r.deck.test.other-product";

        /// A leaked product whose requirement is exactly this identifier.
        pub(crate) fn product(identifier: &str) -> &'static ProductIdentity {
            Box::leak(Box::new(ProductIdentity {
                requirement: Box::leak(format!("identifier \"{identifier}\"").into_boxed_str()),
            }))
        }

        /// The product "exactly the signing identifier this file carries".
        pub(crate) fn product_of(path: &Path) -> &'static ProductIdentity {
            product(&identifier_of(path).expect("a signed file names its identifier"))
        }

        /// A new private directory, canonical (the kernel reports real paths).
        pub(crate) fn directory(label: &str) -> PathBuf {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!(
                "deck-identity-{label}-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("fixture directory");
            std::fs::canonicalize(&dir).expect("canonical fixture directory")
        }

        fn run(tool: &str, args: &[&str]) {
            let output = Command::new(tool).args(args).output().expect(tool);
            assert!(
                output.status.success(),
                "{tool} {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        /// Sign `path` ad hoc under `identifier`, replacing any signature.
        pub(crate) fn sign(path: &Path, identifier: &str) {
            run(
                "/usr/bin/codesign",
                &["-f", "-s", "-", "-i", identifier, path.to_str().unwrap()],
            );
        }

        /// Remove the signature of `path` altogether.
        pub(crate) fn unsign(path: &Path) {
            run(
                "/usr/bin/codesign",
                &["--remove-signature", path.to_str().unwrap()],
            );
        }

        /// Overwrite the bytes that follow `marker` in the file with `tag`
        /// (the file's build tag): another build of the same program.
        pub(crate) fn retag(path: &Path, marker: &[u8], tag: &str) {
            let mut bytes = std::fs::read(path).expect("fixture bytes");
            let at = bytes
                .windows(marker.len())
                .position(|window| window == marker)
                .expect("the build tag marker is in the file")
                + marker.len();
            assert!(tag.len() <= 16, "a build tag has sixteen bytes");
            bytes[at..at + tag.len()].copy_from_slice(tag.as_bytes());
            std::fs::write(path, bytes).expect("retagged fixture");
        }

        /// An idle program: it prints `<tag> <pid>`, then reads lines. `exec
        /// <path>` replaces its image with that program in the same pid; the
        /// end of its input ends it. (Every line is indented: the source
        /// scanners take a line at column 0 for the end of a test module.)
        const SOURCE: &str = r#"
            #include <stdio.h>
            #include <string.h>
            #include <unistd.h>
            int main(void) {
                char line[4096];
                printf("%s %d\n", TAG, getpid());
                fflush(stdout);
                while (fgets(line, sizeof line, stdin)) {
                    line[strcspn(line, "\n")] = 0;
                    if (!strncmp(line, "exec ", 5)) {
                        char *next[] = { line + 5, 0 };
                        execv(next[0], next);
                    }
                }
                return 0;
            }
        "#;

        /// Build the idle program at `path` for `archs`, with `tag` compiled
        /// in (so two tags are two different contents), signed under
        /// `identifier`.
        pub(crate) fn build(path: &Path, tag: &str, archs: &[&str], identifier: &str) {
            let source = path.with_extension("c");
            std::fs::write(&source, SOURCE).expect("fixture source");
            let define = format!("-DTAG=\"{tag}\"");
            let mut args = vec!["-O1", &define, "-o", path.to_str().unwrap()];
            for arch in archs {
                args.extend(["-arch", arch]);
            }
            args.push(source.to_str().unwrap());
            run("/usr/bin/cc", &args);
            std::fs::remove_file(&source).expect("fixture source removed");
            sign(path, identifier);
        }

        /// A running fixture. Dropping it ends the process.
        pub(crate) struct Process {
            child: Child,
            lines: BufReader<std::process::ChildStdout>,
        }

        impl Process {
            /// Start `program` (through `/usr/bin/arch -<slice>` when a slice
            /// is named) and wait for its first line.
            pub(crate) fn start(program: &Path, slice: Option<&str>) -> Option<(Self, String)> {
                let mut command = match slice {
                    Some(slice) => {
                        let mut command = Command::new("/usr/bin/arch");
                        command.arg(format!("-{slice}")).arg(program);
                        command
                    }
                    None => Command::new(program),
                };
                let mut child = command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .ok()?;
                let mut process = Self {
                    lines: BufReader::new(child.stdout.take()?),
                    child,
                };
                let tag = process.line()?;
                Some((process, tag))
            }

            /// The tag of the next `<tag> <pid>` line the process prints.
            pub(crate) fn line(&mut self) -> Option<String> {
                let mut line = String::new();
                self.lines.read_line(&mut line).ok().filter(|n| *n > 0)?;
                let (tag, pid) = line.trim_end().rsplit_once(' ')?;
                (pid.parse::<u32>().ok() == Some(self.pid())).then(|| tag.to_string())
            }

            pub(crate) fn pid(&self) -> u32 {
                self.child.id()
            }

            /// Replace the process's image with `program`, in the same pid.
            pub(crate) fn exec(&mut self, program: &Path) -> Option<String> {
                let input = self.child.stdin.as_mut()?;
                writeln!(input, "exec {}", program.display()).ok()?;
                input.flush().ok()?;
                self.line()
            }
        }

        impl Drop for Process {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }

    /// A dynamic validation that returns late: the stand-in for a Security
    /// call that cannot be interrupted. The validation itself has already
    /// run and its answer is the truthful one; a hold only keeps that answer
    /// back, so what a test sees is a LATE answer, never a changed one.
    ///
    /// A hold names the validations it keeps by a predicate over the
    /// validated pid, evaluated on the validating thread — so a test can
    /// name "this process, on the receipt worker" or "any other process, on
    /// my own thread" — and nothing of another test, whose processes and
    /// threads are its own.
    pub(crate) mod hold {
        use crate::sync::{wait_timeout_or_recover, LockRecover};
        use std::sync::{Arc, Condvar, Mutex};
        use std::time::{Duration, Instant};

        /// How long a kept answer stays back.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub(crate) enum Until {
            /// Until the test releases the hold, or drops it.
            Released,
            /// Until the validating thread's own deadline has passed (a
            /// thread without a deadline is not held), or the hold is
            /// released.
            DeadlinePassed,
        }

        #[derive(Default)]
        struct State {
            open: bool,
            entered: usize,
            /// The processes whose validations reached this hold, in order.
            pids: Vec<u32>,
        }

        struct Spec {
            matches: Box<dyn Fn(u32) -> bool + Send + Sync>,
            until: Until,
            state: Mutex<State>,
            changed: Condvar,
        }

        static HOLDS: Mutex<Vec<Arc<Spec>>> = Mutex::new(Vec::new());

        /// One installed hold. Dropping it — a failed assertion included —
        /// releases every answer it keeps and removes it.
        pub(crate) struct Hold(Arc<Spec>);

        /// Keep back the answer of every dynamic validation of a pid that
        /// `matches`, from now on.
        pub(crate) fn install(
            matches: impl Fn(u32) -> bool + Send + Sync + 'static,
            until: Until,
        ) -> Hold {
            let spec = Arc::new(Spec {
                matches: Box::new(matches),
                until,
                state: Mutex::new(State::default()),
                changed: Condvar::new(),
            });
            HOLDS.lock_or_recover().push(spec.clone());
            Hold(spec)
        }

        impl Hold {
            /// How many validations reached this hold so far.
            pub(crate) fn entered(&self) -> usize {
                self.0.state.lock_or_recover().entered
            }

            /// The processes whose validations reached this hold, in order.
            pub(crate) fn pids(&self) -> Vec<u32> {
                self.0.state.lock_or_recover().pids.clone()
            }

            /// Wait until `count` validations reached this hold; `false`
            /// when `limit` passed first.
            pub(crate) fn wait_entered(&self, count: usize, limit: Duration) -> bool {
                let end = Instant::now() + limit;
                let mut state = self.0.state.lock_or_recover();
                while state.entered < count {
                    let left = end.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return false;
                    }
                    state = wait_timeout_or_recover(&self.0.changed, state, left);
                }
                true
            }

            /// Let every kept answer return, now and from here on.
            pub(crate) fn release(&self) {
                self.0.state.lock_or_recover().open = true;
                self.0.changed.notify_all();
            }
        }

        impl Drop for Hold {
            fn drop(&mut self) {
                self.release();
                HOLDS
                    .lock_or_recover()
                    .retain(|spec| !Arc::ptr_eq(spec, &self.0));
            }
        }

        /// Called with a validation's answer in hand, before it is returned.
        pub(in crate::procinfo::code_identity) fn keep_back(pid: u32) {
            let holds: Vec<Arc<Spec>> = HOLDS
                .lock_or_recover()
                .iter()
                .filter(|spec| (spec.matches)(pid))
                .cloned()
                .collect();
            for spec in holds {
                let mut state = spec.state.lock_or_recover();
                state.entered += 1;
                state.pids.push(pid);
                spec.changed.notify_all();
                while !state.open
                    && (spec.until == Until::Released
                        || (crate::session_runtime::deadline_active()
                            && crate::session_runtime::check_deadline().is_ok()))
                {
                    state = wait_timeout_or_recover(&spec.changed, state, Duration::from_millis(5));
                }
            }
        }
    }

    use fixture::{build, directory, product, Process, IDENTIFIER, OTHER_IDENTIFIER};

    fn valid(path: &Path, product: &ProductIdentity) -> CodeIdentity {
        match on_disk(path, product) {
            OnDisk::Valid(code) => code,
            other => panic!("the fixture validates on disk: {other:?}"),
        }
    }

    /// The positive case: an unchanged file and the process started from it.
    #[test]
    fn a_running_image_is_bound_to_the_unchanged_file_it_was_started_from() {
        let dir = directory("bound");
        let program = dir.join("client");
        build(&program, "build-A", &[], IDENTIFIER);
        let test = product(IDENTIFIER);
        let code = valid(&program, test);
        let (process, tag) = Process::start(&program, None).expect("the fixture runs");
        assert_eq!(tag, "build-A");
        assert_eq!(running_product(process.pid(), test), Running::Is);
        assert_eq!(running(process.pid(), &code), Running::Is);
        // another product, or another code directory of the same product
        assert_eq!(
            running_product(process.pid(), product(OTHER_IDENTIFIER)),
            Running::IsNot
        );
        let other = dir.join("other-build");
        build(&other, "build-B", &[], IDENTIFIER);
        let other_code = valid(&other, test);
        assert!(other_code != code, "two builds, two code identities");
        assert_eq!(running(process.pid(), &other_code), Running::IsNot);
        assert_eq!(format!("{code:?}"), "CodeIdentity(..)");
        drop(process);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// THE counterexample: the old process still runs build A while its path
    /// already holds build B — another validly signed file of the same
    /// product. The file validates; the running image is not bound to it.
    #[test]
    fn an_old_image_is_not_bound_to_a_validly_signed_file_that_replaced_it_at_the_same_path() {
        let dir = directory("replaced");
        let program = dir.join("client");
        build(&program, "build-A", &[], IDENTIFIER);
        let test = product(IDENTIFIER);
        let code_a = valid(&program, test);
        let (process, tag) = Process::start(&program, None).expect("the fixture runs");
        assert_eq!(tag, "build-A");
        assert_eq!(running(process.pid(), &code_a), Running::Is);
        // an "update": another build is moved over the same path
        let staged = dir.join("staged");
        build(&staged, "build-B", &[], IDENTIFIER);
        std::fs::rename(&staged, &program).unwrap();
        // DISK: the path now holds valid code of the product — build B
        let code_b = valid(&program, test);
        assert!(code_b != code_a);
        // the kernel still names the same path for the old process
        assert_eq!(
            crate::procinfo::image_path(process.pid()).as_deref(),
            Some(program.as_path()),
            "the old process is still reported at the replaced path"
        );
        // RUNNING CODE: it is neither the product "as the file at its path"
        // nor build B, and build A is no longer what its path holds
        assert_eq!(running_product(process.pid(), test), Running::Diverged);
        assert_eq!(running(process.pid(), &code_b), Running::Diverged);
        assert_eq!(running(process.pid(), &code_a), Running::Diverged);
        // a process started from the new file is bound to build B only
        let (fresh, tag) = Process::start(&program, None).expect("the new build runs");
        assert_eq!(tag, "build-B");
        assert_eq!(running(fresh.pid(), &code_b), Running::Is);
        assert_eq!(running(fresh.pid(), &code_a), Running::IsNot);
        drop((process, fresh));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// One pid, two images: after an exec the pid runs build B. A reference
    /// to build A no longer binds it, whatever its pid still is.
    #[test]
    fn a_pid_that_execs_another_signed_image_is_no_longer_bound_to_the_first() {
        let dir = directory("exec");
        let (first, second) = (dir.join("first"), dir.join("second"));
        build(&first, "build-A", &[], IDENTIFIER);
        build(&second, "build-B", &[], IDENTIFIER);
        let test = product(IDENTIFIER);
        let (code_a, code_b) = (valid(&first, test), valid(&second, test));
        let (mut process, tag) = Process::start(&first, None).expect("the fixture runs");
        assert_eq!(tag, "build-A");
        let pid = process.pid();
        let birth = crate::procinfo::process_start(pid);
        assert_eq!(running(pid, &code_a), Running::Is);
        assert_eq!(process.exec(&second).as_deref(), Some("build-B"));
        assert_eq!(process.pid(), pid, "the same pid");
        // Observed, not relied on: the birth instant survives the exec, so
        // pid and birth alone cannot tell the two images apart.
        println!(
            "exec kept the birth instant: {}",
            crate::procinfo::process_start(pid) == birth
        );
        assert_eq!(running(pid, &code_a), Running::IsNot);
        assert_eq!(running(pid, &code_b), Running::Is);
        // both are the product: only the pinned code directory tells them apart
        assert_eq!(running_product(pid, test), Running::Is);
        drop(process);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A file removed, or rewritten in place under the same inode, no longer
    /// holds what the process runs.
    #[test]
    fn a_removed_or_rewritten_file_unbinds_the_process_that_runs_it() {
        let dir = directory("rewritten");
        let test = product(IDENTIFIER);
        let other = dir.join("other-build");
        build(&other, "build-B", &[], IDENTIFIER);
        // rewritten in place: the same inode, another validly signed content
        let program = dir.join("client");
        build(&program, "build-A", &[], IDENTIFIER);
        let code = valid(&program, test);
        let (process, _) = Process::start(&program, None).expect("the fixture runs");
        let inode = |path: &Path| std::os::unix::fs::MetadataExt::ino(&path.metadata().unwrap());
        let before = inode(&program);
        {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&program)
                .unwrap();
            file.write_all(&std::fs::read(&other).unwrap()).unwrap();
        }
        assert_eq!(inode(&program), before, "rewritten in place");
        assert_eq!(running(process.pid(), &code), Running::Diverged);
        assert_eq!(running_product(process.pid(), test), Running::Diverged);
        drop(process);
        // removed: no file is left to hold what the process runs
        let gone = dir.join("gone");
        build(&gone, "build-A", &[], IDENTIFIER);
        let code = valid(&gone, test);
        let (process, _) = Process::start(&gone, None).expect("the fixture runs");
        std::fs::remove_file(&gone).unwrap();
        assert_ne!(running(process.pid(), &code), Running::Is);
        assert_ne!(running_product(process.pid(), test), Running::Is);
        drop(process);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// What a file must be to validate: signed, of this product, with every
    /// page as signed and nothing after the signature.
    #[test]
    fn only_intact_signed_code_of_the_product_validates_on_disk() {
        let dir = directory("static");
        let test = product(IDENTIFIER);
        let good = dir.join("good");
        build(&good, "build-A", &[], IDENTIFIER);
        assert!(matches!(on_disk(&good, test), OnDisk::Valid(_)));
        // another product of the same (absent) signer
        assert_eq!(on_disk(&good, product(OTHER_IDENTIFIER)), OnDisk::Invalid);
        let other = dir.join("other-product");
        build(&other, "build-A", &[], OTHER_IDENTIFIER);
        assert_eq!(on_disk(&other, test), OnDisk::Invalid);
        // no signature at all
        let unsigned = dir.join("unsigned");
        std::fs::copy(&good, &unsigned).unwrap();
        fixture::unsign(&unsigned);
        assert_eq!(on_disk(&unsigned, test), OnDisk::Invalid);
        // one byte of a signed page changed: only the full validation sees it
        let corrupt = dir.join("corrupt");
        std::fs::copy(&good, &corrupt).unwrap();
        fixture::retag(&corrupt, b"build-", "X");
        assert_eq!(on_disk(&corrupt, test), OnDisk::Invalid);
        // bytes after the signature
        let appended = dir.join("appended");
        std::fs::copy(&good, &appended).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&appended)
            .unwrap()
            .write_all(b"another build")
            .unwrap();
        assert_eq!(on_disk(&appended, test), OnDisk::Invalid);
        // not code, not there, not a file
        std::fs::write(dir.join("text"), b"#!/bin/sh\n").unwrap();
        assert_eq!(on_disk(&dir.join("text"), test), OnDisk::Invalid);
        assert!(!matches!(
            on_disk(&dir.join("missing"), test),
            OnDisk::Valid(_)
        ));
        // a system program is validly signed by Apple — and is not this product
        assert_eq!(on_disk(Path::new("/bin/ls"), test), OnDisk::Invalid);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A universal file: every slice validates, the identity is the slice
    /// Security selects for this host, and a process that executes ANOTHER
    /// slice of the same file is not bound to it. Which slices this host can
    /// run is found out by running them; the test fails only on a wrong
    /// answer and says which cases it could exercise.
    #[test]
    fn a_universal_file_binds_only_the_process_that_runs_the_identified_slice() {
        let dir = directory("universal");
        let program = dir.join("client");
        build(&program, "build-U", &["arm64", "arm64e"], IDENTIFIER);
        let test = product(IDENTIFIER);
        let code = valid(&program, test);
        let mut answers = Vec::new();
        for slice in ["arm64", "arm64e"] {
            let Some((process, _)) = Process::start(&program, Some(slice)) else {
                println!("universal: this host does not run the {slice} slice");
                continue;
            };
            // each slice is the product as its own running code
            assert_eq!(running_product(process.pid(), test), Running::Is);
            let answer = running(process.pid(), &code);
            assert!(
                matches!(answer, Running::Is | Running::IsNot),
                "{slice}: {answer:?}"
            );
            answers.push(answer);
        }
        println!("universal: answers per runnable slice = {answers:?}");
        assert!(
            answers.iter().filter(|a| **a == Running::Is).count() <= 1,
            "one identified slice at most"
        );
        if answers.len() == 2 {
            assert_eq!(
                answers.iter().filter(|a| **a == Running::IsNot).count(),
                1,
                "the other slice is refused"
            );
        }
        assert!(!answers.is_empty(), "the native slice runs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// No answer is never a binding: a pid that names no process, and a
    /// Security call that fails.
    #[test]
    fn a_missing_process_or_an_api_failure_is_never_a_binding() {
        let dir = directory("unreadable");
        let program = dir.join("client");
        build(&program, "build-A", &[], IDENTIFIER);
        let test = product(IDENTIFIER);
        let code = valid(&program, test);
        let (process, _) = Process::start(&program, None).expect("the fixture runs");
        let pid = process.pid();
        assert_eq!(running(pid, &code), Running::Is);
        FAULT.with(|fault| fault.set(Some(Fault::Unreadable)));
        let faulted = (
            running(pid, &code),
            running_product(pid, test),
            on_disk(&program, test),
        );
        FAULT.with(|fault| fault.set(None));
        assert_eq!(
            faulted,
            (Running::Unreadable, Running::Unreadable, OnDisk::Unreadable)
        );
        drop(process);
        // the process is gone: the pid names nothing (or, later, somebody else)
        assert_ne!(running(pid, &code), Running::Is);
        assert_ne!(running_product(pid, test), Running::Is);
        assert_ne!(running(u32::MAX - 1, &code), Running::Is);
        // a requirement that does not parse answers nothing
        let malformed = ProductIdentity {
            requirement: "identifier",
        };
        assert_eq!(on_disk(&program, &malformed), OnDisk::Unreadable);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// This very test process, by its own signature: the linker's ad-hoc
    /// signature is enough for the mechanism, and `product_of` names it.
    #[test]
    fn this_process_is_bound_to_its_own_executable() {
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        let own = fixture::product_of(&exe);
        let code = valid(&exe, own);
        assert!(code.is_of(own) && !code.is_of(product(IDENTIFIER)));
        assert_eq!(running(std::process::id(), &code), Running::Is);
        assert_eq!(running_product(std::process::id(), own), Running::Is);
    }
}
