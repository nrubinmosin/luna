//! The background helpers a chat's agent runs, as the window lists them and
//! as a delete of the chat weighs them: the core's rows (`background_helpers`,
//! background/rows.rs), which chat each hangs from, what a person reads of
//! its state, and what deleting the chat does to each. Ported from
//! src/features/agents/ (background.store, helperSummary) and
//! chats/helperDeletes.ts, less the moves 0.20 cut.

use serde::Deserialize;
use std::collections::HashMap;

/// A helper that runs with no row of its own.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Helper {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub provider: String,
    /// working, waiting, question, idle...
    pub state: String,
    pub ask: Option<Ask>,
    pub account: String,
    pub model: Option<String>,
    pub effort: String,
    pub started_ms: i64,
    pub turn_started_ms: Option<i64>,
    /// Between turns, what its turns left running: what Stop ends then.
    pub left_running: Vec<String>,
    pub last_activity_ms: Option<i64>,
    /// The first line of what it was spawned on.
    pub task: Option<String>,
}

/// What a session waits on a person for.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Ask {
    pub kind: String,
    pub questions: Vec<Question>,
    pub tool: Option<String>,
    pub detail: Option<String>,
}

#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Question {
    pub text: String,
}

impl Helper {
    pub fn label(&self) -> &str {
        if self.name.is_empty() { &self.id } else { &self.name }
    }

    /// It waits on an answer to go on.
    pub fn asks(&self) -> bool {
        self.state == "question" || self.state == "waiting"
    }

    /// Whether a turn runs, or it waits on an answer: Open as chat and Delete
    /// wait for it to end.
    pub fn busy(&self) -> bool {
        self.state == "working" || self.state == "starting" || self.asks()
    }

    /// The list's two tabs; None for one that is gone.
    pub fn tab(&self) -> Option<Tab> {
        match self.state.as_str() {
            "starting" | "working" | "background" | "question" | "waiting" => Some(Tab::Working),
            "idle" => Some(Tab::Idle),
            _ => None,
        }
    }

    /// "sonnet · medium · team".
    pub fn setup_line(&self) -> String {
        [self.model.clone().unwrap_or_else(|| "default model".into()), self.effort.clone(), self.account.clone()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// "working 4m" on its turn, "done · up 12m" after it.
    pub fn state_line(&self, now: i64) -> String {
        let label = state_label(&self.state);
        match self.turn_started_ms {
            Some(t) if t != 0 => format!("{label} {}", since(t, now)),
            _ => format!("{label} · up {}", since(self.started_ms, now)),
        }
    }

    /// "active 2m ago", off the last thing Luna saw it do.
    pub fn activity_line(&self, now: i64) -> Option<String> {
        self.last_activity_ms.filter(|t| *t != 0).map(|t| format!("active {} ago", since(t, now)))
    }

    /// What a helper asks, on one line.
    pub fn ask_line(&self) -> Option<String> {
        let a = self.ask.as_ref()?;
        let line = if a.kind == "permission" {
            [a.tool.clone(), a.detail.clone()].into_iter().flatten().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(": ")
        } else {
            a.questions.iter().map(|q| q.text.clone()).collect::<Vec<_>>().join(" · ")
        };
        (!line.is_empty()).then_some(line)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Working,
    Idle,
}

/// A state as a person reads it.
pub fn state_label(state: &str) -> &str {
    match state {
        "starting" => "starting",
        "working" => "working",
        "waiting" => "asks for permission",
        "question" => "asks a question",
        "idle" => "done",
        "background" => "a job still runs",
        "exited" => "gone",
        other => other,
    }
}

/// How long since `at`: "40s", "4m", "1h 12m".
pub fn since(at: i64, now: i64) -> String {
    let s = ((now - at).max(0) as f64 / 1000.0).round() as i64;
    if s < 60 {
        return format!("{s}s");
    }
    let m = s / 60;
    if m < 60 {
        return format!("{m}m");
    }
    let (h, rest) = (m / 60, m % 60);
    if rest > 0 { format!("{h}h {rest}m") } else { format!("{h}h") }
}

/// The helpers a tab lists. Working lists the asking ones first; Idle also
/// lists the gone, which nothing counts but the list still offers to delete.
pub fn in_tab(helpers: &[Helper], tab: Tab) -> Vec<&Helper> {
    match tab {
        Tab::Working => {
            let working = || helpers.iter().filter(|h| h.tab() == Some(Tab::Working));
            working().filter(|h| h.asks()).chain(working().filter(|h| !h.asks())).collect()
        }
        Tab::Idle => helpers.iter().filter(|h| h.tab() != Some(Tab::Working)).collect(),
    }
}

/// The tab a list opens on: Working while anything works, else Idle.
pub fn default_tab(helpers: &[Helper]) -> Tab {
    if helpers.iter().any(|h| h.tab() == Some(Tab::Working)) { Tab::Working } else { Tab::Idle }
}

/// What a helper hangs from: its parent, or up through helpers (which have
/// no rows) to the first that is not one. None when the line ends with no
/// parent at all.
fn root_in<'a>(h: &'a Helper, by_id: &HashMap<&'a str, &'a Helper>) -> Option<&'a str> {
    let mut parent = h.parent.as_deref();
    // The core caps a line at a few generations; this only guards a loop.
    for _ in 0..16 {
        let Some(p) = parent else { break };
        match by_id.get(p) {
            Some(up) => parent = up.parent.as_deref(),
            None => return Some(p),
        }
    }
    parent
}

/// The helpers hanging from chat `parent`, its helpers' helpers with them.
pub fn helpers_under(all: &[Helper], parent: &str) -> Vec<Helper> {
    let by_id: HashMap<&str, &Helper> = all.iter().map(|h| (h.id.as_str(), h)).collect();
    all.iter().filter(|h| root_in(h, &by_id) == Some(parent)).cloned().collect()
}

/// What deleting a helper's chat does to it: what it is doing and what its
/// worktree holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub id: String,
    pub name: String,
    /// What a delete would cut off, in words; None when it is done.
    pub working: Option<String>,
}

