mod accounts;
mod claude;
mod cli;
mod codex;
mod emit;
mod log;
mod media;
mod paths;
mod provider;
mod pty;
mod settings;
mod sysmenu;
mod throttle;
mod worktree;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    log::install_panic_hook();
    // The build stamp, not just the version: 0.1.0 has never moved, and a log
    // that outlives a few builds is unreadable without knowing which one wrote
    // which line.
    log::info(
        "app",
        &format!("starting luna {} build {}", env!("CARGO_PKG_VERSION"), env!("LUNA_BUILD")),
    );

    tauri::Builder::default()
        // Must be the first plugin: a second launch focuses the running
        // instance instead of spawning a duplicate app + tray icon.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        // Restores size, position and maximised state from last run.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(pty::PtyManager::default())
        .setup(|app| {
            claude::models::refresh_soon();
            // Installs the CLIs on a fresh machine and keeps them current after.
            cli::refresh_periodically(app.handle().clone());

            if let Some(w) = app.get_webview_window("main") {
                sysmenu::silence(&w);
            }
            // Once the window and tao's own hidden one both exist.
            sysmenu::exit_with_session();

            // Housekeeping off the startup path, and again every few hours.
            media::prune_periodically();

            let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Luna")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main(app),
                    "quit" => app.exit(0),
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
        // Closing the window hides to tray; sessions keep running. Quit via tray.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            accounts::list_accounts,
            accounts::create_account,
            accounts::delete_account,
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
            media::clear_media,
            media::prune_media,
            pty::ensure_claude_session,
            pty::ensure_codex_session,
            pty::write_session,
            pty::resize_session,
            pty::kill_session,
            pty::session_alive,
            pty::session_meta,
            pty::saved_title,
            pty::orphan_sessions,
            pty::delete_session,
            settings::get_accounts_root,
            settings::set_accounts_root,
            worktree::create_worktree,
            worktree::remove_worktree,
            worktree::orphan_worktrees,
            worktree::remove_orphan_worktrees,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        // Quit comes from the tray while pty sessions are still streaming;
        // their emits raced the event loop's teardown and could panic inside
        // tao ("cannot move state from Destroyed"). Close the emit gate here,
        // which both stops new events and waits for the ones in flight, so the
        // loop is torn down with nothing left to land on it.
        .run(|_app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                emit::stop();
            }
        });
}
