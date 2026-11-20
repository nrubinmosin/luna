//! The window's side of the desktop services (luna_lib's `desktop/`): what
//! the window asks of the desktop through its host (`Desktop`, a part of
//! every `Host`), the commands and events of the tray and the updater, and
//! what the host asks of the window itself (`Request`): to come up from the
//! tray, a notification or a second launch, and to quit.
//!
//! Closing the window hides it while there is a tray to bring it back from,
//! as the WebView window does: the sessions run on in the core, and Quit is
//! the tray's. Without a tray (it could not be made, or Linux) closing it
//! quits. Its size, place and maximized state go to the host as they change,
//! half a second after the last change, and at once as it hides or the app
//! quits; it opens where the host says the last run left it. Neither in the
//! background, whose window is placed off screen on purpose.

use crate::host::Host;
use gpui::{
    App, AppContext, Bounds, Context, DisplayId, Entity, Global, Pixels, Size, Task, WeakEntity, Window, WindowBounds, point, px, size,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// An answer from the desktop, made off the window's thread.
pub type Pending<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// An answer awaited on the window's thread: a dialog, which on macOS runs
/// there.
pub type LocalPending<T> = Pin<Box<dyn Future<Output = Result<T, String>>>>;

/// What the window asks of the desktop beside the host's commands. Every
/// `Host` is one.
pub trait Desktop: Send + Sync + 'static {
    /// A tray is there to bring the window back from: closing the window
    /// hides it rather than quit.
    fn has_tray(&self) -> bool;

    /// A desktop notification. A click on it brings the window up
    /// (`Request::Show`) and, for one about a chat, sends `event::OPEN_CHAT`.
    fn notify(&self, notice: Notice) -> Result<(), String>;

    /// What a paste would put in a terminal.
    fn read_clipboard(&self) -> Pending<Clip>;

    /// Puts `text` on the clipboard in place of what was there.
    fn write_clipboard_text(&self, text: String) -> Pending<()>;

    /// A folder or a file picked in the system's dialog, modal to `parent`;
    /// None if the person cancels.
    fn pick(&self, pick: Pick, parent: Option<&dyn Parent>) -> LocalPending<Option<PathBuf>>;

    /// Where the window opens: where the last run left it, back on the
    /// display it was on where that is still there, held to `displays` (the
    /// primary first) and no smaller than `min`. None the first time, and in
    /// the background.
    fn placement(&self, displays: &[Screen], min: (f64, f64)) -> Option<Placement>;

    /// Keeps where the window is for the next run; returns at once.
    fn keep_placement(&self, placement: Placement);
}

/// A window a dialog can be modal to: a GPUI `Window` is one.
pub trait Parent: HasWindowHandle + HasDisplayHandle {}

impl<T: HasWindowHandle + HasDisplayHandle + ?Sized> Parent for T {}

/// A notification, as the page sends one (src/shared/lib/notify.ts).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// The chat it is about: a click opens it.
    pub chat_id: Option<String>,
    /// With the system's notification sound (the page's sound setting).
    pub sound: bool,
}

/// What is on the clipboard, by what a paste does with it: the first of
/// files, text, an image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Clip {
    #[default]
    Empty,
    Text(String),
    /// Typed as quoted paths.
    Files(Vec<PathBuf>),
    /// An image format is there, not read: the CLI reads it off the
    /// clipboard itself, at its image-paste key (so does a paste of `Empty`).
    Image,
}

/// What to pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickKind {
    Folder,
    File,
}

/// A picker's question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub kind: PickKind,
    pub title: Option<String>,
    /// The folder it opens in, or the nearest one of it that exists.
    pub start: Option<PathBuf>,
    /// For a file: each kind offered, by its name and extensions.
    pub filters: Vec<(String, Vec<String>)>,
}

