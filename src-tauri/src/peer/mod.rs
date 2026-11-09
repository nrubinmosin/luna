//! Who opened the core's endpoint (endpoint/), as the OS says, and whether
//! that is let in.
//!
//! The core has no secret to check. A process is let in for what the OS says
//! of it as it connects: this user's, confined by no sandbox, of the core's
//! integrity or above — the one a window or the test build is, and the one a
//! CLI session in its sandbox is not. What is read, and when:
//!
//! - Windows: the core impersonates the client of the pipe for a moment and
//!   reads the token the client opened it with (`ImpersonateNamedPipeClient`,
//!   at identification level, which is all a window grants): its user, whether
//!   it is restricted (Codex's unelevated sandbox) or an AppContainer, and its
//!   integrity. That token, not the process's: a thread opening the pipe under
//!   a token of its own is judged by that one. A client that opens without a
//!   static security context (a shell's redirect) has no token to read before
//!   it writes, and is refused; Luna's own always pass one. Codex's sandbox
//!   does not get as far: the pipe's DACL refuses its open (endpoint/win.rs);
//!   this check is what holds if a sandbox's token ever passes the DACL.
//! - Linux: the credentials the kernel recorded at connect (`SO_PEERCRED`):
//!   its uid, and from `/proc/<pid>` its user namespace, `no_new_privs` and
//!   seccomp state. Codex's sandbox (bubblewrap) runs its commands in a user
//!   namespace of their own and sets `no_new_privs` for the seccomp filter it
//!   adds, as Landlock needs the bit too; a core started plainly has neither.
//!   A core started with `no_new_privs` (Flatpak, systemd's
//!   NoNewPrivileges) cannot refuse for the bit, which its windows carry
//!   too; it still refuses another namespace or more seccomp filters than
//!   its own, but a client confined by Landlock alone is not seen: the kernel
//!   shows no Landlock domain to anyone else (`note_launch`, ADR-0021). The
//!   pid is held by a pidfd where the kernel gives one (`SO_PEERPIDFD`, 6.5),
//!   so what `/proc` said is known to be of the process that connected, not
//!   of one that took its pid since.
//! - macOS: the uid (`getpeereid`) and that the pid can be read
//!   (`LOCAL_PEERPID`). A Seatbelt sandbox is not seen there.
//!
//! Anything that cannot be read refuses the connection (fail closed). The
//! decision (`verdict`) is pure, so a test feeds it the facts a connection
//! would carry; reading them is each OS's (`imp`).

#[cfg_attr(windows, path = "win.rs")]
#[cfg_attr(not(windows), path = "unix.rs")]
mod imp;

use crate::endpoint::Stream;

/// `SECURITY_MANDATORY_MEDIUM_RID`: an ordinary process's integrity. Off
/// Windows every process of a user other than root counts as this.
#[cfg_attr(windows, allow(dead_code))]
pub const MEDIUM: u32 = 0x2000;
/// `SECURITY_MANDATORY_HIGH_RID`: an elevated administrator's. Off Windows,
/// root counts as this.
pub const HIGH: u32 = 0x3000;

/// What the OS says of the process at the other end of a connection.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerFacts {
    pub pid: u32,
    /// Its image file name, for the log; empty when it cannot be read.
    pub image: String,
    /// The core's own user: the same SID, or the same uid.
    pub same_user: bool,
    /// Windows: a restricted token, as Codex's unelevated sandbox gives.
    pub restricted: bool,
    /// Windows: an AppContainer token.
    pub app_container: bool,
    /// The mandatory-label RID on Windows; `MEDIUM`, or `HIGH` for root, off it.
    pub integrity: u32,
    /// Linux: in a user namespace other than the core's (bubblewrap's).
    pub foreign_userns: bool,
    /// Linux: `no_new_privs` set, as a seccomp or Landlock sandbox sets it.
    pub no_new_privs: bool,
    /// Linux: its seccomp state.
    pub seccomp: Seccomp,
}

/// A process's seccomp state, from `/proc/<pid>/status`: the mode (0 none, 1
/// strict, 2 filter) and, where the kernel says (5.9), how many filters are
/// stacked. A process can add filters and never take one away, and a child
/// starts with its parent's, so a peer with a stricter mode or more filters
/// than the core has been confined since it parted from it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Seccomp {
    pub mode: u32,
    pub filters: Option<u32>,
}

