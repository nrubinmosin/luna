//! The dialogs' fixtures: what the fake host answers each of their commands
//! with in the tests and the shots, two Claude Code models with one that takes
//! no effort, a Codex one, a few helpers under a chat and a conversation to
//! watch.

use super::cmd;
use crate::demo::FakeHost;
use serde_json::{Value, json};

pub fn claude_models() -> Value {
    json!([
        { "slug": "opus", "displayName": "Opus", "defaultEffort": "high", "efforts": ["low", "medium", "high", "xhigh", "max"], "more": false,
          "effortHints": { "high": "deeper thinking" } },
        { "slug": "sonnet", "displayName": "Sonnet", "defaultEffort": "medium", "efforts": ["low", "medium", "high"], "more": false },
        { "slug": "haiku", "displayName": "Haiku", "defaultEffort": null, "efforts": [], "more": true },
    ])
}

pub fn claude_defaults() -> Value {
    json!({
        "model": { "value": "opus", "source": "account" },
        "effort": { "value": "high", "source": "stock" },
        "permissionMode": { "value": "acceptEdits", "source": "account" },
        "effortByModel": { "sonnet": { "value": "low", "source": "account", "key": "modelSettings.sonnet.effortLevel" } },
    })
}

pub fn codex_models() -> Value {
    json!([
        { "slug": "gpt-6", "displayName": "GPT-6", "defaultEffort": "medium", "efforts": ["low", "medium", "high"] },
        { "slug": "gpt-6-mini", "displayName": "GPT-6 mini", "defaultEffort": "low", "efforts": ["low", "medium"] },
    ])
}

pub fn codex_defaults() -> Value {
    json!({
        "model": { "value": "gpt-6", "source": "codex" },
        "effort": { "value": "medium", "source": "codex" },
        "approval": { "value": "on-request", "source": "account" },
        "sandbox": { "value": "workspace-write", "source": "account" },
    })
}

/// The helpers `background_helpers` lists: two under the chat `c-parser`, one
/// of them with a helper of its own, and one under a chat that is gone.
pub fn helpers() -> Value {
    let now = crate::panels::now_ms();
    json!([
        { "id": "h-review", "parent": "c-parser", "name": "review-plan", "provider": "claude", "state": "working", "account": "work",
          "model": "sonnet", "effort": "medium", "startedMs": now - 12 * 60_000, "turnStartedMs": now - 4 * 60_000,
          "lastActivityMs": now - 5_000, "task": "Review the parser plan for missed error paths" },
        { "id": "h-sub", "parent": "h-review", "name": "grep-callers", "provider": "claude", "state": "question", "account": "work",
          "model": "haiku", "effort": "", "startedMs": now - 3 * 60_000, "turnStartedMs": now - 60_000,
          "ask": { "kind": "question", "questions": [{ "text": "Include the vendored folder?" }] }, "task": "Find the callers of lex()" },
        { "id": "h-done", "parent": "c-parser", "name": "write-tests", "provider": "codex", "state": "idle", "account": "main",
          "model": "gpt-6", "effort": "medium", "startedMs": now - 50 * 60_000, "lastActivityMs": now - 20 * 60_000,
          "task": "Write tests for the lexer" },
        { "id": "h-loose", "parent": "c-gone", "name": "stray", "provider": "claude", "state": "idle", "account": "work",
          "model": "opus", "effort": "high", "startedMs": now - 90 * 60_000, "task": null },
    ])
}

pub fn transcript() -> Value {
    json!({
        "messages": [
            { "role": "user", "text": "Review the parser plan for missed error paths.\nStart with lex()." },
            { "role": "assistant", "text": "I read plan.md. Two paths are not handled: a lone surrogate in a string, and an unterminated block comment at the end of the file." },
            { "role": "assistant", "text": "[tool] Read crates/parser/src/lex.rs" },
        ],
        "cursor": 3,
        "file": "C:\\accounts\\work\\projects\\x\\h-review.jsonl",
        "running": false,
    })
}

/// Every command of the dialogs' answered as a person's Luna might: nothing
/// is refused, every folder is trusted and in git, the run before ended by
/// itself.
pub fn answer_all(host: &FakeHost) {
    host.answer(cmd::CLAUDE_DEFAULTS, claude_defaults());
    host.answer(cmd::CLAUDE_MODELS, claude_models());
    host.answer(cmd::CODEX_DEFAULTS, codex_defaults());
    host.answer(cmd::CODEX_MODELS, codex_models());
    host.answer(cmd::CHAT_FOLDER_CHECK, json!({ "refusal": null, "inGit": true }));
    host.answer(cmd::CLAUDE_FOLDER_TRUSTED, json!(true));
    host.answer(cmd::CODEX_FOLDER_TRUSTED, json!(true));
    host.answer(cmd::CLAUDE_TRUST_FOLDER, Value::Null);
    host.answer(cmd::CODEX_TRUST_FOLDER, Value::Null);
    host.answer(cmd::DELETE_SESSION, json!(null));
    host.answer(cmd::BACKGROUND_HELPERS, helpers());
    host.answer(
        cmd::BACKGROUND_AT_STAKE,
        json!([
            { "id": "h-review", "working": "a turn, 4m in" },
            { "id": "h-sub", "working": "asks a question" },
            { "id": "h-done" },
        ]),
    );
    host.answer(cmd::BACKGROUND_DELETE, json!({}));
    host.answer(cmd::BACKGROUND_KILL, Value::Null);
    host.answer(cmd::BACKGROUND_TRANSCRIPT, transcript());
    host.answer(cmd::CRASH_REPORT, Value::Null);
}

/// The scenes the shots take: the dialogs, each a driver script.
pub fn scenes() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("dialog-new-chat", vec!["dialog new-chat"]),
        ("dialog-new-chat-codex", vec!["dialog new-chat", "newchat account 3"]),
        ("dialog-new-chat-untrusted", vec!["dialog new-chat"]),
        ("dialog-new-chat-worktree", vec!["dialog new-chat", "newchat worktree"]),
        ("dialog-new-chat-no-git", vec!["dialog new-chat"]),
        ("dialog-quick-chat", vec!["dialog quick"]),
        ("dialog-delete", vec!["dialog delete c-notes"]),
        ("dialog-delete-helpers", vec!["dialog delete c-parser", "deletechat helpers", "deletechat stop"]),
        ("dialog-helpers", vec!["dialog helpers c-parser"]),
        ("dialog-helpers-idle", vec!["dialog helpers c-parser", "helpers tab idle"]),
        ("dialog-helpers-watch", vec!["dialog helpers c-parser", "helpers watch h-review"]),
        ("dialog-crash", vec!["dialog crash"]),
    ]
}
