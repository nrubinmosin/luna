//! `luna_guide_get`: the guide a topic at a time. Its topics are the tools'
//! names and a few that are no tool's (notices, restart, helpers), fixed, so a
//! caller names the one it needs rather than guessing at a title's words; with
//! none, a short index. Nobody is asked to read the whole guide.

/// The guide: an opening, then `## ` sections, each the answer to some topics.
pub(super) const GUIDE: &str = include_str!("../luna_guide.md");

/// Each topic `luna_guide_get` takes, and the title of the section of the guide
/// that answers it.
pub(super) const TOPICS: [(&str, &str); 19] = [
    ("luna_session_list", "Sessions"),
    ("luna_session_get", "Sessions"),
    ("luna_session_stop", "Sessions"),
    ("luna_session_delete", "Sessions"),
    ("luna_account_list", "Accounts and models"),
    ("luna_account_get", "Accounts and models"),
    ("luna_session_spawn", "Spawning"),
    ("notices", "Getting the answer"),
    ("luna_session_wait", "Getting the answer"),
    ("luna_report_get", "Getting the answer"),
    ("luna_message_list", "Getting the answer"),
    ("luna_session_send", "Answering and steering"),
    ("luna_session_answer", "Answering and steering"),
    ("restart", "Restart"),
    ("helpers", "Working with helpers"),
    ("luna_run_list", "Working with helpers"),
    ("luna_notification_send", "Working with helpers"),
    ("luna_power_set", "Working with helpers"),
    ("luna_guide_get", "Working with helpers"),
];

/// The topics to read before a first spawn, as the instructions name them.
pub(super) const FIRST_READ: [&str; 2] = ["luna_session_spawn", "notices"];

/// The guide's sections, by title, each with its text (its `## ` line included).
pub(super) fn sections() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    let mut starts: Vec<usize> = GUIDE.match_indices("\n## ").map(|(i, _)| i + 1).collect();
    starts.push(GUIDE.len());
    for pair in starts.windows(2) {
        let text = &GUIDE[pair[0]..pair[1]];
        let title = text.lines().next().unwrap_or_default().trim_start_matches("## ").trim();
        out.push((title, text.trim_end()));
    }
    out
}

/// What `luna_guide_get` says of `topic`: its section; with none, the guide's
/// opening and which topic each section answers.
pub(super) fn guide_text(topic: Option<&str>) -> Result<String, String> {
    let sections = sections();
    let Some(topic) = topic.map(str::trim).filter(|t| !t.is_empty()) else {
        let opening = GUIDE.split("\n## ").next().unwrap_or_default().trim_end();
        let index: Vec<String> = sections
            .iter()
            .map(|(title, _)| {
                let topics: Vec<&str> = TOPICS.iter().filter(|(_, t)| t == title).map(|(topic, _)| *topic).collect();
                format!("{title}: {}", topics.join(", "))
            })
            .collect();
        return Ok(format!(
            "{opening}\n\nTopics, by section: {}. Before your first spawn: {}.",
            index.join("; "),
            FIRST_READ.join(" and ")
        ));
    };
    let title = TOPICS.iter().find(|(t, _)| *t == topic).map(|(_, title)| *title);
    let text = title.and_then(|title| sections.iter().find(|(t, _)| *t == title).map(|(_, text)| *text));
    text.map(str::to_owned).ok_or_else(|| {
        let all: Vec<&str> = TOPICS.iter().map(|(t, _)| *t).collect();
        format!("no topic {topic:?}; the topics: {}", all.join(", "))
    })
}
