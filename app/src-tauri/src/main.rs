// Prevent a console window on Windows builds; harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! deck backend — Tauri assembly only. Domain logic lives in the modules:
//! tmux (server/exec), pty (attach bridge), scheduler (prompts), storage
//! (atomic persistence + logs), history (completion), commands (the rest).

mod admission;
mod agent_status;
mod applog;
mod bell;
mod commands;
mod connector;
mod context;
mod datadir;
mod diagnostics;
mod documents;
mod drops;
mod error;
mod history;
mod inbound;
mod inbound_channel;
mod inbound_clock;
mod inbound_slack;
mod input_source;
mod instance_lock;
mod intelligence;
mod keychain;
mod launch_args;
mod ledger;
#[cfg(test)]
mod limits_mirror;
mod links;
mod mcp;
mod mcp_fs;
mod notify;
mod procinfo;
mod prompt_delivery;
mod pty;
mod redact;
mod relaunch;
mod reminder;
mod restart;
mod resume;
mod scheduler;
mod session_runtime;
mod shell_exit;
mod shell_state;
#[cfg(test)]
mod signal_trace;
mod slack_api;
mod slack_transport;
mod smoke_faults;
mod smoke_native;
mod storage;
mod sync;
mod terminal;
mod terminal_scroll;
mod terminal_selection;
mod tmux;
mod tmux_clients;
mod tmux_lifecycle;
mod tunnel_helper;
mod updater;
mod voice;

pub(crate) use applog::applog;

use crate::error::{DeckError, ErrorKind};
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

/// The webview resolves `system` against `navigator.languages` (`i18n.js`)
/// and reports the RESOLVED locale here, so the native menu never has to
/// read system preferences itself (no `defaults` spawn).
fn native_strings(locale: &str) -> NativeStrings {
    if locale == "zh-Hans" {
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
    if !matches!(locale.as_str(), "en" | "zh-Hans") {
        return Err(DeckError::new(ErrorKind::Other, "invalid locale"));
    }
    let s = native_strings(&locale);
    menu.clear
        .set_text(s.clear)
        .map_err(|e| DeckError::classified(e.to_string()))?;
    menu.export_logs
        .set_text(s.export_logs)
        .map_err(|e| DeckError::classified(e.to_string()))?;
    menu.check_updates
        .set_text(s.check_updates)
        .map_err(|e| DeckError::classified(e.to_string()))?;
    menu.terminal
        .set_text(s.terminal)
        .map_err(|e| DeckError::classified(e.to_string()))
}

// ---------- main ---------------------------------------------------------------

/// Debug-only WKWebView smoke modes (`--smoke-wkwebview <mode>`) and the
/// `ui/test/wk-smoke.mjs` entry each one calls. This table, the smoke
/// manifest (`ui/test/fixtures/smoke-manifest.json`) and `app/run.sh`'s mode
/// list name the same modes (diagnostics.rs and smoke-manifest.test.mjs
/// check it); any other value, such as `DECK_SMOKE_WKWEBVIEW=1`, runs `run`.
pub(crate) const SMOKE_ENTRIES: &[(&str, &str)] = &[
    ("run", "m.run()"),
    ("restart", "m.verifyRestart()"),
    ("ambiguous", "m.verifyAmbiguousBoot()"),
    ("review", "m.verifyReview()"),
    ("review-restart", "m.verifyReview(true)"),
    ("attention", "m.verifyAttention()"),
    ("settings", "m.verifySettings()"),
    ("voice", "m.verifyVoice()"),
    ("translation", "m.verifyTranslation()"),
    ("translation-native", "m.verifyTranslationNative()"),
    ("translation-guard", "m.verifyTranslationGuard()"),
    ("resume", "m.verifyResume()"),
    ("buffer", "m.verifyBuffer()"),
    ("buffer-narrow", "m.verifyBufferNarrow()"),
    ("channel", "m.verifyChannel()"),
    ("channel-fault", "m.verifyChannelFault()"),
    ("channel-first-send", "m.verifyChannelFirstSend()"),
    ("connector", "m.verifyConnector()"),
    ("connector-transport", "m.verifyConnectorTransport()"),
    ("selection-events", "m.verifySelectionEvents()"),
    ("signal-finish", "m.verifySignalFinish()"),
    ("authority-live", "m.verifyAuthorityLive()"),
    ("empty-start", "m.verifyEmptyStart()"),
    ("clock-live", "m.verifyClockLive()"),
    ("reminder", "m.verifyReminder()"),
    ("reminder-native", "m.verifyReminderNative()"),
    ("board-lost", "m.verifyBoardLost()"),
    ("approval", "m.verifyApproval()"),
];

fn smoke_entry(mode: &str) -> &'static str {
    SMOKE_ENTRIES
        .iter()
        .find(|(name, _)| *name == mode)
        .map_or("m.run()", |(_, entry)| entry)
}

