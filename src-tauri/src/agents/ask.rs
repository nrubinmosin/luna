//! What a session waits on a person for (`Ask`): a question its model put or
//! leave to run a tool, from Claude Code's hooks or off a Codex screen, and the
//! state `list` and `wait` report with it.

use crate::agents::delivery::still_starting;
use crate::agents::read::transcript_path;
use crate::agents::registry::{reg, waits_on_helpers};
use crate::provider::Provider;
use serde::Serialize;

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
    /// A permission's tool, and what it would do with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
        Ask { id: None, kind: "question", blocking, questions, tool: None, detail: None }
    }

    pub fn permission(tool: Option<String>, detail: Option<String>) -> Ask {
        Ask { id: None, kind: "permission", blocking: true, questions: Vec::new(), tool, detail }
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
    /// field of its input that says what it would do.
    pub fn from_claude_tool(name: &str, input: &serde_json::Value) -> Ask {
        let detail = ["command", "file_path", "path", "url", "pattern", "description"]
            .iter()
            .find_map(|k| input[*k].as_str())
            .map(str::to_owned)
            .or_else(|| input.is_object().then(|| input.to_string()))
            .map(|d| d.chars().take(400).collect());
        Ask::permission(Some(name.to_string()), detail)
    }

    /// One line for a notice: the questions with their options numbered.
    pub fn line(&self) -> String {
        if self.kind == "permission" {
            let what = [self.tool.as_deref(), self.detail.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(": ");
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
    let agent = v["agent_id"].as_str().filter(|a| !a.is_empty()).map(str::to_string);
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
    held.retain(|(_, _, asker)| *asker != agent);
    if raises {
        held.push((tool.to_string(), v["tool_input"].clone(), agent));
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
                .map(|(name, input, _)| (name.clone(), input.clone()));
            let (name, input) = hooked
                .filter(|_| turn.is_some_and(|t| t != crate::activity::Turn::Idle))
                .or_else(|| waiting.then(|| crate::claude::session::pending_tool(&transcript_path(pty, id)?)).flatten())?;
            if name == "AskUserQuestion" {
                Ask::from_claude_questions(&input)
            } else {
                waiting.then(|| Ask::from_claude_tool(&name, &input))
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

/// The command a Codex approval dialog shows, on its `$ ` line.
pub(super) fn codex_command_on(screen: &str) -> Option<String> {
    screen.lines().map(str::trim).find_map(|l| l.strip_prefix("$ ")).map(|c| c.trim().to_string()).filter(|c| !c.is_empty())
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
}
