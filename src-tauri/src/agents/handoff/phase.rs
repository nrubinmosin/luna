//! The switch from one conversation of a chat to the next, as a machine whose
//! every phase is saved (store.rs): `next` says what a look at the chat calls
//! for, and `run` does it through a `Term`, the chat's terminal or a test's
//! stand-in for one.
//!
//! Scheduled → (the chat at rest on two looks, no draft, no job of its own)
//! Resetting: the reset command typed → (the CLI on another conversation)
//! Prompting: the continuation prompt typed → (the new transcript holds it)
//! done. A core that stops in Resetting or Prompting leaves the switch
//! Recovering: once the chat is back and at rest, what it is on says which —
//! the old conversation (tried again from Scheduled), or the new one with the
//! prompt in it (done) or without (typed now).

use crate::provider::Provider;
use serde::{Deserialize, Serialize};

/// Looks in a row the chat is found at rest before the reset goes in: a turn
/// that has just ended may still be followed by one it queued.
pub(super) const REST_LOOKS: u32 = 2;
/// How long the CLI gets to be on a new conversation after the reset command.
pub(super) const RESET_WAIT_MS: u64 = 60_000;
/// How long the new conversation gets to hold the continuation prompt: Codex
/// writes a new thread's rollout only with its first turn.
pub(super) const PROMPT_WAIT_MS: u64 = 60_000;
/// How long a scheduled handoff waits for the chat to come to rest: past its
/// own turn, a person typing, a draft. A draft holds it as long as it stands.
pub(super) const REST_MAX_MS: u64 = 30 * 60_000;
/// How long a chat that is not running gets to come back: a core restart has
/// the window start its CLI again, seconds later.
pub(super) const RETURN_MS: u64 = 10 * 60_000;
/// Resets tried before a handoff is given up: one, and one more after a core
/// stopped in the middle of the first.
pub(super) const MAX_ATTEMPTS: u32 = 2;

/// Where a handoff stands.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub(in crate::agents) enum Phase {
    /// The snapshot is taken; the chat's notices and the text sent to it wait.
    Scheduled,
    /// The reset command went in at `at`.
    Resetting { at: u64 },
    /// The continuation prompt went in at `at`.
    Prompting { at: u64 },
    /// A core stopped in the middle of the switch; the next looked from `since`.
    Recovering { since: u64 },
}

/// The conversation a handoff leaves: the CLI's id for it (Claude Code's
/// session id, the start of Codex's thread id), and its transcript.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) struct Before {
    pub conversation: Option<String>,
    pub transcript: Option<String>,
}

/// A handoff under way.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) struct Switch {
    pub n: u32,
    pub provider: Provider,
    /// Luna's copy of the file, and its SHA-256.
    pub snapshot: String,
    pub sha256: String,
    /// The file as the session named it.
    pub source: String,
    pub scheduled_at: u64,
    pub phase: Phase,
    pub before: Before,
    /// Resets typed so far.
    #[serde(default)]
    pub attempts: u32,
    /// The instructions the chat was served to write it by (served.rs); none
    /// when it went by the notices alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<super::served::Served>,
}

/// What a look at the chat finds.
#[derive(Clone, Debug, Default)]
pub(super) struct Look {
    pub now: u64,
    /// Its CLI runs.
    pub alive: bool,
    /// Between turns, drawn, nobody typing into it for a while
    /// (notifier `ready_for_notice`).
    pub at_rest: bool,
    /// A person's draft is in its composer.
    pub draft: bool,
    /// A person typed into it a moment ago (delivery.rs SEND_TYPING_MS).
    pub typing: bool,
    /// What its CLI runs of its own (activity `own_jobs`).
    pub jobs: Vec<String>,
    /// The conversation its CLI is on now, as `Before::conversation` says one.
    pub conversation: Option<String>,
    /// A transcript other than the one left holds this handoff's prompt.
    pub prompted: bool,
}

