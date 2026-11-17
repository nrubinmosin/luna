//! What a session waits on a person for (`Ask`): a question its model put or
//! leave to run a tool, from Claude Code's hooks or off a Codex screen, and the
//! state `list` and `wait` report with it.

use crate::agents::delivery::still_starting;
use crate::agents::read::transcript_path;
use crate::agents::registry::{reg, waits_on_helpers};
use crate::provider::Provider;
use serde::Serialize;

/// The most of a permission's detail an ask keeps, in bytes: a whole command
/// as a rule, cut only past this, with a mark saying how much is left out.
pub const DETAIL_MAX: usize = 8 * 1024;
/// The most of it a notice line quotes, in characters: the rest is in
/// `luna_session_get`'s `ask.detail`, and the line says so.
const LINE_DETAIL: usize = 200;

/// What a session waits on a person for: a question its model put (Claude
/// Code's `AskUserQuestion`, Codex's `request_user_input`), or leave to run a
/// tool. `luna_session_answer` answers either.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Ask {
    /// A background helper's: which of its held asks this is, for `luna_session_answer`
    /// to name, so that an answer written on one cannot land on the next. A
    /// terminal shows one dialog at a time and has none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// `question` or `permission`.
    pub kind: &'static str,
    /// The turn holds until it is answered. Codex's async question does not
    /// hold it: the model works on beside it.
    pub blocking: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub questions: Vec<AskQuestion>,
    /// A permission's tool, and what it would do with it: the one field of its
    /// input that says so, whole up to `DETAIL_MAX`, past it cut with a mark
    /// (`Ask::permission`). A parent allows what it reads here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Characters of the detail left out past `DETAIL_MAX`, which is also
    /// the text's last words (`… [N more characters cut]`); absent when it is
    /// whole, so that a command that itself ends in such words is told from a
    /// cut one. A Codex screen's own marks (`codex_command_on`) are in the text.
    #[serde(skip_serializing_if = "is_zero")]
    pub detail_cut: usize,
    /// Who in the session asks, where its CLI says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<AskSource>,
}

/// Who in a session made the call it asks about: the session itself, or a
/// native subagent its CLI runs inside it, whose call the session's own
/// transcript does not hold. Claude Code says so in a `PermissionRequest`
/// hook's `agent_id` / `agent_type` and on a stream-json message's `agent_id` /
/// `subagent_type`; a Codex screen says nothing of it.
#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskSource {
    /// `session` or `subagent`.
    pub by: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
}

impl AskSource {
    pub fn session() -> AskSource {
        AskSource { by: "session", agent_id: None, agent_type: None }
    }

    pub fn subagent(agent_id: Option<&str>, agent_type: Option<&str>) -> AskSource {
        let some = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(str::to_owned);
        AskSource { by: "subagent", agent_id: some(agent_id), agent_type: some(agent_type) }
    }

    /// From a hook's input, which names a subagent's id only when one asks.
    pub fn of(agent_id: Option<&str>, agent_type: Option<&str>) -> AskSource {
        match agent_id.filter(|a| !a.is_empty()) {
            Some(_) => AskSource::subagent(agent_id, agent_type),
            None => AskSource::session(),
        }
    }
}

/// `text` up to `DETAIL_MAX` bytes, cut on a character's boundary: whole, or
/// the part kept, ending in `…` and how many characters are left out; with
/// that count.
fn capped(text: String) -> (String, usize) {
    if text.len() <= DETAIL_MAX {
        return (text, 0);
    }
    let at = (0..=DETAIL_MAX).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    let left = text[at..].chars().count();
    let unit = if left == 1 { "character" } else { "characters" };
    (format!("{}… [{left} more {unit} cut]", &text[..at]), left)
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// How a notice shows a line break of a detail: visibly, between spaces.
const BREAK: &str = " ⏎ ";

/// `text` on one line for a notice, each line break (of any kind a terminal or
/// a shell takes for one) shown as `BREAK`, the text's ends trimmed.
fn show_breaks(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.trim().chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' if chars.peek() == Some(&'\n') => {}
            '\r' | '\n' | '\u{85}' | '\u{2028}' | '\u{2029}' => out.push_str(BREAK),
            c => out.push(c),
        }
    }
    out
}

