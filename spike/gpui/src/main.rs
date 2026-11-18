// The spike's window: some text and a gpui-component Button on GPUI, to prove
// the stack links, starts and renders on Windows from a Linux cross-build.
//
//   spike.exe [--background] [--hidden] [--exit-after SECS] [--size WxH]
//             [--editor PATH] [--stats PATH] [--shot PATH] (--features shot)
//             [--term | --pty | --feed burst[:SECS] | --feed-file PATH ...] (src/term/mod.rs)
//
// --background opens the window off every display, never activated and off
// the taskbar (the patched gpui-pre-windows does this for a window opened
// without focus; see README.md), so it can run beside a desktop in use.
// --hidden opens it not shown at all. --shot renders the window's frame to a
// PNG in-app (Window::render_to_image, no PrintWindow) and quits; with the
// editor open, also its window to <PATH stem>-editor.png. --editor opens the
// Markdown file PATH in an editor window (src/editor.rs) beside the main one.
// --stats writes a JSON line of stats to PATH when --exit-after quits.
// --term and the other terminal flags open the terminal pane's window
// (src/term/) instead of the main one; with a feed it writes its summary to
// --term-stats, or to --stats when no editor window is open.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod editor;
mod idle;
mod term;
mod win;

use std::{path::PathBuf, time::Duration};

use gpui::{
    App, Bounds, Context, SharedString, Window, WindowBounds, WindowOptions, div, point,
    prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Root,
    button::{Button, ButtonVariants},
};

struct Args {
    background: bool,
    hidden: bool,
    shot: Option<PathBuf>,
    editor: Option<PathBuf>,
    stats: Option<PathBuf>,
    exit_after: Option<f64>,
    size: (f32, f32),
    idle: idle::IdleArgs,
    term: term::TermArgs,
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Args {
    let mut args = Args {
        background: false,
        hidden: false,
        shot: None,
        editor: None,
        stats: None,
        exit_after: None,
        size: (800.0, 500.0),
        idle: idle::IdleArgs::default(),
        term: term::TermArgs::default(),
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--background" => args.background = true,
            "--hidden" => args.hidden = true,
            "--shot" => args.shot = it.next().map(Into::into),
            "--editor" => args.editor = it.next().map(Into::into),
            "--stats" => args.stats = it.next().map(Into::into),
            "--exit-after" => args.exit_after = it.next().and_then(|s| s.parse().ok()),
            "--size" => {
                if let Some((w, h)) = it.next().as_deref().and_then(|s| s.split_once('x')) {
                    if let (Ok(w), Ok(h)) = (w.parse(), h.parse()) {
                        args.size = (w, h);
                    }
                }
            }
            other if args.term.parse(other, &mut it) => {}
            other if args.idle.parse(other, &mut it) => {}
            other => eprintln!("spike: ignoring unknown argument {other}"),
        }
    }
    args
}

struct Spike {
    clicks: usize,
}

impl Render for Spike {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        idle::count(&idle::MAIN_RENDERS);
        let status: SharedString = format!("Button clicks: {}", self.clicks).into();
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap_4()
            .p_6()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(div().text_2xl().child("GPUI spike"))
            .child(div().child("gpui-pre 0.3.12 + gpui-component 0.10.0, release, cross-built"))
            .child(div().text_color(theme.muted_foreground).child(status))
            .child(
                div().flex().gap_2().child(
                    Button::new("hello")
                        .primary()
                        .label("A gpui-component Button")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.clicks += 1;
                            cx.notify();
                        })),
                ),
            )
    }
}

