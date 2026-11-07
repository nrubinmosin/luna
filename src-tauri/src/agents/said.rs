//! What a session said since a point in its transcript (`Said`), and how a
//! tool result or a notice puts it: its report first, a long one in a file.

use crate::agents::notes_dir;
use crate::agents::read::{transcript_seen, Message};
use crate::provider::Provider;

/// How much of a report's first line a notice quotes.
const NOTICE_QUOTE: usize = 150;

/// What the session has said since `cursor` (see `Owed::cursor`), read
/// straight off its transcript, its own messages in order (`own_words`;
/// `stopped`: its turn stopped short); empty when there is no transcript to
/// read (`transcript_found` tells that from saying nothing). A transcript
/// with nothing past the cursor has started over in a file of its own (a
/// resume): the last message there is the answer.
pub(super) fn replies_since(pty: &crate::pty::PtyManager, id: &str, provider: Provider, cursor: u64, stopped: bool) -> Said {
    let (Some(path), moved) = transcript_seen(pty, id) else { return Said::default() };
    let cursor = moved.unwrap_or(cursor);
    let mut messages = messages_at(&path, provider, cursor);
    if messages.is_empty() && cursor > 0 {
        messages = messages_at(&path, provider, 0).into_iter().last().into_iter().collect();
    }
    own_words(messages, stopped)
}

/// `replies_since` for a caller already told of what came before `cursor`:
/// nothing past it is nothing to tell, not a resume to look further back for.
pub(super) fn said_since(pty: &crate::pty::PtyManager, id: &str, provider: Provider, cursor: u64) -> Said {
    let (Some(path), moved) = transcript_seen(pty, id) else { return Said::default() };
    own_words(messages_at(&path, provider, moved.unwrap_or(cursor)), false)
}

pub(super) fn messages_at(path: &std::path::Path, provider: Provider, from: u64) -> Vec<Message> {
    match provider {
        Provider::Claude => crate::claude::session::messages_from(path, from, false).0,
        Provider::Codex => crate::codex::session::messages_from(path, from).0,
    }
}

/// What a session said since a point: its own messages in order, and where
/// the last of its turns that said anything begins among them. Text that
/// reached it mid-turn starts a turn of its own after that one, and the
/// newest turn is where it stands now: its report is the one to lead with.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Said {
    pub(super) texts: Vec<String>,
    /// Index in `texts` of the first message of the last turn that wrote any.
    last_turn: usize,
}

impl Said {
    /// Messages of one turn, or of turns that cannot be told apart.
    pub(super) fn one_turn(texts: Vec<String>) -> Said {
        Said { texts, last_turn: 0 }
    }
}

impl std::ops::Deref for Said {
    type Target = [String];
    fn deref(&self) -> &[String] {
        &self.texts
    }
}

/// The session's own messages of these, in order, with where its last turn
/// begins: a prompt between two of them is a turn's start, the CLI's own too. None while a prompt
/// is the last word: its reply is still to come, and what came before it
/// answered something else. Not so for a turn that `stopped` short (an error, a
/// core's going), which has said all it will: what it got to say is handed
/// over. What the CLI put in itself (a background task's end it takes up) is no
/// prompt, and its transcript reader leaves it out.
fn own_words(messages: Vec<Message>, stopped: bool) -> Said {
    if !stopped && messages.last().is_none_or(|m| m.role != "assistant") {
        return Said::default();
    }
    let mut said = Said::default();
    let mut prompted = false;
    for m in messages {
        if m.role != "assistant" {
            prompted = true;
            continue;
        }
        if prompted || m.after_cli {
            said.last_turn = said.texts.len();
            prompted = false;
        }
        said.texts.push(m.text);
    }
    said
}

/// Where a notice says the answer is: a file holding it, report first
/// (`answer_text`). The notice itself quotes no more than the report's first
/// line (`begins`) — a caller that had a long answer cut read the whole of it
/// back anyway, and a long notice reaches the CLI as a paste.
pub(super) fn answer_file(id: &str, replies: &Said) -> String {
    answer_where(note_file(id, &answer_text(replies)).as_deref())
}

pub(super) fn answer_where(file: Option<&std::path::Path>) -> String {
    match file {
        Some(path) => format!("the answer is in {}, its report first", path.display()),
        None => "luna_read gives the answer".into(),
    }
}