/// What a look calls for.
#[derive(Debug, PartialEq)]
pub(super) enum Step {
    Wait,
    /// Waiting on a person's draft.
    Deferred,
    /// Type the reset command.
    Reset,
    /// Type the continuation prompt.
    Prompt,
    /// Back to `Scheduled`: the reset did not hold across a core's going.
    Retry,
    Done,
    Fail(String),
    /// Before its reset, the chat is on another conversation than the one
    /// that called: the person's own `/clear` or `/new`. Given up, nothing
    /// typed: a reset now would clear a conversation nobody asked to hand
    /// off, and open it with the one before's continuation.
    Left,
}

/// Why a handoff is given up whose chat went on in another conversation
/// before its reset (`Step::Left`).
pub(super) const LEFT: &str = "the chat went on in another conversation (a /clear or /new of the person's) before Luna's reset";

/// What `look` calls for in switch `s`, the chat found at rest on `rest_looks`
/// looks in a row.
pub(super) fn next(s: &Switch, look: &Look, rest_looks: u32) -> Step {
    // The CLI says it is on another conversation: Claude Code's registry, the
    // thread in Codex's title.
    let moved = look.conversation.is_some() && look.conversation != s.before.conversation;
    match s.phase {
        Phase::Scheduled => {
            if !look.alive {
                return if look.now.saturating_sub(s.scheduled_at) > RETURN_MS {
                    Step::Fail("its CLI is not running".into())
                } else {
                    Step::Wait
                };
            }
            if look.draft {
                return Step::Deferred;
            }
            if !look.at_rest {
                return if look.now.saturating_sub(s.scheduled_at) > REST_MAX_MS {
                    Step::Fail(format!("the chat did not come to rest within {} minutes", REST_MAX_MS / 60_000))
                } else {
                    Step::Wait
                };
            }
            if !look.jobs.is_empty() {
                return Step::Fail(format!(
                    "its CLI's own jobs are running ({}), which cannot cross to a new conversation",
                    look.jobs.join(", ")
                ));
            }
            // The conversation that called, the only one the reset may
            // clear; one Luna cannot name is waited for, not for ever.
            match look.conversation.as_deref() {
                Some(now) if s.before.conversation.as_deref() != Some(now) => return Step::Left,
                None if look.now.saturating_sub(s.scheduled_at) > REST_MAX_MS => {
                    return Step::Fail(format!(
                        "Luna could not tell which conversation the chat was on within {} minutes",
                        REST_MAX_MS / 60_000
                    ))
                }
                None => return Step::Wait,
                Some(_) => {}
            }
            if rest_looks < REST_LOOKS {
                Step::Wait
            } else {
                Step::Reset
            }
        }
        Phase::Resetting { at } => {
            if moved {
                // The new conversation is there: its first prompt is Luna's
                // only into a composer nobody is writing in.
                if look.draft || look.typing {
                    Step::Deferred
                } else {
                    Step::Prompt
                }
            } else if look.now.saturating_sub(at) > RESET_WAIT_MS {
                Step::Fail(format!("the CLI did not start a new conversation within {} s of the reset", RESET_WAIT_MS / 1000))
            } else {
                Step::Wait
            }
        }
        Phase::Prompting { at } => {
            if look.prompted {
                Step::Done
            } else if look.now.saturating_sub(at) > PROMPT_WAIT_MS {
                Step::Fail(format!("the new conversation did not take Luna's prompt within {} s", PROMPT_WAIT_MS / 1000))
            } else {
                Step::Wait
            }
        }
        Phase::Recovering { since } => {
            if !look.alive || !look.at_rest {
                return if look.now.saturating_sub(since) > RETURN_MS {
                    Step::Fail("the chat did not come back after Luna's core restarted".into())
                } else {
                    Step::Wait
                };
            }
            if look.prompted {
                Step::Done
            } else if look.draft || look.typing {
                Step::Deferred
            } else if moved {
                Step::Prompt
            } else if s.attempts >= MAX_ATTEMPTS {
                Step::Fail(format!("the reset did not hold in {MAX_ATTEMPTS} tries"))
            } else {
                Step::Retry
            }
        }
    }
}

