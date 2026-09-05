// Prevent a console window on Windows builds; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! deck backend — Tauri assembly only. Domain logic lives in the modules:
//! tmux (server/exec), pty (attach bridge), scheduler (prompts), storage
//! (atomic persistence + logs), history (completion), commands (the rest).

mod agent_status;
mod applog;
mod commands;
mod context;
mod datadir;
mod diagnostics;
mod documents;
mod drops;
mod error;
mod history;
mod inbound;
mod inbound_slack;
mod instance_lock;
mod keychain;
mod launch_args;
mod links;
mod procinfo;
mod pty;
mod redact;
mod relaunch;
mod scheduler;
mod shell_state;
mod smoke_faults;
mod storage;
mod sync;
mod terminal;
mod terminal_scroll;
mod terminal_selection;
mod tmux;
mod tmux_lifecycle;
mod updater;

pub(crate) use applog::applog;

use crate::error::DeckError;
use std::process::Command;
use tauri::{Emitter, Manager};

#[derive(Clone, Copy)]
struct NativeStrings {
    clear: &'static str,
    export_logs: &'static str,
    check_updates: &'static str,
    terminal: &'static str,
}

const NATIVE_EN: NativeStrings = NativeStrings {
    clear: "Clear",
    export_logs: "Export Logs…",
    check_updates: "Check for Updates…",
    terminal: "Terminal",
};
const NATIVE_ZH_HANS: NativeStrings = NativeStrings {
    clear: "清除",
    export_logs: "导出日志…",
    check_updates: "检查更新…",
    terminal: "终端",
};

fn resolve_native_locale(preference: &str, system_languages: &str) -> &'static str {
    if preference == "zh-Hans"
        || (preference == "system"
            && system_languages
                .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '"'))
                .any(|tag| {
                    let tag = tag.to_ascii_lowercase();
                    tag == "zh-cn"
                        || tag == "zh-sg"
                        || tag == "zh-hans"
                        || tag.starts_with("zh-hans-")
                }))
    {
        "zh-Hans"
    } else {
        "en"
    }
}

