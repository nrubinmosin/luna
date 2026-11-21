//! A chat's name being edited in place, in the sidebar or on a pane's title
//! bar: gpui-component's text input over the name, Enter or a click
//! elsewhere keeps what was typed, Esc (keys.rs) lets it go.

use crate::workspace::Workspace;
use gpui::{AppContext, Context, Entity, Subscription, Window};
use gpui_component::input::{InputEvent, InputState};

pub struct Renaming {
    pub chat: String,
    pub input: Entity<InputState>,
    _sub: Subscription,
}

impl Renaming {
    /// Starts editing `chat`'s name in the view whose `slot` holds it.
    pub fn start<V: 'static>(
        chat: &str,
        name: &str,
        workspace: Entity<Workspace>,
        slot: fn(&mut V) -> &mut Option<Renaming>,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> Renaming {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name.to_owned()));
        input.update(cx, |i, cx| i.focus(window, cx));
        let id = chat.to_owned();
        let sub = cx.subscribe_in(&input, window, move |this, input, e: &InputEvent, _, cx| {
            if !matches!(e, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                return;
            }
            let held = slot(this);
            if held.as_ref().is_some_and(|r| r.chat == id) {
                *held = None;
                let value = input.read(cx).value().to_string();
                workspace.update(cx, |w, cx| w.rename(&id, &value, cx));
                cx.notify();
            }
        });
        Renaming { chat: chat.to_owned(), input, _sub: sub }
    }
}