/// `text` in a file for a notice to name, under `notes_dir`. Named by what it
/// holds: a notice a draft keeps back is worked out again at every look, and the
/// same answer is the same file. Two answers are two files however close together
/// they come, as a name of 128 bits of their contents cannot land one on the other.
pub(super) fn note_file(id: &str, text: &str) -> Option<std::path::PathBuf> {
    use sha2::{Digest, Sha256};
    let dir = notes_dir();
    let digest: String = Sha256::digest(text.as_bytes()).iter().take(16).map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("{id}-{digest}.md"));
    if path.is_file() {
        // Named just now, it is not to go with the week's sweep, which goes by
        // when a file was last written.
        let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
        return Some(path);
    }
    std::fs::create_dir_all(&dir).and_then(|()| crate::secret::write_private(&path, text.as_bytes())).ok().map(|()| path)
}

/// A helper's messages as one answer: the report (`report_of`), ahead of the
/// progress lines that came before it; then the last one, a closing word
/// ("that was a stale wakeup, nothing changed") the report must not bury;
/// then the rest, in order, earlier turns' reports among them.
pub(super) fn answer_text(replies: &Said) -> String {
    let longest = report_of(replies);
    let last = replies.len().saturating_sub(1);
    let mut whole = replies[longest].clone();
    if last != longest {
        whole.push_str(&format!("\n\n---\n\nIts last message:\n\n{}", replies[last]));
    }
    let rest: Vec<&str> = (0..replies.len()).filter(|&i| i != longest && i != last).map(|i| replies[i].as_str()).collect();
    if !rest.is_empty() {
        whole.push_str(&format!("\n\n---\n\nEverything else it said, in order:\n\n{}", rest.join("\n\n---\n\n")));
    }
    whole
}

/// Shorter than this, a message is a line or two: progress, a closing word, a
/// short answer. None of them is a report the others lead up to.
pub(super) const SHORT_MESSAGE: usize = 400;

/// Which message is the report: the longest of the last turn that wrote any,
/// unless every one of those is short — a Codex helper writes "still
/// running…" a dozen times before "DONE", the longest of those a progress line
/// — when it is the last, where it ended up. Not the longest of all: a turn on
/// text sent mid-turn answers after the first, and the first's long report
/// would lead with what it has since moved past (a commit it has rebased).
fn report_of(replies: &Said) -> usize {
    let from = replies.last_turn.min(replies.len().saturating_sub(1));
    let longest = (from..replies.len()).max_by_key(|&i| replies[i].chars().count()).unwrap_or(from);
    if replies.get(longest).map_or(0, |r| r.chars().count()) < SHORT_MESSAGE {
        replies.len().saturating_sub(1)
    } else {
        longest
    }
}

/// A report's first line that says anything, on one line and short, without
/// its markdown: the verdict, for a caller deciding whether to open the file.
fn opening_line(report: &str) -> Option<String> {
    first_words(report, NOTICE_QUOTE)
}

/// The most of a helper's task `luna_list` gives: it comes with every row.
pub(super) const TASK_CHARS: usize = 100;

/// The first line of `text` that says anything, as `opening_line` puts it, cut to
/// `most` characters.
pub(super) fn first_words(text: &str, most: usize) -> Option<String> {
    let mut fenced = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let bare = line.trim_start_matches(|c: char| c == '#' || c == '>' || c.is_whitespace());
        let bare = ["- ", "* ", "+ "].iter().find_map(|m| bare.strip_prefix(m)).unwrap_or(bare);
        let bare = bare.replace("**", "").replace('`', "");
        let bare = bare.trim_matches(|c: char| c == '*' || c == '_' || c.is_whitespace());
        let flat = bare.split_whitespace().collect::<Vec<_>>().join(" ");
        let flat: String = flat.chars().filter(|c| !c.is_control()).collect();
        // A rule, a table's dashes, an empty heading.
        if !flat.chars().any(char::is_alphanumeric) {
            continue;
        }
        if flat.chars().count() <= most {
            return Some(flat);
        }
        let head: String = flat.chars().take(most).collect();
        return Some(format!("{}…", head.trim_end()));
    }
    None
}