#[derive(Clone, Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskQuestion {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// More than one option may be picked.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub multi: bool,
    /// A list on a Codex screen (`codex_list_on`): the option its cursor is
    /// on. Such a list takes only its own options, no words of one's own.
    #[serde(skip)]
    pub list_at: Option<usize>,
}

impl Ask {
    pub fn question(questions: Vec<AskQuestion>, blocking: bool) -> Ask {
        Ask { id: None, kind: "question", blocking, questions, tool: None, detail: None, detail_cut: 0, source: None }
    }

    /// A permission, whichever CLI it comes from: its detail capped at
    /// `DETAIL_MAX`, never cut without a mark.
    pub fn permission(tool: Option<String>, detail: Option<String>) -> Ask {
        let (detail, detail_cut) = match detail.map(capped) {
            Some((d, cut)) => (Some(d), cut),
            None => (None, 0),
        };
        Ask { id: None, kind: "permission", blocking: true, questions: Vec::new(), tool, detail, detail_cut, source: None }
    }

    /// The same ask, said to come from `source`.
    pub fn asked_by(self, source: Option<AskSource>) -> Ask {
        Ask { source, ..self }
    }

    /// Claude Code's `AskUserQuestion` input: `questions[]` with `question`,
    /// `options[].label` and `multiSelect`.
    pub fn from_claude_questions(input: &serde_json::Value) -> Option<Ask> {
        let questions: Vec<AskQuestion> = input["questions"]
            .as_array()?
            .iter()
            .filter_map(|q| {
                let text = q["question"].as_str()?.trim().to_string();
                let options = q["options"]
                    .as_array()
                    .map(|o| o.iter().filter_map(|o| o["label"].as_str().map(str::to_owned)).collect())
                    .unwrap_or_default();
                Some(AskQuestion { text, options, multi: q["multiSelect"].as_bool().unwrap_or(false), list_at: None })
            })
            .collect();
        (!questions.is_empty()).then(|| Ask::question(questions, true))
    }

    /// A Claude Code tool call held for permission: the tool, and the one
    /// field of its input that says what it would do, whole (`permission`).
    pub fn from_claude_tool(name: &str, input: &serde_json::Value) -> Ask {
        let detail = ["command", "file_path", "path", "url", "pattern", "description"]
            .iter()
            .find_map(|k| input[*k].as_str())
            .map(str::to_owned)
            .or_else(|| input.is_object().then(|| input.to_string()));
        Ask::permission(Some(name.to_string()), detail)
    }

    /// One line for a notice: the questions with their options numbered, or
    /// the permission's tool, who asks where that is known, and its detail. A
    /// detail longer than `LINE_DETAIL` is shortened with a mark pointing to
    /// `luna_session_get`, first with each line's runs of whitespace made one
    /// space, so that padding with spaces, tabs or other Unicode whitespace
    /// cannot push what follows out of the part shown, and each line break
    /// shown as `⏎`, not a space. Characters that are invisible without being
    /// whitespace (zero-width ones, a braille blank) are not collapsed: such
    /// padding can still push the end out, but the mark is there then, and it
    /// says the detail has the rest.
    pub fn line(&self) -> String {
        if self.kind == "permission" {
            let tool = self.tool.as_deref().map(|t| match &self.source {
                Some(AskSource { by: "subagent", agent_id, agent_type }) => {
                    let named = agent_type.as_deref().map(|a| format!(" {a:?}")).unwrap_or_default();
                    let id = agent_id.as_deref().map(|a| format!(", agentId {a}")).unwrap_or_default();
                    format!("{t} (a call of its subagent{named}{id})")
                }
                Some(_) => format!("{t} (its own call)"),
                None => t.to_string(),
            });
            let detail = self.detail.as_deref().map(|d| {
                if d.chars().count() <= LINE_DETAIL && self.detail_cut == 0 {
                    return show_breaks(d);
                }
                // Each line's whitespace runs made one space, the lines
                // apart by a visible break: a newline is where a command ends
                // and the next begins, not a space within one.
                let flat = show_breaks(d)
                    .split(BREAK)
                    .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(BREAK);
                let chars = flat.chars().count();
                let shown = if chars > LINE_DETAIL { format!("{}…", flat.chars().take(LINE_DETAIL).collect::<String>()) } else { flat };
                let whole = if self.detail_cut > 0 {
                    format!("its first {} KB, cut there with a mark", DETAIL_MAX / 1024)
                } else {
                    "it whole".to_string()
                };
                format!("{shown} [shortened; luna_session_get's ask.detail has {whole}]")
            });
            let what = [tool, detail].into_iter().flatten().collect::<Vec<_>>().join(": ");
            return if what.is_empty() { "asks for permission".into() } else { format!("asks for permission to run {what}") };
        }
        let qs: Vec<String> = self
            .questions
            .iter()
            .map(|q| {
                let opts: Vec<String> = q.options.iter().enumerate().map(|(i, o)| format!("{}. {o}", i + 1)).collect();
                if opts.is_empty() {
                    format!("\"{}\"", q.text)
                } else {
                    format!("\"{}\" ({})", q.text, opts.join(", "))
                }
            })
            .collect();
        format!("asks: {}", qs.join(" "))
    }
}

