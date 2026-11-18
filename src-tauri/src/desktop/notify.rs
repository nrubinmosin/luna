//! Desktop notifications, the page's `notify(title, body, chatId)`
//! (src/shared/lib/notify.ts) as Tauri's notification plugin sends it: a
//! Windows toast on `tauri-winrt-notification`, a macOS notification on
//! `notify-rust`, the crates the plugin is built on. A click on one calls the
//! window back (`on_click`) with the chat it is about, if any, and the window
//! brings itself up and opens that chat, as lib.rs does with the plugin's
//! `chatId`.
//!
//! On Windows the toast goes out under the app's AUMID (`desktop::identifier`),
//! the one the installer gives the Start-menu shortcut: Windows shows a toast
//! of an unpackaged app only under the AUMID of a shortcut of its own, and
//! drops any other without a word. Run from a cargo target folder there is no
//! such shortcut, and the toast goes out under PowerShell's, as the plugin
//! does. The click comes through the toast's own activation callback, which
//! Windows calls on a thread of its own; no thread waits on a toast, so one
//! dropped or never acted on leaves nothing behind (notify-rust's
//! `wait_for_response` would block a thread on it for good). A click reaches
//! the running Luna only: a toast left in the notification center after Luna
//! has quit does nothing, as today.
//!
//! On macOS the notification goes out under the bundle's identifier when Luna
//! runs from its `.app`, which LaunchServices knows once the app has been
//! opened; no signing is asked for. notify-rust has no callback there: a
//! thread waits for each answer, at most `WAITING_MAX` at once. macOS
//! delivers the answer on the main run loop, which the window runs. Off a
//! bundle (a bare exe), macOS picks the sender. Linux, which Luna does not
//! support, shows the notification and reports no click.

use std::sync::{Arc, Mutex};

/// One notification, as the page sends it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// The chat it is about: a click opens it.
    pub chat_id: Option<String>,
    /// With the system's own notification sound (the page's sound setting).
    pub sound: bool,
}

type OnClick = Arc<dyn Fn(Option<String>) + Send + Sync>;

static ON_CLICK: Mutex<Option<OnClick>> = Mutex::new(None);

/// What a click on a notification does: `f` gets the chat id it carried. Set
/// once, before the first notification; it runs on a thread of its own.
pub fn on_click(f: impl Fn(Option<String>) + Send + Sync + 'static) {
    *ON_CLICK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

/// The system's notification sound, by the name each platform knows it by.
/// Without one, Windows shows the toast silent.
pub fn sound_name() -> &'static str {
    if cfg!(windows) {
        "Default"
    } else if cfg!(target_os = "macos") {
        "Ping"
    } else {
        "message-new-instant"
    }
}

/// The AUMID a toast goes out under, for an exe in `exe_dir`: the app's,
/// except in a cargo target folder, which has no shortcut to carry it.
pub fn app_id(exe_dir: &std::path::Path) -> Option<&'static str> {
    let mut tail = exe_dir.components().rev().map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase());
    let last = tail.next().unwrap_or_default();
    let parent = tail.next().unwrap_or_default();
    if parent == "target" && (last == "debug" || last == "release") {
        None
    } else {
        Some(super::identifier())
    }
}

/// How many notifications may wait for their answer at once on macOS. Each
/// waits on a thread of its own, which notify-rust's `wait_for_response`
/// blocks until the person acts on it, and for one never acted on (left in
/// the notification center), for good: past this many, a notification is
/// sent without waiting, and a click on it brings Luna up, as macOS does,
/// without opening its chat.
#[cfg(target_os = "macos")]
const WAITING_MAX: usize = 4;

