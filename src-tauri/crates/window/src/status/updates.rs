//! The Luna update as the bar tells it: where it stands (`Update`), the
//! chip's words, and what the confirmation says. The steps themselves, the
//! calls to the host, are the model's (vitals.rs).
//!
//! The page's cycle, kept: a check at startup, quiet about failing; a
//! release found lights a chip that waits for a click; the click opens the
//! confirmation with the notes; its yes downloads the installer, then, if the
//! release's core is not the running one, stops every session, then installs.
//! New here: an install that cannot first write what Luna holds is refused
//! (`unsaved: `), and the person chooses Retry, Install anyway, or Cancel.

use crate::desktop::Offer;

/// What the host's install refusal begins with when what Luna holds could not
/// be written first (native/desktop.rs `UNSAVED`).
pub const UNSAVED: &str = "unsaved: ";

/// Why an install is refused when the core running as the installer is
/// about to start is not the one the update stopped the sessions in (or
/// kept them in): a core started meanwhile would run under the installer.
pub const CORE_CHANGED: &str = "Luna's core changed while the update was getting ready; check again";

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Update {
    /// Nothing on offer, or not looked yet.
    #[default]
    Idle,
    /// A release waits for a click.
    Available(Offer),
    /// The installer coming down; `update://progress` says how far.
    Downloading { offer: Offer, got: u64, total: Option<u64> },
    /// Every session being asked to quit, before the install.
    Stopping(Offer),
    /// The install asked for; Luna ends as it starts.
    Installing(Offer),
    /// The install was refused: Luna's own state could not be written first.
    /// `keep`: the sessions were left running (the release's core is the
    /// running one), so Cancel has nothing to call off.
    Unsaved { offer: Offer, why: String, keep: bool },
    /// A check, a download or an install failed. With an offer, it can be
    /// tried again.
    Failed { offer: Option<Offer>, why: String },
}

impl Update {
    /// A download, an install or a stop is under way: no check, no second
    /// install.
    pub fn busy(&self) -> bool {
        matches!(self, Update::Downloading { .. } | Update::Stopping(_) | Update::Installing(_))
    }
}

/// How far a download is, 0 to 100; None before the server says its length.
pub fn percent(got: u64, total: Option<u64>) -> Option<u64> {
    total.filter(|t| *t > 0).map(|t| (got.saturating_mul(100) / t).min(100))
}

/// The chip's kind, which decides its fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// News that waits for a click.
    Loud,
    /// Work under way.
    Busy,
    /// Something to look at.
    Bad,
}

/// The chip, or None while there is no news.
pub fn chip(update: &Update) -> Option<(Tone, String)> {
    Some(match update {
        Update::Idle => return None,
        Update::Available(o) => (Tone::Loud, format!("update {}", o.version)),
        Update::Downloading { got, total, .. } => {
            (Tone::Busy, format!("downloading {}", percent(*got, *total).map_or("…".to_owned(), |p| format!("{p}%"))))
        }
        Update::Stopping(_) => (Tone::Busy, "stopping sessions…".into()),
        Update::Installing(_) => (Tone::Busy, "installing…".into()),
        Update::Unsaved { .. } => (Tone::Bad, "update: not saved".into()),
        Update::Failed { .. } => (Tone::Bad, "update failed".into()),
    })
}

/// The reason the install gives for Luna's exit, for the log.
pub fn reason(offer: &Offer, keep: bool) -> String {
    format!("installing the update to {}{}", offer.version, if keep { "; the sessions keep running in the core" } else { "" })
}

/// Whether installing `offer` leaves the running core, and the sessions in
/// it, alone: the release was built with the core that runs.
pub fn keeps_sessions(offer: &Offer, running: Option<&str>) -> bool {
    matches!((offer.core_fingerprint.as_deref(), running), (Some(a), Some(b)) if a == b)
}

/// What the confirmation says of the sessions.
pub fn sessions_line(live: usize, keep: bool) -> String {
    let (one, n) = (live == 1, live);
    match (live, keep) {
        (0, _) => "No session is running, so nothing stops.".into(),
        (_, true) => format!(
            "This update leaves Luna's core as it is, so only the window restarts: {} running, background work and all, and the new window picks {} up.",
            if one { "the session keeps".to_owned() } else { format!("the {n} sessions keep") },
            if one { "it" } else { "them" }
        ),
        (_, false) => format!(
            "Once the download is through, {} asked to quit, the way deleting a chat does: whatever is mid-turn or running in the background stops with {}. The chats resume from their transcripts when Luna is back.",
            if one { "the running session is".to_owned() } else { format!("all {n} running sessions are") },
            if one { "it" } else { "them" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(core: Option<&str>) -> Offer {
        Offer { version: "0.20.1".into(), core_fingerprint: core.map(str::to_owned), ..Default::default() }
    }

    #[test]
    fn the_chip_speaks_only_while_there_is_news() {
        assert_eq!(chip(&Update::Idle), None);
        let o = offer(None);
        assert_eq!(chip(&Update::Available(o.clone())), Some((Tone::Loud, "update 0.20.1".into())));
        let down = |got, total| Update::Downloading { offer: o.clone(), got, total };
        assert_eq!(chip(&down(0, None)), Some((Tone::Busy, "downloading …".into())));
        assert_eq!(chip(&down(50, Some(200))), Some((Tone::Busy, "downloading 25%".into())));
        assert_eq!(chip(&down(500, Some(200))).unwrap().1, "downloading 100%", "never past the whole");
        assert_eq!(chip(&Update::Stopping(o.clone())).unwrap().1, "stopping sessions…");
        assert_eq!(chip(&Update::Unsaved { offer: o.clone(), why: "x".into(), keep: false }).unwrap().0, Tone::Bad);
        assert_eq!(chip(&Update::Failed { offer: None, why: "x".into() }).unwrap().1, "update failed");
        assert!(down(1, None).busy() && Update::Stopping(o.clone()).busy() && Update::Installing(o).busy());
        assert!(!Update::Idle.busy());
    }

    #[test]
    fn a_release_built_with_the_running_core_keeps_the_sessions() {
        assert!(keeps_sessions(&offer(Some("abc")), Some("abc")));
        assert!(!keeps_sessions(&offer(Some("abc")), Some("abd")));
        assert!(!keeps_sessions(&offer(None), Some("abc")), "a release that does not say stops them");
        assert!(!keeps_sessions(&offer(Some("abc")), None), "and so does a core that is not there");
        assert_eq!(reason(&offer(None), false), "installing the update to 0.20.1");
        assert_eq!(reason(&offer(None), true), "installing the update to 0.20.1; the sessions keep running in the core");
    }

    #[test]
    fn the_confirmation_tells_what_happens_to_the_sessions() {
        assert!(sessions_line(0, false).starts_with("No session"));
        assert!(sessions_line(1, true).contains("the session keeps running"));
        assert!(sessions_line(3, true).contains("the 3 sessions keep running"));
        assert!(sessions_line(3, false).contains("all 3 running sessions are asked to quit"));
        assert!(sessions_line(1, false).contains("the running session is asked to quit"));
    }
}