/// A rectangle in GPUI's logical pixels: its top left and size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Where the window is: its normal bounds (where it goes when not
/// maximized), whether it is maximized, and the display it is on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub bounds: Rect,
    pub maximized: bool,
    /// The display it is on, where that display has an identity
    /// (`Screen::id`): kept with the window's place, and given back as the
    /// display the host fitted it to, which the window then opens on.
    pub on: Option<OnDisplay>,
}

/// The display a window is on, and where on it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OnDisplay {
    /// The display's `Screen::id`.
    pub id: String,
    /// The window's top left from the top left of the display's usable area.
    pub dx: f64,
    pub dy: f64,
}

/// A display, as the host is told of it: its usable area, and its identity
/// from one run to the next where gpui gives it one (on Windows a UUID made
/// from the monitor's device name), which neither its place among the
/// others nor its area is: either moves as monitors are added, removed or
/// scaled.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub id: Option<String>,
    pub area: Rect,
}

/// What the host asks of the window itself, through its `Mailbox`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Come up: shown, restored, brought to the front (in the background
    /// only shown, where it is, never activated).
    Show,
    /// Go to the tray at once: the tray's Quit, which takes seconds to stop
    /// the core first.
    Hide,
    /// Quit the app: the window's process ends, the core runs on.
    Quit,
}

/// The commands of the desktop's the window names, beside `cmd::ALL` and
/// `cmd::SENT` (a test in luna_lib holds its dispatch to these too).
pub mod cmd {
    /// `{ holders }`: the tray while the PC is held awake, who holds it, or
    /// null for the plain icon (the page's `tray_awake`). Sent.
    pub const TRAY_AWAKE: &str = "tray_awake";
    /// `{}`: `UpdateCheck`, a release newer than this build if there is one.
    pub const UPDATE_CHECK: &str = "update_check";
    /// `{}`: downloads and checks the release the last check offered, with
    /// `event::UPDATE_PROGRESS` as it comes; `{ version }`.
    pub const UPDATE_DOWNLOAD: &str = "update_download";
    /// `{ reason }`: starts the installer of the release downloaded, the
    /// reason the log gives for the exit, then the app quits.
    pub const UPDATE_INSTALL: &str = "update_install";
    /// `{ why }`: a `Request::Quit` the window turned down, the chat list not
    /// written (workspace.rs `quit`); the window stays. The host lets go of
    /// the way out it claimed for it (the tray's Quit, Restart window).
    /// Sent. A quit that goes ahead needs no word: the app's quit is it.
    pub const QUIT_REFUSED: &str = "quit_refused";

    pub const CALLED: &[&str] = &[UPDATE_CHECK, UPDATE_DOWNLOAD, UPDATE_INSTALL];
    pub const SENT: &[&str] = &[TRAY_AWAKE, QUIT_REFUSED];
}

/// The desktop's events the window takes, beside `event::ALL`.
pub mod event {
    /// `{ chatId }`: a notification about that chat was clicked.
    pub const OPEN_CHAT: &str = "ui://open-chat";
    /// `{ got, total }`: an update's download so far; total when the server
    /// says it.
    pub const UPDATE_PROGRESS: &str = "update://progress";

    pub const ALL: &[&str] = &[OPEN_CHAT, UPDATE_PROGRESS];
}

/// `update_check`'s answer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    /// This build's version.
    pub current: String,
    pub offer: Option<Offer>,
}

/// A release on offer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Offer {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    /// The core it was built with: the same as the running one's, and the
    /// update restarts only the window.
    pub core_fingerprint: Option<String>,
}

/// The chat a notification's click asked to open, until the workspace
/// takes it.
#[derive(Default)]
pub struct OpenChat(pub Option<String>);

impl Global for OpenChat {}

/// An update's download so far, `(got, total)`, until the status bar shows it.
#[derive(Default)]
pub struct UpdateProgress(pub Option<(u64, Option<u64>)>);

impl Global for UpdateProgress {}

/// The window's state as the desktop sees it.
#[derive(Default)]
pub(crate) struct Shown {
    /// Placed off screen, never activated (`Options::background`).
    pub background: bool,
    /// Hidden to the tray.
    pub hidden: bool,
}

