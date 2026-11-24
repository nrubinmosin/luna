//! `luna_guide_get`: the guide a topic at a time. Its topics are the tools'
//! names and a few that are no tool's (notices, restart, helpers), fixed, so a
//! caller names the one it needs rather than guessing at a title's words; with
//! none, a short index. Nobody is asked to read the whole guide. Every session
//! with Luna's tools reads the same guide, but for what only a person's chat
//! may call: a lead's index names no such tool, and its sections drop the
//! lines that do (`guide_text_as`).

use super::tools::PERSONS_ONLY;

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

/// What `luna_guide_get` says of `topic` to a person's chat (`guide_text_as`),
/// as the tests and the context page read it.
#[cfg(test)]
pub(super) fn guide_text(topic: Option<&str>) -> Result<String, String> {
    guide_text_as(topic, true)
}

/// The topic's section, or the offered topics by section with none. Leads only see
/// topics and guidance for tools they may call.
pub(super) fn guide_text_as(topic: Option<&str>, persons: bool) -> Result<String, String> {
    let offered: Vec<&str> = TOPICS.iter().map(|(topic, _)| *topic).filter(|t| persons || !PERSONS_ONLY.contains(t)).collect();
    let sections = sections();
    let Some(topic) = topic.map(str::trim).filter(|t| !t.is_empty()) else {
        let groups: Vec<String> = sections
            .iter()
            .map(|(title, _)| {
                let topics: Vec<&str> =
                    TOPICS.iter().filter(|(topic, section)| section == title && offered.contains(topic)).map(|(topic, _)| *topic).collect();
                format!("{title}: {}", topics.join(", "))
            })
            .collect();
        return Ok(format!("Pick a topic:\n{}", groups.join("\n")));
    };
    let title = TOPICS.iter().find(|(t, _)| *t == topic && offered.contains(t)).map(|(_, title)| *title);
    let text = title.and_then(|title| sections.iter().find(|(t, _)| *t == title).map(|(_, text)| *text));
    let text = text.ok_or_else(|| format!("no topic {topic:?}; the topics: {}", offered.join(", ")))?;
    Ok(if persons { text.to_owned() } else { without_persons_tools(text) })
}

/// A section less each of its bullets (a `- ` line and the indented lines
/// under it) that names a tool only a person's chat may call.
fn without_persons_tools(section: &str) -> String {
    fn keep<'a>(bullet: &mut Vec<&'a str>, out: &mut Vec<&'a str>) {
        if !bullet.iter().any(|line| PERSONS_ONLY.iter().any(|tool| line.contains(tool))) {
            out.append(bullet);
        }
        bullet.clear();
    }
    let mut out: Vec<&str> = Vec::new();
    let mut bullet: Vec<&str> = Vec::new();
    for line in section.lines() {
        if line.starts_with("  ") {
            bullet.push(line);
            continue;
        }
        keep(&mut bullet, &mut out);
        if line.starts_with("- ") {
            bullet.push(line);
        } else {
            out.push(line);
        }
    }
    keep(&mut bullet, &mut out);
    out.join("\n")
}
