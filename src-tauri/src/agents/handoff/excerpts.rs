//! What a document Luna writes for a moved chat (document.rs) reads of the
//! chat's transcript, bounded in what it reads, not only in what it keeps:
//! the start of the file for the first prompt and the conversation's start,
//! and a window at its end for the turns that matter, each line no longer
//! than LINE_MAX (longer ones skipped and counted), the file's size and time
//! looked at before and after so a transcript that changed meanwhile is said
//! to have. Never `messages_from(0)`: both readers keep every message from
//! where they start, and one line can be as long as the CLI wrote it.
//!
//! Said lines are read by the CLIs' own readers (`said_line`), so what is
//! quoted is what `luna_message_list` would give, tool calls and their output
//! never. Beside them, from the same lines, what the document notes for
//! itself and is fragile by nature (shapes no CLI documents, left out
//! silently when they do not match): the last plan proposed (Claude Code's
//! `ExitPlanMode`, Codex's `update_plan`), Claude Code's last compaction
//! summary, and the paths its edit tools were called on.
//!
//! A document built after the chat was asked for its own leaves that ask out:
//! a turn opened by Luna's ask line (`Skip` says which), up to the next prompt, is the
//! handoff attempt, not the work, and neither its words nor what its tools
//! did are read. A window that starts inside a turn, its prompt before it,
//! is inside the ask's turn where the window's first line falls within the
//! move's ask, from its typing to its close (or still open).

use crate::provider::Provider;
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The most read from the start of a transcript.
pub(super) const HEAD_MAX: u64 = 512 * 1024;
/// The most read from its end.
pub(super) const TAIL_MAX: u64 = 16 * 1024 * 1024;
/// A line longer than this is skipped, and counted.
pub(super) const LINE_MAX: usize = 1024 * 1024;
/// The most edit paths kept.
const EDITS_MAX: usize = 100;
/// The most of a plan or a summary kept, in bytes.
pub(super) const PLAN_MAX: usize = 8 * 1024;
pub(super) const SUMMARY_MAX: usize = 16 * 1024;

/// One thing said, as the document quotes it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Said {
    pub role: String,
    pub text: String,
    pub final_answer: bool,
    /// When, as the line has it.
    pub at: Option<String>,
}

/// What was read of a transcript.
#[derive(Clone, Debug, Default)]
pub(super) struct Excerpted {
    /// The first thing a person said: the task as it began.
    pub first: Option<Said>,
    /// What was said in the window at the end, in order.
    pub said: Vec<Said>,
    /// The window did not reach back to the file's start: earlier turns are
    /// not in `said`.
    pub cut_front: bool,
    /// Lines skipped for their length.
    pub skipped: usize,
    /// The file changed while it was read.
    pub changed: bool,
    /// When the conversation began, as its first line has it.
    pub began: Option<String>,
    pub plan: Option<String>,
    pub summary: Option<String>,
    pub edits: Vec<String>,
    /// The skills the conversation loaded, the ask's turns left out.
    pub skills: Skills,
}

/// The skills a conversation loaded, as its transcript records them: Claude
/// Code's `Skill` tool calls, the blocks Codex puts a skill's text in
/// (`<skill>`, docs/cli-compatibility.md). Names only, as the CLI wrote them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Skills {
    /// In order of first use, each once, at most SKILLS_MAX.
    pub names: Vec<String>,
    /// The transcript runs past what was read for them (SKILLS_READ_MAX).
    pub cut: bool,
    /// Codex ran code mode's scripts (`exec`), which are not read for the
    /// files they print (CODEX_SHELLS): a skill read there is not seen.
    pub code_mode: bool,
}

/// What the skills' line says beside "none" where code mode ran
/// (`Skills::code_mode`).
pub(super) const CODE_MODE_UNCOUNTED: &str = "reads through code mode are not counted";

/// The most of a transcript read, from its start, for the skills it loaded:
/// only lines that may name one are parsed, and a skill loaded past it is
/// said to be possible (`Skills::cut`).
pub(super) const SKILLS_READ_MAX: u64 = 256 * 1024 * 1024;
/// The most skill names kept, and the longest name.
const SKILLS_MAX: usize = 50;
const SKILL_NAME_MAX: usize = 128;

/// The turns a reading leaves out: those a prompt `opens` picks, and, for a
/// window that starts mid-turn, the move's last ask as (typed, closed) in ms.
/// `from_call`: such a turn only from its first `luna_session_handoff` call
/// on (`calls_handoff`), for an ask that lets the chat finish its work first;
/// read for the skills alone (`skills_in`), a move's whole turn being its
/// handoff.
#[derive(Clone, Copy)]
pub(super) struct Skip {
    pub opens: fn(&str) -> bool,
    pub ask: Option<(u64, Option<u64>)>,
    pub from_call: bool,
}

impl Skip {
    /// Whether a window whose first line was written at `first` starts inside
    /// the ask's turn: written once the ask was typed and, where the ask has
    /// closed, no later than that. A line past the close may be a turn of the
    /// chat's own, whose prompt lies before the window too: it is read, as
    /// nothing of the chat's work is to go missing.
    fn starts_in_ask(&self, first: Option<u64>) -> bool {
        match (self.ask, first) {
            (Some((typed, closed)), Some(at)) => at >= typed && closed.is_none_or(|c| at <= c),
            _ => false,
        }
    }
}

/// A file's size and time: what tells it changed.
fn stamp(path: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()))
}

/// The complete lines of `bytes`, a chunk read from `start`: a first line cut
/// by the chunk's start left out unless it starts the file, the last left out
/// unless it ends it (`whole_end`). Over-long ones counted, not kept.
fn lines_of(bytes: &[u8], from_start: bool, whole_end: bool, skipped: &mut usize) -> Vec<String> {
    let mut parts: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    if !from_start && !parts.is_empty() {
        parts.remove(0);
    }
    if !whole_end {
        parts.pop();
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        if p.len() > LINE_MAX {
            *skipped += 1;
            continue;
        }
        let line = String::from_utf8_lossy(p);
        let line = line.trim_end_matches('\r');
        if !line.trim().is_empty() {
            out.push(line.to_string());
        }
    }
    out
}

fn read_range(f: &mut std::fs::File, start: u64, len: u64) -> std::io::Result<Vec<u8>> {
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(len as usize);
    f.take(len).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Reads the transcript at `path` of a `provider` chat as the module says,
/// the turns `skip` picks left out.
pub(super) fn read(path: &Path, provider: Provider, skip: Option<Skip>) -> Result<Excerpted, String> {
    read_bounded(path, provider, HEAD_MAX, TAIL_MAX, skip)
}

pub(super) fn read_bounded(path: &Path, provider: Provider, head_max: u64, tail_max: u64, skip: Option<Skip>) -> Result<Excerpted, String> {
    let before = stamp(path).ok_or_else(|| format!("{} cannot be read", path.display()))?;
    let len = before.0;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut skipped = 0;
    let (head, tail, cut_front) = if len <= tail_max {
        let all = read_range(&mut f, 0, len).map_err(|e| e.to_string())?;
        let lines = lines_of(&all, true, true, &mut skipped);
        (Vec::new(), lines, false)
    } else {
        let head = read_range(&mut f, 0, head_max.min(len)).map_err(|e| e.to_string())?;
        let head = lines_of(&head, true, false, &mut skipped);
        // One convention at the cut, which the read before the window
        // (`each_line`) keeps too: a line belongs to the side it starts on.
        // So the window's first bytes are a line of its own only where the
        // byte before them ends one; else they finish one begun before.
        let start = len - tail_max;
        let tail = read_range(&mut f, start - 1, tail_max + 1).map_err(|e| e.to_string())?;
        let whole_first = tail.first() == Some(&b'\n');
        let tail = lines_of(&tail[1..], whole_first, true, &mut skipped);
        (head, tail, true)
    };
    // The skills loaded before the window, read from the file's start through
    // the line the window's start cuts, the ask's turns left out there too;
    // the window's own come with its lines below, the look and whether it
    // ended inside an ask's turn carried over.
    let early = if cut_front { Some(skills_within(&mut f, len - tail_max, provider, skip).map_err(|e| e.to_string())?) } else { None };
    let after = stamp(path);
    let mut out = Excerpted { cut_front, skipped, changed: after != Some(before), ..Default::default() };
    let said = |line: &str| match provider {
        Provider::Claude => crate::claude::session::said_line(line),
        Provider::Codex => crate::codex::session::said_line(line),
    };
    // The start: when it began, and the first thing a person said.
    for line in head.iter().chain(tail.iter().take(if cut_front { 0 } else { usize::MAX })) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if out.began.is_none() {
            out.began = v["timestamp"].as_str().map(str::to_owned);
        }
        if let Some(m) = said(line).filter(|m| m.role == "user" && !skip.is_some_and(|s| (s.opens)(&m.text))) {
            out.first = Some(Said { role: m.role, text: m.text, final_answer: false, at: v["timestamp"].as_str().map(str::to_owned) });
            break;
        }
    }
    let mut edits: Vec<String> = Vec::new();
    // Inside a turn the skipped prompt opened: until the next prompt. A
    // window cut from the front may start inside the ask's turn, its prompt
    // before the window.
    let first_at = tail
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find_map(|v| v["timestamp"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()))
        .map(|t| t.timestamp_millis().max(0) as u64);
    let mut skipping = cut_front && skip.is_some_and(|s| s.starts_in_ask(first_at));
    // The skills go by what the read before the window found of the ask's
    // turn, where it read all of it: it saw the prompt that opened the turn.
    let (mut look, mut skills_skipping, cut) = match early {
        Some(e) if !e.cut => (e.look, e.skipping, false),
        Some(e) => (e.look, skipping, true),
        None => (SkillLook::default(), skipping, false),
    };
    for line in &tail {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = v["timestamp"].as_str().map(str::to_owned);
        let m = said(line);
        if let (Some(skip), Some(m)) = (skip, m.as_ref().filter(|m| m.role == "user")) {
            skipping = (skip.opens)(&m.text);
            skills_skipping = skipping;
        }
        if !skills_skipping {
            look.note(&v, provider);
        }
        if skipping {
            continue;
        }
        match provider {
            Provider::Claude => claude_artifacts(&v, &mut out, &mut edits),
            Provider::Codex => codex_artifacts(&v, &mut out, &mut edits),
        }
        if let Some(m) = m {
            out.said.push(Said { role: m.role, text: m.text, final_answer: m.final_answer, at });
        }
    }
    out.edits = edits;
    out.skills = Skills { names: look.names(), cut, code_mode: look.code_mode };
    Ok(out)
}

