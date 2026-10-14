//! Brings the window back when its webview dies.
//!
//! WebView2 runs the page in processes of its own. When those go — a crash,
//! a driver reset, something killing them from outside — the window stays up
//! and paints black, and nothing in Tauri says so: the sessions carry on in
//! here with nobody able to see them, until the whole app is restarted, which
//! ends them. The page beats every few seconds instead (`ui_heartbeat`), and a
//! window that has been on screen for a while without a beat is rebuilt from
//! its config. Nothing is lost by that: the sessions live on this side, and the
//! new page reattaches to them as it would after a reload.
//!
//! Only a window that is on screen is judged. A hidden or minimised page has
//! its timers throttled to a beat a minute or less, and coming back from the
//! tray must not read as a dead page; the clock starts when the window does.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// No beat for this long, on screen for at least this long: the page is gone.
const SILENCE_MS: u64 = 45_000;
const EVERY: Duration = Duration::from_secs(5);

static LAST_BEAT: AtomicU64 = AtomicU64::new(0);
/// Set from closing the old window until the new one is up: closing the
/// last window would otherwise end the app (lib.rs prevents that exit).
static REBUILDING: AtomicBool = AtomicBool::new(false);

pub fn rebuilding() -> bool {
    REBUILDING.load(Ordering::SeqCst)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tauri::command]
pub fn ui_heartbeat() {
    LAST_BEAT.store(now_ms(), Ordering::Relaxed);
}

/// Whether a window on screen since `shown_at` and last heard from at
/// `beat` has gone quiet. Pure, for the test.
fn silent(now: u64, beat: u64, shown_at: Option<u64>) -> bool {
    match shown_at {
        // Not judged until the page has beaten once: a slow first load is not a crash.
        Some(since) => beat > 0 && now.saturating_sub(since) >= SILENCE_MS && now.saturating_sub(beat) >= SILENCE_MS,
        None => false,
    }
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        let mut shown_at: Option<u64> = None;
        loop {
            std::thread::sleep(EVERY);
            let Some(w) = app.get_webview_window("main") else {
                shown_at = None;
                continue;
            };
            let on_screen = w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false);
            let now = now_ms();
            shown_at = if on_screen { Some(shown_at.unwrap_or(now)) } else { None };
            if !silent(now, LAST_BEAT.load(Ordering::Relaxed), shown_at) {
                continue;
            }
            crate::log::warn("app", "the window's page stopped answering; rebuilding the window");
            LAST_BEAT.store(0, Ordering::Relaxed);
            shown_at = None;
            rebuild(&app);
        }
    });
}

fn rebuild(app: &AppHandle) {
    let Some(old) = app.get_webview_window("main") else { return };
    REBUILDING.store(true, Ordering::SeqCst);
    let _ = old.destroy();
    // The label is free only once the old window is gone, a turn of the event
    // loop later.
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(600));
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(config) = handle.config().app.windows.iter().find(|w| w.label == "main").cloned() else {
                REBUILDING.store(false, Ordering::SeqCst);
                return;
            };
            match tauri::WebviewWindowBuilder::from_config(&handle, &config).and_then(|b| b.build()) {
                Ok(w) => {
                    crate::sysmenu::silence(&w);
                    crate::sysmenu::exit_with_session();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
                Err(e) => crate::log::error("app", &format!("could not rebuild the window: {e}")),
            }
            REBUILDING.store(false, Ordering::SeqCst);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_window_on_screen_and_quiet_long_enough_counts() {
        let t = 1_000_000;
        // Hidden: never.
        assert!(!silent(t, 1, None));
        // Never beaten yet: a first load still going.
        assert!(!silent(t, 0, Some(0)));
        // Just shown after a long stay in the tray: the clock starts now.
        assert!(!silent(t, t - 600_000, Some(t - 5_000)));
        // On screen a while, beating: fine.
        assert!(!silent(t, t - 5_000, Some(t - 600_000)));
        // On screen a while, silent: gone.
        assert!(silent(t, t - 60_000, Some(t - 600_000)));
    }
}
