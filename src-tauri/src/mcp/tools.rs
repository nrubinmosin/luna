//! What `tools/list` offers: sixteen tools named `luna_<resource>_<verb>`. A
//! resource is read as a `list` of compact rows (filters, pages) or a `get` of
//! one item whole; anything else is a verb. Every word here lands in a tooled
//! session's context once, so each description says what a model needs to
//! call it and no more; `luna_guide_get` has the rest, by topic.

use super::guide::TOPICS;
use crate::agents;
use serde_json::{json, Value};

/// The names Luna offered before 0.11. Called, each is refused with the tools
/// there are now: no stand-in answers for it.
pub(super) const REMOVED: [&str; 12] = [
    "luna_list",
    "luna_sessions",
    "luna_spawn",
    "luna_send",
    "luna_answer",
    "luna_read",
    "luna_wait",
    "luna_kill",
    "luna_delete",
    "luna_guide",
    "luna_notify",
    "luna_power",
];

pub(super) fn tools() -> Value {
    let s = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required });
    // What a client may assume of each tool without asking the user. All of
    // them stay inside this Luna.
    let reads = json!({ "readOnlyHint": true, "openWorldHint": false });
    let acts = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let ends = json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    // Past the caller's own helpers only when asked (`agents::in_scope`).
    let scope = json!({ "type": "string", "enum": ["mine", "all"] });
    let string = json!({ "type": "string" });
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let flag = json!({ "type": "boolean" });
    let count = json!({ "type": "integer" });
    let provider = json!({ "type": "string", "enum": ["claude", "codex"] });
    // `requestKey`: a change a caller may retry without making it twice
    // (dispatch.rs `keyed`); the guide says so, once for the three.
    let key = json!({ "type": "string" });
    let topics: Vec<&str> = TOPICS.iter().map(|(topic, _)| *topic).collect();
    json!([
        {
            "name": "luna_session_list",
            "description": "Your sessions as compact rows (state, askId, turnsEnded, task, lastActivityAt); scope all: every one; ids: those (others' task, askId: scope all). Filters: state, provider, account, group (a number, I=1, or \"mine\"), q (name, folder, account, id). limit (50); nextCursor goes in cursor. places.",
            "inputSchema": s(json!({
                "scope": scope, "ids": strings, "state": strings, "provider": provider, "account": string,
                "group": { "anyOf": [{ "type": "integer" }, { "type": "string" }] }, "q": string, "limit": count, "cursor": string
            }), &[]),
            "annotations": reads
        },
        {
            "name": "luna_session_get",
            "description": "One session whole: its row with settings, folder, cwd, transcript, worktree, branch, sha, turnStartedAt, context, resolvedModel, cliVersion, leftRunning and ask (another's ask: scope all); a helper's worktree: lastCommit, status.",
            "inputSchema": s(json!({ "id": string, "scope": scope }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_session_spawn",
            "description": "Start a helper in the background; the user can watch it, or open it as a chat, which takes it from you. Omitted: your mode or nearest below (modeFrom), else the account's defaults (luna_account_get). Returns id, settings, turnsEnded, sameFolder, worktree, accountChoice, places.",
            "inputSchema": s(json!({
                "prompt": string,
                "provider": provider,
                "account": { "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }], "description": "default: yours; a list: the first with room" },
                "model": { "type": "string", "description": "claude also a full id" },
                "effort": string,
                "permissionMode": { "type": "string", "description": "claude: default|acceptEdits|plan|auto|dontAsk|bypassPermissions" },
                "approval": { "type": "string", "description": "codex: never|auto-review" },
                "sandbox": { "type": "string", "description": "codex: read-only|read-only-net|workspace-write|danger-full-access" },
                "readOnly": { "type": "boolean", "description": "claude: no edit tools, Bash still writes; codex: read-only sandbox" },
                "agent": { "type": "string", "description": "claude: its --agent" },
                "folder": { "type": "string", "description": "default: yours" },
                "worktree": { "type": "boolean", "description": "its own git worktree" },
                "name": string,
                "tools": { "type": "boolean", "description": "these tools too (one level)" },
                "autoDelete": { "type": "boolean", "description": "default true: deleted 30 min after its answer reached you" },
                "resumeOnRestart": flag,
                "budgetTokens": { "type": "integer", "description": "context tokens; past them you hear, once" },
                "connectors": { "type": "boolean", "description": "its account's MCP servers and connectors too" },
                "requestKey": key
            }), &["prompt"]),
            "annotations": acts
        },
        {
            "name": "luna_session_send",
            "description": "Send a prompt to a session you spawned (scope all: any, opened \"[From session <you> via Luna]\"), read as its user's. Returns stateBefore, queued (it was working), joined (a Claude helper takes it at its next tool), held (a user's draft: typed in later), started, turnsEnded.",
            "inputSchema": s(json!({
                "id": string, "text": string,
                "interrupt": { "type": "boolean", "description": "stop its turn first (yours only)" },
                "scope": scope, "requestKey": key
            }), &["id", "text"]),
            "annotations": acts
        },
        {
            "name": "luna_session_answer",
            "description": "Answer what a session waits on (state question or waiting; askId from its row): answers, one per question (an option's number or label, or your own words); allow true/false for your helper's permission, note: why not. Another's model question: scope all.",
            "inputSchema": s(json!({ "id": string, "askId": string, "answers": strings, "allow": flag, "note": string, "scope": scope }), &["id"]),
            "annotations": acts
        },
        {
            "name": "luna_session_wait",
            "description": "Block until a session (yours; scope all: any) ends a turn, stops on a question or permission, or exits; or end your turn instead: a notice brings it. reply: report (default: rest says how many more, file has all), all, none. ids: several, mode any|all. afterTurn: past that turnsEnded. screen: the terminal too.",
            "inputSchema": s(json!({
                "id": string, "ids": strings,
                "mode": { "type": "string", "enum": ["any", "all"] },
                "until": { "type": "string", "enum": ["turn_done", "waiting", "exit"] },
                "timeoutSec": { "type": "integer", "description": "default 600, max 1800" },
                "afterTurn": count,
                "reply": { "type": "string", "enum": ["report", "all", "none"] },
                "screen": flag, "scope": scope
            }), &[]),
            "annotations": reads
        },
        {
            "name": "luna_session_stop",
            "description": "Stop your helper's turn (no autoDelete after; a send goes on); between turns, what its turns left running.",
            "inputSchema": s(json!({ "id": string }), &["id"]),
            "annotations": ends
        },
        {
            "name": "luna_session_delete",
            "description": "Stop and remove your helpers, id or ids; one gone already answers already. dropWorktree: its worktree and worktree-* branch too, unless another session is in it; refused if that loses work, unless force.",
            "inputSchema": s(json!({ "id": string, "ids": strings, "dropWorktree": flag, "force": flag, "requestKey": key }), &[]),
            "annotations": ends
        },
        {
            "name": "luna_message_list",
            "description": "Messages from a session's transcript (yours; scope all: any). since: a cursor from before; last: the newest N (default 1; all after since). Past 60k chars, rest = how many left out. toolCalls: call markers (Claude). screen: the terminal.",
            "inputSchema": s(json!({ "id": string, "since": count, "last": count, "toolCalls": flag, "screen": flag, "scope": scope }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_report_get",
            "description": "A session's report alone (yours; scope all: any), of what it said since your question or since; rest: how many more, file: all. ack: collects the answer owed when this is all of it, so no notice follows. Starts its autoDelete count over.",
            "inputSchema": s(json!({ "id": string, "since": count, "ack": flag, "scope": scope }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_account_list",
            "description": "Accounts you may spawn on: provider, name, usage (% per window, resets).",
            "inputSchema": s(json!({ "provider": provider }), &[]),
            "annotations": reads
        },
        {
            "name": "luna_account_get",
            "description": "One account: a helper's defaults there (in folder, default yours), usage, and the models it may ask for.",
            "inputSchema": s(json!({ "name": string, "provider": provider, "folder": string }), &["name"]),
            "annotations": reads
        },
        {
            "name": "luna_run_list",
            "description": "Past spawns, newest first, deleted helpers too: id, at, ended, how, name, account, prompt and last reply (cut), transcript. Yours and your helpers'; scope all: everyone's. q searches names and prompts; limit (10).",
            "inputSchema": s(json!({ "q": string, "limit": count, "scope": scope }), &[]),
            "annotations": reads
        },
        {
            "name": "luna_guide_get",
            "description": "How Luna works, a topic at a time: a tool's name, notices, restart, helpers. None: the topics.",
            "inputSchema": s(json!({ "topic": { "type": "string", "enum": topics } }), &[]),
            "annotations": reads
        },
        {
            "name": "luna_notification_send",
            "description": "Desktop notification (person's chats only), for a result or blocker the user must see now, not progress. One per 10 s.",
            "inputSchema": s(json!({ "text": string }), &["text"]),
            "annotations": acts
        },
        {
            "name": "luna_power_set",
            "description": "Person's chats, when the user asked: shut down, restart, hibernate or sleep the PC once Luna is quiet for quietSec (default: the user's), CLIs closed first; cancel disarms. drain: till then agents' spawns and sends are refused. Returns busy, waitingForAPerson.",
            "inputSchema": s(json!({
                "action": { "type": "string", "enum": ["shutdown", "restart", "hibernate", "sleep", "cancel"] },
                "quietSec": count, "drain": flag
            }), &["action"]),
            "annotations": ends
        }
    ])
}

/// The names of the tools there are, for an error that has to say them.
pub(super) fn names() -> Vec<String> {
    tools().as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect()
}

/// The tool a person's chat with Luna's tools is offered besides the rest,
/// and no other session (agents/handoff `may_list`). No target: it acts on
/// the caller's own chat, the one its token is. It answers only while the
/// chat is asked for a handoff: without `file` how to write it; with one it
/// hands off.
pub(super) fn handoff_tool() -> Value {
    json!({
        "name": "luna_session_handoff",
        "description": "Go on in a fresh conversation of this chat, from a handoff you wrote. Without file: how to write it, and where. With file (Markdown, absolute path): Luna copies it, waits for your turn to end, clears this conversation and opens the next with the file and the facts it knows (helpers, debts, accounts). Call it only when a notice from Luna asks this chat for a handoff (the person's request, or auto-handoff at its threshold); unasked it refuses. After a call with file, end your turn.",
        "inputSchema": { "type": "object", "properties": { "file": { "type": "string" } }, "required": [] },
        "annotations": { "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false }
    })
}

/// What a background Claude Code helper's CLI calls before a tool runs and
/// with every `AskUserQuestion` (`--permission-prompt-tool`): never the
/// model's to call, and offered only to such a helper.
fn permission_tool() -> Value {
    json!({
        "name": "luna_permission",
        "description": "Luna's permission handler for this session; never call it yourself.",
        "inputSchema": {
            "type": "object",
            "properties": { "tool_name": { "type": "string" }, "input": { "type": "object" }, "tool_use_id": { "type": "string" } },
            "required": ["tool_name", "input"]
        },
        "annotations": { "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false }
    })
}

/// The tools a caller is offered: Luna's own when it was given them, the
/// handoff when it is a person's chat with them, and the permission handler
/// when it is a background Claude Code helper. Nothing of Codex calls that
/// one: a Codex helper's model would only see a tool it must not use.
pub(super) fn tools_for(caller: &str) -> Value {
    let mut list: Vec<Value> = if agents::has_tools(caller) { tools().as_array().cloned().unwrap_or_default() } else { Vec::new() };
    if agents::handoff::may_list(caller) {
        list.push(handoff_tool());
        agents::handoff::listed(caller);
    }
    if crate::background::asks_through_luna(caller) {
        list.push(permission_tool());
    }
    Value::Array(list)
}
