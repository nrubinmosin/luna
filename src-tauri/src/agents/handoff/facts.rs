//! What Luna says in a handoff: the notices that ask a chat for one, with the
//! procedure and the handoff's sections, and the prompt that opens the next
//! conversation, the session's handoff pointed at and the facts Luna knows
//! itself written out: its helpers, what they owe it, what it sent that has not
//! gone in, the accounts and their use, the chat's own settings. The session
//! writes judgment; the facts come from Luna, so they cannot go stale in a file.

use crate::provider::Provider;
use serde_json::Value;

/// How the continuation prompt opens: what tells it from a person's words, and
/// a chat's name from its first prompt (pty/probe.rs `session_meta`).
pub const PROMPT_TAG: &str = "[Luna handoff";

/// The opening of handoff `n`'s prompt, which its transcript is searched for.
pub(super) fn prompt_opening(n: u32) -> String {
    format!("{PROMPT_TAG} {n}, not typed by the user]")
}

/// What a handoff holds, section by section: what the next conversation needs
/// of this one's judgment.
pub(super) const SECTIONS: &str = "objective, authorized scope and explicit stop conditions; completed phases and the work \
that remains; decisions and their reasons, approaches rejected; approvals given, questions pending, risks unresolved; \
repositories and worktrees, branches and SHAs, uncommitted changes; helpers: ids, what each was asked, the answers \
expected, report paths; checks run and their results, claims not checked; the next concrete action, with the evidence \
and transcript references it needs";

/// The threshold notice's words, the notifier's tag aside: where the context
/// stands, then what to do. `firm`: the second, near the point where the CLI
/// compacts the conversation on its own.
pub(super) fn notice(pct: u64, threshold: u8, firm: bool) -> String {
    let lead = if firm {
        format!(
            "This chat's context is at {pct}%, near where its CLI compacts the conversation on its own, which keeps far less than a \
             handoff: hand off now, before your next step (auto-handoff is set at {threshold}%)."
        )
    } else {
        format!("This chat's context is at {pct}% (auto-handoff at {threshold}%): hand off to a fresh conversation at the next point where your work can pause.")
    };
    format!(
        "{lead} 1) Write a handoff for a successor that knows nothing of this conversation: a Markdown file (.md, under {} KB, \
         outside Luna's folder) with these sections: {SECTIONS}. Write meaning and priorities; Luna adds the live facts \
         itself (your helpers and their state, the answers they owe you, accounts and their use, this chat's settings). \
         2) Let your CLI's own background jobs and subagents finish, or stop them: they cannot cross over. Luna helpers \
         do, ids unchanged. 3) Call luna_session_handoff with file set to its absolute path, then end your turn. Luna \
         then clears this conversation and starts the next one in this chat from your file. Nothing changes until you \
         call it.",
        super::file::MAX_BYTES / 1024
    )
}

/// Whether a line queued for a chat's notice is a threshold notice: one not
/// yet typed when a handoff is scheduled is dropped, the new conversation
/// having nothing to hand off.
pub(super) fn is_notice(line: &str) -> bool {
    line.starts_with("This chat's context is at ")
}

/// One helper of the chat's, as the fact block names it.
#[derive(Default, Debug, Clone)]
pub(super) struct Helper {
    pub id: String,
    pub name: String,
    pub account: String,
    pub state: String,
    pub task: Option<String>,
    pub worktree: Option<String>,
    pub branch: Option<String>,
    pub sha: Option<String>,
    /// It owes the chat an answer.
    pub owes: bool,
    /// Messages the chat sent it that never went in.
    pub undelivered: usize,
}

/// What Luna knows of a chat as its handoff goes through.
#[derive(Default, Debug, Clone)]
pub(super) struct Facts {
    pub provider: Option<Provider>,
    pub account: String,
    pub folder: String,
    pub settings: Value,
    pub threshold: Option<u8>,
    pub helpers: Vec<Helper>,
    /// Sessions not its helpers that owe it an answer: ones it wrote to.
    pub owed_by_others: Vec<String>,
    /// Sessions its text waits to be typed into, behind a person's draft.
    pub held_for: Vec<String>,
    /// `provider/name` and its use, a line each.
    pub accounts: Vec<(String, String)>,
    /// When, as an instant.
    pub at: String,
}

