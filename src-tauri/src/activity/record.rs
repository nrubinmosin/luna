//! The lifecycle record: one per session, a chat's or a helper's, of either
//! CLI, made of what the CLI itself says. Every view of a session's liveness
//! is read off it — the power rule, `luna_session_wait` and the notices, the
//! rows and their status, the delete warnings, the core's idle exit — and none
//! keeps a verdict of its own.
//!
//! What it says, and nothing it guesses: a turn running, a turn held on a
//! question or a permission, the last turn ended (and how), what the session
//! runs beside its turns (shells, crons, subagents), or that nothing has been
//! read yet. Unknown is active: better a PC left on than work cut off.

use super::Turn;
use serde::Serialize;

/// Where a session's turn stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// A turn runs.
    Running,
    /// A turn, or a subagent of it, is held on a question or a permission
    /// until someone answers.
    Asking,
    /// No turn runs: the last one ended (`Life::outcome`), or none has begun.
    Ended,
    /// Nothing the CLI says has been read yet, or what it says does not read.
    #[default]
    Unknown,
}

/// How the last turn ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Ok,
    /// On an error: an API refusal other than a usage limit, a failed run.
    Failed,
    /// Luna stopped it: `luna_session_stop`, the window's Stop, a send with
    /// `interrupt`. No error of the CLI's, and not finished.
    Stopped,
    /// On a usage limit: nothing goes on before the limit resets.
    UsageLimit,
}

impl Outcome {
    /// A turn's end with the error it ended on, if any, in the CLIs' words.
    pub fn of_error(error: Option<&str>) -> Outcome {
        match error {
            None => Outcome::Ok,
            Some(e) if is_usage_limit(e) => Outcome::UsageLimit,
            Some(_) => Outcome::Failed,
        }
    }
}

/// Claude Code's `rate_limit` (`StopFailure`, an API error line); Codex's
/// limits as Luna words them (`codex::session::rollout_limit`): `usage_limit`,
/// `rate_limit`, `credits_depleted`, or its own `usage_limit_exceeded`.
fn is_usage_limit(error: &str) -> bool {
    error == "rate_limit" || error == "credits_depleted" || error.contains("usage_limit")
}

/// What a session runs beside its turns: each of it is work going on, turn
/// over or not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Background {
    /// Commands left running: Claude Code's background shells (its registry's
    /// `shell`, `Stop`'s `background_tasks`); a Codex chat's or a helper's
    /// processes in its own job.
    pub shells: u32,
    /// Claude Code's session crons (`Stop`'s `session_crons`): turns the CLI
    /// will start by itself.
    pub crons: u32,
    /// Subagents of its own at work.
    pub subagents: u32,
    /// What runs, by name where Luna can tell ("tail.exe (pid 22104)").
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
    /// Its job would not list at the last look: what runs beside its turns is
    /// not known, which counts as something running.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unlisted: bool,
}

impl Background {
    pub fn any(&self) -> bool {
        self.shells > 0 || self.crons > 0 || self.subagents > 0 || self.unlisted
    }

    /// "2 shells, 1 subagent", or "" for nothing.
    pub fn line(&self) -> String {
        let count = |n: u32, one: &str, many: &str| match n {
            0 => None,
            1 => Some(format!("1 {one}")),
            n => Some(format!("{n} {many}")),
        };
        let mut parts: Vec<String> =
            [count(self.shells, "shell", "shells"), count(self.crons, "cron", "crons"), count(self.subagents, "subagent", "subagents")]
                .into_iter()
                .flatten()
                .collect();
        if !self.names.is_empty() {
            parts.push(self.names.join(", "));
        }
        if self.unlisted {
            parts.push("what else runs is not known (its job would not list)".to_string());
        }
        parts.join(", ")
    }
}

/// One session's record.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Life {
    pub phase: Phase,
    /// How the last turn ended, between turns; None before any has.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<Outcome>,
    /// The error the last turn ended on, in the CLI's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// While a turn runs or asks, when it began.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_started_ms: Option<u64>,
    pub background: Background,
    /// Turns ended, across respawns and resumes: what a `wait` counts. It only
    /// grows, apart from whether anything is alive.
    pub turns_ended: u32,
}

impl Life {
    /// Whether anything of the session goes on: a turn, a wait for an answer,
    /// what it runs beside its turns, or a state not known. What keeps the PC
    /// awake and holds a power-off back.
    pub fn active(&self) -> bool {
        self.phase != Phase::Ended || self.background.any()
    }

    /// The turn as the agents' layer has always counted it: not known is not over.
    pub fn turn(&self) -> Turn {
        match self.phase {
            Phase::Running | Phase::Unknown => Turn::Busy,
            Phase::Asking => Turn::Waiting,
            Phase::Ended => Turn::Idle,
        }
    }

