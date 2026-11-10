//! How a Claude Code session in a pane is started: its arguments and
//! environment, laid out from what the start has already settled. Pure:
//! nothing here reads the environment, the disk or Luna's state, logs,
//! writes, registers or mints. The start does that under the chat's spawn
//! gate (pty/start.rs) and hands the results in.

use crate::applied::{Applied, Item};
use std::path::Path;

/// A Claude Code chat's start as `chat_args` lays it out: what the window
/// asked for, and what the start made for it under the spawn gate — the
/// files and the token, which this only names.
pub(crate) struct ChatStart<'a> {
    pub(crate) id: &'a str,
    pub(crate) account_path: &'a str,
    pub(crate) model: &'a str,
    pub(crate) effort: &'a str,
    pub(crate) permission_mode: &'a str,
    pub(crate) resume: Option<&'a str>,
    /// `--worktree`, by this name if it has one.
    pub(crate) worktree: Option<Option<&'a str>>,
    /// `--settings`: a file's path, or inline JSON (a login's), and what it
    /// holds (`settings_applied`).
    pub(crate) settings: Option<(&'a str, &'a [Applied])>,
    pub(crate) agent: Option<&'a str>,
    pub(crate) read_only: bool,
    /// Luna's MCP config file and the token its header takes.
    pub(crate) mcp: Option<(&'a Path, &'a str)>,
    pub(crate) notes_dir: Option<&'a Path>,
    pub(crate) instructions_dir: Option<&'a Path>,
    /// Auto-handoff's threshold, and the folder its copies are kept in, or
    /// why it is not given: its drafts folder could not be made private
    /// (agents/handoff `drafts_at_start`).
    pub(crate) handoff: Option<(u8, Result<&'a Path, &'a str>)>,
    pub(crate) skill_args: &'a [String],
}