/// The skills the conversation in the transcript at `path` loaded, read from
/// its start (`skills_within`), the turns `skip` picks left out: what the
/// next conversation of a handoff is told it had (facts.rs `fact_block`).
pub(super) fn skills_in(path: &Path, provider: Provider, skip: Option<Skip>) -> Result<Skills, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = f.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    let early = skills_within(&mut f, len, provider, skip).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Skills { names: early.look.names(), cut: early.cut, code_mode: early.look.code_mode })
}

/// What a read of a transcript's start for its skills ends on.
struct Early {
    look: SkillLook,
    /// It stopped at SKILLS_READ_MAX, short of where it was to.
    cut: bool,
    /// It ended inside a turn `skip` leaves out.
    skipping: bool,
}

/// The skills named in the first `end` bytes of transcript `f` and the line
/// running past them (`each_line`), the turns `skip` picks left out as
/// `read_bounded` leaves them: from a prompt `skip.opens` to the next, or
/// with `skip.from_call` from the turn's first handoff call to it. Only a
/// line that may name a skill, one that may be a prompt (`may_be_prompt`)
/// where turns are left out, or one that may be that call, is parsed: a
/// transcript is mostly lines that are none of these.
fn skills_within(f: &mut std::fs::File, end: u64, provider: Provider, skip: Option<Skip>) -> std::io::Result<Early> {
    let mut look = SkillLook::default();
    let mut skipping = false;
    // In a turn `skip.opens` picked, its handoff call not seen yet.
    let mut asked = false;
    each_line(f, end, SKILLS_READ_MAX, |text| {
        let prompt = skip.is_some() && may_be_prompt(text, provider);
        let call = asked && text.contains(HANDOFF_TOOL);
        if !prompt && !call && !look.wants(text, provider) {
            return true;
        }
        let Ok(v) = serde_json::from_str::<Value>(text) else { return true };
        if let Some(skip) = skip.filter(|_| prompt) {
            let said = match provider {
                Provider::Claude => crate::claude::session::said_line(text),
                Provider::Codex => crate::codex::session::said_line(text),
            };
            if let Some(m) = said.filter(|m| m.role == "user") {
                let opens = (skip.opens)(&m.text);
                (asked, skipping) = (opens && skip.from_call, opens && !skip.from_call);
            }
        }
        if call && calls_handoff(&v, provider) {
            (asked, skipping) = (false, true);
        }
        if !skipping {
            look.note(&v, provider);
        }
        true
    })?;
    Ok(Early { look, cut: end > SKILLS_READ_MAX, skipping })
}

/// Each line of the first `end` bytes of transcript `f`, and of the line
/// running on past them to its newline, read no further than `max`, a line
/// at a time, each no longer than LINE_MAX (a longer one passed over as it
/// streams by, never held whole) and in UTF-8, until `each` says to stop
/// (false). At `end` a line belongs to the side it starts on, as the window
/// `read_bounded` reads from there has it too: every line starting before
/// `end` is read here, the one `end` cuts whole, and one starting right at
/// `end` is left to the window.
fn each_line(f: &mut std::fs::File, end: u64, max: u64, mut each: impl FnMut(&str) -> bool) -> std::io::Result<()> {
    use std::io::BufRead;
    f.seek(SeekFrom::Start(0))?;
    let stop = end.min(max);
    let room = if end <= max { LINE_MAX as u64 + 1 } else { 0 };
    let mut r = std::io::BufReader::with_capacity(64 * 1024, (&*f).take(stop + room));
    let mut line: Vec<u8> = Vec::new();
    let mut over = false;
    let mut at: u64 = 0;
    let mut scan = |line: &[u8]| std::str::from_utf8(line).map_or(true, &mut each);
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            break;
        }
        let (part, ends) = match buf.iter().position(|b| *b == b'\n') {
            Some(i) => (&buf[..i], true),
            None => (buf, false),
        };
        if !over && line.len() + part.len() <= LINE_MAX {
            line.extend_from_slice(part);
        } else {
            over = true;
            line.clear();
        }
        let used = part.len() + usize::from(ends);
        r.consume(used);
        at += used as u64;
        if ends {
            if !over && !scan(&line) {
                return Ok(());
            }
            line.clear();
            over = false;
            if at >= stop {
                return Ok(());
            }
        }
    }
    if !over && !line.is_empty() {
        scan(&line);
    }
    Ok(())
}

/// Whether transcript line `text` may be a prompt, as `said_line` reads one:
/// a line of the user's role, and in Claude Code's transcript not one with a
/// tool's result, which has that role too but carries no text of its own (in
/// this machine's transcripts, 21,240 such lines and none with any). Those
/// words in a prompt's own text are escaped (`\"tool_result\"`) and match
/// neither, so no prompt is passed over.
fn may_be_prompt(text: &str, provider: Provider) -> bool {
    text.contains("\"user\"") && !(provider == Provider::Claude && text.contains("\"tool_result\""))
}

/// Luna's tool a chat hands off through, as each CLI names it with its MCP
/// server's prefix (`mcp__luna__luna_session_handoff`).
const HANDOFF_TOOL: &str = "luna_session_handoff";

/// Whether transcript line `v` calls the handoff tool: Claude Code's
/// `tool_use` of it, Codex's `function_call` of it or a code-mode script
/// (`exec`) that names it as it calls a tool (`tools.mcp__luna__…(`), there
/// being no other record of a call made from a script.
fn calls_handoff(v: &Value, provider: Provider) -> bool {
    let named = |n: &Value| n.as_str().is_some_and(|n| n.ends_with(HANDOFF_TOOL));
    match provider {
        Provider::Claude => {
            v["type"] == "assistant"
                && v["message"]["content"].as_array().is_some_and(|b| b.iter().any(|b| b["type"] == "tool_use" && named(&b["name"])))
        }
        Provider::Codex => {
            let p = &v["payload"];
            match p["type"].as_str() {
                Some("function_call") => named(&p["name"]),
                Some("custom_tool_call") => {
                    p["name"] == "exec" && p["input"].as_str().is_some_and(|s| s.contains(&format!("{HANDOFF_TOOL}(")))
                }
                _ => false,
            }
        }
    }
}

/// How Claude Code opens the text of a skill it loads (2.1.322, a skill's
/// `getPromptForCommand`), by the `Skill` tool or a person's `/name` alike.
const SKILL_TEXT: &str = "Base directory for this skill: ";

/// The file a skill is.
const SKILL_FILE: &str = "SKILL.md";

/// The most calls kept waiting for their result, and sightings kept.
const PENDING_MAX: usize = 64;
const SEEN_MAX: usize = 4 * SKILLS_MAX;

/// Codex's shell tools, by the name its rollout gives a function call: each
/// runs the one command line its arguments name, and its own result states
/// how that line exited (`exit_status`). A command in any other tool's
/// arguments (a message to an agent, a search) runs nothing, and code mode's
/// script (`exec`) is not read for the commands it runs: a command in a
/// string it only passes on, in a branch it never takes, or in a call it
/// yields before the end of leaves no status of its own.
const CODEX_SHELLS: [&str; 4] = ["shell", "shell_command", "exec_command", "local_shell"];

/// One sighting of a skill loaded: its name, and where it was by reading its
/// SKILL.md, that file's path (`path_key`).
#[derive(Clone, PartialEq)]
struct Seen {
    name: String,
    read: Option<String>,
}

/// What reading a transcript for its skills keeps from line to line: what
/// was seen of skills loaded, in order, and what a later line settles.
#[derive(Default)]
struct SkillLook {
    seen: Vec<Seen>,
    /// Claude Code: the last command line a person typed as `/name`, by its
    /// line's uuid (`slash_skill`).
    command: Option<(String, String)>,
    /// Calls that would load a skill (Claude Code's `Skill` or a `Read` of a
    /// SKILL.md, Codex's command printing one), by call id, until their result
    /// says whether they did.
    pending: Vec<(String, Vec<Seen>)>,
    /// The session's working folder, as its transcript names it (`path_key`).
    cwd: Option<String>,
    /// The SKILL.md files the conversation edited or wrote (`path_key`).
    edited: Vec<String>,
    /// Codex ran a code-mode script (`Skills::code_mode`).
    code_mode: bool,
}

impl SkillLook {
    /// Whether transcript line `text` may say anything of a skill: one that
    /// cannot is not parsed.
    fn wants(&self, text: &str, provider: Provider) -> bool {
        let results = !self.pending.is_empty();
        match provider {
            Provider::Claude => {
                ["\"Skill\"", SKILL_FILE, "<command-name>", SKILL_TEXT].iter().any(|m| text.contains(m))
                    || (results && text.contains("\"tool_result\""))
            }
            Provider::Codex => {
                ["<skill", SKILL_FILE, "\"turn_context\"", "\"session_meta\""].iter().any(|m| text.contains(m))
                    || (results && text.contains("\"function_call_output\""))
                    || (!self.code_mode && text.contains("\"custom_tool_call\""))
            }
        }
    }

    fn see(&mut self, seen: Seen) {
        if self.seen.len() < SEEN_MAX && !self.seen.contains(&seen) {
            self.seen.push(seen);
        }
    }

    fn wait_for(&mut self, id: &str, seen: Vec<Seen>) {
        if seen.is_empty() {
            return;
        }
        if self.pending.len() >= PENDING_MAX {
            self.pending.remove(0);
        }
        self.pending.push((id.to_string(), seen));
    }

    /// The result of call `id` came back: what it would show is seen where it
    /// went through (`clean`).
    fn settle(&mut self, id: &str, clean: bool) {
        let Some(at) = self.pending.iter().position(|(p, _)| p == id) else { return };
        let (_, seen) = self.pending.remove(at);
        if clean {
            seen.into_iter().for_each(|s| self.see(s));
        }
    }

    /// The names seen, each once, in order of first sighting: a SKILL.md read
    /// in the session's own folder that the same conversation edited or
    /// wrote is work on the skill, not the skill loaded, and leaves no name.
    fn names(&self) -> Vec<String> {
        let mine = |p: &str| self.cwd.as_deref().is_some_and(|c| p.starts_with(&format!("{c}/")));
        let worked_on = |s: &Seen| s.read.as_deref().is_some_and(|p| mine(p) && self.edited.iter().any(|e| e == p));
        let mut out: Vec<String> = Vec::new();
        for s in self.seen.iter().filter(|s| !worked_on(s)) {
            let name: String = s.name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            let name = clip(name.trim(), SKILL_NAME_MAX).to_string();
            if !name.is_empty() && out.len() < SKILLS_MAX && !out.contains(&name) {
                out.push(name);
            }
        }
        out
    }