/// A Claude Code hook (hub.rs): what it puts up for an answer, kept until
/// the tool runs or the turn is over — the turn's own, or the subagent's that
/// asks: a background agent's tool coming back answers nothing the turn put.
pub fn on_hook(chat: &str, body: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else { return };
    let tool = v["tool_name"].as_str().unwrap_or("");
    let asker = AskSource::of(v["agent_id"].as_str(), v["agent_type"].as_str());
    let event = v["hook_event_name"].as_str();
    let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
    if event == Some("SessionEnd") {
        r.hook_asks.remove(chat);
        return;
    }
    let raises = event == Some("PermissionRequest") || (event == Some("PreToolUse") && tool == "AskUserQuestion");
    let settles =
        matches!(event, Some("PostToolUse" | "PostToolUseFailure" | "Stop" | "StopFailure" | "UserPromptSubmit" | "SubagentStop"));
    if !raises && !settles {
        return;
    }
    // One held ask per asker: what it puts up next, or its tool coming back,
    // is the end of the one it had.
    let held = r.hook_asks.entry(chat.to_string()).or_default();
    held.retain(|(_, _, by)| by.agent_id != asker.agent_id);
    if raises {
        held.push((tool.to_string(), v["tool_input"].clone(), asker));
    } else if held.is_empty() {
        r.hook_asks.remove(chat);
    }
}

/// Whether the session is there to talk to:a pty session that runs, or a
/// background helper, which is there between its turns too.
pub(super) fn live(pty: &crate::pty::PtyManager, id: &str) -> bool {
    crate::background::has(id) || pty.alive(id)
}

/// What a session waits on a person for, if anything: a background helper's
/// held call; a Claude Code `AskUserQuestion` with no answer yet, or the tool
/// a permission prompt is up for; a Codex question tool not yet answered, or
/// the approval its screen shows.
pub fn ask_of(id: &str) -> Option<Ask> {
    if crate::background::has(id) {
        return crate::background::ask(id);
    }
    let pty = crate::pty::manager();
    let p = pty.probe(id)?;
    let turn = crate::activity::turn_state(id).map(|t| t.turn);
    let waiting = turn == Some(crate::activity::Turn::Waiting);
    match p.provider {
        Provider::Claude => {
            // The hooks say it whole; the transcript has a tool call held for
            // permission only sometimes, and a question not before its answer.
            let hooked = reg()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hook_asks
                .get(id)
                .and_then(|held| held.first())
                .map(|(name, input, by)| (name.clone(), input.clone(), Some(by.clone())));
            // A call read off the transcript names nobody: it says who only by
            // which file it is in.
            let (name, input, by) = hooked.filter(|_| turn.is_some_and(|t| t != crate::activity::Turn::Idle)).or_else(|| {
                waiting.then(|| crate::claude::session::pending_tool(&transcript_path(pty, id)?).map(|(n, i)| (n, i, None))).flatten()
            })?;
            if name == "AskUserQuestion" {
                Ask::from_claude_questions(&input)
            } else {
                waiting.then(|| Ask::from_claude_tool(&name, &input).asked_by(by))
            }
        }
        Provider::Codex => crate::codex::session::pending_ask(&crate::codex::session::Live {
            chat_id: &p.id,
            cwd: &p.cwd,
            spawned_at_ms: p.spawned_at_ms,
            resume: p.resume.as_deref(),
            account_path: &p.account_path,
            last_output_ms: p.last_output_ms,
        })
        .or_else(|| waiting.then(|| Ask::permission(None, pty.screen(id).as_deref().and_then(codex_command_on))))
        .or_else(|| (turn != Some(crate::activity::Turn::Busy)).then(|| pty.screen(id).as_deref().and_then(codex_list_on)).flatten()),
    }
}

