// Idle diagnostics: render counters for the spike's root views, a bare GPUI
// window without gpui-component (`--bare`), and `--frames PATH`, which writes
// the counters to PATH once a second, so measure-idle.ps1 can tell repaints
// from mere wakeups.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, App, AppContext as _, Bounds, Context, IntoElement, ParentElement as _,
    Pixels, Render, Styled as _, Window, WindowBounds, WindowOptions, div, rgb,
};

/// Renders of the main window's root view (the gpui-component one or the bare one).
pub static MAIN_RENDERS: AtomicUsize = AtomicUsize::new(0);
/// Renders of the editor window's view.
pub static EDITOR_RENDERS: AtomicUsize = AtomicUsize::new(0);

pub fn count(counter: &AtomicUsize) {
    counter.fetch_add(1, Ordering::Relaxed);
}

#[derive(Default)]
pub struct IdleArgs {
    /// A plain GPUI window, and gpui-component never initialized.
    pub bare: bool,
    pub frames: Option<PathBuf>,
    /// The bare window changes every this many seconds: a repaint on demand.
    pub tick: Option<f64>,
}

impl IdleArgs {
    /// Takes `arg` (and its value from `it`) if it is one of these flags.
    pub fn parse(&mut self, arg: &str, it: &mut impl Iterator<Item = String>) -> bool {
        match arg {
            "--bare" => self.bare = true,
            "--frames" => self.frames = it.next().map(Into::into),
            "--tick" => self.tick = it.next().and_then(|s| s.parse().ok()),
            _ => return false,
        }
        true
    }
}

struct Bare {
    ticks: usize,
}

impl Render for Bare {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        count(&MAIN_RENDERS);
        div()
            .size_full()
            .p_6()
            .bg(rgb(0x202020))
            .text_color(rgb(0xffffff))
            .child(format!("A bare GPUI window: no gpui-component (tick {})", self.ticks))
    }
}

pub fn open_bare(
    bounds: Bounds<Pixels>,
    focus: bool,
    show: bool,
    tick: Option<f64>,
    cx: &mut App,
) -> AnyWindowHandle {
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("gpui spike".into()),
                ..Default::default()
            }),
            focus,
            show,
            ..Default::default()
        },
        |_, cx| {
            cx.new(|cx: &mut Context<Bare>| {
                if let Some(secs) = tick {
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_secs_f64(secs))
                                .await;
                            let alive = this.update(cx, |bare: &mut Bare, cx| {
                                bare.ticks += 1;
                                cx.notify();
                            });
                            if alive.is_err() {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                Bare { ticks: 0 }
            })
        },
    )
    .expect("failed to open the bare window")
    .into()
}

/// With `--frames PATH`, rewrites PATH once a second with the counters so far.
pub fn start(args: &IdleArgs, cx: &mut App) {
    let Some(path) = args.frames.clone() else {
        return;
    };
    let started = Instant::now();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let line = format!(
                "{{\"t\":{:.3},\"main_renders\":{},\"editor_renders\":{}}}\n",
                started.elapsed().as_secs_f64(),
                MAIN_RENDERS.load(Ordering::Relaxed),
                EDITOR_RENDERS.load(Ordering::Relaxed),
            );
            let _ = std::fs::write(&path, line);
        }
    })
    .detach();
}