/// What a notice says of the answer beyond where it is. Not for a turn that
/// stopped short: the first line of what it got to say is no verdict on it.
pub(super) fn begins(replies: &Said) -> String {
    match replies.get(report_of(replies)).and_then(|r| opening_line(r)) {
        Some(line) => format!(". It begins: \"{line}\""),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::read::Message;

    #[test]
    fn an_answer_file_puts_the_report_first_and_the_last_word_next() {
        let read = |line: String| {
            let file = line.trim_start_matches("the answer is in ").trim_end_matches(", its report first").to_string();
            let whole = std::fs::read_to_string(&file).unwrap();
            let _ = std::fs::remove_file(&file);
            whole
        };
        assert_eq!(read(answer_file("q", &Said::one_turn(vec!["Done.".into()]))), "Done.");
        let replies = vec![
            "Let me look.".to_string(),
            format!("REPORT: the long one, with what was done. {}", "Details. ".repeat(50)),
            "Now the tests.".to_string(),
            "Nothing else changed.".to_string(),
        ];
        let whole = read(answer_file("q", &Said::one_turn(replies)));
        assert!(whole.starts_with("REPORT: the long one"), "the report leads: {whole}");
        let last = whole.find("Nothing else changed.").unwrap();
        assert!(last < whole.find("Let me look.").unwrap(), "the last word before the progress lines");
        assert!(whole.find("Let me look.").unwrap() < whole.find("Now the tests.").unwrap(), "the rest in order");
    }

    /// As Codex helper qa-r2 put it: a progress line every so often while its
    /// command ran, then the answer, shorter than most of them.
    #[test]
    fn with_every_message_short_the_last_leads() {
        let replies: Vec<String> = [
            "I’ll run the ping in the foreground and wait for it to finish.",
            "The restart disconnected the shell session. I’m checking the running ping processes so I can wait for completion.",
            "The ping remains active.",
            "I’m waiting for the ping process to exit.",
            "R2 DONE FOURTH\nKIWI",
        ]
        .map(String::from)
        .to_vec();
        let replies = Said::one_turn(replies);
        let whole = answer_text(&replies);
        assert!(whole.starts_with("R2 DONE FOURTH\nKIWI\n\n---\n\nEverything else it said, in order:\n\nI’ll run the ping"), "{whole}");
        assert!(!whole.contains("Its last message"), "it leads already");
        assert_eq!(begins(&replies), ". It begins: \"R2 DONE FOURTH\"");
        // One that is not short is the report, wherever it is.
        let mut with_report = replies.texts.clone();
        with_report.insert(1, format!("Findings: {}", "a finding. ".repeat(40)));
        assert!(answer_text(&Said::one_turn(with_report)).starts_with("Findings:"));
    }

    /// As helper B put it on 2026-11-04: a long report at one commit, then text
    /// queued mid-turn, then a shorter report at the commit it moved on to.
    #[test]
    fn the_last_turn_reports() {
        let m = |role: &str, text: String| Message { role: role.into(), text, after_cli: false };
        let first = format!("The new tip is 5ce3b92d2. {}", "Details. ".repeat(120));
        let second = format!("The new tip is cfeb7186f. {}", "Lanes green. ".repeat(40));
        let said = own_words(
            vec![
                m("user", "Rebase and verify.".into()),
                m("assistant", "Rebasing.".into()),
                m("assistant", first.clone()),
                m("user", "Also add the two ride-alongs.".into()),
                m("assistant", "On it.".into()),
                m("assistant", second.clone()),
                m("assistant", "Nothing pushed.".into()),
            ],
            false,
        );
        assert_eq!(said.last_turn, 2, "the second prompt's turn starts at its first message");
        assert_eq!(said[report_of(&said)], second, "not the longest of all: that one is behind it");
        assert!(begins(&said).starts_with(". It begins: \"The new tip is cfeb7186f."), "the notice quotes it too");
        let whole = answer_text(&said);
        assert!(whole.starts_with("The new tip is cfeb7186f."), "{whole}");
        let (last, older) = (whole.find("Nothing pushed.").unwrap(), whole.find("The new tip is 5ce3b92d2.").unwrap());
        assert!(last < older, "the last word, then the earlier turn among the rest");
        // A last turn of short lines only: its last line leads, not the
        // earlier turn's long report.
        let said = own_words(
            vec![m("assistant", first.clone()), m("user", "Status?".into()), m("assistant", "Still green at 5ce3b92d2.".into())],
            false,
        );
        assert_eq!(said[report_of(&said)], "Still green at 5ce3b92d2.");
        // Stopped on its prompt, with nothing said to it: the turn before stands.
        let said = own_words(vec![m("assistant", first.clone()), m("user", "And now?".into())], true);
        assert_eq!((said.last_turn, said[report_of(&said)].clone()), (0, first.clone()));
        // A turn the CLI started itself, on a background job's end, is one
        // too, though its prompt is not among the messages.
        let mut taken_up = m("assistant", "The lane finished green at 9f3e2a1; nothing else changed.".into());
        taken_up.after_cli = true;
        let said = own_words(vec![m("user", "Verify it.".into()), m("assistant", first), taken_up], false);
        assert_eq!(said[report_of(&said)], "The lane finished green at 9f3e2a1; nothing else changed.");
    }

    #[test]
    fn the_same_answer_is_the_same_file() {
        let path = |replies: &[String]| {
            answer_file("same", &Said::one_turn(replies.to_vec()))
                .trim_start_matches("the answer is in ")
                .trim_end_matches(", its report first")
                .to_string()
        };
        let replies = vec!["The report.".to_string()];
        let (first, again) = (path(&replies), path(&replies));
        assert_eq!(first, again, "a notice a draft holds back writes no new file at every look");
        let other = path(&["Another report.".to_string()]);
        assert_ne!(first, other, "a different answer is another file");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "The report.");
        // Named again, it is as good as new for the week's sweep.
        let week_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 24 * 3600);
        std::fs::File::options().write(true).open(&first).unwrap().set_modified(week_ago).unwrap();
        assert_eq!(path(&replies), first);
        let age = std::fs::metadata(&first).unwrap().modified().unwrap().elapsed().unwrap();
        assert!(age < std::time::Duration::from_secs(60), "{age:?}");
        // Gone meanwhile (the week's sweep), it is written again.
        std::fs::remove_file(&first).unwrap();
        assert_eq!(path(&replies), first);
        assert!(std::path::Path::new(&first).is_file());
        let _ = std::fs::remove_file(&first);
        let _ = std::fs::remove_file(&other);
    }

    #[test]
    fn a_notice_quotes_the_first_line_of_the_report_alone() {
        assert_eq!(opening_line("## **Done**: all `tests` pass\n\nDetails"), Some("Done: all tests pass".into()));
        assert_eq!(opening_line("---\n\n# \n> *Fixed the bug*"), Some("Fixed the bug".into()), "rules and empty headings say nothing");
        assert_eq!(opening_line("- first point\n- second"), Some("first point".into()));
        assert_eq!(opening_line("```rust\nfn main() {}\n```\nAfter the code"), Some("After the code".into()));
        assert_eq!(opening_line("|---|---|\n| a | b |"), Some("| a | b |".into()));
        assert_eq!(opening_line("  \n---\n"), None);
        assert_eq!(opening_line("bell\u{7} in\tit"), Some("bell in it".into()));

        let long = "word ".repeat(100);
        let cut = opening_line(&long).unwrap();
        assert!(cut.ends_with("word…"), "{cut}");
        assert!(cut.chars().count() <= NOTICE_QUOTE + 1, "{}", cut.chars().count());

        let report = format!("# Report\nThe long one, with what was done. {}", "Details. ".repeat(50));
        let replies = Said::one_turn(vec!["Let me look.".to_string(), report, "Nothing else.".to_string()]);
        assert_eq!(begins(&replies), ". It begins: \"Report\"", "the longest message, as the file leads with it");
        assert!(!begins(&replies).contains('\n'));
        assert_eq!(begins(&Said::one_turn(vec!["---".to_string()])), "", "nothing worth quoting, no clause");
        assert_eq!(begins(&Said::default()), "");
    }

    #[test]
    fn answer_files_never_share_a_name() {
        let line = |text: &str| answer_file("same-ms", &Said::one_turn(vec![text.to_string()]));
        let lines: Vec<String> = (0..20).map(|i| line(&format!("answer {i}"))).collect();
        for (i, l) in lines.iter().enumerate() {
            let file = l.trim_start_matches("the answer is in ").trim_end_matches(", its report first");
            assert_eq!(std::fs::read_to_string(file).unwrap(), format!("answer {i}"));
            let _ = std::fs::remove_file(file);
        }
    }
}