    /// The skills transcript line `v` says were loaded.
    ///
    /// Claude Code, never a subagent's line (`isSidechain`): a `Skill` call,
    /// `input.skill` its name, and a `Read` of a SKILL.md, its folder's name,
    /// each once its result came back and was no error (`is_error`); a skill
    /// a person started as `/name` (`slash_skill`). Its edits of a SKILL.md
    /// (`Edit`, `Write`, `MultiEdit`) are kept for `names`.
    ///
    /// Codex: a skill's text as it puts it in a message, `<skill>`
    /// `<name>…</name>` … `</skill>`, of either role: 0.167.0 recognizes it
    /// among the user's, and 0.167.1's binary names it beside the
    /// developer's. And a SKILL.md a command line run by a shell tool printed
    /// (`skills_read_by`, CODEX_SHELLS): 0.167.2 has the model read a
    /// filesystem skill from the filesystem. Seen once the call's own result
    /// came back stating exit status 0 (`exit_status`): one line may chain
    /// several commands, so any of them failing it leaves all its reads
    /// unseen. A patch's or a writing command's SKILL.md is kept for `names`.
    fn note(&mut self, v: &Value, provider: Provider) {
        match provider {
            Provider::Claude => self.note_claude(v),
            Provider::Codex => self.note_codex(v),
        }
    }

    fn note_claude(&mut self, v: &Value) {
        if v["isSidechain"] == true {
            return;
        }
        if self.cwd.is_none() {
            self.cwd = v["cwd"].as_str().filter(|c| !c.is_empty()).map(|c| path_key(c, None));
        }
        let none = Vec::new();
        let blocks = v["message"]["content"].as_array().unwrap_or(&none);
        match v["type"].as_str() {
            Some("assistant") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
                    let file = b["input"]["file_path"].as_str().or_else(|| b["input"]["notebook_path"].as_str());
                    let seen = match b["name"].as_str() {
                        Some("Skill") => b["input"]["skill"].as_str().map(|n| Seen { name: n.to_string(), read: None }),
                        Some("Read") => {
                            file.and_then(|f| Some(Seen { name: skill_folder(f)?, read: Some(path_key(f, self.cwd.as_deref())) }))
                        }
                        Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => {
                            if let Some(f) = file.filter(|f| skill_folder(f).is_some()) {
                                self.edited.push(path_key(f, self.cwd.as_deref()));
                            }
                            None
                        }
                        _ => None,
                    };
                    if let (Some(seen), Some(id)) = (seen, b["id"].as_str()) {
                        self.wait_for(id, vec![seen]);
                    }
                }
            }
            Some("user") => {
                for b in blocks.iter().filter(|b| b["type"] == "tool_result") {
                    if let Some(id) = b["tool_use_id"].as_str() {
                        self.settle(id, b["is_error"] != true);
                    }
                }
                if let Some(name) = self.slash_skill(v) {
                    self.see(Seen { name, read: None });
                }
            }
            _ => {}
        }
    }

    fn note_codex(&mut self, v: &Value) {
        let p = &v["payload"];
        if matches!(v["type"].as_str(), Some("turn_context" | "session_meta")) {
            if let Some(c) = p["cwd"].as_str().filter(|c| !c.is_empty()) {
                self.cwd = Some(path_key(c, None));
            }
            return;
        }
        if v["type"] != "response_item" {
            return;
        }
        let id = p["call_id"].as_str().unwrap_or("");
        match p["type"].as_str() {
            Some("message") if matches!(p["role"].as_str(), Some("user" | "developer")) => {
                let Some(blocks) = p["content"].as_array() else { return };
                for text in blocks.iter().filter_map(|b| b["text"].as_str()).map(str::trim) {
                    let wrapped = text.strip_prefix("<skill").is_some_and(|r| r.starts_with(['>', ' '])) && text.ends_with("</skill>");
                    let name = text.split_once("<name>").and_then(|(_, r)| r.split_once("</name>")).map(|(n, _)| n);
                    if let Some(name) = name.filter(|_| wrapped) {
                        self.see(Seen { name: name.to_string(), read: None });
                    }
                }
            }
            Some("function_call" | "custom_tool_call" | "local_shell_call") => {
                self.code_mode |= p["type"] == "custom_tool_call" && p["name"] == "exec";
                let input = p["arguments"].as_str().or_else(|| p["input"].as_str()).unwrap_or("");
                // A patch's files, whatever tool carries it (code mode's
                // script calls apply_patch too, its patch in a string).
                for f in patched_files(input).into_iter().filter(|f| skill_folder(f).is_some()) {
                    self.edited.push(path_key(&f, self.cwd.as_deref()));
                }
                let commands = match (p["type"].as_str(), p["name"].as_str()) {
                    (Some("function_call"), Some(name)) if CODEX_SHELLS.contains(&name) => {
                        let args = serde_json::from_str::<Value>(input).unwrap_or(Value::Null);
                        let mut commands = command_segments(&args["command"]);
                        commands.extend(command_segments(&args["cmd"]));
                        commands
                    }
                    (Some("local_shell_call"), _) => command_segments(&p["action"]["command"]),
                    _ => Vec::new(),
                };
                let seen = skills_read_by(&commands)
                    .into_iter()
                    .map(|(name, path)| Seen { name, read: Some(path_key(&path, self.cwd.as_deref())) })
                    .collect();
                for path in skills_written_by(&commands) {
                    self.edited.push(path_key(&path, self.cwd.as_deref()));
                }
                self.wait_for(id, seen);
            }
            // A shell call's result, a local shell's as well: 0.167.2 has no
            // output item of its own for one.
            Some("function_call_output") => {
                let out = &p["output"];
                let text = match out.as_str() {
                    Some(s) => s.to_string(),
                    None => out.as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
                };
                self.settle(id, exit_status(&text) == Some(0));
            }
            _ => {}
        }
    }

    /// A skill a person started by typing `/name` into Claude Code, as
    /// 2.1.322 writes one (read in its binary, `nt`): a line of the
    /// person's, `<command-message>name</command-message>` then
    /// `<command-name>/name</command-name>`, and after it the skill's text, a
    /// line of the CLI's own (`isMeta`) opening with SKILL_TEXT whose
    /// `parentUuid` is that line's uuid. The `Skill` tool's text names its
    /// call (`sourceToolUseID`) instead and follows its result; a built-in
    /// such as `/clear` or `/effort` writes its command line the other way
    /// round, its name first, and is followed by its output
    /// (`<local-command-stdout>`), never by a skill's text. A command line is
    /// kept until the next one; the name, once its skill's text names it as
    /// the parent.
    fn slash_skill(&mut self, v: &Value) -> Option<String> {
        let text = user_text(v);
        if v["isMeta"] != true {
            let named =
                text.trim_start().strip_prefix("<command-message>").and_then(|r| r.split_once("</command-message>")).and_then(|(_, r)| {
                    let (name, _) = r.trim_start().strip_prefix("<command-name>/")?.split_once("</command-name>")?;
                    Some(name.to_string())
                });
            if let (Some(name), Some(uuid)) = (named, v["uuid"].as_str()) {
                self.command = Some((uuid.to_string(), name));
            }
            return None;
        }
        if !v["sourceToolUseID"].is_null() || !text.starts_with(SKILL_TEXT) {
            return None;
        }
        let parent = v["parentUuid"].as_str()?;
        self.command.take_if(|c| c.0 == parent).map(|(_, name)| name)
    }
}

/// A Claude Code user line's text: its content, or its text blocks joined.
fn user_text(v: &Value) -> String {
    let content = &v["message"]["content"];
    match content.as_str() {
        Some(s) => s.to_string(),
        None => content.as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
    }
}

/// The commands a shell call's `command` or `cmd` value `v` runs, each as the
/// words of a segment (`segments`): a string is a shell's script, and parsed
/// as one; an argv (`["cat", "a/SKILL.md"]`) is a program and its arguments,
/// taken as they are, unless it is a shell given a script (`shell_script`),
/// whose script is parsed. No shell parses an argv's own words: a `;` in one
/// is an argument's.
fn command_segments(v: &Value) -> Vec<Vec<String>> {
    match v {
        Value::String(s) => segments(s),
        Value::Array(a) => {
            let words: Vec<String> = a.iter().filter_map(Value::as_str).map(str::to_string).collect();
            match shell_script(&words) {
                Some(script) => segments(&script),
                None if words.is_empty() => Vec::new(),
                None => vec![words],
            }
        }
        _ => Vec::new(),
    }
}

/// The program an argv's first word names, as its file's name: lowercase,
/// no folder and no `.exe` (`/bin/cat`, `C:\…\pwsh.exe`).
fn program(word: &str) -> String {
    let name = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    name.strip_suffix(".exe").map(str::to_string).unwrap_or(name)
}