/// The arguments and environment of a Claude Code chat's start, and what
/// Luna applied in them (applied.rs).
pub(crate) fn chat_args(s: &ChatStart) -> (Vec<String>, Vec<(&'static str, String)>, Vec<Applied>) {
    let path = |p: &Path| p.to_string_lossy().into_owned();
    let mut applied = vec![
        Item::Account.with(format!("CLAUDE_CONFIG_DIR={}", s.account_path)),
        Item::NoAutoUpdate.with("DISABLE_AUTOUPDATER=1"),
        session_applied(s.id, None),
        Item::Model.with(format!("--model {}", s.model)),
    ];
    let mut args = vec!["--model".to_string(), s.model.to_string()];
    // No effort for a model that takes none (claude/models.rs): no flag.
    if !s.effort.is_empty() {
        args.extend(["--effort".to_string(), s.effort.to_string()]);
        applied.push(Item::Effort.with(format!("--effort {}", s.effort)));
    }
    args.extend(["--permission-mode".to_string(), s.permission_mode.to_string()]);
    applied.push(Item::PermissionMode.with(format!("--permission-mode {}", s.permission_mode)));
    if let Some(session_id) = s.resume {
        args.push("--resume".into());
        args.push(session_id.to_string());
        applied.push(Item::Resume.with(format!("--resume {session_id}")));
    }
    if let Some(name) = s.worktree {
        let arg = crate::worktree::worktree_arg(name);
        applied.push(Item::Worktree.with(&arg));
        args.push(arg);
    }
    if let Some((settings, holds)) = s.settings {
        args.push("--settings".into());
        args.push(settings.to_string());
        applied.extend(holds.iter().cloned());
    }
    if let Some(agent) = s.agent {
        args.push("--agent".into());
        args.push(agent.to_string());
        applied.push(Item::Agent.with(format!("--agent {agent}")));
    }
    if s.read_only {
        args.push("--disallowedTools".into());
        args.push(CLAUDE_EDIT_TOOLS.join(","));
        applied.push(Item::ReadOnly.with(format!("--disallowedTools {}", CLAUDE_EDIT_TOOLS.join(","))));
    }
    if let Some((mcp, _)) = s.mcp {
        // Luna's tools without a permission prompt in every mode: the
        // user ticked the box that gives them. Variadic like
        // `--mcp-config`, so it goes where a flag follows it.
        args.push("--allowedTools".into());
        args.push("mcp__luna".into());
        args.push("--mcp-config".into());
        args.push(path(mcp));
        applied.push(Item::LunaTools.with(luna_tools_value(&path(mcp))));
    }
    // A long message from another session comes as a file there
    // (agents/delivery.rs `spill`), and so does a helper's answer named in a
    // notice, to read without asking.
    if let Some(notes) = s.notes_dir {
        args.push("--add-dir".into());
        args.push(path(notes));
        applied.push(Item::NotesDir.with(format!("--add-dir {}", path(notes))));
    }
    if let Some(dir) = s.instructions_dir {
        args.push("--add-dir".into());
        args.push(path(dir));
        applied.push(instructions_applied(&path(dir)));
    }
    // Auto-handoff (agents/handoff): the copy of a handoff is what the next
    // conversation reads first, from a folder of the chat's own, without asking.
    if let Some((threshold, dir)) = s.handoff {
        let mut value = format!("at {threshold}% of the context: luna_session_handoff");
        match dir {
            Ok(dir) => {
                args.push("--add-dir".into());
                args.push(path(dir));
                value.push_str(&format!("; --add-dir {}", path(dir)));
            }
            Err(why) => value.push_str(&crate::agents::handoff::no_drafts_applied(why)),
        }
        applied.push(Item::AutoHandoff.with(value));
    }
    args.extend(s.skill_args.iter().cloned());
    applied.extend(skills_applied(s.skill_args));
    let mut envs = vec![
        ("CLAUDE_CONFIG_DIR", s.account_path.to_string()),
        // Luna updates the CLI itself; the CLI's own updater would install a
        // second copy under the user profile that nothing here ever runs.
        ("DISABLE_AUTOUPDATER", "1".to_string()),
    ];
    if s.instructions_dir.is_some() {
        envs.push((CLAUDE_ADDED_DIR_MEMORY, "1".to_string()));
    }
    if let Some((_, token)) = s.mcp {
        envs.push((CLAUDE_MCP_TOKEN_ENV, token.to_string()));
    }
    envs.extend(crate::pty::session_envs(s.id, None));
    (args, envs, applied)
}

/// `session_envs` as applied: the session's id, and its parent's.
pub(crate) fn session_applied(id: &str, parent: Option<&str>) -> Applied {
    Item::SessionId.with(match parent {
        Some(parent) => format!("LUNA_SESSION_ID={id}, LUNA_PARENT_ID={parent}"),
        None => format!("LUNA_SESSION_ID={id}"),
    })
}

/// Luna's MCP server as applied: the config file by its path, the token by
/// the variable it travels in.
pub(crate) fn luna_tools_value(config: &str) -> String {
    format!("--mcp-config {config}, --allowedTools mcp__luna; its token in {CLAUDE_MCP_TOKEN_ENV}, not shown")
}

/// The library's instructions folder as applied, with the variable that has
/// it read as memory.
pub(crate) fn instructions_applied(dir: &str) -> Applied {
    Item::LibraryInstructions.with(format!("--add-dir {dir}, {CLAUDE_ADDED_DIR_MEMORY}=1"))
}

/// The library's `--add-dir`s of skills (library/folders.rs `session_args`)
/// as applied; nothing without any.
pub(crate) fn skills_applied(skill_args: &[String]) -> Option<Applied> {
    let dirs: Vec<&str> = skill_args.iter().filter(|a| *a != "--add-dir").map(String::as_str).collect();
    (!dirs.is_empty()).then(|| Item::LibrarySkills.with(format!("--add-dir {}", dirs.join(", --add-dir "))))
}

/// What a `--settings` file of `claude_settings`, at `path`, holds, as
/// applied: its hooks, the guard as `guard` says it is there (a chat's
/// choice or a helper's), the core's deny, and the library's.
pub(crate) fn settings_applied(
    path: &str,
    hooks: bool,
    guard: Option<Item>,
    library: &crate::library::SessionSkills,
    core: &str,
) -> Vec<Applied> {
    let mut applied = Vec::new();
    if hooks {
        applied.push(Item::ActivityHooks.with(format!("hooks in --settings {path}")));
    }
    if let Some(guard) = guard {
        applied.push(guard.with(format!("a PreToolUse hook on {} in --settings {path}", crate::guard::TOOLS.replace('|', ", "))));
    }
    applied.push(Item::CoreReadDeny.with(format!("permissions.deny {} in --settings {path}", claude_read_deny(core, cfg!(windows)))));
    if !library.hidden.is_empty() {
        applied.push(Item::LibraryHidden.with(format!("skillOverrides off: {}", library.hidden.join(", "))));
    }
    if !library.excluded.is_empty() {
        applied.push(
            Item::LibraryAccountInstructions
                .with(format!("claudeMdExcludes {}", library.excluded.iter().map(|p| glob_literal(p)).collect::<Vec<_>>().join(", "))),
        );
    }
    applied.extend(library.excluded_kept.iter().map(|note| Item::LibraryAccountInstructionsKept.with(note)));
    applied.extend(crate::library::audience_applied(library));
    if !library.memory_off.is_empty() {
        applied.push(Item::MemoryNarrowed.with(format!("claudeMdExcludes {}", library.memory_off.join(", "))));
    }
    applied.extend(library.memory_kept.iter().map(|note| Item::MemoryNotNarrowed.with(note)));
    applied
}

/// A folder added to a session, as `memory_excludes` takes it: each spelling
/// Claude Code may meet it by (as given, and as the disk resolves it), and
/// the resolved one, which the containment checks go by.
pub(crate) struct AddedDir {
    pub(crate) spellings: Vec<String>,
    pub(crate) resolved: String,
}

/// What `CLAUDE_ADDED_DIR_MEMORY` would have a session read as memory beyond
/// the library's instructions, as `claudeMdExcludes` patterns: the memory
/// files of each folder in `added` — every `--add-dir` but the instructions'
/// own, and those the user's settings add — where the session working in
/// `folder` (resolved) would not read them anyway. Claude Code reads the
/// memory of the folders above the one it works in and loads that of the
/// folders below on demand, so a folder on either side of it is left alone,
/// judged on resolved paths: `<folder>/../shared` is beside it, not in it. The
/// variable is a switch (2.1.318: every additional folder's or none), and
/// these patterns are what narrows it; each folder's own name goes in
/// literally (`glob_literal`).
pub(crate) fn memory_excludes(added: &[AddedDir], folder: &str) -> Vec<String> {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_string();
    let same = |a: &str, b: &str| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
    let within = |inner: &str, outer: &str| {
        same(inner, outer) || (inner.get(..outer.len()).is_some_and(|head| same(head, outer)) && inner[outer.len()..].starts_with('/'))
    };
    let folder = norm(folder);
    let mut out = Vec::new();
    for dir in added {
        let resolved = norm(&dir.resolved);
        if resolved.is_empty() || within(&folder, &resolved) || within(&resolved, &folder) {
            continue;
        }
        for spelled in dir.spellings.iter().map(|s| norm(s)).filter(|s| !s.is_empty()) {
            for file in ["CLAUDE.md", "CLAUDE.local.md", ".claude/CLAUDE.md", ".claude/rules/**"] {
                let pattern = format!("{}/{file}", glob_literal(&spelled));
                if !out.contains(&pattern) {
                    out.push(pattern);
                }
            }
        }
    }
    out
}

/// A folder whose instruction files this launch's Claude Code reads as its
/// own project's: the folder it works in and those below it (`nested`), the
/// folders above it, an added folder left unexcluded, the library's
/// instructions folder.
pub(crate) struct OwnMemory {
    pub(crate) root: String,
    pub(crate) nested: bool,
}

/// `patterns` (from `memory_excludes`) less any that Claude Code's picomatch
/// would also match against one of the launch's `own` instruction files, and
/// a note for each dropped. A pattern of ours matches two things there: the
/// path it names with its escapes read as the characters they stand for, and
/// — picomatch's shortcut for an input spelled exactly like the pattern — the
/// path its own text spells (`E:/w/[[]ab[]]/CLAUDE.md` is also a file in a
/// folder named `[[]ab[]]`). Both readings are judged here, as folders, so a
/// pattern that could take this project's own instructions is not used and
/// the folder it was for is read as the CLI would read it.
pub(crate) fn keep_own_memory(patterns: Vec<String>, own: &[OwnMemory]) -> (Vec<String>, Vec<String>) {
    let norm = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_string();
    let same = |a: &str, b: &str| if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b };
    let within = |inner: &str, outer: &str| {
        same(inner, outer) || (inner.get(..outer.len()).is_some_and(|head| same(head, outer)) && inner[outer.len()..].starts_with('/'))
    };
    let mut kept = Vec::new();
    let mut notes = Vec::new();
    for pattern in patterns {
        // Longest first: a .claude/CLAUDE.md also ends in /CLAUDE.md.
        let folder = ["/.claude/rules/**", "/.claude/CLAUDE.md", "/CLAUDE.local.md", "/CLAUDE.md"]
            .iter()
            .find_map(|file| pattern.strip_suffix(file))
            .unwrap_or(&pattern)
            .to_string();
        let readings = [folder.clone(), glob_unescape(&folder)];
        let hit = own.iter().find_map(|o| {
            let root = norm(&o.root);
            readings.iter().any(|r| if o.nested { within(r, &root) } else { same(r, &root) }).then_some(root)
        });
        match hit {
            Some(root) => {
                let note =
                    format!("could not exclude {}: its name collides with this project's instruction path {root}", glob_unescape(&folder));
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
            None => kept.push(pattern),
        }
    }
    (kept, notes)
}

/// `glob_literal` undone: each one-character bracket class it made, as the
/// character.
fn glob_unescape(pattern: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' && chars.get(i + 2) == Some(&']') {
            out.push(chars[i + 1]);
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// `path` (with `/`) as a `claudeMdExcludes` pattern matching that path alone.
/// Claude Code matches those patterns with picomatch (2.1.318: "Glob patterns
/// or absolute paths of CLAUDE.md files to exclude from loading. Patterns are
/// matched against absolute file paths using picomatch", its settings schema;
/// https://code.claude.com/docs/en/memory#exclude-specific-claudemd-files),
/// after turning every `\` in a pattern into `/`, so a backslash cannot escape
/// anything there: a character that means something goes in a bracket class
/// of its own instead (`[[]` is a literal `[`), and only such a character.
/// picomatch also takes a pattern as matching the input spelled exactly like
/// it, so every needless escape would match one more folder (`a[!]b` matches
/// a folder named `a[!]b` as well as `a!b`). Which characters, found with
/// Claude Code 2.1.319's own bundled picomatch, `{ dot: true }`, POSIX and
/// Windows, on every printable ASCII character, a space and non-ASCII ones,
/// alone and before `(`, `(…|…)`, `{a,b}`, `[ab]` and `*`, each name having to
/// match itself and none of its neighbors: `* ? [ ] { } ( ) |` and `"`, which
/// picomatch reads as quoting. `! @ + ,` are literal there once the `(` an
/// extglob would need, or the braces a list would, are escaped. A folder that
/// does need an escape is still matched by a folder spelled as the escaped
/// pattern (`[[]ab[]]`), picomatch's own shortcut.
pub(crate) fn glob_literal(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '|' | '"' => {
                out.push('[');
                out.push(c);
                out.push(']');
            }
            c => out.push(c),
        }
    }
    out
}

