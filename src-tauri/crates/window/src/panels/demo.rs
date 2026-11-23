//! The panels' fixtures: what the fake host answers each of their commands
//! with in the tests and the shots, a library, two Claude Code accounts and a
//! Codex one, and settings as a person might have them.

use super::cmd;
use crate::demo::FakeHost;
use serde_json::{Value, json};

pub const SKILL_TEXT: &str = "---\nname: review\ndescription: Reviews a change for correctness bugs before it is committed.\n---\n\n# Review\n\n\
    Read the diff whole first. Then, for each file:\n\n1. Say what it changes, in a line.\n2. Look for **correctness** bugs: \
    off-by-one, a missed error path, a race.\n3. Name the line and the failure it causes.\n\n```sh\ngit diff --stat\n```\n";

/// The demo library's folder, and its identity on its drive.
pub const LIBRARY_ROOT: &str = "C:\\Users\\me\\AppData\\Luna\\library";
pub const LIBRARY_ROOT_ID: &str = "00000000a1b2c3d4:000000000000000000010000000000a1";

pub fn overview() -> Value {
    json!({
        "root": LIBRARY_ROOT,
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
            { "name": "role-lead", "kind": "role", "state": "library", "version": "0.20.0", "sha256": "9a8b7c6d5e4f3a2b1c", "path": "C:\\lib\\system\\role-lead\\TEXT.md", "problem": null },
            { "name": "role-helper", "kind": "role", "state": "luna", "version": "0.20.0", "sha256": "4f1c2a9b7d3e5f60aa", "path": "C:\\lib\\system\\role-helper\\TEXT.md", "problem": null },
            { "name": "session-handoff", "kind": "skill", "state": "luna", "version": "0.19.0", "sha256": "0011223344556677", "path": "", "problem": null },
        ],
        "systemAside": [],
        "places": [],
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
        cmd::LIBRARY_NEW_SKILL,
        cmd::LIBRARY_DELETE_SKILL,
        cmd::LIBRARY_NEW_INSTRUCTIONS,
        cmd::LIBRARY_RENAME_INSTRUCTIONS,
        cmd::LIBRARY_DELETE_INSTRUCTIONS,
    ] {
        host.answer(c, overview());
    }
    host.answer(cmd::LIBRARY_RENAME_SKILL, json!({ "outcome": "committed", "why": null, "overview": overview() }));
    // Each entry its own file, so the editor shows the one picked.
    host.respond(cmd::LIBRARY_READ, |args| {
        let entry = &args["entry"];
        let (path, text) = if let Some(name) = entry["instructions"].as_str() {
            (format!("instructions\\{name}.md"), format!("# {name}\n\nAnswer in short sentences. Say what you changed and why.\n"))
        } else if let Some(name) = entry["system"].as_str() {
            (format!("system\\{name}\\TEXT.md"), "You are a lead. Report in English.\n".to_owned())
        } else {
            let name = entry["skill"].as_str().unwrap_or("review");
            (format!("skills\\{name}\\SKILL.md"), SKILL_TEXT.to_owned())
        };
        json!({ "path": format!("{LIBRARY_ROOT}\\{path}"), "text": text, "exists": true,
                "root": LIBRARY_ROOT, "rootId": LIBRARY_ROOT_ID, "revision": "r1" })
    });
    host.answer(cmd::LIBRARY_WRITE_CHECKED, json!({ "conflict": null, "revision": "r2", "overview": overview() }));
    host.answer(cmd::LIBRARY_DRAFT, Value::Null);
    host.answer(cmd::LIBRARY_CHECK_SYSTEM, Value::Null);
    host.answer(cmd::LIBRARY_SYSTEM_COMPARE, json!({ "luna": "You are a lead.\n", "yours": "You are a lead. Report in English.\n" }));

    host.answer(cmd::APP_VERSION, json!("0.20.0"));
    host.answer(cmd::WINDOW_PREFS, json!({ "sound": true, "newChat": { "worktree": true, "tools": false } }));
    host.answer(cmd::GET_ACCOUNTS_ROOT, json!({ "path": "C:\\Users\\me\\luna-accounts", "isDefault": true }));
    host.answer(cmd::SET_ACCOUNTS_ROOT, json!({ "path": "D:\\accounts", "isDefault": false }));
    let helpers = json!({ "maxHelpers": 16, "min": 1, "max": 64, "helperContextPct": 50, "leadHandoff": { "on": true, "threshold": 60 } });
    for c in [cmd::HELPER_SETTINGS, cmd::SET_MAX_HELPERS, cmd::SET_HELPER_CONTEXT_PCT] {
        host.answer(c, helpers.clone());
    }
    host.answer(cmd::CODEX_TRUSTED_ROOTS, json!(["E:\\Projects"]));
    host.answer(cmd::SET_CODEX_TRUSTED_ROOTS, json!(["E:\\Projects", "D:\\work"]));
    host.answer(cmd::COPY_ON_SELECT, json!("off"));
    host.respond(cmd::SET_COPY_ON_SELECT, |args| args["value"].clone());
    // Claude Code keeps a version to go back to; Codex's is one Luna turned away.
    host.respond(cmd::CLI_STATUS, |args| {
        let (provider, version, path) = match args["provider"].as_str() {
            Some("codex") => ("codex", "0.171.0", "C:\\luna\\codex-cli\\current\\codex.exe"),
            _ => ("claude", "2.1.330", "C:\\luna\\claude-cli\\current\\claude.exe"),
        };
        json!({ "provider": provider, "phase": "idle", "version": version, "path": path, "latest": version, "got": 0, "total": null,
                "error": null, "note": null, "checkedAtMs": 1_763_540_000_000u64 })
    });
    host.respond(cmd::CLI_VERSIONS, |args| {
        let row = |version: &str, current: bool, previous: bool, bad: Value| {
            json!({ "dir": version, "version": version, "current": current, "previous": previous, "inUse": current, "belowFloor": false, "bad": bad })
        };
        match args["provider"].as_str() {
            Some("codex") => json!({
                "provider": "codex",
                "versions": [row("0.171.0", true, false, Value::Null), row("0.170.0", false, true, json!("rolled back by hand"))],
                "canary": null,
                "wayBack": { "dir": "0.170.0", "version": "0.170.0", "refusal": "0.170.0 was turned away (rolled back by hand)" },
            }),
            _ => json!({
                "provider": "claude",
                "versions": [row("2.1.330", true, false, Value::Null), row("2.1.328", false, true, Value::Null)],
                "canary": null,
                "wayBack": { "dir": "2.1.328", "version": "2.1.328", "refusal": null },
            }),
        }
    });
    host.answer(cmd::CLI_ROLL_BACK, json!({ "from": "2.1.330", "to": "2.1.328" }));
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
        // Signed out, as a fresh account is: its row offers Sign in.
        json!({ "signedIn": false, "email": null, "plan": null, "haveUsage": false, "windows": [],
                "source": "network", "fetchedAtMs": 1_763_540_000_000u64, "stale": false, "rateLimited": null }),
    );
    host.answer(
        cmd::CREATE_ACCOUNT,
        json!({ "provider": "claude", "name": "new", "path": "C:\\Users\\me\\luna-accounts\\anthropic\\new", "id": "f00d:02" }),
    );
    host.answer(
        cmd::RENAME_ACCOUNT,
        json!({ "provider": "claude", "name": "office", "path": "C:\\Users\\me\\luna-accounts\\anthropic\\office" }),
    );
    host.answer(cmd::DELETE_ACCOUNT, Value::Null);
    host.answer(cmd::ACCOUNT_IDENTITY, json!({ "path": "C:\\Users\\me\\luna-accounts\\anthropic\\work", "id": "f00d:01" }));
    host.answer(cmd::DELETE_ACCOUNT_CHECKED, Value::Null);
    let sync = json!({ "claude": "work", "codex": null, "onStart": false });
    for c in [cmd::SETTINGS_SYNC_STATE, cmd::SET_SETTINGS_PARENT, cmd::SET_SETTINGS_SYNC_ON_START] {
        host.answer(c, sync.clone());
    }
    host.answer(cmd::SYNC_SETTINGS_NOW, json!({ "synced": ["home"], "failed": [] }));
    // One account kept from agents, as its switch shows it.
    host.answer(cmd::AGENT_BLOCKED_ACCOUNTS, json!(["claude/home"]));
    host.answer(cmd::SET_AGENT_ACCOUNT, json!(["claude/home"]));
}

