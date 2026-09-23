//! Bounded subprocess output. The caller names the exact executable and argv;
//! this module adds no shell and no PATH lookup. Output is capped and the
//! whole run has one deadline. Waiting blocks in poll(2) on the two pipes
//! (no periodic wake-up); once both reach EOF the child is exiting, and only
//! that short window is awaited with a capped backoff. A timed-out or
//! over-limit child is killed and reaped.

use std::io::{Error, ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const OUTPUT_LIMIT: usize = 64 * 1024;

pub fn output(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let result = collect(&mut child, deadline);
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn remaining(deadline: Instant) -> std::io::Result<Duration> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(Error::new(ErrorKind::TimedOut, "subprocess timeout"));
    }
    Ok(left)
}

fn collect(child: &mut Child, deadline: Instant) -> std::io::Result<Output> {
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut captured = [Vec::new(), Vec::new()];
    let mut open = [true, true];
    while open[0] || open[1] {
        let millis = remaining(deadline)?
            .as_millis()
            .clamp(1, libc::c_int::MAX as u128) as libc::c_int;
        let mut fds = [stdout.as_raw_fd(), stderr.as_raw_fd()].map(|fd| libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        });
        for (fd, open) in fds.iter_mut().zip(open) {
            if !open {
                fd.fd = -1; // poll(2) ignores negative descriptors
            }
        }
        // SAFETY: `fds` is a live array of two pollfd values.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, millis) } < 0 {
            let error = Error::last_os_error();
            if error.kind() == ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        for index in 0..2 {
            if fds[index].revents == 0 {
                continue;
            }
            let pipe: &mut dyn Read = if index == 0 { &mut stdout } else { &mut stderr };
            let mut buffer = [0u8; 8192];
            match pipe.read(&mut buffer) {
                Ok(0) => open[index] = false,
                Ok(count) => {
                    captured[index].extend_from_slice(&buffer[..count]);
                    if captured[index].len() > OUTPUT_LIMIT {
                        return Err(Error::new(
                            ErrorKind::InvalidData,
                            "subprocess output limit",
                        ));
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
    let mut pause = Duration::from_millis(1);
    loop {
        if let Some(status) = child.try_wait()? {
            let [stdout, stderr] = captured;
            return Ok(Output {
                status,
                stdout,
                stderr,
            });
        }
        std::thread::sleep(pause.min(remaining(deadline)?));
        pause = (pause * 2).min(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hung_child_is_killed_at_the_deadline() {
        let started = Instant::now();
        let error = output(
            Command::new("/bin/sleep").arg("10"),
            Duration::from_millis(40),
        )
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn noisy_child_is_killed_at_the_output_limit() {
        let error = output(&mut Command::new("/usr/bin/yes"), Duration::from_secs(2)).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::InvalidData);
    }

    #[test]
    fn output_and_status_are_collected() {
        let output = output(
            Command::new("/bin/echo").arg("fixture"),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"fixture\n");
        assert!(output.stderr.is_empty());
    }
}