/// Claude Code's flag for the MCP servers of `--mcp-config` alone: none of the
/// account's, the project's, its plugins' or its claude.ai connectors (2.1.319:
/// "MCP servers are restricted to explicitly passed config here, e.g.
/// --strict-mcp-config"). With no `--mcp-config` beside it, no server at all.
pub(crate) const STRICT_MCP: &str = "--strict-mcp-config";

/// What a helper's settings file holds beyond a chat's. A helper is briefed by
/// an agent, not by the person, so it starts with none of the account's reach
/// out of Luna but what it was spawned with:
/// - no auto-memory: what it would save there reaches every later session of
///   the account, unseen. `autoMemoryEnabled: false`, the key Claude Code's
///   memory docs give for it (code.claude.com/docs/en/memory; 2.1.319's own
///   settings schema: "When false, Claude will not read from or write to the
///   auto-memory directory");
/// - unless `connectors`, no claude.ai connectors (`disableClaudeAiConnectors`,
///   settings-reference, honored from any scope), behind `STRICT_MCP`, which
///   leaves them out already: the setting holds should the flag ever not.
///
/// The variable `NO_AUTO_MEMORY_ENV` goes with the setting, `1` in the helper's
/// environment and in this file's `env`: 2.1.319 reads it before the setting,
/// and a `0` there, inherited or from the account's settings, forces
/// auto-memory on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HelperSettings {
    pub(crate) connectors: bool,
}