fn system_languages() -> String {
    Command::new("defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn native_strings(preference: &str) -> NativeStrings {
    if resolve_native_locale(preference, &system_languages()) == "zh-Hans" {
        NATIVE_ZH_HANS
    } else {
        NATIVE_EN
    }
}

struct NativeMenu {
    clear: tauri::menu::MenuItem<tauri::Wry>,
    export_logs: tauri::menu::MenuItem<tauri::Wry>,
    check_updates: tauri::menu::MenuItem<tauri::Wry>,
    terminal: tauri::menu::Submenu<tauri::Wry>,
}

#[tauri::command]
fn set_native_locale(locale: String, menu: tauri::State<'_, NativeMenu>) -> Result<(), DeckError> {
    if !matches!(locale.as_str(), "system" | "en" | "zh-Hans") {
        return Err("invalid locale".into());
    }
    let s = native_strings(&locale);
    menu.clear.set_text(s.clear).map_err(DeckError::text)?;
    menu.export_logs
        .set_text(s.export_logs)
        .map_err(DeckError::text)?;
    menu.check_updates
        .set_text(s.check_updates)
        .map_err(DeckError::text)?;
    menu.terminal.set_text(s.terminal).map_err(DeckError::text)
}

// ---------- main ---------------------------------------------------------------

fn main() {
    if let Some(code) = relaunch::run_helper_from_args() {
        std::process::exit(code);
    }
    relaunch::capture_current_target();
    let deck_dir = crate::datadir::deck_dir();
    // idempotent permission migration BEFORE anything touches the data files:
    // ~/.deck → 0700, every file an older deck may have left 0644 → 0600.
    // A failure is surfaced (log + boot toast), never silently ignored.
    if let Err(e) = crate::datadir::harden_data_dir(&deck_dir) {
        storage::warn(format!("data privacy hardening incomplete: {e}"));
    }
    // one-time redaction of logs/exports an OLDER deck wrote (absolute
    // paths, URLs, token shapes, raw session names). Runs before anything
    // appends to app.log, rewrites in place 0600, keeps no raw copy.
    let cleaned = crate::applog::sanitize_existing_logs(&deck_dir);
    crate::applog::rotate_log();
    if crate::launch_args::command_flag("--debug-logging") {
        applog("[boot] verbose diagnostics enabled");
    }
    if cleaned > 0 {
        applog(&format!(
            "[boot] redacted {cleaned} pre-existing log/export file(s)"
        ));
    }
    // dropped/pasted files only exist so their path could be typed into a
    // session — a week later nobody references them anymore
    crate::datadir::prune_old_files(&deck_dir.join("drops"), 7 * 24 * 3600);
    if let Err(e) = crate::instance_lock::acquire_instance_lock(&deck_dir) {
        applog(&format!(
            "[boot] instance lock unavailable ({}) — exiting",
            e.code()
        ));
        // No alert: the only way to show one without a dialog plugin is
        // `osascript`, and AppleScript execution from a third-party app is
        // an EDR signature. The running instance stays where it is.
        std::process::exit(0);
    }
    shell_state::cleanup_restore_temps();
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(pty::PtyState::default())
        .manage(scheduler::boot_queues())
        .setup(|app| {
            // This must finish before the scheduler or webview can create a
            // session. It reuses an exact current server, records an occupied
            // legacy/old server as pending, and replaces only an empty one.
            tmux_lifecycle::reconcile_on_boot();
            scheduler::spawn_scheduler(app.handle().clone());
            inbound::spawn_inbound(app.handle().clone());
            // Agent-status socket: content-free state words from agent hooks
            // (see agent_status.rs). Re-points already-installed hook
            // entries at this install's bundled helper and retires the
            // legacy ~/.deck/bin copy; never installs hooks by itself.
            agent_status::migrate_hooks_on_boot();
            agent_status::spawn_listener();
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
            let strings = native_strings(&documents::locale_setting());
            let menu = tauri::menu::Menu::default(handle)?;
            let clear = tauri::menu::MenuItemBuilder::with_id("clear", strings.clear)
                .accelerator("Cmd+K")
                .build(app)?;
            let export =
                tauri::menu::MenuItemBuilder::with_id("export-logs", strings.export_logs).build(app)?;
            let check =
                tauri::menu::MenuItemBuilder::with_id("check-updates", strings.check_updates)
                    .build(app)?;
            // standard macOS spot: application menu, right under "About deck"
            let mut in_app_menu = false;
            if let Some(first) = menu.items()?.into_iter().next() {
                if let Some(sub) = first.as_submenu() {
                    in_app_menu = sub.insert(&check, 1).is_ok();
                }
            }
            let mut tb = tauri::menu::SubmenuBuilder::new(app, strings.terminal)
                .item(&clear)
                .separator()
                .item(&export);
            if !in_app_menu {
                tb = tb.item(&check);
            }
            let term_menu = tb.build()?;
            app.manage(NativeMenu {
                clear: clear.clone(), export_logs: export.clone(),
                check_updates: check.clone(), terminal: term_menu.clone(),
            });
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
                    match diagnostics::export_logs() {
                        Ok(_) => applog("[export] logs written"),
                        Err(err) => {
                            applog(&format!("[export] FAILED ({})", err.code()))
                        }
                    }
                }
            });
            Ok(())
        })
        .on_page_load(|webview, payload| {
            // The window is created hidden. The frontend reveals it only
            // after typed settings load and the resolved theme (including
            // system light/dark) has been applied, preventing a first-frame
            // palette flash without duplicating settings into another store.
            if payload.event() == tauri::webview::PageLoadEvent::Finished {
                if let Some(mode) = crate::launch_args::debug_arg("--smoke-wkwebview") {
                    let entry = if mode == "restart" {
                        "m.verifyRestart()"
                    } else if mode == "ambiguous" {
                        "m.verifyAmbiguousBoot()"
                    } else if mode == "settings" {
                        "m.verifySettings()"
                    } else {
                        "m.run()"
                    };
                    let script = format!(
                        "setTimeout(() => import('./test/wk-smoke.mjs').then(m => {entry}).catch(e => {{ window.__TAURI__.core.invoke('ui_event', {{code:'js-reject',detail:(e&&e.name)||'error',a:0,b:0}}); window.__TAURI__.core.invoke('ui_event', {{code:'smoke-check',detail:'done',a:0,b:-1}}); }}), 1800)"
                    );
                    let _ = webview.eval(&script);
                }
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
            documents::load_board,
            documents::save_board,
            documents::load_settings,
            documents::save_settings,
            updater::build_identity,
            tmux_lifecycle::tmux_server_status,
            tmux_lifecycle::defer_tmux_restart,
            tmux_lifecycle::acknowledge_tmux_lifecycle_notice,
            tmux_lifecycle::restart_tmux_server,
            updater::check_for_update,
            updater::install_update,
            relaunch::relaunch_after_update,
            commands::set_terminal_mode_style,
            set_native_locale,
            commands::detect_editors,
            commands::default_dir,
            commands::tmux_available,
            commands::start_session,
            commands::kill_session,
            terminal::scroll_session,
            terminal::scroll_bottom,
            terminal::clear_history,
            terminal::terminal_selection_start,
            terminal::terminal_selection_update,
            terminal::terminal_selection_finish,
            terminal::terminal_selection_copy,
            terminal::terminal_selection_scroll,
            terminal::terminal_selection_cancel,
            terminal::terminal_metrics,
            commands::write_clipboard,
            commands::poll_sessions,
            shell_state::shell_snapshots_clear,
            pty::attach_session,
            pty::pty_write,
            pty::pty_ack,
            pty::pty_resize,
            pty::detach_session,
            links::open_target,
            links::resolve_parent_dir,
            links::terminal_paths_exist,
            history::recent_commands,
            history::record_command,
            history::history_clear,
            diagnostics::debug_logging_enabled,
            diagnostics::log_size,
            diagnostics::reset_logs,
            diagnostics::export_logs,
            diagnostics::ui_event,
            commands::ping_event,
            scheduler::queue_list,
            scheduler::queue_probe_context,
            scheduler::smoke_seed_ambiguous,
            scheduler::smoke_queue_state,
            scheduler::smoke_flush_queue,
            scheduler::queue_add,
            scheduler::queue_update,
            scheduler::queue_remove,
            scheduler::queue_pause,
            scheduler::queue_retry,
            scheduler::queue_acknowledge,
            scheduler::queue_skip,
            scheduler::queue_send_now,
            documents::storage_warnings,
            scheduler::queue_clear_sessions,
            drops::save_dropped_file,
            agent_status::agent_hooks_status,
            agent_status::agent_hooks_set,
            inbound::inbound_status,
            inbound::inbound_pending,
            inbound::inbound_ack,
            inbound::inbound_set_secret,
            inbound::inbound_check_now,
            inbound::inbound_setup,
            smoke_faults::smoke_fault_set,
            smoke_faults::smoke_clipboard_metrics,
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

#[cfg(test)]
mod i18n_tests {
    use super::resolve_native_locale;
    #[test]
    fn native_locale_resolution_matches_web_runtime() {
        assert_eq!(
            resolve_native_locale("system", "(\n zh-Hans-CN,\n en-US\n)"),
            "zh-Hans"
        );
        assert_eq!(resolve_native_locale("system", "(zh-CN)"), "zh-Hans");
        assert_eq!(resolve_native_locale("system", "(zh-SG)"), "zh-Hans");
        assert_eq!(resolve_native_locale("system", "(zh-Hant, en-US)"), "en");
        assert_eq!(resolve_native_locale("en", "(zh-CN)"), "en");
    }
}