/// Shows `notice`. Best effort, as the page's was: an error is said, and
/// nothing waits on it.
pub fn show(notice: Notice) -> Result<(), String> {
    #[cfg(windows)]
    {
        use tauri_winrt_notification::{Duration, Sound, Toast};
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let id = exe.parent().and_then(app_id).unwrap_or(Toast::POWERSHELL_APP_ID);
        let chat = notice.chat_id;
        // As notify-rust builds it: the title, no subtitle, the body; the
        // system's sound or none. Windows calls `on_activated` on a thread of
        // its own, and only if the toast is acted on; nothing waits for it.
        Toast::new(id)
            .title(&notice.title)
            .text1("")
            .text2(&notice.body)
            .sound(notice.sound.then_some(Sound::Default))
            .duration(Duration::Short)
            .on_activated(move |action| {
                if toast_clicked(action.as_deref()) {
                    clicked(chat.clone());
                }
                Ok(())
            })
            .show()
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let mut n = notify_rust::Notification::new();
        n.summary(&notice.title).body(&notice.body);
        if notice.sound {
            n.sound_name(sound_name());
        }
        #[cfg(target_os = "macos")]
        {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static WAITING: AtomicUsize = AtomicUsize::new(0);
            mac_sender();
            let handle = n.show().map_err(|e| e.to_string())?;
            if WAITING.fetch_add(1, Ordering::SeqCst) >= WAITING_MAX {
                WAITING.fetch_sub(1, Ordering::SeqCst);
                // Dropped unwaited, the handle sends it as it goes.
                drop(handle);
                return Ok(());
            }
            let chat = notice.chat_id;
            let spawned = std::thread::Builder::new().name("notification".into()).spawn(move || {
                let _ = handle.wait_for_response(|response: &notify_rust::NotificationResponse| {
                    if matches!(response, notify_rust::NotificationResponse::Default) {
                        clicked(chat.clone());
                    }
                });
                WAITING.fetch_sub(1, Ordering::SeqCst);
            });
            if let Err(e) = spawned {
                WAITING.fetch_sub(1, Ordering::SeqCst);
                return Err(e.to_string());
            }
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = notice.chat_id;
            n.show().map(drop).map_err(|e| e.to_string())
        }
    }
}

/// Whether a toast's activation is a click on the toast itself, which opens
/// its chat: Windows names no action then. Its buttons name theirs.
#[cfg(any(windows, test))]
fn toast_clicked(action: Option<&str>) -> bool {
    action.is_none()
}

/// Calls the window back (`on_click`) for a click on a notification about
/// `chat`. Taken out of the lock first: the callback runs without it, and a
/// panic in it stays on this side of the OS's call.
#[cfg(any(windows, target_os = "macos", test))]
fn clicked(chat: Option<String>) {
    let f = ON_CLICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(f) = f {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(chat)));
    }
}

/// The sender macOS shows a notification under: the bundle's identifier
/// when Luna runs from its `.app`. Set once; notify-rust keeps it.
#[cfg(target_os = "macos")]
fn mac_sender() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let in_bundle = std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains(".app/Contents/MacOS/"));
        if in_bundle {
            let _ = notify_rust::set_application(super::identifier());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_click_on_the_toast_opens_its_chat_and_its_buttons_do_not() {
        assert!(toast_clicked(None));
        assert!(!toast_clicked(Some("snooze")));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        on_click(move |chat| log.lock().unwrap().push(chat));
        clicked(Some("chat-1".into()));
        clicked(None);
        assert_eq!(*seen.lock().unwrap(), [Some("chat-1".to_string()), None]);
        // A callback that panics is contained.
        on_click(|_| panic!("the window's callback panicked"));
        clicked(None);
        *ON_CLICK.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    #[test]
    fn a_toast_goes_out_under_the_apps_id_except_from_a_cargo_target() {
        let installed = Path::new("E:/Programs/Luna");
        assert_eq!(app_id(installed), Some(crate::desktop::identifier()));
        assert_eq!(app_id(Path::new("C:/src/luna/src-tauri/target/release")), None);
        assert_eq!(app_id(Path::new("C:/src/luna/src-tauri/target/debug")), None);
        // A folder merely called release is someone's install.
        assert_eq!(app_id(Path::new("D:/apps/release")), Some(crate::desktop::identifier()));
    }
}