/// The command that starts a new conversation in the chat's CLI: fixed text,
/// never anything a session wrote.
pub(super) fn reset_command(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "/clear",
        Provider::Codex => "/new",
    }
}

/// What a try at typing came to.
#[derive(Debug, PartialEq)]
pub(super) enum Typed {
    In,
    /// The composer holds something, or a person typed a moment ago: nothing
    /// was committed or typed.
    NotClear,
    /// The phase the text was to go in under could not be taken: nothing was
    /// typed.
    NotCommitted(Refused),
}

/// Why a phase could not be taken.
#[derive(Debug, PartialEq)]
pub(super) enum Refused {
    /// The record holds no longer this handoff in the phase it was read in:
    /// given up meanwhile (auto-handoff turned off), or moved on.
    Gone,
    /// It could not be saved.
    Unsaved(String),
}

/// The chat's terminal, as the switch drives it.
pub(super) trait Term {
    fn look(&self, chat: &str, s: &Switch) -> Look;
    /// The conversation the chat's CLI is on now, as `Look::conversation`
    /// says one: read again with the person's keys held, right before the
    /// reset goes in.
    fn conversation(&self, chat: &str, s: &Switch) -> Option<String>;
    /// Types `text` as a prompt once the chat's composer is seen empty with a
    /// person's keys held back, and `commit` (the phase the text goes in
    /// under) has been taken; the keys stay held from that look to the Enter.
    /// Nothing is typed when either fails.
    fn type_line(&self, chat: &str, text: &str, commit: &dyn Fn() -> Result<(), Refused>) -> Result<Typed, String>;
    /// Between looks.
    fn pause(&self);
}

/// What the switch came to.
#[derive(Debug, PartialEq)]
pub(super) enum Ended {
    Done,
    /// Given up, and whether the reset had gone through by then: the old
    /// conversation is gone, and the new one is to hear what it missed.
    Failed {
        why: String,
        reset: bool,
    },
    /// Given up before its reset: the chat went on in another conversation
    /// (`Step::Left`). Its ask closes with it: the conversation asked is gone.
    Left,
    /// Handed back to the beat: the chat is not ready for the next step, or
    /// the handoff is no longer this run's.
    Back,
}