/// Plain text on one line: a name or a task with a control character in it
/// is no reason for the prompt to fail its check.
fn plain(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

/// The chat's settings as words: the flags it runs with, not what Luna keeps
/// beside them.
fn settings_words(s: &Value) -> String {
    const SHOWN: [&str; 8] = ["model", "effort", "permissionMode", "approval", "sandbox", "worktree", "readOnly", "agent"];
    let words: Vec<String> = SHOWN
        .iter()
        .filter_map(|k| {
            let v = s.get(*k).filter(|v| !v.is_null() && **v != Value::Bool(false))?;
            Some(match v {
                Value::String(t) => format!("{k} {}", plain(t)),
                other => format!("{k} {other}"),
            })
        })
        .collect();
    if words.is_empty() {
        "its account's defaults".into()
    } else {
        words.join(", ")
    }
}

fn helper_line(h: &Helper) -> String {
    let name = plain(&h.name);
    let named = if name.is_empty() { h.id.clone() } else { format!("{} \"{name}\"", h.id) };
    let mut parts = vec![format!("{named} on {}: {}", plain(&h.account), h.state)];
    if let Some(t) = &h.task {
        parts.push(format!("task: {}", plain(t)));
    }
    if let Some(w) = &h.worktree {
        let at = match (&h.branch, &h.sha) {
            (Some(b), Some(s)) => format!(", branch {b} at {s}"),
            (Some(b), None) => format!(", branch {b}"),
            (None, Some(s)) => format!(" at {s}"),
            (None, None) => String::new(),
        };
        parts.push(format!("worktree {w}{at}"));
    }
    if h.owes {
        parts.push("owes you an answer".into());
    }
    if h.undelivered > 0 {
        parts.push(format!("{} message(s) you sent it never went in", h.undelivered));
    }
    format!("  - {}", parts.join("; "))
}

/// The facts as the prompt carries them, a line each.
pub(super) fn fact_block(f: &Facts) -> String {
    let who = match f.provider {
        Some(Provider::Codex) => "Codex",
        _ => "Claude Code",
    };
    let mut lines = vec![format!("What Luna knows now ({}):", f.at)];
    let handoff = f.threshold.map(|t| format!("; auto-handoff at {t}%")).unwrap_or_default();
    lines.push(format!(
        "- This chat: {who} on account \"{}\", folder {}; {}{handoff}.",
        plain(&f.account),
        plain(&f.folder),
        settings_words(&f.settings)
    ));
    if f.helpers.is_empty() {
        lines.push("- Your helpers: none.".into());
    } else {
        lines.push(
            "- Your helpers, ids unchanged (luna_session_wait, luna_report_get and notices reach them as before; luna_session_list has more):".into(),
        );
        lines.extend(f.helpers.iter().map(helper_line));
    }
    if !f.owed_by_others.is_empty() {
        lines.push(format!("- Sessions you wrote to that still owe you an answer: {}.", f.owed_by_others.join(", ")));
    }
    if !f.held_for.is_empty() {
        lines.push(format!(
            "- Text of yours Luna holds behind a person's draft, to go in once it is sent or cleared: to {}.",
            f.held_for.join(", ")
        ));
    }
    if f.accounts.is_empty() {
        lines.push("- Accounts: no use known yet (luna_account_list).".into());
    } else {
        let accounts: Vec<String> = f.accounts.iter().map(|(a, u)| format!("{a}: {u}")).collect();
        lines.push(format!("- Accounts: {}.", accounts.join("; ")));
    }
    lines.join("\n")
}

/// The prompt that opens handoff `n`'s conversation: the snapshot to read
/// first, the conversation before for a detail it lacks, and the facts, or
/// where they are when they did not fit (`facts_file`).
#[allow(clippy::too_many_arguments)]
pub(super) fn prompt(
    n: u32,
    chat: &str,
    name: Option<&str>,
    snapshot: &str,
    sha256: &str,
    source: &str,
    previous: Option<&str>,
    facts: &str,
) -> String {
    let named = name.map(|n| format!("\"{}\" ({chat})", plain(n))).unwrap_or_else(|| chat.to_string());
    let before = previous.map(|p| format!(" The conversation before, for a detail the handoff lacks: {p}.")).unwrap_or_default();
    let short: String = sha256.chars().take(16).collect();
    format!(
        "{} This is a new conversation of chat {named}: the one before it wrote a handoff and called \
         luna_session_handoff. Read the whole handoff first, at {snapshot} (Luna's copy of {source}, sha256 {short}…), \
         then go on from its next action.{before}\n{facts}",
        prompt_opening(n),
    )
}

/// The line into the chat when a handoff is given up. Before the reset took,
/// the conversation it is in is the one that asked: it goes on. After, it is
/// the new one, which has to hear what it is and where to start.
pub(super) fn failed_line(n: u32, why: &str, snapshot: &str, reset: bool, prompt: &str) -> String {
    if reset {
        format!("Handoff {n} cleared the conversation, but Luna's prompt for the new one did not go in ({why}). It was:\n{prompt}")
    } else {
        format!(
            "Handoff {n} did not go through: {why}. This conversation goes on as it was; your handoff's copy is at \
             {snapshot}. Call luna_session_handoff again once what stood in the way is done."
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_carries_the_procedure_and_every_section() {
        let first = notice(71, 70, false);
        let firm = notice(91, 70, true);
        for text in [&first, &firm] {
            for word in ["luna_session_handoff", "objective", "stop conditions", "next concrete action", ".md", "end your turn"] {
                assert!(text.contains(word), "{word}: {text}");
            }
            assert!(text.chars().all(|c| !c.is_control()), "one line, as a notice is");
        }
        assert!(first.contains("71%") && first.contains("70%"));
        assert!(firm.contains("hand off now") && firm.contains("91%"));
        assert!(is_notice(&first) && is_notice(&firm) && !is_notice("Your helper h1 answered"));
    }

    #[test]
    fn the_prompt_points_at_the_copy_and_states_the_facts() {
        let facts = Facts {
            provider: Some(Provider::Claude),
            account: "backend".into(),
            folder: "E:\\work".into(),
            settings: serde_json::json!({ "model": "opus", "effort": "high", "worktree": false, "budgetTold": true }),
            threshold: Some(70),
            helpers: vec![Helper {
                id: "h1".into(),
                name: "tests\u{1b}[2J".into(),
                account: "team".into(),
                state: "working".into(),
                task: Some("Run the suite".into()),
                worktree: Some("E:\\wt".into()),
                branch: Some("feat/x".into()),
                sha: Some("abc1234".into()),
                owes: true,
                undelivered: 1,
            }],
            owed_by_others: vec!["c9".into()],
            held_for: vec!["c8".into()],
            accounts: vec![("anthropic/backend".into(), "5 hours 42%".into())],
            at: "2026-11-09T10:00:00Z".into(),
        };
        let block = fact_block(&facts);
        let p = prompt(
            2,
            "c1",
            Some("Campaign"),
            "D:\\luna\\handoffs\\c1\\2.md",
            "0123456789abcdef0123",
            "E:\\work\\h.md",
            Some("E:\\t\\old.jsonl"),
            &block,
        );
        assert!(p.starts_with("[Luna handoff 2, not typed by the user]"));
        for want in [
            "D:\\luna\\handoffs\\c1\\2.md",
            "0123456789abcdef…",
            "E:\\t\\old.jsonl",
            "model opus, effort high",
            "auto-handoff at 70%",
            "h1 \"tests [2J\" on team: working",
            "branch feat/x at abc1234",
            "owes you an answer",
            "1 message(s) you sent it never went in",
            "still owe you an answer: c9",
            "to c8",
            "anthropic/backend: 5 hours 42%",
        ] {
            assert!(p.contains(want), "{want}:\n{p}");
        }
        assert!(!p.contains("budgetTold"), "what Luna keeps beside the flags is not a setting");
        crate::agents::privilege::plain_text(&p).expect("a prompt Luna types passes its own check");
    }
}