    /// The window's status word, as its rows have it: `busy`, `waiting`,
    /// `background` (turn over, work beside it), `idle`.
    pub fn status(&self) -> &'static str {
        match self.phase {
            Phase::Running | Phase::Unknown => "busy",
            Phase::Asking => "waiting",
            Phase::Ended if self.background.any() => "background",
            Phase::Ended => "idle",
        }
    }

    /// What keeps it active, for the log and the power chip, or "" when nothing.
    pub fn why(&self) -> String {
        let mut parts = Vec::new();
        match self.phase {
            Phase::Running => parts.push("mid-turn".to_string()),
            Phase::Asking => parts.push("waiting for an answer".to_string()),
            Phase::Unknown => parts.push("state not known yet".to_string()),
            Phase::Ended => {}
        }
        let bg = self.background.line();
        if !bg.is_empty() {
            parts.push(format!("running {bg}"));
        }
        parts.join(", ")
    }

    /// The usage limit the last turn ended on, if it did.
    pub fn limit(&self) -> Option<&str> {
        (self.phase == Phase::Ended && self.outcome == Some(Outcome::UsageLimit)).then(|| self.error.as_deref().unwrap_or("rate_limit"))
    }
}

/// What a Codex rollout says of the session's turn at one read
/// (`codex::session::Scan`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RolloutRead {
    /// The rollout was found and would not open: it says nothing now.
    pub unreadable: bool,
    /// A turn event was read at all (`task_started`, an end): with none, the
    /// rollout says no more than no rollout does.
    pub seen_turn: bool,
    /// The last turn started and has neither completed nor aborted
    /// (`task_started` / `task_complete`, `turn_aborted`).
    pub working: bool,
    /// When this process's first turn began, by its `task_started` line
    /// (epoch ms).
    pub first_started_ms: Option<u64>,
    /// How long the file is: it growing is activity.
    pub len: u64,
    /// Turns this process of the session has ended.
    pub completed: u32,
    /// Why the last turn stopped short, as the rollout says.
    pub failed: Option<String>,
    /// The usage limit the last turn stopped on (`rollout_limit`).
    pub limit: Option<&'static str>,
    /// A question its model put that holds the turn (`request_user_input`).
    pub asks: bool,
}

/// A Codex session's phase, outcome and error off its rollout's own
/// boundaries. Missing evidence is never idle: no rollout found (it appears
/// only with the first turn, so a chat just started has none), one that will
/// not open, or one with no turn event read yet is not known, and active,
/// until the rollout says otherwise. A turn held on an approval dialog its
/// screen shows (`dialog`, `codex::session::approval_on`) is asking.
pub fn codex_phase(read: Option<&RolloutRead>, dialog: bool) -> (Phase, Option<Outcome>, Option<String>) {
    let Some(r) = read.filter(|r| !r.unreadable && r.seen_turn) else {
        return (Phase::Unknown, None, None);
    };
    if r.working {
        return (if r.asks || dialog { Phase::Asking } else { Phase::Running }, None, None);
    }
    let error = r.limit.map(str::to_owned).or_else(|| r.failed.clone());
    let outcome = match (r.limit, &r.failed) {
        (Some(_), _) => Outcome::UsageLimit,
        (None, Some(f)) => Outcome::of_error(Some(f)),
        (None, None) => Outcome::Ok,
    };
    (Phase::Ended, Some(outcome), error)
}

