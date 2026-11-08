//! What a session said since a point in its transcript (`Said`), and how a
//! tool result or a notice puts it: its report first, a long one in a file.

use crate::agents::delivery::notes_dir;
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
    /// Index in `texts` of the first message of each turn that wrote any, in
    /// order: the last of them is `last_turn`.
    starts: Vec<usize>,
    /// That turn was one its CLI started by itself (a background job's end it
    /// took up), with no prompt of anyone's behind it.
    by_cli: bool,
    /// Beside `texts`: each one its CLI marked a turn's final answer
    /// (`Message::final_answer`).
    finals: Vec<bool>,
}

impl Said {
    /// Messages of one turn, or of turns that cannot be told apart.
    pub(super) fn one_turn(texts: Vec<String>) -> Said {
        Said { texts, last_turn: 0, starts: vec![0], by_cli: false, finals: Vec::new() }
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
        if prompted || m.after_cli || said.texts.is_empty() {
            if !said.texts.is_empty() {
                said.last_turn = said.texts.len();
                said.by_cli = !prompted;
            }
            said.starts.push(said.texts.len());
            prompted = false;
        }
        said.finals.push(m.final_answer);
        said.texts.push(m.text);
    }
    said
}

/// `own_words` of messages a reader read itself, its cursor its own.
pub(super) fn said_of(messages: Vec<Message>) -> Said {
    own_words(messages, false)
}

/// Where a notice says the answer is: a file holding it, report first
/// (`answer_text`). The notice itself quotes no more than the report's first
/// line (`begins`) — a caller that had a long answer cut read the whole of it
/// back anyway, and a long notice reaches the CLI as a paste.
pub(super) fn answer_file(id: &str, replies: &Said) -> String {
    let text = answer_text(replies);
    answer_where(note_file(id, &text).as_deref(), report_span(replies, &text))
}

/// Where the answer is, and with `span` (`report_span`) which lines of the file
/// are the report: a caller that wants only the report reads those.
pub(super) fn answer_where(file: Option<&std::path::Path>, span: Option<(usize, usize)>) -> String {
    match (file, span) {
        (Some(path), Some((report, all))) if report < all => {
            format!("the answer is in {}, its report first (lines 1-{report} of {all})", path.display())
        }
        // The report is all there is: nothing to put first.
        (Some(path), Some(_)) => format!("the answer is in {}", path.display()),
        (Some(path), None) => format!("the answer is in {}, its report first", path.display()),
        (None, _) => "luna_message_list gives the answer".into(),
    }
}

/// How many lines of `text`, an answer file that opens with `replies`'s report
/// (`answer_text`), are the report, and how many it has in all. None with
/// nothing said.
pub(super) fn report_span(replies: &Said, text: &str) -> Option<(usize, usize)> {
    let report = replies.get(report_of(replies))?;
    Some((report.lines().count().max(1), text.lines().count().max(1)))
}

