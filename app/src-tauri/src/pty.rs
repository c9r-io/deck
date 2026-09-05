//! PTY bridge: `tmux attach` inside a portable-pty, bytes streamed to the
//! webview as base64 `pty-data` events. Detach kills only the tmux client.
//!
//! End-to-end flow control: every event carries the attachment's generation
//! and a monotonically increasing sequence number; the webview ACKs each
//! sequence after xterm has actually written the bytes (`pty_ack`). The
//! emitter never has more than MAX_INFLIGHT_BATCHES un-ACKed events
//! outstanding — past that it WAITS instead of calling app.emit, so a slow
//! or wedged webview stalls the emitter → the bounded channel fills → the
//! reader blocks → the kernel PTY buffer fills → the tmux client stalls.
//! Bytes are never dropped or reordered, and a stale generation's tail is
//! discarded by the frontend (gen mismatch) without being ACKed — its gate
//! is closed by the detach/re-attach that replaced it, which is also what
//! releases a waiting emitter.
//!
//! The gate tracks an emitted HIGH-WATER mark: an ACK is honored only for
//! `acked < seq <= emitted` on an open gate, so a buggy or hostile webview
//! ACKing sequences that were never sent cannot widen the window. A failed
//! app.emit ends the pump and closes the gate (the webview can never ACK an
//! event it never received); sequence overflow ends the stream cleanly.
//!
//! # Contract
//! Attach = `tmux attach` inside a portable-pty, bytes streamed as base64 over the
//! `pty-data` event to xterm.js; detach kills only the tmux *client*. Reader threads
//! carry a generation counter so a stale thread never removes a newer attachment.
//! Flow control is END-TO-END: pty-data events carry `gen`+`seq`; the frontend
//! ACKs (`pty_ack`) only after xterm's write callback, and the emitter never
//! runs more than MAX_INFLIGHT_BATCHES (4 × ≤256KB) past the last ACK — past
//! that it waits on the attachment's AckGate (closed by detach/re-attach, which
//! is what releases a stalled emitter; stalls are logged). The gate tracks an
//! emitted HIGH-WATER mark: an ACK counts only for acked < seq ≤ emitted on an
//! open gate, so a buggy/hostile webview ACKing sequences never sent cannot
//! widen the window; a failed app.emit ends the pump and closes the gate (the
//! webview can never ACK an event it never received); seq overflow ends the
//! stream cleanly. A wedged webview
//! therefore stalls emitter → bounded channel → kernel PTY → tmux client, with
//! memory bounded at ~1.5MB per attachment. The frontend drops (without ACKing)
//! events whose gen is older than the current attachment, and accepts+adopts a
//! NEWER gen (the first event can beat the attach invoke's resolution).

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::applog::applog;
use crate::error::{DeckError, ErrorKind};
use crate::sync::LockRecover;
use crate::tmux::{session_target, socket, tmux_bin, tmux_conf};

// ---------- ACK window ---------------------------------------------------------

/// Max un-ACKed pty-data events in flight to the webview. With batches
/// coalesced to ≤EMIT_COALESCE_MAX this bounds webview-side queueing at
/// ~1MB per attachment (plus the 512KB reader channel behind it).
pub(crate) const MAX_INFLIGHT_BATCHES: u64 = 4;

#[derive(Default)]
struct AckInner {
    acked: u64,
    /// high-water mark: the last sequence actually handed to the webview.
    /// ACKs are only valid up to here — the window can never be widened by
    /// acknowledging events that were never sent.
    emitted: u64,
    closed: bool,
}

/// The emitter⇄webview flow-control gate for ONE attachment. `ack` and
/// `close` are cheap (their own mutex + condvar, never the PtyState map
/// lock), so acknowledging can never block input, resize or detach.
pub(crate) struct AckGate {
    name: String,
    inner: Mutex<AckInner>,
    cv: Condvar,
}

