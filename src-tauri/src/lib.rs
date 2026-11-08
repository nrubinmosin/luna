mod account_settings;
mod account_use;
mod accounts;
mod activity;
mod admit;
mod agentlog;
mod agents;
mod asking;
mod background;
mod browsers;
mod checks;
mod claude;
mod cli;
mod codex;
mod corecmd;
mod corelink;
mod coreproc;
mod emit;
#[cfg(test)]
mod golden;
mod guard;
mod hub;
mod ipc;
#[cfg(unix)]
mod keeper;
mod library;
mod lifecycle;
#[cfg(test)]
mod live;
mod log;
mod mcp;
mod media;
mod paths;
mod power;
mod procs;
mod provider;
mod pty;
mod secret;
mod settings;
mod sysmenu;
mod throttle;
mod watchdog;
mod worktree;

/// The transcript readers as pure functions of a file's bytes, for the fuzz
/// targets in fuzz/: the one way into the crate from outside it.
#[doc(hidden)]
pub mod transcripts {
    pub use crate::agents::{Ask, AskQuestion, Message};
    pub use crate::claude::session::{read_transcript, TranscriptRead};
    pub use crate::codex::session::{read_rollout, RolloutRead, Tail};
}

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

/// The test build kept out of the way of the person at the screen while an
/// agent drives it (`LUNA_TEST_WINDOW=background`, which
/// `scripts/test-build.ps1 -Background` sets): its window is made off screen,
/// without taking focus and without a taskbar button. The release ignores the
/// variable.
pub(crate) fn background() -> bool {
    paths::is_test() && std::env::var("LUNA_TEST_WINDOW").is_ok_and(|v| v == "background")
}

/// Brings the window up. A test window run in the background comes on screen
/// with it: only a person's click on the tray or a toast gets here then, and
/// that is how one looks at it.
pub(crate) fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if background() {
            let _ = w.set_focusable(true);
            let _ = w.set_skip_taskbar(false);
            let _ = w.center();
        }
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

/// The window as tauri.conf.json describes it. Made here rather than by Tauri
/// (`create: false`) so that on Windows its WebView keeps its profile in the
/// data dir, not under `LOCALAPPDATA\<identifier>`.
pub(crate) fn build_main(app: &tauri::AppHandle) -> Result<tauri::WebviewWindow, String> {
    let config = app.config().app.windows.iter().find(|w| w.label == "main").ok_or("no main window in the config")?;
    let builder = tauri::WebviewWindowBuilder::from_config(app, config).map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let builder = builder.data_directory(paths::webview_dir());
    if !background() {
        return builder.build().map_err(|e| e.to_string());
    }
    // Shown rather than hidden, since a hidden WebView2 stops rendering and
    // throttles the page, its heartbeat (watchdog.rs) and terminals with it;
    // but made just past the right edge of the rightmost monitor, so nothing
    // of it is on screen. Made there, not moved there: tao shows a window it
    // makes unfocused without activating it, a later show() activates, and
    // tao drops a starting position no monitor holds, while a monitor's very
    // edge still counts as on it. Unfocusable too (WS_EX_NOACTIVATE on
    // Windows), so nothing done to it takes focus from the person at the
    // screen. On Linux and macOS the window manager may still place it on
    // screen (Wayland ignores positions); it takes no focus and no taskbar
    // entry there either.
    let edge = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .max_by_key(|m| m.position().x + m.size().width as i32)
        .map(|m| ((m.position().x + m.size().width as i32) as f64 / m.scale_factor(), m.position().y as f64 / m.scale_factor()));
    let (x, y) = edge.unwrap_or((10_000.0, 0.0));
    let builder = builder.maximized(false).visible(true).focused(false).focusable(false).skip_taskbar(true).position(x, y);
    // Chromium counts a window off every monitor as covered and backgrounds
    // its page; these keep it running as if seen. The first switch is Tauri's
    // own default, which setting any arguments replaces.
    #[cfg(windows)]
    let builder = builder.additional_browser_args(
        "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion \
         --disable-background-timer-throttling --disable-renderer-backgrounding --disable-backgrounding-occluded-windows",
    );
    let w = builder.build().map_err(|e| e.to_string())?;
    // tao takes the button away before the window is first shown, and the
    // shell may give it one back then.
    let _ = w.set_skip_taskbar(true);
    Ok(w)
}

/// A link a terminal was clicked on, in a browser of the user's (browsers.rs)
/// or the mail app: a window the WebView opened would be a browser of Luna's
/// own, with none of the user's sign-ins. Web and mail links only; a
/// terminal prints anything.
#[tauri::command]
fn open_link(url: String, ctx: Option<browsers::Context>) -> Result<(), String> {
    let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
    match scheme.as_str() {
        "http" | "https" => browsers::open(&url, &ctx.unwrap_or_default()),
        "mailto" => tauri_plugin_opener::open_url(&url, None::<&str>).map_err(|e| e.to_string()),
        // Not the link itself: the log gets this, and a link may carry a
        // sign-in code.
        _ => Err(format!("not a web or mail link ({scheme}:)")),
    }
}