/// Runs the switch of `chat`'s handoff `n` from where its phase stands to its
/// end, or back to the beat when the chat is not ready: types the reset and the
/// prompt as `next` calls for them. Each goes in only under a phase taken for
/// this very handoff from the phase it was read in, saved before the text
/// (`Phases::advance`): a handoff given up meanwhile types nothing, and a core
/// that stops in between knows to look. `prompt` makes the continuation prompt,
/// as the moment it is typed finds the facts, not the moment the run began:
/// the reset can take a minute. The terminal is the caller's to claim for the
/// whole of it.
pub(super) fn run(term: &dyn Term, chat: &str, n: u32, prompt: &dyn Fn() -> String, phases: &dyn Phases) -> Ended {
    loop {
        let Some(s) = phases.switch_of(chat).filter(|s| s.n == n) else { return Ended::Back };
        let look = term.look(chat, &s);
        let reset_done = matches!(s.phase, Phase::Prompting { .. })
            || (matches!(s.phase, Phase::Resetting { .. } | Phase::Recovering { .. })
                && look.conversation.is_some()
                && look.conversation != s.before.conversation);
        let unsaved =
            |e: String, reset: bool| Ended::Failed { why: format!("its state could not be saved, so nothing more was typed: {e}"), reset };
        match next(&s, &look, REST_LOOKS) {
            Step::Reset => {
                // Looked at again under the held keys: a `/clear` the person
                // sent since the look would have the reset clear the
                // conversation they opened.
                let left = std::cell::Cell::new(false);
                let commit = || {
                    match term.conversation(chat, &s) {
                        Some(now) if s.before.conversation.as_deref() == Some(now.as_str()) => {}
                        Some(_) => {
                            left.set(true);
                            return Err(Refused::Gone);
                        }
                        None => return Err(Refused::Gone),
                    }
                    phases.advance(chat, n, s.phase, Phase::Resetting { at: look.now }, s.attempts + 1)
                };
                match term.type_line(chat, reset_command(s.provider), &commit) {
                    Ok(Typed::In) => {}
                    Ok(Typed::NotCommitted(Refused::Gone)) if left.get() => return Ended::Left,
                    Ok(Typed::NotClear | Typed::NotCommitted(Refused::Gone)) => return Ended::Back,
                    Ok(Typed::NotCommitted(Refused::Unsaved(e))) => return unsaved(e, false),
                    Err(e) => return Ended::Failed { why: format!("the reset could not be typed: {e}"), reset: false },
                }
            }
            Step::Prompt => {
                let commit = || phases.advance(chat, n, s.phase, Phase::Prompting { at: look.now }, s.attempts);
                match term.type_line(chat, &prompt(), &commit) {
                    Ok(Typed::In) => {}
                    // Someone is writing in the new conversation: looked at
                    // again, by the beat, once they are through.
                    Ok(Typed::NotClear) => {
                        let _ = phases.advance(chat, n, s.phase, Phase::Recovering { since: look.now }, s.attempts);
                        return Ended::Back;
                    }
                    Ok(Typed::NotCommitted(Refused::Gone)) => return Ended::Back,
                    Ok(Typed::NotCommitted(Refused::Unsaved(e))) => return unsaved(e, true),
                    Err(e) => return Ended::Failed { why: format!("the prompt could not be typed: {e}"), reset: true },
                }
            }
            Step::Deferred if matches!(s.phase, Phase::Resetting { .. }) => {
                let _ = phases.advance(chat, n, s.phase, Phase::Recovering { since: look.now }, s.attempts);
                return Ended::Back;
            }
            Step::Wait if matches!(s.phase, Phase::Resetting { .. } | Phase::Prompting { .. }) => term.pause(),
            // Not this run's to wait out: the beat looks again, and starts a
            // run once the chat is ready.
            Step::Wait | Step::Deferred => return Ended::Back,
            Step::Retry => {
                let _ = phases.advance(chat, n, s.phase, Phase::Scheduled, s.attempts);
                return Ended::Back;
            }
            Step::Done => return Ended::Done,
            Step::Left => return Ended::Left,
            Step::Fail(why) => return Ended::Failed { why, reset: reset_done },
        }
    }
}

/// Where switches are kept, for `run`: the store, or a test's.
pub(super) trait Phases {
    fn switch_of(&self, chat: &str) -> Option<Switch>;
    /// Moves `chat`'s handoff `n` from phase `from` to `to` with the resets
    /// typed so far, saved before it returns; refused when the record holds
    /// another handoff or this one in another phase, or the save fails.
    fn advance(&self, chat: &str, n: u32, from: Phase, to: Phase, attempts: u32) -> Result<(), Refused>;
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    fn switch(provider: Provider) -> Switch {
        Switch {
            n: 3,
            provider,
            snapshot: "/data/handoffs/c1/3.md".into(),
            sha256: "ab".into(),
            source: "/work/handoff.md".into(),
            scheduled_at: 1_000,
            phase: Phase::Scheduled,
            before: Before { conversation: Some("old".into()), transcript: Some("/t/old.jsonl".into()) },
            attempts: 0,
            instructions: None,
        }
    }

    pub(in crate::agents::handoff) fn rest(now: u64) -> Look {
        Look { now, alive: true, at_rest: true, conversation: Some("old".into()), ..Default::default() }
    }