impl Seccomp {
    /// Stricter than `other`: a higher mode, or as many filters and then some.
    /// Where either count is unknown, the mode alone decides.
    pub fn stricter_than(&self, other: &Seccomp) -> bool {
        self.mode > other.mode || (self.mode == other.mode && matches!((self.filters, other.filters), (Some(a), Some(b)) if a > b))
    }
}

/// What the core says of itself, to judge a peer against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CoreFacts {
    pub integrity: u32,
    pub no_new_privs: bool,
    pub seccomp: Seccomp,
    /// It runs as administrator and starts nothing (elevation.rs).
    pub refuses_work: bool,
}

/// Whether a connection is let through, and why not when it is turned away.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Allow,
    /// Let in to stop the core and nothing else: an ordinary window below a
    /// core run as administrator, which runs no session and so has nothing
    /// else to give (elevation.rs), and which that window has no other way to
    /// stop; or an administrator of another account, who could end the
    /// process anyway and may want it gone before an uninstall.
    StopOnly,
    /// Refused, with a reason safe to log.
    Refuse(&'static str),
}

/// The decision, fail closed: a peer that could not be read, one confined by
/// a sandbox the OS marks, another user (but an administrator, who may stop
/// it), or one below the core's integrity is refused; this user's own
/// unconfined processes at the core's integrity or above are let in.
pub fn verdict(peer: Option<&PeerFacts>, core: &CoreFacts) -> Verdict {
    let Some(p) = peer else {
        return Verdict::Refuse("the connecting process could not be read");
    };
    if p.restricted {
        Verdict::Refuse("a restricted token (a CLI sandbox)")
    } else if p.app_container {
        Verdict::Refuse("an app container")
    } else if p.foreign_userns {
        Verdict::Refuse("another user namespace (a CLI sandbox)")
    } else if p.no_new_privs && !core.no_new_privs {
        Verdict::Refuse("no_new_privs set, where the core's is not (a CLI sandbox)")
    } else if p.seccomp.stricter_than(&core.seccomp) {
        Verdict::Refuse("a stricter seccomp filter than the core's (a CLI sandbox)")
    } else if !p.same_user && p.integrity >= HIGH {
        Verdict::StopOnly
    } else if !p.same_user {
        Verdict::Refuse("another user")
    } else if p.integrity < core.integrity && core.refuses_work {
        Verdict::StopOnly
    } else if p.integrity < core.integrity {
        Verdict::Refuse("a lower integrity level")
    } else {
        Verdict::Allow
    }
}

/// How far `stream`'s client is let in: `Ok(false)` all the way, `Ok(true)`
/// to stop the core only, or why it is refused, naming the process for the
/// log.
pub fn admission(stream: &Stream, core_refuses_work: bool) -> Result<bool, String> {
    // Without the core's own facts there is nothing to judge a peer against,
    // so nobody is let in until they can be read.
    let Some(core) = imp::core_facts(core_refuses_work) else {
        return Err("the core's own token could not be read".to_string());
    };
    let facts = imp::peer_facts(stream);
    match verdict(facts.as_ref(), &core) {
        Verdict::Allow => Ok(false),
        Verdict::StopOnly => Ok(true),
        Verdict::Refuse(why) => Err(match &facts {
            Some(f) => format!("pid {} ({}): {why}", f.pid, f.image),
            None => why.to_string(),
        }),
    }
}

/// Said once as the core starts, where it runs with no_new_privs (inherited
/// from what started it: Flatpak, systemd's NoNewPrivileges, a container).
/// Every window has the bit then too, so it tells a sandbox nothing, and a
/// client confined by Landlock alone in the core's own user namespace, under
/// its seccomp state, cannot be told from a window: the kernel shows no
/// Landlock domain to anyone else (ADR-0021).
pub fn note_launch() {
    if imp::core_facts(false).is_some_and(|c| c.no_new_privs) {
        crate::log::warn(
            "core",
            "the core runs with no_new_privs, inherited from what started it: a client confined by Landlock alone in this user \
             namespace cannot be told from a window (another namespace or a stricter seccomp filter still is)",
        );
    }
}

