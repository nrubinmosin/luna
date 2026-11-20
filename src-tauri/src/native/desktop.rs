//! The desktop services (desktop/) wired to the native window: what the
//! window asks of the desktop (`luna_window::Desktop`), the tray and its menu,
//! a notification's click, a second launch, the end of the Windows session,
//! the updater's commands and the window's place. What wants the window
//! itself (to come up, to hide, to quit) is a `Request` in its mailbox, from
//! whatever thread asks.

use super::{NativeHost, Writer};
use crate::desktop::{clipboard, dialog, notify, tray, updater, window_state};
use luna_window::desktop::{
    event, Clip, CoreId, LocalPending, Notice, Offer, OnDisplay, Parent, Pending, Pick, PickKind, Placement, Rect, Screen, UpdateCheck,
    CORE_CHANGED,
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
    // there: whichever Windows tells first, what the host holds is written
    // (`drain_at_session_end`) and the session's end goes in the log with
    // its reason and how that went (sysmenu.rs), and gpui's hidden window
    // then quits the app as the tray's Quit does (`on_quit`).
    crate::sysmenu::drain_with_session(super::drain_at_session_end);
    crate::sysmenu::exit_with_session(false);
    #[cfg(all(windows, luna_test))]
    test_hook::install();
}

/// As the app quits, on the window's thread: the tray goes (its icon would
/// stay in the notification area until the pointer passed over it). The
/// window's last place is written with the other writers (native.rs
/// `drain_writers`).
pub(super) fn stop() {
    TRAY_UP.store(false, Ordering::SeqCst);
    TRAY.with(|slot| drop(slot.borrow_mut().take()));
}

/// An update's way out, which ends the window's app: through its loop, or
/// at once where it is gone.
fn quit_app() {
    if !request(Request::Quit) {
        crate::lifecycle::end_process();
    }
}

/// A way out from the tray, claimed (`lifecycle::leave`) and waiting for the
/// window to write its chat list: it goes as the app quits (`take_way`,
/// native.rs `on_quit`), or is set aside, its claim let go of, if the window
/// turns the quit down (`quit_refused`). Until it goes nothing of it has
/// happened: the run is not marked as ended, the core is up, no new window
/// started.
pub(super) enum Way {
    /// The tray's Quit: the core goes too, once the window has quit.
    Quit,
    /// Restart window: this exe started again once the window has quit.
    Restart(PathBuf),
}

static WAY: Mutex<Option<(Way, crate::lifecycle::Leaving)>> = Mutex::new(None);

/// A way out the window turned down (`quit_refused`), its claim let go of:
/// the window's notice offers Retry and Quit anyway, and a quit that comes
/// of either is still that way out's.
static REFUSED: Mutex<Option<Way>> = Mutex::new(None);

fn ways() -> std::sync::MutexGuard<'static, Option<(Way, crate::lifecycle::Leaving)>> {
    WAY.lock().unwrap_or_else(|e| e.into_inner())
}

fn refused() -> std::sync::MutexGuard<'static, Option<Way>> {
    REFUSED.lock().unwrap_or_else(|e| e.into_inner())
}

/// The way out the app's quit is for, with its claim on the way out: the
/// one a tray's Quit or Restart window asked for, or one the window turned
/// down and then quit for after all (Retry, Quit anyway), claimed again
/// unless another way out holds the claim now (an update's install).
pub(super) fn take_way() -> Option<(Way, crate::lifecycle::Leaving)> {
    if let Some(way) = ways().take() {
        return Some(way);
    }
    let way = refused().take()?;
    crate::lifecycle::leave().map(|leaving| (way, leaving))
}

/// Claims the way out for `way` and asks the window to quit, once its chat
/// list is written (workspace.rs `quit`). Nothing while another way out
/// holds the claim. Where there is no window loop left to ask, it goes now.
fn leave_by(way: Way) {
    let Some(leaving) = crate::lifecycle::leave() else { return };
    let quit = matches!(way, Way::Quit);
    *refused() = None;
    *ways() = Some((way, leaving));
    // The Quit hides at once: the core's shutdown after the window's takes
    // seconds.
    if quit {
        request(Request::Hide);
    }
    if !request(Request::Quit) {
        if let Some((way, leaving)) = take_way() {
            finish(way, leaving);
        }
        crate::lifecycle::end_process();
    }
}