    #[test]
    fn a_scheduled_handoff_waits_for_rest_and_resets_on_the_second_look() {
        let s = switch(Provider::Claude);
        assert_eq!(next(&s, &rest(2_000), 0), Step::Wait);
        assert_eq!(next(&s, &rest(2_000), 1), Step::Wait, "a turn it queued may follow");
        assert_eq!(next(&s, &rest(2_000), 2), Step::Reset);
        let busy = Look { at_rest: false, ..rest(2_000) };
        assert_eq!(next(&s, &busy, 5), Step::Wait);
        assert!(matches!(next(&s, &Look { now: 1_000 + REST_MAX_MS + 1, ..busy }, 5), Step::Fail(w) if w.contains("come to rest")));
        let gone = Look { alive: false, ..rest(2_000) };
        assert_eq!(next(&s, &gone, 5), Step::Wait, "a CLI starting again after a core restart");
        assert!(matches!(next(&s, &Look { now: 1_000 + RETURN_MS + 1, ..gone }, 5), Step::Fail(w) if w.contains("not running")));
    }

    #[test]
    fn a_draft_defers_the_reset_however_long_it_stands() {
        let s = switch(Provider::Codex);
        let draft = Look { draft: true, ..rest(1_000 + REST_MAX_MS * 10) };
        assert_eq!(next(&s, &draft, 9), Step::Deferred, "the person's: never given up on, never typed over");
        assert_eq!(next(&s, &rest(5_000), 2), Step::Reset, "cleared: on it goes");
    }