impl Global for Shown {}

/// Hides or shows the window as far as the models know: the workspace's
/// and the status bar's looks slow or stop while it is hidden
/// (`observe_global`, which a replaced global always tells).
fn set_hidden(hidden: bool, cx: &mut App) {
    let background = cx.default_global::<Shown>().background;
    if cx.global::<Shown>().hidden != hidden {
        cx.set_global(Shown { background, hidden });
    }
}

/// One of the desktop's events, into the global it is about.
pub(crate) fn on_event(name: &str, payload: &Value, cx: &mut App) {
    match name {
        event::OPEN_CHAT => {
            if let Some(chat) = payload.get("chatId").and_then(Value::as_str) {
                cx.set_global(OpenChat(Some(chat.to_owned())));
            }
        }
        event::UPDATE_PROGRESS => {
            let got = payload.get("got").and_then(Value::as_u64).unwrap_or(0);
            let total = payload.get("total").and_then(Value::as_u64);
            cx.set_global(UpdateProgress(Some((got, total))));
        }
        _ => {}
    }
}

/// One of the host's requests, on the window's thread.
pub(crate) fn on_request(request: Request, cx: &mut App) {
    match request {
        Request::Show => {
            set_hidden(false, cx);
            let background = cx.default_global::<Shown>().background;
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, cx| show(window, background, cx));
            }
        }
        Request::Hide => {
            set_hidden(true, cx);
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, cx| hide(window, cx));
            }
        }
        // Once the chat list is written (workspace.rs `quit`).
        Request::Quit => crate::workspace::quit(cx),
    }
}

/// The window's close: hidden while there is a tray, its place kept first;
/// without one the app quits, once the chat list is written (workspace.rs
/// `quit`), and the window stays open until it does.
pub(crate) fn on_close(host: &Arc<dyn Host>, keeper: Option<&Entity<Keeper>>, window: &mut Window, cx: &mut App) -> bool {
    if !host.has_tray() {
        crate::workspace::quit(cx);
        return false;
    }
    if let Some(keeper) = keeper {
        keeper.update(cx, |k, cx| k.keep_now(window, cx));
    }
    set_hidden(true, cx);
    hide(window, cx);
    false
}

/// How long the window's place waits after a change before it is kept: a
/// drag moves it many times a second.
const KEEP_AFTER: Duration = Duration::from_millis(500);

/// Keeps the window's place with the host as it changes (`KEEP_AFTER` after
/// the last change), as it hides, and as the app quits.
pub(crate) struct Keeper {
    host: Arc<dyn Host>,
    latest: Option<Placement>,
    pending: Option<Task<()>>,
}

impl Keeper {
    /// Watches `window`'s bounds; lives for as long as the entity does.
    pub(crate) fn watch(host: Arc<dyn Host>, window: &mut Window, cx: &mut App) -> Entity<Keeper> {
        let keeper = cx.new(|cx: &mut Context<Keeper>| {
            cx.observe_window_bounds(window, |k, window, cx| k.changed(window, cx)).detach();
            Keeper { host, latest: None, pending: None }
        });
        cx.default_global::<Keepers>().0.push(keeper.downgrade());
        keeper
    }

    fn changed(&mut self, window: &Window, cx: &mut Context<Keeper>) {
        self.latest = Some(placement_of(window, cx));
        let timer = cx.background_executor().timer(KEEP_AFTER);
        self.pending = Some(cx.spawn(async move |k, cx| {
            timer.await;
            let _ = k.update(cx, |k, _| k.flush());
        }));
    }

    /// The place not kept yet, kept now.
    fn flush(&mut self) {
        if self.pending.take().is_some()
            && let Some(p) = self.latest.clone()
        {
            self.host.keep_placement(p);
        }
    }

    fn keep_now(&mut self, window: &Window, cx: &App) {
        self.pending = None;
        let p = placement_of(window, cx);
        self.latest = Some(p.clone());
        self.host.keep_placement(p);
    }
}