/// What a helper's settings file and start hold beyond a chat's, as applied:
/// auto-memory off, and unless `connectors` Luna's MCP server alone.
pub(crate) fn helper_settings_applied(h: HelperSettings, path: &str) -> Vec<Applied> {
    let mut applied = vec![Item::HelperNoMemory.with(format!("autoMemoryEnabled: false in {path}, {NO_AUTO_MEMORY_ENV}=1"))];
    if !h.connectors {
        applied.push(Item::HelperMcpAlone.with(format!("{STRICT_MCP}, disableClaudeAiConnectors: true in {path}")));
    }
    applied
}

/// Claude Code's variable that turns auto-memory off (`1`) or on (`0`) above
/// the settings (code.claude.com/docs/en/memory).
pub(crate) const NO_AUTO_MEMORY_ENV: &str = "CLAUDE_CODE_DISABLE_AUTO_MEMORY";

/// What `claude_instructions_dir` asks of a session's start: the folder and
/// the variable that has its `CLAUDE.md` read.
pub(crate) const CLAUDE_ADDED_DIR_MEMORY: &str = "CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD";

/// A hook command posting its stdin to `url`. curl.exe by name on Windows:
/// there `curl` in PowerShell is an alias for Invoke-WebRequest. Its stdin —
/// the hook's JSON — goes up as the body; a 204 answer has no body, so nothing
/// comes back to print, and the guard's refusal is printed for the CLI to read.
fn hook_command(url: &str) -> String {
    let curl = if cfg!(windows) { "curl.exe" } else { "curl" };
    format!("{curl} -s -m 3 -X POST --data-binary @- {url}")
}

