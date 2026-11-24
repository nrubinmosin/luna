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
    pub(crate) permission_mode: Option<&'a str>,
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
    /// The URL its status hooks post to, its secret in it (hub.rs
    /// `hook_url`), which their curl reads from the environment
    /// (codex/hooks.rs `HOOK_URL_ENV`), as a Codex chat's does.
    pub(crate) hook_url: Option<&'a str>,
    /// The chat's role layers (pty/start.rs `claude_instructions_dir`), the
    /// file appended to its system prompt.
    pub(crate) instructions_file: Option<&'a Path>,
    /// Render the chat's system prompt afresh when its launch or transcript
    /// calls for it (`FRESH_PROMPT`).
    pub(crate) fresh_prompt: bool,
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
    if let Some(mode) = s.permission_mode {
        args.extend(["--permission-mode".to_string(), mode.to_string()]);
        applied.push(Item::PermissionMode.with(format!("--permission-mode {mode}")));
    }
    if let Some(session_id) = s.resume {
        args.push("--resume".into());
        args.push(session_id.to_string());
        applied.push(Item::Resume.with(format!("--resume {session_id}")));
    }
    if let Some(name) = s.worktree {
        let arg = crate::checkout::worktree_arg(name);
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
        // Variadic like `--mcp-config`, so a flag follows it.
        let (allowed, mcp) = (luna_allowed_tools(), path(mcp));
        applied.push(Item::LunaTools.with(luna_tools_value(&mcp, &allowed)));
        args.push("--allowedTools".into());
        args.push(allowed);
        args.push("--mcp-config".into());
        args.push(mcp);
    }
    // The notes folder is read by a rule in `--settings` (`notes_allow`), not
    // given by `--add-dir`: since 2.1.326 an interactive session asks to trust
    // each added folder at every start, and does not remember the answer.
    if let Some(file) = s.instructions_file {
        args.push("--append-system-prompt-file".into());
        args.push(path(file));
        applied.push(chat_instructions_applied(&path(file)));
    }
    if s.fresh_prompt {
        args.extend(FRESH_PROMPT.iter().map(|a| a.to_string()));
        applied.push(Item::FreshPrompt.with(FRESH_PROMPT.join(" ")));
    }
    args.extend(s.skill_args.iter().cloned());
    applied.extend(skills_applied(s.skill_args));
    let mut envs = vec![
        ("CLAUDE_CONFIG_DIR", s.account_path.to_string()),
        // Luna updates the CLI itself; the CLI's own updater would install a
        // second copy under the user profile that nothing here ever runs.
        ("DISABLE_AUTOUPDATER", "1".to_string()),
        (NO_FLICKER_ENV, "1".to_string()),
    ];
    applied.push(Item::Fullscreen.with(match s.settings {
        Some((settings, _)) => format!("tui: {FULLSCREEN} in --settings {settings}, {NO_FLICKER_ENV}=1"),
        None => format!("{NO_FLICKER_ENV}=1"),
    }));
    if let Some((_, token)) = s.mcp {
        envs.push((CLAUDE_MCP_TOKEN_ENV, token.to_string()));
    }
    if let Some(url) = s.hook_url {
        envs.push((crate::codex::hooks::HOOK_URL_ENV, url.to_string()));
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
pub(crate) fn luna_tools_value(config: &str, allowed: &str) -> String {
    format!("--mcp-config {config}, --allowedTools {allowed}; its token in {CLAUDE_MCP_TOKEN_ENV}, not shown")
}

/// Spawn follows Claude Code's permission mode. An explicit ask rule would
/// override even auto and bypassPermissions.
pub(crate) fn luna_allowed_tools() -> String {
    crate::mcp::tool_names()
        .into_iter()
        .filter(|name| name != "luna_session_spawn")
        .map(|name| format!("mcp__luna__{name}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A helper's role layers as applied: their folder, with the variable that
/// has it read as memory.
pub(crate) fn instructions_applied(dir: &str) -> Applied {
    Item::LibraryInstructions.with(format!("--add-dir {dir}, {CLAUDE_ADDED_DIR_MEMORY}=1"))
}

/// A chat's role layers as applied: the file appended to its system prompt.
pub(crate) fn chat_instructions_applied(file: &str) -> Applied {
    Item::LibraryInstructions.with(format!("--append-system-prompt-file {file}"))
}

/// Render the system prompt fresh for chats with role layers, Luna tools,
/// library plugins, a named agent, or a resume that may replay Luna tools.
/// A plain chat uses the CLI's default snapshot to keep the prompt cache warm.
pub(crate) const FRESH_PROMPT: [&str; 2] = ["--system-prompt-snapshot", "off"];

/// The library's skills for the session as applied: its `--plugin-dir` of
/// folder skills (library/session_plugin.rs `give_plugins`); nothing without
/// one.
pub(crate) fn skills_applied(skill_args: &[String]) -> Option<Applied> {
    let given: Vec<String> = skill_args.windows(2).step_by(2).map(|pair| format!("{} {}", pair[0], pair[1])).collect();
    (!given.is_empty()).then(|| Item::LibrarySkills.with(given.join(", ")))
}

/// What a `--settings` file of `claude_settings`, at `path`, holds, as
/// applied: its hooks, the core's deny, and the library's.
pub(crate) fn settings_applied(
    path: &str,
    hooks: bool,
    library: &crate::library::SessionSkills,
    core: &str,
    notes: Option<&str>,
) -> Vec<Applied> {
    let mut applied = Vec::new();
    if hooks {
        let env = crate::codex::hooks::HOOK_URL_ENV;
        applied.push(Item::ActivityHooks.with(format!("hooks in --settings {path}; their URL in {env}, not shown")));
    }
    applied.push(Item::CoreReadDeny.with(format!("permissions.deny {} in --settings {path}", claude_read_deny(core, cfg!(windows)))));
    if let Some(notes) = notes {
        applied.push(Item::NotesDir.with(format!("permissions.allow {} in --settings {path}", read_rule(notes, cfg!(windows)))));
    }
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

/// A status hook's command: a Codex chat's (codex/hooks.rs `command`), by
/// the curl at `curl`, an absolute path found at the launch (pty/start.rs
/// `codex_hook_curl`): quiet, no `.curlrc`, no proxy, the hook's JSON posted
/// to the URL in `HOOK_URL_ENV`, which curl reads itself, so the hub's
/// secret is on no command line and in no file. And it ends 0 whatever curl
/// does: Claude Code takes a hook's exit 2 as a block (the prompt refused,
/// the tool stopped, the turn kept going), and, where PowerShell stands in
/// for a Git Bash it did not find, any other failure of a tool's hook too
/// (2.1.330). On Windows, the shell may diagnose a missing or killed curl
/// on stderr. A successful hook's model-facing content comes from stdout.
pub(crate) fn hook_command(curl: &str) -> String {
    if cfg!(windows) {
        format!("{}; exit 0", crate::codex::hooks::command(&curl.replace('\\', "/"), true))
    } else {
        format!("exec >/dev/null 2>&1; {}; exit 0", crate::codex::hooks::command(curl, false))
    }
}

/// A status hook: `hook_command`, and what does not reach its own exit (a
/// shell that will not start, a hook timed out) left to pass, which Claude
/// Code otherwise takes for a block where PowerShell stands in for Git Bash
/// (2.1.330).
pub(crate) fn status_hook(curl: &str) -> serde_json::Value {
    serde_json::json!({ "type": "command", "command": hook_command(curl), "timeout": 5, "onFailure": "continue" })
}

/// `claude_settings_file`'s settings: the session's hooks, run by `curl`, the
/// skills and instructions it does not get (the account's, and the other
/// memory `memory_excludes` names), `FULLSCREEN` for a session that draws into
/// a terminal (no `helper`, which has none), and `core`, Luna's `core/`,
/// denied to its file tools: the core's own folder,
/// no session's business, though nothing in it has let anyone in since the
/// core took its clients by what the OS says of them (peer.rs, ADR-0021). A
/// rule, not a sandbox: Bash, where it runs unasked, still reads anything.
pub(crate) fn claude_settings(
    curl: Option<&str>,
    library: &crate::library::SessionSkills,
    core: &str,
    helper: Option<HelperSettings>,
) -> serde_json::Map<String, serde_json::Value> {
    let mut settings = serde_json::Map::new();
    settings.insert("permissions".into(), serde_json::json!({ "deny": [claude_read_deny(core, cfg!(windows))] }));
    if helper.is_none() {
        settings.insert("tui".into(), FULLSCREEN.into());
    }
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
    if let Some(curl) = curl {
        let handler = status_hook(curl);
        let hook = || serde_json::json!([{ "hooks": [handler] }]);
        let only = |matcher: &str| serde_json::json!([{ "matcher": matcher, "hooks": [handler] }]);
        for event in ["UserPromptSubmit", "PostToolUse", "Stop", "StopFailure", "SessionEnd", "PermissionRequest"] {
            hooks.insert(event.into(), hook());
        }
        // The two notifications Luna reads (activity/hooks.rs): a permission
        // prompt or a question up. Not `idle_prompt` and the rest, which
        // would each run a curl for nothing.
        hooks.insert("Notification".into(), only("permission_prompt|elicitation_dialog"));
        // A subagent's life, which a background agent lives past the turn
        // that started it (activity/hooks.rs `HookEvent::AgentAtWork`).
        hooks.insert("SubagentStart".into(), hook());
        hooks.insert("SubagentStop".into(), hook());
        // What a prompt asks, which neither the screen nor the transcript
        // gives whole: the question and its options (the transcript gets the
        // call only once it is answered), the command up for leave.
        hooks.insert("PreToolUse".into(), only("AskUserQuestion"));
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

/// Claude Code's `tui` setting that draws on the alternate screen with its own
/// scrollback: no flicker, and a resize of the pane redraws cleanly. Passed in
/// every terminal session's `--settings`, with `NO_FLICKER_ENV`, never written
/// into the account (2.1.325, docs/cli-compatibility.md).
pub(crate) const FULLSCREEN: &str = "fullscreen";

/// The variable that asks for the same renderer, and outranks the CLI's own
/// `fullscreenAutoDisabled` in the account's `.claude.json`, which the CLI sets
/// after launches that did not finish.
pub(crate) const NO_FLICKER_ENV: &str = "CLAUDE_CODE_NO_FLICKER";

/// Claude Code's deny rule for reading `dir` and everything in it
/// (`read_rule`), `windows` saying whether it runs there.
pub(crate) fn claude_read_deny(dir: &str, windows: bool) -> String {
    read_rule(dir, windows)
}

/// A chat's notes folder (agents/delivery.rs `notes_dir`), readable without a
/// prompt: a long message from another session comes as a file there
/// (`spill`), and so does a helper's answer named in a notice. A rule in
/// `--settings` rather than an `--add-dir`, which since 2.1.326 makes an
/// interactive session ask at every start whether to trust the folder.
/// Read alone: nothing there is loaded as skills, commands or memory, and
/// the session's other tools ask as they would anywhere else.
pub(crate) fn notes_allow(settings: &mut serde_json::Map<String, serde_json::Value>, notes: &str) {
    let rule = serde_json::Value::from(read_rule(notes, cfg!(windows)));
    let permissions = settings.entry("permissions").or_insert_with(|| serde_json::json!({}));
    match permissions.get_mut("allow").and_then(|a| a.as_array_mut()) {
        Some(allow) => allow.push(rule),
        None => permissions["allow"] = serde_json::json!([rule]),
    }
}

/// A `Read` rule for `dir` and everything in it: `//` and the path from the
/// root, a Windows drive as a folder of its letter (`E:\x` is `//e/x`), as
/// Claude Code spells a Windows path for its rules (2.1.318), which it
/// matches without regard to case.
pub(crate) fn read_rule(dir: &str, windows: bool) -> String {
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
    fn a_chats_notes_are_allowed_by_a_read_rule_beside_the_core_deny() {
        let none = crate::library::SessionSkills::default();
        let mut settings = claude_settings(None, &none, "/d/core", None);
        notes_allow(&mut settings, "/d/agents");
        let deny = claude_read_deny("/d/core", cfg!(windows));
        let allow = read_rule("/d/agents", cfg!(windows));
        assert_eq!(settings["permissions"], serde_json::json!({ "deny": [deny], "allow": [allow.clone()] }));
        assert_eq!(read_rule(r"E:\Programs\Luna\agents", true), "Read(//e/Programs/Luna/agents/**)");
        // An allow list already there keeps its rules.
        settings["permissions"]["allow"] = serde_json::json!(["Read(//x/**)"]);
        notes_allow(&mut settings, "/d/agents");
        assert_eq!(settings["permissions"]["allow"], serde_json::json!(["Read(//x/**)", allow]));
    }

    #[test]
    fn every_settings_file_denies_the_core_folder_to_the_file_tools() {
        assert_eq!(claude_read_deny(r"E:\Programs\Luna\core", true), "Read(//e/Programs/Luna/core/**)");
        assert_eq!(claude_read_deny("C:/Users/a.b/Lu na/core/", true), "Read(//c/Users/a.b/Lu na/core/**)");
        assert_eq!(claude_read_deny("/home/u/.local/share/luna/core", false), "Read(//home/u/.local/share/luna/core/**)");
        assert_eq!(claude_read_deny(r"/home/u/a\b/core", false), r"Read(//home/u/a\b/core/**)", "a backslash is part of a name there");
        let none = crate::library::SessionSkills::default();
        for hook in [None, Some("/usr/bin/curl")] {
            let settings = claude_settings(hook, &none, "/d/core", None);
            let want = claude_read_deny("/d/core", cfg!(windows));
            assert_eq!(settings["permissions"], serde_json::json!({ "deny": [want] }), "{hook:?}");
        }
    }

    /// Every status hook runs Codex's curl command: quiet, off any proxy,
    /// the URL and its secret in no argument; the notifications only that
    /// Luna reads.
    #[test]
    fn a_chats_status_hooks_run_the_hardened_curl() {
        let none = crate::library::SessionSkills::default();
        // On Windows the path goes with forward slashes, which Git Bash and
        // PowerShell both take.
        let (found, runs) = if cfg!(windows) {
            (r"C:\WINDOWS\System32\curl.exe", "C:/WINDOWS/System32/curl.exe")
        } else {
            ("/usr/bin/curl", "/usr/bin/curl")
        };
        let settings = claude_settings(Some(found), &none, "/d/core", None);
        let hooks = settings["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 10);
        assert_eq!(hooks["Notification"][0]["matcher"], "permission_prompt|elicitation_dialog");
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "AskUserQuestion");
        let prefix = if cfg!(windows) { "" } else { "exec >/dev/null 2>&1; " };
        let command = format!("{prefix}{}; exit 0", crate::codex::hooks::command(runs, cfg!(windows)));
        for hook in hooks.values() {
            let want = serde_json::json!({ "type": "command", "command": command, "timeout": 5, "onFailure": "continue" });
            assert_eq!(hook[0]["hooks"][0], want);
        }
        for part in [format!("{runs} -q "), "--noproxy \"*\"".into(), "--expand-url \"{{LUNA_HOOK_URL}}\"".into()] {
            assert!(command.contains(part.as_str()), "{command}");
        }
        let held = settings_applied("/s.json", true, &none, "/d/core", None);
        assert_eq!(held[0].value, "hooks in --settings /s.json; their URL in LUNA_HOOK_URL, not shown");
    }

    /// `command` run as Claude Code runs a hook, in each shell it may: `sh`
    /// off Windows, there whichever of Git Bash, Windows PowerShell and pwsh
    /// this machine has, PowerShell's with the end Claude Code adds to it
    /// (2.1.330). The shell's name, how it ended, and what it printed.
    fn in_each_shell(command: &str, url: Option<&str>) -> Vec<(String, std::process::Output)> {
        in_each_shell_with(command, url, |_| {})
    }

    fn in_each_shell_with(command: &str, url: Option<&str>, started: impl Fn(&std::process::Child)) -> Vec<(String, std::process::Output)> {
        let mut shells: Vec<(String, Vec<String>)> = Vec::new();
        if cfg!(windows) {
            let path: Vec<_> = std::env::split_paths(&std::env::var_os("PATH").unwrap()).collect();
            let has = |dir: &Path, exe: &str| Some(dir.join(exe)).filter(|p| p.is_file());
            let git = path.iter().filter(|d| d.join("git.exe").is_file());
            let bash = std::env::var_os("CLAUDE_CODE_GIT_BASH_PATH")
                .map(std::path::PathBuf::from)
                .filter(|p| p.is_file())
                .or_else(|| git.flat_map(|d| d.ancestors().take(4)).find_map(|d| has(d, r"bin\bash.exe")))
                .or_else(|| {
                    ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"].into_iter().filter_map(std::env::var_os).find_map(|dir| {
                        has(Path::new(&dir), r"Git\bin\bash.exe").or_else(|| has(Path::new(&dir), r"Programs\Git\bin\bash.exe"))
                    })
                });
            if let Some(bash) = bash {
                shells.push((bash.to_string_lossy().into(), vec!["-c".into(), command.into()]));
            } else {
                eprintln!("skipped Git Bash: Git for Windows is not installed where Claude Code looks");
            }
            let end = "\n; if (-not $?) { if ($LASTEXITCODE -eq 2) { exit 2 } }\n; if (-not $?) { exit 1 }";
            let flags = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"].map(String::from);
            let args: Vec<String> = flags.into_iter().chain([format!("{command}{end}")]).collect();
            let system = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
            if let Some(powershell) = has(&system, r"WindowsPowerShell\v1.0\powershell.exe") {
                shells.push((powershell.to_string_lossy().into(), args.clone()));
            } else {
                eprintln!("skipped Windows PowerShell: powershell.exe is missing from System32");
            }
            let pwsh = path.iter().find_map(|d| has(d, "pwsh.exe")).or_else(|| {
                ["ProgramFiles", "ProgramFiles(x86)"]
                    .into_iter()
                    .filter_map(std::env::var_os)
                    .find_map(|dir| has(Path::new(&dir), r"PowerShell\7\pwsh.exe"))
            });
            if let Some(pwsh) = pwsh {
                shells.push((pwsh.to_string_lossy().into(), args));
            } else {
                eprintln!("skipped pwsh: pwsh.exe is absent from PATH and the standard installation folders");
            }
        } else {
            shells.push(("/bin/sh".into(), vec!["-c".into(), command.into()]));
        }
        let run = |(shell, args): (String, Vec<String>)| {
            use std::io::Write;
            let mut cmd = std::process::Command::new(&shell);
            cmd.args(args).env_remove("LUNA_HOOK_URL").envs(url.map(|u| ("LUNA_HOOK_URL", u)));
            let piped = std::process::Stdio::piped;
            let mut child = cmd.stdin(piped()).stdout(piped()).stderr(piped()).spawn().unwrap();
            // A command that ends without reading it breaks the pipe.
            let _ = child.stdin.take().unwrap().write_all(b"{\"hook_event_name\":\"Stop\"}\n");
            started(&child);
            (shell, child.wait_with_output().unwrap())
        };
        shells.into_iter().map(run).collect()
    }

    /// Curl's own failures stay quiet and the Claude hook exits 0. Codex
    /// blocks on exit 2 only with a reason on stderr. Use this machine's curl
    /// where it has a version that takes the hook command.
    #[test]
    fn a_status_hook_that_cannot_post_is_silent_and_ends_0() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let found = crate::pty::codex_hook_curl();
        assert!(found.is_some() || cfg!(not(windows)), "Windows has a curl of its own");
        let Some(curl) = found else { return };
        let url = |port: u16| format!("http://127.0.0.1:{port}/hook/c/secret");
        // Answers with a body, which stays out of the hook's stdout.
        let hub = TcpListener::bind("127.0.0.1:0").unwrap();
        let posted = url(hub.local_addr().unwrap().port());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for conn in hub.incoming() {
                let mut conn = conn.unwrap();
                let (mut got, mut buf) = (Vec::new(), [0; 4096]);
                while !got.ends_with(b"}\n") {
                    match conn.read(&mut buf) {
                        Ok(n) if n > 0 => got.extend_from_slice(&buf[..n]),
                        _ => break,
                    }
                }
                let _ = conn.write_all(b"HTTP/1.1 500 No\r\nContent-Length: 5\r\nConnection: close\r\n\r\nnoise");
                let _ = tx.send(String::from_utf8_lossy(&got).into_owned());
            }
        });
        // Takes connections and answers none.
        let mute = TcpListener::bind("127.0.0.1:0").unwrap();
        let mute = (url(mute.local_addr().unwrap().port()), mute);
        let closed = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();

        let ours = hook_command(&curl);
        let codex = crate::codex::hooks::command(&curl, cfg!(windows));
        // An option this curl has never heard of: what `--variable` is to one
        // before 8.3.
        let old = |command: &str| command.replace("--variable", "--no-such-option");
        for (what, url, broken) in [
            ("answered", Some(posted.as_str()), false),
            ("the variable unset", None, false),
            ("refused", Some(url(closed).as_str()), false),
            ("timed out", Some(mute.0.as_str()), false),
            ("a curl too old", Some(posted.as_str()), true),
        ] {
            let command = if broken { old(&ours) } else { ours.clone() };
            for (shell, out) in in_each_shell(&command, url) {
                assert_eq!(out.status.code(), Some(0), "{what}, {shell}");
                assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{what}, {shell}: both streams empty");
                if what == "answered" {
                    let got = rx.recv_timeout(std::time::Duration::from_secs(10)).expect(&shell);
                    assert!(
                        got.starts_with("POST /hook/c/secret ") && got.ends_with("\r\n\r\n{\"hook_event_name\":\"Stop\"}\n"),
                        "{shell}: request reached the hook endpoint with its body"
                    );
                }
            }
            let command = if broken { old(&codex) } else { codex.clone() };
            // Codex runs Windows PowerShell or pwsh on Windows.
            for (shell, out) in in_each_shell(&command, url).into_iter().filter(|(s, _)| !s.ends_with("bash.exe")) {
                assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{what}, Codex in {shell}: both streams empty");
                if what == "answered" {
                    rx.recv_timeout(std::time::Duration::from_secs(10)).expect(&shell);
                }
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_missing_or_terminated_curl_exits_0_in_windows_hook_shells() {
        use std::net::TcpListener;
        use std::time::{Duration, Instant};
        let curl = crate::pty::codex_hook_curl().expect("System32 curl 8.3 or later");
        let gone = Path::new(&curl).with_file_name(format!("luna-absent-{}.exe", std::process::id()));
        assert!(!gone.exists());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let missing = in_each_shell(&hook_command(gone.to_str().unwrap()), Some(&url));
        let terminated = in_each_shell_with(&hook_command(&curl), Some(&url), |shell| {
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                if let Some((child, _)) = crate::procs::listed(None, Some(shell.id()))
                    .and_then(|children| children.into_iter().find(|(_, name)| name.eq_ignore_ascii_case("curl.exe")))
                {
                    assert!(crate::procs::end(child.pid, child.started_ms), "could not terminate curl");
                    break;
                }
                assert!(Instant::now() < deadline, "the shell did not start curl");
                std::thread::sleep(Duration::from_millis(15));
            }
        });
        assert!(!missing.is_empty(), "no Windows hook shell was found");
        // Only these two cases may leave shell diagnostics on stderr.
        // The refused, timed-out and old-curl cases above require both streams empty.
        for (case, outputs) in [("missing", missing), ("terminated", terminated)] {
            for (shell, out) in outputs {
                assert_eq!(out.status.code(), Some(0), "{case}, {shell}");
                assert!(out.stdout.is_empty(), "{case}, {shell}: stdout empty");
            }
        }
    }

    /// And off Windows whatever it is that stands at curl's path: one that
    /// prints and fails of its own, is killed, or is gone.
    #[cfg(unix)]
    #[test]
    fn no_curl_gets_a_word_or_an_exit_code_past_the_hook() {
        let dir = std::env::temp_dir().join(format!("luna-hook-curl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (i, end) in ["exit 2", "exit 1", "exit 28", "kill -9 $$", "exit 0"].into_iter().enumerate() {
            let curl = dir.join(format!("curl{i}"));
            // Written by a process of its own: a file this one had open to
            // write, a child another test forks may hold, and it does not run.
            let write = format!("printf '#!/bin/sh\\necho out\\necho err >&2\\n{end}\\n' > {0} && chmod 755 {0}", curl.display());
            assert!(std::process::Command::new("/bin/sh").args(["-c", &write]).status().unwrap().success());
            for (_, out) in in_each_shell(&hook_command(curl.to_str().unwrap()), Some("http://127.0.0.1:9/")) {
                assert_eq!(out.status.code(), Some(0), "{end}");
                assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{end}: both streams empty");
            }
        }
        for (_, out) in in_each_shell(&hook_command(dir.join("gone").to_str().unwrap()), None) {
            assert_eq!(out.status.code(), Some(0), "gone");
            assert!(out.stdout.is_empty() && out.stderr.is_empty(), "gone: both streams empty");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
