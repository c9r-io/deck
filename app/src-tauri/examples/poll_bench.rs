//! Poll-cost benchmark: old per-card pattern vs the batched pattern
//! poll_sessions ships, at 5 / 20 / 50 sessions on a THROWAWAY tmux server
//! (socket `deck-bench-<pid>`, never the live `deck` socket).
//!
//! Run: `cargo run --example poll_bench [--release]`
//!
//! Reference numbers (M-series MacBook, release, 2026-08): see PERF note in
//! commands.rs — the batched poll holds at 3 subprocesses per tick for any
//! session count; the old pattern was 3 + one capture-pane per visible card.

use std::process::Command;
use std::time::Instant;

fn tmux_bin() -> String {
    format!(
        "{}/binaries/tmux-aarch64-apple-darwin",
        env!("CARGO_MANIFEST_DIR")
    )
}

struct Bench(String);

impl Bench {
    fn run(&self, args: &[&str]) -> String {
        let out = Command::new(tmux_bin())
            .args(["-f", "/dev/null", "-L", &self.0])
            .args(args)
            .output()
            .expect("tmux spawn");
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = Command::new(tmux_bin())
            .args(["-L", &self.0, "kill-server"])
            .output();
    }
}

const PANE_FMT: &str = "#{session_name}\t#{pane_pid}\t#{window_activity}\t#{pane_current_command}";

fn main() {
    let b = Bench(format!("deck-bench-{}", std::process::id()));
    const ROUNDS: u32 = 10;

    println!("sessions  pattern  subprocs/poll  ms/poll (avg of {ROUNDS})");
    let mut created = 0usize;
    for n in [5usize, 20, 50] {
        while created < n {
            b.run(&[
                "new-session",
                "-d",
                "-s",
                &format!("s{created}"),
                "-x",
                "120",
                "-y",
                "30",
                "/bin/sh",
            ]);
            created += 1;
        }
        let names: Vec<String> = (0..n).map(|i| format!("s{i}")).collect();

        // old pattern: list-sessions + list-panes + capture-pane per session
        let t = Instant::now();
        for _ in 0..ROUNDS {
            b.run(&["list-sessions", "-F", "#{session_name}"]);
            b.run(&["list-panes", "-a", "-F", PANE_FMT]);
            for name in &names {
                b.run(&[
                    "capture-pane",
                    "-p",
                    "-t",
                    &format!("={name}:"),
                    "-S",
                    "-30",
                ]);
            }
        }
        let old_ms = t.elapsed().as_secs_f64() * 1000.0 / ROUNDS as f64;

        // new pattern: list-panes + ONE batched display/capture invocation
        // (poll_sessions additionally caps captures at 16)
        let mut batch: Vec<String> = Vec::new();
        for (i, name) in names.iter().take(16).enumerate() {
            if i > 0 {
                batch.push(";".into());
            }
            batch.push("display-message".into());
            batch.push("-p".into());
            batch.push(format!("\u{1}deck-tail\u{1}{name}"));
            batch.push(";".into());
            batch.push("capture-pane".into());
            batch.push("-p".into());
            batch.push("-t".into());
            batch.push(format!("={name}:"));
            batch.push("-S".into());
            batch.push("-30".into());
        }
        let batch_ref: Vec<&str> = batch.iter().map(|s| s.as_str()).collect();
        let t = Instant::now();
        for _ in 0..ROUNDS {
            b.run(&["list-panes", "-a", "-F", PANE_FMT]);
            b.run(&batch_ref);
        }
        let new_ms = t.elapsed().as_secs_f64() * 1000.0 / ROUNDS as f64;

        println!("{n:>8}  old      {:>13}  {old_ms:>7.1}", 2 + n);
        println!("{n:>8}  batched  {:>13}  {new_ms:>7.1}", 2);
    }
}