fn main() {
    let args = parse_args(std::env::args().skip(1));
    gpui_platform::application().run(move |cx: &mut App| {
        // --bare leaves gpui-component out altogether, init included.
        if !args.idle.bare {
            gpui_component::init(cx);
            editor::init(cx);
        }

        let window_size = size(px(args.size.0), px(args.size.1));
        let bounds = if args.background {
            // Past the right edge of every display, so no display shows any of it.
            win::offscreen_bounds(window_size, cx)
        } else {
            Bounds::centered(None, window_size, cx)
        };
        let window: gpui::AnyWindowHandle = if args.term.enabled {
            let stats = args.term.stats_path(args.stats.as_ref(), args.editor.is_some());
            term::open_window(&args.term, stats, bounds, !args.background, !args.hidden, cx)
                .expect("failed to open the terminal window")
                .into()
        } else if args.idle.bare {
            idle::open_bare(bounds, !args.background, !args.hidden, args.idle.tick, cx)
        } else { cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("gpui spike".into()),
                    ..Default::default()
                }),
                focus: !args.background,
                show: !args.hidden,
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|_| Spike { clicks: 0 });
                cx.new(|cx| Root::new(view, window, cx))
            },
        )
        .expect("failed to open the window")
        .into() };

        let editor = args.editor.clone().map(|path| {
            // Beside the main window: off screen right of it, or offset on screen.
            let origin = if args.background {
                point(bounds.right() + px(100.), bounds.top())
            } else {
                bounds.origin + point(px(40.), px(40.))
            };
            let editor_bounds = Bounds::new(origin, size(px(900.), px(700.)));
            editor::open(path, editor_bounds, !args.background, !args.hidden, cx)
                .expect("failed to open the editor window")
        });

        if let Some(path) = args.shot.clone() {
            let mut shots = vec![(window, path.clone())];
            if let Some((editor_window, _)) = &editor {
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                shots.push(((*editor_window).into(), path.with_file_name(format!("{stem}-editor.png"))));
            }
            shot(shots, cx);
        }

        idle::start(&args.idle, cx);

        // The editor's idle count starts once it has settled (first frames,
        // the first highlight pass), and runs until --exit-after quits.
        let settle = Duration::from_secs(2);
        if let Some((_, view)) = &editor {
            let view = view.clone();
            cx.spawn(async move |cx| {
                cx.background_executor().timer(settle).await;
                cx.update(|cx| editor::count_from_now(&view, cx));
            })
            .detach();
        }

        if let Some(secs) = args.exit_after {
            let stats = args.stats.clone();
            let view = editor.as_ref().map(|(_, view)| view.clone());
            let term_on = args.term.enabled;
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(Duration::from_secs_f64(secs))
                    .await;
                cx.update(|cx| {
                    if let Some(path) = &stats {
                        let idle = (secs - settle.as_secs_f64()).max(0.);
                        let line = match &view {
                            Some(view) => Some(editor::stats_json(view, idle, cx)),
                            // The pane's feed summary (also written when the feed ended).
                            None if term_on => term::summary(cx),
                            None => Some("{}".to_string()),
                        };
                        if let Some(line) = line
                            && let Err(e) = std::fs::write(path, line + "
") {
                            eprintln!("spike: writing {}: {e}", path.display());
                        }
                    }
                    cx.quit()
                });
            })
            .detach();
        }
    });
}

// Renders each window's last frame to its path once it has had time to draw,
// prints the outcome and quits (exit code 1 if any failed).
#[cfg(feature = "shot")]
fn shot(shots: Vec<(gpui::AnyWindowHandle, PathBuf)>, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(2500))
            .await;
        let mut code = 0;
        for (window, path) in shots {
            let result = window
                .update(cx, |_, window, _| window.render_to_image())
                .and_then(|image| image)
                .and_then(|image| {
                    let (w, h) = image.dimensions();
                    image.save(&path)?;
                    Ok((w, h))
                });
            match result {
                Ok((w, h)) => println!("spike: rendered {w}x{h} to {}", path.display()),
                Err(e) => {
                    eprintln!("spike: render_to_image of {} failed: {e:#}", path.display());
                    code = 1;
                }
            }
        }
        cx.update(|cx| cx.quit());
        std::process::exit(code);
    })
    .detach();
}

#[cfg(not(feature = "shot"))]
fn shot(_: Vec<(gpui::AnyWindowHandle, PathBuf)>, _: &mut App) {
    eprintln!("spike: --shot needs a build with --features shot");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::parse_args;

    fn parse(args: &[&str]) -> super::Args {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn reads_the_flags() {
        let args = parse(&["--background", "--size", "640x480", "--exit-after", "2.5"]);
        assert!(args.background && !args.hidden);
        assert_eq!(args.size, (640.0, 480.0));
        assert_eq!(args.exit_after, Some(2.5));
    }

    #[test]
    fn keeps_the_default_size_on_a_bad_one() {
        assert_eq!(parse(&["--size", "wide"]).size, (800.0, 500.0));
    }
}
