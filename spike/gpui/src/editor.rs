// The editor window: gpui-component's code editor (`EditorState` + `Editor`)
// on a Markdown file, highlighted with tree-sitter-markdown, with a steady
// (never blinking) caret so an idle window schedules no frames.

use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

use gpui::{
    App, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _,
    Pixels, Render, SharedString, Styled as _, Subscription, Window, WindowBounds, WindowHandle,
    WindowOptions, div,
};
use gpui_component::{
    ActiveTheme as _, Root, Theme,
    input::{CursorBlink, Editor, EditorState},
};

/// The window title, which shot.ps1 picks the editor's HWND by.
pub const TITLE: &str = "gpui spike editor";

/// Makes every caret in the app steady: no blink timer, so a focused editor
/// left alone causes no repaints. Call once after `gpui_component::init`.
pub fn init(cx: &mut App) {
    Theme::update(cx, |theme| theme.cursor_blink = CursorBlink::steady());
}

pub struct EditorView {
    pub state: Entity<EditorState>,
    title: SharedString,
    /// Notifications from the editor's state since the last `count_from_now`:
    /// what an idle editor would repaint for (a blink, a highlight pass).
    pub notifications: Rc<Cell<usize>>,
    opened_in_ms: Cell<u128>,
    _observe: Subscription,
}

impl EditorView {
    pub fn new(path: Option<&Path>, text: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("markdown")
                .default_value(text)
        });
        let notifications = Rc::new(Cell::new(0));
        let count = notifications.clone();
        let _observe = cx.observe(&state, move |_, _, cx| {
            count.set(count.get() + 1);
            cx.notify();
        });
        // EditorState::focus, not just its focus handle's: it also starts the
        // caret, which a steady policy then shows without any timer.
        state.update(cx, |state, cx| state.focus(window, cx));
        let title = path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(untitled)".into())
            .into();
        Self {
            state,
            title,
            notifications,
            opened_in_ms: Cell::new(0),
            _observe,
        }
    }
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::idle::count(&crate::idle::EDITOR_RENDERS);
        let theme = cx.theme();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(self.title.clone()),
            )
            .child(div().flex_1().min_h_0().child(Editor::new(&self.state).h_full()))
    }
}

/// Opens the editor window on `path` with `bounds`; `focus: false` makes it
/// the patched never-activated kind (see README.md).
pub fn open(
    path: PathBuf,
    bounds: Bounds<Pixels>,
    focus: bool,
    show: bool,
    cx: &mut App,
) -> anyhow::Result<(WindowHandle<Root>, Entity<EditorView>)> {
    let started = Instant::now();
    let text = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;
    let mut view = None;
    let window = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(TITLE.into()),
                ..Default::default()
            }),
            focus,
            show,
            ..Default::default()
        },
        |window, cx| {
            let editor = cx.new(|cx| EditorView::new(Some(&path), text, window, cx));
            view = Some(editor.clone());
            cx.new(|cx| Root::new(editor, window, cx))
        },
    )?;
    let view = view.expect("open_window builds the view");
    view.read(cx).opened_in_ms.set(started.elapsed().as_millis());
    Ok((window, view))
}

/// The editor's stats as one JSON object, for `--stats`: the file's size, how
/// long reading it and opening the window took, and how often the editor's
/// state notified (each a repaint) since `count_from_now`.
pub fn stats_json(view: &Entity<EditorView>, idle_seconds: f64, cx: &App) -> String {
    let view = view.read(cx);
    format!(
        "{{\"editor_bytes\":{},\"editor_open_ms\":{},\"editor_idle_seconds\":{:.1},\"editor_idle_notifications\":{}}}",
        view.state.read(cx).value().len(),
        view.opened_in_ms.get(),
        idle_seconds,
        view.notifications.get(),
    )
}