/// The report alone (`report_of`), what `luna_report_get` gives.
pub(super) fn report_text(replies: &Said) -> Option<String> {
    replies.get(report_of(replies)).cloned()
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
/// But a short last turn its CLI started by itself (a background job's end)
/// answers nobody: the report is that of the latest turn before it that wrote a
/// message that is not short, the longest of that turn's — not an older turn's
/// longer one, which the newer has moved past — and the short line goes after
/// it as its last word.
pub(super) fn report_of(replies: &Said) -> usize {
    let (from, to) = report_turn(replies);
    // Its CLI's word for it, where it gives one: that turn's final answer.
    if let Some(at) = final_of(replies, from, to) {
        return at;
    }
    let len = |i: usize| replies[i].chars().count();
    let longest = (from..to).max_by_key(|&i| len(i)).unwrap_or(from);
    if replies.get(longest).is_some_and(|r| r.chars().count() >= SHORT_MESSAGE) || to < replies.len() {
        return longest;
    }
    replies.len().saturating_sub(1)
}

/// The turn the report comes from, as a range of messages: the last that wrote
/// any, unless that one its CLI started by itself and every message of it is
/// short (a background job's end taken up, "nothing else changed"), when the
/// latest turn before it with a message that is not short. Chosen before any
/// CLI's mark is looked at: a wakeup marked final answers nobody.
fn report_turn(replies: &Said) -> (usize, usize) {
    let from = replies.last_turn.min(replies.len().saturating_sub(1));
    let len = |i: usize| replies[i].chars().count();
    let short = (from..replies.len()).all(|i| len(i) < SHORT_MESSAGE);
    if replies.by_cli && short {
        // Each earlier turn as a range of messages, newest first.
        let mut bounds: Vec<usize> = replies.starts.iter().copied().filter(|&s| s < from).collect();
        bounds.push(from);
        if let Some(w) = bounds.windows(2).rev().find(|w| (w[0]..w[1]).any(|i| len(i) >= SHORT_MESSAGE)) {
            return (w[0], w[1]);
        }
    }
    (from, replies.len())
}

/// The last message of `from..to` that its CLI marked final.
fn final_of(replies: &Said, from: usize, to: usize) -> Option<usize> {
    (from..to).rev().find(|&i| replies.finals.get(i).copied().unwrap_or(false))
}

/// Whether `report_of` picked the report by length, with no CLI's mark on any
/// message of the turn it reports from: a guess, which a caller is told of.
pub(super) fn report_guessed(replies: &Said) -> bool {
    let (from, to) = report_turn(replies);
    !replies.is_empty() && final_of(replies, from, to).is_none()
}

/// A report's first line that says anything, on one line and short, without
/// its markdown: the verdict, for a caller deciding whether to open the file.
/// A heading of a few words ("Report", "## Summary") names the report rather
/// than says what it found: the line after it, where one says anything.
fn opening_line(report: &str) -> Option<String> {
    let label = |line: &str| {
        let t = line.trim_start();
        t.starts_with('#') && t.trim_start_matches('#').split_whitespace().count() < HEADING_WORDS
    };
    let body: Vec<&str> = report.lines().filter(|l| !label(l)).collect();
    first_words(&body.join("\n"), NOTICE_QUOTE).or_else(|| first_words(report, NOTICE_QUOTE))
}

/// A heading of fewer words than this is a label (`opening_line`); one of more
/// may be the verdict itself ("## Done: all tests pass").
const HEADING_WORDS: usize = 4;

/// Up to this many characters, all a helper said goes into the notice itself
/// (`inline_answer`), and no file: a "Done." costs its caller no read.
pub(super) const INLINE_ANSWER: usize = 300;

/// All `replies` say, on one line for a notice, when it is short enough to
/// go there whole (`INLINE_ANSWER`): messages apart by " / ".
pub(super) fn inline_answer(replies: &Said) -> Option<String> {
    let flat: Vec<String> = replies.iter().map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty()).collect();
    let line = flat.join(" / ");
    let line: String = line.chars().filter(|c| !c.is_control()).collect();
    (!line.is_empty() && line.chars().count() <= INLINE_ANSWER).then_some(line)
}