/// What Codex's exec approval dialog says above its command, and the other
/// approval, of edits, above theirs.
const CODEX_RUN_HEAD: &str = "Would you like to run the following command?";
const CODEX_EDIT_HEAD: &str = "Would you like to make the following edits?";
const CODEX_START_OFF: &str = "[the command could not be read whole: its start is above what the terminal shows, so none of it is quoted]";
const CODEX_END_OFF: &str = "[the command could not be read whole: its end is not on the terminal's screen]";
const CODEX_TWO_DIALOGS: &str = "[the screen holds more than one approval dialog; the command may start at any later `$ ` row]";

/// The command a Codex approval dialog shows, from its `$ ` row down to the
/// options, each row trimmed, apart by line breaks as drawn. Codex puts `$ `
/// before the first line only; a line too long for the screen wraps onto the
/// rows below it and a command of several lines goes on there, so the first
/// row alone would be a cut with no mark.
///
/// The dialog is found, not any row that looks like a part of it: the options
/// are the last `1. ` row with a `2. ` row below it (the command is drawn
/// above them, so a heredoc line `1. …` is not taken for them), and the
/// command starts at the `$ ` row right under the dialog's heading. What the
/// screen does not hold whole is said so in the text, never left out quietly:
/// a command whose end is not drawn (no options below it), whose start is off
/// the top of the screen, or a screen with two such dialogs. Any other
/// screen: none.
pub(super) fn codex_command_on(screen: &str) -> Option<String> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let option = |l: &str, n: &str| l.strip_prefix('›').map(str::trim_start).unwrap_or(l).starts_with(n);
    let options = (0..lines.len()).rev().find(|&i| option(lines[i], "1. ") && lines[i + 1..].iter().any(|l| option(l, "2. ")));
    let above = options.unwrap_or(lines.len());
    // Each `$ ` row right under a heading, above the options.
    let starts: Vec<usize> = (0..above)
        .filter(|&i| lines[i] == CODEX_RUN_HEAD)
        .filter_map(|i| lines[i + 1..above].iter().position(|l| !l.is_empty()).map(|k| i + 1 + k))
        .filter(|&d| lines[d].starts_with("$ "))
        .collect();
    let rows_from = |start: usize| -> String {
        let mut rows: Vec<&str> = lines[start..above].to_vec();
        rows[0] = &rows[0][2..];
        rows.join("\n").trim().to_string()
    };
    let (Some(&first), Some(&last)) = (starts.first(), starts.last()) else {
        // Options of a dialog with no heading: its top is off the screen, and
        // so is the start of the command.
        let off =
            options.is_some_and(|i| option(lines[i], "1. Yes")) && !lines.iter().any(|l| *l == CODEX_RUN_HEAD || *l == CODEX_EDIT_HEAD);
        return off.then(|| CODEX_START_OFF.to_string());
    };
    let command = rows_from(last);
    if command.is_empty() {
        return None;
    }
    if options.is_none() {
        return Some(format!("{command}\n{CODEX_END_OFF}"));
    }
    Some(if first == last { command } else { format!("{CODEX_TWO_DIALOGS}\n{}", rows_from(first)) })
}