/// Restarts the count of the editor's notifications.
pub fn count_from_now(view: &Entity<EditorView>, cx: &App) {
    view.read(cx).notifications.set(0);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui::{ClipboardItem, Focusable as _, TestAppContext, VisualTestContext};
    use gpui_component::input::{Paste, Redo, Undo};

    use super::*;

    fn setup<'a>(
        cx: &'a mut TestAppContext,
        text: &str,
        blink: Option<CursorBlink>,
    ) -> (Entity<EditorView>, &'a mut VisualTestContext) {
        cx.update(|cx| {
            gpui_component::init(cx);
            init(cx);
            if let Some(blink) = blink {
                Theme::update(cx, |theme| theme.cursor_blink = blink);
            }
        });
        let mut view = None;
        let text = text.to_string();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let editor = cx.new(|cx| EditorView::new(None, text, window, cx));
            view = Some(editor.clone());
            Root::new(editor, window, cx)
        });
        let view = view.unwrap();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        (view, cx)
    }

    fn value(view: &Entity<EditorView>, cx: &mut VisualTestContext) -> String {
        cx.update(|_, cx| view.read(cx).state.read(cx).value().to_string())
    }

    fn dispatch(cx: &mut VisualTestContext, action: impl gpui::Action) {
        cx.update(|window, cx| {
            window.dispatch_action(action.boxed_clone(), cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }

    /// Notifications from the editor's state over `idle` of (fake) time.
    fn idle_notifications(view: &Entity<EditorView>, cx: &mut VisualTestContext, idle: Duration) -> usize {
        cx.run_until_parked();
        let count = cx.update(|_, cx| view.read(cx).notifications.clone());
        count.set(0);
        cx.executor().advance_clock(idle);
        cx.run_until_parked();
        count.get()
    }

    #[gpui::test]
    fn editor_is_focused_and_holds_the_file(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, "# Title\n\nBody.\n", None);
        assert_eq!(value(&view, cx), "# Title\n\nBody.\n");
        let focused = cx.update(|window, cx| {
            view.read(cx).state.read(cx).focus_handle(cx).is_focused(window)
        });
        assert!(focused);
        let language = cx.update(|_, cx| view.read(cx).state.read(cx).language_name());
        assert_eq!(language.as_ref(), "markdown");
    }

    #[gpui::test]
    fn undo_and_redo(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, "# Title\n", None);
        cx.update(|window, cx| {
            view.read(cx).state.clone().update(cx, |state, cx| {
                state.insert("first ", window, cx);
            })
        });
        cx.run_until_parked();
        assert_eq!(value(&view, cx), "first # Title\n");
        dispatch(cx, Undo);
        assert_eq!(value(&view, cx), "# Title\n");
        dispatch(cx, Redo);
        assert_eq!(value(&view, cx), "first # Title\n");
        dispatch(cx, Undo);
        dispatch(cx, Undo); // nothing older: stays as loaded
        assert_eq!(value(&view, cx), "# Title\n");
    }

    #[gpui::test]
    fn pastes_a_megabyte_and_undoes_it(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, "start\n", None);
        let line = "- a list item with **bold**, `code` and a [link](https://example.com)\n";
        let big: String = line.repeat((1 << 20) / line.len() + 1);
        assert!(big.len() >= 1 << 20);
        cx.update(|_, cx| cx.write_to_clipboard(ClipboardItem::new_string(big.clone())));
        let started = Instant::now();
        dispatch(cx, Paste);
        let elapsed = started.elapsed();
        let after = value(&view, cx);
        assert_eq!(after.len(), "start\n".len() + big.len());
        assert!(after.starts_with(&big));
        // Printed for the report; the bound only catches something pathological.
        println!("pasted {} bytes and drew the window in {elapsed:?}", big.len());
        assert!(elapsed < Duration::from_secs(20), "paste took {elapsed:?}");
        dispatch(cx, Undo);
        assert_eq!(value(&view, cx), "start\n");
    }

    #[gpui::test]
    fn a_steady_caret_schedules_nothing_when_idle(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, "# Idle\n", None);
        assert_eq!(idle_notifications(&view, cx, Duration::from_secs(5)), 0);
    }

    // The control for the test above: with gpui-component's default blinking
    // caret, the same idle editor does notify, so a zero there means steady.
    #[gpui::test]
    fn a_blinking_caret_would_notify_when_idle(cx: &mut TestAppContext) {
        let (view, cx) = setup(cx, "# Idle\n", Some(CursorBlink::default()));
        assert!(idle_notifications(&view, cx, Duration::from_secs(5)) > 0);
    }
}