/// What the core says a helper has at stake in a delete
/// (`background_at_stake`).
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct AtStake {
    pub id: String,
    pub working: Option<String>,
}

/// A state as a person reads it, for one that is at work.
fn working_word(state: &str) -> Option<&'static str> {
    match state {
        "working" => Some("working"),
        "waiting" => Some("asks for permission"),
        "question" => Some("asks a question"),
        "background" => Some("a job it started still runs"),
        _ => None,
    }
}

/// Each helper with what it has at stake, from its row and the core's look
/// (`stake`, empty until it is in).
pub fn lines(helpers: &[Helper], stake: &HashMap<String, AtStake>) -> Vec<Line> {
    helpers
        .iter()
        .map(|h| {
            let s = stake.get(&h.id);
            Line {
                id: h.id.clone(),
                name: h.label().to_owned(),
                working: s.and_then(|s| s.working.clone()).or_else(|| working_word(&h.state).map(str::to_owned)),
            }
        })
        .collect()
}

/// What the person ticked.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Choices {
    /// Delete the chat's helpers at all.
    pub with_helpers: bool,
    /// Stop and delete the ones still working too.
    pub stop_working: bool,
}

/// One helper's delete, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct Deletion {
    pub id: String,
    pub stop_working: bool,
}

/// The helpers to delete and how; the rest are left to go by themselves once
/// idle. A helper at work is stopped only with a yes of its own (the core
/// holds the same line, agents/teardown.rs `delete_by_user`). Only what the
/// dialog listed (`shown`): a helper spawned under the chat since was never
/// shown.
pub fn deletions(lines: &[Line], c: Choices, shown: &[String]) -> Vec<Deletion> {
    if !c.with_helpers {
        return Vec::new();
    }
    lines
        .iter()
        .filter(|l| shown.contains(&l.id))
        .filter(|l| l.working.is_none() || c.stop_working)
        .map(|l| Deletion { id: l.id.clone(), stop_working: l.working.is_some() && c.stop_working })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper(id: &str, parent: Option<&str>, state: &str) -> Helper {
        Helper { id: id.into(), parent: parent.map(str::to_owned), state: state.into(), name: id.into(), ..Helper::default() }
    }

    #[test]
    fn a_helpers_helper_hangs_from_the_chat_at_the_top() {
        let all = [
            helper("h1", Some("chat-a"), "idle"),
            helper("h2", Some("h1"), "working"),
            helper("h3", Some("chat-b"), "idle"),
            helper("h4", None, "idle"),
        ];
        let ids = |chat| helpers_under(&all, chat).into_iter().map(|h| h.id).collect::<Vec<_>>();
        assert_eq!(ids("chat-a"), ["h1", "h2"]);
        assert_eq!(ids("chat-b"), ["h3"]);
        assert!(ids("chat-c").is_empty());
    }

    #[test]
    fn the_tabs_list_the_asking_first_and_the_gone_with_the_idle() {
        let all = [helper("a", None, "working"), helper("b", None, "question"), helper("c", None, "idle"), helper("d", None, "exited")];
        let names = |t| in_tab(&all, t).into_iter().map(|h| h.id.clone()).collect::<Vec<_>>();
        assert_eq!(names(Tab::Working), ["b", "a"]);
        assert_eq!(names(Tab::Idle), ["c", "d"]);
        assert_eq!(default_tab(&all), Tab::Working);
        assert_eq!(default_tab(&all[2..]), Tab::Idle);
        assert!(all[0].busy() && all[1].busy() && !all[2].busy());
    }

    #[test]
    fn times_read_as_the_page_read_them() {
        assert_eq!(since(0, 40_000), "40s");
        assert_eq!(since(0, 4 * 60_000), "4m");
        assert_eq!(since(0, 72 * 60_000), "1h 12m");
        assert_eq!(since(0, 120 * 60_000), "2h");
        let mut h = helper("a", None, "working");
        h.turn_started_ms = Some(1_000);
        assert_eq!(h.state_line(241_000), "working 4m");
        h.turn_started_ms = None;
        h.state = "idle".into();
        assert_eq!(h.state_line(720_000), "done · up 12m");
    }

    #[test]
    fn a_delete_stops_the_working_only_when_told() {
        let busy = helper("busy", Some("c"), "working");
        let done = helper("done", Some("c"), "idle");
        let stake = HashMap::from([("done".to_owned(), AtStake { id: "done".into(), working: None })]);
        let l = lines(&[busy, done], &stake);
        assert_eq!(l[0].working.as_deref(), Some("working"));
        assert_eq!(l[1].working, None);
        let shown = vec!["busy".to_owned(), "done".to_owned()];
        // Nothing ticked: no helper goes.
        assert!(deletions(&l, Choices::default(), &shown).is_empty());
        // Helpers alone: the one at work is left to finish.
        let c = Choices { with_helpers: true, ..Choices::default() };
        let d = deletions(&l, c, &shown);
        assert_eq!(d, [Deletion { id: "done".into(), stop_working: false }]);
        // Stopping it is a yes of its own.
        let c = Choices { with_helpers: true, stop_working: true };
        let d = deletions(&l, c, &shown);
        assert_eq!(d.len(), 2);
        assert!(d[0].stop_working && !d[1].stop_working);
        // A helper spawned since was never shown.
        assert_eq!(deletions(&l, c, &["done".to_owned()]).len(), 1);
    }
}
