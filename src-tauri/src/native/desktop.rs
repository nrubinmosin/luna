//! The desktop services (desktop/) wired to the native window: what the
//! window asks of the desktop (`luna_window::Desktop`), the tray and its menu,
//! a notification's click, a second launch, the end of the Windows session,
//! the updater's commands and the window's place. What wants the window
//! itself (to come up, to hide, to quit) is a `Request` in its mailbox, from
//! whatever thread asks.

use super::{NativeHost, Writer};
use crate::desktop::{clipboard, dialog, notify, tray, updater, window_state};
use luna_window::desktop::{event, Clip, LocalPending, Notice, Offer, Parent, Pending, Pick, PickKind, Placement, Rect, UpdateCheck};
use luna_window::{Mailbox, Request};
use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The window's mailbox, for the requests of the tray, a toast, a second
/// launch and an update.
static WINDOW: OnceLock<Mailbox> = OnceLock::new();

/// Asks the window for `r`; false when there is no window loop to ask.
fn request(r: Request) -> bool {
    WINDOW.get().is_some_and(|m| m.request(r))
}

thread_local! {
    /// The tray, on the window's thread, which made it and drops it.
    static TRAY: RefCell<Option<tray::Tray>> = const { RefCell::new(None) };
}

/// The tray is up: closing the window hides it to the tray.
static TRAY_UP: AtomicBool = AtomicBool::new(false);

/// Starts the desktop services beside the open window, on its thread, as the
/// WebView window's `setup()` does: what brings the window up (a second
/// launch, a notification's click), the tray, the session's end. The single
/// instance was claimed before anything started (lib.rs `run`).
pub(super) fn start(mailbox: Mailbox) {
    let _ = WINDOW.set(mailbox);
    crate::desktop::on_bring_up(|| {
        request(Request::Show);
    });
    // A click on a notification brings the window up, from the tray if need
    // be, and one about a chat opens that chat in it.
    notify::on_click(|chat| {
        request(Request::Show);
        if let Some(chat_id) = chat {
            crate::emit::to_ui(event::OPEN_CHAT, json!({ "chatId": chat_id }));
        }
    });
    let actions = tray::Actions {
        open: Box::new(|| {
            request(Request::Show);
        }),
        restart: Box::new(restart),
        quit: Box::new(quit),
    };
    match tray::Tray::new(actions) {
        Ok(t) => {
            TRAY.with(|slot| *slot.borrow_mut() = Some(t));
            TRAY_UP.store(true, Ordering::SeqCst);
        }
        Err(e) => crate::log::warn("window", &format!("no tray, so closing the window quits: {e}")),
    }
    // Once the window, gpui's own hidden window and the tray's are all
    // there: whichever Windows tells first, the session's end goes in the
    // log with its reason (sysmenu.rs).
    crate::sysmenu::exit_with_session();
    #[cfg(windows)]
    if crate::paths::is_test() {
        test_hook::install();
    }
}

/// As the app quits, on the window's thread: the tray goes (its icon would
/// stay in the notification area until the pointer passed over it), and the
/// window's last place is written.
pub(super) fn stop() {
    TRAY_UP.store(false, Ordering::SeqCst);
    TRAY.with(|slot| drop(slot.borrow_mut().take()));
    if !placement_writer().drain(super::THEME_DRAIN) {
        crate::log::warn("window", "the window's place was not written in time");
    }
}

/// The ways out that end the window's app: through its loop, or at once
/// where it is gone.
fn quit_app() {
    if !request(Request::Quit) {
        crate::lifecycle::end_process();
    }
}

/// The tray's Restart window: a new window of this kind, the core left alone.
fn restart() {
    crate::lifecycle::restart(|| std::env::current_exe().map_err(|e| e.to_string()), false, quit_app);
}

/// The tray's Quit: the sessions, the core and the window.
fn quit() {
    crate::lifecycle::quit(
        || {
            request(Request::Hide);
        },
        quit_app,
    );
}

/// `tray_awake` `{ holders }`, sent on the window's thread: the dot on the
/// tray's icon and who holds the PC awake in its tooltip, or the plain icon.
pub(super) fn tray_awake(args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Args {
        holders: Option<String>,
    }
    let Args { holders } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    TRAY.with(|slot| match slot.borrow().as_ref() {
        Some(t) => t.set_awake(holders.as_deref()),
        None => Ok(()),
    })?;
    Ok(Value::Null)
}

fn placement_writer() -> &'static Arc<Writer<window_state::WindowState>> {
    static WRITER: OnceLock<Arc<Writer<window_state::WindowState>>> = OnceLock::new();
    WRITER.get_or_init(|| Writer::new("window-place", window_state::save))
}

impl luna_window::Desktop for NativeHost {
    fn has_tray(&self) -> bool {
        TRAY_UP.load(Ordering::SeqCst)
    }