/// The reason to refuse `stream`'s client from a core that works, or None to
/// let it through (`admission`).
#[cfg_attr(not(test), allow(dead_code))]
pub fn refuse_reason(stream: &Stream) -> Option<String> {
    admission(stream, false).err()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOW: u32 = 0x1000;

    fn plain() -> PeerFacts {
        PeerFacts {
            pid: 100,
            image: "luna.exe".into(),
            same_user: true,
            restricted: false,
            app_container: false,
            integrity: MEDIUM,
            foreign_userns: false,
            no_new_privs: false,
            seccomp: Seccomp::default(),
        }
    }

    fn core(integrity: u32) -> CoreFacts {
        CoreFacts { integrity, no_new_privs: false, seccomp: Seccomp::default(), refuses_work: false }
    }

    fn filtered(filters: u32) -> Seccomp {
        Seccomp { mode: 2, filters: Some(filters) }
    }

    fn refused(v: Verdict) -> bool {
        matches!(v, Verdict::Refuse(_))
    }

    #[test]
    fn a_plain_peer_at_the_cores_integrity_is_let_in() {
        assert_eq!(verdict(Some(&plain()), &core(MEDIUM)), Verdict::Allow);
        // Above the core's integrity is fine too: an elevated window of the
        // same user, with the de-elevated core it started.
        let high = PeerFacts { integrity: HIGH, ..plain() };
        assert_eq!(verdict(Some(&high), &core(MEDIUM)), Verdict::Allow);
    }

    #[test]
    fn a_sandboxed_or_unknown_peer_is_refused() {
        let restricted = PeerFacts { restricted: true, ..plain() };
        let contained = PeerFacts { app_container: true, ..plain() };
        let low = PeerFacts { integrity: LOW, ..plain() };
        for (facts, which) in [(&restricted, "restricted"), (&contained, "app container"), (&low, "low integrity")] {
            assert!(refused(verdict(Some(facts), &core(MEDIUM))), "{which} should be refused");
        }
        // The process could not be read: fail closed.
        assert!(refused(verdict(None, &core(MEDIUM))), "unknown is refused");
    }

    /// Off Windows a sandbox shows as a user namespace of its own (bubblewrap,
    /// Codex's Linux sandbox) or as no_new_privs (its seccomp filter, or
    /// Landlock), with the same uid as the core.
    #[test]
    fn a_peer_in_another_user_namespace_or_with_no_new_privs_is_refused() {
        let namespaced = PeerFacts { foreign_userns: true, ..plain() };
        assert_eq!(verdict(Some(&namespaced), &core(MEDIUM)), Verdict::Refuse("another user namespace (a CLI sandbox)"));
        let nnp = PeerFacts { no_new_privs: true, ..plain() };
        assert!(refused(verdict(Some(&nnp), &core(MEDIUM))));
        // Neither lets a sandbox through a core that refuses work.
        let refusing = CoreFacts { refuses_work: true, ..core(HIGH) };
        assert!(refused(verdict(Some(&PeerFacts { integrity: LOW, ..namespaced }), &refusing)));
        assert!(refused(verdict(Some(&PeerFacts { integrity: LOW, ..nnp }), &refusing)));
    }

    /// A peer under a stricter seccomp state than the core's has been confined
    /// since it parted from it (Codex's bubblewrap sandbox adds a filter):
    /// refused. One under the core's own, or a laxer one, is not.
    #[test]
    fn a_peer_under_a_stricter_seccomp_than_the_cores_is_refused() {
        let reason = Verdict::Refuse("a stricter seccomp filter than the core's (a CLI sandbox)");
        // A filter where the core has none; one more filter than the core's.
        assert_eq!(verdict(Some(&PeerFacts { seccomp: filtered(1), ..plain() }), &core(MEDIUM)), reason);
        let under_two = CoreFacts { seccomp: filtered(2), ..core(MEDIUM) };
        assert_eq!(verdict(Some(&PeerFacts { seccomp: filtered(3), ..plain() }), &under_two), reason);
        // Strict mode is stricter than any filter.
        assert_eq!(verdict(Some(&PeerFacts { seccomp: Seccomp { mode: 1, filters: Some(0) }, ..plain() }), &core(MEDIUM)), reason);
        // The core's own state (a window started where the core was, inside a
        // container's filters), or fewer filters: let in.
        assert_eq!(verdict(Some(&PeerFacts { seccomp: filtered(2), ..plain() }), &under_two), Verdict::Allow);
        assert_eq!(verdict(Some(&PeerFacts { seccomp: filtered(1), ..plain() }), &under_two), Verdict::Allow);
        // With no count from the kernel (before 5.9) the mode alone decides.
        let uncounted = |mode| Seccomp { mode, filters: None };
        let core_uncounted = CoreFacts { seccomp: uncounted(2), ..core(MEDIUM) };
        assert_eq!(verdict(Some(&PeerFacts { seccomp: uncounted(2), ..plain() }), &core_uncounted), Verdict::Allow);
        assert_eq!(verdict(Some(&PeerFacts { seccomp: uncounted(2), ..plain() }), &core(MEDIUM)), reason);
    }

    /// A core started with no_new_privs (under Flatpak, systemd's
    /// NoNewPrivileges, a container) cannot refuse a peer for it: the window
    /// that started it, and every later one, has it too. There it still
    /// refuses another user namespace and a stricter seccomp state. What it
    /// cannot see is Landlock: the kernel shows no Landlock domain to procfs,
    /// so a client confined by Landlock alone, in the core's namespace and
    /// under its seccomp state, is let in — the residual ADR-0021 names.
    #[test]
    fn under_a_core_with_no_new_privs_a_namespace_or_seccomp_still_refuses_and_landlock_alone_does_not() {
        let under_nnp = CoreFacts { no_new_privs: true, seccomp: filtered(1), ..core(MEDIUM) };
        let window = PeerFacts { no_new_privs: true, seccomp: filtered(1), ..plain() };
        assert_eq!(verdict(Some(&window), &under_nnp), Verdict::Allow, "its own window");
        let landlock_only = window.clone();
        assert_eq!(verdict(Some(&landlock_only), &under_nnp), Verdict::Allow, "the documented residual: nothing tells it apart");
        let codex = PeerFacts { seccomp: filtered(2), ..window.clone() };
        assert!(refused(verdict(Some(&codex), &under_nnp)), "a seccomp sandbox on top is refused");
        let namespaced = PeerFacts { foreign_userns: true, ..window };
        assert!(refused(verdict(Some(&namespaced), &under_nnp)), "so is a namespace of its own");
    }

    /// Another user is refused; an administrator of another account (or root)
    /// may stop the core and do nothing else, as before, when the core's file
    /// let Administrators read its token.
    #[test]
    fn another_user_is_refused_and_an_administrator_may_only_stop_it() {
        let other = PeerFacts { same_user: false, ..plain() };
        assert_eq!(verdict(Some(&other), &core(MEDIUM)), Verdict::Refuse("another user"));
        let admin = PeerFacts { same_user: false, integrity: HIGH, ..plain() };
        assert_eq!(verdict(Some(&admin), &core(MEDIUM)), Verdict::StopOnly);
        let system = PeerFacts { same_user: false, integrity: 0x4000, ..plain() };
        assert_eq!(verdict(Some(&system), &core(MEDIUM)), Verdict::StopOnly);
        // A sandboxed administrator is a sandbox first.
        assert!(refused(verdict(Some(&PeerFacts { restricted: true, ..admin.clone() }), &core(MEDIUM))));
        assert!(refused(verdict(Some(&PeerFacts { foreign_userns: true, ..admin }), &core(MEDIUM))));
    }

    #[test]
    fn a_core_run_as_administrator_may_be_stopped_by_an_ordinary_window_and_no_sandbox() {
        let refusing = CoreFacts { refuses_work: true, ..core(HIGH) };
        // The ordinary window below an elevated core that refuses all work:
        // let in to stop it, nothing more.
        assert_eq!(verdict(Some(&plain()), &refusing), Verdict::StopOnly);
        // Not by a core that works: there it is refused as ever.
        assert!(refused(verdict(Some(&plain()), &core(HIGH))));
        // A sandboxed, contained or unknown peer is refused even by a core that
        // refuses work, at any integrity.
        for p in [PeerFacts { restricted: true, ..plain() }, PeerFacts { app_container: true, ..plain() }] {
            assert!(refused(verdict(Some(&p), &refusing)), "{p:?}");
            let low = PeerFacts { integrity: LOW, ..p };
            assert!(refused(verdict(Some(&low), &refusing)), "{low:?}");
        }
        assert!(refused(verdict(None, &refusing)));
        // One of the core's own integrity has the full way in.
        assert_eq!(verdict(Some(&PeerFacts { integrity: HIGH, ..plain() }), &refusing), Verdict::Allow);
    }

    /// The test process, connecting to an endpoint it serves itself, is a
    /// plain process of the core's own user: let all the way in.
    #[test]
    fn this_process_is_let_in_at_its_own_endpoint() {
        let (_l, _client, server) = crate::endpoint::tests::pair();
        assert_eq!(admission(&server, false), Ok(false));
        let facts = imp::peer_facts(&server).expect("facts");
        assert_eq!(facts.pid, std::process::id());
        assert!(facts.same_user && !facts.restricted && !facts.foreign_userns, "{facts:?}");
    }
}
