//! PTY bridge: `tmux attach` inside a portable-pty, bytes streamed to the
//! webview as base64 `pty-data` events. Detach kills only the tmux client.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::storage::applog;
use crate::tmux::{session_target, tmux_bin, tmux_conf, SOCKET};

// ---------- PTY bridge (the open session) -------------------------------------

pub(crate) struct PtyEntry {
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    generation: u64,
}

#[derive(Default)]
pub(crate) struct PtyState {
    map: Mutex<HashMap<String, PtyEntry>>,
    counter: Mutex<u64>,
}

#[derive(Clone, Serialize)]
pub(crate) struct PtyData {
    name: String,
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
) -> Result<(), String> {
    crate::tmux::validate_session_name(&name)?;
    // replace any previous attachment for this session
    if let Some(mut old) = state.map.lock().unwrap().remove(&name) {
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
        applog(&format!("[pty] attach spawn failed for {name}: {e}"));
        e.to_string()
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
    state.map.lock().unwrap().insert(
        name.clone(),
        PtyEntry {
            writer,
            master: pair.master,
            child,
            generation,
        },
    );

    // Internal buffering + event coalescing between the PTY and the webview:
    // reader → sync channel → coalescing emitter. The channel caps
    // reader→emitter in-flight data at PTY_CHANNEL_CHUNKS × 8KB (a slow
    // EMITTER stalls the reader, the kernel PTY buffer fills, and the tmux
    // client blocks), and the emitter drains whatever accumulated into ONE
    // pty-data event (up to EMIT_COALESCE_MAX) so a fast producer yields few
    // large events rather than thousands of 8KB ones.
    //
    // HONEST LIMIT — this is NOT end-to-end backpressure: app.emit() is
    // fire-and-forget, so nothing bounds the Tauri→WKWebView event queue if
    // the webview itself stops consuming. Coalescing keeps the event RATE
    // low, which is what matters in practice, but a wedged webview could
    // still accumulate emitted events. A real fix needs an ACK-window
    // protocol (frontend acknowledges written bytes; emitter waits when the
    // window is exhausted) — planned as its own change, too invasive to
    // ship alongside the scheduler rework.
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
        pump_coalesced(&rx, EMIT_COALESCE_MAX, |batch| {
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
                    data: B64.encode(&batch),
                },
            );
            if emits == 1 {
                applog(&format!("[pty] first emit result: {:?}", r.is_ok()));
            }
        });
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

    Ok(())
}

/// Drain the channel into as-large-as-available batches: blocking recv for
/// the first chunk, then non-blocking drains until `max` bytes or the queue
/// is momentarily empty. Returns when the sender is dropped.
pub(crate) fn pump_coalesced<F: FnMut(Vec<u8>)>(
    rx: &std::sync::mpsc::Receiver<Vec<u8>>,
    max: usize,
    mut emit: F,
) {
    while let Ok(first) = rx.recv() {
        let mut batch = first;
        while batch.len() < max {
            match rx.try_recv() {
                Ok(more) => batch.extend_from_slice(&more),
                Err(_) => break,
            }
        }
        emit(batch);
    }
}

#[cfg(test)]
mod tests {
    use super::pump_coalesced;
    use std::sync::mpsc::sync_channel;

    #[test]
    fn coalesces_queued_chunks_into_one_emit() {
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        for i in 0u8..10 {
            tx.send(vec![i; 100]).unwrap();
        }
        drop(tx);
        let mut emits: Vec<usize> = Vec::new();
        pump_coalesced(&rx, 1 << 20, |b| emits.push(b.len()));
        assert_eq!(emits, vec![1000], "10 queued chunks → one emit");
    }

    #[test]
    fn respects_coalesce_ceiling() {
        let (tx, rx) = sync_channel::<Vec<u8>>(64);
        for _ in 0..10 {
            tx.send(vec![0u8; 100]).unwrap();
        }
        drop(tx);
        let mut emits: Vec<usize> = Vec::new();
        pump_coalesced(&rx, 250, |b| emits.push(b.len()));
        // ceiling is checked before each drain, so batches stop growing at
        // the first size ≥ max — bounded, order-preserving, nothing lost
        assert_eq!(emits.iter().sum::<usize>(), 1000);
        assert!(emits.len() > 1, "ceiling must split the stream: {emits:?}");
        assert!(emits.iter().all(|&n| n <= 300), "{emits:?}");
    }

    #[test]
    fn ends_when_sender_drops() {
        let (tx, rx) = sync_channel::<Vec<u8>>(4);
        drop(tx);
        let mut called = false;
        pump_coalesced(&rx, 1024, |_| called = true);
        assert!(!called);
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

#[tauri::command]
pub(crate) fn detach_session(state: State<'_, PtyState>, name: String) {
    if let Some(mut entry) = state.map.lock().unwrap().remove(&name) {
        let _ = entry.child.kill();
    }
}