    fn notify(&self, n: Notice) -> Result<(), String> {
        notify::show(notify::Notice { title: n.title, body: n.body, chat_id: n.chat_id, sound: n.sound })
    }

    fn read_clipboard(&self) -> Pending<Clip> {
        if let Some(file) = test_clipboard() {
            return off_thread("read the test clipboard", move || test_clip::read(&file));
        }
        off_thread("read the clipboard", || clipboard::read().map(clip))
    }

    fn write_clipboard_text(&self, text: String) -> Pending<()> {
        if let Some(file) = test_clipboard() {
            return off_thread("write the test clipboard", move || test_clip::write(&file, &text));
        }
        off_thread("write the clipboard", move || clipboard::write_text(&text))
    }

    fn pick(&self, pick: Pick, parent: Option<&dyn Parent>) -> LocalPending<Option<PathBuf>> {
        let ask = dialog::Ask {
            title: pick.title,
            start: pick.start,
            filters: pick.filters.into_iter().map(|(name, extensions)| dialog::Filter { name, extensions }).collect(),
        };
        let parent = parent.map(Over);
        let parent = parent.as_ref().map(|p| p as &dyn dialog::Parent);
        // The dialog is made here, on the window's thread; the future holds
        // neither the question nor the window.
        let picked: std::pin::Pin<Box<dyn std::future::Future<Output = Option<PathBuf>>>> = match pick.kind {
            PickKind::Folder => Box::pin(dialog::folder_async(&ask, parent)),
            PickKind::File => Box::pin(dialog::file_async(&ask, parent)),
        };
        Box::pin(async move { Ok(picked.await) })
    }

    fn placement(&self, displays: &[Rect], min: (f64, f64)) -> Option<Placement> {
        let state = window_state::load()?;
        let displays: Vec<window_state::Bounds> = displays.iter().map(|d| bounds(*d)).collect();
        let fitted = window_state::fit(state, &displays, min);
        Some(Placement { bounds: rect(fitted.bounds), maximized: fitted.maximized })
    }

    fn keep_placement(&self, p: Placement) {
        placement_writer().put(window_state::WindowState { bounds: bounds(p.bounds), maximized: p.maximized });
    }
}

fn bounds(r: Rect) -> window_state::Bounds {
    window_state::Bounds { x: r.x, y: r.y, width: r.width, height: r.height }
}

fn rect(b: window_state::Bounds) -> Rect {
    Rect { x: b.x, y: b.y, width: b.width, height: b.height }
}

/// The test build's stand-in for the clipboard: `LUNA_TEST_CLIPBOARD` names
/// a file the window's paste reads and its copy writes instead of the
/// system's, so a smoke run never touches the clipboard of the person at the
/// machine. Ignored by any other build.
fn test_clipboard() -> Option<PathBuf> {
    std::env::var_os("LUNA_TEST_CLIPBOARD").filter(|_| crate::paths::is_test()).map(PathBuf::from)
}

/// The stand-in's file: `{"text": "..."}`, `{"files": ["..."]}` or
/// `{"image": true}`, a copy writing `{"text": "..."}`. Files are taken as
/// the system's reader takes them: only when every path can be typed.
mod test_clip {
    use super::{clipboard, Clip};
    use serde::{Deserialize, Serialize};
    use std::path::{Path, PathBuf};

    #[derive(Default, Deserialize, Serialize)]
    #[serde(default)]
    struct File {
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        files: Vec<PathBuf>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        image: bool,
    }

    pub(super) fn read(path: &Path) -> Result<Clip, String> {
        let Ok(bytes) = std::fs::read(path) else { return Ok(Clip::Empty) };
        let file: File = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let typeable = |p: &PathBuf| p.to_str().is_some_and(clipboard::typeable_path);
        Ok(if !file.files.is_empty() && file.files.iter().all(typeable) {
            Clip::Files(file.files)
        } else if let Some(text) = file.text.filter(|t| !t.is_empty()) {
            Clip::Text(text)
        } else if file.image {
            Clip::Image { width: 0, height: 0, png: Vec::new() }
        } else {
            Clip::Empty
        })
    }