impl AckGate {
    pub(crate) fn new(name: String) -> Arc<Self> {
        Arc::new(AckGate {
            name,
            inner: Mutex::new(AckInner::default()),
            cv: Condvar::new(),
        })
    }

    /// Frontend confirmed everything up to `seq` was written to xterm.
    /// Valid only when `acked < seq <= emitted` on an open gate: a FUTURE
    /// ack (buggy or hostile webview acking a seq that was never emitted),
    /// a regressed ack, a duplicate, or an ack after close is ignored and
    /// can never expand the in-flight window past MAX_INFLIGHT_BATCHES.
    pub(crate) fn ack(&self, seq: u64) {
        let mut g = self.inner.lock_or_recover();
        if !g.closed && seq > g.acked && seq <= g.emitted {
            g.acked = seq;
            self.cv.notify_all();
        }
    }

    /// Register `seq` as handed to the webview. MUST run before the event
    /// becomes visible to the frontend, so a fast ACK is never rejected as
    /// "future" — pump_gated calls this between admit() and the emit.
    pub(crate) fn mark_emitted(&self, seq: u64) {
        let mut g = self.inner.lock_or_recover();
        if seq > g.emitted {
            g.emitted = seq;
        }
    }

    /// (acked, emitted, closed) — test observability.
    #[cfg(test)]
    pub(crate) fn state(&self) -> (u64, u64, bool) {
        let g = self.inner.lock_or_recover();
        (g.acked, g.emitted, g.closed)
    }

    /// Release any waiting emitter and mark the attachment finished —
    /// called on detach, on replacement by a newer attachment, and is the
    /// answer to "the webview will never ACK again".
    pub(crate) fn close(&self) {
        let mut g = self.inner.lock_or_recover();
        g.closed = true;
        self.cv.notify_all();
    }

    /// Block until `seq` fits in the un-ACKed window (true) or the gate is
    /// closed (false). Logs once per stall episode so a wedged webview is
    /// visible in app.log while memory stays bounded.
    pub(crate) fn admit(&self, seq: u64, window: u64) -> bool {
        let mut g = self.inner.lock_or_recover();
        let mut stalled = false;
        loop {
            if g.closed {
                return false;
            }
            if seq <= g.acked.saturating_add(window) {
                return true;
            }
            if !stalled {
                stalled = true;
                applog(&format!(
                    "[pty] ack stall for {}: seq {seq} waiting on ack {} (webview not consuming)",
                    crate::applog::session_tag(&self.name),
                    g.acked
                ));
            }
            g = crate::sync::wait_or_recover(&self.cv, g);
        }
    }
}

// ---------- PTY bridge (the open session) -------------------------------------

pub(crate) struct PtyEntry {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    generation: u64,
    gate: Arc<AckGate>,
}

#[derive(Default)]
pub(crate) struct PtyState {
    map: Mutex<HashMap<String, PtyEntry>>,
    counter: Mutex<u64>,
}

impl PtyState {
    /// Detach every GUI tmux client before an intentional server restart.
    /// The tmux sessions still belong to the server until the lifecycle
    /// transaction kills it; this only prevents stale PTY-exit events and
    /// releases all ACK waiters.
    pub(crate) fn detach_all(&self) {
        let mut entries = self.map.lock_or_recover();
        for (_, mut entry) in entries.drain() {
            entry.gate.close();
            let _ = entry.child.kill();
        }
    }
}

#[derive(Clone, Serialize)]
pub(crate) struct PtyData {
    name: String,
    gen: u64,
    seq: u64,
    data: String, // base64
}

#[derive(Clone, Serialize)]
pub(crate) struct PtyExit {
    name: String,
}

