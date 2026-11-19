//! Luna's native window, on GPUI: the page of the WebView window, in Rust.
//!
//! It knows the app only through its host (host.rs): the commands it calls and
//! the events it hears are the page's own, by name and shape. The host is
//! luna_lib (src/native.rs), which answers them from the same functions the
//! Tauri commands are; a fake one answers in the tests and the screenshots
//! (demo.rs). The state the host reports lives in GPUI entities (models.rs),
//! updated by one task that drains the inbox; views observe them, and frames
//! are drawn on demand, so an idle window draws nothing.

mod accounts;
mod banner;
mod board;
#[cfg(any(test, feature = "shots"))]
pub mod demo;
pub mod desktop;
pub mod dialogs;
#[cfg(luna_test)]
mod drive;
pub mod dto;
mod font;
mod host;
mod import;
mod keys;
mod library;
mod models;
mod offscreen;
pub mod panels;
mod rename;
mod root;
mod settings;
#[cfg(feature = "shots")]
mod shots;
mod sidebar;
pub mod state;
mod term;
mod theme;
mod title_bar;
mod widgets;
pub mod workspace;

#[cfg(test)]
mod board_tests;
#[cfg(test)]
mod tests;

pub use desktop::{Desktop, Request};
pub use dto::ThemePref;
pub use host::{Answer, Host, Inbox, Mailbox, OutputSink, cmd, event, mailbox};
pub use import::chat_list_size;
#[cfg(feature = "shots")]
pub use shots::{bench, run as shots};
pub use term::SessionSpec;
pub use term::paste::Provider;

use gpui::{App, AppContext, DisplayId, Entity, Size, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, point, px, size};
use gpui_component::Root;
use models::Models;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// How the window opens.
pub struct Options {
    /// Its title, for the taskbar and Alt+Tab.
    pub title: String,
    /// The theme the person chose last time.
    pub theme: ThemePref,
    /// Off every display, never activated, off the taskbar: the test build
    /// run in the background, out of the way of the person at the screen.
    pub background: bool,
    /// A file of lines the window acts on (drive.rs): the test build's
    /// `LUNA_TEST_DRIVE`, for a smoke run that may use no global input.
    #[cfg(luna_test)]
    pub drive_file: Option<std::path::PathBuf>,
    /// Called once the window is open, on its thread: what needs the window
    /// there starts here, so that a window that cannot open leaves nothing
    /// started for the WebView window to find in its way.
    pub on_open: Box<dyn FnOnce() + 'static>,
    /// Called once as the app quits after the window opened, on the window's
    /// thread, before the process goes: on macOS `run` never returns.
    pub on_quit: Box<dyn FnOnce() + 'static>,
}

/// The window's size the first time it opens.
const SIZE: Size<gpui::Pixels> = size(px(1280.), px(800.));
// The WebView window's minimum (tauri.conf.json), so the two fit the same screens.
const MIN_SIZE: Size<gpui::Pixels> = size(px(900.), px(600.));

/// Opens the window and runs it until it closes, which quits the app. Returns
/// once it has (on Windows and Linux; macOS ends the process instead), with
/// why when the window could not open at all: no usable GPU device, shaders
/// that do not compile. Neither `on_open` nor `on_quit` has run then.
pub fn run(host: Arc<dyn Host>, inbox: Inbox, options: Options) -> Result<(), String> {
    #[cfg(windows)]
    if options.background {
        gpui_windows::set_unattended_windows(true);
    }
    let failed = Rc::new(RefCell::new(None));
    let failure = failed.clone();
    gpui_platform::application().run(move |cx: &mut App| {
        let Options {
            title,
            theme,
            background,
            #[cfg(luna_test)]
            drive_file,
            on_open,
            on_quit,
        } = options;
        let opened = Rc::new(Cell::new(false));
        let (is_open, mut on_quit) = (opened.clone(), Some(on_quit));
        cx.on_app_quit(move |cx| {
            // The window's last place and state reach the host before its
            // quit waits for what it was given.
            workspace::flush(cx);
            desktop::flush_all(cx);
            if is_open.get()
                && let Some(f) = on_quit.take()
            {
                f();
            }
            async {}
        })
        .detach();
        // The window's close hides it to the tray (desktop.rs `on_close`);
        // without a tray it closes, which quits the app. The core runs on.
        cx.default_global::<desktop::Shown>().background = background;
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let models = start(host, inbox, theme, cx);
        // Ctrl+digit by the physical key, which gpui on Windows does not
        // name the same on every layout (keys.rs).
        #[cfg(windows)]
        keys::watch_digit_row(models.clone(), keys::key_is_down, keys::key_names, cx);
        let (bounds, display) = if background {
            (WindowBounds::Windowed(offscreen::bounds(SIZE, cx)), None)
        } else {
            desktop::initial(&*models.host, SIZE, MIN_SIZE, cx)
        };
        match open(&models, &title, bounds, display, background, cx) {
            Ok((_window, _view)) => {
                #[cfg(luna_test)]
                if let Some(file) = drive_file {
                    drive::start(file, _window.into(), _view, cx);
                }
                opened.set(true);
                on_open();
            }
            Err(e) => {
                *failure.borrow_mut() = Some(format!("{e:#}"));
                cx.quit();
            }
        }
    });
    failed.take().map_or(Ok(()), Err)
}

