//! The screenshot harness: every demo scene (demo.rs) opened in turn, off
//! screen and never activated, over a fake host, and its frame rendered to a
//! PNG with `Window::render_to_image`, which gpui keeps behind its test
//! support. That is why this is a feature and an exe of its own
//! (`luna-window-shots`): the app's exe never carries it. Driven by
//! scripts/gpui-shots.ps1; later slices add their scenes to demo.rs.

use crate::demo::{self, FakeHost, Scene, TermDemo};
use crate::models::Models;
use crate::term::TermPane;
use crate::{font, offscreen, root, theme};
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use gpui::{AnyWindowHandle, App, AppContext, AsyncApp, Entity, EntityInputHandler, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// How long a scene gets to lay out and draw its first frames.
const SETTLE: Duration = Duration::from_millis(1500);

/// The terminal's burst bench (term/bench): `secs` of bursts into a pane in
/// an off-screen window, its summary written to `out` as JSON; exits 1 if it
/// could not run.
pub fn bench(out: PathBuf, secs: f64) {
    gpui_platform::application().run(move |cx: &mut App| {
        if let Err(e) = font::register(cx) {
            eprintln!("bench: the font did not load: {e}");
        }
        gpui_component::init(cx);
        crate::term::bind_keys(cx);
        crate::keys::bind(cx);
        let scene = demo::scenes().into_iter().find(|s| s.name == "term-selection").expect("the terminal scene");
        cx.spawn(async move |cx| {
            let summary = match open(&scene, cx) {
                Ok(opened) => {
                    cx.background_executor().timer(Duration::from_millis(500)).await;
                    match terminal(&opened, demo::DEMO_CHAT, cx) {
                        Some(pane) => crate::term::bench::run(pane, opened.host, secs, cx).await,
                        None => Err("the scene has no terminal".into()),
                    }
                }
                Err(e) => Err(e),
            };
            let written = summary.and_then(|s| {
                let text = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
                std::fs::write(&out, text + "\n").map_err(|e| format!("{}: {e}", out.display()))
            });
            match written {
                Ok(()) => {
                    println!("bench: {}", out.display());
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("bench: {e}");
                    std::process::exit(1);
                }
            }
        })
        .detach();
    });
}

/// How long each of a scene's driver lines gets before the next: the fake
/// host's answers come on the next turns of the window's thread.
const STEP: Duration = Duration::from_millis(200);

/// Shoots every scene whose name contains `only` (all when empty) into
/// `out` as `<scene>-<theme>.png`; exits 1 if any shot failed or came out
/// blank.
pub fn run(out: PathBuf, only: String) {
    gpui_platform::application().run(move |cx: &mut App| {
        if let Err(e) = font::register(cx) {
            eprintln!("shots: the font did not load: {e}");
        }
        gpui_component::init(cx);
        crate::term::bind_keys(cx);
        crate::keys::bind(cx);
        crate::panels::bind_keys(cx);
        crate::dialogs::bind_keys(cx);
        let scenes: Vec<Scene> = demo::scenes().into_iter().filter(|s| only.is_empty() || s.name.contains(only.as_str())).collect();
        cx.spawn(async move |cx| {
            let (mut failed, mut previous) = (0, None);
            for scene in &scenes {
                let file = out.join(format!("{}-{}.png", scene.name, scene.theme.name()));
                let shot = match open(scene, cx) {
                    Ok(opened) => {
                        let window = opened.window;
                        // The scene before closes only now: closing the last
                        // window would end the app's loop under this task.
                        if let Some(done) = previous.replace(window) {
                            let _ = done.update(cx, |_, window, _| window.remove_window());
                        }
                        show(scene, &opened, cx).await;
                        drive(scene, &opened, cx).await;
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

/// A scene's open window, and what a scene needs of it.
struct Opened {
    window: AnyWindowHandle,
    view: Entity<root::LunaWindow>,
    host: Arc<FakeHost>,
    models: Models,
}

/// The terminal of chat `id`, once the board has made it.
fn terminal(opened: &Opened, id: &str, cx: &mut AsyncApp) -> Option<Entity<TermPane>> {
    cx.update(|cx| opened.view.read(cx).board.read(cx).terminal_of(id))
}

/// The scene's window, off screen and never activated, over its fake host.
fn open(scene: &Scene, cx: &mut AsyncApp) -> Result<Opened, String> {
    #[cfg(windows)]
    gpui_windows::set_unattended_windows(true);
    let (host, _mailbox, inbox) = scene.host();
    let fake = host.clone();
    cx.update(|cx| {
        let models = Models::new(host, scene.theme, false, cx);
        theme::apply_to_components(&models.shell.read(cx).palette(), cx);
        crate::pump(models.clone(), inbox, cx);
        models.ask_core_state(cx);
        models.workspace.update(cx, |w, cx| w.load(cx));
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
        let (window_models, mut root_view) = (models.clone(), None);
        let window = cx
            .open_window(options, |window, cx| {
                let view = cx.new(|cx| root::LunaWindow::new(window_models, window, cx));
                root_view = Some(view.clone());
                cx.new(|cx| Root::new(view, window, cx))
            })
            .map_err(|e| format!("{e:#}"))?;
        Ok(Opened { window: window.into(), view: root_view.expect("the view was made"), host: fake, models })
    })
}

/// What a scene shows once its window has laid out: a terminal scene's
/// output, selection, composition or exit; the chats' sessions some text
/// each; a pane's menu open.
async fn show(scene: &Scene, opened: &Opened, cx: &mut AsyncApp) {
    cx.background_executor().timer(Duration::from_millis(300)).await;
    let (host, models, window) = (&opened.host, &opened.models, opened.window);
    if let Some(at) = scene.menu {
        let board = cx.update(|cx| opened.view.read(cx).board.clone());
        board.update(cx, |b, cx| {
            b.menu = Some(at);
            cx.notify();
        });
    }
    if let Some(id) = scene.click {
        models.workspace.update(cx, |w, cx| w.show_chat(id, cx));
        cx.background_executor().timer(Duration::from_millis(200)).await;
    }
    let Some(demo) = scene.terminal else {
        let shown = cx.update(|cx| models.workspace.read(cx).state.shown());
        for id in shown {
            host.output(&id, demo::term_output(TermDemo::Selection).as_bytes());
        }
        return;
    };
    let Some(pane) = terminal(opened, demo::DEMO_CHAT, cx) else { return };
    let pane = &pane;
    if demo == TermDemo::Waiting {
        return;
    }
    host.output(&demo::session().id, demo::term_output(demo).as_bytes());
    pane.update(cx, |p, _| p.parser().settle());
    let _ = window.update(cx, |_, window, cx| {
        pane.update(cx, |p, cx| match demo {
            TermDemo::Selection => {
                let mut term = p.parser().term.lock();
                let mut sel = Selection::new(SelectionType::Simple, GridPoint::new(Line(2), Column(11)), Side::Left);
                sel.update(GridPoint::new(Line(4), Column(20)), Side::Right);
                term.selection = Some(sel);
            }
            TermDemo::Ime => {
                p.focus(window, cx);
                p.replace_and_mark_text_in_range(None, "にほんご", Some(4..4), window, cx); // lang-check: allow
            }
            _ => {}
        });
        if demo == TermDemo::Exited {
            let exit = serde_json::json!({ "id": demo::session().id, "code": 0, "generation": 1 });
            crate::models::on_event(models, crate::host::event::PTY_EXIT, exit, cx);
        }
        pane.update(cx, |_, cx| cx.notify());
    });
}

/// The scene's driver lines, a step apart (panels/mod.rs `drive`).
async fn drive(scene: &Scene, opened: &Opened, cx: &mut AsyncApp) {
    for line in &scene.drive {
        cx.background_executor().timer(STEP).await;
        let (verb, rest) = line.split_once(' ').unwrap_or((line, ""));
        let done = opened
            .window
            .update(cx, |_, window, cx| crate::panels::drive(verb, rest, window, cx) || crate::dialogs::drive(verb, rest, window, cx));
        if !matches!(done, Ok(true)) {
            eprintln!("shots: {}: {line} did nothing", scene.name);
        }
    }
    // The session of a terminal the driver opened prints.
    if let Some((id, text)) = &scene.output {
        cx.background_executor().timer(STEP).await;
        opened.host.output(id, text.as_bytes());
        let panes = cx.update(|cx| opened.models.panes_of(id, cx));
        for pane in panes {
            pane.update(cx, |p, _| p.parser().settle());
        }
    }
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