    #[test]
    fn jobs_of_the_clis_own_fail_the_switch_with_their_names() {
        let s = switch(Provider::Claude);
        let jobs = Look { jobs: vec!["tail.exe (pid 7)".into(), "1 subagent".into()], ..rest(2_000) };
        match next(&s, &jobs, 2) {
            Step::Fail(why) => assert!(why.contains("tail.exe (pid 7), 1 subagent"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn each_phase_moves_on_what_the_cli_shows_or_fails_past_its_wait() {
        let mut s = switch(Provider::Claude);
        s.phase = Phase::Resetting { at: 10_000 };
        assert_eq!(next(&s, &rest(11_000), 0), Step::Wait);
        let moved = Look { conversation: Some("new".into()), ..rest(11_000) };
        assert_eq!(next(&s, &moved, 0), Step::Prompt);
        assert!(matches!(next(&s, &rest(10_000 + RESET_WAIT_MS + 1), 0), Step::Fail(w) if w.contains("new conversation")));
        s.phase = Phase::Prompting { at: 20_000 };
        assert_eq!(next(&s, &moved, 0), Step::Wait);
        assert_eq!(next(&s, &Look { prompted: true, ..moved.clone() }, 0), Step::Done);
        assert!(matches!(next(&s, &Look { now: 20_000 + PROMPT_WAIT_MS + 1, ..moved }, 0), Step::Fail(w) if w.contains("prompt")));
    }

    #[test]
    fn after_a_core_restart_what_the_chat_is_on_says_resume_or_roll_back() {
        let mut s = switch(Provider::Codex);
        s.phase = Phase::Recovering { since: 50_000 };
        s.attempts = 1;
        // Not back yet: wait for it, but not for ever.
        assert_eq!(next(&s, &Look { alive: false, ..rest(51_000) }, 0), Step::Wait);
        assert!(matches!(next(&s, &Look { alive: false, ..rest(50_000 + RETURN_MS + 1) }, 0), Step::Fail(_)));
        // On the conversation it left: the reset did not hold; once more.
        assert_eq!(next(&s, &rest(51_000), 0), Step::Retry);
        s.attempts = MAX_ATTEMPTS;
        assert!(matches!(next(&s, &rest(51_000), 0), Step::Fail(w) if w.contains("did not hold")));
        // On a new one: the prompt, unless it is in already.
        let moved = Look { conversation: Some("new".into()), ..rest(51_000) };
        assert_eq!(next(&s, &moved, 0), Step::Prompt);
        assert_eq!(next(&s, &Look { prompted: true, ..moved }, 0), Step::Done);
    }

    /// A chat's terminal as a test has it: what typing does to it is the
    /// test's to say.
    pub(in crate::agents::handoff) struct Fake {
        pub look: RefCell<Look>,
        pub typed: RefCell<Vec<String>>,
        on_type: Box<dyn Fn(&str, &mut Look)>,
        clock: RefCell<u64>,
        /// What the composer holds at the look made with the keys held, for
        /// each text in turn: a person's keys landing after the switch's own look.
        pub busy_at_type: RefCell<Vec<bool>>,
        /// Called at each look, before it is made: something happening meanwhile.
        pub on_look: RefCell<Option<Box<dyn FnOnce()>>>,
        /// What the conversation is read as with the keys held, where it is
        /// not the look's: the person's `/clear` landing after the look.
        pub conversation_at_type: RefCell<Vec<Option<String>>>,
    }

    impl Term for Fake {
        fn look(&self, _: &str, _: &Switch) -> Look {
            if let Some(f) = self.on_look.borrow_mut().take() {
                f();
            }
            Look { now: *self.clock.borrow(), ..self.look.borrow().clone() }
        }
        fn conversation(&self, _: &str, _: &Switch) -> Option<String> {
            match self.conversation_at_type.borrow_mut().pop() {
                Some(c) => c,
                None => self.look.borrow().conversation.clone(),
            }
        }
        fn type_line(&self, _: &str, text: &str, commit: &dyn Fn() -> Result<(), Refused>) -> Result<Typed, String> {
            let busy = {
                let mut b = self.busy_at_type.borrow_mut();
                if b.is_empty() {
                    false
                } else {
                    b.remove(0)
                }
            };
            if busy {
                return Ok(Typed::NotClear);
            }
            if let Err(r) = commit() {
                return Ok(Typed::NotCommitted(r));
            }
            self.typed.borrow_mut().push(text.to_string());
            (self.on_type)(text, &mut self.look.borrow_mut());
            Ok(Typed::In)
        }
        fn pause(&self) {
            *self.clock.borrow_mut() += 1_000;
        }
    }

    struct Kept(std::rc::Rc<RefCell<HashMap<String, Switch>>>, RefCell<bool>);

    impl Phases for Kept {
        fn switch_of(&self, chat: &str) -> Option<Switch> {
            self.0.borrow().get(chat).cloned()
        }
        fn advance(&self, chat: &str, n: u32, from: Phase, to: Phase, attempts: u32) -> Result<(), Refused> {
            if *self.1.borrow() {
                return Err(Refused::Unsaved("the disk is full".into()));
            }
            match self.0.borrow_mut().get_mut(chat).filter(|s| s.n == n && s.phase == from) {
                Some(s) => {
                    s.phase = to;
                    s.attempts = attempts;
                    Ok(())
                }
                None => Err(Refused::Gone),
            }
        }
    }

    pub(in crate::agents::handoff) fn fake(on_type: impl Fn(&str, &mut Look) + 'static) -> Fake {
        Fake {
            look: RefCell::new(rest(5_000)),
            typed: RefCell::default(),
            on_type: Box::new(on_type),
            clock: RefCell::new(5_000),
            busy_at_type: RefCell::default(),
            on_look: RefCell::default(),
            conversation_at_type: RefCell::default(),
        }
    }

    fn kept(s: Switch) -> Kept {
        Kept(std::rc::Rc::new(RefCell::new(HashMap::from([("c1".to_string(), s)]))), RefCell::new(false))
    }

    #[test]
    fn a_run_types_the_reset_then_the_prompt_and_ends_once_it_is_in() {
        for provider in [Provider::Claude, Provider::Codex] {
            // The CLI goes on in a new conversation at its reset command; Codex
            // writes its file only with the prompt.
            let t = fake(move |text, l| {
                if text == reset_command(provider) {
                    l.conversation = Some("new".into());
                } else {
                    l.prompted = true;
                }
            });
            let phases = kept(switch(provider));
            assert_eq!(run(&t, "c1", 3, &|| "[Luna handoff 3, …]".into(), &phases), Ended::Done, "{provider:?}");
            assert_eq!(*t.typed.borrow(), [reset_command(provider), "[Luna handoff 3, …]"]);
            assert_eq!(phases.switch_of("c1").unwrap().attempts, 1);
        }
    }

    #[test]
    fn the_prompt_is_made_after_the_reset_as_it_is_typed() {
        // What the facts are when the run begins is not what they are a minute
        // of reset later: the prompt is made at the moment it goes in.
        let made = std::rc::Rc::new(RefCell::new(Vec::<usize>::new()));
        let typed_at = made.clone();
        let t = fake(move |text, l| {
            typed_at.borrow_mut().push(usize::MAX);
            if text == "/clear" {
                l.conversation = Some("new".into());
            } else {
                l.prompted = true;
            }
        });
        let phases = kept(switch(Provider::Claude));
        let log = made.clone();
        let prompt = move || {
            let at = log.borrow().len();
            log.borrow_mut().push(at);
            "made now".to_string()
        };
        assert_eq!(run(&t, "c1", 3, &prompt, &phases), Ended::Done);
        assert_eq!(*t.typed.borrow(), ["/clear", "made now"]);
        assert_eq!(*made.borrow(), [usize::MAX, 1, usize::MAX], "made once, after the reset went in, before it was typed");
    }

    #[test]
    fn a_reset_that_never_shows_fails_with_the_old_conversation_kept() {
        let t = fake(|_, _| {});
        let phases = kept(switch(Provider::Claude));
        match run(&t, "c1", 3, &|| "prompt".into(), &phases) {
            Ended::Failed { why, reset } => {
                assert!(why.contains("did not start a new conversation"), "{why}");
                assert!(!reset, "the old conversation is still the one it is on");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(*t.typed.borrow(), ["/clear"], "no prompt into the old conversation");
    }

    #[test]
    fn a_prompt_that_never_lands_fails_after_the_reset() {
        let t = fake(|text, l| {
            if text == "/new" {
                l.conversation = Some("new".into());
            }
        });
        let phases = kept(switch(Provider::Codex));
        assert!(matches!(run(&t, "c1", 3, &|| "prompt".into(), &phases), Ended::Failed { reset: true, .. }));
    }

    #[test]
    fn a_run_on_a_chat_not_ready_hands_back_without_typing() {
        let t = fake(|_, _| {});
        t.look.borrow_mut().draft = true;
        let phases = kept(switch(Provider::Claude));
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &phases), Ended::Back);
        assert!(t.typed.borrow().is_empty());
    }

    #[test]
    fn a_run_after_a_restart_types_only_what_is_missing() {
        // The core went after the reset: the chat is back on the new
        // conversation, without the prompt.
        let t = fake(|_, l| l.prompted = true);
        t.look.borrow_mut().conversation = Some("new".into());
        let mut s = switch(Provider::Claude);
        s.phase = Phase::Recovering { since: 4_000 };
        s.attempts = 1;
        let phases = kept(s);
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &phases), Ended::Done);
        assert_eq!(*t.typed.borrow(), ["prompt"], "no second reset");
        // Back on the conversation it left: Scheduled again, nothing typed yet.
        let t = fake(|_, _| {});
        let mut s = switch(Provider::Codex);
        s.phase = Phase::Recovering { since: 4_000 };
        s.attempts = 1;
        let phases = kept(s);
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &phases), Ended::Back);
        assert_eq!(phases.switch_of("c1").unwrap().phase, Phase::Scheduled);
        assert!(t.typed.borrow().is_empty());
    }

