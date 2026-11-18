//! Said across the top of the board while the core is not there to run the
//! sessions (the page's CoreBanner.tsx, in its words). While the core is
//! being found, started or replaced the window waits and says so, in a color
//! of its own: that is no failure, and nothing here calls it one. A core that
//! stopped, one of another build, or none that is not run as administrator is
//! said in the colors of trouble, with the way back where a restart helps.
//! Never restarted behind the person's back: a core that keeps falling over
//! must not hide it.

use crate::dto::{CoreState, LinkState};
use crate::models::{CoreLink, Models};
use crate::theme::Palette;
use crate::widgets::button;
use gpui::{
    Context, FontWeight, InteractiveElement, IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    div, px,
};

/// How a banner is colored: by what it asks of the person.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Nothing: Luna is getting there by itself.
    Waiting,
    /// A decision: the sessions run in another build's core.
    Decide,
    /// Something went wrong.
    Trouble,
}

/// What the banner says for a state, or None for no banner.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub tone: Tone,
    pub text: String,
    /// Restart the core is offered.
    pub restart: bool,
}

/// `was_up`: the core had been up in this window, so one starting now is one
/// being replaced.
pub fn said(link: &CoreState, was_up: bool) -> Option<Said> {
    let (tone, text, restart) = match link.state {
        LinkState::Up => return None,
        LinkState::Starting if was_up => {
            (Tone::Waiting, "Waiting for Luna's core: it is being replaced. The chats resume in the new one.".to_owned(), false)
        }
        LinkState::Starting => (Tone::Waiting, "Waiting for Luna's core: finding the one running, or starting it.".to_owned(), false),
        LinkState::Mismatch => {
            let (version, sessions) = link.core.as_ref().map(|c| (c.version.as_str(), c.sessions)).unwrap_or(("?", 0));
            (
                Tone::Decide,
                format!(
                    "The sessions are running in Luna's core from another build ({version}, {sessions} running). Restart it to work \
                     with them here: they quit and resume in the new one."
                ),
                true,
            )
        }
        LinkState::Down => {
            let why = link.error.as_ref().map(|e| format!(" ({e})")).unwrap_or_default();
            (
                Tone::Trouble,
                format!("Luna's core stopped, and the sessions in it ended{why}. Restart it and the chats resume from their transcripts."),
                true,
            )
        }
        // A restart helps only when there is an elevated core with sessions
        // to replace; a window that is itself elevated is mended by starting
        // Luna normally.
        LinkState::Elevated => (
            Tone::Trouble,
            link.error.clone().unwrap_or_else(|| "Luna is running as administrator; restart it normally.".to_owned()),
            link.core.is_some(),
        ),
    };
    Some(Said { tone, text, restart })
}

pub struct Banner {
    models: Models,
    pub(crate) renders: usize,
}

impl Banner {
    pub fn new(models: Models, cx: &mut Context<Self>) -> Banner {
        cx.observe(&models.core, |_, _, cx| cx.notify()).detach();
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        Banner { models, renders: 0 }
    }
}

fn fill(p: &Palette, tone: Tone) -> gpui::Hsla {
    match tone {
        Tone::Waiting => p.info,
        Tone::Decide => p.attention,
        Tone::Trouble => p.error,
    }
}

impl Render for Banner {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        let core: &CoreLink = self.models.core.read(cx);
        let Some(said) = said(core.link(), core.was_up()) else { return div().id("core-banner") };
        let p = self.models.shell.read(cx).palette();
        let models = self.models.clone();
        let restart = said.restart.then(|| {
            div()
                .id("core-restart")
                .debug_selector(|| "core-restart".into())
                .flex_none()
                .on_click(move |_, _, cx| models.restart_core(cx))
                .child(button(&p, "Restart the core"))
        });
        div()
            .id("core-banner")
            .flex()
            .flex_none()
            .items_center()
            .gap(px(10.))
            .px(px(8.))
            .py(px(4.))
            .bg(fill(&p, said.tone))
            .text_color(p.on_status)
            .border_b_1()
            .border_color(p.frame)
            .child(div().flex_1().min_w_0().font_weight(FontWeight::BOLD).child(SharedString::from(said.text)))
            .children(restart)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::Core;

    fn link(state: LinkState, core: Option<Core>, error: Option<&str>) -> CoreState {
        CoreState { state, core, error: error.map(String::from) }
    }

    #[test]
    fn each_state_says_its_own_and_offers_restart_where_it_helps() {
        assert_eq!(said(&link(LinkState::Up, None, None), true), None);

        let first = said(&link(LinkState::Starting, None, None), false).unwrap();
        let again = said(&link(LinkState::Starting, None, None), true).unwrap();
        for s in [&first, &again] {
            assert_eq!(s.tone, Tone::Waiting);
            assert!(!s.restart);
            assert!(s.text.starts_with("Waiting for Luna's core"), "{}", s.text);
            // Waiting is never told as a failure.
            assert!(!s.text.contains("not running") && !s.text.contains("stopped"), "{}", s.text);
        }
        assert!(again.text.contains("being replaced"));

        let down = said(&link(LinkState::Down, None, Some("no core came up within 10s")), true).unwrap();
        assert_eq!((down.tone, down.restart), (Tone::Trouble, true));
        assert!(down.text.contains("(no core came up within 10s)"), "{}", down.text);

        let other = Core { version: "0.18.2".into(), sessions: 3, ..Default::default() };
        let mismatch = said(&link(LinkState::Mismatch, Some(other.clone()), None), true).unwrap();
        assert_eq!((mismatch.tone, mismatch.restart), (Tone::Decide, true));
        assert!(mismatch.text.contains("(0.18.2, 3 running)"), "{}", mismatch.text);

        let stuck = said(&link(LinkState::Elevated, None, Some("why")), false).unwrap();
        assert_eq!((stuck.tone, stuck.restart, stuck.text.as_str()), (Tone::Trouble, false, "why"));
        assert!(said(&link(LinkState::Elevated, Some(other), Some("why")), false).unwrap().restart);
        assert!(said(&link(LinkState::Elevated, None, None), false).unwrap().text.contains("administrator"));
    }
}
