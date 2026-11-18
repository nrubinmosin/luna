//! Luna's native window, on GPUI: the page of the WebView window, in Rust.
//!
//! It knows the app only through its host (host.rs): the commands it calls and
//! the events it hears are the page's own, by name and shape. The host is
//! luna_lib (src/native.rs), which answers them from the same functions the
//! Tauri commands are; a fake one answers in the tests and the screenshots
//! (demo.rs). The state the host reports lives in GPUI entities (models.rs),
//! updated by one task that drains the inbox; views observe them, and frames
//! are drawn on demand, so an idle window draws nothing.

mod banner;
mod board;
#[cfg(any(test, feature = "shots"))]
pub mod demo;
pub mod dto;
mod font;
mod host;
mod models;
mod offscreen;
mod root;
#[cfg(feature = "shots")]
mod shots;
mod theme;
mod title_bar;
mod widgets;

#[cfg(test)]
mod tests;

pub use dto::ThemePref;
pub use host::{Answer, Host, Inbox, Mailbox, cmd, event, mailbox};
#[cfg(feature = "shots")]
pub use shots::run as shots;

use futures::StreamExt;
use gpui::{App, AppContext, Bounds, Size, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, point, px, size};
use gpui_component::Root;
use models::Models;
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
    /// Called once as the app quits, on the window's thread, before the
    /// process goes: on macOS `run` never returns.
    pub on_quit: Box<dyn FnOnce() + 'static>,
}

/// The window's size the first time it opens.
const SIZE: Size<gpui::Pixels> = size(px(1280.), px(800.));
// The WebView window's minimum (tauri.conf.json), so the two fit the same screens.
const MIN_SIZE: Size<gpui::Pixels> = size(px(900.), px(600.));

/// Opens the window and runs it until it closes, which quits the app. Returns
/// once it has (on Windows and Linux; macOS ends the process instead).
pub fn run(host: Arc<dyn Host>, inbox: Inbox, options: Options) {
    gpui_platform::application().run(move |cx: &mut App| {
        let Options { title, theme, background, on_quit } = options;
        let mut on_quit = Some(on_quit);
        cx.on_app_quit(move |_| {
            if let Some(f) = on_quit.take() {
                f();
            }
            async {}
        })
        .detach();
        // The window's close quits the app; the core runs on.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let models = start(host, inbox, theme, cx);
        let size = SIZE;
        let bounds = if background { offscreen::bounds(size, cx) } else { Bounds::centered(None, size, cx) };
        if let Err(e) = open(&models, &title, bounds, background, cx) {
            log::error!("cannot open the window: {e:#}");
            cx.quit();
        }
    });
}

/// Everything but the window: the font, gpui-component, the models and the
/// theme kept on them, the inbox's pump, and the first look at the core.
pub(crate) fn start(host: Arc<dyn Host>, inbox: Inbox, theme: ThemePref, cx: &mut App) -> Models {
    if let Err(e) = font::register(cx) {
        log::error!("the window's font did not load, text falls back to the system's: {e}");
    }
    gpui_component::init(cx);
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
    models
}

/// The one task that drains the inbox, on the window's thread, until the
/// host's mailboxes are all gone.
fn pump(models: Models, mut inbox: Inbox, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(message) = inbox.0.next().await {
            cx.update(|cx| models::deliver(&models, message, cx));
        }
    })
    .detach();
}

fn open(models: &Models, title: &str, bounds: Bounds<gpui::Pixels>, background: bool, cx: &mut App) -> gpui::Result<WindowHandle<Root>> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
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
    cx.open_window(options, move |window: &mut Window, cx| {
        let view = cx.new(|cx| root::LunaWindow::new(models, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
}