/// The tray while the PC is held awake: a dot on the icon, and who holds it
/// in the tooltip, so a hidden window still says why the PC will not sleep.
/// `None` puts the plain icon back.
#[tauri::command]
fn tray_awake(app: tauri::AppHandle, holders: Option<String>) -> Result<(), String> {
    let Some(tray) = app.tray_by_id("main") else { return Ok(()) };
    let Some(base) = app.default_window_icon() else { return Ok(()) };
    let (icon, tip) = match holders {
        Some(h) => (with_dot(base), format!("Luna — keeping the PC awake:\n{h}")),
        None => (base.clone().to_owned(), "Luna".to_string()),
    };
    // Windows cuts a tray tooltip at 127 characters.
    let tip: String = if tip.chars().count() > 127 { format!("{}…", tip.chars().take(126).collect::<String>()) } else { tip };
    tray.set_icon(Some(icon)).map_err(|e| e.to_string())?;
    tray.set_tooltip(Some(tip)).map_err(|e| e.to_string())
}

/// The icon with an amber dot, ringed in black, at its lower right.
fn with_dot(base: &tauri::image::Image<'_>) -> tauri::image::Image<'static> {
    let (w, h) = (base.width() as i64, base.height() as i64);
    let mut rgba = base.rgba().to_vec();
    let r = (w.min(h) as f64 * 0.24).max(2.0);
    let (cx, cy) = (w as f64 - r - 1.0, h as f64 - r - 1.0);
    for y in 0..h {
        for x in 0..w {
            let d = ((x as f64 + 0.5 - cx).powi(2) + (y as f64 + 0.5 - cy).powi(2)).sqrt();
            let px = ((y * w + x) * 4) as usize;
            if d <= r - 1.5 {
                rgba[px..px + 4].copy_from_slice(&[0xF5, 0xA6, 0x23, 0xFF]);
            } else if d <= r {
                rgba[px..px + 4].copy_from_slice(&[0x10, 0x10, 0x10, 0xFF]);
            }
        }
    }
    tauri::image::Image::new_owned(rgba, w as u32, h as u32)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // The exit watcher, the keeper and the core are this same exe; none may
    // become a second Luna window.
    if lifecycle::watch_from_args()
        || procs::keeper_from_args()
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
    // focus the first. The same variable turns on the dev caller (agents/);
    // the test flavor (scripts/test-build.ps1) has an identifier of its own anyway.
    let dev = std::env::var_os("LUNA_DEV_TOKEN").is_some();
    let builder = tauri::Builder::default();
    let builder = if dev {
        builder
    } else {
        // Must be the first plugin: a second launch focuses the running
        // instance instead of spawning a duplicate app + tray icon.
        // A second launch of a test build run in the background is a
        // script's, and must not bring the window up.
        builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if !background() {
                show_main(app);
            }
        }))
    };
    // Restores size, position and maximized state from last run. Not in the
    // background: it would bring the test window back on screen, and save
    // the off-screen place for the next run that is not.
    let builder = if background() { builder } else { builder.plugin(tauri_plugin_window_state::Builder::default().build()) };
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Here, past the single-instance check: a second launch must not
            // pull the profile from under the running Luna.
            #[cfg(windows)]
            paths::adopt_webview_profile(&app.path().app_local_data_dir()?);
            build_main(app.handle())?;
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
            log::start(&format!("starting luna {} build {} core {}", env!("CARGO_PKG_VERSION"), env!("LUNA_BUILD"), env!("LUNA_CORE_FP")));
            // The previous run's end is reported before anything new is logged.
            lifecycle::start(app.handle().clone());
            // The sessions, the hooks' listener, the agents, activity and power
            // all run in the core (coreproc.rs); the window finds or starts it.
            corelink::start();
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
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
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
            account_settings::settings_sync_state,
            account_settings::set_settings_parent,
            account_settings::set_settings_sync_on_start,
            account_settings::read_account_config,
            account_settings::check_account_config,
            account_settings::format_account_config,
            account_settings::write_account_config,
            accounts::list_accounts,
            accounts::create_account,
            accounts::delete_account,
            agents::spawn::account::agent_blocked_accounts,
            agents::spawn::account::set_agent_account,
            agents::privilege::helpers_above_caller,
            agents::privilege::set_helpers_above_caller,
            cli::cli_status,
            cli::cli_update_now,
            claude::defaults::claude_defaults,
            claude::limits::account_limits,
            claude::trust::claude_folder_trusted,
            claude::trust::claude_trust_folder,
            codex::defaults::codex_defaults,
            codex::hooks::codex_hooks_start,
            codex::hooks::set_codex_hooks_default,
            codex::hooks::keep_codex_hooks,
            codex::limits::codex_limits,
            codex::models::codex_models,
            codex::trust::codex_trusted_roots,
            codex::trust::set_codex_trusted_roots,
            library::library_overview,
            library::library_sync,
            library::library_set_source,
            library::library_set_instructions,
            library::library_set_accounts,
            library::library_set_folder,
            library::library_set_folder_skill,
            library::library_set_folder_instructions,
            library::library_skip,
            library::library_resolve,
            library::library_diff,
            library::library_read,
            library::library_write,
            library::library_check_skill,
            library::library_new_skill,
            library::library_delete_skill,
            library::library_new_instructions,
            library::library_rename_instructions,
            library::library_delete_instructions,
            library::library_open,
            log::append_log,
            media::save_media,
            open_link,
            browsers::browsers,
            browsers::set_browsers,
            tray_awake,
            corelink::core_call,
            corelink::core_send,
            corelink::core_state,
            corelink::core_restart,
            lifecycle::note_exit,
            pty::probe::saved_title,
            settings::get_accounts_root,
            settings::set_accounts_root,
            watchdog::ui_heartbeat,
            worktree::fetch_before_worktree,
            worktree::set_fetch_before_worktree,
            asking::waiting_marker,
            asking::set_waiting_marker,
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
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                emit::stop();
                power::clear();
            }
        });
}
