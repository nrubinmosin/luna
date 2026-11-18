//! Desktop notifications, the page's `notify(title, body, chatId)`
//! (src/shared/lib/notify.ts) as Tauri's notification plugin sends it: on
//! `notify-rust`, the crate the plugin is built on, a Windows toast or a macOS
//! notification. A click on one calls the window back (`on_click`) with the
//! chat it is about, if any, and the window brings itself up and opens that
//! chat, as lib.rs does with the plugin's `chatId`.
//!
//! On Windows the toast goes out under the app's AUMID (`desktop::identifier`),
//! the one the installer gives the Start-menu shortcut: Windows shows a toast
//! of an unpackaged app only under the AUMID of a shortcut of its own. Run
//! from a cargo target folder there is no such shortcut, and the toast goes
//! out under PowerShell's, as the plugin does. A click reaches the running
//! Luna only: a toast left in the notification center after Luna has quit
//! does nothing, as today.
//!
//! On macOS the notification goes out under the bundle's identifier when Luna
//! runs from its `.app`, which LaunchServices knows once the app has been
//! opened; no signing is asked for. macOS delivers the click on the main
//! run loop, which the window runs. Off a bundle (a bare exe), macOS picks
//! the sender. Linux, which Luna does not support, shows the notification
//! and reports no click.

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

/// Shows `notice`. Best effort, as the page's was: an error is said, and
/// nothing waits on it.
pub fn show(notice: Notice) -> Result<(), String> {
    let mut n = notify_rust::Notification::new();
    n.summary(&notice.title).body(&notice.body);
    if notice.sound {
        n.sound_name(sound_name());
    }
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        if let Some(id) = exe.parent().and_then(app_id) {
            n.app_id(id);
        }
    }
    #[cfg(target_os = "macos")]
    mac_sender();
    let chat = notice.chat_id;
    // Shown from a thread of its own, which then waits for the answer:
    // a toast's activation, macOS's response.
    std::thread::Builder::new()
        .name("notification".into())
        .spawn(move || {
            #[cfg(any(windows, target_os = "macos"))]
            {
                let Ok(handle) = n.show() else { return };
                let _ = handle.wait_for_response(|response: &notify_rust::NotificationResponse| {
                    // Taken out first: the callback runs without the lock.
                    let f = ON_CLICK.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(f) = f {
                        answer(response, chat.clone(), &*f);
                    }
                });
            }
            #[cfg(not(any(windows, target_os = "macos")))]
            {
                let _ = (n.show(), chat);
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Calls `f` for a click on the notification itself, with its chat. The
/// buttons it might have had, and its closing, are no click.
#[cfg(any(windows, target_os = "macos", test))]
fn answer(response: &notify_rust::NotificationResponse, chat: Option<String>, f: &dyn Fn(Option<String>)) {
    if matches!(response, notify_rust::NotificationResponse::Default) {
        f(chat);
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
    fn a_click_on_the_notification_opens_its_chat_and_nothing_else_does() {
        let seen = Mutex::new(Vec::new());
        let f = |chat: Option<String>| seen.lock().unwrap().push(chat);
        answer(&notify_rust::NotificationResponse::Default, Some("chat-1".into()), &f);
        answer(&notify_rust::NotificationResponse::Default, None, &f);
        answer(&notify_rust::NotificationResponse::Action("snooze".into()), Some("chat-2".into()), &f);
        assert_eq!(*seen.lock().unwrap(), [Some("chat-1".to_string()), None]);
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