/// Every window's keeper, for the app's quit to flush (`flush_all`).
#[derive(Default)]
struct Keepers(Vec<WeakEntity<Keeper>>);

impl Global for Keepers {}

/// The places not kept yet, kept now: as the app quits, before the host's
/// own quit waits for what it was given (lib.rs `run`).
pub(crate) fn flush_all(cx: &mut App) {
    let keepers = std::mem::take(&mut cx.default_global::<Keepers>().0);
    for keeper in keepers.iter().filter_map(WeakEntity::upgrade) {
        keeper.update(cx, |k, _| k.flush());
    }
}

/// Where `window` is, as it is kept: on the display gpui says it is on.
pub(crate) fn placement_of(window: &Window, cx: &App) -> Placement {
    let mut p = placement(window.window_bounds(), restores_maximized(window));
    p.on = window.display(cx).and_then(|d| on_display(screen_id(&*d), rect(d.visible_bounds()), p.bounds));
    p
}

/// A display's identity from one run to the next: gpui's UUID for it.
fn screen_id(d: &dyn gpui::PlatformDisplay) -> Option<String> {
    d.uuid().ok().map(|u| u.to_string())
}

/// Where `bounds` are on the display of identity `id` whose usable area is
/// `area`; None for a display without one.
fn on_display(id: Option<String>, area: Rect, bounds: Rect) -> Option<OnDisplay> {
    id.map(|id| OnDisplay { id, dx: bounds.x - area.x, dy: bounds.y - area.y })
}

/// `bounds` as they are kept. A minimized window's are its normal bounds,
/// and gpui calls it windowed whatever it was: `minimized` says what it
/// comes back as, maximized or not, so that a window maximized, minimized
/// and quit from the tray opens maximized again.
fn placement(bounds: WindowBounds, minimized: Option<bool>) -> Placement {
    let (bounds, maximized) = match bounds {
        WindowBounds::Windowed(b) => (b, false),
        WindowBounds::Maximized(b) => (b, true),
        // Its restore size; it comes back windowed.
        WindowBounds::Fullscreen(b) => (b, false),
    };
    Placement { bounds: rect(bounds), maximized: minimized.unwrap_or(maximized), on: None }
}

/// While `window` is minimized, whether it comes back maximized (Windows'
/// `WPF_RESTORETOMAXIMIZED`); None while it is not, and off Windows, where
/// gpui's bounds say it.
fn restores_maximized(window: &Window) -> Option<bool> {
    #[cfg(windows)]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, IsIconic, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED};
        let hwnd = hwnd(window)?;
        // SAFETY: this window's own handle, on its thread; the struct is
        // sized as the call requires.
        unsafe {
            if !IsIconic(hwnd).as_bool() {
                return None;
            }
            let mut p = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            GetWindowPlacement(hwnd, &mut p).ok()?;
            Some(p.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        None
    }
}

fn rect(b: Bounds<Pixels>) -> Rect {
    Rect {
        x: f64::from(f32::from(b.origin.x)),
        y: f64::from(f32::from(b.origin.y)),
        width: f64::from(f32::from(b.size.width)),
        height: f64::from(f32::from(b.size.height)),
    }
}

fn bounds(r: Rect) -> Bounds<Pixels> {
    Bounds::new(point(px(r.x as f32), px(r.y as f32)), size(px(r.width as f32), px(r.height as f32)))
}

/// The bounds the window opens with, and the display they are on: where the
/// host says the last run left it, on the display it names (`Placement::on`)
/// where that is one of them, else on the one whose usable area holds its
/// center (`holding`); else `default` centered on the primary display. gpui
/// gives each display's bounds in its own logical pixels and takes the
/// window's in those of the display it is opened on, so the display goes
/// with them: without it gpui takes the primary's, and puts bounds on
/// another display back on the primary.
pub(crate) fn initial(host: &dyn Host, default: Size<Pixels>, min: Size<Pixels>, cx: &App) -> (WindowBounds, Option<DisplayId>) {
    let primary = cx.primary_display();
    let primary_id = primary.as_ref().map(|d| d.id());
    let screen = |d: &std::rc::Rc<dyn gpui::PlatformDisplay>| (d.id(), Screen { id: screen_id(&**d), area: rect(d.visible_bounds()) });
    let mut displays: Vec<(DisplayId, Screen)> = primary.iter().map(screen).collect();
    displays.extend(cx.displays().iter().filter(|d| Some(d.id()) != primary_id).map(screen));
    let screens: Vec<Screen> = displays.iter().map(|(_, s)| s.clone()).collect();
    let min = (f64::from(f32::from(min.width)), f64::from(f32::from(min.height)));
    match host.placement(&screens, min) {
        Some(p) => {
            let on = display_for(&screens, &p).map(|i| displays[i].0);
            let b = bounds(p.bounds);
            (if p.maximized { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) }, on)
        }
        None => (WindowBounds::Windowed(Bounds::centered(primary_id, default, cx)), primary_id),
    }
}

