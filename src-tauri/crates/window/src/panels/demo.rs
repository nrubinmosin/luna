//! The panels' fixtures: what the fake host answers each of their commands
//! with in the tests and the shots, a library, two Claude Code accounts and a
//! Codex one, and settings as a person might have them.

use super::cmd;
use crate::demo::FakeHost;
use serde_json::{Value, json};

pub const SKILL_TEXT: &str = "---\nname: review\ndescription: Reviews a change for correctness bugs before it is committed.\n---\n\n# Review\n\n\
    Read the diff whole first. Then, for each file:\n\n1. Say what it changes, in a line.\n2. Look for **correctness** bugs: \
    off-by-one, a missed error path, a race.\n3. Name the line and the failure it causes.\n\n```sh\ngit diff --stat\n```\n";

pub fn overview() -> Value {
    json!({
        "root": "C:\\Users\\me\\AppData\\Luna\\library",
        "isDefault": true,
        "found": true,
        "instructions": [
            { "name": "main", "description": "Talking to the operator", "problem": null, "accounts": ["all"], "audience": "every", "perSession": false },
            { "name": "terse", "description": "Short answers", "problem": null, "accounts": ["claude/work"], "audience": ["chat"], "perSession": false },
            { "name": "lead-rules", "description": "", "problem": "The file is blank", "accounts": [], "audience": ["lead"], "perSession": true },
        ],
        "noInstructions": [],
        "instructionsFor": { "claude/work": "terse", "claude/home": "main", "codex/main": "main" },
        "oldInstructions": null,
        "skills": [
            { "name": "review", "description": "Reviews a change for correctness bugs before it is committed.", "problem": null, "accounts": ["all"], "audience": "every" },
            { "name": "release", "description": "Cuts a release end to end.", "problem": null, "accounts": ["claude"], "audience": ["chat", "lead"] },
            { "name": "draft", "description": "", "problem": "The front matter has no description", "accounts": [], "audience": "every" },
        ],
        "accounts": [
            { "provider": "claude", "name": "work" },
            { "provider": "claude", "name": "home" },
            { "provider": "codex", "name": "main" },
        ],
        "folders": [
            { "path": "E:\\Projects\\luna", "skills": { "release": [] }, "instructions": { "claude/work": null }, "found": true,
              "inherited": { "review": ["claude/work", "claude/home", "codex/main"], "release": ["claude/work", "claude/home"] } },
        ],
        "overridden": [],
        "system": [
            { "name": "role-chat", "kind": "role", "state": "luna", "version": "0.20.0", "sha256": "4f1c2a9b7d3e5f60aa", "path": "C:\\lib\\system\\role-chat\\TEXT.md", "problem": null },
            { "name": "role-lead", "kind": "role", "state": "library", "version": "0.20.0", "sha256": "9a8b7c6d5e4f3a2b1c", "path": "C:\\lib\\system\\role-lead\\TEXT.md", "problem": null },
            { "name": "session-handoff", "kind": "skill", "state": "luna", "version": "0.19.0", "sha256": "0011223344556677", "path": "", "problem": null },
        ],
        "systemAside": [],
        "places": [],
        "kept": [],
    })
}

pub fn accounts() -> Value {
    json!([
        { "provider": "claude", "name": "work", "path": "C:\\Users\\me\\luna-accounts\\anthropic\\work" },
        { "provider": "claude", "name": "home", "path": "C:\\Users\\me\\luna-accounts\\anthropic\\home" },
        { "provider": "codex", "name": "main", "path": "C:\\Users\\me\\luna-accounts\\openai\\main" },
    ])
}