/// A list Codex holds its screen on until an option is picked — "Hooks need
/// review" at startup and the like — as a question: the lines above the
/// options for its text, the options as numbered, and the one the cursor is
/// on. Only at the bottom of the screen, above a footer naming Enter and Esc,
/// so that a numbered list in the conversation is not taken for one.
pub(super) fn codex_list_on(screen: &str) -> Option<Ask> {
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let foot = lines.iter().rposition(|l| !l.is_empty())?;
    let f = lines[foot].to_lowercase();
    if !(f.contains("enter") && f.contains("esc")) {
        return None;
    }
    let numbered = |l: &str| -> Option<(usize, bool, String)> {
        let (on, rest) = match l.strip_prefix('›') {
            Some(r) => (true, r.trim_start()),
            None => (false, l),
        };
        let digits = rest.find(|c: char| !c.is_ascii_digit()).filter(|&d| d > 0)?;
        let n = rest[..digits].parse().ok()?;
        // A description may follow the label after a run of spaces.
        let label = rest[digits..].strip_prefix(". ")?.split("  ").next()?.trim();
        (!label.is_empty()).then(|| (n, on, label.to_string()))
    };
    // Up from the footer to option 1; lines between options are descriptions.
    let mut found: Vec<(usize, bool, String)> = Vec::new();
    let mut first = None;
    for i in (foot.saturating_sub(30)..foot).rev() {
        // The composer's `›` below a numbered list: the conversation, with a
        // footer of its own (a selection's "enter copy · esc clear").
        if lines[i].starts_with('›') && numbered(lines[i]).is_none() {
            return None;
        }
        if let Some(o) = numbered(lines[i]) {
            if found.last().is_some_and(|last| last.0 != o.0 + 1) {
                return None;
            }
            let one = o.0 == 1;
            found.push(o);
            if one {
                first = Some(i);
                break;
            }
        }
    }
    let first = first.filter(|_| found.len() >= 2)?;
    found.reverse();
    let at = found.iter().position(|o| o.1).unwrap_or(0);
    let options = found.into_iter().map(|o| o.2).collect();
    // The paragraph right above the options.
    let mut text: Vec<&str> = lines[..first].iter().rev().skip_while(|l| l.is_empty()).take_while(|l| !l.is_empty()).copied().collect();
    text.reverse();
    let text = if text.is_empty() { "Codex asks".to_string() } else { text.join(" ") };
    Some(Ask::question(vec![AskQuestion { text, options, multi: false, list_at: Some(at) }], true))
}

/// One word for where a session stands, and what it asks if it asks:
/// `working`, `waiting` (a permission), `question` (a question that holds its
/// turn), `background` (its turn is over, a job it started still runs),
/// `idle`, `starting` (its CLI has not drawn yet), `exited`.
pub fn state_of(id: &str) -> (&'static str, Option<Ask>) {
    use crate::activity::Turn;
    let pty = crate::pty::manager();
    if !live(pty, id) {
        return ("exited", None);
    }
    if still_starting(pty, id) {
        return ("starting", None);
    }
    let ask = ask_of(id);
    let ts = crate::activity::turn_state(id);
    let state = match (ts.map(|t| t.turn), &ask) {
        (_, Some(a)) if a.blocking && a.kind == "question" => "question",
        (Some(Turn::Waiting), _) => "waiting",
        (Some(Turn::Busy), _) => "working",
        _ if ts.is_some_and(|t| t.children_busy) || waits_on_helpers(id, "") => "background",
        _ => "idle",
    };
    (state, ask)
}