/// Attach = subscribe to the session's byte stream. The tmux session keeps
/// running whether or not anyone is attached; detach just closes the stream.
#[tauri::command]
pub(crate) fn attach_session(
    app: AppHandle,
    state: State<'_, PtyState>,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<u64, DeckError> {
    crate::tmux::validate_session_name(&name)?;
    // replace any previous attachment for this session; closing its gate
    // releases an emitter that may be waiting on ACKs that will never come
    if let Some(mut old) = state.map.lock_or_recover().remove(&name) {
        old.gate.close();
        let _ = old.child.kill();
    }

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| DeckError::classified(e.to_string()))?;

    let conf = tmux_conf();
    let mut cmd = CommandBuilder::new(tmux_bin());
    cmd.args([
        "-f",
        &conf,
        "-L",
        socket(),
        "attach-session",
        "-t",
        &session_target(&name),
    ]);
    cmd.env("TERM", "xterm-256color");
    cmd.env("LANG", "en_US.UTF-8");
    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        // the raw error (may embed the tmux path) goes to the caller only
        let error = DeckError::classified(e.to_string());
        applog(&format!(
            "[pty] attach spawn failed for {} ({})",
            crate::applog::session_tag(&name),
            error.code()
        ));
        error
    })?;
    drop(pair.slave);
    let attached_at = std::time::Instant::now();
    applog(&format!(
        "[pty] attached {} ({cols}x{rows})",
        crate::applog::session_tag(&name)
    ));

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| DeckError::classified(e.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| DeckError::classified(e.to_string()))?;

    let generation = {
        let mut c = state.counter.lock_or_recover();
        *c += 1;
        *c
    };
    let gate = AckGate::new(name.clone());
    state.map.lock_or_recover().insert(
        name.clone(),
        PtyEntry {
            writer,
            master: pair.master,
            child,
            generation,
            gate: gate.clone(),
        },
    );

    // Pipeline: reader → bounded sync channel → coalescing, ACK-gated
    // emitter (see module docs). The channel caps reader→emitter in-flight
    // data at PTY_CHANNEL_CHUNKS × 8KB; the emitter drains whatever
    // accumulated into ONE pty-data event (up to EMIT_COALESCE_MAX) so a
    // fast producer yields few large events, and never runs more than
    // MAX_INFLIGHT_BATCHES ahead of the webview's ACKs.
    const PTY_CHANNEL_CHUNKS: usize = 64; // × 8KB = 512KB bound
    const EMIT_COALESCE_MAX: usize = 256 * 1024;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(PTY_CHANNEL_CHUNKS);

    let reader_name = name.clone();
    std::thread::spawn(move || {
        applog(&format!(
            "[pty] reader started for {}",
            crate::applog::session_tag(&reader_name)
        ));
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break; // emitter gone
                    }
                }
            }
        }
        // dropping tx ends the emitter's recv loop
    });

    let thread_app = app.clone();
    let thread_name = name.clone();
    std::thread::spawn(move || {
        let mut emits: u64 = 0;
        pump_gated(
            &rx,
            EMIT_COALESCE_MAX,
            &gate,
            MAX_INFLIGHT_BATCHES,
            |seq, batch| {
                emits += 1;
                if emits <= 3 || emits.is_multiple_of(200) {
                    applog(&format!(
                        "[pty] emit #{emits} {}B to {}",
                        batch.len(),
                        crate::applog::session_tag(&thread_name)
                    ));
                }
                let r = thread_app.emit(
                    "pty-data",
                    PtyData {
                        name: thread_name.clone(),
                        gen: generation,
                        seq,
                        data: B64.encode(&batch),
                    },
                );
                if emits == 1 {
                    // time from attach to the first byte the webview sees:
                    // for a fresh pane this is the login shell's startup
                    applog(&format!(
                        "[pty] first emit result: {:?} after {}ms",
                        r.is_ok(),
                        attached_at.elapsed().as_millis()
                    ));
                }
                // a failed emit ends the pump (see pump_gated) — the webview
                // can never ACK an event it never received
                r.map_err(|e| DeckError::classified(e.to_string()))
            },
        );
        // clean up only if this attachment is still the current one
        let state = thread_app.state::<PtyState>();
        let mut map = state.map.lock_or_recover();
        if map.get(&thread_name).map(|e| e.generation) == Some(generation) {
            map.remove(&thread_name);
            drop(map);
            applog(&format!(
                "[pty] stream ended for {}",
                crate::applog::session_tag(&thread_name)
            ));
            let _ = thread_app.emit("pty-exit", PtyExit { name: thread_name });
        }
    });

    Ok(generation)
}

