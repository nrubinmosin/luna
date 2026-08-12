mod accounts;
pub mod cli;
mod limits;
mod log;
mod media;
mod pty;
mod trust;
mod worktree;

use tauri::{
    menu::{Menu, MenuItem},
    Emitter,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

/// Requests already handed to the UI. The watcher polls a directory, so
/// without this it would re-emit the same request every tick until the chat
/// finishes opening.
static CLAIMED: std::sync::Mutex<Option<std::collections::HashSet<String>>> =
    std::sync::Mutex::new(None);

fn claim(id: &str) -> bool {
    let Ok(mut guard) = CLAIMED.lock() else { return false };
    guard.get_or_insert_with(Default::default).insert(id.to_string())
}

/// Reports back to the waiting `llm-desktop-cli` process, and lets the same
/// request be retried if it failed.
#[tauri::command]
fn ack_chat_request(id: String, error: Option<String>) {
    if error.is_some() {
        if let Ok(mut guard) = CLAIMED.lock() {
            guard.get_or_insert_with(Default::default).remove(&id);
        }
    }
    cli::acknowledge(&id, error.as_deref());
}

/// Watches the request directory. Polling rather than a filesystem notifier:
/// one directory listing every half second costs nothing, and it also picks up
/// anything submitted while the app was closed.
fn watch_requests(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        cli::sweep_stale();
        loop {
            for req in cli::pending_requests() {
                if claim(&req.id) {
                    log::info("cli", &format!("new chat requested: {req:?}"));
                    let _ = app.emit("app://new-chat", req);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

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
    log::info("app", &format!("starting llm-desktop {}", env!("CARGO_PKG_VERSION")));

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
            watch_requests(app.handle().clone());

            // Housekeeping off the startup path.
            std::thread::spawn(|| {
                let _ = media::prune_media();
            });

            let open = MenuItem::with_id(app, "open", "Open", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("llm-desktop")
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
            limits::account_limits,
            ack_chat_request,
            log::append_log,
            log::log_path,
            log::reveal_log,
            media::save_media,
            media::clear_media,
            media::prune_media,
            pty::ensure_session,
            pty::write_session,
            pty::resize_session,
            pty::kill_session,
            pty::session_alive,
            pty::session_meta,
            pty::delete_session,
            trust::folder_trusted,
            trust::trust_folder,
            worktree::remove_worktree,
            worktree::orphan_worktrees,
            worktree::remove_orphan_worktrees,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
