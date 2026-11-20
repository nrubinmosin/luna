//! The crate's debug selectors: `.selector(..)` where gpui has
//! `.debug_selector(..)`, which it calls. In the crate's own tests it also
//! draws a probe inside the element that notes the content mask it is painted
//! under: what every ancestor that clips (a frame's `overflow_hidden`, a
//! scroller's viewport) leaves of the window there. gpui's `debug_bounds`
//! say only where an element was laid out, not how much of it shows, and
//! fit_tests.rs needs both to find a control cut off inside a frame that is
//! itself on screen.

use gpui::{InteractiveElement, ParentElement};

pub trait Selector: InteractiveElement + ParentElement + Sized {
    /// Names the element for the tests' `debug_bounds` (debug builds only,
    /// as gpui records them), and in the crate's tests notes its clip.
    fn selector(self, name: impl FnOnce() -> String) -> Self;
}

impl<E: InteractiveElement + ParentElement> Selector for E {
    #[cfg(not(test))]
    fn selector(self, name: impl FnOnce() -> String) -> Self {
        self.debug_selector(name)
    }

    #[cfg(test)]
    fn selector(self, name: impl FnOnce() -> String) -> Self {
        let name = name();
        let probe = clips::probe(name.clone());
        self.debug_selector(move || name).child(probe)
    }
}

/// The content masks the probes noted, by selector: the last frame each was
/// painted in, on this thread (a test's window lives on its own).
#[cfg(test)]
pub mod clips {
    use gpui::{Bounds, IntoElement, Pixels, Styled, canvas};
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static CLIPS: RefCell<HashMap<String, Bounds<Pixels>>> = RefCell::new(HashMap::new());
    }

    /// Nothing to see and no room taken: positioned out of the flow, with no
    /// size, it moves no sibling and catches no click. A view gpui reuses
    /// from the last frame repaints none of its probes, and keeps its notes:
    /// gpui reuses it only where its bounds and its mask are as they were.
    pub(super) fn probe(name: String) -> impl IntoElement {
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let mask = window.content_mask().bounds;
                CLIPS.with(|c| c.borrow_mut().insert(name, mask));
            },
        )
        .absolute()
        .size_0()
    }

    /// The mask `selector` was last painted under.
    pub fn of(selector: &str) -> Option<Bounds<Pixels>> {
        CLIPS.with(|c| c.borrow().get(selector).copied())
    }

    /// Every selector a probe has noted since `forget`.
    pub fn noted() -> Vec<String> {
        CLIPS.with(|c| c.borrow().keys().cloned().collect())
    }

    /// Forgets every note, as a new window opens.
    pub fn forget() {
        CLIPS.with(|c| c.borrow_mut().clear());
    }
}