fn main() {
    launch_args::configure_isolated_carrier();
    if smoke_native::reminder_maintenance() {
        return;
    }
    if let Some(code) = relaunch::run_helper_from_args() {
        std::process::exit(code);
    }
    relaunch::capture_current_target();
    let deck_dir = crate::datadir::deck_dir();
    // idempotent permission migration BEFORE anything touches the data files:
    // ~/.deck → 0700, every file an older deck may have left 0644 → 0600.
    // A failure is surfaced (log + boot toast), never silently ignored.
    if let Err(e) = crate::datadir::harden_data_dir(&deck_dir) {
        storage::warn(
            storage::StorageNotice::Privacy,
            format!("data privacy hardening incomplete: {e}"),
        );
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
            tmux::exit_on_termination_signals(app.handle().clone());
            // Deck's automatic work starts once the launch is known to be an
            // ordinary one, which the webview's first question about its
            // window settles. A launch the system made to deliver "remind in
            // 1 hour" transacts that answer and ends without starting any
            // of it (reminder.rs, response-only launch).
            {
                let handle = app.handle().clone();
                reminder::defer_automatic_work(app.handle().clone(), move || {
                    scheduler::spawn_scheduler(handle.clone());
                    inbound::spawn_inbound(handle.clone());
                    slack_transport::spawn(handle.clone());
                    connector::spawn_connector(handle.clone());
                    mcp::spawn(handle);
                });
            }
            // Agent-status socket: content-free state words from agent hooks
            // (see agent_status.rs). Re-points already-installed hook
            // entries at this install's bundled helper and retires the
            // legacy ~/.deck/bin copy; never installs hooks by itself.
            agent_status::migrate_hooks_on_boot();
            // Away notifications + Dock badge (notify.rs): the click
            // delegate and the saved switch, before the first hook event.
            {
                let (enabled, sound) = documents::notify_settings();
                notify::init(app.handle().clone(), enabled, sound);
            }
            agent_status::spawn_listener();
            input_source::init(app.handle().clone());
            // The minimum supported window is tauri.conf.json minWidth ×
            // minHeight (content points); the window manager is its only
            // enforcement, and it binds user resizes, not programmatic ones.
            // The narrow smoke records what AppKit holds Deck's window to and
            // the size it runs at, without a frontend window-control
            // permission or production seam.
            if crate::launch_args::debug_arg("--smoke-wkwebview").as_deref()
                == Some("buffer-narrow")
            {
                let conf = app.config().app.windows.first();
                let min_w = conf.and_then(|w| w.min_width).unwrap_or(f64::MAX);
                let min_h = conf.and_then(|w| w.min_height).unwrap_or(f64::MAX);
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    let [enforced_w, enforced_h, w, h] =
                        smoke_native::window_min().unwrap_or_default();
                    let held = enforced_w >= min_w && enforced_h >= min_h && w >= min_w && h >= min_h;
                    diagnostics::ui_event(
                        "smoke-check".into(),
                        Some("window-min-clamp".into()),
                        Some(if held { enforced_w as i64 } else { -(enforced_w as i64) - 1 }),
                        Some(enforced_h as i64 * 10_000 + h as i64),
                        None,
                    );
                    applog(&format!(
                        "[smoke] window min {enforced_w:.0}x{enforced_h:.0} content {w:.0}x{h:.0}"
                    ));
                });
            }
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
                    if mode == "translation" || mode == "translation-native" {
                        let _ = webview.eval("window.__DECK_SMOKE_TRANSLATION = true");
                    }
                    let entry = smoke_entry(&mode);
                    let script = format!(
                        "setTimeout(() => import('./test/wk-smoke.mjs').then(m => {entry}).catch(e => {{ window.__TAURI__.core.invoke('ui_event', {{code:'js-reject',detail:(e&&e.name)||'error',a:0,b:0}}); window.__TAURI__.core.invoke('ui_event', {{code:'smoke-check',detail:'done',a:0,b:-1}}); }}), 1800)"
                    );
                    let _ = webview.eval(&script);
                }
            }
        })
        .on_window_event(|window, event| {
            // Away = the main window is not in front (notify.rs).
            if let tauri::WindowEvent::Focused(focused) = event {
                notify::set_focused(*focused);
                intelligence::pasteboard::focus_changed(*focused);
                if *focused {
                    input_source::resync();
                }
            }
            // ⌘W / red button hides instead of destroying the only window;
            // the Dock icon (Reopen) brings it back.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                voice::voice_cancel(0);
                intelligence::pasteboard::translation_clipboard_disarm();
                let _ = window.emit("voice-window-hidden", ());
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            intelligence::translation::translation_capability,
            intelligence::translation::translation_translate,
            intelligence::translation::translation_cancel,
            intelligence::translation::translation_pack_status,
            intelligence::translation::translation_pack_install,
            intelligence::translation::translation_pack_delete,
            intelligence::translation::translation_unload,
            intelligence::pasteboard::translation_clipboard_arm,
            intelligence::pasteboard::translation_clipboard_disarm,
            intelligence::pasteboard::translation_clipboard_poll,
            intelligence::pasteboard::translation_clipboard_write,
            input_source::input_source_snapshot,
            notify::notify_configure,
            reminder::reminder_launch_visible,
            reminder::reminder_response_finish,
            reminder::reminder_status,
            reminder::reminder_actions,
            reminder::reminder_ack,
            reminder::reminder_show,
            reminder::reminder_request_permission,
            notify::notify_status,
            notify::notify_cards,
            notify::notify_dismiss,
            voice::voice_bind,
            voice::voice_start,
            voice::voice_snapshot,
            voice::voice_stop,
            voice::voice_cancel,
            voice::voice_deliver,
            links::voice_open_settings,
            documents::load_board,
            documents::save_board,
            documents::board_recovery_state,
            documents::board_lost_exit,
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
            history::recent_commands,
            resume::terminal_resume_hints,
            history::record_command,
            history::history_clear,
            diagnostics::debug_logging_enabled,
            diagnostics::log_size,
            diagnostics::reset_logs,
            diagnostics::export_logs,
            diagnostics::ui_event,
            commands::ping_event,
            scheduler::queue_list,
            scheduler::queue_review_preview,
            scheduler::queue_review_confirm,
            scheduler::queue_review_mode,
            scheduler::queue_cancel_list,
            scheduler::queue_probe_context,
            scheduler::smoke_seed_ambiguous,
            scheduler::smoke_queue_state,
            scheduler::smoke_flush_queue,
            scheduler::queue_add,
            scheduler::channel_queue_add,
            scheduler::channel_queue_add_reviewed_list,
            scheduler::queue_add_reviewed_list,
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
            inbound::inbound_run_ended,
            inbound::inbound_runs,
            inbound::inbound_set_secret,
            inbound::inbound_check_now,
            inbound::inbound_setup,
            inbound_channel::channel_pending,
            inbound_channel::channel_ack,
            inbound_channel::channel_check_pending,
            inbound_channel::channel_status,
            slack_transport::slack_connection_status,
            slack_transport::slack_legacy_credentials_clear,
            slack_transport::slack_channel_prepare,
            slack_transport::slack_channel_prepare_cancel,
            slack_api::slack_manifest,
            inbound_channel::channel_smoke_seed,
            inbound_channel::channel_smoke_identity,
            inbound_channel::channel_smoke_envelope,
            connector::connector_status,
            connector::connector_addresses,
            connector::connector_enable,
            connector::connector_disable,
            connector::connector_pairing,
            connector::connector_revoke,
            connector::connector_reset_identity,
            connector::connector_pending,
            connector::connector_claim,
            connector::connector_complete,
            connector::connector_validate,
            connector::connector_validate_admission,
            connector::connector_execute_native,
            connector::connector_smoke_seed,
            connector::connector_smoke_transport,
            connector::connector_smoke_window,
            mcp::mcp_status,
            mcp::mcp_adapter_path,
            mcp::mcp_enable,
            mcp::mcp_output_retention,
            mcp::mcp_disable,
            mcp::mcp_client_add,
            mcp::mcp_client_revoke,
            mcp::mcp_client_delete,
            mcp::mcp_scope_preview,
            mcp::mcp_execution_grant,
            mcp::mcp_execution_revoke,
            mcp::mcp_pending,
            mcp::mcp_claim,
            mcp::mcp_close_admit,
            mcp::mcp_start_session,
            mcp::mcp_complete,
            mcp::mcp_card_closed,
            mcp::mcp_takeover,
            mcp::mcp_return_control,
            mcp::mcp_session_ui,
            tunnel_helper::tunnel_helper_status,
            tunnel_helper::tunnel_helper_start,
            tunnel_helper::tunnel_helper_stop,
            tunnel_helper::tunnel_helper_remove,
            tunnel_helper::tunnel_helper_setup_command,
            smoke_faults::smoke_fault_set,
            smoke_faults::smoke_clipboard_metrics,
            smoke_faults::smoke_query_channel,
            smoke_faults::smoke_signal_fixture,
            smoke_faults::smoke_channel_fixture,
            smoke_native::smoke_native_input,
            smoke_native::smoke_reminder_inventory,
            smoke_native::smoke_reminder_withdraw,
            smoke_native::smoke_native_snapshot,
            smoke_native::smoke_native_fixture,
            smoke_native::smoke_pasteboard,
            smoke_native::smoke_pasteboard_audit,
            smoke_native::smoke_native_fixture_receipt,
            smoke_native::smoke_native_scenario,
        ])
        .build(tauri::generate_context!())
        .expect("error while building deck")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                // A response-only launch has no window to bring back; the
                // click still counts: that launch will stay (reminder.rs).
                if reminder::background_launch() {
                    reminder::reopen_requested();
                    return;
                }
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            if matches!(&event, tauri::RunEvent::Exit) {
                tmux::stop_query_channel();
                // A launch made only to deliver a notification answer ends
                // without leaving the tmux server its boot gate started when
                // none existed. Every other exit keeps the server.
                if reminder::background_launch() {
                    tmux_lifecycle::retire_boot_server();
                }
            }
            let _ = (app, &event);
        });
}
