//! The crate's debug selectors: `.selector(..)` where gpui has
//! `.debug_selector(..)`, which it calls. In the crate's own tests it also
//! draws a probe inside the element that notes the content mask it is painted
//! under (what every ancestor that clips, a frame's `overflow_hidden` or a
//! scroller's viewport, leaves of the window there) and a hitbox over it.
//! gpui's `debug_bounds` say only where an element was laid out, not how much
//! of it shows nor whether a click there reaches it; fit_tests.rs needs all
//! three to find a control cut off inside a frame that is itself on screen,
//! or covered by something drawn over it.

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

/// What the probes noted, by selector: the content mask each was last
/// painted under and its hitbox, on this thread (a test's window lives on
/// its own).
#[cfg(test)]
pub mod clips {
    use gpui::{Bounds, Hitbox, HitboxBehavior, IntoElement, Pixels, Styled, canvas};
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    thread_local! {
        static CLIPS: RefCell<HashMap<String, Note>> = RefCell::new(HashMap::new());
        static NEXT: Cell<u64> = const { Cell::new(0) };
    }

    /// What a probe noted: the mask, the hitbox, and when it was laid out,
    /// counted across frames, so a probe laid out later in a frame (inside an
    /// element whose probe came first, or drawn over it) has the higher one.
    #[derive(Clone)]
    struct Note {
        mask: Bounds<Pixels>,
        hitbox: Hitbox,
        order: u64,
    }

    /// Nothing to see and no room taken: positioned out of the flow over the
    /// element, it moves no sibling. Its hitbox blocks nothing (`Normal`), so
    /// every click and hover goes where it went without it; a hit test at a
    /// point says whether a click there would reach the element, or whether
    /// something drawn over it that takes the mouse (`occlude`) is in the way.
    /// A view gpui reuses from the last frame repaints none of its probes,
    /// and keeps its notes: gpui reuses it, hitboxes and all, only where its
    /// bounds and its mask are as they were.
    pub(super) fn probe(name: String) -> impl IntoElement {
        canvas(
            |bounds, window, _| {
                let order = NEXT.with(|n| n.replace(n.get() + 1));
                (window.insert_hitbox(bounds, HitboxBehavior::Normal), order)
            },
            move |_, (hitbox, order), window, _| {
                let mask = window.content_mask().bounds;
                CLIPS.with(|c| c.borrow_mut().insert(name, Note { mask, hitbox, order }));
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The mask `selector` was last painted under.
    pub fn of(selector: &str) -> Option<Bounds<Pixels>> {
        CLIPS.with(|c| c.borrow().get(selector).map(|n| n.mask))
    }

    /// The hitbox of `selector`'s probe, over the element.
    pub fn hitbox(selector: &str) -> Option<Hitbox> {
        CLIPS.with(|c| c.borrow().get(selector).map(|n| n.hitbox.clone()))
    }

    /// When `selector`'s probe was laid out, against the others.
    pub fn order(selector: &str) -> Option<u64> {
        CLIPS.with(|c| c.borrow().get(selector).map(|n| n.order))
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
