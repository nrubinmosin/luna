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

/// Read all newly written words, including earlier turns, with the cursor
/// after the last complete line the reader saw.
pub(super) fn written_range(pty: &crate::pty::PtyManager, id: &str, provider: Provider, cursor: u64) -> (Said, u64) {
    let (Some(path), moved) = transcript_seen(pty, id) else { return (Said::default(), cursor) };
    let cursor = moved.unwrap_or(cursor);
    // The general readers rewind if an offset is past EOF. A notice must
    // never take that rewind for fresh text.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() < cursor) {
        return (Said::default(), cursor);
    }
    let (messages, end) = read_at(&path, provider, cursor);
    if end < cursor {
        return (Said::default(), cursor);
    }
    (own_words(messages, true), end)
}

pub(super) fn messages_at(path: &std::path::Path, provider: Provider, from: u64) -> Vec<Message> {
    read_at(path, provider, from).0
}

fn read_at(path: &std::path::Path, provider: Provider, from: u64) -> (Vec<Message>, u64) {
    match provider {
        Provider::Claude => crate::claude::session::messages_from(path, from, false),
        Provider::Codex => crate::codex::session::messages_from(path, from),
    }
}

/// What a session said since a point: its own messages in order, and where
/// the last of its turns that said anything begins among them. A joined
/// prompt can change which request the words answer within a turn.
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
    /// The prompts among them, each with the index in `texts` of the first
    /// message after it: which of the caller's requests a turn answers is read
    /// off these (`asked`).
    prompts: Vec<(usize, String)>,
    /// The prompt index of each turn's first input, or None when the read
    /// starts mid-turn or the CLI started a turn on its own.
    turn_prompts: Vec<Option<usize>>,
    /// The caller's requests these answer, in the order they went in, and beside
    /// `texts` the one each message answers, by index into them (`asked`).
    requests: Vec<Request>,
    re: Vec<usize>,
    matches: Vec<Option<usize>>,
    prior: Option<u32>,
}

/// A request of a caller's that a debt covers: its number among the caller's
/// requests to that session (a spawn 1, each send one more), the `requestKey`
/// it came with, and how its text began, to find the turn it started by.
#[derive(Debug, Clone, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct Request {
    pub(super) n: u32,
    pub(super) key: Option<String>,
    pub(super) head: String,
    #[serde(default = "reply_by_default")]
    pub(super) reply: bool,
    #[serde(default)]
    pub(super) taken: bool,
    #[serde(default)]
    pub(super) told: bool,
    #[serde(default)]
    pub(super) baseline: u32,
    #[serde(default)]
    pub(super) cursor: u64,
    #[serde(default)]
    pub(super) queued: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) delivery: Option<String>,
}

/// How much of a request's text is kept to find it again among the prompts:
/// enough to tell two apart, short of where a CLI might fold or note it.
const REQUEST_HEAD: usize = 60;

/// The start of a request's text, as `Request::head` keeps it.
pub(super) fn request_head(text: &str) -> String {
    flat(text).chars().take(REQUEST_HEAD).collect()
}

