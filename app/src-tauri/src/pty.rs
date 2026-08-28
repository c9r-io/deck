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

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Condvar, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::storage::applog;
use crate::tmux::{session_target, tmux_bin, tmux_conf, SOCKET};

// ---------- ACK window ---------------------------------------------------------

/// Max un-ACKed pty-data events in flight to the webview. With batches
/// coalesced to ≤EMIT_COALESCE_MAX this bounds webview-side queueing at
/// ~1MB per attachment (plus the 512KB reader channel behind it).
pub(crate) const MAX_INFLIGHT_BATCHES: u64 = 4;

#[derive(Default)]
struct AckInner {
    acked: u64,
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
    pub(crate) fn ack(&self, seq: u64) {
        let mut g = self.inner.lock().unwrap();
        if seq > g.acked {
            g.acked = seq;
            self.cv.notify_all();
        }
    }

    /// Release any waiting emitter and mark the attachment finished —
    /// called on detach, on replacement by a newer attachment, and is the
    /// answer to "the webview will never ACK again".
    pub(crate) fn close(&self) {
        let mut g = self.inner.lock().unwrap();
        g.closed = true;
        self.cv.notify_all();
    }

    /// Block until `seq` fits in the un-ACKed window (true) or the gate is
    /// closed (false). Logs once per stall episode so a wedged webview is
    /// visible in app.log while memory stays bounded.
    pub(crate) fn admit(&self, seq: u64, window: u64) -> bool {
        let mut g = self.inner.lock().unwrap();
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
                    self.name, g.acked
                ));
            }
            g = self.cv.wait(g).unwrap();
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
) -> Result<u64, String> {
    crate::tmux::validate_session_name(&name)?;
    // replace any previous attachment for this session; closing its gate
    // releases an emitter that may be waiting on ACKs that will never come
    if let Some(mut old) = state.map.lock().unwrap().remove(&name) {
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
        .map_err(|e| e.to_string())?;

    let conf = tmux_conf();
    let mut cmd = CommandBuilder::new(tmux_bin());
    cmd.args([
        "-f",
        &conf,
        "-L",
        SOCKET,
        "attach-session",
        "-t",
        &session_target(&name),
    ]);
    cmd.env("TERM", "xterm-256color");
    cmd.env("LANG", "en_US.UTF-8");
    let child = pair.slave.spawn_command(cmd).map_err(|e| {
        // the raw error (may embed the tmux path) goes to the caller only
        let msg = e.to_string();
        applog(&format!(
            "[pty] attach spawn failed for {name} ({})",
            crate::storage::err_code(&msg)
        ));
        msg
    })?;
    drop(pair.slave);
    applog(&format!("[pty] attached {name} ({cols}x{rows})"));

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    let generation = {
        let mut c = state.counter.lock().unwrap();
        *c += 1;
        *c
    };
    let gate = AckGate::new(name.clone());
    state.map.lock().unwrap().insert(
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
        applog(&format!("[pty] reader started for {reader_name}"));
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
                        "[pty] emit #{emits} {}B to {thread_name}",
                        batch.len()
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
                    applog(&format!("[pty] first emit result: {:?}", r.is_ok()));
                }
            },
        );
        // clean up only if this attachment is still the current one
        let state = thread_app.state::<PtyState>();
        let mut map = state.map.lock().unwrap();
        if map.get(&thread_name).map(|e| e.generation) == Some(generation) {
            map.remove(&thread_name);
            drop(map);
            applog(&format!("[pty] stream ended for {thread_name}"));
            let _ = thread_app.emit("pty-exit", PtyExit { name: thread_name });
        }
    });

    Ok(generation)
}

/// Drain the channel into as-large-as-available batches: blocking recv for
/// the first chunk, then non-blocking drains until `max` bytes or the queue
/// is momentarily empty. Each batch gets the next sequence number and is
/// emitted only once the gate admits it (≤`window` un-ACKed). Returns when
/// the sender is dropped (stream end) or the gate closes (detach/replace) —
/// order-preserving, nothing dropped, nothing emitted past the window.
pub(crate) fn pump_gated<F: FnMut(u64, Vec<u8>)>(
    rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    max: usize,
    gate: &AckGate,
    window: u64,
    mut emit: F,
) {
    let mut seq: u64 = 0;
    while let Ok(first) = rx.recv() {
        let mut batch = first;
        while batch.len() < max {
            match rx.try_recv() {
                Ok(more) => batch.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        seq += 1;
        if !gate.admit(seq, window) {
            return; // closed: this attachment is over, drop the tail
        }
        emit(seq, batch);
    }
}

#[cfg(test)]
mod tests {
    use super::{pump_gated, AckGate, MAX_INFLIGHT_BATCHES};
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
        emit: F,
    ) {
        let gate = open_gate();
        pump_gated(rx, max, &gate, u64::MAX, emit);
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
    fn ack_regression_is_ignored() {
        let gate = open_gate();
        gate.ack(5);
        gate.ack(3); // out-of-order ACK must not move the window backwards
        assert!(gate.admit(5 + MAX_INFLIGHT_BATCHES, MAX_INFLIGHT_BATCHES));
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
            gate.ack(seq); // consumer keeps up
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
) -> Result<(), String> {
    let bytes = B64.decode(data_b64).map_err(|e| e.to_string())?;
    let mut map = state.map.lock().unwrap();
    let entry = map.get_mut(&name).ok_or("not attached")?;
    entry.writer.write_all(&bytes).map_err(|e| e.to_string())?;
    entry.writer.flush().map_err(|e| e.to_string())
}

#[tauri::command]
pub(crate) fn pty_resize(
    state: State<'_, PtyState>,
    name: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let map = state.map.lock().unwrap();
    let entry = map.get(&name).ok_or("not attached")?;
    entry
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
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
