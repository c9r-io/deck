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

    let thread_app = app.clone();
    let thread_name = name.clone();
    std::thread::spawn(move || {
        applog(&format!("[pty] reader started for {thread_name}"));
        let mut buf = [0u8; 8192];
        let mut chunks: u64 = 0;
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    chunks += 1;
                    if chunks <= 3 || chunks.is_multiple_of(200) {
                        applog(&format!(
                            "[pty] read chunk #{chunks} {n}B from {thread_name}"
                        ));
                    }
                    let r = thread_app.emit(
                        "pty-data",
                        PtyData {
                            name: thread_name.clone(),
                            data: B64.encode(&buf[..n]),
                        },
                    );
                    if chunks == 1 {
                        applog(&format!("[pty] first emit result: {:?}", r.is_ok()));
                    }
                }
            }
        }
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
