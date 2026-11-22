//! A chat's name being edited in place, in the sidebar or on a pane's title
//! bar: gpui-component's text input over the name, Enter or a click
//! elsewhere keeps what was typed, Esc (keys.rs) lets it go. Either way the
//! keyboard, if it was in the input, goes back where it belongs (`give_back`).

use crate::workspace::Workspace;
use gpui::{App, AppContext, Context, Entity, FocusHandle, Focusable, Subscription, Window};
use gpui_component::input::{InputEvent, InputState};

pub struct Renaming {
    pub chat: String,
    pub input: Entity<InputState>,
    /// The window's own focus (`Models::home`).
    home: FocusHandle,
    workspace: Entity<Workspace>,
    _sub: Subscription,
}

impl Renaming {
    /// Starts editing `chat`'s name in the view whose `slot` holds it.
    /// `home` is the window's own focus (`Models::home`).
    pub fn start<V: 'static>(
        chat: &str,
        name: &str,
        workspace: Entity<Workspace>,
        home: FocusHandle,
        slot: fn(&mut V) -> &mut Option<Renaming>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Renaming {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name.to_owned()));
        input.update(cx, |i, cx| i.focus(window, cx));
        let id = chat.to_owned();
        let ws = workspace.clone();
        let sub = cx.subscribe_in(&input, window, move |this, input, e: &InputEvent, window, cx| {
            if !matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let held = slot(this);
            if let Some(r) = held.take_if(|r| r.chat == id) {
                r.give_back(window, cx);
                let value = input.read(cx).value().to_string();
                ws.update(cx, |w, cx| w.rename(&id, &value, cx));
                cx.notify();
            }
        });
        Renaming { chat: chat.to_owned(), input, home, workspace, _sub: sub }
    }

    /// The input let go: the keyboard, if it was there, goes back to the
    /// window and on to the terminal of the chat the board's keyboard
    /// belongs to (`Workspace::focus_home`), as a dialog's close sends it.
    /// Left in an input that is gone, gpui would send every key from the
    /// window's root, above the window's key context, and none of the
    /// window's keys (Ctrl+Tab among them) would work.
    pub fn give_back(&self, window: &mut Window, cx: &mut App) {
        if self.input.read(cx).focus_handle(cx).contains_focused(window, cx) {
            window.focus(&self.home, cx);
            self.workspace.update(cx, |w, cx| {
                w.focus_home();
                cx.notify();
            });
        }
    }
}