/// Everything but the window: the font, gpui-component, the models and the
/// theme kept on them, the inbox's pump, and the first look at the core.
pub(crate) fn start(host: Arc<dyn Host>, inbox: Inbox, theme: ThemePref, cx: &mut App) -> Models {
    if let Err(e) = font::register(cx) {
        log::error!("the window's font did not load, text falls back to the system's: {e}");
    }
    gpui_component::init(cx);
    term::bind_keys(cx);
    keys::bind(cx);
    panels::bind_keys(cx);
    dialogs::bind_keys(cx);
    let models = Models::new(host, theme, root::is_dark(cx.window_appearance()), cx);
    let mut dark = models.shell.read(cx).dark();
    theme::apply_to_components(&models.shell.read(cx).palette(), cx);
    cx.observe(&models.shell, move |shell, cx| {
        // Only a change of light and dark: gpui-component's own refresh of
        // every window redraws the cached views too.
        let now = shell.read(cx).dark();
        if now != dark {
            dark = now;
            theme::apply_to_components(&shell.read(cx).palette(), cx);
        }
    })
    .detach();
    pump(models.clone(), inbox, cx);
    models.ask_core_state(cx);
    models.workspace.update(cx, |w, cx| w.load(cx));
    models
}

/// The most messages the window's thread takes from the inbox in one turn.
const BATCH: usize = 64;

/// The one task that drains the inbox, on the window's thread, until the
/// host's mailboxes are all gone: what is there in batches of `BATCH` at most,
/// each in one update, and after a full one it lets the rest of the thread's
/// work (input, frames) run before it takes the next.
fn pump(models: Models, mut inbox: Inbox, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(batch) = inbox.next_batch(BATCH).await {
            let full = batch.len() >= BATCH;
            cx.update(|cx| {
                for message in batch {
                    models::deliver(&models, message, cx);
                }
            });
            if full {
                YieldNow(false).await;
            }
        }
    })
    .detach();
}

/// Pending once, woken at once: the task goes to the back of the thread's
/// queue, behind what came in meanwhile.
pub(crate) struct YieldNow(pub(crate) bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

fn open(
    models: &Models,
    title: &str,
    bounds: WindowBounds,
    display: Option<DisplayId>,
    background: bool,
    cx: &mut App,
) -> gpui::Result<(WindowHandle<Root>, Entity<root::LunaWindow>)> {
    let options = WindowOptions {
        window_bounds: Some(bounds),
        // The display whose pixels `bounds` are in (desktop.rs `initial`).
        display_id: display,
        // The window draws its own title bar (title_bar.rs).
        titlebar: Some(TitlebarOptions {
            title: Some(title.to_owned().into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(9.), px(7.))),
        }),
        app_owns_titlebar_drag: true,
        focus: !background,
        show: true,
        // gpui caps a window that is not active at 30 frames a second; one in
        // the background never is, and frames come on demand anyway.
        inactive_frame_interval: None,
        window_min_size: Some(MIN_SIZE),
        ..Default::default()
    };
    let models = models.clone();
    let mut root = None;
    let window = cx.open_window(options, |window: &mut Window, cx| {
        // Its place is kept as it changes, except in the background; the
        // close handler holds the keeper for as long as the window lives.
        let keeper = (!background).then(|| desktop::Keeper::watch(models.host.clone(), window, cx));
        let host = models.host.clone();
        window.on_window_should_close(cx, move |window, cx| desktop::on_close(&host, keeper.as_ref(), window, cx));
        let view = cx.new(|cx| root::LunaWindow::new(models, window, cx));
        root = Some(view.clone());
        cx.new(|cx| Root::new(view, window, cx))
    })?;
    Ok((window, root.expect("the window's view was made")))
}