/// Text on one line, its runs of whitespace one space.
fn flat(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn reply_by_default() -> bool {
    true
}

impl Request {
    pub(super) fn new(n: u32, key: Option<String>, head: String, reply: bool) -> Self {
        Self { n, key, head, reply, taken: false, told: false, baseline: 0, cursor: 0, queued: false, delivery: None }
    }

    /// The heading used in answer files and tool replies.
    pub(super) fn name(&self) -> String {
        match &self.key {
            Some(key) => format!("request {} (requestKey {key:?})", self.n),
            None => format!("request {}", self.n),
        }
    }
}

impl Said {
    /// Messages of one turn, or of turns that cannot be told apart.
    pub(super) fn one_turn(texts: Vec<String>) -> Said {
        Said { texts, last_turn: 0, starts: vec![0], ..Said::default() }
    }

    /// These messages as answers to requests, with no known match before the read.
    #[cfg(test)]
    pub(super) fn asked(self, requests: Vec<Request>) -> Said {
        self.asked_after(requests, None)
    }

    /// Match prompts in order, with the last request in a batch taking its
    /// reply. For text before the first match, prefer the match from a
    /// previous notice or wait, if that request is still open.
    pub(super) fn asked_after(mut self, requests: Vec<Request>, previous: Option<u32>) -> Said {
        self.re.clear();
        self.prior = previous;
        if !requests.is_empty() {
            let mut from = 0;
            let mut last_head = "";

            let found: Vec<Option<usize>> = requests
                .iter()
                .map(|q| {
                    if previous == Some(q.n) {
                        return None;
                    }
                    // One prompt can batch distinct sends. Repeating the
                    // same request text needs a second occurrence.
                    let start = from + usize::from(last_head == q.head && !last_head.is_empty());
                    let at = self.prompts.iter().skip(start).position(|(_, text)| {
                        !q.head.is_empty() && !text.trim_start().starts_with(super::notifier::NOTICE_TAG) && flat(text).contains(&q.head)
                    });
                    let at = at.map(|i| i + start);
                    if let Some(i) = at {
                        from = i;
                        last_head = &q.head;
                    }
                    at
                })
                .collect();
            let prior = previous.and_then(|n| requests.iter().position(|q| q.n == n)).or_else(|| (requests.len() == 1).then_some(0));
            let before_first = prior.or_else(|| found.iter().position(Option::is_some).map(|i| i.saturating_sub(1)));
            if let Some(before_first) = before_first {
                self.re = (0..self.texts.len())
                    .map(|i| {
                        (0..requests.len()).rev().find(|&k| found[k].is_some_and(|at| self.prompts[at].0 <= i)).unwrap_or(before_first)
                    })
                    .collect();
            }
            self.matches = found;
        }
        self.requests = requests;
        self
    }

    /// The prompt match used for notices excludes the display fallback for
    /// words before an unmatched request. In particular, queueing a request
    /// gives the preceding turn no authority to report status for it.
    pub(super) fn notice_requests(&self) -> (Vec<Request>, bool, bool) {
        let mut requests = self.requests.clone();
        let mut current = self.prior.and_then(|n| requests.iter().position(|q| q.n == n));
        if let Some(k) = current {
            requests[k].taken = true;
        } else {
            current = requests.iter().rposition(|q| q.taken);
        }
        let mut plan = false;
        let mut lasting_in_last_turn = false;
        let mut matches = self.matches.iter().enumerate().filter_map(|(k, at)| Some((k, self.prompts[(*at)?].0))).peekable();
        for i in 0..=self.texts.len() {
            while let Some((k, _)) = matches.next_if(|(_, at)| *at == i) {
                requests[k].taken = true;
                current = Some(k);
            }
            let Some(status) = self.texts.get(i).and_then(|text| status_word(text)) else { continue };
            if let Some(k) = current {
                requests[k].told |= status.lasting();
            }
            if i >= self.last_turn {
                lasting_in_last_turn |= status.lasting();
                plan |= status == Status::Plan && (requests.is_empty() || current.is_some_and(|k| requests[k].reply));
            }
        }
        // An FYI can follow an independent turn before the next look. It
        // leaves a plan asleep only if every observed turn was an FYI.
        let fyi = !self.turn_prompts.is_empty()
            && self.turn_prompts.iter().all(|prompt| {
                prompt.is_some() && self.matches.iter().rposition(|matched| matched == prompt).is_some_and(|k| !requests[k].reply)
            });
        (requests, plan && !lasting_in_last_turn, fyi)
    }

    /// The requests these answer, in the order they were asked.
    pub(super) fn requests(&self) -> &[Request] {
        &self.requests
    }

    /// The request message `i` answers, where that is known.
    pub(super) fn re_of(&self, i: usize) -> Option<&Request> {
        self.requests.get(*self.re.get(i)?)
    }

    /// The request the final run of messages belongs to.
    pub(super) fn last_request(&self) -> Option<u32> {
        self.re_of(self.texts.len().checked_sub(1)?).map(|q| q.n)
    }

    /// Messages `from..to` alone, their turns as they were.
    fn part(&self, from: usize, to: usize) -> Said {
        let mut starts: Vec<usize> = self.starts.iter().filter(|&&s| s > from && s < to).map(|s| s - from).collect();
        starts.insert(0, 0);
        Said {
            texts: self.texts[from..to].to_vec(),
            last_turn: starts.last().copied().unwrap_or(0),
            starts,
            finals: self.finals.get(from..to).map(<[bool]>::to_vec).unwrap_or_default(),
            ..Said::default()
        }
    }

    /// Runs of messages that answer one request each, newest first, each with
    /// its request: none unless they answer several and whose is which is known.
    fn by_request(&self) -> Vec<(&Request, Said)> {
        if self.requests().len() <= 1 || self.re.len() != self.texts.len() {
            return Vec::new();
        }
        let mut runs: Vec<(usize, usize, usize)> = Vec::new();
        for (i, &k) in self.re.iter().enumerate() {
            match runs.last_mut() {
                Some((_, to, last)) if *last == k => *to = i + 1,
                _ => runs.push((i, i + 1, k)),
            }
        }
        runs.into_iter().rev().map(|(from, to, k)| (&self.requests[k], self.part(from, to))).collect()
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
    let mut starter = None;
    for m in messages {
        if m.role != "assistant" {
            if !m.joined {
                starter.get_or_insert(said.prompts.len());
            }
            said.prompts.push((said.texts.len(), m.text));
            continue;
        }
        if starter.is_some() || m.after_cli || said.texts.is_empty() {
            if !said.texts.is_empty() {
                said.last_turn = said.texts.len();
                said.by_cli = starter.is_none();
            }
            said.starts.push(said.texts.len());
            said.turn_prompts.push(starter.take());
        }
        said.finals.push(m.final_answer);
        said.texts.push(m.text);
    }
    if starter.is_some() {
        said.turn_prompts.push(starter);
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
    answer_where(note_file(id, &text).as_deref(), report_span(replies))
}

/// Where the answer is and which lines hold the report.
pub(super) fn answer_where(file: Option<&std::path::Path>, lines: Option<usize>) -> String {
    match (file, lines) {
        (Some(path), Some(lines)) => format!("Report {}:1–{lines}", path.display()),
        (Some(path), None) => format!("File {}", path.display()),
        (None, _) => "Answer: luna_message_list".into(),
    }
}

/// How many lines of the answer file hold the report, including the request
/// heading and blank line when the file opens with a request section.
pub(super) fn report_span(replies: &Said) -> Option<usize> {
    if let Some((_, part)) = replies.by_request().first() {
        let report = part.get(report_of(part))?;
        return Some(report.lines().count().max(1) + 2);
    }
    let report = replies.get(report_of(replies))?;
    Some(report.lines().count().max(1))
}

/// The report alone (`report_of`), what `luna_report_get` gives; where the
/// messages answer several requests, opened by the one it answers, as its
/// section in `answer_text` is.
pub(super) fn report_text(replies: &Said) -> Option<String> {
    let report = replies.get(report_of(replies))?;
    Some(match report_re(replies).filter(|_| !replies.by_request().is_empty()) {
        Some(q) => format!("Re {}:\n\n{report}", q.name()),
        None => report.clone(),
    })
}

/// The request the report answers (`report_of`), where it is known.
pub(super) fn report_re(replies: &Said) -> Option<&Request> {
    replies.re_of(report_of(replies))
}

/// The opening words that can wake a caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(super) enum Status {
    Done,
    Blocked,
    Question,
    Failed,
    Plan,
}

impl Status {
    /// A plan covers its own turn. Other statuses cover the rest of the task.
    pub(super) fn lasting(self) -> bool {
        self != Self::Plan
    }
}

const STATUS_WORDS: [(&str, Status); 5] = [
    ("DONE", Status::Done),
    ("BLOCKED", Status::Blocked),
    ("QUESTION", Status::Question),
    ("FAILED", Status::Failed),
    ("PLAN", Status::Plan),
];

/// Whether `text`'s first line opens with a status word (STATUS_WORDS) as
/// written, case and all, alone or followed by a colon or a space; markdown's
/// emphasis or heading marks around it do not count against it.
pub(super) fn says_status(text: &str) -> bool {
    status_word(text).is_some()
}

fn status_word(text: &str) -> Option<Status> {
    let first = text.trim_start().lines().next().unwrap_or("");
    let bare = first.trim_start_matches(|c: char| matches!(c, '#' | '*' | '_') || c.is_whitespace());
    STATUS_WORDS.iter().find_map(|(word, status)| {
        let rest = bare.strip_prefix(word)?.trim_start_matches(['*', '_']);
        (rest.is_empty() || rest.starts_with(':') || rest.starts_with(' ')).then_some(*status)
    })
}

/// Prefer the latest lasting status over a plan in the same batch of messages.
pub(super) fn fresh_status(said: &Said) -> Option<Status> {
    let mut found = said.iter().rev().filter_map(|text| status_word(text));
    let latest = found.next()?;
    if latest.lasting() {
        Some(latest)
    } else {
        found.find(|status| status.lasting()).or(Some(latest))
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
    let path = dir.join(format!("{}-{digest}.md", crate::paths::id_file_name(id)));
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
    // Answers to several requests: a section each, the newest first, opened
    // by the request it answers.
    let parts = replies.by_request();
    if !parts.is_empty() {
        let sections: Vec<String> = parts.iter().map(|(q, part)| format!("Re {}:\n\n{}", q.name(), answer_text(part))).collect();
        return sections.join("\n\n===\n\n");
    }
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
        t.starts_with('#') && !says_status(t) && t.trim_start_matches('#').split_whitespace().count() < HEADING_WORDS
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

/// The most of a helper's task `luna_session_get` gives.
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
/// Whether the report was picked by length is the reads' to say
/// (`reportGuessed`); the file holds every message either way.
pub(super) fn begins(replies: &Said) -> String {
    let sections = replies.by_request();
    let first = sections.first().map_or(replies, |(_, part)| part);
    match first.get(report_of(first)).and_then(|r| opening_line(r)) {
        Some(line) => format!(" \"{line}\""),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::read::Message;

    #[test]
    fn the_last_request_in_a_batch_owns_the_reply() {
        let request = |n, head: &str| Request::new(n, None, head.into(), true);
        let queued =
            ["check the warnings", "check the tests"].map(|text| crate::background::Queued::new(Some("caller".into()), text.into()));
        let message = |role: &str, text: String| Message { role: role.into(), text, after_cli: false, joined: false, final_answer: false };
        let said = said_of(vec![message("user", crate::background::joined(&queued)), message("assistant", "Both checks passed.".into())])
            .asked(vec![request(2, "check the warnings"), request(3, "check the tests")]);
        assert_eq!(said.last_request(), Some(3));
        assert!(answer_text(&said).starts_with("Re request 3:"));
    }

    #[test]
    fn a_status_word_opens_the_first_line_as_written() {
        for yes in [
            "DONE",
            "DONE: all green",
            "BLOCKED on review",
            "QUESTION:",
            "FAILED",
            "PLAN: two lanes",
            "## DONE",
            "**BLOCKED**: x",
            "  DONE\nmore",
        ] {
            assert!(says_status(yes), "{yes}");
        }
        for no in ["done", "Done: x", "DONEZO", "Notes\nDONE", "Status: DONE", "PLANNING", ""] {
            assert!(!says_status(no), "{no}");
        }
    }

    #[test]
    fn a_plan_in_a_later_turn_is_independent_of_a_status_in_an_earlier_turn() {
        let m =
            |role: &str, text: &str| Message { role: role.into(), text: text.into(), after_cli: false, joined: false, final_answer: false };
        let read = |last| {
            said_of(vec![
                m("user", "First task"),
                m("assistant", "DONE: first task"),
                m("user", &format!("{} says Next task is queued", super::super::notifier::NOTICE_TAG)),
                m("assistant", "BLOCKED: another session is blocked"),
                m("user", "Next task"),
                m("assistant", last),
            ])
            .asked(vec![Request::new(1, None, "First task".into(), true), Request::new(2, None, "Next task".into(), true)])
        };
        let (requests, plan, _) = read("PLAN: check the logs").notice_requests();
        assert!(plan && requests[0].told && !requests[1].told);
        let (requests, plan, _) = read("> DONE: status from another session").notice_requests();
        assert!(!plan && !requests[1].told);
    }

    #[test]
    fn an_fyi_after_an_independent_turn_is_not_the_only_work() {
        let m =
            |role: &str, text: &str| Message { role: role.into(), text: text.into(), after_cli: false, joined: false, final_answer: false };
        let fyi = [m("user", "Heads up"), m("assistant", "Noted.")];
        let match_fyi = |messages| said_of(messages).asked(vec![Request::new(1, None, "Heads up".into(), false)]).notice_requests().2;
        assert!(match_fyi(fyi.to_vec()));
        let independent = [m("user", "Go ahead"), m("assistant", "I reviewed the change.")];
        assert!(!match_fyi([independent, fyi].concat()));
    }

    #[test]
    fn a_plan_covers_the_turn_when_an_fyi_joins_after_it() {
        use serde_json::json;
        for provider in [Provider::Claude, Provider::Codex] {
            let m = |role, text| match provider {
                Provider::Claude => json!({"type": role, "message": {"content": text}}),
                Provider::Codex => json!({"type": "response_item", "payload": {"type": "message", "role": role,
                    "content": [{"type": if role == "user" { "input_text" } else { "output_text" }, "text": text}]}}),
            };
            let start = json!({"type": "event_msg", "payload": {"type": "task_started"}});
            let joined = match provider {
                Provider::Claude => json!({"type": "attachment", "attachment": {"type": "queued_command", "prompt": "/tmp contains logs"}}),
                Provider::Codex => m("user", "/tmp contains logs"),
            };
            let path = std::env::temp_dir().join(format!("luna-joined-plan-{}-{}.jsonl", provider.as_str(), std::process::id()));
            let lines = [start, m("user", "Check the changes"), m("assistant", "PLAN: inspect logs"), joined, m("assistant", "Noted.")];
            let lines: Vec<_> = lines.into_iter().map(|line| line.to_string()).collect();
            std::fs::write(&path, lines.join("\n") + "\n").unwrap();
            let said = said_of(messages_at(&path, provider, 0)).asked(vec![
                Request::new(1, None, "Check the changes".into(), true),
                Request::new(2, None, "/tmp contains logs".into(), false),
            ]);
            let (requests, plan, fyi_only) = said.notice_requests();
            assert!(requests.iter().all(|q| q.taken), "{provider:?}: both prompts are taken");
            assert!(plan && !fyi_only, "{provider:?}: the FYI joined the PLAN's turn");
            assert_eq!(said.last_request(), Some(2), "the joined FYI has its own reply");
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn answers_to_several_requests_are_put_down_to_the_turn_each_started() {
        let m =
            |role: &str, text: &str| Message { role: role.into(), text: text.into(), after_cli: false, joined: false, final_answer: false };
        let q = |n: u32, head: &str| Request::new(n, None, request_head(head), true);
        let said = || {
            said_of(vec![
                m("assistant", "Looking at it."),
                m("assistant", "NO-GO: tests fail."),
                m("user", "[From session x via Luna] Now   the second\none, please"),
                m("assistant", "GO."),
            ])
        };
        let one = said().asked(vec![q(1, "first")]);
        assert!(one.requests().len() == 1 && answer_text(&one).starts_with("GO."), "one request: nothing set apart");
        let two = said().asked(vec![q(1, "the first, spawned"), q(2, "Now the second one, please")]);
        assert_eq!(two.requests().len(), 2);
        assert_eq!(two.re_of(0).map(|q| q.n), Some(1), "before the second's prompt: the first's");
        assert_eq!(two.re_of(2).map(|q| q.n), Some(2));
        assert_eq!(report_re(&two).map(|q| q.n), Some(2));
        let text = answer_text(&two);
        assert!(text.starts_with("Re request 2:\n\nGO.\n\n===\n\nRe request 1:\n\nNO-GO: tests fail."), "{text}");
        assert_eq!(report_text(&two).as_deref(), Some("Re request 2:\n\nGO."));
        assert_eq!(report_span(&two), Some(3), "the heading, a blank, the report");
        // Neither found among the prompts: named, not set apart.
        let lost = said().asked(vec![q(1, "something else"), q(2, "and another")]);
        assert_eq!(lost.requests().len(), 2);
        assert!(lost.re_of(0).is_none() && !answer_text(&lost).starts_with("Re "));
    }

    /// A transcript that holds none of the requests made before (a
    /// conversation that began afresh): what it says there is the newest of
    /// those's, under its number, up to a request sent since.
    #[test]
    fn answers_in_a_fresh_conversation_keep_the_numbers_of_requests_before_it() {
        let m =
            |role: &str, text: &str| Message { role: role.into(), text: text.into(), after_cli: false, joined: false, final_answer: false };
        let q = |n: u32, head: &str| Request::new(n, None, request_head(head), true);
        let said = said_of(vec![
            m("user", "This is a new conversation of session x, a lead."),
            m("assistant", "Picked up where it stood."),
            m("assistant", "Lane 2 is green."),
            m("user", "[From session p via Luna] And the third ask"),
            m("assistant", "Third: done."),
        ])
        .asked(vec![q(1, "the spawn's prompt"), q(2, "Where do you stand?"), q(3, "And the third ask")]);
        let re: Vec<Option<u32>> = (0..said.len()).map(|i| said.re_of(i).map(|q| q.n)).collect();
        assert_eq!(re, [Some(2), Some(2), Some(3)]);
    }

    #[test]
    fn a_notice_says_which_lines_are_the_report() {
        let replies = Said::one_turn(vec![
            "Let me look.".to_string(),
            format!("REPORT: line one\nline two\nline three {}", "x".repeat(400)),
            "Nothing else changed.".to_string(),
        ]);
        let line = answer_file("span", &replies);
        assert!(line.ends_with(":1–3"), "{line}");
        let file = line.trim_start_matches("Report ").rsplit_once(":1–").unwrap().0.to_string();
        let whole = std::fs::read_to_string(&file).unwrap();
        let _ = std::fs::remove_file(&file);
        let head: Vec<&str> = whole.lines().take(3).collect();
        assert_eq!(head[0], "REPORT: line one");
        assert!(head[2].starts_with("line three"));
        assert_eq!(whole.lines().count(), 15);
        assert_eq!(report_text(&replies).unwrap().lines().count(), 3);
        // Even a file consisting of just the report gives its line range.
        let one = answer_file("span", &Said::one_turn(vec!["Done.".into()]));
        assert!(one.ends_with(":1–1"), "{one}");
        let _ = std::fs::remove_file(one.trim_start_matches("Report ").trim_end_matches(":1–1"));
        assert_eq!(report_text(&Said::default()), None);
    }

    #[test]
    fn an_answer_file_puts_the_report_first_and_the_last_word_next() {
        let read = |line: String| {
            let file = line.trim_start_matches("Report ").rsplit_once(":1–").unwrap().0.to_string();
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
        assert_eq!(begins(&replies), " \"R2 DONE FOURTH\"");
        // One that is not short is the report, wherever it is.
        let mut with_report = replies.texts.clone();
        with_report.insert(1, format!("Findings: {}", "a finding. ".repeat(40)));
        assert!(answer_text(&Said::one_turn(with_report)).starts_with("Findings:"));
    }

    /// As helper B put it on 2026-11-04: a long report at one commit, then text
    /// queued mid-turn, then a shorter report at the commit it moved on to.
    #[test]
    fn a_final_answer_its_cli_marked_is_the_report_whatever_its_length() {
        let m = |role: &str, text: String, final_answer: bool| Message {
            role: role.into(),
            text,
            after_cli: false,
            joined: false,
            final_answer,
        };
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
        let m = |role: &str, text: String| Message { role: role.into(), text, after_cli: false, joined: false, final_answer: false };
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
        assert!(begins(&said).starts_with(" \"The new tip is cfeb7186f."), "the notice quotes it too");
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
        let cli = |text: &str| Message { role: "assistant".into(), text: text.into(), after_cli: true, joined: false, final_answer: true };
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
        assert!(begins(&said).starts_with(" \"The new tip is 5ce3b92d2."));
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
            answer_file("same", &Said::one_turn(replies.to_vec())).trim_start_matches("Report ").trim_end_matches(":1–1").to_string()
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
        assert!(begins(&replies).starts_with(" \"The long one, with what was done."), "a label of a heading is passed over");
        assert_eq!(opening_line("## Done: all tests pass\nDetails"), Some("Done: all tests pass".into()), "a heading that says it");
        assert_eq!(opening_line("# DONE: passed\nDetails"), Some("DONE: passed".into()));
        assert_eq!(opening_line("# Report"), Some("Report".into()), "a heading alone is what there is");
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
            let file = l.trim_start_matches("Report ").trim_end_matches(":1–1");
            assert_eq!(std::fs::read_to_string(file).unwrap(), format!("answer {i}"));
            let _ = std::fs::remove_file(file);
        }
    }
}