    #[test]
    fn after_the_reset_a_person_writing_holds_the_prompt_back() {
        // The look before the prompt: someone writes in the new conversation.
        let mut s = switch(Provider::Claude);
        s.phase = Phase::Resetting { at: 10_000 };
        let moved = Look { conversation: Some("new".into()), ..rest(11_000) };
        assert_eq!(next(&s, &Look { draft: true, ..moved.clone() }, 0), Step::Deferred);
        assert_eq!(next(&s, &Look { typing: true, ..moved.clone() }, 0), Step::Deferred);
        s.phase = Phase::Recovering { since: 10_000 };
        assert_eq!(next(&s, &Look { draft: true, ..moved }, 0), Step::Deferred, "in recovery too");
        // Their keys land after that look, before the paste: the composer is
        // looked at again with the keys held, and nothing is typed into it.
        let t = fake(|text, l| {
            if text == "/clear" {
                l.conversation = Some("new".into());
            }
        });
        *t.busy_at_type.borrow_mut() = vec![false, true];
        let phases = kept(switch(Provider::Claude));
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &phases), Ended::Back);
        assert_eq!(*t.typed.borrow(), ["/clear"], "no prompt appended to what they write");
        assert!(matches!(phases.switch_of("c1").unwrap().phase, Phase::Recovering { .. }), "the beat looks again");
        // Once they are through, the prompt goes in.
        let t2 = fake(|_, l| l.prompted = true);
        t2.look.borrow_mut().conversation = Some("new".into());
        assert_eq!(run(&t2, "c1", 3, &|| "prompt".into(), &phases), Ended::Done);
        assert_eq!(*t2.typed.borrow(), ["prompt"]);
    }

    /// The person's `/clear` or `/new` while a handoff waits for rest: the
    /// reset would clear the conversation they opened and open it with the
    /// one before's continuation. Given up, nothing typed, whether the look
    /// sees it or only the read under the held keys does.
    #[test]
    fn a_chat_on_another_conversation_before_the_reset_is_given_up_untouched() {
        let s = switch(Provider::Claude);
        let theirs = Look { conversation: Some("theirs".into()), ..rest(2_000) };
        assert_eq!(next(&s, &theirs, 5), Step::Left);
        let unknown = Look { conversation: None, ..rest(2_000) };
        assert_eq!(next(&s, &unknown, 5), Step::Wait, "not told which: waited for");
        assert!(
            matches!(next(&s, &Look { now: 1_000 + REST_MAX_MS + 1, ..unknown }, 5), Step::Fail(w) if w.contains("which conversation"))
        );
        let t = fake(|_, _| {});
        t.look.borrow_mut().conversation = Some("theirs".into());
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &kept(switch(Provider::Codex))), Ended::Left);
        assert!(t.typed.borrow().is_empty());
        // The look saw the old one; the keys held, it is theirs.
        let t = fake(|_, _| {});
        t.conversation_at_type.borrow_mut().push(Some("theirs".into()));
        let phases = kept(switch(Provider::Claude));
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &phases), Ended::Left);
        assert!(t.typed.borrow().is_empty(), "no /clear into their conversation");
        assert_eq!(phases.switch_of("c1").unwrap().phase, Phase::Scheduled, "no phase taken");
        // Not readable then: nothing typed, back to the beat.
        let t = fake(|_, _| {});
        t.conversation_at_type.borrow_mut().push(None);
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &kept(switch(Provider::Claude))), Ended::Back);
        assert!(t.typed.borrow().is_empty());
    }

    #[test]
    fn nothing_is_typed_under_a_phase_that_could_not_be_taken() {
        // Given up between the look and the reset (auto-handoff turned off):
        // the switch is no longer there to move, and no /clear goes in.
        let t = fake(|_, _| {});
        let phases = kept(switch(Provider::Claude));
        let store = phases.0.clone();
        *t.on_look.borrow_mut() = Some(Box::new(move || {
            store.borrow_mut().remove("c1");
        }));
        assert_eq!(run(&t, "c1", 3, &|| "prompt".into(), &phases), Ended::Back);
        assert!(t.typed.borrow().is_empty());
        // Another handoff's number: not this run's.
        let t = fake(|_, _| {});
        let phases = kept(switch(Provider::Claude));
        assert_eq!(run(&t, "c1", 4, &|| "prompt".into(), &phases), Ended::Back);
        assert!(t.typed.borrow().is_empty());
        // A phase that will not save: nothing typed, given up and said.
        let t = fake(|_, _| {});
        let phases = kept(switch(Provider::Codex));
        *phases.1.borrow_mut() = true;
        match run(&t, "c1", 3, &|| "prompt".into(), &phases) {
            Ended::Failed { why, reset } => assert!(why.contains("could not be saved") && !reset, "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(t.typed.borrow().is_empty());
    }
}