/// Every command of the panels' answered as a person's Luna might.
pub fn answer_all(host: &FakeHost) {
    host.answer(cmd::LIBRARY_OVERVIEW, overview());
    host.answer(cmd::LIBRARY_SYNC, overview());
    for c in [
        cmd::LIBRARY_SET_SOURCE,
        cmd::LIBRARY_SET_INSTRUCTIONS,
        cmd::LIBRARY_SET_ACCOUNTS,
        cmd::LIBRARY_SET_AUDIENCE,
        cmd::LIBRARY_SET_FOLDER,
        cmd::LIBRARY_SET_FOLDER_SKILL,
        cmd::LIBRARY_SET_FOLDER_INSTRUCTIONS,
        cmd::LIBRARY_WRITE,
        cmd::LIBRARY_SYSTEM_RESET,
        cmd::LIBRARY_SYSTEM_ADOPT,
        cmd::LIBRARY_NEW_SKILL,
        cmd::LIBRARY_RENAME_SKILL,
        cmd::LIBRARY_DELETE_SKILL,
        cmd::LIBRARY_NEW_INSTRUCTIONS,
        cmd::LIBRARY_RENAME_INSTRUCTIONS,
        cmd::LIBRARY_DELETE_INSTRUCTIONS,
    ] {
        host.answer(c, overview());
    }
    host.answer(
        cmd::LIBRARY_READ,
        json!({ "path": "C:\\Users\\me\\AppData\\Luna\\library\\skills\\review\\SKILL.md", "text": SKILL_TEXT, "exists": true }),
    );
    host.answer(cmd::LIBRARY_CHECK_SKILL, Value::Null);
    host.answer(cmd::LIBRARY_CHECK_SYSTEM, Value::Null);
    host.answer(cmd::LIBRARY_SYSTEM_COMPARE, json!({ "luna": "You are a lead.\n", "yours": "You are a lead. Report in English.\n" }));

    host.answer(cmd::APP_VERSION, json!("0.20.0"));
    host.answer(cmd::WINDOW_PREFS, json!({ "sound": true, "newChat": { "worktree": true, "tools": false } }));
    host.answer(cmd::GET_ACCOUNTS_ROOT, json!({ "path": "C:\\Users\\me\\luna-accounts", "isDefault": true }));
    host.answer(cmd::SET_ACCOUNTS_ROOT, json!({ "path": "D:\\accounts", "isDefault": false }));
    host.answer(cmd::HELPERS_ABOVE_CALLER, json!(false));
    host.answer(cmd::SET_HELPERS_ABOVE_CALLER, json!(true));
    let helpers = json!({ "maxHelpers": 8, "min": 1, "max": 32, "helperMode": "inherit", "helperContextPct": 50, "leadHandoff": { "on": true, "threshold": 60 } });
    for c in [cmd::HELPER_SETTINGS, cmd::SET_MAX_HELPERS, cmd::SET_HELPER_MODE, cmd::SET_HELPER_CONTEXT_PCT] {
        host.answer(c, helpers.clone());
    }
    host.answer(cmd::CODEX_TRUSTED_ROOTS, json!(["E:\\Projects"]));
    host.answer(cmd::SET_CODEX_TRUSTED_ROOTS, json!(["E:\\Projects", "D:\\work"]));
    host.answer(
        cmd::CLI_STATUS,
        json!({ "provider": "claude", "phase": "idle", "version": "2.1.330", "path": "C:\\luna\\claude-cli\\current\\claude.exe",
                "latest": "2.1.330", "got": 0, "total": null, "error": null, "note": null, "checkedAtMs": 1_763_540_000_000u64 }),
    );
    host.answer(
        cmd::CLI_VERSIONS,
        json!({ "provider": "claude", "versions": [
            { "dir": "2.1.330", "version": "2.1.330", "current": true, "previous": false, "inUse": true, "belowMinSafe": false, "bad": null },
            { "dir": "2.1.328", "version": "2.1.328", "current": false, "previous": true, "inUse": false, "belowMinSafe": false, "bad": null },
        ], "canary": null }),
    );
    host.answer(cmd::CLI_UPDATE_NOW, Value::Null);
    host.answer(
        crate::desktop::cmd::UPDATE_CHECK,
        json!({ "current": "0.20.0", "offer": { "version": "0.20.1", "notes": "- The library's editor keeps its caret steady.\n- Settings say a CLI's check in words.", "pubDate": "2026-11-20", "coreFingerprint": null } }),
    );

    host.answer(cmd::LIST_ACCOUNTS, accounts());
    host.answer(
        cmd::ACCOUNT_LIMITS,
        json!({ "signedIn": true, "email": "me@example.com", "plan": "Max", "haveUsage": true, "h5": 0.42, "week": 0.71,
                "resetH5": "2026-11-19T15:00:00Z", "resetWeek": "2026-11-24T09:00:00Z",
                "scoped": [{ "label": "Fable", "used": 0.88, "resetsAt": "2026-11-24T09:00:00Z" }], "other": [],
                "source": "network", "fetchedAtMs": 1_763_540_000_000u64, "stale": false, "refreshError": null, "rateLimited": null }),
    );
    host.answer(
        cmd::CODEX_LIMITS,
        json!({ "signedIn": true, "email": null, "plan": "Pro", "haveUsage": true, "windows": [
            { "id": "primary", "label": "5 hours", "used": 0.12, "windowMinutes": 300, "resetsAt": "2026-11-19T16:30:00Z" },
            { "id": "secondary", "label": "week", "used": 0.35, "windowMinutes": 10080, "resetsAt": "2026-11-25T00:00:00Z" },
        ], "source": "network", "fetchedAtMs": 1_763_540_000_000u64, "stale": false, "rateLimited": null }),
    );
    host.answer(
        cmd::CREATE_ACCOUNT,
        json!({ "provider": "claude", "name": "new", "path": "C:\\Users\\me\\luna-accounts\\anthropic\\new" }),
    );
    host.answer(
        cmd::RENAME_ACCOUNT,
        json!({ "provider": "claude", "name": "office", "path": "C:\\Users\\me\\luna-accounts\\anthropic\\office" }),
    );
    host.answer(cmd::DELETE_ACCOUNT, Value::Null);
    let sync = json!({ "claude": "work", "codex": null, "onStart": false });
    for c in [cmd::SETTINGS_SYNC_STATE, cmd::SET_SETTINGS_PARENT, cmd::SET_SETTINGS_SYNC_ON_START] {
        host.answer(c, sync.clone());
    }
    host.answer(cmd::SYNC_SETTINGS_NOW, json!({ "synced": ["home"], "failed": [] }));
}

/// The panels' scenes for the shots: each opened, then the driver's lines.
pub fn scenes() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("library", vec!["panel library"]),
        ("library-skill", vec!["panel library", "select skill review"]),
        ("library-instructions", vec!["panel library", "select instructions terse"]),
        ("library-reach", vec!["panel library", "select skill review", "menu lib-reach"]),
        ("library-folder", vec!["panel library", "select folder E:\\Projects\\luna"]),
        ("library-system", vec!["panel library", "select system role-lead", "compare role-lead"]),
        ("settings", vec!["panel settings"]),
        ("settings-chats", vec!["panel settings", "page chats"]),
        ("settings-agents", vec!["panel settings", "page agents"]),
        ("settings-clis", vec!["panel settings", "page clis"]),
        ("settings-updates", vec!["panel settings", "page updates", "update-check"]),
        ("settings-update-confirm", vec!["panel settings", "page updates", "update-check", "update-confirm"]),
        ("accounts", vec!["panel accounts"]),
        ("accounts-delete", vec!["panel accounts", "delete 0"]),
    ]
}