/// The most of a helper's task `luna_session_list` gives: it comes with every row.
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
    // Several messages and no CLI's mark on any: which leads was a guess.
    let guessed = if replies.len() > 1 && report_guessed(replies) { " (its report picked by length)" } else { "" };
    match replies.get(report_of(replies)).and_then(|r| opening_line(r)) {
        Some(line) => format!(". It begins: \"{line}\"{guessed}"),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::read::Message;

    #[test]
    fn a_notice_says_which_lines_are_the_report() {
        let replies = Said::one_turn(vec![
            "Let me look.".to_string(),
            format!("REPORT: line one\nline two\nline three {}", "x".repeat(400)),
            "Nothing else changed.".to_string(),
        ]);
        let line = answer_file("span", &replies);
        assert!(line.ends_with(", its report first (lines 1-3 of 15)"), "{line}");
        let file = line.trim_start_matches("the answer is in ").split(", its report first").next().unwrap().to_string();
        let whole = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        let head: Vec<&str> = whole.lines().take(3).collect();
        assert_eq!(head[0], "REPORT: line one");
        assert!(head[2].starts_with("line three"));
        assert_eq!(whole.lines().count(), 15);
        assert_eq!(report_text(&replies).unwrap().lines().count(), 3);
        // A file that is the report alone says nothing of lines, nor of a report first.
        let one = answer_file("span", &Said::one_turn(vec!["Done.".into()]));
        assert!(!one.contains("report first"), "{one}");
        let _ = std::fs::remove_file(one.trim_start_matches("the answer is in "));
        assert_eq!(report_text(&Said::default()), None);
    }

    #[test]
    fn an_answer_file_puts_the_report_first_and_the_last_word_next() {
        let read = |line: String| {
            let file = line.trim_start_matches("the answer is in ").split(", its report first").next().unwrap().to_string();
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
        assert_eq!(begins(&replies), ". It begins: \"R2 DONE FOURTH\" (its report picked by length)");
        // One that is not short is the report, wherever it is.
        let mut with_report = replies.texts.clone();
        with_report.insert(1, format!("Findings: {}", "a finding. ".repeat(40)));
        assert!(answer_text(&Said::one_turn(with_report)).starts_with("Findings:"));
    }

    /// As helper B put it on 2026-11-04: a long report at one commit, then text
    /// queued mid-turn, then a shorter report at the commit it moved on to.
    #[test]
    fn a_final_answer_its_cli_marked_is_the_report_whatever_its_length() {
        let m = |role: &str, text: String, final_answer: bool| Message { role: role.into(), text, after_cli: false, final_answer };
        let progress = format!("Working through the files. {}", "Notes. ".repeat(150));
        let said = own_words(
            vec![
                m("user", "Fix it.".into(), false),
                m("assistant", progress.clone(), false),
                m("assistant", "Fixed: 3 files.".into(), true),
            ],
            false,
        );
        assert_eq!(said[report_of(&said)], "Fixed: 3 files.", "the CLI's final answer, not the longest message");
        assert!(!report_guessed(&said));
        // No mark: the longest, and the caller is told it was picked so.
        let said = own_words(
            vec![m("user", "Fix it.".into(), false), m("assistant", progress.clone(), false), m("assistant", "ok".into(), false)],
            false,
        );
        assert_eq!(said[report_of(&said)], progress);
        assert!(report_guessed(&said));
        // A mark in an earlier turn says nothing of the last one.
        let said = own_words(
            vec![
                m("user", "A?".into(), false),
                m("assistant", "A.".into(), true),
                m("user", "B?".into(), false),
                m("assistant", progress.clone(), false),
            ],
            false,
        );
        assert_eq!(said[report_of(&said)], progress);
        assert!(report_guessed(&said));
    }

    #[test]
    fn the_last_turn_reports() {
        let m = |role: &str, text: String| Message { role: role.into(), text, after_cli: false, final_answer: false };
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
        // As power-fix put it on 2026-11-08: a long report, then a turn its CLI
        // started by itself on a background job's end, short. The report
        // stands; the short line comes next as its last word.
        // The CLI marks its own wakeup's line final as it does any: it answers nobody all the same.
        let cli = |text: &str| Message { role: "assistant".into(), text: text.into(), after_cli: true, final_answer: true };
        let said = own_words(
            vec![
                m("user", "Fix the shutdown.".into()),
                m("assistant", "Reading the crate.".into()),
                m("assistant", first.clone()),
                cli("A leftover wait loop I started has stopped; nothing else changed."),
            ],
            false,
        );
        assert_eq!(said.last_turn, 2, "the CLI's own turn is a turn");
        assert_eq!(said[report_of(&said)], first, "the turn before reports");
        let whole = answer_text(&said);
        assert!(whole.starts_with("The new tip is 5ce3b92d2."), "{whole}");
        assert!(whole.contains("Its last message:\n\nA leftover wait loop"), "{whole}");
        assert!(begins(&said).starts_with(". It begins: \"The new tip is 5ce3b92d2."));
        // Two turns with reports before it: the newer's leads, though the
        // older's is longer — the newer has moved past it.
        let older = format!("The tip is a1. {}", "Older detail. ".repeat(150));
        let newer = format!("The tip is b2. {}", "Newer. ".repeat(120));
        assert!(older.len() > 2000 && (800..1000).contains(&newer.len()));
        let said = own_words(
            vec![
                m("user", "Do A.".into()),
                m("assistant", older.clone()),
                m("user", "Now B.".into()),
                m("assistant", "On B.".into()),
                m("assistant", newer.clone()),
                m("assistant", "B pushed.".into()),
                cli("A leftover job ended."),
            ],
            false,
        );
        assert_eq!(said.starts, vec![0, 1, 4]);
        assert_eq!(said[report_of(&said)], newer, "the latest turn with a report, not the longest of all");
        // A newer turn with nothing long is passed over for the one before it.
        let said =
            own_words(vec![m("assistant", older.clone()), m("user", "And?".into()), m("assistant", "Fine.".into()), cli("Ended.")], false);
        assert_eq!(said[report_of(&said)], older);
        // With nothing long before it, the CLI's short turn still leads.
        let said = own_words(vec![m("assistant", "Done.".into()), cli("A job ended.")], false);
        assert_eq!(said[report_of(&said)], "A job ended.");
        // Stopped on its prompt, with nothing said to it: the turn before stands.
        let said = own_words(vec![m("assistant", first.clone()), m("user", "And now?".into())], true);
        assert_eq!((said.last_turn, said[report_of(&said)].clone()), (0, first.clone()));
        // A turn the CLI started itself, on a background job's end, is one
        // too, though its prompt is not among the messages; short, it leaves
        // the report to the turn before.
        let mut taken_up = m("assistant", "The lane finished green at 9f3e2a1; nothing else changed.".into());
        taken_up.after_cli = true;
        let said = own_words(vec![m("user", "Verify it.".into()), m("assistant", first.clone()), taken_up], false);
        assert_eq!(said.last_turn, 1);
        assert_eq!(said[report_of(&said)], first);
    }

    #[test]
    fn a_codex_prompt_in_tags_starts_a_turn() {
        // Two turns of a Codex helper, each prompted in tags of the caller's own,
        // with what Codex injects around them.
        let line = |role: &str, kind: &str, text: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": role,
                "content": [{ "type": kind, "text": text }] } })
            .to_string()
        };
        let first = format!("REPORT one: {}", "Details. ".repeat(60));
        let lines = [
            line("user", "input_text", "<environment_context>\n<cwd>E:\\p</cwd>\n</environment_context>"),
            line("user", "input_text", "<task>Fix the build</task>"),
            line("assistant", "output_text", &first),
            line("user", "input_text", "<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>"),
            line("user", "input_text", "<task>Now the docs</task>"),
            line("assistant", "output_text", "Docs done."),
        ];
        let path = std::env::temp_dir().join(format!("luna-said-tags-{}.jsonl", std::process::id()));
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        let messages = messages_at(&path, Provider::Codex, 0);
        let _ = std::fs::remove_file(&path);
        let users: Vec<&str> = messages.iter().filter(|m| m.role == "user").map(|m| m.text.as_str()).collect();
        assert_eq!(users, ["<task>Fix the build</task>", "<task>Now the docs</task>"]);
        let said = own_words(messages, false);
        assert_eq!(said.last_turn, 1, "the second prompt starts the turn that answers it");
        assert_eq!(said[report_of(&said)], "Docs done.", "not the longer report of the turn before");
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
        assert!(begins(&replies).starts_with(". It begins: \"The long one, with what was done."), "a label of a heading is passed over");
        assert_eq!(opening_line("## Done: all tests pass\nDetails"), Some("Done: all tests pass".into()), "a heading that says it");
        assert_eq!(opening_line("# Report"), Some("Report".into()), "a heading alone is what there is");
        assert!(begins(&replies).ends_with("(its report picked by length)"), "{}", begins(&replies));
        // Short, all of it goes inline; long, none.
        let short = Said::one_turn(vec!["Done.".into(), "Tests\n  pass.".into()]);
        assert_eq!(inline_answer(&short).as_deref(), Some("Done. / Tests pass."));
        assert_eq!(inline_answer(&replies), None);
        assert_eq!(inline_answer(&Said::default()), None);
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
