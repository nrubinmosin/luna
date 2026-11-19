//! The desktop services (desktop/) wired to the native window: what the
//! window asks of the desktop (`luna_window::Desktop`), the tray and its menu,
//! a notification's click, a second launch, the end of the Windows session,
//! the updater's commands and the window's place. What wants the window
//! itself (to come up, to hide, to quit) is a `Request` in its mailbox, from
//! whatever thread asks.

use super::{NativeHost, Writer};
use crate::desktop::{clipboard, dialog, notify, tray, updater, window_state};
use luna_window::desktop::{
    event, Clip, LocalPending, Notice, Offer, OnDisplay, Parent, Pending, Pick, PickKind, Placement, Rect, Screen, UpdateCheck,
};
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
    // log with its reason (sysmenu.rs), and gpui's hidden window then quits
    // the app as the tray's Quit does (`on_quit`).
    crate::sysmenu::exit_with_session(false);
    #[cfg(all(windows, luna_test))]
    test_hook::install();
}

/// As the app quits, on the window's thread: the tray goes (its icon would
/// stay in the notification area until the pointer passed over it), and the
/// window's last place is written.
pub(super) fn stop() {
    TRAY_UP.store(false, Ordering::SeqCst);
    TRAY.with(|slot| drop(slot.borrow_mut().take()));
    if let Err(why) = placement_writer().flush(super::THEME_DRAIN) {
        crate::log::warn("window", &format!("the window's place was not written: {why}"));
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
        #[cfg(luna_test)]
        if let Some(file) = test_clipboard() {
            return off_thread("read the test clipboard", move || test_clip::read(&file));
        }
        off_thread("read the clipboard", || clipboard::read().map(clip))
    }

    fn write_clipboard_text(&self, text: String) -> Pending<()> {
        #[cfg(luna_test)]
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

    fn placement(&self, displays: &[Screen], min: (f64, f64)) -> Option<Placement> {
        let state = window_state::load()?;
        let displays: Vec<window_state::Display> =
            displays.iter().map(|d| window_state::Display { id: d.id.clone(), area: bounds(d.area) }).collect();
        let fitted = window_state::fit(state, &displays, min);
        let on = fitted.on.map(|o| OnDisplay { id: o.id, dx: o.dx, dy: o.dy });
        Some(Placement { bounds: rect(fitted.bounds), maximized: fitted.maximized, on })
    }

    fn keep_placement(&self, p: Placement) {
        let on = p.on.map(|o| window_state::OnDisplay { id: o.id, dx: o.dx, dy: o.dy });
        placement_writer().put(window_state::WindowState { bounds: bounds(p.bounds), maximized: p.maximized, on });
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
/// machine. Compiled into the test build alone.
#[cfg(luna_test)]
fn test_clipboard() -> Option<PathBuf> {
    std::env::var_os("LUNA_TEST_CLIPBOARD").map(PathBuf::from)
}

/// The stand-in's file: `{"text": "..."}`, `{"files": ["..."]}` or
/// `{"image": true}`, a copy writing `{"text": "..."}`. Files are taken as
/// the system's reader takes them: only when every path can be typed.
///
/// An image, or nothing at all, would have the window press the CLI's
/// image-paste key, and the CLI would read the real clipboard, the person's:
/// it reads as the text `IMAGE_KEY` instead, which the smoke run looks for.
#[cfg(luna_test)]
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

    /// What a paste types where the window would press the image key.
    pub(super) const IMAGE_KEY: &str = "[test clipboard: image key]";

    pub(super) fn read(path: &Path) -> Result<Clip, String> {
        let Ok(bytes) = std::fs::read(path) else { return Ok(Clip::Text(IMAGE_KEY.into())) };
        let file: File = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let typeable = |p: &PathBuf| p.to_str().is_some_and(|p| clipboard::typeable_path(p, cfg!(windows)));
        Ok(if !file.files.is_empty() && file.files.iter().all(typeable) {
            Clip::Files(file.files)
        } else if let Some(text) = file.text.filter(|t| !t.is_empty()) {
            Clip::Text(text)
        } else {
            // `image` or nothing: either would press the key.
            let _ = file.image;
            Clip::Text(IMAGE_KEY.into())
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
            let key = Ok(Clip::Text(IMAGE_KEY.into()));
            assert_eq!(read(&file), key, "no file: what would press the image key");
            std::fs::write(&file, r#"{"files":["/x y.png"],"text":"t"}"#).unwrap();
            assert_eq!(read(&file), Ok(Clip::Files(vec![PathBuf::from("/x y.png")])));
            // A path that cannot be typed: not taken as files.
            let quote = serde_json::json!({ "files": ["/x\ny.png"], "text": "t" });
            std::fs::write(&file, quote.to_string()).unwrap();
            assert_eq!(read(&file), Ok(Clip::Text("t".into())));
            // An image never presses the real key: the CLI would read the
            // person's clipboard.
            std::fs::write(&file, r#"{"image":true}"#).unwrap();
            assert_eq!(read(&file), key);
            std::fs::write(&file, r#"{}"#).unwrap();
            assert_eq!(read(&file), key);
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
        clipboard::Clip::Image => Clip::Image,
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

/// The updater's state: the release the last check offered, its installer
/// once downloaded, and whether a download or an install is under way. Each
/// offer has a number, one more each time a check offers another release or
/// none: a download is kept only while the offer it was made for is the one
/// on offer, and only the download of the offer on offer now is installed.
/// While an install is under way, from its claim on the way out to its
/// setup started, the offer stays as it is: a check, a download, a second
/// install wait for it to fail or refuse. What `update_download` and
/// `update_install` take, never anything the window passes them.
struct Updates(Mutex<UpdateState>);

struct UpdateState {
    /// The number of the offer on offer.
    generation: u64,
    offered: Option<updater::Release>,
    /// The installer staged, and the number of the offer it was made for.
    downloaded: Option<(u64, updater::Downloaded)>,
    downloading: bool,
    installing: bool,
}

static UPDATES: Updates = Updates::new();

/// How often a download's progress is told at most.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// What a check, a download or an install says while an install is under way.
const INSTALLING: &str = "an update is being installed";

/// The same release as far as its installer goes: what is fetched and what
/// it is checked against.
fn same_release(a: &Option<updater::Release>, b: &Option<updater::Release>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.version == b.version && a.url == b.url && a.signature == b.signature,
        (None, None) => true,
        _ => false,
    }
}

impl Updates {
    const fn new() -> Updates {
        Updates(Mutex::new(UpdateState { generation: 0, offered: None, downloaded: None, downloading: false, installing: false }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, UpdateState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A check's answer, on offer from now on: another release than the one
    /// on offer, or none, under a new number, and the download made for the
    /// old one let go; the same release keeps its number and its download.
    /// Refused while an install is under way.
    fn offer(&self, offered: Option<updater::Release>) -> Result<(), String> {
        let mut s = self.lock();
        if s.installing {
            return Err(INSTALLING.into());
        }
        if !same_release(&s.offered, &offered) {
            s.generation += 1;
            s.downloaded = None;
        }
        s.offered = offered;
        Ok(())
    }

    /// The release on offer, to download, and its number; the download is
    /// under way until what is returned with them is dropped.
    fn begin_download(&self) -> Result<(u64, updater::Release, Downloading<'_>), String> {
        let mut s = self.lock();
        if s.installing {
            return Err(INSTALLING.into());
        }
        let release = s.offered.clone().ok_or("no update is on offer: check for one first")?;
        if s.downloading {
            return Err("an update is being downloaded already".into());
        }
        s.downloading = true;
        Ok((s.generation, release, Downloading(self)))
    }

    /// Keeps `got`, downloaded for the offer numbered `generation`, if that
    /// offer is still the one on offer.
    fn finish_download(&self, generation: u64, got: updater::Downloaded) -> Result<(), String> {
        let mut s = self.lock();
        if s.generation != generation {
            return Err("the update on offer changed while it downloaded: check for one again".into());
        }
        s.downloaded = Some((generation, got));
        Ok(())
    }

    /// The installer staged for the offer on offer now, the offer held as it
    /// is until what is returned with it is dropped (`Installing`).
    fn begin_install(&self) -> Result<(updater::Downloaded, Installing<'_>), String> {
        let mut s = self.lock();
        if s.installing {
            return Err(INSTALLING.into());
        }
        if s.downloading {
            return Err("an update is being downloaded: install it once it is".into());
        }
        let got = match &s.downloaded {
            Some((generation, got)) if *generation == s.generation => got.clone(),
            _ => return Err("no update has been downloaded".into()),
        };
        s.installing = true;
        Ok((got, Installing { updates: self, started: false }))
    }
}

/// A download under way: however it ends, a panic included, the next may
/// start once this is dropped.
struct Downloading<'a>(&'a Updates);

impl Drop for Downloading<'_> {
    fn drop(&mut self) {
        self.0.lock().downloading = false;
    }
}

/// An install under way: the offer is held until it is dropped, unless its
/// setup started (`started`), after which the process ends.
struct Installing<'a> {
    updates: &'a Updates,
    started: bool,
}

impl Installing<'_> {
    fn started(mut self) {
        self.started = true;
    }
}

impl Drop for Installing<'_> {
    fn drop(&mut self) {
        if !self.started {
            self.updates.lock().installing = false;
        }
    }
}

/// `update_check` `{}`: `UpdateCheck`. The test build is never offered one.
pub(super) fn update_check(_args: Value) -> Result<Value, String> {
    serde_json::to_value(check_with(&UPDATES, &updater::Config::luna())?).map_err(|e| e.to_string())
}

/// `update_download` `{}`: the offered release's installer, checked and
/// staged, `update://progress` told as it comes; `{ version }`.
pub(super) fn update_download(_args: Value) -> Result<Value, String> {
    download_with(&UPDATES, &updater::Config::luna(), &crate::paths::data_dir())
}

/// `update_install` `{ reason }`: the downloaded installer started, then the
/// app quits so that the installer finds no window running.
pub(super) fn update_install(args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Args {
        reason: String,
    }
    let Args { reason } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    install_with(&UPDATES, &updater::Config::luna(), reason, quit_app)
}

fn check_with(updates: &Updates, config: &updater::Config) -> Result<UpdateCheck, String> {
    let offered = updater::check(config).inspect_err(|e| crate::log::warn("updater", &format!("check failed: {e}")))?;
    updates.offer(offered.clone()).inspect_err(|e| crate::log::warn("updater", &format!("check not taken: {e}")))?;
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

fn download_with(updates: &Updates, config: &updater::Config, root: &Path) -> Result<Value, String> {
    let (generation, release, _downloading) = updates.begin_download()?;
    let mut told: Option<Instant> = None;
    let got = updater::download(config, &release, root, |got, total| {
        let now = Instant::now();
        if told.is_none_or(|t| now.duration_since(t) >= PROGRESS_EVERY) || total == Some(got) {
            told = Some(now);
            crate::emit::to_ui(event::UPDATE_PROGRESS, json!({ "got": got, "total": total }));
        }
    });
    let got = got.inspect_err(|e| crate::log::warn("updater", &format!("download failed: {e}")))?;
    let version = got.version().to_string();
    updates.finish_download(generation, got).inspect_err(|e| crate::log::warn("updater", e))?;
    Ok(json!({ "version": version }))
}

/// Starts the installer staged for the offer on offer now, then `exit`s.
/// The offer is held as it is (`Updates::begin_install`) and the way out
/// claimed (`lifecycle::leave`) before anything waits: a check that
/// changes the offer meanwhile is refused rather than leave a setup it
/// superseded to start. While an install holds the way out, the tray's
/// Quit and Restart window do nothing (a Restart would start the very exe
/// being replaced), and neither does a second install; one that fails gives
/// both back, and takes back the reason it gave for the exit only if that
/// reason was its own.
fn install_with(updates: &Updates, config: &updater::Config, reason: String, exit: impl FnOnce()) -> Result<Value, String> {
    let (downloaded, installing) = updates.begin_install()?;
    let leaving = crate::lifecycle::leave().ok_or("Luna is already quitting, restarting or installing an update")?;
    // Said before, as the page does: on success the process ends at once.
    let noted = crate::lifecycle::exit_for_install(&reason);
    match updater::install(config, &downloaded) {
        Ok(()) => {
            installing.started();
            leaving.go();
            exit();
            Ok(Value::Null)
        }
        Err(e) => {
            if let Some(noted) = noted {
                crate::lifecycle::take_back_exit(noted);
            }
            drop(leaving);
            drop(installing);
            crate::log::warn("updater", &format!("install failed: {e}"));
            Err(e)
        }
    }
}

/// The test build's hook for a script driving it in the background: a
/// message to one of its own top-level windows does what the tray's menu
/// item would (`tray::on_menu`), or says whether the tray is up, so that the
/// close-to-tray, Restart and Quit paths are tried without a click. Compiled
/// into the test flavor alone (`luna_test`, build.rs): no other build has the
/// message. It answers only a message that carries this run's secret, which
/// it writes at start into the test build's data dir (`SECRET_FILE`), a
/// private file of this user's: a process that cannot read that file (another
/// user's; one that only finds the window) is refused.
#[cfg(all(windows, luna_test))]
mod test_hook {
    use std::sync::OnceLock;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
    use windows::Win32::UI::WindowsAndMessaging::{EnumThreadWindows, WM_APP};

    /// The message: wParam 0 Open, 1 Restart window, 2 Quit, 3 asks whether
    /// the tray is up (1) or not (0); lParam this run's secret, the decimal
    /// number `SECRET_FILE` holds. One without it is answered -1.
    pub const MESSAGE: u32 = WM_APP + 0x4C;

    /// Where the secret is, in the data dir.
    pub const SECRET_FILE: &str = "test-hook.secret";

    /// Distinct from sysmenu.rs's subclasses.
    const ID: usize = 0x4C;

    static SECRET: OnceLock<isize> = OnceLock::new();

    unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == MESSAGE {
            if SECRET.get() != Some(&lparam.0) {
                crate::log::warn("window", "test hook: a message without this run's secret, refused");
                return LRESULT(-1);
            }
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

    /// A fresh secret, written for the script, then the hook on every
    /// top-level window of the window's thread. No hook where the secret
    /// cannot be written, nor where it could not be kept private: on a
    /// volume that keeps no ACLs (FAT, exFAT) every user who reaches it
    /// would read it.
    pub fn install() {
        let dir = crate::paths::data_dir();
        if let Err(why) = keeps_acls(&dir) {
            crate::log::warn("window", &format!("no test hook: {}: {why}", dir.display()));
            return;
        }
        let secret = new_secret();
        let file = dir.join(SECRET_FILE);
        if let Err(e) = crate::secret::write_private(&file, secret.to_string().as_bytes()) {
            crate::log::warn("window", &format!("no test hook: {}: {e}", file.display()));
            return;
        }
        let _ = SECRET.set(secret);
        // SAFETY: the calling thread's own windows, subclassed from it.
        unsafe {
            let _ = EnumThreadWindows(GetCurrentThreadId(), Some(hook), LPARAM(0));
        }
    }

    /// Whether the volume `dir` is on keeps ACLs, asked through a handle on
    /// `dir`.
    fn keeps_acls(dir: &std::path::Path) -> Result<(), String> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;
        let held =
            std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0).open(dir).map_err(|e| e.to_string())?;
        let path = crate::desktop::volume::final_path(&held).map_err(|e| e.to_string())?;
        let volume = crate::desktop::volume::of(&held, &path).map_err(|e| e.to_string())?;
        if volume.persistent_acls() {
            Ok(())
        } else {
            Err(format!("its volume ({}) keeps no ACLs, so the secret could not be kept private", volume.file_system))
        }
    }

    /// 63 random bits, never 0 (the lParam of a message that carries none).
    pub fn new_secret() -> isize {
        let n = u64::from_str_radix(&crate::secret::random_hex(8), 16).unwrap_or(1) >> 1;
        n.max(1) as isize
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::updater::tests::{manifest_for, ok, serve, Key, SETUP};

    fn release(version: &str, url: &str) -> updater::Release {
        updater::Release {
            version: semver::Version::parse(version).unwrap(),
            notes: None,
            pub_date: None,
            url: url.into(),
            signature: "sig".into(),
            core_fingerprint: None,
        }
    }

    fn staged(version: &str) -> updater::Downloaded {
        updater::Downloaded::staged_for_test(semver::Version::parse(version).unwrap())
    }

    /// What an install would start now, the offer let go again at once.
    fn to_install(updates: &Updates) -> Result<updater::Downloaded, String> {
        updates.begin_install().map(|(got, _installing)| got)
    }

    /// An install holds the offer from its start to its setup's: a check
    /// that would change it, a download and a second install are refused
    /// meanwhile, so the setup started is the one the offer still names.
    /// One that fails lets go; one whose setup started keeps it. Nor does
    /// an install start while a download runs.
    #[test]
    fn an_install_holds_the_offer_until_its_setup_starts_or_it_fails() {
        let updates = Updates::new();
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let (generation, _, downloading) = updates.begin_download().unwrap();
        updates.finish_download(generation, staged("0.20.0")).unwrap();
        assert!(updates.begin_install().err().unwrap().contains("being downloaded"));
        drop(downloading);

        let (got, installing) = updates.begin_install().unwrap();
        assert_eq!(got.version(), &semver::Version::new(0, 20, 0));
        assert_eq!(updates.offer(Some(release("0.21.0", "https://a/2.exe"))).unwrap_err(), INSTALLING);
        assert_eq!(updates.offer(None).unwrap_err(), INSTALLING);
        assert_eq!(updates.begin_download().err().unwrap(), INSTALLING);
        assert_eq!(updates.begin_install().err().unwrap(), INSTALLING);
        assert_eq!(updates.lock().generation, generation, "the offer did not move");
        drop(installing);

        // Failed: a check may change the offer again.
        updates.offer(Some(release("0.21.0", "https://a/2.exe"))).unwrap();
        assert!(to_install(&updates).unwrap_err().contains("no update has been downloaded"));
        let (generation, _, downloading) = updates.begin_download().unwrap();
        updates.finish_download(generation, staged("0.21.0")).unwrap();
        drop(downloading);
        let (_, installing) = updates.begin_install().unwrap();
        installing.started();
        assert_eq!(updates.offer(None).unwrap_err(), INSTALLING, "held until the process ends");
    }

    /// A download is kept only for the offer still on offer, and only that
    /// offer's is installed: a check that withdraws or changes the offer
    /// while a download runs leaves nothing to install, one that offers the
    /// same release again keeps what was downloaded.
    #[test]
    fn a_download_counts_only_for_the_offer_still_on_offer() {
        let updates = Updates::new();
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let (generation, got, downloading) = updates.begin_download().unwrap();
        assert_eq!(got.version, semver::Version::new(0, 20, 0));
        assert!(updates.begin_download().err().unwrap().contains("being downloaded already"));
        // A check during the download withdraws the offer.
        updates.offer(None).unwrap();
        assert!(updates.finish_download(generation, staged("0.20.0")).unwrap_err().contains("changed while it downloaded"));
        drop(downloading);
        assert!(to_install(&updates).is_err());
        assert!(updates.begin_download().err().unwrap().contains("check for one first"));

        // Offered again: a download for it, then the same release offered once more.
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let (generation, _, downloading) = updates.begin_download().unwrap();
        updates.finish_download(generation, staged("0.20.0")).unwrap();
        drop(downloading);
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        assert_eq!(to_install(&updates).unwrap().version(), &semver::Version::new(0, 20, 0));
        // Another release, or the same version from elsewhere: nothing to install.
        updates.offer(Some(release("0.20.0", "https://a/other.exe"))).unwrap();
        assert!(to_install(&updates).unwrap_err().contains("no update has been downloaded"));
    }

    /// A download that ends in a panic does not keep every later one out.
    #[test]
    fn a_download_that_panics_lets_the_next_one_start() {
        let updates = Updates::new();
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _under_way = updates.begin_download().unwrap();
            panic!("the download's progress panicked");
        }));
        assert!(panicked.is_err());
        assert!(updates.begin_download().is_ok());
    }

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

        let updates = Updates::new();
        let exit = || panic!("nothing was installed");
        assert!(install_with(&updates, &config, "test".into(), exit).unwrap_err().contains("no update has been downloaded"));
        assert!(download_with(&updates, &config, &root).unwrap_err().contains("check for one first"));
        let checked = check_with(&updates, &config).unwrap();
        assert_eq!(checked.current, "0.19.0");
        assert_eq!(checked.offer.as_ref().map(|o| o.version.as_str()), Some("0.20.0"));
        assert_eq!(download_with(&updates, &config, &root).unwrap(), json!({ "version": "0.20.0" }));
        assert_eq!(std::fs::read(updater::downloads_dir(&root).join(updater::file_name(&semver::Version::new(0, 20, 0)))).unwrap(), SETUP);
        assert!(to_install(&updates).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The test hook's secret is never 0, the lParam of a message that
    /// carries none, and differs from run to run.
    #[cfg(all(windows, luna_test))]
    #[test]
    fn the_test_hooks_secret_is_never_the_empty_lparam() {
        let secrets: Vec<isize> = (0..64).map(|_| test_hook::new_secret()).collect();
        assert!(secrets.iter().all(|s| *s > 0));
        assert!(secrets.windows(2).any(|w| w[0] != w[1]));
    }

    #[test]
    fn the_windows_place_is_fitted_to_the_displays_there_are_now() {
        let gone = rect(window_state::Bounds { x: 5000.0, y: 0.0, width: 1280.0, height: 800.0 });
        let display = Rect { x: 0.0, y: 0.0, width: 1920.0, height: 1040.0 };
        let fitted = window_state::fit(
            window_state::WindowState { bounds: bounds(gone), maximized: false, on: None },
            &[window_state::Display { id: None, area: bounds(display) }],
            (900.0, 600.0),
        );
        assert_eq!(rect(fitted.bounds), Rect { x: 320.0, y: 120.0, width: 1280.0, height: 800.0 });
        assert_eq!(clip(clipboard::Clip::Text("a".into())), Clip::Text("a".into()));
    }
}