    pub(super) fn write(path: &Path, text: &str) -> Result<(), String> {
        let json = serde_json::to_vec(&File { text: Some(text.to_owned()), ..Default::default() }).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_test_clipboard_is_a_file() {
            let dir = std::env::temp_dir().join(format!("luna-test-clip-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("clipboard.json");
            assert_eq!(read(&file), Ok(Clip::Empty), "no file, nothing on it");
            std::fs::write(&file, r#"{"files":["/x y.png"],"text":"t"}"#).unwrap();
            assert_eq!(read(&file), Ok(Clip::Files(vec![PathBuf::from("/x y.png")])));
            // A path that cannot be typed: not taken as files.
            let quote = serde_json::json!({ "files": ["/x\"y.png"], "text": "t" });
            std::fs::write(&file, quote.to_string()).unwrap();
            assert_eq!(read(&file), Ok(Clip::Text("t".into())));
            std::fs::write(&file, r#"{"image":true}"#).unwrap();
            assert!(matches!(read(&file), Ok(Clip::Image { .. })));
            write(&file, "copied").unwrap();
            assert_eq!(read(&file), Ok(Clip::Text("copied".into())));
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn clip(c: clipboard::Clip) -> Clip {
    match c {
        clipboard::Clip::Empty => Clip::Empty,
        clipboard::Clip::Text(t) => Clip::Text(t),
        clipboard::Clip::Files(f) => Clip::Files(f),
        clipboard::Clip::Image(i) => Clip::Image { width: i.width, height: i.height, png: i.png },
    }
}

/// The window's handles as a dialog's parent takes them.
struct Over<'a>(&'a dyn Parent);

impl raw_window_handle::HasWindowHandle for Over<'_> {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.0.window_handle()
    }
}

impl raw_window_handle::HasDisplayHandle for Over<'_> {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.0.display_handle()
    }
}

/// `f` on a thread of its own, its answer awaited.
fn off_thread<T: Send + 'static>(what: &'static str, f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Pending<T> {
    let (tx, rx) = futures::channel::oneshot::channel();
    let spawned = std::thread::Builder::new().name("luna-desktop".into()).spawn(move || {
        let _ = tx.send(f());
    });
    Box::pin(async move {
        spawned.map_err(|e| format!("cannot {what}: {e}"))?;
        rx.await.unwrap_or_else(|_| Err(format!("cannot {what}: no answer")))
    })
}

/// The release the last check offered, and its installer once downloaded:
/// what `update_download` and `update_install` take, never anything the
/// window passes them.
struct Update {
    offered: Option<updater::Release>,
    downloaded: Option<updater::Downloaded>,
}

static UPDATE: Mutex<Update> = Mutex::new(Update { offered: None, downloaded: None });

/// A download under way: one at a time.
static DOWNLOADING: AtomicBool = AtomicBool::new(false);

/// How often a download's progress is told at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

fn update() -> std::sync::MutexGuard<'static, Update> {
    UPDATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// `update_check` `{}`: `UpdateCheck`. The test build is never offered one.
pub(super) fn update_check(_args: Value) -> Result<Value, String> {
    serde_json::to_value(check_with(&updater::Config::luna())?).map_err(|e| e.to_string())
}

/// `update_download` `{}`: the offered release's installer, checked and
/// staged, `update://progress` told as it comes; `{ version }`.
pub(super) fn update_download(_args: Value) -> Result<Value, String> {
    download_with(&updater::Config::luna(), &crate::paths::data_dir())
}

/// `update_install` `{ reason }`: the downloaded installer started, then the
/// app quits so that the installer finds no window running.
pub(super) fn update_install(args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Args {
        reason: String,
    }
    let Args { reason } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    install_with(&updater::Config::luna(), reason)
}

fn check_with(config: &updater::Config) -> Result<UpdateCheck, String> {
    let offered = updater::check(config).inspect_err(|e| crate::log::warn("updater", &format!("check failed: {e}")))?;
    let mut u = update();
    // A download for another version than the one now offered is let go.
    if u.downloaded.as_ref().map(|d| d.version()) != offered.as_ref().map(|r| &r.version) {
        u.downloaded = None;
    }
    u.offered = offered.clone();
    if let Some(r) = &offered {
        crate::log::info("updater", &format!("{} is available", r.version));
    }
    let offer = offered.map(|r| Offer {
        version: r.version.to_string(),
        notes: r.notes,
        pub_date: r.pub_date,
        core_fingerprint: r.core_fingerprint,
    });
    Ok(UpdateCheck { current: config.current.to_string(), offer })
}

fn download_with(config: &updater::Config, root: &Path) -> Result<Value, String> {
    let release = update().offered.clone().ok_or("no update is on offer: check for one first")?;
    if DOWNLOADING.swap(true, Ordering::SeqCst) {
        return Err("an update is being downloaded already".into());
    }
    let mut told: Option<Instant> = None;
    let got = updater::download(config, &release, root, |got, total| {
        let now = Instant::now();
        if told.is_none_or(|t| now.duration_since(t) >= PROGRESS_EVERY) || total == Some(got) {
            told = Some(now);
            crate::emit::to_ui(event::UPDATE_PROGRESS, json!({ "got": got, "total": total }));
        }
    });
    DOWNLOADING.store(false, Ordering::SeqCst);
    let got = got.inspect_err(|e| crate::log::warn("updater", &format!("download failed: {e}")))?;
    let version = got.version().to_string();
    update().downloaded = Some(got);
    Ok(json!({ "version": version }))
}

fn install_with(config: &updater::Config, reason: String) -> Result<Value, String> {
    let downloaded = update().downloaded.clone().ok_or("no update has been downloaded")?;
    // Said before, as the page does: on success the process ends at once.
    crate::lifecycle::note_exit(Some(reason));
    match updater::install(config, &downloaded) {
        Ok(()) => {
            quit_app();
            Ok(Value::Null)
        }
        Err(e) => {
            crate::lifecycle::note_exit(None);
            crate::log::warn("updater", &format!("install failed: {e}"));
            Err(e)
        }
    }
}

/// The test build's hook for a script driving it in the background: a
/// message to one of its own top-level windows does what the tray's menu
/// item would (`tray::on_menu`), or says whether the tray is up, so that the
/// close-to-tray and Quit paths are tried without a click. Only in the test
/// flavor; the release has no such message.
#[cfg(windows)]
mod test_hook {
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, WM_APP};

    /// The message: wParam 0 Open, 1 Restart window, 2 Quit, 3 asks whether
    /// the tray is up (1) or not (0).
    pub const MESSAGE: u32 = WM_APP + 0x4C;

    /// Distinct from sysmenu.rs's subclasses.
    const ID: usize = 0x4C;

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == MESSAGE {
            crate::log::info("window", &format!("test hook: tray item {}", wparam.0));
            return match wparam.0 {
                0 => {
                    crate::desktop::tray::on_menu(crate::desktop::tray::OPEN);
                    LRESULT(0)
                }
                1 => {
                    crate::desktop::tray::on_menu(crate::desktop::tray::RESTART);
                    LRESULT(0)
                }
                2 => {
                    crate::desktop::tray::on_menu(crate::desktop::tray::QUIT);
                    LRESULT(0)
                }
                _ => LRESULT(super::TRAY_UP.load(std::sync::atomic::Ordering::SeqCst) as isize),
            };
        }
        // SAFETY: passes on, unchanged, a message Windows handed this subclass.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    unsafe extern "system" fn hook(hwnd: HWND, _: LPARAM) -> BOOL {
        // SAFETY: called back from EnumThreadWindows below, on the thread
        // that owns `hwnd`.
        unsafe {
            let _ = SetWindowSubclass(hwnd, Some(proc_), ID, 0);
        }
        BOOL(1)
    }