/// What a way out does once the window has written its chat list and quit
/// (or there is no window): the run marked as ended on purpose, then the
/// core shut down (Quit) or the new window started (Restart window). As the
/// Windows session ends neither: the core and the window go with it.
pub(super) fn finish(way: Way, leaving: crate::lifecycle::Leaving) {
    leaving.go();
    let ending = crate::sysmenu::session_ending();
    match way {
        Way::Quit => {
            crate::lifecycle::exiting(crate::lifecycle::QUIT_REASON);
            if !ending {
                crate::corelink::shut_down();
            }
        }
        Way::Restart(exe) => {
            crate::lifecycle::exiting(crate::lifecycle::RESTART_REASON);
            if !ending {
                if let Err(e) = crate::lifecycle::start_window_after(&exe, false) {
                    crate::log::error("app", &format!("the window quit for a restart, but the new one did not start: {e}"));
                }
            }
        }
    }
}

/// `quit_refused` `{ why }`, sent on the window's thread: the window turned
/// down the quit a way out asked for (its chat list was not written) and
/// stays. The claim is let go of, so the core stays up and another way out
/// is not refused; nothing of it was marked. The way out is kept aside for
/// the notice's Retry and Quit anyway (`take_way`).
pub(super) fn quit_refused(args: Value) -> Result<Value, String> {
    let why = args.get("why").and_then(Value::as_str).unwrap_or("no reason given");
    let Some((way, leaving)) = ways().take() else { return Ok(Value::Null) };
    let what = match way {
        Way::Quit => "Quit",
        Way::Restart(_) => "Restart window",
    };
    crate::log::warn("app", &format!("{what} from the tray waits: the window's chat list was not saved ({why})"));
    drop(leaving);
    *refused() = Some(way);
    Ok(Value::Null)
}

/// The tray's Restart window: a new window of this kind, the core left
/// alone, started once this one has written its chat list and quit.
fn restart() {
    match std::env::current_exe() {
        Ok(exe) => leave_by(Way::Restart(exe)),
        Err(e) => crate::log::warn("app", &format!("cannot restart: {e}")),
    }
}

/// The crash screen's Restart window, the tray's own: a new window, the core
/// left alone. Does nothing while another way out holds the exit.
pub(super) fn restart_window(_: Value) -> Result<Value, String> {
    restart();
    Ok(Value::Null)
}