/// The script argv `words` has a shell run: a POSIX shell's after a flag
/// cluster holding `c` (`bash -lc "<script>"`), PowerShell's words after
/// `-Command` or its `-c`, cmd's after `/c` or `/k`. None for any other.
fn shell_script(words: &[String]) -> Option<String> {
    let (first, args) = words.split_first()?;
    match program(first).as_str() {
        "bash" | "sh" | "zsh" | "dash" | "ksh" => {
            let flags = args.iter().take_while(|a| a.starts_with('-') && !a.starts_with("--"));
            let at = flags.enumerate().find(|(_, a)| a[1..].contains('c')).map(|(i, _)| i)?;
            args.get(at + 1).cloned()
        }
        "pwsh" | "powershell" => {
            let at = args.iter().position(|a| {
                let a = a.to_ascii_lowercase();
                a.len() >= 2 && "-command".starts_with(&a)
            })?;
            Some(args[at + 1..].join(" "))
        }
        "cmd" => {
            let at = args.iter().position(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k"))?;
            Some(args[at + 1..].join(" "))
        }
        _ => None,
    }
}

/// `path` as two paths in one transcript are compared: either slash as `/`,
/// lowercase, no trailing slash, and where relative joined to `cwd` (a
/// command's path is its working folder's). A transcript says where its CLI
/// ran, not what file system Luna reads it on.
fn path_key(path: &str, cwd: Option<&str>) -> String {
    let p = path.trim().replace('\\', "/");
    let absolute = p.starts_with('/') || p.as_bytes().get(1) == Some(&b':');
    let p = match cwd.filter(|_| !absolute) {
        Some(c) => format!("{c}/{}", p.trim_start_matches("./")),
        None => p,
    };
    p.trim_end_matches('/').to_lowercase()
}

/// The exit status a Codex shell call's result `text` states, read from its
/// header alone, the lines before its `Output:` line: `Exit code: N` (the
/// shell tools) or `Process exited with code N` (`exec_command`, whose call
/// still running at its yield says `Process running with session ID …`
/// instead, its end coming in another call's result). The command's own
/// output after the header is never read for one: a SKILL.md that mentions
/// an exit code states none. None where the result has no such header.
fn exit_status(text: &str) -> Option<i64> {
    let mut status = None;
    for line in text.lines() {
        if line.trim_end() == "Output:" {
            return status;
        }
        if let Some(code) = line.strip_prefix("Exit code: ").or_else(|| line.strip_prefix("Process exited with code ")) {
            status = Some(code.trim().parse().ok()?);
        }
    }
    None
}

/// The files a patch in `input` updates or adds (`*** Update File: …`,
/// `*** Add File: …`), as plain text or inside a script's string: each up to
/// the end of its line, an escaped newline or a quote.
fn patched_files(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for head in ["*** Update File: ", "*** Add File: "] {
        for (at, _) in input.match_indices(head) {
            let rest = &input[at + head.len()..];
            let end = ["\n", "\\n", "\"", "'", "`"].iter().filter_map(|e| rest.find(e)).min().unwrap_or(rest.len());
            out.push(rest[..end].trim().to_string());
        }
    }
    out
}

/// The segments of shell command `text`, each as its words: parted at `;`,
/// `|`, `&` and newlines outside quotes, words at whitespace outside quotes,
/// single and double quotes taken off (`"C:\…\SKILL.md"` one word).
fn segments(text: &str) -> Vec<Vec<String>> {
    let (mut all, mut words, mut word) = (Vec::new(), Vec::new(), String::new());
    let (mut quote, mut quoted) = (None, false);
    let end_word = |words: &mut Vec<String>, word: &mut String, quoted: &mut bool| {
        if !word.is_empty() || *quoted {
            words.push(std::mem::take(word));
        }
        *quoted = false;
    };
    for c in text.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    quoted = true;
                }
                ';' | '|' | '&' | '\n' => {
                    end_word(&mut words, &mut word, &mut quoted);
                    if !words.is_empty() {
                        all.push(std::mem::take(&mut words));
                    }
                }
                c if c.is_whitespace() => end_word(&mut words, &mut word, &mut quoted),
                c => word.push(c),
            },
        }
    }
    end_word(&mut words, &mut word, &mut quoted);
    if !words.is_empty() {
        all.push(words);
    }
    all
}

/// The skill whose file `path` is, by its folder's name: `…/<name>/SKILL.md`,
/// either slash, doubled too: a Windows path as a JSON string or a script's
/// literal escapes it (`'C:\\…\\windows-ssh\\SKILL.md'`, seen in Codex 0.167.2's
/// code mode). None for any other file.
fn skill_folder(path: &str) -> Option<String> {
    let file = path.trim().strip_suffix(SKILL_FILE)?;
    let folder = file.trim_end_matches(['/', '\\']);
    if folder.len() == file.len() {
        return None;
    }
    let name = folder.rsplit(['/', '\\']).next()?;
    let plain = !name.is_empty() && name != "." && name != ".." && name.chars().all(|c| c.is_alphanumeric() || "-_.:".contains(c));
    plain.then(|| name.to_string())
}

/// The commands that print a file: one of them reading a SKILL.md is what
/// loading a skill from the filesystem is.
const READERS: [&str; 10] = ["get-content", "gc", "cat", "type", "head", "sed", "nl", "less", "more", "bat"];

/// The commands that write a file.
const WRITERS: [&str; 5] = ["set-content", "add-content", "out-file", "tee", "tee-object"];

/// The SKILL.md paths that a command of `kinds`, the program its segment
/// opens with (`program`), takes as an argument among command segments
/// `commands`: any other command naming one (a search, a listing) is passed
/// over.
fn skill_files_of(commands: &[Vec<String>], kinds: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for words in commands {
        let first = words.first().map(|w| program(w)).unwrap_or_default();
        if !kinds.contains(&first.as_str()) {
            continue;
        }
        for word in &words[1..] {
            let word = word.trim_matches(['(', ')', ',']);
            if skill_folder(word).is_some() && !out.iter().any(|w| w == word) {
                out.push(word.to_string());
            }
        }
    }
    out
}

/// The skills command segments `commands` print the SKILL.md of: each by its
/// folder's name, with the path as written.
fn skills_read_by(commands: &[Vec<String>]) -> Vec<(String, String)> {
    skill_files_of(commands, &READERS).into_iter().filter_map(|p| Some((skill_folder(&p)?, p))).collect()
}

/// The SKILL.md files command segments `commands` write.
fn skills_written_by(commands: &[Vec<String>]) -> Vec<String> {
    skill_files_of(commands, &WRITERS)
}

/// The most of a transcript read for its first turn (`handoffs_first_turn`),
/// on the beat's thread: Luna's prompt and the work of one turn.
const FIRST_TURN_READ_MAX: u64 = 32 * 1024 * 1024;

/// What a conversation Luna's handoff prompt opened says of its first turn
/// (`handoffs_first_turn`).
#[derive(Debug, PartialEq)]
pub(super) struct FirstTurn {
    /// Something has been said to it since that prompt: a person's prompt, a
    /// notice, a draft sent.
    pub over: bool,
    /// The share of its context the turn ended at: the last usage before the
    /// next thing said, where the turn is over and stated one.
    pub ended_pct: Option<u64>,
}

/// The first turn of the conversation in the transcript at `path`, where
/// Luna's handoff prompt opened it: that prompt the first thing said in it,
/// what the CLI writes itself aside (the `/clear` that opened it included).
/// None for any other, or one that cannot be read. Read from the start a line
/// at a time (`each_line`) up to the second thing said and no further, nor
/// past FIRST_TURN_READ_MAX: a first turn running past it is taken as not
/// over. The context at the turn's end by the readings the pane's meter goes
/// by (`context_in_text`).
pub(super) fn handoffs_first_turn(path: &Path, provider: Provider) -> Option<FirstTurn> {
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let (mut opened, mut over, mut other) = (false, false, false);
    let mut last: Option<(f64, f64)> = None;
    let usage = match provider {
        Provider::Claude => "\"usage\"",
        Provider::Codex => "\"token_count\"",
    };
    each_line(&mut f, len, FIRST_TURN_READ_MAX, |text| {
        if text.contains(usage) {
            let read = match provider {
                Provider::Claude => crate::claude::session::context_in_text(text),
                Provider::Codex => crate::codex::session::context_in_text(text),
            };
            if read.is_some() {
                last = read;
            }
        }
        if !may_be_prompt(text, provider) {
            return true;
        }
        let said = match provider {
            Provider::Claude => crate::claude::session::said_line(text),
            Provider::Codex => crate::codex::session::said_line(text),
        };
        let Some(m) = said.filter(|m| m.role == "user") else { return true };
        if opened {
            over = true;
        } else if super::facts::unwrapped(&m.text).starts_with(super::facts::PROMPT_TAG) {
            opened = true;
            last = None;
        } else {
            other = true;
        }
        !(over || other)
    })
    .ok()?;
    let pct = |(tokens, window): (f64, f64)| ((tokens / window).min(1.0) * 100.0).round() as u64;
    (opened && !other).then(|| FirstTurn { over, ended_pct: last.filter(|(t, w)| *t > 0.0 && *w > 0.0).filter(|_| over).map(pct) })
}

/// `text` cut to `max` bytes at a character's edge.
pub(super) fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn note_edit(edits: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() && edits.len() < EDITS_MAX && !edits.iter().any(|e| e == path) {
        edits.push(path.to_string());
    }
}

/// Claude Code: the last `ExitPlanMode` plan, the last compaction summary,
/// and the paths its edit tools were called on.
fn claude_artifacts(v: &Value, out: &mut Excerpted, edits: &mut Vec<String>) {
    if v["isCompactSummary"] == true {
        if let Some(text) = crate::claude::session::content_text(&v["message"]["content"]) {
            out.summary = Some(clip(&text, SUMMARY_MAX).to_string());
        }
        return;
    }
    if v["type"] != "assistant" || v["isSidechain"] == true {
        return;
    }
    let Some(blocks) = v["message"]["content"].as_array() else { return };
    for b in blocks.iter().filter(|b| b["type"] == "tool_use") {
        match b["name"].as_str() {
            Some("ExitPlanMode") => {
                if let Some(plan) = b["input"]["plan"].as_str() {
                    out.plan = Some(clip(plan, PLAN_MAX).to_string());
                }
            }
            Some("Edit" | "Write" | "MultiEdit" | "NotebookEdit") => {
                if let Some(p) = b["input"]["file_path"].as_str().or_else(|| b["input"]["notebook_path"].as_str()) {
                    note_edit(edits, p);
                }
            }
            _ => {}
        }
    }
}