/// `claude_settings_file`'s settings: the session's hooks to `hook_url`, the
/// guard's to `guard_url`, the skills and instructions it does not get (the
/// account's, and the other memory `memory_excludes` names), and
/// `core`, Luna's `core/`, denied to its file tools: the core's own folder,
/// no session's business, though nothing in it has let anyone in since the
/// core took its clients by what the OS says of them (peer.rs, ADR-0021). A
/// rule, not a sandbox: Bash, where it runs unasked, still reads anything.
pub(crate) fn claude_settings(
    hook_url: Option<&str>,
    guard_url: Option<&str>,
    library: &crate::library::SessionSkills,
    core: &str,
    helper: Option<HelperSettings>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut settings = serde_json::Map::new();
    settings.insert("permissions".into(), serde_json::json!({ "deny": [claude_read_deny(core, cfg!(windows))] }));
    if let Some(h) = helper {
        settings.insert("autoMemoryEnabled".into(), false.into());
        // Above the account's and the project's `env`, as `--settings` is.
        let env: serde_json::Map<String, serde_json::Value> = [(NO_AUTO_MEMORY_ENV.to_string(), "1".into())].into_iter().collect();
        settings.insert("env".into(), env.into());
        if !h.connectors {
            settings.insert("disableClaudeAiConnectors".into(), true.into());
        }
    }
    let mut hooks = serde_json::Map::new();
    let mut pre_tool_use = Vec::new();
    if let Some(url) = hook_url {
        let command = hook_command(url);
        let hook = |timeout: u32| serde_json::json!([{ "hooks": [{ "type": "command", "command": command, "timeout": timeout }] }]);
        for event in ["UserPromptSubmit", "PostToolUse", "Stop", "StopFailure", "Notification", "SessionEnd", "PermissionRequest"] {
            hooks.insert(event.into(), hook(5));
        }
        // A subagent's life, which a background agent lives past the turn
        // that started it (activity/hooks.rs `HookEvent::AgentAtWork`).
        hooks.insert("SubagentStart".into(), hook(5));
        hooks.insert("SubagentStop".into(), hook(5));
        // What a prompt asks, which neither the screen nor the transcript
        // gives whole: the question and its options (the transcript gets the
        // call only once it is answered), the command up for leave.
        pre_tool_use
            .push(serde_json::json!({ "matcher": "AskUserQuestion", "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }));
    }
    if let Some(url) = guard_url {
        pre_tool_use.push(serde_json::json!({ "matcher": crate::guard::TOOLS, "hooks": [{ "type": "command", "command": hook_command(url), "timeout": 5 }] }));
    }
    if !pre_tool_use.is_empty() {
        hooks.insert("PreToolUse".into(), pre_tool_use.into());
    }
    if !hooks.is_empty() {
        settings.insert("hooks".into(), hooks.into());
    }
    if !library.hidden.is_empty() {
        settings.insert("skillOverrides".into(), library.hidden.iter().map(|n| (n.clone(), serde_json::json!("off"))).collect());
    }
    // The account's instructions file is a path, not a pattern: taken literally.
    let excluded: Vec<String> = library.excluded.iter().map(|p| glob_literal(p)).chain(library.memory_off.iter().cloned()).collect();
    if !excluded.is_empty() {
        settings.insert("claudeMdExcludes".into(), excluded.into());
    }
    settings
}

/// Claude Code's deny rule for reading `dir` and everything in it, `windows`
/// saying whether it runs there: `//` and the path from the root, a Windows
/// drive as a folder of its letter (`E:\x` is `//e/x`), as Claude Code spells
/// a Windows path for its rules (2.1.318), which it matches without regard to
/// case.
pub(crate) fn claude_read_deny(dir: &str, windows: bool) -> String {
    let path = if windows { dir.replace('\\', "/") } else { dir.to_string() };
    let path = path.trim_end_matches('/');
    let path = match path.as_bytes() {
        [drive, b':', b'/', ..] if windows && drive.is_ascii_alphabetic() => {
            format!("{}{}", char::from(drive.to_ascii_lowercase()), &path[2..])
        }
        _ => path.trim_start_matches('/').to_string(),
    };
    format!("Read(//{path}/**)")
}

/// `helper_line` for Claude Code, added to its system prompt.
pub(crate) fn claude_helper_args(parent: &str) -> Vec<String> {
    vec!["--append-system-prompt".into(), crate::pty::helper_line(parent)]
}

/// What `readOnly` takes away from Claude Code. Bash stays, and can still
/// write: it is a checker's tool, and no flag tells its reads from its writes.
pub(crate) const CLAUDE_EDIT_TOOLS: [&str; 3] = ["Edit", "Write", "NotebookEdit"];

/// `readOnly` and `agent` as they show in a Claude Code session's settings,
/// where they are set: a helper opened as a chat takes them from there.
pub(crate) fn with_role(mut settings: serde_json::Value, read_only: bool, agent: Option<&str>) -> serde_json::Value {
    if read_only {
        settings["readOnly"] = true.into();
    }
    if let Some(agent) = agent {
        settings["agent"] = agent.into();
    }
    settings
}

/// What a Claude Code session's MCP header takes its bearer token from, as
/// Codex's `bearer_token_env_var` does: set in the session's environment
/// (`CLAUDE_MCP_TOKEN_ENV`), never written to the config file, where any
/// other session of the user's could read it and act as this one.
pub(crate) const CLAUDE_MCP_TOKEN_ENV: &str = "LUNA_MCP_TOKEN";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_settings_file_denies_the_core_folder_to_the_file_tools() {
        assert_eq!(claude_read_deny(r"E:\Programs\Luna\core", true), "Read(//e/Programs/Luna/core/**)");
        assert_eq!(claude_read_deny("C:/Users/a.b/Lu na/core/", true), "Read(//c/Users/a.b/Lu na/core/**)");
        assert_eq!(claude_read_deny("/home/u/.local/share/luna/core", false), "Read(//home/u/.local/share/luna/core/**)");
        assert_eq!(claude_read_deny(r"/home/u/a\b/core", false), r"Read(//home/u/a\b/core/**)", "a backslash is part of a name there");
        let none = crate::library::SessionSkills::default();
        for (hook, guard) in [(None, None), (Some("http://h/hook/c/s"), None), (None, Some("http://h/guard/c/s"))] {
            let settings = claude_settings(hook, guard, &none, "/d/core", None);
            let want = claude_read_deny("/d/core", cfg!(windows));
            assert_eq!(settings["permissions"], serde_json::json!({ "deny": [want] }), "{hook:?} {guard:?}");
        }
    }
}
