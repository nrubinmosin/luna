mod account_settings;
mod accounts;
mod activity;
mod agentlog;
mod agents;
mod background;
mod claude;
mod cli;
mod codex;
mod corecmd;
mod corelink;
mod coreproc;
mod emit;
mod hub;
mod ipc;
mod lifecycle;
mod log;
mod mcp;
mod media;
mod paths;
mod power;
mod procs;
mod provider;
mod pty;
mod settings;
mod sysmenu;
mod throttle;
mod watchdog;
mod worktree;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The exit watcher and the core are this same exe; neither may become a
    // second Luna window.
    if lifecycle::watch_from_args()
        || coreproc::from_args()
        || coreproc::fingerprint_from_args()
        || coreproc::stop_from_args()
    {
        return;
    }
    lifecycle::wait_for_previous();
    log::install_panic_hook();

    // A dev build driven by a script runs beside the Luna a person is using,
    // possibly under the same identifier, and the second launch would only
    // focus the first. The same variable turns on the dev caller (agents.rs);
    // the test flavour (test-build.ps1) has an identifier of its own anyway.
    let dev = std::env::var_os("LUNA_DEV_TOKEN").is_some();
    let builder = tauri::Builder::default();
    let builder = if dev {
        builder
    } else {
        // Must be the first plugin: a second launch focuses the running
        // instance instead of spawning a duplicate app + tray icon.
        builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
    };
    builder
        // Restores size, position and maximised state from last run.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Events reach the page through the window; installed before
            // anything that could send one starts.
            let handle = app.handle().clone();
            emit::install(move |event, payload| {
                use tauri::Emitter;
                let _ = handle.emit(event, payload);
            });
            // A click on a notification brings the window up, from the tray
            // if need be, and one about a chat opens that chat in it.
            let handle = app.handle().clone();
            tauri_plugin_notification::NotificationExt::notification(app.handle()).on_action(move |done| {
                if done.action_id() != "tap" {
                    return;
                }
                show_main(&handle);
                let chat = done.notification().and_then(|n| n.extra().get("chatId")).and_then(|v| v.as_str());
                if let Some(chat_id) = chat {
                    emit::to_ui("ui://open-chat", serde_json::json!({ "chatId": chat_id }));
                }
            })?;
            // Here rather than at the top of run(): a second launch that only
            // focuses the running Luna has exited by now, and must not read as
            // a restart. The build stamp as well as the version: test builds
            // between two releases share a version, and only the commit tells
            // them apart.
            log::start(&format!(
                "starting luna {} build {} core {}",
                env!("CARGO_PKG_VERSION"),
                env!("LUNA_BUILD"),
                env!("LUNA_CORE_FP")
            ));
            // The previous run's end is reported before anything new is logged.
            lifecycle::start(app.handle().clone());
            // The sessions, the hooks' listener, the agents, activity and power
            // all run in the core (coreproc.rs); the window finds or starts it.
            corelink::start();
            claude::models::refresh_soon();
            // Installs the CLIs on a fresh machine and keeps them current after.
            cli::refresh_periodically();

            if let Some(w) = app.get_webview_window("main") {
                sysmenu::silence(&w);
            }
            // Once the window and tao's own hidden one both exist.
            sysmenu::exit_with_session();

            // A parent account's settings onto the others, when that is asked for.
            account_settings::sync_on_start();
            // A window whose page has died is rebuilt rather than left black.
            watchdog::start(app.handle().clone());

            let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
            // The window only: the sessions run on in the core.
            let restart = MenuItem::with_id(app, "restart", "Restart window", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &restart, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Luna")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "restart" => lifecycle::restart_from_tray(app),
                    "quit" => lifecycle::quit_from_tray(app),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                })
                .build(app)?;
            Ok(())
        })
        // Closing the window hides to tray; sessions keep running. Quit via tray
        // ends them, the core and the window together.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            account_settings::recommended_settings,
            account_settings::apply_recommended,
            account_settings::settings_sync_state,
            account_settings::set_settings_parent,
            account_settings::set_settings_sync_on_start,
            account_settings::sync_settings_now,
            account_settings::read_account_config,
            account_settings::check_account_config,
            account_settings::format_account_config,
            account_settings::write_account_config,
            accounts::list_accounts,
            accounts::create_account,
            accounts::delete_account,
            agents::agent_blocked_accounts,
            agents::set_agent_account,
            cli::cli_status,
            cli::cli_update_now,
            claude::defaults::claude_defaults,
            claude::limits::account_limits,
            claude::trust::claude_folder_trusted,
            claude::trust::claude_trust_folder,
            codex::defaults::codex_defaults,
            codex::limits::codex_limits,
            codex::models::codex_models,
            codex::trust::codex_folder_trusted,
            codex::trust::codex_trust_folder,
            log::append_log,
            media::save_media,
            corelink::core_call,
            corelink::core_send,
            corelink::core_state,
            corelink::core_restart,
            lifecycle::note_exit,
            pty::saved_title,
            settings::get_accounts_root,
            settings::set_accounts_root,
            watchdog::ui_heartbeat,
            worktree::fetch_before_worktree,
            worktree::set_fetch_before_worktree,
            worktree::inspect_worktrees,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        // Quit comes from the tray while pty sessions are still streaming;
        // their emits raced the event loop's teardown and could panic inside
        // tao ("cannot move state from Destroyed"). Close the emit gate here,
        // which both stops new events and waits for the ones in flight, so the
        // loop is torn down with nothing left to land on it.
        .run(|_app, event| {
            // The watchdog closing the last window to rebuild it is not a quit.
            if let tauri::RunEvent::ExitRequested { code: None, api, .. } = &event {
                if watchdog::rebuilding() {
                    api.prevent_exit();
                    return;
                }
            }
            if let tauri::RunEvent::Exit = &event {
                // Every way out that names itself has done so by now.
                lifecycle::exiting("the event loop ended");
            }
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                emit::stop();
                power::clear();
            }
        });
}