/// Which of `screens` the window placed at `p` opens on: the one `p` names
/// where it is there, else the one that holds its center.
fn display_for(screens: &[Screen], p: &Placement) -> Option<usize> {
    let named = p.on.as_ref().and_then(|on| screens.iter().position(|s| s.id.as_deref() == Some(on.id.as_str())));
    named.or_else(|| holding(&screens.iter().map(|s| s.area).collect::<Vec<_>>(), p.bounds))
}

/// Which of `displays` holds the center of `r`, the first that does; None
/// for a center on none of them. The host fits the window wholly onto one
/// (`Desktop::placement`), whose area then holds its center.
fn holding(displays: &[Rect], r: Rect) -> Option<usize> {
    let (x, y) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
    displays.iter().position(|d| x >= d.x && x < d.x + d.width && y >= d.y && y < d.y + d.height)
}

/// Shows the window: on Windows by its own handle, restored if minimized,
/// and brought to the front with the foreground right the tray's click, a
/// toast's or a second launch's hand-over gave this process (gpui's
/// `activate` would press Alt for it); in the background only shown, never
/// activated. macOS shows the app and the window.
///
/// On Windows the calls go on a task of the window's thread, after the
/// update this runs in: `ShowWindow` sends the window its size, move and
/// activation at once, which gpui could not take while the app is borrowed.
fn show(window: &mut Window, background: bool, cx: &mut App) {
    #[cfg(windows)]
    if let Some(hwnd) = hwnd(window) {
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE, SetForegroundWindow, ShowWindow};
        cx.foreground_executor()
            .spawn(async move {
                // SAFETY: this window's own handle, on the thread that made
                // it, which hides it and never destroys it before the app quits.
                unsafe {
                    if background {
                        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    } else {
                        let _ = ShowWindow(hwnd, if IsIconic(hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
                        let _ = SetForegroundWindow(hwnd);
                    }
                }
            })
            .detach();
    }
    // A window of the platform's own only: the test platform's has no handle.
    #[cfg(not(windows))]
    if !background && HasWindowHandle::window_handle(window).is_ok() {
        cx.activate(true);
        window.activate_window();
    }
}

/// Hides the window: on Windows by its own handle; macOS hides the app,
/// which `show` brings back. Elsewhere there is no tray to come back from,
/// and closing quits (`on_close`). On Windows on a task, as `show`.
fn hide(window: &mut Window, cx: &mut App) {
    #[cfg(windows)]
    if let Some(hwnd) = hwnd(window) {
        use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, ShowWindow};
        cx.foreground_executor()
            .spawn(async move {
                // SAFETY: as in `show`.
                let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            })
            .detach();
    }
    #[cfg(target_os = "macos")]
    if HasWindowHandle::window_handle(window).is_ok() {
        cx.hide();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = (window, cx);
}

/// The window's own handle, where it has one (the test platform's has none).
#[cfg(windows)]
fn hwnd(window: &Window) -> Option<windows::Win32::Foundation::HWND> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => Some(windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// The fake host's desktop (demo.rs): what it was asked, and what it answers.
#[cfg(any(test, feature = "shots"))]
#[derive(Default)]
pub struct FakeDesktop {
    pub tray: std::sync::atomic::AtomicBool,
    pub notices: std::sync::Mutex<Vec<Notice>>,
    pub clip: std::sync::Mutex<Clip>,
    /// While Some, a read waits here until the test answers it.
    pub reads: std::sync::Mutex<Option<Vec<futures::channel::oneshot::Sender<Clip>>>>,
    pub written: std::sync::Mutex<Vec<String>>,
    pub picks: std::sync::Mutex<Vec<Pick>>,
    pub picked: std::sync::Mutex<Option<PathBuf>>,
    pub placement: std::sync::Mutex<Option<Placement>>,
    pub kept: std::sync::Mutex<Vec<Placement>>,
}

#[cfg(any(test, feature = "shots"))]
impl Desktop for crate::demo::FakeHost {
    fn has_tray(&self) -> bool {
        self.desktop.tray.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn notify(&self, notice: Notice) -> Result<(), String> {
        self.desktop.notices.lock().unwrap().push(notice);
        Ok(())
    }

    fn read_clipboard(&self) -> Pending<Clip> {
        if let Some(waiting) = self.desktop.reads.lock().unwrap().as_mut() {
            let (tx, rx) = futures::channel::oneshot::channel();
            waiting.push(tx);
            return Box::pin(async move { rx.await.map_err(|_| "no answer".to_string()) });
        }
        let clip = self.desktop.clip.lock().unwrap().clone();
        Box::pin(async move { Ok(clip) })
    }

    fn write_clipboard_text(&self, text: String) -> Pending<()> {
        self.desktop.written.lock().unwrap().push(text);
        Box::pin(async { Ok(()) })
    }

    fn pick(&self, pick: Pick, _parent: Option<&dyn Parent>) -> LocalPending<Option<PathBuf>> {
        self.desktop.picks.lock().unwrap().push(pick);
        let picked = self.desktop.picked.lock().unwrap().clone();
        Box::pin(async move { Ok(picked) })
    }

    fn placement(&self, _displays: &[Screen], _min: (f64, f64)) -> Option<Placement> {
        self.desktop.placement.lock().unwrap().clone()
    }

    fn keep_placement(&self, placement: Placement) {
        self.desktop.kept.lock().unwrap().push(placement);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::FakeHost;
    use crate::host::mailbox;
    use gpui::TestAppContext;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    /// The window as `run` opens it, over a fake host with a tray or none.
    fn open(cx: &mut TestAppContext, tray: bool) -> (Arc<FakeHost>, crate::host::Mailbox, Entity<Keeper>, &mut gpui::VisualTestContext) {
        let host = FakeHost::new();
        host.desktop.tray.store(tray, Ordering::SeqCst);
        let (mailbox, inbox) = mailbox();
        let models = cx.update(|cx| crate::start(host.clone(), inbox, crate::ThemePref::Light, cx));
        let dyn_host: Arc<dyn Host> = host.clone();
        let (keeper, cx) = cx.add_window_view(move |window, cx| {
            drop(models);
            Keeper2(Keeper::watch(dyn_host.clone(), window, cx))
        });
        let keeper = keeper.read_with(cx, |k, _| k.0.clone());
        cx.run_until_parked();
        (host, mailbox, keeper, cx)
    }

    /// A view holding the keeper, as the window's close handler does.
    struct Keeper2(Entity<Keeper>);

    impl gpui::Render for Keeper2 {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    #[gpui::test]
    fn the_desktops_events_and_requests_reach_the_window(cx: &mut TestAppContext) {
        let (host, mailbox, _keeper, cx) = open(cx, true);
        // What the board will send when a chat waits, through the host.
        let notice =
            Notice {
                title: "Claude is waiting".into(), body: "“chat” needs your input".into(), chat_id: Some("c1".into()), sound: true
            };
        host.notify(notice.clone()).unwrap();
        assert_eq!(*host.desktop.notices.lock().unwrap(), vec![notice]);
        // Its click, as the host answers it: the window comes up, and the chat is asked for.
        cx.update(|_, cx| cx.default_global::<Shown>().hidden = true);
        assert!(mailbox.request(Request::Show));
        assert!(mailbox.event(event::OPEN_CHAT, json!({ "chatId": "c1" })));
        assert!(mailbox.event(event::UPDATE_PROGRESS, json!({ "got": 10, "total": 40 })));
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(!cx.global::<Shown>().hidden);
            // Asked for, and taken at once by the workspace (workspace.rs
            // `open_chat`), which opens it when it is listed.
            assert_eq!(cx.global::<OpenChat>().0, None);
            assert_eq!(cx.global::<UpdateProgress>().0, Some((10, Some(40))));
        });
    }

    #[gpui::test]
    fn closing_hides_to_the_tray_and_keeps_the_place_or_quits_without_one(cx: &mut TestAppContext) {
        let (host, _mailbox, keeper, cx) = open(cx, true);
        let dyn_host: Arc<dyn Host> = host.clone();
        let closes = cx.update(|window, cx| on_close(&dyn_host, Some(&keeper), window, cx));
        assert!(!closes, "hidden, not closed");
        assert_eq!(host.desktop.kept.lock().unwrap().len(), 1, "kept as it hid");
        cx.update(|_, cx| assert!(cx.global::<Shown>().hidden));
        // Without a tray the app quits, the window open until it does.
        host.desktop.tray.store(false, Ordering::SeqCst);
        assert!(!cx.update(|window, cx| on_close(&dyn_host, Some(&keeper), window, cx)));
    }

    #[gpui::test]
    fn the_place_is_kept_once_after_a_burst_of_changes(cx: &mut TestAppContext) {
        let (host, _mailbox, _keeper, cx) = open(cx, true);
        for w in [1000., 1010., 1020.] {
            cx.simulate_resize(size(px(w), px(700.)));
        }
        cx.run_until_parked();
        assert!(host.desktop.kept.lock().unwrap().is_empty(), "not before the burst is over");
        cx.executor().advance_clock(KEEP_AFTER);
        cx.run_until_parked();
        let kept = host.desktop.kept.lock().unwrap().clone();
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!((kept[0].bounds.width, kept[0].maximized), (1020., false));
        // A change not kept yet is kept as the app quits.
        cx.simulate_resize(size(px(1100.), px(700.)));
        cx.run_until_parked();
        cx.update(|_, cx| flush_all(cx));
        assert_eq!(host.desktop.kept.lock().unwrap().last().map(|p| p.bounds.width), Some(1100.));
    }

    #[gpui::test]
    fn the_window_opens_where_the_host_says(cx: &mut TestAppContext) {
        let host = FakeHost::new();
        let at = Rect { x: 40., y: 30., width: 1000., height: 700. };
        *host.desktop.placement.lock().unwrap() = Some(Placement { bounds: at, maximized: true, on: None });
        let (opened, on) = cx.update(|cx| initial(&*host, size(px(1280.), px(800.)), size(px(900.), px(600.)), cx));
        assert_eq!(opened, WindowBounds::Maximized(bounds(at)));
        let primary = cx.update(|cx| cx.primary_display().map(|d| d.id()));
        assert!(primary.is_some() && on == primary, "on the test platform's one display");
        *host.desktop.placement.lock().unwrap() = None;
        let (fresh, on) = cx.update(|cx| initial(&*host, size(px(1280.), px(800.)), size(px(900.), px(600.)), cx));
        assert!(matches!(fresh, WindowBounds::Windowed(b) if b.size == size(px(1280.), px(800.))));
        assert_eq!(on, primary);
    }

    /// A laptop's screen, a larger one to its right and one above it at a
    /// negative origin: the window goes on the one its center is on.
    #[test]
    fn the_window_opens_on_the_display_that_holds_its_center() {
        let screens = [
            Rect { x: 0., y: 0., width: 1920., height: 1040. },
            Rect { x: 1920., y: 0., width: 2560., height: 1400. },
            Rect { x: 0., y: -1080., width: 1920., height: 1040. },
        ];
        let at = |x: f64, y: f64| Rect { x, y, width: 1280., height: 800. };
        assert_eq!(holding(&screens, at(100., 100.)), Some(0));
        assert_eq!(holding(&screens, at(2100., 300.)), Some(1));
        assert_eq!(holding(&screens, at(300., -1000.)), Some(2));
        // Its center a pixel past the laptop's right edge.
        assert_eq!(holding(&screens, at(1921. - 640., 100.)), Some(1));
        // Above the screen to the right: on none.
        assert_eq!(holding(&screens, at(2500., -900.)), None);
        assert_eq!(holding(&[], at(0., 0.)), None);
    }

    /// The display a window was kept on is the one it opens on wherever the
    /// displays are now, ahead of the one its center happens to be on; one
    /// gone, or kept without an identity, leaves the center to say.
    #[test]
    fn the_window_opens_on_the_display_it_was_kept_on() {
        let area = |x: f64| Rect { x, y: 0., width: 1920., height: 1040. };
        let screens = [
            Screen { id: Some("laptop".into()), area: area(0.) },
            Screen { id: Some("monitor".into()), area: area(1920.) },
            Screen { id: None, area: area(3840.) },
        ];
        let at = Rect { x: 100., y: 100., width: 1280., height: 800. };
        let kept = on_display(Some("monitor".into()), area(1920.), Rect { x: 2020., ..at });
        assert_eq!(kept, Some(OnDisplay { id: "monitor".into(), dx: 100., dy: 100. }));
        assert_eq!(on_display(None, area(0.), at), None);
        // Its center on the laptop, but kept on the monitor.
        let p = Placement { bounds: at, maximized: false, on: kept };
        assert_eq!(display_for(&screens, &p), Some(1));
        // The monitor gone: the center says.
        assert_eq!(display_for(&screens[..1], &p), Some(0));
        let p = Placement { on: None, ..p };
        assert_eq!(display_for(&screens, &p), Some(0));
        let p = Placement { bounds: Rect { x: 4000., ..at }, ..p };
        assert_eq!(display_for(&screens, &p), Some(2));
    }

    /// A window minimized keeps the state it comes back in: maximized, or
    /// not, whatever gpui calls a minimized window.
    #[test]
    fn a_minimized_window_keeps_what_it_comes_back_as() {
        let at = Rect { x: 10., y: 20., width: 1000., height: 700. };
        let b = bounds(at);
        assert!(placement(WindowBounds::Maximized(b), None).maximized);
        assert!(!placement(WindowBounds::Windowed(b), None).maximized);
        // Minimized from maximized: gpui says windowed, Windows that it comes back maximized.
        assert!(placement(WindowBounds::Windowed(b), Some(true)).maximized);
        assert!(!placement(WindowBounds::Windowed(b), Some(false)).maximized);
        assert_eq!(placement(WindowBounds::Windowed(b), Some(true)).bounds, at);
        assert!(!placement(WindowBounds::Fullscreen(b), None).maximized);
    }

    #[test]
    fn the_desktops_names_are_the_ones_luna_uses() {
        assert_eq!(cmd::CALLED, ["update_check", "update_download", "update_install"]);
        assert_eq!(cmd::SENT, ["tray_awake"]);
        for e in event::ALL {
            assert!(crate::event::wants(e), "{e} is not taken");
        }
        let check: UpdateCheck =
            serde_json::from_value(json!({ "current": "0.20.0", "offer": { "version": "0.21.0", "coreFingerprint": "ab" } })).unwrap();
        assert_eq!(check.offer.unwrap().core_fingerprint.as_deref(), Some("ab"));
    }
}