/// The tray's Quit: the window, once its chat list is written, then the core
/// and its sessions. The core goes after the window, not before: a window
/// that could not write its list stays, and stays with its core.
fn quit() {
    leave_by(Way::Quit);
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

/// The window's place written now, as an install flushes what Luna holds.
pub(super) fn flush_placement(within: Duration) -> Result<(), String> {
    placement_writer().flush(within)
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

/// What a download or an install says when the offer the window confirmed is
/// no longer the one on offer: the window knows it, and checks again.
const CHANGED: &str = luna_window::desktop::OFFER_CHANGED;

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
    /// Refused while an install is under way. The offer's number, which the
    /// window's download and install name (`asked`).
    fn offer(&self, offered: Option<updater::Release>) -> Result<u64, String> {
        let mut s = self.lock();
        if s.installing {
            return Err(INSTALLING.into());
        }
        if !same_release(&s.offered, &offered) {
            s.generation += 1;
            s.downloaded = None;
        }
        s.offered = offered;
        Ok(s.generation)
    }

    /// The release on offer, to download, and its number; the download is
    /// under way until what is returned with them is dropped. `asked`, the
    /// number of the offer the window confirmed: refused if another is on
    /// offer now.
    fn begin_download(&self, asked: Option<u64>) -> Result<(u64, updater::Release, Downloading<'_>), String> {
        let mut s = self.lock();
        if s.installing {
            return Err(INSTALLING.into());
        }
        if asked.is_some_and(|g| g != s.generation) {
            return Err(CHANGED.into());
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
    /// is until what is returned with it is dropped (`Installing`); `asked`,
    /// as `begin_download`'s.
    fn begin_install(&self, asked: Option<u64>) -> Result<(updater::Downloaded, Installing<'_>), String> {
        let mut s = self.lock();
        if s.installing {
            return Err(INSTALLING.into());
        }
        if asked.is_some_and(|g| g != s.generation) {
            return Err(CHANGED.into());
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

/// The offer a download or an install is for: the `generation` its check
/// answered with.
#[derive(Deserialize)]
struct Asked {
    generation: u64,
}

/// `update_download` `{ generation }`: the offered release's installer,
/// checked and staged, `update://progress` told as it comes; `{ version,
/// coreFingerprint }`, the core fingerprint the setup's signature names (null
/// where it names none), on which the window decides whether the sessions
/// stop, not on the offer's, which `latest.json` gave unsigned. Refused
/// unless `generation` is the offer on offer now: the one the window
/// confirmed.
pub(super) fn update_download(args: Value) -> Result<Value, String> {
    let Asked { generation } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    download_with(&UPDATES, &updater::Config::luna(), &crate::paths::data_dir(), Some(generation))
}

/// What an install's refusal begins with when what Luna holds (the chat
/// list, the theme, the window's place) could not be written first: the
/// caller offers Retry (the same call) and Install anyway (`anyway: true`).
pub(super) const UNSAVED: &str = "unsaved: ";

/// What an install is asked for (`update_install`): `generation`, the offer
/// (None in tests, any); `core`, the core the window stopped the sessions in,
/// or kept them in, which the installer starts only under (`install_with`).
struct Install {
    generation: Option<u64>,
    reason: String,
    anyway: bool,
    core: CoreId,
    /// The sessions were kept running in `core`: the window found the
    /// release's signed core to be that one (`check_kept`).
    keep: bool,
}

/// `update_install` `{ generation, reason, anyway, core, keep }`: what Luna
/// holds written, the downloaded installer started, then the app quits so
/// that the installer finds no window running. The setup's Restart Manager
/// closes Luna by force once it runs (installer.nsi `LunaCheckIfRunning`), so
/// a change not written by then is lost: where it cannot be written the
/// install is refused with `UNSAVED` and its reason, unless `anyway` is true,
/// the person's choice to install without it. Refused with `CORE_CHANGED`
/// unless `core` is the core connected as the installer is about to start,
/// and, with `keep` (the sessions left running in it), unless the setup's
/// signature names that core's fingerprint (`check_kept`).
pub(super) fn update_install(args: Value) -> Result<Value, String> {
    #[derive(Deserialize)]
    struct Args {
        generation: u64,
        reason: String,
        #[serde(default)]
        anyway: bool,
        core: CoreId,
        #[serde(default)]
        keep: bool,
    }
    let Args { generation, reason, anyway, core, keep } = serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
    let install = Install { generation: Some(generation), reason, anyway, core, keep };
    install_with(&UPDATES, install, super::flush_for_install, start_setup, quit_app)
}

/// An install that leaves the sessions running (`keep`) goes ahead only over
/// the core the setup's signature names (`Downloaded::core_fingerprint`):
/// `running`, the fingerprint of the core they run in, must be it. The
/// window decides this already (`Vitals::proceed`); here the host holds it
/// to what was signed, whatever the window says.
fn check_kept(downloaded: &updater::Downloaded, running: &str) -> Result<(), String> {
    match downloaded.core_fingerprint() {
        Some(signed) if signed == running => Ok(()),
        _ => {
            let why =
                "install not started: the sessions were kept running, but the installer's signature does not name the core they run in";
            crate::log::warn("updater", why);
            Err(why.into())
        }
    }
}

/// The install's last check of the core (`CoreHold::check`), handed to the
/// setup's start (`updater::install`), which asks it right before the start.
type Ready<'a> = &'a dyn Fn() -> Result<crate::corelink::Checked, String>;

/// The release's setup started (`install_with`).
fn start_setup(downloaded: &updater::Downloaded, ready: Ready<'_>) -> Result<(), String> {
    updater::install(&updater::Config::luna(), downloaded, ready)
}

fn check_with(updates: &Updates, config: &updater::Config) -> Result<UpdateCheck, String> {
    let offered = updater::check(config).inspect_err(|e| crate::log::warn("updater", &format!("check failed: {e}")))?;
    let generation = updates.offer(offered.clone()).inspect_err(|e| crate::log::warn("updater", &format!("check not taken: {e}")))?;
    if let Some(r) = &offered {
        crate::log::info("updater", &format!("{} is available", r.version));
    }
    let offer = offered.map(|r| Offer {
        version: r.version.to_string(),
        notes: r.notes,
        pub_date: r.pub_date,
        core_fingerprint: r.core_fingerprint,
        generation,
    });
    Ok(UpdateCheck { current: config.current.to_string(), offer })
}

fn download_with(updates: &Updates, config: &updater::Config, root: &Path, asked: Option<u64>) -> Result<Value, String> {
    let (generation, release, _downloading) = updates.begin_download(asked)?;
    let mut told: Option<Instant> = None;
    let got = updater::download(config, &release, root, |got, total| {
        let now = Instant::now();
        if told.is_none_or(|t| now.duration_since(t) >= PROGRESS_EVERY) || total == Some(got) {
            told = Some(now);
            crate::emit::to_ui(event::UPDATE_PROGRESS, json!({ "got": got, "total": total }));
        }
    });
    let got = got.inspect_err(|e| crate::log::warn("updater", &format!("download failed: {e}")))?;
    let answer = json!({ "version": got.version().to_string(), "coreFingerprint": got.core_fingerprint() });
    updates.finish_download(generation, got).inspect_err(|e| crate::log::warn("updater", e))?;
    Ok(answer)
}

/// Starts the installer staged for the offer on offer now, then `exit`s.
/// The offer is held as it is (`Updates::begin_install`) and the way out
/// claimed (`lifecycle::leave`) before anything waits: a check that
/// changes the offer meanwhile is refused rather than leave a setup it
/// superseded to start. While an install holds the way out, the tray's
/// Quit and Restart window do nothing (a Restart would start the very exe
/// being replaced), and neither does a second install; one that fails gives
/// both back, and takes back the reason it gave for the exit only if that
/// reason was its own. Before the setup starts, `flush` writes what Luna
/// holds; where it cannot, the install is refused (`UNSAVED`) and nothing is
/// started, unless `anyway`.
///
/// The core is held for the install from its start (`corelink::hold_for_install`):
/// no restart of it is admitted, nor a way back to another core, until the
/// setup has started or the install is off. Right before the setup starts,
/// its own checks done (`setup`, `updater::install`), the core connected
/// must be the one the install names (`CoreHold::check`), and stays so until
/// the setup has started: a restart admitted before the hold, or a core that
/// came up while the writers drained or the setup was checked, would run on
/// under the installer, so the install is refused with `CORE_CHANGED`.
fn install_with(
    updates: &Updates,
    install: Install,
    flush: impl FnOnce() -> Result<(), String>,
    setup: impl FnOnce(&updater::Downloaded, Ready<'_>) -> Result<(), String>,
    exit: impl FnOnce(),
) -> Result<Value, String> {
    let Install { generation, reason, anyway, core, keep } = install;
    let (downloaded, installing) = updates.begin_install(generation)?;
    if keep {
        // The core the sessions were kept in is no longer the one up: the
        // window's offer was made for it, so it is let go (`CORE_CHANGED`),
        // as the core's own check below would let it go.
        let Some(running) =
            crate::corelink::core_state().core.filter(|c| c.pid == core.pid && c.started == core.started && c.build == core.build)
        else {
            crate::log::warn("updater", "install not started: the core the sessions were kept running in is not the one up");
            return Err(CORE_CHANGED.into());
        };
        check_kept(&downloaded, &running.fingerprint)?;
    }
    let held = crate::corelink::hold_for_install(core)?;
    let leaving = crate::lifecycle::leave().ok_or("Luna is already quitting, restarting or installing an update")?;
    if let Err(why) = flush() {
        if !anyway {
            crate::log::warn("updater", &format!("install not started: {why}"));
            return Err(format!("{UNSAVED}{why}"));
        }
        crate::log::warn("updater", &format!("installing without what could not be written, as the person chose: {why}"));
    }
    // Said before, as the page does: on success the process ends at once.
    let noted = crate::lifecycle::exit_for_install(&reason);
    let started = setup(&downloaded, &|| held.check());
    match started {
        Ok(()) => {
            installing.started();
            held.handed_off();
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
    use crate::corelink::test_link::{core_id, TestLink};
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
        updates.begin_install(None).map(|(got, _installing)| got)
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
        let (generation, _, downloading) = updates.begin_download(None).unwrap();
        updates.finish_download(generation, staged("0.20.0")).unwrap();
        assert!(updates.begin_install(None).err().unwrap().contains("being downloaded"));
        drop(downloading);

        let (got, installing) = updates.begin_install(None).unwrap();
        assert_eq!(got.version(), &semver::Version::new(0, 20, 0));
        assert_eq!(updates.offer(Some(release("0.21.0", "https://a/2.exe"))).unwrap_err(), INSTALLING);
        assert_eq!(updates.offer(None).unwrap_err(), INSTALLING);
        assert_eq!(updates.begin_download(None).err().unwrap(), INSTALLING);
        assert_eq!(updates.begin_install(None).err().unwrap(), INSTALLING);
        assert_eq!(updates.lock().generation, generation, "the offer did not move");
        drop(installing);

        // Failed: a check may change the offer again.
        updates.offer(Some(release("0.21.0", "https://a/2.exe"))).unwrap();
        assert!(to_install(&updates).unwrap_err().contains("no update has been downloaded"));
        let (generation, _, downloading) = updates.begin_download(None).unwrap();
        updates.finish_download(generation, staged("0.21.0")).unwrap();
        drop(downloading);
        let (_, installing) = updates.begin_install(None).unwrap();
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
        let (generation, got, downloading) = updates.begin_download(None).unwrap();
        assert_eq!(got.version, semver::Version::new(0, 20, 0));
        assert!(updates.begin_download(None).err().unwrap().contains("being downloaded already"));
        // A check during the download withdraws the offer.
        updates.offer(None).unwrap();
        assert!(updates.finish_download(generation, staged("0.20.0")).unwrap_err().contains("changed while it downloaded"));
        drop(downloading);
        assert!(to_install(&updates).is_err());
        assert!(updates.begin_download(None).err().unwrap().contains("check for one first"));

        // Offered again: a download for it, then the same release offered once more.
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let (generation, _, downloading) = updates.begin_download(None).unwrap();
        updates.finish_download(generation, staged("0.20.0")).unwrap();
        drop(downloading);
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        assert_eq!(to_install(&updates).unwrap().version(), &semver::Version::new(0, 20, 0));
        // Another release, or the same version from elsewhere: nothing to install.
        updates.offer(Some(release("0.20.0", "https://a/other.exe"))).unwrap();
        assert!(to_install(&updates).unwrap_err().contains("no update has been downloaded"));
    }

    /// A download and an install are for the offer the window confirmed, by
    /// its number: once a check has put another on offer (another release,
    /// or none), both are refused, so a confirmation never installs a
    /// release it did not name.
    #[test]
    fn a_download_and_an_install_name_the_offer_they_are_for() {
        let updates = Updates::new();
        let confirmed = updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        assert_eq!(updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap(), confirmed, "the same release keeps it");
        let (generation, _, downloading) = updates.begin_download(Some(confirmed)).unwrap();
        updates.finish_download(generation, staged("0.20.0")).unwrap();
        drop(downloading);
        let newer = updates.offer(Some(release("0.21.0", "https://a/2.exe"))).unwrap();
        assert_ne!(newer, confirmed);
        assert_eq!(updates.begin_download(Some(confirmed)).err().unwrap(), CHANGED);
        assert_eq!(updates.begin_install(Some(confirmed)).err().unwrap(), CHANGED);
        // The commands take the number and nothing else.
        assert!(update_download(json!({})).unwrap_err().starts_with("bad arguments"));
        assert!(update_install(json!({ "reason": "x" })).unwrap_err().starts_with("bad arguments"));
        let unnamed = update_install(json!({ "generation": confirmed, "reason": "x", "anyway": false })).unwrap_err();
        assert!(unnamed.starts_with("bad arguments"), "an install names its core: {unnamed}");
    }

    /// An install of the core `pid`, of the offer on offer.
    fn install(pid: u32, anyway: bool) -> Install {
        Install { generation: None, reason: "test".into(), anyway, core: core_id(pid), keep: false }
    }

    /// Sessions kept running go only over the core the setup's signature
    /// names: one that names none, or another, is refused before anything is
    /// held or started. Where the core they were kept in is not the one up,
    /// the install is refused with `CORE_CHANGED`, which lets the window's
    /// offer go, as the core's own check does.
    #[test]
    fn sessions_are_kept_only_over_the_core_the_setup_was_signed_for() {
        let unsigned = updater::Downloaded::staged_for_test(semver::Version::new(0, 20, 0));
        assert!(check_kept(&unsigned, "0123456789abcdef").unwrap_err().contains("does not name the core"));
        let signed = updater::Downloaded::staged_for_test_with_core(semver::Version::new(0, 20, 0), "0123456789abcdef");
        assert_eq!(check_kept(&signed, "0123456789abcdef"), Ok(()));
        assert!(check_kept(&signed, "fedcba9876543210").is_err());
        let link = TestLink::new();
        let updates = Updates::new();
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let (generation, _, downloading) = updates.begin_download(None).unwrap();
        updates.finish_download(generation, unsigned).unwrap();
        drop(downloading);
        let exit = || panic!("nothing was installed");
        let kept = || Install { keep: true, ..install(1, false) };
        let welcome = |pid: u32| -> crate::ipc::Welcome {
            serde_json::from_value(json!({
                "pid": pid, "version": "0.20.0", "build": "test", "fingerprint": "0123456789abcdef", "started": 1_763_500_000_000u64
            }))
            .unwrap()
        };
        // No core up, or another one: the one they were kept in is gone.
        link.state(None);
        assert_eq!(install_with(&updates, kept(), || Ok(()), no_setup, exit), Err(CORE_CHANGED.to_owned()));
        link.state(Some(welcome(2)));
        assert_eq!(install_with(&updates, kept(), || Ok(()), no_setup, exit), Err(CORE_CHANGED.to_owned()));
        // That core up, and the setup's signature naming none.
        link.state(Some(welcome(1)));
        let refused = install_with(&updates, kept(), || Ok(()), no_setup, exit).unwrap_err();
        assert!(refused.contains("does not name the core"), "{refused}");
        link.state(None);
    }

    /// Updates with a setup staged for the offer.
    fn staged_offer() -> Updates {
        let updates = Updates::new();
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let (generation, _, downloading) = updates.begin_download(None).unwrap();
        updates.finish_download(generation, staged("0.20.0")).unwrap();
        drop(downloading);
        updates
    }

    /// What `no_setup` fails with once the core's check has passed.
    const NO_SETUP: &str = "no setup starts here";

    /// The setup's start as `updater::install` makes it, its own checks
    /// passed: the core's check asked last, then no installer ever runs here.
    fn no_setup(_: &updater::Downloaded, ready: Ready<'_>) -> Result<(), String> {
        ready()?;
        Err(NO_SETUP.into())
    }

    /// An install writes what Luna holds before its setup starts (the
    /// setup's Restart Manager ends Luna by force): where that cannot be
    /// done the setup is not started and the offer and the way out are given
    /// back, for Retry; with `anyway` it goes on to the setup.
    #[test]
    fn an_install_is_refused_while_what_luna_holds_cannot_be_written() {
        let link = TestLink::new();
        link.connect(Some(core_id(1)));
        let updates = staged_offer();
        let exit = || panic!("the app does not quit for an install that did not start");
        let refused = install_with(&updates, install(1, false), || Err("the chat list: disk full".into()), no_setup, exit).unwrap_err();
        assert_eq!(refused, format!("{UNSAVED}the chat list: disk full"));
        // Given back: asked again (Retry), and flushed this time, it goes on to the
        // setup.
        let flushed = std::cell::Cell::new(false);
        let flush = || {
            flushed.set(true);
            Ok(())
        };
        assert_eq!(install_with(&updates, install(1, false), flush, no_setup, exit), Err(NO_SETUP.to_owned()));
        assert!(flushed.get());
        // The person's choice: the flush that fails does not stop it.
        assert_eq!(install_with(&updates, install(1, true), || Err("disk full".into()), no_setup, exit), Err(NO_SETUP.to_owned()));
    }

    /// The setup starts only under the core the install names, looked at
    /// last, after the writers' drain and the setup's own checks: a restart
    /// admitted before the install is refused there, and so is a core that
    /// came up during the drain or those checks; a restart asked meanwhile is
    /// not admitted, nor a way back to another core. Each refusal gives the
    /// offer, the way out and the core back.
    #[test]
    fn an_installer_starts_only_under_the_core_the_install_names() {
        let link = TestLink::new();
        let updates = staged_offer();
        let exit = || panic!("no setup starts here");
        let refused = Err(CORE_CHANGED.to_owned());

        // The core named is the one connected: on to the setup, and while it
        // starts, no restart.
        link.connect(Some(core_id(1)));
        let starting = |_: &updater::Downloaded, ready: Ready<'_>| -> Result<(), String> {
            let _checked = ready()?;
            assert!(!link.restart(), "no restart while the setup starts");
            Err(NO_SETUP.into())
        };
        assert_eq!(install_with(&updates, install(1, false), || Ok(()), starting, exit), Err(NO_SETUP.to_owned()));

        // A restart admitted before the install, still under way.
        assert!(link.restart());
        let drained = std::cell::Cell::new(false);
        let drain = || {
            drained.set(true);
            Ok(())
        };
        assert_eq!(install_with(&updates, install(1, false), drain, no_setup, exit), refused);
        assert!(drained.get(), "refused at the setup's start, after the drain");
        link.restarted();

        // Another core connected during the drain, and a restart asked then.
        let replaced = || {
            assert!(!link.restart(), "no restart while the install holds the core");
            link.connect(Some(core_id(2)));
            Ok(())
        };
        assert_eq!(install_with(&updates, install(1, false), replaced, no_setup, exit), refused);

        // Another core connected while the setup was being checked; a way
        // back to another one then is not taken.
        let checked_meanwhile = |downloaded: &updater::Downloaded, ready: Ready<'_>| {
            link.lose();
            assert!(!link.rejoin(core_id(3)), "a way back to another core while held");
            link.connect(Some(core_id(3)));
            no_setup(downloaded, ready)
        };
        assert_eq!(install_with(&updates, install(2, false), || Ok(()), checked_meanwhile, exit), refused);

        // Given back: the offer, the way out, and the core, whose restart is
        // admitted again.
        assert_eq!(install_with(&updates, install(3, false), || Ok(()), no_setup, exit), Err(NO_SETUP.to_owned()));
        assert!(link.restart());
    }

    /// A download that ends in a panic does not keep every later one out.
    #[test]
    fn a_download_that_panics_lets_the_next_one_start() {
        let updates = Updates::new();
        updates.offer(Some(release("0.20.0", "https://a/1.exe"))).unwrap();
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _under_way = updates.begin_download(None).unwrap();
            panic!("the download's progress panicked");
        }));
        assert!(panicked.is_err());
        assert!(updates.begin_download(None).is_ok());
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
        let refused = install_with(&updates, install(1, false), || Ok(()), no_setup, exit).unwrap_err();
        assert!(refused.contains("no update has been downloaded"), "{refused}");
        assert!(download_with(&updates, &config, &root, None).unwrap_err().contains("check for one first"));
        let checked = check_with(&updates, &config).unwrap();
        assert_eq!(checked.current, "0.19.0");
        assert_eq!(checked.offer.as_ref().map(|o| o.version.as_str()), Some("0.20.0"));
        assert_eq!(download_with(&updates, &config, &root, None).unwrap(), json!({ "version": "0.20.0", "coreFingerprint": null }));
        assert_eq!(std::fs::read(updater::downloads_dir(&root).join(updater::file_name(&semver::Version::new(0, 20, 0)))).unwrap(), SETUP);
        assert!(to_install(&updates).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The download answers with the core fingerprint the setup's signature
    /// names, whatever `latest.json` said: that one is not signed.
    #[test]
    fn a_download_answers_with_the_signed_core_not_the_manifests() {
        let key = Key::new(*b"luna-key");
        let signature =
            key.sign_with_comment(SETUP, "timestamp:1763500000	file:setup.exe	version:0.20.0	core_fingerprint:0123456789abcdef");
        let installer = serve(vec![ok(SETUP)]);
        let mut manifest: Value = serde_json::from_str(&manifest_for("0.20.0", &format!("{installer}/setup.exe"), &signature)).unwrap();
        manifest["core_fingerprint"] = json!("fedcba9876543210");
        let endpoint = serve(vec![ok(manifest.to_string().as_bytes())]);
        let config =
            updater::Config { endpoint: format!("{endpoint}/latest.json"), pubkey: key.public(), current: semver::Version::new(0, 19, 0) };
        let root = std::env::temp_dir().join(format!("luna-native-update-core-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let updates = Updates::new();
        let offer = check_with(&updates, &config).unwrap().offer.unwrap();
        assert_eq!(offer.core_fingerprint.as_deref(), Some("fedcba9876543210"), "the offer says what latest.json says");
        let answer = download_with(&updates, &config, &root, None).unwrap();
        assert_eq!(answer, json!({ "version": "0.20.0", "coreFingerprint": "0123456789abcdef" }));
        assert_eq!(to_install(&updates).unwrap().core_fingerprint(), Some("0123456789abcdef"));
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
