//! The screenshot harness: every demo scene (demo.rs) opened in turn, off
//! screen and never activated, over a fake host, and its frame rendered to a
//! PNG with `Window::render_to_image`, which gpui keeps behind its test
//! support. That is why this is a feature and an exe of its own
//! (`luna-window-shots`): the app's exe never carries it. Driven by
//! scripts/gpui-shots.ps1; later slices add their scenes to demo.rs.

use crate::demo::{self, Scene};
use crate::models::Models;
use crate::{font, offscreen, root, theme};
use gpui::{AnyWindowHandle, App, AppContext, AsyncApp, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long a scene gets to lay out and draw its first frames.
const SETTLE: Duration = Duration::from_millis(1500);

/// Shoots every scene whose name contains `only` (all when empty) into
/// `out` as `<scene>-<theme>.png`; exits 1 if any shot failed or came out
/// blank.
pub fn run(out: PathBuf, only: String) {
    gpui_platform::application().run(move |cx: &mut App| {
        if let Err(e) = font::register(cx) {
            eprintln!("shots: the font did not load: {e}");
        }
        gpui_component::init(cx);
        let scenes: Vec<Scene> = demo::scenes().into_iter().filter(|s| only.is_empty() || s.name.contains(only.as_str())).collect();
        cx.spawn(async move |cx| {
            let (mut failed, mut previous) = (0, None);
            for scene in &scenes {
                let file = out.join(format!("{}-{}.png", scene.name, scene.theme.name()));
                let shot = match open(scene, cx) {
                    Ok(window) => {
                        // The scene before closes only now: closing the last
                        // window would end the app's loop under this task.
                        if let Some(done) = previous.replace(window) {
                            let _ = done.update(cx, |_, window, _| window.remove_window());
                        }
                        shoot(window, &file, cx).await
                    }
                    Err(e) => Err(e),
                };
                match shot {
                    Ok(note) => println!("shots: {} {note}", file.display()),
                    Err(e) => {
                        eprintln!("shots: {} failed: {e}", file.display());
                        failed += 1;
                    }
                }
            }
            std::process::exit(if failed == 0 { 0 } else { 1 });
        })
        .detach();
    });
}

/// The scene's window, off screen and never activated, over its fake host.
fn open(scene: &Scene, cx: &mut AsyncApp) -> Result<AnyWindowHandle, String> {
    #[cfg(windows)]
    gpui_windows::set_unattended_windows(true);
    let (host, _mailbox, inbox) = scene.host();
    cx.update(|cx| {
        let models = Models::new(host, scene.theme, false, cx);
        theme::apply_to_components(&models.shell.read(cx).palette(), cx);
        crate::pump(models.clone(), inbox, cx);
        models.ask_core_state(cx);
        scene.tell(&models, cx);
        let bounds = offscreen::bounds(size(px(1000.), px(620.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Luna shots".into()),
                appears_transparent: true,
                traffic_light_position: None,
            }),
            focus: false,
            show: true,
            inactive_frame_interval: None,
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            let view = cx.new(|cx| root::LunaWindow::new(models, window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .map(Into::into)
    })
    .map_err(|e| format!("{e:#}"))
}

/// Its frame once it has had time to draw, into `file`.
async fn shoot(window: AnyWindowHandle, file: &Path, cx: &mut AsyncApp) -> Result<String, String> {
    cx.background_executor().timer(SETTLE).await;
    let image = window.update(cx, |_, window, _| window.render_to_image()).map_err(|e| format!("{e:#}"))?.map_err(|e| format!("{e:#}"))?;
    let (w, h) = image.dimensions();
    let colors = image.pixels().map(|p| p.0).collect::<std::collections::HashSet<_>>().len();
    // A frame with nothing drawn is one color; the title bar alone has five.
    if colors < 4 {
        return Err(format!("{w}x{h} with {colors} color(s): nothing was drawn"));
    }
    image.save(file).map_err(|e| e.to_string())?;
    Ok(format!("{w}x{h}, {colors} colors"))
}