/// Drain the channel into as-large-as-available batches: blocking recv for
/// the first chunk, then wait across a short idle gap while a terminal repaint
/// is still arriving. tmux full-frame redraws commonly cross the reader's 8KB
/// boundary; emitting the clear-prefix separately lets xterm visibly paint a
/// half-frame and looks like flicker during selection and scrolling. The 2ms
/// burst window keeps one repaint atomic without perceptible input latency.
/// Each batch gets the next sequence number and is
/// emitted only once the gate admits it (≤`window` un-ACKed); the sequence
/// is registered as emitted (high-water) BEFORE the emit so the webview's
/// ACK is always valid. Returns when the sender is dropped (stream end),
/// the gate closes (detach/replace), or an emit FAILS — a webview that
/// never received an event can never ACK it, so continuing would deadlock;
/// the failure closes the gate (which also lets the reader thread die via
/// the dropped channel) and logs a classified, content-free diagnostic.
/// Order-preserving, nothing dropped, nothing emitted past the window.
pub(crate) fn pump_gated<F: FnMut(u64, Vec<u8>) -> Result<(), DeckError>>(
    rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    max: usize,
    gate: &AckGate,
    window: u64,
    mut emit: F,
) {
    const BURST_IDLE: Duration = Duration::from_millis(2);
    let mut seq: u64 = 0;
    while let Ok(first) = rx.recv() {
        let mut batch = first;
        while batch.len() < max {
            match rx.recv_timeout(BURST_IDLE) {
                Ok(more) => batch.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        // explicit overflow handling — 2^64 batches is unreachable in any
        // real stream, but wrap-around must end the stream cleanly rather
        // than rely on a debug-build panic (or silently reuse seq 0)
        seq = match seq.checked_add(1) {
            Some(s) => s,
            None => {
                gate.close();
                return;
            }
        };
        if !gate.admit(seq, window) {
            return; // closed: this attachment is over, drop the tail
        }
        gate.mark_emitted(seq);
        if let Err(e) = emit(seq, batch) {
            applog(&format!("[pty] emit failed ({}) — ending stream", e.code()));
            gate.close();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{pump_gated, AckGate, MAX_INFLIGHT_BATCHES};
    use crate::error::DeckError;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc::sync_channel;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// A gate that ACKs everything instantly = pure coalescing behavior.
    fn open_gate() -> Arc<AckGate> {
        AckGate::new("t".into())
    }
    fn pump_free<F: FnMut(u64, Vec<u8>)>(
        rx: &std::sync::mpsc::Receiver<Vec<u8>>,
        max: usize,
        mut emit: F,
    ) {
        let gate = open_gate();
        pump_gated(rx, max, &gate, u64::MAX, |s, b| {
            emit(s, b);
            Ok(())
        });
    }

    #[test]
    fn coalesces_queued_chunks_into_one_emit() {
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        for i in 0u8..10 {
            tx.send(vec![i; 100]).unwrap();
        }
        drop(tx);
        let mut emits: Vec<usize> = Vec::new();
        pump_free(&rx, 1 << 20, |_, b| emits.push(b.len()));
        assert_eq!(emits, vec![1000], "10 queued chunks → one emit");
    }

    #[test]
    fn respects_coalesce_ceiling_and_sequences_batches() {
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        for _ in 0..10 {
            tx.send(vec![0u8; 100]).unwrap();
        }
        drop(tx);
        let mut emits: Vec<(u64, usize)> = Vec::new();
        pump_free(&rx, 250, |s, b| emits.push((s, b.len())));
        // ceiling is checked before each drain, so batches stop growing at
        // the first size ≥ max — bounded, order-preserving, nothing lost
        assert_eq!(emits.iter().map(|&(_, n)| n).sum::<usize>(), 1000);
        assert!(emits.len() > 1, "ceiling must split the stream: {emits:?}");
        assert!(emits.iter().all(|&(_, n)| n <= 300), "{emits:?}");
        let seqs: Vec<u64> = emits.iter().map(|&(s, _)| s).collect();
        assert_eq!(seqs, (1..=emits.len() as u64).collect::<Vec<_>>());
    }

    #[test]
    fn ends_when_sender_drops() {
        let (tx, rx) = sync_channel::<Vec<u8>>(4);
        drop(tx);
        let mut called = false;
        pump_free(&rx, 1024, |_, _| called = true);
        assert!(!called);
    }

    #[test]
    fn slow_consumer_keeps_inflight_bounded_and_ack_resumes() {
        let gate = open_gate();
        let acked = Arc::new(AtomicU64::new(0));
        let max_inflight = Arc::new(AtomicU64::new(0));
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        let g2 = gate.clone();
        let (acked2, maxi2) = (acked.clone(), max_inflight.clone());
        let consumer_gate = gate.clone();
        let h = std::thread::spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            pump_gated(&rx, 512, &g2, MAX_INFLIGHT_BATCHES, |seq, b| {
                let inflight = seq - acked2.load(Ordering::SeqCst);
                maxi2.fetch_max(inflight, Ordering::SeqCst);
                out.extend_from_slice(&b);
                // consumer ACKs ~5ms behind, from "another thread" timing-wise
                let a = acked2.clone();
                let cg = consumer_gate.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(5));
                    a.fetch_max(seq, Ordering::SeqCst); // delayed threads may race
                    cg.ack(seq);
                });
                Ok(())
            });
            out
        });
        let payload: Vec<u8> = (0..60_000u32).map(|i| (i % 251) as u8).collect();
        for chunk in payload.chunks(300) {
            tx.send(chunk.to_vec()).unwrap();
        }
        drop(tx);
        let out = h.join().unwrap();
        assert_eq!(out, payload, "no loss, no reorder");
        assert!(
            max_inflight.load(Ordering::SeqCst) <= MAX_INFLIGHT_BATCHES,
            "window respected: {}",
            max_inflight.load(Ordering::SeqCst)
        );
        assert!(
            acked.load(Ordering::SeqCst) > 0,
            "acks actually resumed the pump"
        );
    }

    #[test]
    fn no_ack_stalls_after_window_memory_stays_bounded() {
        let gate = open_gate();
        let emitted = Arc::new(AtomicU64::new(0));
        let (tx, rx) = sync_channel::<Vec<u8>>(4);
        let (g2, e2) = (gate.clone(), emitted.clone());
        let h = std::thread::spawn(move || {
            pump_gated(&rx, 64, &g2, MAX_INFLIGHT_BATCHES, |_, _| {
                e2.fetch_add(1, Ordering::SeqCst);
                Ok(())
            });
        });
        // feed far more than the window; consumer never ACKs
        let feeder = std::thread::spawn(move || {
            for _ in 0..100 {
                if tx.send(vec![0u8; 64]).is_err() {
                    break;
                }
            }
            // keep tx alive long enough that "pump ended" can't explain the stop
            std::thread::sleep(Duration::from_millis(300));
        });
        std::thread::sleep(Duration::from_millis(200));
        let n = emitted.load(Ordering::SeqCst);
        assert!(
            n <= MAX_INFLIGHT_BATCHES,
            "emitter must stop at the window without ACKs, emitted {n}"
        );
        // detach path: closing the gate releases the stalled emitter promptly
        let t0 = Instant::now();
        gate.close();
        feeder.join().unwrap();
        h.join().unwrap();
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "close() must unblock the emitter"
        );
    }

    #[test]
    fn close_releases_a_waiting_admit() {
        let gate = open_gate();
        let g2 = gate.clone();
        let h = std::thread::spawn(move || g2.admit(10, 2)); // way past the window
        std::thread::sleep(Duration::from_millis(50));
        gate.close();
        assert!(
            !h.join().unwrap(),
            "closed admit reports the stream is over"
        );
    }

    #[test]
    fn stale_generation_gate_ignores_acks_after_close() {
        // replacement closed the old gate; a late ACK routed to it (or a
        // direct call) must not resurrect anything
        let gate = open_gate();
        gate.close();
        gate.ack(99);
        assert!(!gate.admit(1, MAX_INFLIGHT_BATCHES), "closed stays closed");
    }

    #[test]
    fn ack_regression_and_duplicates_are_ignored() {
        let gate = open_gate();
        gate.mark_emitted(5);
        gate.ack(5);
        assert_eq!(gate.state().0, 5);
        gate.ack(3); // out-of-order ACK must not move the window backwards
        assert_eq!(gate.state().0, 5);
        gate.ack(5); // duplicate: no change
        assert_eq!(gate.state().0, 5);
        assert!(gate.admit(5 + MAX_INFLIGHT_BATCHES, MAX_INFLIGHT_BATCHES));
    }

    /// The round-3 exploit this closes: a buggy/hostile webview ACKing a
    /// gigantic seq used to open the window permanently. Now an ACK is only
    /// valid up to the emitted high-water mark.
    #[test]
    fn future_ack_never_widens_the_window() {
        let gate = open_gate();
        gate.mark_emitted(2); // two events actually sent
        gate.ack(u64::MAX); // hostile ACK far past anything emitted
        gate.ack(3); // even one-past-emitted is refused
        assert_eq!(gate.state().0, 0, "future ACKs ignored entirely");
        // the emitter therefore still stalls exactly at the window
        let g2 = gate.clone();
        let h =
            std::thread::spawn(move || g2.admit(MAX_INFLIGHT_BATCHES + 1, MAX_INFLIGHT_BATCHES));
        std::thread::sleep(Duration::from_millis(50));
        assert!(!h.is_finished(), "window must NOT have been widened");
        gate.close();
        assert!(!h.join().unwrap());
    }

    /// An emit failure must end the pump: the webview never received the
    /// event, so it can never ACK it — waiting would deadlock forever.
    #[test]
    fn emit_failure_ends_pump_and_closes_gate() {
        let gate = open_gate();
        let (tx, rx) = sync_channel::<Vec<u8>>(8);
        for _ in 0..8 {
            tx.send(vec![0u8; 16]).unwrap();
        }
        // keep the sender alive: "input exhausted" cannot explain the return
        let mut calls = 0u32;
        pump_gated(&rx, 16, &gate, MAX_INFLIGHT_BATCHES, |_, _| {
            calls += 1;
            Err(DeckError::classified("event bus broken"))
        });
        assert_eq!(calls, 1, "pump stops at the first failed emit");
        assert!(gate.state().2, "gate closed so nothing can wait forever");
        drop(tx);
    }

    /// mark_emitted runs before the emit, so an ACK arriving DURING the
    /// emit callback (the realistic fast-webview race) is always valid.
    #[test]
    fn ack_during_emit_is_valid() {
        let gate = open_gate();
        let (tx, rx) = sync_channel::<Vec<u8>>(4);
        tx.send(vec![1u8; 8]).unwrap();
        drop(tx);
        let g = gate.clone();
        pump_gated(&rx, 64, &gate, MAX_INFLIGHT_BATCHES, |seq, _| {
            g.ack(seq); // webview ACKs while emit is still on the stack
            Ok(())
        });
        assert_eq!(gate.state(), (1, 1, false));
    }

    /// Stress: 8MB through the full pump with an ACKing consumer thread.
    /// Asserts integrity + boundedness and logs throughput for the record.
    #[test]
    fn stress_throughput_with_acking_consumer() {
        let gate = open_gate();
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        const TOTAL: usize = 8 * 1024 * 1024;
        let feeder = std::thread::spawn(move || {
            let chunk: Vec<u8> = (0..8192u32).map(|i| (i % 249) as u8).collect();
            let mut sent = 0;
            while sent < TOTAL {
                tx.send(chunk.clone()).unwrap();
                sent += chunk.len();
            }
            sent
        });
        let g2 = gate.clone();
        let t0 = Instant::now();
        let mut bytes = 0usize;
        let mut batches = 0u64;
        let mut max_batch = 0usize;
        pump_gated(&rx, 256 * 1024, &g2, MAX_INFLIGHT_BATCHES, |seq, b| {
            bytes += b.len();
            batches += 1;
            max_batch = max_batch.max(b.len());
            gate.ack(seq); // consumer keeps up (valid: emitted was marked)
            Ok(())
        });
        let sent = feeder.join().unwrap();
        let el = t0.elapsed();
        assert_eq!(bytes, sent, "every byte accounted for");
        assert!(max_batch <= 256 * 1024 + 8192, "coalesce ceiling held");
        // 8MB should stream in well under a second on any dev machine; the
        // bound is generous so CI noise can't flake it, but a deadlock or
        // per-batch sleep would blow straight past it
        assert!(el < Duration::from_secs(10), "throughput collapsed: {el:?}");
        println!(
            "stress: {}MB in {:?} ({} batches, max {}KB)",
            bytes / (1024 * 1024),
            el,
            batches,
            max_batch / 1024
        );
    }
}

