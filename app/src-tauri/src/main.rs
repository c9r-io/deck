// Prevent a console window on Windows builds; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! deck backend — Tauri assembly only. Domain logic lives in the modules:
//! tmux (server/exec), pty (attach bridge), scheduler (prompts), storage
//! (atomic persistence + logs), history (completion), commands (the rest).

mod commands;
mod history;
mod pty;
mod scheduler;
mod storage;
mod tmux;

pub(crate) use storage::applog;

use std::path::PathBuf;
use std::process::Command;
use tauri::{Emitter, Manager};

// ---------- main ---------------------------------------------------------------

fn main() {
    let deck_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deck");
    // idempotent permission migration BEFORE anything touches the data files:
    // ~/.deck → 0700, every file an older deck may have left 0644 → 0600.
    // A failure is surfaced (log + boot toast), never silently ignored.
    if let Err(e) = storage::harden_data_dir(&deck_dir) {
        storage::warn(format!("data privacy hardening incomplete: {e}"));
    }
    storage::rotate_log();
    if let Err(e) = storage::acquire_instance_lock(&deck_dir) {
        applog(&format!(
            "[boot] instance lock unavailable ({}) — exiting",
            storage::err_code(&e)
        ));
        let _ = Command::new("osascript")
            .args([
                "-e",
                "display alert \"deck is already running\" message \"Another deck instance owns this Mac's sessions. Use the running one (check the Dock).\"",
            ])
            .status();
        std::process::exit(0);
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(pty::PtyState::default())
        .manage(scheduler::Queues::new({
            let mut qs = scheduler::load_queue();
            let notes = scheduler::recover_interrupted(&mut qs);
            if !notes.is_empty() {
                for n in notes {
                    storage::warn(n);
                }
                if let Err(e) = scheduler::save_queue(&qs) {
                    applog(&format!(
                        "[queue] persist (crash recovery) FAILED ({})",
                        storage::err_code(&e)
                    ));
                }
            }
            qs
        }))
        .setup(|app| {
            std::thread::spawn(tmux::init_deck_server);
            scheduler::spawn_scheduler(app.handle().clone());
            // Update-check heartbeat from a Rust thread: webview timers are
            // frozen by App Nap when the app is backgrounded, so a JS
            // setInterval would effectively never fire. One latest.json
            // fetch (~1.4 KB) per 30 min is the entire cost.
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(30 * 60));
                    let _ = handle.emit("update-check", ());
                });
            }
            // Native menu: the default set restores all standard macOS
            // shortcuts (⌘C/V/A/Z/Q/H/M/W…); Terminal→Clear adds ⌘K.
            let handle = app.handle();
            let menu = tauri::menu::Menu::default(handle)?;
            let clear = tauri::menu::MenuItemBuilder::with_id("clear", "Clear")
                .accelerator("Cmd+K")
                .build(app)?;
            let export =
                tauri::menu::MenuItemBuilder::with_id("export-logs", "Export Logs…").build(app)?;
            let check =
                tauri::menu::MenuItemBuilder::with_id("check-updates", "Check for Updates…")
                    .build(app)?;
            // standard macOS spot: application menu, right under "About deck"
            let mut in_app_menu = false;
            if let Some(first) = menu.items()?.into_iter().next() {
                if let Some(sub) = first.as_submenu() {
                    in_app_menu = sub.insert(&check, 1).is_ok();
                }
            }
            let mut tb = tauri::menu::SubmenuBuilder::new(app, "Terminal")
                .item(&clear)
                .separator()
                .item(&export);
            if !in_app_menu {
                tb = tb.item(&check);
            }
            let term_menu = tb.build()?;
            menu.append(&term_menu)?;
            app.set_menu(menu)?;
            app.on_menu_event(|app, e| {
                if e.id() == "clear" {
                    let _ = app.emit("menu-clear", ());
                }
                if e.id() == "check-updates" {
                    let _ = app.emit("update-check-manual", ());
                }
                if e.id() == "export-logs" {
                    // never log the export's absolute path (it embeds the
                    // user's home directory) — Finder reveals it anyway
                    match commands::export_logs() {
                        Ok(_) => applog("[export] logs written"),
                        Err(err) => {
                            applog(&format!("[export] FAILED ({})", storage::err_code(&err)))
                        }
                    }
                }
            });
            Ok(())
        })
        .on_page_load(|webview, payload| {
            // The window is created hidden (no white flash). Reveal it from
            // the Rust side once content is loaded — the JS show() alone
            // doesn't reliably surface a relaunched-by-updater instance.
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                let w = webview.window();
                let _ = w.show();
                let _ = w.set_focus();
            }
        })
        .on_window_event(|window, event| {
            // ⌘W / red button hides instead of destroying the only window;
            // the Dock icon (Reopen) brings it back.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::load_board,
            commands::save_board,
            commands::load_settings,
            commands::save_settings,
            commands::detect_editors,
            commands::default_dir,
            commands::tmux_available,
            commands::start_session,
            commands::kill_session,
            commands::scroll_session,
            commands::clear_history,
            commands::poll_sessions,
            pty::attach_session,
            pty::pty_write,
            pty::pty_ack,
            pty::pty_resize,
            pty::detach_session,
            commands::open_target,
            history::recent_commands,
            history::record_command,
            history::history_clear,
            commands::ui_event,
            commands::ping_event,
            scheduler::queue_list,
            scheduler::queue_add,
            scheduler::queue_update,
            scheduler::queue_remove,
            scheduler::queue_pause,
            scheduler::queue_retry,
            scheduler::queue_skip,
            commands::storage_warnings,
            scheduler::queue_clear_session,
        ])
        .build(tauri::generate_context!())
        .expect("error while building deck")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            let _ = (app, &event);
        });
}
