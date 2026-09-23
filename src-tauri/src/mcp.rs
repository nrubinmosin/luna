//! Luna as an MCP server, for the sessions that asked for it: JSON-RPC over
//! streamable HTTP on the hub listener (`POST /mcp`, bearer token per
//! session). Seven tools, described as briefly as a model can still use
//! them — every word here lands in the caller's context once per session —
//! and a short `instructions` string in place of any system-prompt flag.
//!
//! Each request is answered with a plain JSON body (the transport allows it;
//! no SSE stream is ever opened), so `wait` simply holds its HTTP request
//! until the session it watches gets there.

use crate::agents;
use serde_json::{json, Value};

pub const PROTOCOL: &str = "2025-06-18";

const INSTRUCTIONS: &str = "Luna runs your session and can run helper sessions for you: another model, \
another account, or Codex. Flow: luna_spawn with a prompt → luna_wait(id, \"turn_done\") returns the \
reply → luna_send for follow-ups, luna_wait again → luna_delete when finished (dropWorktree: true if \
you asked for one). You only reach sessions you spawned; luna_list shows them and the accounts you may \
use. Prefer one helper at a time; each costs its account's quota.";

/// One JSON-RPC message in, at most one out (None for notifications).
pub fn handle(caller: &str, body: &str) -> Option<Value> {
    let req: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req["method"].as_str().unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));
    if method.starts_with("notifications/") {
        return None;
    }
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "luna", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => Ok(call(caller, &params)),
        _ => Err((-32601, format!("method not found: {method}"))),
    };
    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, msg)) => error(id, code, &msg),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tools() -> Value {
    let s = |props: Value, required: &[&str]| json!({ "type": "object", "properties": props, "required": required });
    json!([
        {
            "name": "luna_list",
            "description": "Sessions you spawned (id, name, provider, account, turn, busy, turnsEnded) and the accounts you may spawn on.",
            "inputSchema": s(json!({}), &[])
        },
        {
            "name": "luna_spawn",
            "description": "Start a helper session with an opening prompt; it appears under yours in Luna. Omitted settings take the account's defaults. Returns its id; follow with luna_wait.",
            "inputSchema": s(json!({
                "prompt": { "type": "string" },
                "provider": { "type": "string", "enum": ["claude", "codex"], "description": "default: yours" },
                "account": { "type": "string", "description": "account name from luna_list; default: yours" },
                "model": { "type": "string", "description": "claude: opus|sonnet|haiku|fable; codex: any model id" },
                "effort": { "type": "string", "description": "low|medium|high|xhigh|max" },
                "permissionMode": { "type": "string", "description": "claude: default|acceptEdits|plan|bypassPermissions" },
                "approval": { "type": "string", "description": "codex: on-request|never" },
                "sandbox": { "type": "string", "description": "codex: read-only|workspace-write|danger-full-access" },
                "folder": { "type": "string", "description": "project folder; default: yours" },
                "worktree": { "type": "boolean", "description": "run in its own git worktree" },
                "name": { "type": "string", "description": "row title in Luna" },
                "tools": { "type": "boolean", "description": "give it these tools too (one level only)" }
            }), &["prompt"])
        },
        {
            "name": "luna_send",
            "description": "Type text into a session and press Enter.",
            "inputSchema": s(json!({ "id": { "type": "string" }, "text": { "type": "string" } }), &["id", "text"])
        },
        {
            "name": "luna_read",
            "description": "Messages of a session from the transcript. `since` = cursor from an earlier read; `last` = only the last N messages (default 1). Also returns turn state.",
            "inputSchema": s(json!({
                "id": { "type": "string" },
                "since": { "type": "integer" },
                "last": { "type": "integer" },
                "tools": { "type": "boolean", "description": "include one-line tool call markers" }
            }), &["id"])
        },
        {
            "name": "luna_wait",
            "description": "Block until a session finishes a turn (returns its reply), stops for a permission, or exits. `afterTurn`: the turnsEnded you last saw (default: now).",
            "inputSchema": s(json!({
                "id": { "type": "string" },
                "until": { "type": "string", "enum": ["turn_done", "waiting", "exit"] },
                "timeoutS": { "type": "integer", "description": "default 600, max 1800" },
                "afterTurn": { "type": "integer" }
            }), &["id"])
        },
        {
            "name": "luna_kill",
            "description": "Stop a session's process; its row stays in Luna.",
            "inputSchema": s(json!({ "id": { "type": "string" } }), &["id"])
        },
        {
            "name": "luna_delete",
            "description": "Stop a session and remove its row; dropWorktree also deletes the worktree and branch it ran in.",
            "inputSchema": s(json!({ "id": { "type": "string" }, "dropWorktree": { "type": "boolean" } }), &["id"])
        }
    ])
}

fn call(caller: &str, params: &Value) -> Value {
    let name = params["name"].as_str().unwrap_or("");
    let a = &params["arguments"];
    let id = a["id"].as_str().unwrap_or("").to_string();
    let out: Result<Value, String> = match name {
        "luna_list" => Ok(json!(agents::list(caller))),
        "luna_spawn" => serde_json::from_value::<agents::SpawnParams>(a.clone())
            .map_err(|e| format!("bad arguments: {e}"))
            .and_then(|p| agents::spawn(caller, p))
            .map(|s| json!(s)),
        "luna_send" => agents::send(caller, &id, a["text"].as_str().unwrap_or("")).map(|()| json!({ "ok": true })),
        "luna_read" => agents::read(
            caller,
            &id,
            a["since"].as_u64(),
            Some(a["last"].as_u64().unwrap_or(1) as usize),
            a["tools"].as_bool().unwrap_or(false),
        )
        .map(|r| json!(r)),
        "luna_wait" => agents::wait(
            caller,
            &id,
            a["until"].as_str().unwrap_or("turn_done"),
            a["timeoutS"].as_u64().unwrap_or(600),
            a["afterTurn"].as_u64().map(|n| n as u32),
        )
        .map(|w| json!(w)),
        "luna_kill" => agents::kill(caller, &id).map(|()| json!({ "ok": true })),
        "luna_delete" => agents::delete(caller, &id, a["dropWorktree"].as_bool().unwrap_or(false))
            .map(|wt| json!({ "ok": true, "worktreeRemoved": wt })),
        _ => Err(format!("unknown tool {name}")),
    };
    match out {
        Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }] }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_lists_tools_and_answers_ping() {
        let init = handle("me", r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26"}}"#).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert!(init["result"]["instructions"].as_str().unwrap().contains("luna_spawn"));
        assert!(handle("me", r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let list = handle("me", r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["luna_list", "luna_spawn", "luna_send", "luna_read", "luna_wait", "luna_kill", "luna_delete"]);
        assert_eq!(handle("me", r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#).unwrap()["result"], json!({}));
        assert_eq!(handle("me", r#"{"jsonrpc":"2.0","id":4,"method":"nope"}"#).unwrap()["error"]["code"], -32601);
        assert_eq!(handle("me", "{").unwrap()["error"]["code"], -32700);
    }

    #[test]
    fn a_tool_on_a_foreign_session_is_an_error_result() {
        let r = handle("me", r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"luna_send","arguments":{"id":"someone","text":"hi"}}}"#).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("not one you spawned"));
    }

    #[test]
    fn the_tool_text_stays_small() {
        // The whole list is what every tooled session pays for once; keep it
        // under a few thousand characters.
        let text = tools().to_string();
        assert!(text.len() < 4000, "tools/list is {} chars", text.len());
        assert!(INSTRUCTIONS.len() < 700);
    }
}