/// The panels' scenes for the shots: each opened, then the driver's lines.
pub fn scenes() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("library", vec!["panel library"]),
        ("library-skill", vec!["panel library", "select skill review"]),
        ("library-instructions", vec!["panel library", "select instructions terse"]),
        ("library-reach", vec!["panel library", "select skill review", "menu lib-reach"]),
        ("library-folder", vec!["panel library", "select folder E:\\Projects\\luna"]),
        ("library-folder-menu", vec!["panel library", "select folder E:\\Projects\\luna", "menu lib-folder-ins-0"]),
        ("library-system", vec!["panel library", "select system role-lead", "compare role-lead"]),
        ("library-sync", vec!["panel library", "sync"]),
        ("settings", vec!["panel settings"]),
        ("settings-chats", vec!["panel settings", "page chats"]),
        ("settings-agents", vec!["panel settings", "page agents"]),
        ("settings-clis", vec!["panel settings", "page clis"]),
        ("settings-updates", vec!["panel settings", "page updates", "update-check"]),
        ("settings-update-confirm", vec!["panel settings", "page updates", "update-check", "update-confirm"]),
        ("accounts", vec!["panel accounts"]),
        ("accounts-add", vec!["panel accounts", "adding"]),
        ("accounts-parent", vec!["panel accounts", "menu accounts-parent-claude"]),
        ("accounts-rename", vec!["panel accounts", "rename 0"]),
        ("accounts-delete", vec!["panel accounts", "delete 0"]),
        ("accounts-signin", vec!["panel accounts", "signin 2"]),
    ]
}