/// An answer to one question as the CLI is to get it: an option's number
/// (several, apart by commas, where more than one may be picked) or label
/// becomes that option's label; anything else is the answer in its own words.
pub fn resolve_option(q: &AskQuestion, answer: &str) -> String {
    let pick = |part: &str| -> Option<String> {
        let part = part.trim();
        if let Ok(n) = part.parse::<usize>() {
            return q.options.get(n.checked_sub(1)?).cloned();
        }
        q.options.iter().find(|o| o.eq_ignore_ascii_case(part)).cloned()
    };
    if let Some(o) = pick(answer) {
        return o;
    }
    if q.multi {
        let parts: Option<Vec<String>> = answer.split(',').map(pick).collect();
        if let Some(p) = parts.filter(|p| !p.is_empty()) {
            return p.join(", ");
        }
    }
    answer.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::registry::reg;

    /// Two subagents on permission prompts at once: each answer settles its
    /// own, the turn's tools settle none of theirs.
    #[test]
    fn subagents_hold_their_own_asks() {
        let chat = "asks-two";
        let ask = |agent: &str, cmd: &str| {
            on_hook(chat, &serde_json::json!({ "hook_event_name": "PermissionRequest", "agent_id": agent, "tool_name": "Bash", "tool_input": { "command": cmd } }).to_string())
        };
        let held = || {
            reg()
                .lock()
                .unwrap()
                .hook_asks
                .get(chat)
                .map(|h| h.iter().map(|(_, input, _)| input["command"].as_str().unwrap().to_string()).collect::<Vec<_>>())
        };
        ask("a", "rm a");
        ask("b", "rm b");
        on_hook(chat, r#"{"hook_event_name":"PostToolUse","tool_name":"Read"}"#);
        assert_eq!(held(), Some(vec!["rm a".to_string(), "rm b".to_string()]), "the turn's own tool answers neither");
        on_hook(chat, r#"{"hook_event_name":"PostToolUse","tool_name":"Bash","agent_id":"b"}"#);
        assert_eq!(held(), Some(vec!["rm a".to_string()]));
        on_hook(chat, r#"{"hook_event_name":"SubagentStop","agent_id":"a"}"#);
        assert_eq!(held(), None);
    }

    /// What a parent is asked to allow is never shown cut without a mark: not
    /// in `ask.detail`, not in the notice line.
    #[test]
    fn a_long_command_is_never_shown_cut_without_a_mark() {
        let bash = |cmd: &str| Ask::from_claude_tool("Bash", &serde_json::json!({ "command": cmd }));

        // Short: whole in both, as it was.
        let short = bash("ls -la");
        assert_eq!(short.detail.as_deref(), Some("ls -la"));
        assert_eq!(short.line(), "asks for permission to run Bash: ls -la");

        // Past the old 400 characters: whole in the detail, the line shortened
        // with a mark that says where the rest is.
        let tail = "; do rm -f \"$f\"; done # the end";
        let long = format!("for f in {}{tail}", "build/out/a.o ".repeat(80));
        let ask = bash(&long);
        assert_eq!(ask.detail.as_deref(), Some(long.as_str()), "whole");
        let line = ask.line();
        assert!(line.contains('…') && line.contains("luna_session_get") && line.contains("has it whole"), "{line}");
        assert!(!line.contains(tail) && line.chars().count() < 400, "short: {line}");
        assert_eq!(serde_json::to_value(&ask).unwrap()["detail"], long.as_str(), "luna_session_get shows it whole");

        // Past DETAIL_MAX: cut, ending in the mark with what is left out.
        let huge = format!("echo {}", "x".repeat(20_000));
        let ask = bash(&huge);
        let detail = ask.detail.clone().unwrap();
        let left = huge.len() - DETAIL_MAX;
        assert!(detail.starts_with(&huge[..DETAIL_MAX]), "the head kept whole");
        assert!(detail.ends_with(&format!("… [{left} more characters cut]")), "{}", &detail[DETAIL_MAX..]);
        assert_eq!(ask.detail_cut, left);
        let line = ask.line();
        assert!(line.contains('…') && line.contains("first 8 KB, cut there with a mark"), "{line}");

        // Padding cannot push the end out of the part the line shows.
        let padded = format!("ls{}rm -rf ~", " ".repeat(5000));
        let line = bash(&padded).line();
        assert!(line.contains("ls rm -rf ~ [shortened;"), "{line}");

        // A Codex approval goes the same way.
        let codex = Ask::permission(None, Some(huge.clone()));
        assert_eq!(codex.detail, ask.detail);
        assert!(codex.line().contains("cut there with a mark"));
    }

    /// The cap cuts on a character's boundary and counts characters, not bytes.
    #[test]
    fn multi_byte_text_at_the_cap() {
        let e = "é".repeat(DETAIL_MAX / 2);
        assert_eq!(e.len(), DETAIL_MAX);
        let at_cap = Ask::permission(None, Some(e.clone()));
        assert_eq!((at_cap.detail.as_deref(), at_cap.detail_cut), (Some(e.as_str()), 0), "exactly at the cap: whole");
        let over = Ask::permission(None, Some(format!("{e}é")));
        assert_eq!(over.detail, Some(format!("{e}… [1 more character cut]")));

        // Four-byte characters that straddle the cap: the last whole one is kept.
        let emoji = format!("a{}", "😀".repeat(3000));
        let ask = Ask::permission(None, Some(emoji.clone()));
        let kept = (DETAIL_MAX - 1) / 4;
        assert_eq!(ask.detail, Some(format!("a{}… [{} more characters cut]", "😀".repeat(kept), 3000 - kept)));
        assert_eq!(ask.detail_cut, 3000 - kept);
        let line = ask.line();
        assert!(line.contains("😀…") && line.contains("cut there with a mark"), "{line}");
    }

    /// Who asks, where Claude Code says: a hook names a subagent's id and
    /// type, and its absence is the session's own call.
    #[test]
    fn an_ask_names_who_in_the_session_made_the_call() {
        let bash = || Ask::from_claude_tool("Bash", &serde_json::json!({ "command": "rm -f x" }));
        let sub = bash().asked_by(Some(AskSource::of(Some("a1b2"), Some("worker"))));
        assert_eq!(sub.line(), "asks for permission to run Bash (a call of its subagent \"worker\", agentId a1b2): rm -f x");
        assert_eq!(
            serde_json::to_value(&sub).unwrap()["source"],
            serde_json::json!({ "by": "subagent", "agentId": "a1b2", "agentType": "worker" })
        );
        let own = bash().asked_by(Some(AskSource::of(Some(""), None)));
        assert_eq!(own.line(), "asks for permission to run Bash (its own call): rm -f x");
        assert_eq!(serde_json::to_value(&own).unwrap()["source"], serde_json::json!({ "by": "session" }));
        assert!(serde_json::to_value(bash()).unwrap().get("source").is_none(), "not said where nothing says it");

        let chat = "asks-source";
        on_hook(
            chat,
            &serde_json::json!({ "hook_event_name": "PermissionRequest", "agent_id": "a9", "agent_type": "Explore", "tool_name": "Bash", "tool_input": { "command": "ls" } })
                .to_string(),
        );
        let by = reg().lock().unwrap().hook_asks[chat][0].2.clone();
        assert_eq!(by, AskSource::subagent(Some("a9"), Some("Explore")));
        on_hook(chat, r#"{"hook_event_name":"SessionEnd"}"#);
    }

    /// A Codex approval's command is read from every row it is drawn on, not
    /// the `$ ` row alone.
    #[test]
    fn a_codex_command_is_read_whole_off_its_rows() {
        let one = "  Would you like to run the following command?\n\n  $ ls -la\n\n› 1. Yes, proceed (y)\n  2. No (esc)";
        assert_eq!(codex_command_on(one).as_deref(), Some("ls -la"));
        let wrapped = "  Would you like to run the following command?\n\n  $ for f in build/*.o; do\n  rm -f \"$f\"; done\n\n  1. Yes, proceed (y)\n› 2. No (esc)";
        assert_eq!(codex_command_on(wrapped).as_deref(), Some("for f in build/*.o; do\nrm -f \"$f\"; done"));
        assert_eq!(codex_command_on("  $ rm -rf build\n  and more not yet drawn"), None, "not a dialog");
    }

    const CODEX_HEAD: &str = "  Would you like to run the following command?\n\n";
    const CODEX_OPTIONS: &str = "\n› 1. Yes, proceed (y)\n  2. Yes, and don't ask again (p)\n  3. No (esc)";

    /// A heredoc line that looks like option 1 does not end the command.
    #[test]
    fn a_numbered_row_of_the_command_is_not_its_end() {
        let screen = format!("{CODEX_HEAD}  $ cat > notes.md <<'EOF'\n  1. step\n  2. step\n  EOF\n  rm -rf ~\n{CODEX_OPTIONS}");
        assert_eq!(codex_command_on(&screen).as_deref(), Some("cat > notes.md <<'EOF'\n1. step\n2. step\nEOF\nrm -rf ~"));
        // Even where the command's own rows look like the whole option block.
        let tricky = format!("{CODEX_HEAD}  $ cat <<'EOF'\n  › 1. Yes, proceed (y)\n  2. No (esc)\n  EOF\n  rm -rf ~\n{CODEX_OPTIONS}");
        assert_eq!(codex_command_on(&tricky).as_deref(), Some("cat <<'EOF'\n› 1. Yes, proceed (y)\n2. No (esc)\nEOF\nrm -rf ~"));
    }

    /// A `$ ` row above the dialog, in what Codex showed before, is not its command.
    #[test]
    fn a_stale_dollar_row_above_the_dialog_is_not_the_command() {
        let screen = format!("  • Ran\n  $ git status\n  On branch main\n\n{CODEX_HEAD}  $ ls -la\n{CODEX_OPTIONS}");
        assert_eq!(codex_command_on(&screen).as_deref(), Some("ls -la"));
        // A `$ ` row in the command is not its start.
        let inner = format!("{CODEX_HEAD}  $ cat <<'EOF'\n  $ not the start\n  EOF\n{CODEX_OPTIONS}");
        assert_eq!(codex_command_on(&inner).as_deref(), Some("cat <<'EOF'\n$ not the start\nEOF"));
        // A heading with no command under it: none.
        assert_eq!(codex_command_on(&format!("{CODEX_HEAD}{CODEX_OPTIONS}")), None);
        // Two dialogs on the screen: said, and the earlier one's rows come too.
        let two = format!("{CODEX_HEAD}  $ echo one\n{CODEX_OPTIONS}\n\n{CODEX_HEAD}  $ echo two\n{CODEX_OPTIONS}");
        let read = codex_command_on(&two).unwrap();
        assert!(read.starts_with(CODEX_TWO_DIALOGS) && read.contains("echo one") && read.contains("echo two"), "{read}");
    }

    /// A command taller than the screen is said not to be whole, whichever end is off.
    #[test]
    fn a_codex_command_taller_than_the_screen_says_so() {
        let end_off = format!("{CODEX_HEAD}  $ echo 1\n  echo 2\n  echo 3");
        assert_eq!(codex_command_on(&end_off), Some(format!("echo 1\necho 2\necho 3\n{CODEX_END_OFF}")));
        let start_off = "  echo 98\n  echo 99\n  rm -rf ~\n\n› 1. Yes, proceed (y)\n  2. No (esc)";
        assert_eq!(codex_command_on(start_off).as_deref(), Some(CODEX_START_OFF));
        let read = codex_command_on(start_off).unwrap();
        assert!(read.contains("could not be read whole"), "{read}");
        // The edits dialog, and a numbered list in the conversation, are not commands.
        let edits = "  Would you like to make the following edits?\n  src/a.rs\n\n› 1. Yes, proceed (y)\n  2. No (esc)";
        assert_eq!(codex_command_on(edits), None);
        assert_eq!(codex_command_on("  Steps:\n  1. one\n  2. two\n\n› "), None);
    }

    /// A line break in a detail shows in the notice, not as a space.
    #[test]
    fn the_notice_shows_a_line_break_as_one() {
        let bash = |c: &str| Ask::from_claude_tool("Bash", &serde_json::json!({ "command": c }));
        assert_eq!(bash("ls # list\nrm -rf ~\n").line(), "asks for permission to run Bash: ls # list ⏎ rm -rf ~");
        assert_eq!(bash("a\r\nb\rc\u{2028}d").line(), "asks for permission to run Bash: a ⏎ b ⏎ c ⏎ d");
        // Past the line's length too, where whitespace is collapsed.
        let long = format!("ls # list{}\nrm -rf ~\n\n{}", " ".repeat(300), "x".repeat(300));
        let line = bash(&long).line();
        assert!(line.contains("ls # list ⏎ rm -rf ~ ⏎ xxx") && line.contains("[shortened;"), "{line}");
        assert!(!line.contains('\n') && !line.contains("list rm"), "{line}");
    }

    /// `detailCut` tells a cut detail from a whole one that ends in the same words.
    #[test]
    fn a_cut_detail_says_how_much_in_a_field() {
        let whole = Ask::permission(None, Some("echo '… [5 more characters cut]'".into()));
        assert!(serde_json::to_value(&whole).unwrap().get("detailCut").is_none());
        let cut = Ask::permission(None, Some("x".repeat(DETAIL_MAX + 7)));
        assert_eq!(serde_json::to_value(&cut).unwrap()["detailCut"], 7);
    }
}