    /// On every top-level window of the window's thread.
    pub fn install() {
        // SAFETY: the calling thread's own windows, subclassed from it.
        unsafe {
            let _ = EnumThreadWindows(GetCurrentThreadId(), Some(hook), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::updater::tests::{manifest_for, ok, serve, Key, SETUP};

    /// The updater's commands over a local server and a key of the test's:
    /// what a check offers is what the download takes, its progress is told,
    /// and nothing is installed without a download.
    #[test]
    fn the_update_commands_check_and_download_what_was_offered() {
        let key = Key::new(*b"luna-key");
        let signature = key.sign(SETUP, "0.20.0");
        let installer = serve(vec![ok(SETUP)]);
        let manifest = manifest_for("0.20.0", &format!("{installer}/Luna_0.20.0_x64-setup.exe"), &signature);
        let endpoint = serve(vec![ok(manifest.as_bytes())]);
        let config =
            updater::Config { endpoint: format!("{endpoint}/latest.json"), pubkey: key.public(), current: semver::Version::new(0, 19, 0) };
        let root = std::env::temp_dir().join(format!("luna-native-update-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        assert!(install_with(&config, "test".into()).unwrap_err().contains("no update has been downloaded"));
        assert!(download_with(&config, &root).unwrap_err().contains("check for one first"));
        let checked = check_with(&config).unwrap();
        assert_eq!(checked.current, "0.19.0");
        assert_eq!(checked.offer.as_ref().map(|o| o.version.as_str()), Some("0.20.0"));
        assert_eq!(download_with(&config, &root).unwrap(), json!({ "version": "0.20.0" }));
        assert_eq!(std::fs::read(updater::downloads_dir(&root).join(updater::file_name(&semver::Version::new(0, 20, 0)))).unwrap(), SETUP);
        assert!(update().downloaded.is_some());
        *update() = Update { offered: None, downloaded: None };
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_windows_place_is_fitted_to_the_displays_there_are_now() {
        let gone = rect(window_state::Bounds { x: 5000.0, y: 0.0, width: 1280.0, height: 800.0 });
        let display = Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1040.0 };
        let fitted =
            window_state::fit(window_state::WindowState { bounds: bounds(gone), maximized: false }, &[bounds(display)], (900.0, 600.0));
        assert_eq!(rect(fitted.bounds), Rect { x: 320.0, y: 120.0, width: 1280.0, height: 800.0 });
        assert_eq!(clip(clipboard::Clip::Text("a".into())), Clip::Text("a".into()));
    }
}
