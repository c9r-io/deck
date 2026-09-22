use std::io::{Error, ErrorKind, Read};
use std::os::fd::AsRawFd;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

pub const OUTPUT_LIMIT: usize = 64 * 1024;

pub fn output(command: &mut Command, timeout: Duration) -> std::io::Result<Output> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let result = (|| {
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return Err(Error::last_os_error());
            }
        }
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut exited = None;
        loop {
            if Instant::now() >= deadline {
                return Err(Error::new(ErrorKind::TimedOut, "subprocess timeout"));
            }
            let mut caught_up = true;
            for (pipe, bytes) in [
                (&mut stdout as &mut dyn Read, &mut out),
                (&mut stderr as &mut dyn Read, &mut err),
            ] {
                let mut buffer = [0u8; 8192];
                loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            bytes.extend_from_slice(&buffer[..count]);
                            if bytes.len() > OUTPUT_LIMIT {
                                return Err(Error::new(
                                    ErrorKind::InvalidData,
                                    "subprocess output limit",
                                ));
                            }
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            caught_up = false;
                            break;
                        }
                        Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error),
                    }
                }
            }
            if let Some(status) = exited.filter(|_| caught_up) {
                return Ok(Output {
                    status,
                    stdout: out,
                    stderr: err,
                });
            }
            exited = child.try_wait()?;
            std::thread::sleep(Duration::from_millis(2));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
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
}