/// Codex: the last `update_plan`, and the files `apply_patch` names.
fn codex_artifacts(v: &Value, out: &mut Excerpted, edits: &mut Vec<String>) {
    if v["type"] != "response_item" {
        return;
    }
    let p = &v["payload"];
    let name = p["name"].as_str();
    // A function call's arguments are a JSON string; a custom tool's input
    // is the patch as text.
    let input = p["arguments"].as_str().or_else(|| p["input"].as_str()).unwrap_or("");
    match (p["type"].as_str(), name) {
        (Some("function_call"), Some("update_plan")) => {
            let Ok(args) = serde_json::from_str::<Value>(input) else { return };
            let Some(steps) = args["plan"].as_array() else { return };
            let mut text = String::new();
            if let Some(why) = args["explanation"].as_str().filter(|s| !s.trim().is_empty()) {
                text.push_str(why.trim());
                text.push('\n');
            }
            for s in steps {
                let (Some(step), status) = (s["step"].as_str(), s["status"].as_str().unwrap_or("?")) else { continue };
                text.push_str(&format!("- [{status}] {step}\n"));
            }
            if !text.is_empty() {
                out.plan = Some(clip(&text, PLAN_MAX).to_string());
            }
        }
        (Some("function_call" | "custom_tool_call"), Some("apply_patch")) => {
            // The patch's own text, or a JSON `input` field holding it.
            let patch = serde_json::from_str::<Value>(input).ok().and_then(|a| a["input"].as_str().map(str::to_owned));
            for line in patch.as_deref().unwrap_or(input).lines() {
                for head in ["*** Update File: ", "*** Add File: ", "*** Delete File: ", "*** Move to: "] {
                    if let Some(path) = line.strip_prefix(head) {
                        note_edit(edits, path);
                    }
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::agents::handoff) fn claude_line(role: &str, text: &str, at: &str, end_turn: bool) -> String {
        if role == "user" {
            serde_json::json!({ "type": "user", "timestamp": at, "message": { "role": "user", "content": text } }).to_string()
        } else {
            let stop = if end_turn { "end_turn" } else { "tool_use" };
            serde_json::json!({ "type": "assistant", "timestamp": at,
                "message": { "model": "m", "stop_reason": stop, "content": [{ "type": "text", "text": text }] } })
            .to_string()
        }
    }

    fn file(tag: &str, text: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("luna-excerpts-{tag}-{}.jsonl", std::process::id()));
        std::fs::write(&p, text).unwrap();
        p
    }

    #[test]
    fn a_claude_transcript_gives_what_was_said_and_what_it_did_but_no_tool_output() {
        let lines = [
            claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false),
            serde_json::json!({ "type": "assistant", "timestamp": "2026-11-10T10:00:05Z", "message": { "model": "m", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "name": "Edit", "input": { "file_path": "src/a.rs", "old_string": "x" } },
                            { "type": "tool_use", "name": "ExitPlanMode", "input": { "plan": "1. Fix a.rs\n2. Test" } }] } })
            .to_string(),
            serde_json::json!({ "type": "user", "timestamp": "2026-11-10T10:00:06Z", "message": { "role": "user",
                "content": [{ "type": "tool_result", "content": "SECRET TOOL OUTPUT" }] } })
            .to_string(),
            serde_json::json!({ "type": "user", "isCompactSummary": true, "message": { "role": "user", "content": "Summary: we fixed a.rs" } })
                .to_string(),
            claude_line("assistant", "Done: the build is green.", "2026-11-10T10:01:00Z", true),
        ];
        let p = file("claude", &(lines.join("\n") + "\n"));
        let r = read(&p, Provider::Claude, None).unwrap();
        assert_eq!(r.first.as_ref().map(|s| s.text.as_str()), Some("Fix the build."));
        assert_eq!(r.began.as_deref(), Some("2026-11-10T10:00:00Z"));
        let said: Vec<(&str, &str, bool)> = r.said.iter().map(|s| (s.role.as_str(), s.text.as_str(), s.final_answer)).collect();
        assert_eq!(said, [("user", "Fix the build.", false), ("assistant", "Done: the build is green.", true)]);
        assert!(!format!("{r:?}").contains("SECRET TOOL OUTPUT"));
        assert_eq!(r.plan.as_deref(), Some("1. Fix a.rs\n2. Test"));
        assert_eq!(r.summary.as_deref(), Some("Summary: we fixed a.rs"));
        assert_eq!(r.edits, ["src/a.rs"]);
        assert!(!r.cut_front && !r.changed && r.skipped == 0);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_codex_rollout_gives_its_plan_and_patched_files() {
        let msg = |role: &str, kind: &str, text: &str, phase: Option<&str>| {
            let mut p = serde_json::json!({ "type": "message", "role": role, "content": [{ "type": kind, "text": text }] });
            if let Some(ph) = phase {
                p["phase"] = serde_json::json!(ph);
            }
            serde_json::json!({ "timestamp": "2026-11-10T10:00:00Z", "type": "response_item", "payload": p }).to_string()
        };
        let plan = serde_json::json!({ "explanation": "Two steps", "plan": [{ "step": "Patch", "status": "completed" }, { "step": "Test", "status": "in_progress" }] });
        let lines = [
            msg("user", "input_text", "<environment_context>cwd</environment_context>", None),
            msg("user", "input_text", "Add a flag.", None),
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call", "name": "update_plan", "arguments": plan.to_string() } }).to_string(),
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call", "name": "apply_patch",
                "input": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-a\n+b\n*** Add File: docs/x.md\n*** End Patch" } })
            .to_string(),
            msg("assistant", "output_text", "Added.", Some("final_answer")),
        ];
        let p = file("codex", &lines.join("\n"));
        let r = read(&p, Provider::Codex, None).unwrap();
        assert_eq!(r.first.as_ref().map(|s| s.text.as_str()), Some("Add a flag."));
        assert_eq!(r.plan.as_deref(), Some("Two steps\n- [completed] Patch\n- [in_progress] Test\n"));
        assert_eq!(r.edits, ["src/main.rs", "docs/x.md"]);
        assert!(r.said.last().is_some_and(|s| s.final_answer && s.text == "Added."));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn reading_is_bounded_at_both_ends_and_says_what_it_skipped() {
        let mut text = claude_line("user", "The first task.", "2026-11-01T00:00:00Z", false) + "\n";
        for i in 0..2000 {
            text.push_str(&claude_line("assistant", &format!("filler {i} {}", "x".repeat(200)), "2026-11-01T00:00:01Z", true));
            text.push('\n');
        }
        // One line longer than the cap.
        text.push_str(&claude_line("assistant", &"y".repeat(LINE_MAX + 10), "2026-11-01T00:00:02Z", true));
        text.push('\n');
        text.push_str(&claude_line("user", "The last ask.", "2026-11-01T00:00:03Z", false));
        text.push('\n');
        let p = file("bounded", &text);
        let r = read_bounded(&p, Provider::Claude, 4 * 1024, LINE_MAX as u64 + 64 * 1024, None).unwrap();
        assert!(r.cut_front, "the tail did not reach the start");
        assert_eq!(r.first.as_ref().map(|s| s.text.as_str()), Some("The first task."), "the head still gives the task");
        assert_eq!(r.skipped, 1, "the long line skipped, not kept");
        assert!(r.said.len() < 2000 && r.said.last().is_some_and(|s| s.text == "The last ask."));
        assert!(r.said.iter().all(|s| s.text.len() < 1024));
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn the_turn_an_ask_opened_is_left_out_to_the_next_prompt() {
        let ask = "[Luna move m1 request, not typed by the user] The person is moving this chat.";
        let lines = [
            claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false),
            claude_line("assistant", "Fixed.", "2026-11-10T10:01:00Z", true),
            claude_line("user", ask, "2026-11-10T10:02:00Z", false),
            serde_json::json!({ "type": "assistant", "timestamp": "2026-11-10T10:02:05Z", "message": { "model": "m", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "name": "Write", "input": { "file_path": "drafts/move-m1-ab.md" } }] } })
            .to_string(),
            claude_line("assistant", "The document is written.", "2026-11-10T10:03:00Z", true),
            claude_line("user", "Now the tests.", "2026-11-10T10:30:00Z", false),
            claude_line("assistant", "Green.", "2026-11-10T10:31:00Z", true),
        ];
        let p = file("skip", &(lines.join("\n") + "\n"));
        let r =
            read(&p, Provider::Claude, Some(Skip { opens: |t: &str| t.starts_with("[Luna move m1 request"), ask: None, from_call: false }))
                .unwrap();
        let said: Vec<&str> = r.said.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(said, ["Fix the build.", "Fixed.", "Now the tests.", "Green."]);
        assert!(r.edits.is_empty(), "what the ask's turn wrote is not the work: {:?}", r.edits);
        let whole = read(&p, Provider::Claude, None).unwrap();
        assert!(whole.said.iter().any(|s| s.text.starts_with("[Luna move m1 request")));
        assert_eq!(whole.edits, ["drafts/move-m1-ab.md"]);
        let _ = std::fs::remove_file(p);
    }

    /// The ask's turn longer than the window: its prompt is before it, its
    /// narration and edits inside, and they are still left out.
    #[test]
    fn a_window_that_starts_inside_the_asks_turn_leaves_it_out() {
        let ask = "[Luna move m1 request, not typed by the user] The person is moving this chat.";
        let mut text = claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false) + "\n";
        text.push_str(&claude_line("user", ask, "2026-11-10T10:02:00Z", false));
        text.push('\n');
        for i in 0..400 {
            text.push_str(&claude_line("assistant", &format!("writing {i} {}", "x".repeat(200)), "2026-11-10T10:02:30Z", false));
            text.push('\n');
        }
        text.push_str(
            &serde_json::json!({ "type": "assistant", "timestamp": "2026-11-10T10:03:00Z", "message": { "model": "m", "stop_reason": "tool_use",
                "content": [{ "type": "tool_use", "name": "Write", "input": { "file_path": "drafts/move-m1-ab.md" } }] } })
            .to_string(),
        );
        text.push('\n');
        let p = file("skip-mid", &text);
        let typed = chrono::DateTime::parse_from_rfc3339("2026-11-10T10:02:00Z").unwrap().timestamp_millis() as u64;
        let opens: fn(&str) -> bool = |t| t.starts_with("[Luna move m1 request");
        let read_with = |ask| read_bounded(&p, Provider::Claude, 4 * 1024, 32 * 1024, Some(Skip { opens, ask, from_call: false })).unwrap();
        let r = read_with(Some((typed, None)));
        assert!(r.cut_front && !r.said.iter().any(|s| s.text.contains(ask)), "the ask's prompt is before the window");
        assert!(r.said.is_empty() && r.edits.is_empty(), "the ask's turn is not the work: {:?} {:?}", r.said.len(), r.edits);
        // Closed before the window's first line: that may be a turn of the
        // chat's own, and is read.
        let r = read_with(Some((typed, Some(typed + 1_000))));
        assert!(!r.said.is_empty() && r.edits == ["drafts/move-m1-ab.md"]);
        // No ask on record: read as ever.
        assert_eq!(read_with(None).edits, ["drafts/move-m1-ab.md"]);
        let _ = std::fs::remove_file(p);
    }

    /// A Claude Code `Skill` call as 2.1.322 writes it, each by id
    /// `toolu_<name>`, and the result that says it loaded.
    fn skill_call(names: &[&str], sidechain: bool) -> String {
        let id = |n: &str| format!("toolu_{n}");
        let calls: Vec<Value> = names
            .iter()
            .map(|n| serde_json::json!({ "type": "tool_use", "id": id(n), "name": "Skill", "input": { "skill": n }, "caller": { "type": "direct" } }))
            .collect();
        let results: Vec<Value> = names
            .iter()
            .map(|n| serde_json::json!({ "type": "tool_result", "tool_use_id": id(n), "content": format!("Launching skill: {n}") }))
            .collect();
        let call = serde_json::json!({ "type": "assistant", "isSidechain": sidechain, "timestamp": "2026-11-10T10:00:05Z",
            "message": { "model": "m", "stop_reason": "tool_use", "content": calls } });
        let result = serde_json::json!({ "type": "user", "isSidechain": sidechain, "timestamp": "2026-11-10T10:00:06Z",
            "message": { "role": "user", "content": results } });
        format!("{call}\n{result}")
    }

    /// A Claude Code tool call `name` with `input` by id `id`, and its result,
    /// an error or not.
    fn tool_call(id: &str, name: &str, input: Value, error: bool) -> String {
        let call = serde_json::json!({ "type": "assistant", "isSidechain": false,
            "message": { "model": "m", "content": [{ "type": "tool_use", "id": id, "name": name, "input": input }] } });
        let mut result = serde_json::json!({ "type": "tool_result", "tool_use_id": id, "content": "…" });
        if error {
            result["is_error"] = serde_json::json!(true);
            result["content"] = serde_json::json!("<tool_use_error>Unknown skill</tool_use_error>");
        }
        let result = serde_json::json!({ "type": "user", "isSidechain": false, "message": { "role": "user", "content": [result] } });
        format!("{call}\n{result}")
    }

    /// A Codex message carrying a skill's text, as its `<skill>` block is
    /// spelled (0.167.1 `ext/skills/src/fragments.rs`).
    fn skill_block(role: &str, name: &str) -> String {
        let text = format!("<skill>\n<name>{name}</name>\n<path>C:\\acc\\skills\\{name}\\SKILL.md</path>\n# {name}\nsteps\n</skill>");
        serde_json::json!({ "timestamp": "2026-11-10T10:00:00Z", "type": "response_item",
            "payload": { "type": "message", "role": role, "content": [{ "type": "input_text", "text": text }] } })
        .to_string()
    }

    #[test]
    fn the_skills_a_claude_code_conversation_loaded_come_from_its_skill_calls_and_reads() {
        // Claude Code writes the session's folder on every line.
        let at = |line: String| {
            let mut v: Value = serde_json::from_str(&line).unwrap();
            v["cwd"] = serde_json::json!("E:\\p");
            v.to_string()
        };
        let two = |pair: String| pair.split('\n').map(|l| at(l.to_string())).collect::<Vec<_>>().join("\n");
        let lines = [
            claude_line("user", "Release it, and mind the Skill tool.", "2026-11-10T10:00:00Z", false),
            two(skill_call(&["release"], false)),
            // The same call as Claude Code also logs it: no assistant line.
            serde_json::json!({ "type": "api-request-blob", "message": { "content": [
                { "type": "tool_use", "name": "Skill", "input": { "skill": "blob-only" } }] } })
            .to_string(),
            skill_call(&["ui-check", "release"], false),
            // A subagent's own, not the conversation's.
            skill_call(&["sidechain-only"], true),
            // Refused: no skill of that name.
            tool_call("toolu_e", "Skill", serde_json::json!({ "skill": "misspelt" }), true),
            // Called, its result not back (a turn cut short).
            serde_json::json!({ "type": "assistant", "message": { "content": [
                { "type": "tool_use", "id": "toolu_open", "name": "Skill", "input": { "skill": "unanswered" } }] } })
            .to_string(),
            // A skill's file read: its folder's name, once read.
            two(tool_call("toolu_r", "Read", serde_json::json!({ "file_path": "E:\\acc\\skills\\live-test\\SKILL.md" }), false)),
            tool_call("toolu_m", "Read", serde_json::json!({ "file_path": "/home/u/.claude/skills/missing/SKILL.md" }), true),
            tool_call("toolu_o", "Read", serde_json::json!({ "file_path": "E:\\p\\docs\\SKILL.md.bak" }), false),
            tool_call("toolu_x", "Read", serde_json::json!({ "skill": "not-a-skill-call" }), false),
            // A skill of the project's read and then edited: work on the
            // skill, not the skill loaded. One read and not edited counts.
            two(tool_call("toolu_w", "Read", serde_json::json!({ "file_path": "E:\\p\\.agents\\skills\\worked-on\\SKILL.md" }), false)),
            two(tool_call(
                "toolu_we",
                "Edit",
                serde_json::json!({ "file_path": "E:/p/.agents/skills/worked-on/SKILL.md", "old_string": "a", "new_string": "b" }),
                false,
            )),
            two(tool_call("toolu_k", "Read", serde_json::json!({ "file_path": "E:\\p\\.agents\\skills\\kept\\SKILL.md" }), false)),
            claude_line("assistant", "Released.", "2026-11-10T10:01:00Z", true),
        ];
        let p = file("skills-claude", &(lines.join("\n") + "\n"));
        let s = skills_in(&p, Provider::Claude, None).unwrap();
        let want = ["release", "ui-check", "live-test", "kept"];
        assert_eq!(
            s,
            Skills { names: want.iter().map(|n| n.to_string()).collect(), cut: false, code_mode: false },
            "in order of first use"
        );
        assert_eq!(read(&p, Provider::Claude, None).unwrap().skills, s, "the move's reading finds the same");
        assert!(skills_in(&p.with_extension("gone"), Provider::Claude, None).is_err());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn the_skills_a_codex_conversation_loaded_come_from_its_skill_blocks_and_the_files_it_ran_a_reader_on() {
        let msg = |role: &str, text: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": role,
                "content": [{ "type": "input_text", "text": text }] } })
            .to_string()
        };
        // A shell tool's call and its result, headed as 0.167.2's binary
        // writes them: `Exit code: N` (`shell`, `shell_command`, a local
        // shell's), `Process exited with code N` (`exec_command`).
        let function = |id: &str, name: &str, args: Value, output: &str| {
            let call = serde_json::json!({ "type": "response_item", "payload": { "type": "function_call", "name": name, "call_id": id,
                "arguments": args.to_string() } });
            let out = serde_json::json!({ "type": "response_item", "payload": { "type": "function_call_output", "call_id": id, "output": output } });
            format!("{call}\n{out}")
        };
        let shell = |code: i32| format!("Exit code: {code}\nWall time: 0.1 seconds\nOutput:\n# Steps\n");
        let exited = |code: i32| {
            format!(
                "Chunk ID: 5a3ee7\nWall time: 0.1500 seconds\nProcess exited with code {code}\nOriginal token count: 9\nOutput:\n# Steps\n"
            )
        };
        let cmd = |c: &str| serde_json::json!({ "cmd": c });
        let lines = [
            serde_json::json!({ "type": "turn_context", "payload": { "cwd": "E:\\p", "model": "m" } }).to_string(),
            // The list of skills it may load is no skill loaded.
            msg("developer", "<skills_instructions>\n## Skills\n- review: (file: C:\\s\\SKILL.md)\n</skills_instructions>"),
            msg("user", "Use $review, then <skill>s are done."),
            skill_block("developer", "review"),
            skill_block("user", "release"),
            skill_block("developer", "review"),
            msg("assistant", "<skill>\n<name>said-not-loaded</name>\n</skill>"),
            // Read through a shell tool: each command that prints a SKILL.md,
            // a path in either quote, once the call's own result states exit
            // status 0, whatever the file says after its header.
            function("c1", "exec_command", cmd("Get-Content .codex/CODEX.md; Get-Content .agents/skills/write-story/SKILL.md"), &exited(0)),
            function(
                "c2",
                "shell_command",
                serde_json::json!({ "command": "Get-Content 'E:/LLM Accounts/openai/team/skills/background-jobs/SKILL.md'" }),
                &format!("{}Exit code: 1 is what a failure gives", shell(0)),
            ),
            function("c3", "exec_command", cmd("Get-Content -Raw \"C:\\Users\\me\\.codex\\skills\\quoted\\SKILL.md\""), &exited(0)),
            function("c4", "shell", serde_json::json!({ "command": ["bash", "-lc", "sed -n '1,200p' /home/u/.codex/skills/live-test/SKILL.md | head"] }), &shell(0)),
            serde_json::json!({ "type": "response_item", "payload": { "type": "local_shell_call", "call_id": "c5", "status": "completed",
                "action": { "type": "exec", "command": ["cat", "/s/local/SKILL.md"] } } })
            .to_string(),
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call_output", "call_id": "c5", "output": shell(0) } }).to_string(),
            // Failed, still running at its result, no status in its header,
            // or its result not back: not seen.
            function("c6", "exec_command", cmd("Get-Content .agents/skills/gone/SKILL.md"), &exited(1)),
            function("c7", "shell_command", serde_json::json!({ "command": "cat .agents/skills/failed/SKILL.md" }), &shell(2)),
            function(
                "c8",
                "exec_command",
                cmd("Get-Content .agents/skills/running/SKILL.md"),
                "Chunk ID: 1\nWall time: 10.0 seconds\nProcess running with session ID 3\nOutput:\n# Steps\nProcess exited with code 0\n",
            ),
            function("c9", "shell", serde_json::json!({ "command": ["cat", ".agents/skills/unheaded/SKILL.md"] }), "Output:\nExit code: 0\n"),
            serde_json::json!({ "type": "response_item", "payload": { "type": "function_call", "name": "shell", "call_id": "c10",
                "arguments": serde_json::json!({ "command": ["cat", ".agents/skills/unanswered/SKILL.md"] }).to_string() } })
            .to_string(),
            // Code mode's script is not read for what it runs: a command it
            // only passes on in a string is no read either.
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call", "name": "exec", "call_id": "c11",
                "input": "text({cmd: \"cat .agents/skills/scripted/SKILL.md\"});" } })
            .to_string(),
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call_output", "call_id": "c11",
                "output": [{ "type": "input_text", "text": "Script completed\nWall time 1.1 seconds\nOutput:\n{\"exit_code\":0}" }] } })
            .to_string(),
            // Naming one is not reading it: a search, a listing, a write, a
            // command in a tool that runs none (a message to an agent).
            function("c12", "exec_command", cmd("rg -n Steps .agents/skills/searched/SKILL.md; ls .agents/skills/listed/SKILL.md"), &exited(0)),
            function("c13", "shell", serde_json::json!({ "command": ["pwsh", "-c", "Set-Content .agents/skills/written/SKILL.md 'x'"] }), &shell(0)),
            function("c14", "send_message", serde_json::json!({ "to": "a1", "text": "Get-Content .agents/skills/told/SKILL.md" }), &shell(0)),
            // The project's own skill read and then patched: work on it.
            function("c15", "exec_command", cmd("Get-Content .agents/skills/worked-on/SKILL.md"), &exited(0)),
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call", "name": "apply_patch", "call_id": "c16",
                "input": "*** Begin Patch\n*** Update File: .agents/skills/worked-on/SKILL.md\n@@\n-a\n+b\n*** End Patch" } })
            .to_string(),
        ];
        let p = file("skills-codex", &(lines.join("\n") + "\n"));
        let s = skills_in(&p, Provider::Codex, None).unwrap();
        assert_eq!(s.names, ["review", "release", "write-story", "background-jobs", "quoted", "live-test", "local"]);
        assert_eq!(read(&p, Provider::Codex, None).unwrap().skills.names, s.names);
        assert!(s.code_mode, "a code-mode script ran, its reads not counted");
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn a_command_is_read_as_a_shell_reads_it() {
        assert_eq!(
            segments("Get-Content \"C:\\a b\\SKILL.md\"; cat 'x y' | head"),
            [vec!["Get-Content", "C:\\a b\\SKILL.md"], vec!["cat", "x y"], vec!["head"]]
        );
        assert_eq!(
            skills_read_by(&segments("gc \"C:\\a b\\one\\SKILL.md\" && type two/SKILL.md")),
            [("one".to_string(), "C:\\a b\\one\\SKILL.md".to_string()), ("two".to_string(), "two/SKILL.md".to_string())]
        );
        // A Windows path escaped as a JSON string or a script's literal has its
        // backslashes doubled: still its folder's skill.
        let escaped = r"C:\\Users\\x\\.claude\\skills\\windows-ssh\\SKILL.md";
        assert_eq!(skill_folder(escaped).as_deref(), Some("windows-ssh"));
        assert_eq!(skill_folder(&format!("{escaped}\n")).as_deref(), Some("windows-ssh"));
        assert_eq!(
            skills_read_by(&segments(&format!("Get-Content -LiteralPath '{escaped}'"))),
            [("windows-ssh".to_string(), escaped.to_string())]
        );
        // A script's literal inside a JSON string: escaped twice.
        assert_eq!(skill_folder(r"C:\\\\Users\\\\windows-ssh\\\\SKILL.md").as_deref(), Some("windows-ssh"));
        assert_eq!(skill_folder("SKILL.md"), None, "no folder");
        assert_eq!(skill_folder(r"x\\..\\SKILL.md"), None);
        // An argv is a program and its arguments, as they are: a `;` in one
        // parts nothing. Only a shell given a script has it parsed.
        let read = |argv: Value| skills_read_by(&command_segments(&argv)).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(
            command_segments(&serde_json::json!(["/bin/echo", "x;cat", "/s/unread/SKILL.md"])),
            [vec!["/bin/echo", "x;cat", "/s/unread/SKILL.md"]]
        );
        assert!(read(serde_json::json!(["/bin/echo", "x;cat", "/s/unread/SKILL.md"])).is_empty());
        assert_eq!(read(serde_json::json!(["/bin/cat", "a b/one/SKILL.md"])), ["one"]);
        assert_eq!(read(serde_json::json!(["bash", "-lc", "cd /p; sed -n 1,9p two/SKILL.md | head"])), ["two"]);
        assert_eq!(read(serde_json::json!(["C:\\pwsh.exe", "-NoProfile", "-Command", "Get-Content", "three/SKILL.md"])), ["three"]);
        assert_eq!(read(serde_json::json!(["cmd.exe", "/c", "type four\\SKILL.md"])), ["four"]);
        assert!(read(serde_json::json!(["bash", "four/SKILL.md"])).is_empty(), "a script file run, not a script");
        assert_eq!(command_segments(&serde_json::json!("gc x; ls")), [vec!["gc", "x"], vec!["ls"]]);
        // The status is the header's, never the output's after it.
        assert_eq!(exit_status("Exit code: 0\nWall time: 0.1 seconds\nOutput:\nExit code: 1"), Some(0));
        assert_eq!(exit_status("Chunk ID: a\nProcess exited with code -1\nOutput:\n"), Some(-1));
        assert_eq!(exit_status("Process running with session ID 3\nOutput:\nProcess exited with code 0"), None);
        assert_eq!(exit_status("Exit code: 0\n"), None, "no header without its Output: line");
        assert_eq!(exit_status("sent"), None);
        assert_eq!(path_key("./a/SKILL.md", Some("e:/p")), "e:/p/a/skill.md");
        assert_eq!(path_key("E:\\P\\a\\SKILL.md", Some("x")), "e:/p/a/skill.md");
    }

    /// A turn an ask opened, read with `from_call`: its work before the
    /// handoff call counts, from the call on it is the handoff's. Codex's
    /// call from a code-mode script is the script calling the tool, not one
    /// naming it in a string.
    #[test]
    fn an_asks_turn_is_left_out_from_its_handoff_call() {
        let msg = |role: &str, text: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": role,
                "content": [{ "type": "input_text", "text": text }] } })
            .to_string()
        };
        let exec = |input: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "custom_tool_call", "name": "exec", "call_id": "x", "input": input } })
                .to_string()
        };
        let lines = [
            msg("user", "[ask] Hand off at the next point where your work can pause."),
            skill_block("developer", "finishing"),
            exec("text(\"luna_session_handoff comes next\");"),
            skill_block("developer", "still-work"),
            exec("await tools.mcp__luna__luna_session_handoff({});"),
            skill_block("developer", "for-the-handoff"),
            msg("user", "Next."),
            skill_block("developer", "after"),
        ];
        let p = file("skills-from-call", &(lines.join("\n") + "\n"));
        let skip = |from_call| Some(Skip { opens: |t: &str| t.starts_with("[ask]"), ask: None, from_call });
        assert_eq!(skills_in(&p, Provider::Codex, skip(true)).unwrap().names, ["finishing", "still-work", "after"]);
        assert_eq!(skills_in(&p, Provider::Codex, skip(false)).unwrap().names, ["after"], "a move's whole turn");
        let call = serde_json::json!({ "type": "response_item", "payload": { "type": "function_call",
            "name": "mcp__luna__luna_session_handoff", "arguments": "{}", "call_id": "c" } });
        assert!(calls_handoff(&call, Provider::Codex));
        assert!(!calls_handoff(&call, Provider::Claude));
        let _ = std::fs::remove_file(p);
    }

    /// A Claude Code user line as 2.1.322 writes one: `meta` the CLI's own,
    /// `parent` the line before's uuid, `tool` the call a skill's text answers.
    fn user(uuid: &str, parent: &str, meta: bool, tool: Option<&str>, content: Value) -> String {
        let mut v = serde_json::json!({ "type": "user", "uuid": uuid, "parentUuid": parent, "isSidechain": false,
            "message": { "role": "user", "content": content } });
        if meta {
            v["isMeta"] = serde_json::json!(true);
        }
        if let Some(t) = tool {
            v["sourceToolUseID"] = serde_json::json!(t);
        }
        v.to_string()
    }

    fn skill_text(dir: &str) -> Value {
        serde_json::json!([{ "type": "text", "text": format!("Base directory for this skill: {dir}\n\n# Steps") }])
    }

    #[test]
    fn a_skill_a_person_started_with_a_slash_is_one_its_conversation_loaded() {
        let command = |name: &str, args: &str| {
            Value::String(format!(
                "<command-message>{name}</command-message>\n<command-name>/{name}</command-name>\n<command-args>{args}</command-args>"
            ))
        };
        let lines = [
            // Built-ins, as 2.1.322 wrote them in this machine's transcripts:
            // the name first, then their output, never a skill's text.
            user("u1", "u0", false, None, Value::String("<command-name>/effort</command-name>\n            <command-message>effort</command-message>\n            <command-args>high</command-args>".into())),
            user("u2", "u1", false, None, Value::String("<local-command-stdout>Set effort level to high</local-command-stdout>".into())),
            // A skill the person started: its line, then its text naming it as the parent.
            user("u3", "u2", false, None, command("release", "0.18.0")),
            user("u4", "u3", true, None, skill_text("E:\\p\\.claude\\skills\\release")),
            // A command of the person's whose text is no skill's.
            user("u5", "u4", false, None, command("deploy", "")),
            user("u6", "u5", true, None, serde_json::json!([{ "type": "text", "text": "Deploy it." }])),
            // A skill's text that is no child of a command line, and one the
            // Skill tool's call brought (counted by that call, not here).
            user("u7", "u6", true, None, skill_text("E:\\p\\.claude\\skills\\stray")),
            user("u8", "u7", true, Some("toolu_1"), skill_text("E:\\p\\.claude\\skills\\by-tool")),
            // A person's prompt that only quotes the shape.
            user("u9", "u8", false, None, Value::String("<command-message>quoted</command-message> is how it looks".into())),
            user("u10", "u9", true, None, skill_text("E:\\p\\.claude\\skills\\quoted")),
            command_then_skill_by_subagent(),
        ];
        let p = file("skills-slash", &(lines.join("\n") + "\n"));
        assert_eq!(skills_in(&p, Provider::Claude, None).unwrap().names, ["release"]);
        assert_eq!(read(&p, Provider::Claude, None).unwrap().skills.names, ["release"], "the move's reading finds the same");
        let _ = std::fs::remove_file(p);
    }

    /// A subagent's own `/name` and skill text: not the conversation's.
    fn command_then_skill_by_subagent() -> String {
        let mut a: Value = serde_json::from_str(&user(
            "s1",
            "u10",
            false,
            None,
            Value::String("<command-message>sub</command-message>\n<command-name>/sub</command-name>".into()),
        ))
        .unwrap();
        a["isSidechain"] = serde_json::json!(true);
        let mut b: Value = serde_json::from_str(&user("s2", "s1", true, None, skill_text("E:\\p\\sub"))).unwrap();
        b["isSidechain"] = serde_json::json!(true);
        format!("{a}\n{b}")
    }

    /// A Claude Code answer that states the context at `pct` of its model's
    /// window.
    fn answer_at(pct: f64) -> String {
        let window = crate::claude::context::context_window("m");
        let tokens = (window * pct / 100.0).round();
        serde_json::json!({ "type": "assistant", "isSidechain": false, "message": { "model": "m", "stop_reason": "end_turn",
            "usage": { "input_tokens": tokens, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0 },
            "content": [{ "type": "text", "text": "Done." }] } })
        .to_string()
    }

    /// The first turn of a conversation Luna's handoff prompt opened, as Claude
    /// Code 2.1.322 writes one (the `/clear` before Luna's prompt, pasted; the
    /// turn's tool results and a skill's text): still the last, or over with
    /// the share of the context it ended at, whatever came after it first (a
    /// helper's notice, a draft the person sent once it ended).
    #[test]
    fn a_handoffs_first_turn_ends_at_the_context_before_anything_more_is_said() {
        let prompt = "\n\n<pasted_content id=\"4da7\">\n[Luna handoff 1, not typed by the user] This is a new conversation.\n</pasted_content id=\"4da7\">";
        let opening = vec![
            user(
                "c0",
                "",
                true,
                None,
                Value::String("<local-command-caveat>The command below was run directly</local-command-caveat>".into()),
            ),
            user(
                "c1",
                "c0",
                false,
                None,
                Value::String("<command-name>/clear</command-name>\n<command-message>clear</command-message>".into()),
            ),
            user("c2", "c1", false, None, Value::String(prompt.into())),
            skill_call(&["release"], false),
            user("c4", "c3", true, Some("toolu_release"), skill_text("E:\\p\\release")),
            answer_at(12.0),
        ];
        let p = file("first-turn", &(opening.join("\n") + "\n"));
        assert_eq!(handoffs_first_turn(&p, Provider::Claude), Some(FirstTurn { over: false, ended_pct: None }));
        for next in ["[Luna notice, not typed by the user] Your helper h1 answered.", "Now the tests, as I drafted them while you worked."]
        {
            let mut lines = opening.clone();
            lines.push(claude_line("user", next, "2026-11-10T10:30:00Z", false));
            lines.push(answer_at(14.0));
            std::fs::write(&p, lines.join("\n") + "\n").unwrap();
            assert_eq!(handoffs_first_turn(&p, Provider::Claude), Some(FirstTurn { over: true, ended_pct: Some(12) }), "{next}");
        }
        // A conversation Luna's prompt did not open, and none at all.
        std::fs::write(&p, claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false) + "\n").unwrap();
        assert_eq!(handoffs_first_turn(&p, Provider::Claude), None);
        let _ = std::fs::remove_file(&p);
        assert_eq!(handoffs_first_turn(&p, Provider::Claude), None);
        // Codex: what it injects before the prompt is no one's word.
        let msg = |role: &str, text: &str| {
            serde_json::json!({ "type": "response_item", "payload": { "type": "message", "role": role,
                "content": [{ "type": "input_text", "text": text }] } })
            .to_string()
        };
        let count = |tokens: u64| {
            serde_json::json!({ "type": "event_msg", "payload": { "type": "token_count", "info": {
                "last_token_usage": { "total_tokens": tokens, "reasoning_output_tokens": 0 }, "model_context_window": 100_000 } } })
            .to_string()
        };
        let mut rollout = vec![
            msg("user", "<environment_context>\n  <cwd>E:\\p</cwd>\n</environment_context>"),
            msg("user", "[Luna handoff 2, not typed by the user] This is a new conversation."),
            msg("assistant", "On it."),
            count(30_000),
        ];
        let q = file("first-turn-codex", &(rollout.join("\n") + "\n"));
        assert_eq!(handoffs_first_turn(&q, Provider::Codex), Some(FirstTurn { over: false, ended_pct: None }));
        rollout.push(msg("user", "And the docs."));
        rollout.push(count(40_000));
        std::fs::write(&q, rollout.join("\n") + "\n").unwrap();
        assert_eq!(handoffs_first_turn(&q, Provider::Codex), Some(FirstTurn { over: true, ended_pct: Some(30) }));
        let _ = std::fs::remove_file(q);
    }

    /// Read a line at a time, an over-long one passed over; before the move's
    /// window from the file's start, within it as the rest of it is read, the
    /// ask's turn left out.
    #[test]
    fn the_skills_are_read_bounded_and_the_asks_turn_is_left_out() {
        let ask = "[Luna move m1 request, not typed by the user] The person is moving this chat.";
        let mut text = claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false) + "\n";
        text.push_str(&(skill_call(&["early"], false) + "\n"));
        // Longer than a line may be: its skill is not read.
        let zs = "z".repeat(LINE_MAX);
        text.push_str(&(skill_call(&[zs.as_str()], false) + "\n"));
        for i in 0..400 {
            text.push_str(&(claude_line("assistant", &format!("work {i} {}", "x".repeat(200)), "2026-11-10T10:01:00Z", true) + "\n"));
        }
        text.push_str(&(skill_call(&["late"], false) + "\n"));
        text.push_str(&(claude_line("user", ask, "2026-11-10T10:02:00Z", false) + "\n"));
        text.push_str(&(skill_call(&["for-the-ask"], false) + "\n"));
        let p = file("skills-bounded", &text);
        assert_eq!(skills_in(&p, Provider::Claude, None).unwrap().names, ["early", "late", "for-the-ask"]);
        let opens: fn(&str) -> bool = |t| t.starts_with("[Luna move m1 request");
        let r = read_bounded(&p, Provider::Claude, 4 * 1024, 32 * 1024, Some(Skip { opens, ask: None, from_call: false })).unwrap();
        assert!(r.cut_front);
        assert_eq!(r.skills.names, ["early", "late"], "before the window and in it, the ask's turn left out");
        // A transcript past what is read for them says so.
        let mut f = std::fs::File::open(&p).unwrap();
        let past = skills_within(&mut f, SKILLS_READ_MAX + 1, Provider::Claude, None).unwrap();
        assert!(past.cut && past.look.names().len() == 3);
        let _ = std::fs::remove_file(p);
    }

    /// A reading from the start stops once its caller has what it needs (the
    /// first turn, at the second thing said), and at its cap.
    #[test]
    fn a_reading_from_the_start_stops_when_told_and_at_its_cap() {
        let p = file("each-line", "one\ntwo\nthree\n");
        let mut f = std::fs::File::open(&p).unwrap();
        let mut seen = Vec::new();
        each_line(&mut f, 14, u64::MAX, |t| {
            seen.push(t.to_string());
            t != "two"
        })
        .unwrap();
        assert_eq!(seen, ["one", "two"]);
        seen.clear();
        each_line(&mut f, 14, 4, |t| {
            seen.push(t.to_string());
            true
        })
        .unwrap();
        assert_eq!(seen, ["one"]);
        let _ = std::fs::remove_file(p);
    }

    /// The ask's turn begun before the move's window: a skill loaded in it
    /// there is left out too, and so are the ones in it within the window,
    /// the read before carrying over that it ended inside the ask's turn. A
    /// turn of the chat's own after the ask's, before the window, is read.
    #[test]
    fn an_asks_turn_begun_before_the_window_is_left_out_of_the_skills() {
        let ask = "[Luna move m1 request, not typed by the user] The person is moving this chat.";
        let opens: fn(&str) -> bool = |t| t.starts_with("[Luna move m1 request");
        let filler = |text: &mut String| {
            for i in 0..300 {
                text.push_str(
                    &(claude_line("assistant", &format!("writing {i} {}", "x".repeat(200)), "2026-11-10T10:02:30Z", false) + "\n"),
                );
            }
        };
        let mut text = claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false) + "\n";
        text.push_str(&(claude_line("user", ask, "2026-11-10T10:02:00Z", false) + "\n"));
        text.push_str(&(skill_call(&["ask-early"], false) + "\n"));
        filler(&mut text);
        text.push_str(&(skill_call(&["ask-late"], false) + "\n"));
        let p = file("skills-ask-early", &text);
        // An ask on record that has not closed: the window's own timestamps
        // would not tell the skills apart either way; the read before does.
        let r = read_bounded(&p, Provider::Claude, 4 * 1024, 16 * 1024, Some(Skip { opens, ask: None, from_call: false })).unwrap();
        assert!(r.cut_front);
        assert!(r.skills.names.is_empty(), "{:?}", r.skills.names);
        // The chat's own turn after the ask's, before the window: read.
        text.push_str(&(claude_line("user", "Back to the build.", "2026-11-10T10:40:00Z", false) + "\n"));
        text.push_str(&(skill_call(&["own"], false) + "\n"));
        filler(&mut text);
        std::fs::write(&p, &text).unwrap();
        let r = read_bounded(&p, Provider::Claude, 4 * 1024, 16 * 1024, Some(Skip { opens, ask: None, from_call: false })).unwrap();
        assert_eq!(r.skills.names, ["own"]);
        let _ = std::fs::remove_file(p);
    }

    /// A skill call on the line the window's start cuts: the window leaves
    /// that line out, and the read before reads it whole.
    #[test]
    fn a_skill_on_the_line_the_window_cuts_is_still_read() {
        let mut text = claude_line("user", "Fix the build.", "2026-11-10T10:00:00Z", false) + "\n";
        for i in 0..100 {
            text.push_str(&(claude_line("assistant", &format!("work {i} {}", "x".repeat(200)), "2026-11-10T10:01:00Z", true) + "\n"));
        }
        let (call, result) = skill_call(&["straddling"], false).split_once('\n').map(|(a, b)| (a.to_string(), b.to_string())).unwrap();
        let (call_at, result_at) = (text.len(), text.len() + call.len() + 1);
        text.push_str(&(call.clone() + "\n" + &result + "\n"));
        let after_at = text.len();
        for i in 0..20 {
            text.push_str(&(claude_line("assistant", &format!("after {i}"), "2026-11-10T10:02:00Z", true) + "\n"));
        }
        let p = file("skills-straddle", &text);
        // A line belongs to the side it starts on: cut inside the call's line,
        // right after the newline before it, right after its own, and right
        // before the line after its result. Neither reader drops a line.
        for (cut_at, why) in [
            (call_at + call.len() / 2, "inside its line"),
            (call_at, "right at its start"),
            (result_at, "right at its result's start"),
            (after_at, "right after its result"),
        ] {
            let tail_max = (text.len() - cut_at) as u64;
            let r = read_bounded(&p, Provider::Claude, 4 * 1024, tail_max, None).unwrap();
            assert!(r.cut_front, "{why}");
            assert_eq!(r.skills.names, ["straddling"], "{why}");
            let first = r.said.first().map(|s| s.text.clone()).unwrap_or_default();
            assert_eq!(first, "after 0", "{why}: the window keeps the first line that starts in it");
        }
        let _ = std::fs::remove_file(p);
    }
}