#[tauri::command]
pub(crate) fn pty_write(
    state: State<'_, PtyState>,
    name: String,
    data_b64: String,
) -> Result<(), DeckError> {
    let bytes = B64
        .decode(data_b64)
        .map_err(|e| DeckError::classified(e.to_string()))?;
    let mut map = state.map.lock().unwrap();
    let entry = map
        .get_mut(&name)
        .ok_or(DeckError::new(ErrorKind::Other, "not attached"))?;
    entry
        .writer
        .write_all(&bytes)
        .map_err(|e| DeckError::classified(e.to_string()))?;
    entry
        .writer
        .flush()
        .map_err(|e| DeckError::classified(e.to_string()))
}

#[tauri::command]
pub(crate) fn pty_resize(
    state: State<'_, PtyState>,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<(), DeckError> {
    // tmux reflows the pane synchronously when the PTY changes size. Keep
    // that reflow out of the status -> capture row -> cursor movement window
    // used by terminal selection commands.
    let _selection_operation = crate::terminal::terminal_selection_operation_lock()
        .lock()
        .unwrap();
    let map = state.map.lock().unwrap();
    let entry = map
        .get(&name)
        .ok_or(DeckError::new(ErrorKind::Other, "not attached"))?;
    entry
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| DeckError::classified(e.to_string()))
}

/// Webview confirmation that everything up to `seq` of attachment `gen`
/// was written into xterm. Holds the map lock only to clone the gate Arc —
/// the actual wake-up runs on the gate's own mutex, so this can never block
/// pty_write / pty_resize / detach.
#[tauri::command]
pub(crate) fn pty_ack(state: State<'_, PtyState>, name: String, gen: u64, seq: u64) {
    let gate = {
        let map = state.map.lock().unwrap();
        match map.get(&name) {
            Some(e) if e.generation == gen => Some(e.gate.clone()),
            _ => None, // stale generation / already detached: its gate was closed
        }
    };
    if let Some(g) = gate {
        g.ack(seq);
    }
}

#[tauri::command]
pub(crate) fn detach_session(state: State<'_, PtyState>, name: String) {
    if let Some(mut entry) = state.map.lock().unwrap().remove(&name) {
        entry.gate.close(); // release an emitter waiting on ACKs
        let _ = entry.child.kill();
    }
}
