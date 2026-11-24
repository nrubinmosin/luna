//! What `tools/list` offers: sixteen tools (a lead fourteen) named
//! `luna_<resource>_<verb>`. A resource is read as a `list` of compact rows
//! (filters, pages) or a `get` of one item whole; anything else is a verb. Every word here lands in a tooled
//! session's context once, so each description says what a model needs to
//! call it and no more; `luna_guide_get` has the rest, by topic. A caller is
//! offered only what it may use: a lead, whose reach stops at the sessions
//! under it, gets no `scope` and neither tool that reaches the person or the
//! PC (`tools_for`). Offering is not the check: the dispatch refuses them all
//! the same.

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

/// The tools only a person's chat may call (dispatch.rs `persons_chat_only`).
pub(super) const PERSONS_ONLY: [&str; 2] = ["luna_notification_send", "luna_power_set"];

/// Every tool, as a person's chat is offered them: what a call's arguments are
/// held to (args.rs) and what an error names, whoever made the call.
pub(super) fn tools() -> Value {
    tools_as(true)
}

/// The tools as a person's chat (`persons`) or a lead is offered them: a
/// lead's list has no tool of `PERSONS_ONLY`, no `scope` argument and no word
/// of what `scope: "all"` would reach.
pub(super) fn tools_as(persons: bool) -> Value {
    // A description in its person's-chat and its lead wording.
    let say = |chat: &'static str, lead: &'static str| if persons { chat } else { lead };
    let s = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required });
    // What a client may assume of each tool without asking the user. All of
    // them stay inside this Luna.
    let reads = json!({ "readOnlyHint": true, "openWorldHint": false });
    let acts = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let ends = json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    // Behind Claude Code's tool search in spite of the server's `alwaysLoad`
    // (pty/start.rs `claude_mcp_file`): tools a session seldom calls, looked
    // up the once it does. Those that drive a helper stay loaded.
    let rare = json!({ "anthropic/alwaysLoad": false });
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
    // A lead's list names no tool it is not offered, its guide topics included.
    let topics: Vec<&str> = TOPICS.iter().map(|(topic, _)| *topic).filter(|t| persons || !PERSONS_ONLY.contains(t)).collect();
    let mut list = json!([
        {
            "name": "luna_session_list",
            "description": say(
                "Your sessions as compact rows; scope all: every one; ids: those (others' task, askId: scope all). Filters: state, provider, account, group (a number, I=1, or \"mine\"), q (name, folder, account, id). limit (50); nextCursor goes in cursor. places.",
                "Your sessions as compact rows; ids: those. Filters: state, provider, account, group (a number, I=1, or \"mine\"), q (name, folder, account, id). limit (50); nextCursor goes in cursor. places."
            ),
            "inputSchema": s(json!({
                "scope": scope, "ids": strings, "state": strings, "provider": provider, "account": string,
                "group": { "anyOf": [{ "type": "integer" }, { "type": "string" }] }, "q": string, "limit": count, "cursor": string
            }), &[]),
            "annotations": reads
        },
        {
            "name": "luna_session_get",
            "description": say(
                "One session whole: its row with settings, folder, cwd, transcript, turnStartedAt, context, resolvedModel, cliVersion, leftRunning and ask (another's ask: scope all).",
                "One session whole: its row with settings, folder, cwd, transcript, turnStartedAt, context, resolvedModel, cliVersion, leftRunning and ask."
            ),
            "inputSchema": s(json!({ "id": string, "scope": scope }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_session_spawn",
            "description": "Start a background helper the user can watch or take over. Your CLI handles spawn approval. Mode: explicit, CLI config, then Claude Auto or Codex Auto-review (luna_account_get). For isolation, use `git worktree add`, pass it as folder, then remove it.",
            "inputSchema": s(json!({
                "prompt": string,
                "provider": provider,
                "account": { "anyOf": [{ "type": "string" }, { "type": "array", "items": { "type": "string" } }], "description": "default: yours; a list: the first with room" },
                "model": { "type": "string", "description": "claude also a full id" },
                "effort": string,
                "permissionMode": { "type": "string", "description": "claude: default|acceptEdits|plan|auto|dontAsk|bypassPermissions" },
                "approval": { "type": "string", "description": "codex: never|on-request|auto-review; what it asks comes to you" },
                "sandbox": { "type": "string", "description": "codex: read-only|read-only-net|workspace-write|danger-full-access" },
                "readOnly": { "type": "boolean", "description": "claude: no edit tools, Bash still writes; codex: read-only sandbox" },
                "agent": { "type": "string", "description": "claude: its --agent" },
                "folder": { "type": "string", "description": "default: yours" },
                "name": string,
                "tools": { "type": "boolean", "description": "these tools too (one level)" },
                "autoDelete": { "type": "boolean", "description": "default true: deleted 30 min after its answer reached you" },
                "resumeOnRestart": flag,
                "connectors": { "type": "boolean", "description": "its account's MCP servers and connectors too" },
                "constraints": { "type": "string", "description": "rules it and every session under it keep to, after yours; none removed" },
                "requestKey": key
            }), &["prompt"]),
            "annotations": acts
        },
        {
            "name": "luna_session_send",
            "description": say(
                "Send a prompt to a session you spawned (scope all: also the user's other chats, opened \"[From session <you> via Luna]\"; never another's helper), read as its user's.",
                "Send a prompt to a session you spawned, read as its user's."
            ),
            "inputSchema": s(json!({
                "id": string, "text": string,
                "interrupt": { "type": "boolean", "description": "stop its turn first (yours only)" },
                "expectReply": { "type": "boolean", "description": "false: no notice unless a status word or report" },
                "scope": scope, "requestKey": key
            }), &["id", "text"]),
            "annotations": acts
        },
        {
            "name": "luna_session_answer",
            "description": say(
                "Answer what a session waits on (state question or waiting; askId from its row): answers, one per question (an option's number or label, or your own words); allow true/false for your helper's permission, note: why not. Another chat's model question: scope all; never another's helper.",
                "Answer what your helper waits on (state question or waiting; askId from its row): answers, one per question (an option's number or label, or your own words); allow true/false for its permission, note: why not."
            ),
            "inputSchema": s(json!({ "id": string, "askId": string, "answers": strings, "allow": flag, "note": string, "scope": scope }), &["id"]),
            "annotations": acts
        },
        {
            "name": "luna_session_wait",
            "description": say(
                "Block until a session (yours; scope all: any) ends a turn, stops on a question or permission, or exits; or end your turn instead: a notice brings it. reply: report (default: rest says how many more, file has all), all, none. ids: several, mode any|all. afterTurn: past that turnsEnded. screen: the terminal too.",
                "Block until a session of yours ends a turn, stops on a question or permission, or exits; or end your turn instead: a notice brings it. reply: report (default: rest says how many more, file has all), all, none. ids: several, mode any|all. afterTurn: past that turnsEnded. screen: the terminal too."
            ),
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
            "description": "Stop and remove your helpers, id or ids; one gone already answers already. Luna removes no worktree: one you made for a helper is yours to remove.",
            "inputSchema": s(json!({ "id": string, "ids": strings, "requestKey": key }), &[]),
            "annotations": ends
        },
        {
            "name": "luna_message_list",
            "description": say(
                "Messages from a session's transcript (yours; scope all: any). since: a cursor from before; last: the newest N (default 1; all after since). Past 60k chars, rest = how many left out. toolCalls: call markers (Claude). screen: the terminal.",
                "Messages from the transcript of a session of yours. since: a cursor from before; last: the newest N (default 1; all after since). Past 60k chars, rest = how many left out. toolCalls: call markers (Claude). screen: the terminal."
            ),
            "inputSchema": s(json!({ "id": string, "since": count, "last": count, "toolCalls": flag, "screen": flag, "scope": scope }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_report_get",
            "description": say(
                "A session's report alone (yours; scope all: any), of what it said since your question or since; rest: how many more, file: all. ack: collects the answer owed when this is all of it, so no notice follows. Starts its autoDelete count over.",
                "The report alone of a session of yours, of what it said since your question or since; rest: how many more, file: all. ack: collects the answer owed when this is all of it, so no notice follows. Starts its autoDelete count over."
            ),
            "inputSchema": s(json!({ "id": string, "since": count, "ack": flag, "scope": scope }), &["id"]),
            "annotations": reads
        },
        {
            "name": "luna_account_list",
            "description": "Accounts you may spawn on: provider, name, usage (% per window, resets).",
            "inputSchema": s(json!({ "provider": provider }), &[]),
            "annotations": reads,
            "_meta": rare
        },
        {
            "name": "luna_account_get",
            "description": "Account usage, allowed models, and helper defaults for the given folder (default yours). modeFrom identifies the mode source.",
            "inputSchema": s(json!({ "name": string, "provider": provider, "folder": string }), &["name"]),
            "annotations": reads,
            "_meta": rare
        },
        {
            "name": "luna_run_list",
            "description": say(
                "Past spawns, newest first, deleted helpers too, with prompt, last reply and transcript. Yours and your helpers'; scope all: everyone's. q searches names and prompts; limit (10).",
                "Past spawns, newest first, deleted helpers too, with prompt, last reply and transcript: yours and your helpers'. q searches names and prompts; limit (10)."
            ),
            "inputSchema": s(json!({ "q": string, "limit": count, "scope": scope }), &[]),
            "annotations": reads,
            "_meta": rare
        },
        {
            "name": "luna_guide_get",
            "description": "How Luna works, a topic at a time: a tool's name, notices, restart, helpers. None: the topics.",
            "inputSchema": s(json!({ "topic": { "type": "string", "enum": topics } }), &[]),
            "annotations": reads,
            "_meta": rare
        },
        {
            "name": "luna_notification_send",
            "description": "Desktop notification (person's chats only), for a result or blocker the user must see now, not progress. One per 10 s.",
            "inputSchema": s(json!({ "text": string }), &["text"]),
            "annotations": acts,
            "_meta": rare
        },
        {
            "name": "luna_power_set",
            "description": "Person's chats, when the user asked: shut down, restart, hibernate or sleep the PC once no session is active for quietSec (default: the user's), CLIs closed first (not for sleep/hibernate); cancel disarms. drain: till then agents' spawns and sends are refused.",
            "inputSchema": s(json!({
                "action": { "type": "string", "enum": ["shutdown", "restart", "hibernate", "sleep", "cancel"] },
                "quietSec": count, "drain": flag
            }), &["action"]),
            "annotations": ends,
            "_meta": rare
        }
    ]);
    if let (false, Some(all)) = (persons, list.as_array_mut()) {
        all.retain(|t| !PERSONS_ONLY.iter().any(|n| t["name"] == *n));
        for t in all.iter_mut() {
            if let Some(props) = t["inputSchema"]["properties"].as_object_mut() {
                props.remove("scope");
            }
        }
    }
    list
}

/// The names of the tools a person's chat (`persons`) or a lead is offered,
/// for an error that has to say them.
pub(super) fn names_as(persons: bool) -> Vec<String> {
    tools_as(persons).as_array().into_iter().flatten().filter_map(|t| t["name"].as_str().map(str::to_owned)).collect()
}

/// The names offered by Luna, also used when configuring the CLIs.
pub(crate) fn names() -> Vec<String> {
    names_as(true)
}

/// The tools a caller is offered: Luna's own when it was given them, all of
/// them to a person's chat (and the dev caller), a lead's share to any other
/// (`tools_as`). A background Claude Code helper's permission prompts come on
/// its own stdout (`--permission-prompt-tool stdio`), not through here.
pub(super) fn tools_for(caller: &str) -> Value {
    if agents::has_tools(caller) {
        tools_as(agents::is_persons_chat(caller))
    } else {
        Value::Array(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RARE: [&str; 6] =
        ["luna_run_list", "luna_account_get", "luna_account_list", "luna_guide_get", "luna_power_set", "luna_notification_send"];

    fn names_in(list: &Value) -> Vec<&str> {
        list.as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect()
    }

    /// A person's chat is offered every tool, and only the rare ones behind
    /// Claude Code's tool search: what drives a helper, `luna_report_get`
    /// after each notice among it, stays loaded.
    #[test]
    fn a_chat_gets_every_tool_and_only_the_rare_ones_deferred() {
        let list = tools_as(true);
        assert_eq!(list, tools());
        assert_eq!(names_in(&list).len(), 16);
        for t in list.as_array().unwrap() {
            let name = t["name"].as_str().unwrap();
            if RARE.contains(&name) {
                assert_eq!(t["_meta"], json!({ "anthropic/alwaysLoad": false }), "{name}");
            } else {
                assert!(t.get("_meta").is_none(), "{name} is to stay loaded");
            }
        }
        assert!(list.to_string().contains("scope all"));
    }

    /// A lead is offered the tools it may use: neither of a person's chat's
    /// own, and no word of a scope, in a schema, a description or a topic.
    #[test]
    fn a_lead_gets_no_persons_tool_and_no_scope() {
        let list = tools_as(false);
        let names = names_in(&list);
        assert_eq!(names.len(), 14, "{names:?}");
        assert!(PERSONS_ONLY.iter().all(|n| !names.contains(n)), "{names:?}");
        let text = list.to_string();
        assert!(!text.contains("scope"), "{text}");
        assert!(PERSONS_ONLY.iter().all(|n| !text.contains(n)), "{text}");
        // The same tools otherwise, in the same order, deferred alike.
        let chat = tools_as(true);
        let kept: Vec<&Value> = chat.as_array().unwrap().iter().filter(|t| !PERSONS_ONLY.iter().any(|n| t["name"] == *n)).collect();
        for (lead, chat) in list.as_array().unwrap().iter().zip(kept) {
            assert_eq!(lead["name"], chat["name"]);
            assert_eq!(lead["_meta"], chat["_meta"], "{}", lead["name"]);
            assert_eq!(lead["annotations"], chat["annotations"], "{}", lead["name"]);
        }
    }

    /// Who is offered which list: a tooled person's chat all of it, the dev
    /// caller too (in the test build alone), a lead its share, a session
    /// without tools nothing. A lead whose chat is gone stays one; a helper
    /// the person took over is their chat from its next start.
    #[test]
    fn the_list_goes_by_the_callers_role() {
        use crate::provider::Provider;
        let acc = "C:/acc/anthropic/one";
        let bg = json!({ "background": true });
        agents::register("tr-chat", Provider::Claude, "C:/p", acc, None, true, json!({}));
        agents::register("tr-lead", Provider::Claude, "C:/p", acc, Some("tr-chat"), true, bg.clone());
        agents::register("tr-plain", Provider::Claude, "C:/p", acc, Some("tr-chat"), false, bg.clone());
        assert_eq!(tools_for("tr-chat"), tools_as(true));
        assert_eq!(tools_for("tr-lead"), tools_as(false));
        assert_eq!(tools_for("tr-plain"), json!([]));
        if cfg!(luna_test) {
            assert_eq!(tools_for(agents::DEV_CALLER), tools_as(true));
        }
        // Orphaned: its chat deleted, and started again with no parent and
        // not in the background, it is still no person's chat.
        agents::forget("tr-chat");
        assert_eq!(agents::parent_of("tr-lead"), None);
        assert_eq!(tools_for("tr-lead"), tools_as(false));
        agents::register("tr-lead", Provider::Claude, "C:/p", acc, None, true, json!({}));
        assert_eq!(tools_for("tr-lead"), tools_as(false), "an orphan stays a lead");
        // Taken over by the person and started as their chat.
        agents::register("tr-top", Provider::Claude, "C:/p", acc, None, true, json!({}));
        agents::register("tr-took", Provider::Claude, "C:/p", acc, Some("tr-top"), true, bg);
        assert_eq!(tools_for("tr-took"), tools_as(false));
        agents::take_over("tr-took", || None).unwrap();
        agents::register("tr-took", Provider::Claude, "C:/p", acc, None, true, json!({}));
        assert_eq!(tools_for("tr-took"), tools_as(true), "the person's chat now");
        for id in ["tr-took", "tr-top", "tr-plain", "tr-lead"] {
            agents::forget(id);
        }
    }

    /// What a lead reads in the guide and hears in an error names only what
    /// it is offered; what it is refused is refused as before.
    #[test]
    fn a_lead_is_told_only_of_what_it_is_offered() {
        use super::super::args::arguments_fit;
        use super::super::guide::{guide_text, guide_text_as, TOPICS};
        let lead_names = |text: &str| PERSONS_ONLY.iter().any(|n| text.contains(n));
        let index = guide_text_as(None, false).unwrap();
        assert!(!lead_names(&index) && index.contains("luna_run_list"), "{index}");
        assert!(lead_names(&guide_text(None).unwrap()));
        for (topic, _) in TOPICS.iter().filter(|(t, _)| !PERSONS_ONLY.contains(t)) {
            let text = guide_text_as(Some(topic), false).unwrap();
            assert!(!lead_names(&text), "{topic}: {text}");
        }
        // The section the two are in keeps the rest of its lines.
        let helpers = guide_text_as(Some("helpers"), false).unwrap();
        assert!(helpers.starts_with("## Working with helpers") && helpers.contains("`luna_run_list`"), "{helpers}");
        assert!(helpers.contains("`luna_guide_get` with no topic"), "{helpers}");
        assert!(guide_text(Some("helpers")).unwrap().contains("luna_power_set"));
        for topic in PERSONS_ONLY {
            let e = guide_text_as(Some(topic), false).unwrap_err();
            let offered = e.split("the topics: ").nth(1).unwrap_or_default();
            assert!(e.starts_with("no topic") && offered.contains("luna_run_list") && !lead_names(offered), "{e}");
        }
        // An argument's error names the lead's arguments and values, and a
        // scope it is not offered is to be left out; a chat's names them all.
        let e = arguments_fit("luna_session_list", &json!({ "bogus": 1 }), false).unwrap_err();
        assert!(e.contains("it takes: ") && !e.contains("scope"), "{e}");
        assert!(arguments_fit("luna_session_list", &json!({ "bogus": 1 }), true).unwrap_err().contains("scope"));
        let e = arguments_fit("luna_session_list", &json!({ "scope": "everyone" }), false).unwrap_err();
        assert!(e.contains("leave it out") && !e.contains("all"), "{e}");
        assert!(arguments_fit("luna_session_list", &json!({ "scope": "everyone" }), true).unwrap_err().contains("mine, all"));
        let e = arguments_fit("luna_guide_get", &json!({ "topic": "weather" }), false).unwrap_err();
        assert!(e.contains("luna_run_list") && !lead_names(&e), "{e}");
        // The check is the same for both: a scope it says is passed on to be
        // refused (or not) by the tool, as is a tool it is not offered.
        assert!(arguments_fit("luna_session_list", &json!({ "scope": "all" }), false).is_ok());
        assert!(arguments_fit("luna_guide_get", &json!({ "topic": "luna_power_set" }), false).is_ok());
        assert!(arguments_fit("luna_power_set", &json!({ "action": "cancel" }), false).is_ok());
        assert!(!names_as(false).iter().any(|n| PERSONS_ONLY.contains(&n.as_str())) && names_as(true).len() == 16);
    }
}