/// A Claude Code session's phase off its turn as hooks and registry fold it
/// (`fold_claude`), with what an ended turn ended on. Waiting on a dialog an
/// API error left (`api_error`: a usage limit, a lost login) is no question
/// to anyone: the turn is over.
pub fn claude_phase(turn: Option<Turn>, failed: Option<&str>, api_error: Option<&str>) -> (Phase, Option<Outcome>, Option<String>) {
    match turn {
        None => (Phase::Unknown, None, None),
        Some(Turn::Busy) => (Phase::Running, None, None),
        Some(Turn::Waiting) => match api_error.or(failed) {
            Some(e) => (Phase::Ended, Some(Outcome::of_error(Some(e))), Some(e.to_string())),
            None => (Phase::Asking, None, None),
        },
        Some(Turn::Idle) => (Phase::Ended, Some(Outcome::of_error(failed)), failed.map(str::to_owned)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn life(phase: Phase, bg: Background) -> Life {
        Life { phase, background: bg, ..Default::default() }
    }

    #[test]
    fn unknown_is_active_and_ended_with_nothing_beside_it_is_not() {
        assert!(life(Phase::Unknown, Background::default()).active());
        assert!(life(Phase::Running, Background::default()).active());
        assert!(life(Phase::Asking, Background::default()).active(), "a turn held on a question is not over");
        assert!(!life(Phase::Ended, Background::default()).active());
        for bg in [
            Background { shells: 1, ..Default::default() },
            Background { crons: 1, ..Default::default() },
            Background { subagents: 1, ..Default::default() },
        ] {
            let l = life(Phase::Ended, bg);
            assert!(l.active(), "{l:?}");
            assert_eq!(l.status(), "background");
            assert_eq!(l.turn(), Turn::Idle, "its turn is over all the same");
        }
        assert_eq!(life(Phase::Unknown, Background::default()).turn(), Turn::Busy, "not known is not over");
        assert_eq!(Life::default().phase, Phase::Unknown, "a record begins not known");
    }

    #[test]
    fn a_turns_end_says_how() {
        assert_eq!(Outcome::of_error(None), Outcome::Ok);
        assert_eq!(Outcome::of_error(Some("rate_limit")), Outcome::UsageLimit);
        assert_eq!(Outcome::of_error(Some("usage_limit_exceeded")), Outcome::UsageLimit);
        assert_eq!(Outcome::of_error(Some("authentication_failed")), Outcome::Failed);
        let l = Life { phase: Phase::Ended, outcome: Some(Outcome::UsageLimit), error: Some("rate_limit".into()), ..Default::default() };
        assert_eq!(l.limit(), Some("rate_limit"));
        assert!(!l.active(), "a usage limit ends the turn: nothing goes on by itself");
    }

    /// Claude Code: the hooks' and registry's turn, folded, and the error the
    /// last turn ended on.
    #[test]
    fn claude_phases_from_the_folded_turn() {
        assert_eq!(claude_phase(None, None, None).0, Phase::Unknown, "neither hook nor registry yet");
        assert_eq!(claude_phase(Some(Turn::Busy), None, None).0, Phase::Running);
        assert_eq!(claude_phase(Some(Turn::Waiting), None, None).0, Phase::Asking);
        assert_eq!(claude_phase(Some(Turn::Idle), None, None), (Phase::Ended, Some(Outcome::Ok), None));
        assert_eq!(
            claude_phase(Some(Turn::Idle), Some("rate_limit"), None),
            (Phase::Ended, Some(Outcome::UsageLimit), Some("rate_limit".into())),
            "StopFailure"
        );
        assert_eq!(
            claude_phase(Some(Turn::Waiting), None, Some("rate_limit")),
            (Phase::Ended, Some(Outcome::UsageLimit), Some("rate_limit".into())),
            "the limit dialog is up, no StopFailure heard: the transcript's last line says"
        );
        assert_eq!(claude_phase(Some(Turn::Waiting), Some("server_error"), None).1, Some(Outcome::Failed));
    }

    /// Codex: the rollout's own boundaries, and no rollout at all.
    #[test]
    fn codex_phases_from_the_rollout() {
        assert_eq!(codex_phase(None, false), (Phase::Unknown, None, None), "no rollout: not known, whatever went in");
        let unreadable = RolloutRead { unreadable: true, ..Default::default() };
        assert_eq!(codex_phase(Some(&unreadable), false).0, Phase::Unknown, "a rollout that will not open");
        let done_unreadable = RolloutRead { unreadable: true, seen_turn: true, completed: 1, ..Default::default() };
        assert_eq!(codex_phase(Some(&done_unreadable), false).0, Phase::Unknown, "what was read before is no word now");
        assert_eq!(codex_phase(Some(&RolloutRead::default()), false).0, Phase::Unknown, "no turn event read yet");
        let working = RolloutRead { seen_turn: true, working: true, ..Default::default() };
        assert_eq!(codex_phase(Some(&working), false).0, Phase::Running, "task_started, no end yet");
        assert_eq!(codex_phase(Some(&working), true).0, Phase::Asking, "an approval dialog on its screen");
        let asks = RolloutRead { asks: true, ..working.clone() };
        assert_eq!(codex_phase(Some(&asks), false).0, Phase::Asking, "request_user_input holds the turn");
        let done = RolloutRead { seen_turn: true, ..Default::default() };
        assert_eq!(codex_phase(Some(&done), false), (Phase::Ended, Some(Outcome::Ok), None), "task_complete");
        assert_eq!(codex_phase(Some(&done), true).0, Phase::Ended, "a dialog's words left on screen after the turn: no turn");
        let aborted = RolloutRead { failed: Some("interrupted".into()), ..done.clone() };
        assert_eq!(codex_phase(Some(&aborted), false), (Phase::Ended, Some(Outcome::Failed), Some("interrupted".into())));
        let limited = RolloutRead { failed: Some("x".into()), limit: Some("usage_limit_exceeded"), ..done.clone() };
        assert_eq!(codex_phase(Some(&limited), false).1, Some(Outcome::UsageLimit));
    }

    #[test]
    fn why_names_what_keeps_it() {
        let bg = Background { shells: 2, subagents: 1, names: vec!["tail.exe (pid 7)".into()], ..Default::default() };
        assert_eq!(life(Phase::Ended, bg.clone()).why(), "running 2 shells, 1 subagent, tail.exe (pid 7)");
        assert_eq!(life(Phase::Running, Background::default()).why(), "mid-turn");
        assert_eq!(life(Phase::Ended, Background::default()).why(), "");
        let json = serde_json::to_value(life(Phase::Ended, bg)).unwrap();
        assert_eq!(json["phase"], "ended");
        assert_eq!(json["background"]["shells"], 2);
    }
}
