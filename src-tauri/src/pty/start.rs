//! Starting a chat's CLI in a pane: what each provider's start settles under
//! the chat's spawn gate (see `spawn`) — its MCP token, the settings, MCP
//! and instructions files it reads at start, its worktree, its place in the
//! agents' registry, what its sandbox takes from this machine —
//! before the arguments are laid out by `claude::launch` and `codex::launch`,
//! which only lay out what they are handed, and the process is run. A
//! helper's turn (background/command.rs) settles the same through the
//! functions here.

use super::folder::folder_refusal;
use super::same_dir;
use super::spawn::{spawn, Attached, Launch};
use crate::claude::launch::{self as claude_launch, claude_settings, with_role, HelperSettings, CLAUDE_MCP_TOKEN_ENV};
use crate::codex::launch::{self as codex_launch, AUTO_REVIEW, AUTO_REVIEW_SANDBOX, READ_ONLY_NET};
use crate::provider::Provider;

/// The settings a Claude Code session is started with, as its `--settings`
/// (one only: a second takes the first's place). `hooks`: each event posts
/// its stdin to Luna's loopback listener (hub.rs) and prints nothing — a
/// hook's stdout is added to the model's context, silence is free — while
/// the listener is on. `library`: what the
/// library's folders keep from the session (library/folders.rs
/// `SessionSkills`) — skills off in `skillOverrides`, which settings of this
/// source beat whatever the account's or the project's say, and the
/// account's instructions and the other memory in `claudeMdExcludes`.
/// `notes`: a chat's notes folder, readable without a prompt (claude/launch.rs
/// `notes_allow`). Always: Luna's `core/` denied to its file tools (`claude_read_deny`), so
/// there is always a file to write. Written to a file under the data dir
/// (`hooks/`, named for what it first held, the chat's id spelled as a file
/// name by `paths::id_file_name`) rather than passed inline, so the command
/// line stays readable and quoting is not a concern. Err if it cannot
/// be written: a session carries the core-folder deny or does not start
/// (threat model F3), so the caller refuses the launch rather than run without
/// it. With the file, what it holds as applied (claude/launch.rs
/// `settings_applied`).
pub(crate) fn claude_settings_file(
    chat_id: &str,
    hooks: bool,
    library: &crate::library::SessionSkills,
    helper: Option<HelperSettings>,
    notes: Option<&std::path::Path>,
) -> Result<(std::path::PathBuf, Vec<crate::applied::Applied>), String> {
    let core = crate::ipc::core_dir().to_string_lossy().into_owned();
    let hook_url = crate::hub::hook_url(chat_id).filter(|_| hooks);
    let notes = notes.map(|n| n.to_string_lossy().into_owned());
    let mut settings = claude_settings(hook_url.as_deref(), library, &core, helper);
    if let Some(notes) = &notes {
        claude_launch::notes_allow(&mut settings, notes);
    }
    // Owner-only: the hook URLs in it carry the hub's secret.
    let dir = crate::paths::data_dir().join("hooks");
    crate::secret::private_dir(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let path = dir.join(format!("{}.json", crate::paths::id_file_name(chat_id)));
    let json = serde_json::to_string_pretty(&settings).map_err(|e| format!("the settings do not serialize: {e}"))?;
    crate::secret::write_private(&path, json.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut applied = claude_launch::settings_applied(&path.to_string_lossy(), hook_url.is_some(), library, &core, notes.as_deref());
    applied.extend(helper.map(|h| claude_launch::helper_settings_applied(h, &path.to_string_lossy())).unwrap_or_default());
    Ok((path, applied))
}

/// A login session's `--settings` (the providers' `loginSession`): the
/// core-folder deny every session carries (threat model F3) and nothing else,
/// as inline JSON, which Claude Code reads in place of a file's path when it
/// is wrapped in braces (2.1.319). A sign-in has no use for hooks, tools or
/// the library, so it is given no file of Luna's at all; its id,
/// `login:<provider>:<name>`, once named one (`hooks/login:claude:<name>.json`),
/// which Windows refuses (os error 123), and the login never started. With
/// what it holds as applied.
pub(crate) fn claude_login_settings() -> (String, Vec<crate::applied::Applied>) {
    let core = crate::ipc::core_dir().to_string_lossy().into_owned();
    let none = crate::library::SessionSkills::default();
    let json = serde_json::Value::Object(claude_settings(None, &none, &core, None)).to_string();
    let applied = claude_launch::settings_applied(&json, false, &none, &core, None);
    (json, applied)
}

/// The folders a Claude Code session in `folder` on `account_path` would be
/// given by the user's own settings (`permissions.additionalDirectories` in
/// the account's, the project's and the project's local settings), each made
/// absolute against `folder`: for `memory_excludes`, as the library's
/// instructions have the memory of every added folder read.
pub(crate) fn claude_settings_dirs(account_path: &str, folder: &str) -> Vec<String> {
    let base = std::path::Path::new(folder);
    let files = [
        std::path::Path::new(account_path).join("settings.json"),
        base.join(".claude").join("settings.json"),
        base.join(".claude").join("settings.local.json"),
    ];
    let mut dirs = Vec::new();
    for file in files {
        let Some(v) = crate::claude::defaults::read_settings(&file) else { continue };
        for d in v["permissions"]["additionalDirectories"].as_array().into_iter().flatten().filter_map(|d| d.as_str()) {
            dirs.push(base.join(d).to_string_lossy().into_owned());
        }
    }
    dirs
}

/// What `memory_excludes` leaves out of a Claude Code session in `folder`
/// given the library's instructions (in `instructions`): every other folder
/// it is added, by Luna (`luna_dirs`) or by the user's settings. Fail-safe:
/// a pattern that could also match one of this session's own instruction
/// files (`keep_own_memory`) is dropped, and said in the second list.
pub(crate) fn claude_memory_off(
    luna_dirs: &[String],
    account_path: &str,
    folder: &str,
    instructions: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    let added = claude_added_dirs(luna_dirs, account_path, folder);
    let patterns = claude_launch::memory_excludes(&added, &resolved_path(folder));
    let own = own_memory(folder, &added, &patterns, instructions);
    claude_launch::keep_own_memory(patterns, &own)
}

/// `excluded`, the account's instructions file that a folder or an entry's
/// audience leaves out of a Claude Code session (library/folders.rs
/// `folder_args`), less one whose literal pattern Claude Code's matcher would
/// also take for one of the session's own instruction files
/// (`keep_own_memory`): that file is read after all, said in the second list.
/// The others' memory counts as the session's own only where the library's
/// instructions have it read (`instructions`).
pub(crate) fn claude_account_excludes(
    excluded: Vec<String>,
    luna_dirs: &[String],
    account_path: &str,
    folder: &str,
    instructions: Option<&str>,
) -> (Vec<String>, Vec<String>) {
    if excluded.is_empty() {
        return (excluded, Vec::new());
    }
    let (added, patterns) = match instructions {
        Some(_) => {
            let added = claude_added_dirs(luna_dirs, account_path, folder);
            let patterns = claude_launch::memory_excludes(&added, &resolved_path(folder));
            (added, patterns)
        }
        None => (Vec::new(), Vec::new()),
    };
    let own = own_memory(folder, &added, &patterns, instructions);
    let (mut kept, mut notes) = (Vec::new(), Vec::new());
    for path in excluded {
        let (safe, said) = claude_launch::keep_own_memory(vec![claude_launch::glob_literal(&path)], &own);
        if safe.is_empty() {
            notes.extend(said);
        } else {
            kept.push(path);
        }
    }
    (kept, notes)
}

/// Every folder a Claude Code session is added beside the one it works in:
/// Luna's (`luna_dirs`) and the user's settings', as `memory_excludes` takes
/// them.
fn claude_added_dirs(luna_dirs: &[String], account_path: &str, folder: &str) -> Vec<claude_launch::AddedDir> {
    let mut added = luna_dirs.to_vec();
    added.extend(claude_settings_dirs(account_path, folder));
    added.iter().map(|d| added_dir(d)).collect()
}

/// The session's own instruction folders, each by every spelling it may be met
/// by: where it works and below, every folder above, the added folders none of
/// `patterns` leaves out (read as ever), the library's instructions.
fn own_memory(
    folder: &str,
    added: &[claude_launch::AddedDir],
    patterns: &[String],
    instructions: Option<&str>,
) -> Vec<claude_launch::OwnMemory> {
    use claude_launch::OwnMemory;
    let mut own = Vec::new();
    for spelled in [lexical_path(folder), resolved_path(folder)] {
        let mut dir = Some(std::path::Path::new(&spelled));
        let mut nested = true;
        while let Some(d) = dir {
            own.push(OwnMemory { root: d.to_string_lossy().replace('\\', "/"), nested });
            nested = false;
            dir = d.parent();
        }
    }
    for dir in added {
        let left_alone =
            dir.spellings.iter().all(|s| !patterns.iter().any(|p| p.contains(&claude_launch::glob_literal(&s.replace('\\', "/")))));
        if left_alone {
            own.extend(dir.spellings.iter().map(|s| OwnMemory { root: s.replace('\\', "/"), nested: true }));
        }
    }
    own.extend(instructions.map(|i| OwnMemory { root: i.replace('\\', "/"), nested: false }));
    own
}

/// `dir` as `memory_excludes` takes it: as given with `.` and `..` worked out,
/// and as the disk resolves it (links followed), each a spelling Claude Code
/// may load its files by.
fn added_dir(dir: &str) -> claude_launch::AddedDir {
    let lexical = lexical_path(dir);
    let resolved = resolved_path(dir);
    let mut spellings = vec![lexical];
    if !spellings.contains(&resolved) {
        spellings.push(resolved.clone());
    }
    claude_launch::AddedDir { spellings, resolved }
}

/// `path` as the disk resolves it — links, junctions, `..`, short names and
/// case — as far as it is there, the rest as written with `.` and `..` worked
/// out; without Windows' `\\?\` prefix, which no path Claude Code reports
/// carries.
fn resolved_path(path: &str) -> String {
    let lexical = std::path::PathBuf::from(lexical_path(path));
    let mut base = lexical.as_path();
    let mut rest = Vec::new();
    let canonical = loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            break Some(c);
        }
        match (base.parent(), base.file_name()) {
            (Some(up), Some(name)) => {
                rest.push(name.to_owned());
                base = up;
            }
            _ => break None,
        }
    };
    let Some(mut full) = canonical else {
        return lexical.to_string_lossy().into_owned();
    };
    full.extend(rest.iter().rev());
    let p = full.to_string_lossy().into_owned();
    match p.strip_prefix(r"\\?\UNC\") {
        Some(unc) => format!(r"\\{unc}"),
        None => p.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(p),
    }
}

/// `path` with `.` and `..` worked out on the text alone.
fn lexical_path(path: &str) -> String {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in std::path::Path::new(path).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out.to_string_lossy().into_owned()
}

/// A Claude Code session's role layers (library/role.rs `role_layers`: the
/// instructions a folder of the library gives in place of its account's, its
/// role's default, the library's instructions for its role, its
/// constraints), as the `CLAUDE.md` of a folder of its own (`hooks/<id>/`).
/// A helper is given the folder by `--add-dir` and reads it as memory, with
/// `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD` set — that of every folder
/// it is given then, Luna's own holding none: memory outweighs
/// `--append-system-prompt`, and a resume reads it again. A chat is given the
/// file alone, by `--append-system-prompt-file`, and a fresh prompt
/// (`FRESH_PROMPT`): since 2.1.326 an interactive session asks at every start
/// whether to trust each added folder, and a `-p` helper does not.
/// Written at every start and every
/// helper turn, so an edit reaches a helper at its next turn and a chat at
/// its next start; with nothing to give, gone, and None. Err, with why, where
/// it could not be written: the caller decides whether the session may start
/// without them.
pub(crate) fn claude_instructions_dir(chat_id: &str, text: Option<&str>) -> Result<Option<std::path::PathBuf>, String> {
    let dir = crate::paths::data_dir().join("hooks").join(crate::paths::id_file_name(chat_id));
    let Some(text) = text else {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(None);
    };
    // `hooks/` made owner-only first, if this is what makes it: the settings
    // files beside this folder carry the hub's secret.
    let hooks = dir.parent().map_or(Ok(()), crate::secret::private_dir);
    let written = hooks
        .and_then(|()| std::fs::create_dir_all(&dir))
        .and_then(|()| crate::secret::write_private(&dir.join("CLAUDE.md"), text.as_bytes()));
    written.map(|()| Some(dir.clone())).map_err(|e| {
        crate::log::warn("library", &format!("could not give {chat_id} its role layers: {e}"));
        format!("Luna could not write its role layers to {}: {e}", dir.join("CLAUDE.md").display())
    })
}

/// The MCP config a tooled Claude Code session starts with: Luna's server on
/// the hub listener, the bearer token in the header as `${LUNA_MCP_TOKEN}`,
/// which Claude Code expands from its environment as it connects (seen on
/// 2.1.319, `--mcp-config` included). A file per chat under the data dir,
/// like the hooks.
pub(crate) fn claude_mcp_file(chat_id: &str) -> Option<std::path::PathBuf> {
    let url = crate::hub::mcp_url()?;
    let config = serde_json::json!({
        "mcpServers": {
            "luna": {
                "type": "http",
                "url": url,
                "headers": { "Authorization": format!("Bearer ${{{CLAUDE_MCP_TOKEN_ENV}}}") },
                // Claude Code drops an HTTP tool call that has sent nothing
                // for 5 min, and the server's `timeout` is what raises that
                // (as well as the hard limit per call). A `wait` answers
                // with one plain JSON body at the end, nothing before it.
                "timeout": MCP_TOOL_TIMEOUT_MS,
                // Not behind tool search: a model that has spawned a helper
                // found the tools to talk to it deferred, and had to look each
                // one up first. The rare ones defer all the same: a tool's own
                // `_meta` `anthropic/alwaysLoad: false` outranks this
                // (mcp/tools.rs, seen on 2.1.330).
                "alwaysLoad": true
            }
        }
    });
    let dir = crate::paths::data_dir().join("mcp");
    crate::secret::private_dir(&dir).ok()?;
    let path = dir.join(format!("{}.json", crate::paths::id_file_name(chat_id)));
    crate::secret::write_private(&path, serde_json::to_string_pretty(&config).ok()?.as_bytes()).ok()?;
    Some(path)
}

/// How long a Luna tool call may take, in ms, as both CLIs are told: `wait`
/// holds its request for up to 30 min, `spawn` for up to two.
pub(crate) const MCP_TOOL_TIMEOUT_MS: u64 = 1_900_000;

/// A name for Luna's profile of `sandbox`, new on every start, that no config
/// can have rules ready for (codex/launch.rs `profile_args`); None where
/// Codex's own mode serves.
fn codex_profile(sandbox: &str) -> Option<String> {
    codex_launch::takes_profile(sandbox, cfg!(windows)).then(|| format!("luna-{}", &crate::agents::mint()[..16]))
}

/// Luna's `core/`, for a session in `sandbox` to be kept from reading where
/// Codex can do that (codex/launch.rs `denies_core`).
fn codex_core_deny(sandbox: &str) -> Option<String> {
    codex_launch::denies_core(sandbox, cfg!(windows)).then(|| crate::ipc::core_dir().to_string_lossy().into_owned())
}

/// A chat's approval and sandbox as it is started, from the words the window
/// saved: as Luna runs them (codex/defaults.rs `launch_mode`: what it does not
/// know closed, `on-failure` as `on-request`, Codex's own `OWN` with no flag),
/// `auto-review` in its own sandbox, and what the sandbox takes from this
/// machine, worked out ahead of the registration: the profile's new name and
/// what it denies, PATH and git's entries (each nothing in a sandbox that
/// takes none).
pub(crate) struct CodexModes {
    pub(crate) approval: String,
    pub(crate) sandbox: String,
    pub(crate) profile: Option<String>,
    pub(crate) deny: Option<String>,
    pub(crate) sandbox_path: Option<String>,
    pub(crate) git_envs: Vec<(&'static str, String)>,
}

pub(crate) fn codex_modes(approval: &str, sandbox: &str) -> CodexModes {
    let (approval, sandbox) = crate::codex::defaults::launch_mode(approval, sandbox);
    let sandbox = if approval == AUTO_REVIEW { AUTO_REVIEW_SANDBOX.to_string() } else { sandbox };
    CodexModes {
        profile: codex_profile(&sandbox),
        deny: codex_core_deny(&sandbox),
        sandbox_path: codex_sandbox_path(&sandbox),
        git_envs: codex_git_envs(&sandbox),
        approval,
        sandbox,
    }
}

/// A sandbox as `-c` overrides, for `codex exec`, which asks nobody; one of
/// Luna's profiles named afresh.
pub(crate) fn codex_sandbox_args(sandbox: &str) -> Vec<String> {
    let profile = codex_profile(sandbox);
    codex_launch::sandbox_args(sandbox, profile.as_deref(), codex_core_deny(sandbox).as_deref())
}

/// codex/launch.rs `git_ssl_backend_envs` for a session in that sandbox.
/// Worked out once: the environment they build on is this process's, which
/// stays as it started, and the names are made only the once for their
/// `'static` lifetime.
pub(crate) fn codex_git_envs(sandbox: &str) -> Vec<(&'static str, String)> {
    static ENVS: std::sync::OnceLock<Vec<(&'static str, String)>> = std::sync::OnceLock::new();
    if !cfg!(windows) || sandbox != READ_ONLY_NET {
        return Vec::new();
    }
    ENVS.get_or_init(|| {
        let count = std::env::var("GIT_CONFIG_COUNT").ok();
        codex_launch::git_ssl_backend_envs(count.as_deref(), |i| std::env::var(format!("GIT_CONFIG_KEY_{i}")).ok())
            .into_iter()
            .map(|(k, v)| {
                let k: &'static str = Box::leak(k.into_boxed_str());
                (k, v)
            })
            .collect()
    })
    .clone()
}

/// The curl a Codex chat's status hooks post with (codex/hooks.rs `command`),
/// by an absolute path found now, so no shell looks for one in the project's
/// folder: on Windows its own, `%SystemRoot%\System32\curl.exe`; elsewhere the
/// first `curl` on this process's PATH. None, and the chat gets no hooks,
/// where there is none or its path would not stand bare in a shell
/// (`usable_curl`).
pub(crate) fn codex_hook_curl() -> Option<String> {
    let found = if cfg!(windows) {
        let root = std::env::var_os("SystemRoot")?;
        Some(std::path::Path::new(&root).join("System32").join("curl.exe")).filter(|p| p.is_file())
    } else {
        std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join("curl")).find(|p| p.is_file())
    };
    let path = found?.to_str()?.to_string();
    crate::codex::hooks::usable_curl(&path, cfg!(windows)).then_some(path)
}

/// PATH for a Codex session in that sandbox, where it has a Windows one: this
/// process's, without the Store's app aliases (codex/launch.rs
/// `without_windows_apps`).
pub(crate) fn codex_sandbox_path(sandbox: &str) -> Option<String> {
    codex_launch::codex_windows_sandbox(sandbox)?;
    let path = std::env::var("PATH").ok()?;
    Some(codex_launch::without_windows_apps(&path))
}

/// What tells a session which one it is, for what it runs: every session its
/// own id, a helper the session that started it too.
pub(crate) fn session_envs(id: &str, parent: Option<&str>) -> Vec<(&'static str, String)> {
    let mut envs = vec![("LUNA_SESSION_ID", id.to_string())];
    if let Some(parent) = parent {
        envs.push(("LUNA_PARENT_ID", parent.to_string()));
    }
    envs
}

/// A Codex session's role layers (library/role.rs) as its `-c
/// developer_instructions`, after the value its config stack sets
/// (codex/config.rs `developer_instructions_in`), which the `-c` replaces;
/// `trust_key` follows the trust passed on the command line.
/// Nothing where it has no layers, as for a chat with nothing set: Codex's
/// own value stands. Layers too long for a command line are written whole to a
/// file of Luna's, which the value points at (library/role.rs `Spill`). Err
/// where not even that can be given: the config's own value leaves no room for
/// the pointer, or the file will not write.
pub(crate) fn codex_role_args(
    session: &str,
    account_path: &str,
    folder: &str,
    trust_key: Option<&str>,
    texts: &crate::library::RoleTexts,
    skills: &crate::library::SessionSkills,
    constraints: &[String],
) -> Result<(Vec<String>, Vec<crate::applied::Applied>), String> {
    let Some(layers) = crate::library::role_layers(texts, skills, constraints) else { return Ok((Vec::new(), Vec::new())) };
    let stack = crate::codex::config::developer_instructions_in(account_path, folder, trust_key);
    let given = crate::library::codex_developer_instructions(stack.value.as_deref(), &layers, constraints, &crate::paths::data_dir())?;
    let mut how = match stack.value {
        Some(_) => "-c developer_instructions, after the config's own".to_string(),
        None => "-c developer_instructions".to_string(),
    };
    if let Some(spill) = &given.spill {
        crate::library::write_spill(spill, session)?;
        crate::log::warn(
            "library",
            &format!(
                "role layers of {} characters are too long for a Codex command line (over {}): kept whole in {}, with a pointer in developer_instructions",
                spill.text.chars().count(),
                crate::library::CODEX_ROLE_MAX,
                spill.path.display()
            ),
        );
        how = format!("{how}, pointing at {}, which holds them whole: too long for a command line", spill.path.display());
    }
    let mut applied = crate::library::layers_applied(texts, skills, constraints, &how);
    if !stack.unsure.is_empty() {
        applied.push(crate::applied::Item::RoleLayersUnsure.with(stack.unsure.join("; ")));
    }
    Ok((crate::library::codex_developer_args(&given.value).to_vec(), applied))
}

/// A task from another agent, with the note after it (library/role.rs
/// `RoleTexts::note`, one for a lead and a helper): every turn of a Codex
/// lead or helper, and the first of a Claude Code one. Its role layers (`claude_instructions_dir`,
/// Codex's `developer_instructions`) say what it is at memory level; the note
/// stands beside the task, where an instruction to the person's chat would
/// otherwise outweigh it: told to speak the user's language, a helper went on
/// in it to the agent that asked in English, progress notes and all. After the
/// prompt, not before: the opening words are what names the chat, and a slash
/// command has to come first. One that does goes without the note, which
/// would land in its arguments; its role layers still say it.
pub(crate) fn helper_prompt(note: &str, prompt: &str) -> String {
    if prompt.trim_start().starts_with('/') {
        return prompt.to_string();
    }
    format!("{prompt}{NOTE_TAG}{note}]")
}

/// Where the note starts: what comes before it is the prompt as it was asked.
/// Mechanism, not the person's to change: the readers know a note by it.
const NOTE_TAG: &str = "\n\n[A note, not part of the task: ";

/// The tag earlier Lunas opened the note on, still in conversations they began.
const NOTE_TAG_BEFORE: &str = "\n\n[A note from Luna, not part of the task: ";

/// A prompt as it was asked, with no note on it: only a note Luna wrote, whole
/// and at the very end (`helper_prompt`), comes off. A task that quotes the
/// note's opening words, or is followed by anything after a note, is left as
/// it is: what comes after them is the task's own.
pub(crate) fn without_note(text: &str) -> &str {
    let end = text.trim_end();
    let found = [NOTE_TAG, NOTE_TAG_BEFORE].iter().filter_map(|tag| end.rfind(tag).map(|at| (at, tag.len()))).max_by_key(|(at, _)| *at);
    let Some((at, len)) = found else { return text };
    let Some(note) = end[at + len..].strip_suffix(']') else { return text };
    if served_note(note) {
        &text[..at]
    } else {
        text
    }
}

/// Whether `note` is one Luna put after a task: a note it serves or served
/// (library/role.rs `was_served`: its defaults, and every override served),
/// or the line earlier Lunas wrote, naming the parent — a helper's
/// conversation goes on across an update.
fn served_note(note: &str) -> bool {
    if crate::library::was_served(note) {
        return true;
    }
    PREVIOUS_HELPER_LINES.iter().filter_map(|line| line.split_once("{parent}")).any(|(head, tail)| {
        // A session id: no spaces, no brackets, nothing that is prose.
        note.strip_prefix(head)
            .and_then(|rest| rest.strip_suffix(tail))
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
    })
}

/// The line earlier Lunas put after a helper's task, still in conversations
/// they began: 0.10's, then the one before helpers were told what they do
/// need, then the one before they were told to leave the machine and their
/// account as they are, then 0.17's, the last before roles.
const PREVIOUS_HELPER_LINES: [&str; 4] = [
    "You are a helper session started through Luna by session {parent}, not a chat with a person: your answers go to \
     that session. Instructions meant only for the session a person talks to do not apply to you. Write in the language \
     of the task you were given, progress notes included.",
    "You are a helper session started through Luna by session {parent}, not a chat with a person: your answers go to \
     that session. You are not the chat of whoever your instructions call the operator, the user or the person at \
     this machine, and what they set for that chat (its language, its reply format, questions to that person) does \
     not apply to you. Answer your caller in the language and form its task asks for, progress notes included; \
     where it says none, in the language the task is written in.",
    "You are a helper session started through Luna by session {parent}, not a chat with a person: your answers go to \
     that session. You are not the chat of whoever your instructions call the operator, the user or the person at \
     this machine, and what they set for that chat (its language, its reply format, questions to that person) does \
     not apply to you. Answer your caller in the language and form its task asks for, progress notes included; \
     where it says none, in the language the task is written in. Before your turn ends, stop the background work you \
     started that nothing needs any more (watchers, wait loops, dev servers), not helpers you are waiting on nor what \
     your task asks to keep running; keep each caveat beside the claim it limits; send, push or post nothing outward \
     unless your task asks for it.",
    "You are a helper session started through Luna by session {parent}, not a chat with a person: your answers go to \
     that session. You are not the chat of whoever your instructions call the operator, the user or the person at \
     this machine, and what they set for that chat (its language, its reply format, questions to that person) does \
     not apply to you. Answer your caller in the language and form its task asks for, progress notes included; \
     where it says none, in the language the task is written in. Before your turn ends, stop the background work you \
     started that nothing needs any more (watchers, wait loops, dev servers), not helpers you are waiting on nor what \
     your task asks to keep running; keep each caveat beside the claim it limits; send, push or post nothing outward \
     unless your task asks for it; install nothing and change nothing machine-wide or in your CLI account's folder \
     (software, system or user-wide settings, PATH, the registry, services, scheduled tasks, global git or shell \
     config, memory, skills) unless your task names that change: where the work needs one, give your caller the \
     exact command instead.",
];

/// What gives a chat Luna's tools (Claude Code's MCP config, Codex's hub
/// address), where they were asked for and it could be had: the registry
/// records the tools as given only then, so an orphan adopted later
/// (`orphan_sessions`) does not say it had tools its CLI never got, nor a
/// restart give them.
fn delivered<T>(asked: bool, give: impl FnOnce() -> Option<T>) -> Option<T> {
    asked.then(give).flatten()
}

/// A helper's id runs in a pane only once the helper is the person's
/// (background/rows.rs `background_promoted`): before that its conversation is
/// the helper's, held or going on, and a pane resuming it would run beside it
/// and take it from its parent unannounced (`register` with no parent).
fn not_a_helper(id: &str) -> Result<(), String> {
    if crate::background::has(id) {
        return Err(format!("{id} is still a helper being opened as your chat; it opens here once that is through"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn ensure_claude_session(
    id: String,
    folder: String,
    account_path: String,
    model: String,
    effort: String,
    permission_mode: String,
    worktree: bool,
    // With `worktree`: the name to ask Claude Code for (`--worktree=<name>`).
    worktree_name: Option<String>,
    resume: Option<String>,
    // Luna's MCP tools (agents/): off unless the chat asked.
    tools: Option<bool>,
    // Without the edit tools (`CLAUDE_EDIT_TOOLS`).
    read_only: Option<bool>,
    // The agent it runs as: `--agent`.
    agent: Option<String>,
    // The account folder's identity it must still have (a new account's
    // first sign-in, `accounts::CreatedAccount`).
    account_id: Option<String>,
    // An adopted orphan's first start: an attach to that start alone
    // (`spawn`).
    attach: Option<super::Start>,
    mode_from: Option<luna_window::dto::ModeSource>,
) -> Result<Attached, String> {
    not_a_helper(&id)?;
    let on = account_path.clone();
    // Built only if no process is running (see `spawn`).
    let bound = account_id.clone();
    spawn(id.clone(), &on, bound.as_deref(), attach, move || {
        if let Some(account_id) = &account_id {
            crate::accounts::still_the_account(&account_path, account_id)?;
        }
        // A login session runs in the account folder itself (the providers'
        // `loginSession`).
        let login = same_dir(&folder, &account_path);
        // A folder its CLI could not start in, refused before anything of the
        // start is made (folder.rs): `--worktree` needs a git repository.
        if let Some(why) = (!login).then(|| folder_refusal(&folder, worktree)).flatten() {
            return Err(why);
        }
        // `--resume` of a session with no transcript fails with "No
        // conversation found" and leaves the pane at a dead CLI: a chat that
        // was opened and never typed into starts afresh instead.
        let transcript = resume.as_deref().and_then(|sid| crate::claude::session::find_transcript(&account_path, sid));
        let resume = resume.filter(|_| transcript.is_some());
        // The skills the library gives sessions in this folder and keeps from
        // them, at every start (library/folders.rs `session_args`); not to a login
        // session.
        // The account counted as in use from before the pass until the process
        // is (`lay_out_for`): no rename moves it from under the start.
        let (mut skills, reserved) = if login {
            (crate::library::SessionSkills::default(), None)
        } else {
            let reserved = crate::library::lay_out_for("before a session starts", &account_path)?;
            (crate::library::session_args(Provider::Claude, &account_path, &folder, crate::library::Role::Chat), Some(reserved))
        };
        // A login session is no chat: no tools, so no notes either.
        let tools = tools.unwrap_or(false) && !login;
        let notes = tools.then(crate::agents::notes_dir).filter(|notes| std::fs::create_dir_all(notes).is_ok());
        // Its role layers (library/role.rs): the library's instructions for
        // its folder or for chats, nothing of Luna's. No constraints: a chat
        // is the person's.
        let role = (!login).then(|| crate::library::role_texts(crate::library::Role::Chat));
        let layers = role.as_ref().and_then(|t| crate::library::role_layers(t, &skills, &[]));
        // A chat is the person's: one whose layers will not write starts
        // without them, and says so (`role.layers-off`), as does a Codex one
        // whose config leaves no room on the command line.
        let (instructions, layers_off) = match login {
            true => (None, None),
            false => match claude_instructions_dir(&id, layers.as_deref()) {
                Ok(dir) => (dir, None),
                Err(why) => (None, Some(why)),
            },
        };
        let instructions_file = instructions.map(|dir| dir.join("CLAUDE.md"));
        // The role layers go in the system prompt, not as memory of an added
        // folder, so no added folder's memory is read and none is left out.
        // The account's instructions left out only where that takes nothing
        // of the project's own.
        (skills.excluded, skills.excluded_kept) =
            claude_account_excludes(std::mem::take(&mut skills.excluded), &[], &account_path, &folder, None);
        // Rewritten at every spawn: the listener's port is new with every Luna run.
        // Mandatory: it carries the core-folder deny (threat model F3). If it
        // cannot be written, the session does not start rather than run
        // without it. A login session's goes inline (`claude_login_settings`).
        let (settings_given, settings_applied) = if login {
            claude_login_settings()
        } else {
            let (file, applied) = claude_settings_file(&id, true, &skills, None, notes.as_deref())?;
            (file.to_string_lossy().into_owned(), applied)
        };
        let read_only = read_only.unwrap_or(false);
        let agent = agent.filter(|a| !a.is_empty());
        let default_mode = permission_mode.trim().is_empty().then(|| crate::claude::defaults::chat_mode(&account_path, &folder));
        use luna_window::dto::ModeSource;
        let mode_from =
            default_mode.as_ref().map(|mode| if mode.mode_source() == "cli" { ModeSource::Cli } else { ModeSource::Luna }).or(mode_from);
        // A CLI-owned chat keeps following Claude's settings on every start.
        let own_mode = crate::claude::defaults::OWN_MODE;
        let permission_mode =
            if mode_from == Some(ModeSource::Cli) { own_mode.to_owned() } else { default_mode.map_or(permission_mode, |mode| mode.value) };
        let mut settings = with_role(
            serde_json::json!({
                "model": model, "effort": effort, "permissionMode": permission_mode, "worktree": worktree,
            }),
            read_only,
            agent.as_deref(),
        );
        if let Some(source) = mode_from {
            settings["modeFrom"] = serde_json::json!(source);
        }
        // A login session is not a chat, so not one agents may see or write to.
        // Its tools are recorded as given only once the config that gives
        // them is written (`delivered`): one that would not write starts
        // without them, and nothing says it has them.
        let mcp_file = delivered(tools, || claude_mcp_file(&id));
        let token = if login {
            None
        } else {
            crate::agents::register(&id, Provider::Claude, &folder, &account_path, None, mcp_file.is_some(), settings)
        };
        let mcp = token.zip(mcp_file).map(|(t, file)| (file, t));
        // The folders' skills as a plugin of this chat's own
        // (library/session_plugin.rs), made once nothing that can refuse the
        // start is left, so a refused one leaves none behind. A chat whose
        // plugin will not write starts without them, and says so.
        let skills_off = (!login).then(|| crate::library::give_plugins(&id, &mut skills).err()).flatten();
        // Before 0.20.1 a chat with Luna tools could record a snapshot.
        // Render fresh if its tools may still be recorded in a resumed chat.
        let fresh_prompt = !login
            && (instructions_file.is_some()
                || mcp.is_some()
                || !skills.args.is_empty()
                || agent.is_some()
                || transcript.as_deref().is_some_and(|path| crate::claude::session::snapshot_names_luna(path).unwrap_or(true)));
        // Copy-on-select as Settings has it, in the one place Claude Code
        // reads it (claude/config.rs); a write that fails holds nothing up.
        // A sign-in is left alone: it is no chat, and as a rule the
        // account's first start, before the CLI has made its `.claude.json`;
        // the chats after it get the setting.
        let copy_on_select = (!login).then(|| crate::claude::config::copy_on_select_before_chat(&account_path)).flatten();
        let (args, envs, mut applied) = claude_launch::chat_args(&claude_launch::ChatStart {
            id: &id,
            account_path: &account_path,
            model: &model,
            effort: &effort,
            permission_mode: (permission_mode != own_mode).then_some(&permission_mode),
            resume: resume.as_deref(),
            worktree: worktree.then_some(worktree_name.as_deref()),
            settings: Some((&settings_given, &settings_applied)),
            agent: agent.as_deref(),
            read_only,
            mcp: mcp.as_ref().map(|(file, token)| (file.as_path(), token.as_str())),
            instructions_file: instructions_file.as_deref(),
            fresh_prompt,
            skill_args: &skills.args,
        });
        if let (Some(t), Some(file)) = (&role, &instructions_file) {
            applied.extend(crate::library::layers_applied(t, &skills, &[], &file.display().to_string()));
        }
        if let Some(why) = layers_off {
            applied.push(crate::applied::Item::RoleLayersOff.with(why));
        }
        if let Some(why) = skills_off {
            applied.push(crate::applied::Item::LibrarySkillsOff.with(why));
        }
        applied.extend(copy_on_select);
        Ok(Launch {
            provider: Provider::Claude,
            cwd: folder,
            account_path,
            args,
            envs,
            resume,
            describe: format!("model {model} perm {permission_mode} worktree {worktree}"),
            applied,
            skills_unhidden: 0,
            reserved,
        })
    })
    .await
}

/// A Codex session. `folder` is where it runs: the project, which `worktree`
/// has Codex leave for a checkout of its own (`--worktree`), or a worktree it
/// made earlier, to resume in. `login` runs `codex login` instead of a chat.
#[allow(clippy::too_many_arguments)]
pub async fn ensure_codex_session(
    id: String,
    folder: String,
    account_path: String,
    model: Option<String>,
    effort: String,
    approval: String,
    sandbox: String,
    worktree: Option<bool>,
    resume: Option<String>,
    login: bool,
    tools: Option<bool>,
    // As for Claude Code: the account folder's identity it must still have.
    account_id: Option<String>,
    // An adopted orphan's first start (`spawn`).
    attach: Option<super::Start>,
    mode_from: Option<luna_window::dto::ModeSource>,
    trust_folder: Option<luna_window::dto::ProjectTrust>,
) -> Result<Attached, String> {
    not_a_helper(&id)?;
    let on = account_path.clone();
    // Built only if no process is running (see `spawn`).
    let bound = account_id.clone();
    spawn(id.clone(), &on, bound.as_deref(), attach, move || {
        if let Some(account_id) = &account_id {
            crate::accounts::still_the_account(&account_path, account_id)?;
        }
        if login {
            let envs = vec![("CODEX_HOME", account_path.clone())];
            let args = vec!["login".to_string()];
            let applied = vec![crate::applied::Item::Account.with(format!("CODEX_HOME={account_path}"))];
            let describe = "login".into();
            return Ok(Launch {
                provider: Provider::Codex,
                cwd: folder,
                account_path,
                args,
                envs,
                resume: None,
                describe,
                applied,
                skills_unhidden: 0,
                reserved: None,
            });
        }
        // A folder Codex could not start in, refused before anything of the
        // start is made (folder.rs): `--worktree` goes only to a new session.
        let resume = resume.as_deref().map(crate::codex::launch::thread_to_resume).transpose()?;
        let new_worktree = worktree.unwrap_or(false) && resume.is_none();
        if let Some(why) = folder_refusal(&folder, new_worktree) {
            return Err(why);
        }
        use crate::codex::trust;
        let approved_key = trust_folder.as_ref().and_then(|grant| trust::granted_key(grant, &folder));
        let dropped_trust = trust_folder.is_some() && approved_key.is_none();
        let run_trust = approved_key
            .map(|key| (key, crate::applied::Item::SessionTrust))
            .or_else(|| trust::trusted_root_key(&folder).map(|key| (key, crate::applied::Item::TrustedRoot)));
        let trust_key = run_trust.as_ref().map(|(key, _)| key.as_str());
        let trust = run_trust.as_ref().map(|(key, item)| (trust::trust_override(key), *item));
        let chosen_model = model.as_deref().map(str::trim).filter(|m| !m.is_empty());
        crate::codex::defaults::refuses_untrusted(&account_path, &folder, trust_key)?;
        let default_mode = (approval.trim().is_empty() && sandbox.trim().is_empty())
            .then(|| crate::codex::defaults::chat_mode(&account_path, &folder, trust_key));
        let (approval, sandbox) = default_mode.map_or((approval.as_str(), sandbox.as_str()), |mode| mode.flags());
        let modes = codex_modes(approval, sandbox);
        let (approval, sandbox) = (modes.approval.as_str(), modes.sandbox.as_str());
        let tools = tools.unwrap_or(false);
        let mut settings = serde_json::json!({
            "model": chosen_model,
            "effort": effort, "approval": approval, "sandbox": sandbox,
        });
        if let (false, Some(grant)) = (dropped_trust, trust_folder) {
            settings[crate::agents::CODEX_TRUST] = serde_json::json!(grant);
        }
        if let Some(mode) = default_mode {
            settings["modeFrom"] = serde_json::json!(mode.source());
        } else if let Some(source) = mode_from {
            settings["modeFrom"] = serde_json::json!(source);
        }
        // Started into a worktree Codex makes in the account's folder: Luna's
        // own word for it, before any rollout says where the chat runs.
        if new_worktree {
            settings["worktree"] = serde_json::json!(true);
        }
        // The person's own chat: no parent, and its account's servers and apps.
        // Its tools are recorded as given only with the hub's address to give
        // them by (`delivered`).
        let url = delivered(tools, crate::hub::mcp_url);
        // Luna's server goes by `luna`, a name its config's layers must leave
        // to it (codex/launch.rs `luna_server_free`): checked before the chat
        // is registered as given its tools.
        if url.is_some() {
            let out = "or start the chat without Luna tools";
            let found = crate::codex::config::servers_named(&account_path, &folder, codex_launch::LUNA_SERVER, trust_key).map_err(|e| {
                format!("the chat is not started: Luna could not read whether it has an MCP server named luna ({e}); {out}")
            })?;
            codex_launch::luna_server_free(&found, false, "the chat").map_err(|e| format!("{e}, {out}"))?;
        }
        let token = crate::agents::register(&id, Provider::Codex, &folder, &account_path, None, url.is_some(), settings);
        let mcp = token.zip(url).map(|(t, url)| (url, t));
        // The skills the library gives sessions in this folder and keeps
        // from them, at every start (library/folders.rs `session_args`).
        let reserved = crate::library::lay_out_for("before a session starts", &account_path)?;
        let skills = crate::library::session_args(Provider::Codex, &account_path, &folder, crate::library::Role::Chat);
        let (hook_url, hook_curl) = (crate::hub::hook_url(&id), codex_hook_curl());
        // Copy on select as Settings has it; a settings.json that does not
        // read gives this start the `never` it had before the setting.
        let read = crate::settings::get_checked().map(|s| s.copy_on_select);
        if let Err(e) = &read {
            crate::log::warn("codex-launch", &format!("copy on select as before, never: Luna's settings do not read: {e}"));
        }
        let copy_on_select = codex_launch::copy_on_select_as_read(read);
        let (args, envs, mut applied) = codex_launch::chat_args(&codex_launch::ChatStart {
            id: &id,
            folder: &folder,
            account_path: &account_path,
            resume: resume.as_deref(),
            trust,
            own_windows_sandbox: crate::codex::config::windows_sandbox_set(&account_path),
            worktree: worktree.unwrap_or(false),
            model: chosen_model,
            effort: &effort,
            approval,
            sandbox,
            profile: modes.profile.as_deref(),
            deny: modes.deny.as_deref(),
            sandbox_path: modes.sandbox_path.as_deref(),
            git_envs: &modes.git_envs,
            mcp: mcp.as_ref().map(|(url, token)| (url.as_str(), token.as_str())),
            prompt_for_spawn: mcp.is_some() && crate::codex::defaults::prompt_for_spawn(Some(approval), &account_path, &folder),
            // Its status hooks post where a Claude Code chat's do.
            hooks: hook_url.as_deref().zip(hook_curl.as_deref()),
            skill_args: &skills.args,
            copy_on_select,
        });
        applied.extend(crate::library::audience_applied(&skills));
        if dropped_trust {
            applied
                .push(crate::applied::Item::SessionTrustDropped.with("The project directory differs from the one approved for this chat."));
        }
        // Its role layers in developer_instructions, or in a file of Luna's
        // that it points at where they are too long for the command line; a
        // chat that can have neither starts without them, and says so.
        let role = crate::library::role_texts(crate::library::Role::Chat);
        let mut args = args;
        match codex_role_args(&id, &account_path, &folder, trust_key, &role, &skills, &[]) {
            Ok((role_args, role_applied)) => {
                args.extend(role_args);
                applied.extend(role_applied);
            }
            Err(why) => applied.push(crate::applied::Item::RoleLayersOff.with(why)),
        }
        // A helper the person took over: the thread it resumes keeps the
        // developer message it was started with (agents/handover.rs).
        if let Some(carried) = crate::agents::carried_into(&id, resume.as_deref()) {
            applied.push(crate::applied::Item::RoleLayersKept.with(format!("this conversation still carries {carried}")));
        }
        let describe = format!("model {} effort {effort} approval {approval} sandbox {sandbox}", model.as_deref().unwrap_or("default"));
        Ok(Launch {
            provider: Provider::Codex,
            cwd: folder,
            account_path,
            args,
            envs,
            resume,
            describe,
            applied,
            skills_unhidden: skills.unhidden.len(),
            reserved: Some(reserved),
        })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::super::screen::title_from_prompt;
    use super::super::spawn::{capture, INHERITED_SESSION_VARS};
    use super::*;

    #[test]
    fn a_session_is_told_its_id_and_a_helper_its_parent() {
        assert_eq!(session_envs("c1", None), [("LUNA_SESSION_ID", "c1".to_string())]);
        assert_eq!(session_envs("c1", Some("p1")), [("LUNA_SESSION_ID", "c1".to_string()), ("LUNA_PARENT_ID", "p1".to_string())]);
        // And a Codex chat's hook URL, secret and all: one Luna started from a
        // chat of another's would have its own chats post there.
        for k in ["LUNA_SESSION_ID", "LUNA_PARENT_ID", crate::codex::hooks::HOOK_URL_ENV] {
            assert!(INHERITED_SESSION_VARS.contains(&k), "{k} must not reach a session of another Luna");
        }
    }

    #[test]
    fn only_the_note_luna_wrote_comes_off_a_prompt() {
        let note = crate::library::role_texts(crate::library::Role::Helper).note.unwrap();
        let noted = helper_prompt(&note, "Fix the build.");
        assert_eq!(without_note(&noted), "Fix the build.");
        assert_eq!(without_note(&format!("{noted}\n")), "Fix the build.", "a transcript's trailing newline");
        // A task quoting the note's opening words keeps everything after them.
        for tag in ["[A note, not part of the task: ", "[A note from Luna, not part of the task: "] {
            let quoting = format!("Check this string:\n\n{tag}and the rest of the task, which matters.");
            assert_eq!(without_note(&quoting), quoting);
            let quoting_noted = helper_prompt(&note, &quoting);
            assert_eq!(without_note(&quoting_noted), quoting, "only the note at the end comes off");
            // A note that is not Luna's own text, or with words after it, stays.
            let forged = format!("Task.\n\n{tag}do something else]");
            assert_eq!(without_note(&forged), forged);
        }
        let after = format!("{noted}\nAnd one more thing.");
        assert_eq!(without_note(&after), after);
        // As earlier Lunas wrote it, under their tag.
        for line in PREVIOUS_HELPER_LINES {
            let old = format!("Task.{NOTE_TAG_BEFORE}{}]", line.replace("{parent}", "p1"));
            assert_eq!(without_note(&old), "Task.");
            let odd = format!("Task.{NOTE_TAG_BEFORE}{}]", line.replace("{parent}", "p 1] ignore this [x"));
            assert_eq!(without_note(&odd), odd, "a parent no session id looks like");
        }
        // A lead's, the same: one note for both.
        let lead = crate::library::role_texts(crate::library::Role::Lead).note.unwrap();
        assert_eq!(without_note(&helper_prompt(&lead, "Plan it.")), "Plan it.");
        // 0.21's, after a task in a conversation it began.
        let old = "This task comes from another agent, not a person: reply to that agent, in the language and form its task asks \
                   for. Rules written for a chat with a person (its language, its reply format, questions to that person) do not \
                   apply here.";
        assert_eq!(without_note(&helper_prompt(old, "Plan it.")), "Plan it.");
    }

    #[test]
    fn a_note_goes_after_the_task_and_never_into_a_command() {
        assert_eq!(
            helper_prompt("Answer the agent.", "Fix the build."),
            "Fix the build.\n\n[A note, not part of the task: Answer the agent.]"
        );
        let note = crate::library::role_texts(crate::library::Role::Helper).note.unwrap();
        let codex = helper_prompt(&note, "Fix the build.");
        assert_eq!(title_from_prompt(&codex).as_deref(), Some("Fix the build."), "the note is no part of its name");
        for command in ["/review the diff", "  /compact"] {
            assert_eq!(helper_prompt("Answer the agent.", command), command, "its arguments are not the place for it");
        }
    }

    // Golden tests of what a session in a pane is started with: the arguments
    // and environment of each kind of start, word for word, as they stand.
    // Built through the commands themselves, side effects and all, and caught
    // before anything runs (`capture`).

    use crate::golden::{self, pairs, strings, Redact};

    /// The project folder, as what is compared has it (`golden::project`).
    const FOLDER: &str = golden::PROJECT;

    /// A start as `spawn` would have run it, placeholders in, with the
    /// settings, MCP and instructions files it was given read back (and
    /// removed).
    struct Started {
        args: Vec<String>,
        envs: Vec<(String, String)>,
        cwd: String,
        resume: Option<String>,
        describe: String,
        settings: Option<serde_json::Value>,
        registered: serde_json::Value,
        permission: Option<luna_window::dto::ResolvedPermission>,
        skills_unhidden: usize,
        mcp: Option<serde_json::Value>,
        instructions: Option<String>,
        /// A Codex start's `developer_instructions`, placeholders in: `args`
        /// has `<role-layers>` in its place.
        role: Option<String>,
        /// What Luna applied, as (key, class, value), placeholders in.
        applied: Vec<(String, crate::applied::Class, String)>,
    }

    impl Started {
        fn keys(&self) -> Vec<&str> {
            self.applied.iter().map(|(k, _, _)| k.as_str()).collect()
        }
        fn value(&self, key: &str) -> Option<&str> {
            self.applied.iter().find(|(k, _, _)| k == key).map(|(_, _, v)| v.as_str())
        }
    }

    fn started(id: &str, r: &Redact, start: impl std::future::Future<Output = Result<Attached, String>>) -> Started {
        capture::expect(id);
        let built = crate::runtime::block_on(start);
        let launch = capture::take(id);
        // The environment read while the session is still registered: its
        // token has to be the one Luna knows it by.
        let envs = launch.as_ref().map(|l| r.envs(&l.envs));
        let registered = crate::agents::settings_of(id);
        crate::agents::forget(id);
        let attached = built.expect("the captured start succeeded");
        assert!(attached.backlog.is_empty());
        let (l, mut envs) = (launch.expect("the start was caught"), envs.unwrap());
        no_way_into_the_core(&l.args, &l.envs);
        assert!(!l.envs.iter().any(|(name, _)| *name == crate::claude::compact::AUTO_COMPACT_WINDOW_ENV));
        assert!(!l.args.iter().any(|arg| arg.starts_with(crate::codex::config::AUTO_COMPACT_LIMIT_KEY)));
        Redact::assert_shown(&l.args, &l.envs, &l.cwd, &l.applied);
        Redact::assert_probed(l.provider, &l.args);
        // Every pane's CLI ends its environment on the terminal it draws into
        // (`Launch::finished`); the cases below leave it out.
        assert_eq!(envs.pop(), Some(("TERM".to_string(), "xterm-256color".to_string())));
        let applied = r.applied(&l.applied, &l.envs);
        let role = golden::developer_instructions(&l.args).map(|v| r.text(&v));
        let args = r.args(&l.args);
        let data = crate::paths::data_dir();
        let read = |sub: &str, placeholder: &str| {
            let path = data.join(sub).join(format!("{}.json", crate::paths::id_file_name(id)));
            let v = args.iter().any(|a| a == placeholder).then(|| r.file(&path.to_string_lossy()));
            let _ = std::fs::remove_file(&path);
            v
        };
        let (settings, mcp) = (read("hooks", "<settings-file>"), read("mcp", "<mcp-file>"));
        let instructions = golden::instructions(id, &args);
        Started {
            envs,
            cwd: r.text(&l.cwd),
            resume: l.resume,
            describe: l.describe,
            settings,
            registered,
            permission: attached.permission,
            skills_unhidden: attached.skills_unhidden,
            mcp,
            instructions,
            role,
            args,
            applied,
        }
    }

    /// A CLI is given nothing of the core's: not its endpoint, nothing of its
    /// `core.json`, no variable of the core's own. There is no token to give
    /// (ADR-0021); this keeps it so for every kind of start, and the core's
    /// endpoint handles are never inherited (endpoint/ `no_child_inherits_either_end`).
    fn no_way_into_the_core(args: &[String], envs: &[(&str, String)]) {
        let all = args.iter().map(String::as_str).chain(envs.iter().flat_map(|(k, v)| [*k, v.as_str()]));
        for s in all {
            let endpoint = s.contains(r"\pipe\rm-luna-core-") || s.contains("/core/core-") || s.contains(r"\core\core-");
            assert!(!endpoint && !s.contains("core.json"), "a start carries something of the core's: {s}");
        }
        assert!(envs.iter().all(|(k, _)| !k.starts_with("LUNA_CORE")), "{envs:?}");
    }

    /// The window's ask for a Claude Code chat; each case changes what it is about.
    struct ClaudeAsk<'a> {
        folder: String,
        account: String,
        model: &'static str,
        effort: &'static str,
        mode: &'a str,
        mode_from: Option<luna_window::dto::ModeSource>,
        worktree: bool,
        worktree_name: Option<&'static str>,
        resume: Option<String>,
        tools: Option<bool>,
        read_only: Option<bool>,
        agent: Option<&'static str>,
    }

    impl ClaudeAsk<'_> {
        fn chat() -> Self {
            ClaudeAsk {
                folder: golden::project(),
                account: "/golden/acc-claude".into(),
                model: "opus",
                effort: "high",
                mode: "default",
                mode_from: None,
                worktree: false,
                worktree_name: None,
                resume: None,
                tools: None,
                read_only: None,
                agent: None,
            }
        }

        fn start(self, id: &str, r: &Redact) -> Started {
            started(id, r, self.ensure(id))
        }

        fn ensure(self, id: &str) -> impl std::future::Future<Output = Result<Attached, String>> {
            let s = |v: Option<&str>| v.map(str::to_owned);
            ensure_claude_session(
                id.into(),
                self.folder,
                self.account,
                self.model.into(),
                self.effort.into(),
                self.mode.into(),
                self.worktree,
                s(self.worktree_name),
                self.resume,
                self.tools,
                self.read_only,
                s(self.agent),
                None,
                None,
                self.mode_from,
            )
        }
    }

    /// A start refused before anything of it was built: no launch caught,
    /// and so no CLI started, nothing for the canary to watch. Its reason back.
    fn refused(id: &str, start: impl std::future::Future<Output = Result<Attached, String>>) -> String {
        capture::expect(id);
        let got = crate::runtime::block_on(start);
        let launch = capture::take(id);
        crate::agents::forget(id);
        assert!(launch.is_none(), "{id}: a launch was built");
        got.expect_err("refused")
    }

    /// A folder the CLI could not start in is refused by either CLI's start
    /// before its files are written, a token minted or a CLI run: one that is
    /// not there, and a worktree outside a git repository, which Claude Code
    /// 2.1.319 and Codex 0.167.0 each exit on at once. So the CLI canary,
    /// which watches only what runs, never counts it.
    #[test]
    fn a_start_in_a_folder_its_cli_cannot_run_in_is_refused_before_anything_is_made() {
        let root = std::env::temp_dir().join(format!("luna-golden-folders-{}", std::process::id()));
        let plain = root.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let (gone, plain) = (root.join("luna;C").to_string_lossy().into_owned(), plain.to_string_lossy().into_owned());
        let id = "golden-refused";
        let why = refused(id, ClaudeAsk { folder: gone.clone(), ..ClaudeAsk::chat() }.ensure(id));
        assert!(why.contains("does not exist"), "{why}");
        let why = refused(id, CodexAsk { folder: gone, ..CodexAsk::chat() }.ensure(id));
        assert!(why.contains("does not exist"), "{why}");
        use super::super::folder::in_git_repository;
        if !in_git_repository(std::path::Path::new(&plain)) {
            let why = refused(id, ClaudeAsk { folder: plain.clone(), worktree: true, ..ClaudeAsk::chat() }.ensure(id));
            assert!(why.contains("not in a git repository"), "{why}");
            let named = ClaudeAsk { folder: plain.clone(), worktree: true, worktree_name: Some("feature-x"), ..ClaudeAsk::chat() };
            assert!(refused(id, named.ensure(id)).contains("not in a git repository"), "a named worktree too");
            let why = refused(id, CodexAsk { folder: plain.clone(), worktree: Some(true), ..CodexAsk::chat() }.ensure(id));
            assert!(why.contains("not in a git repository"), "{why}");
        }
        let data = crate::paths::data_dir();
        for file in [data.join("hooks").join(format!("{id}.json")), data.join("mcp").join(format!("{id}.json"))] {
            assert!(!file.exists(), "{} was written", file.display());
        }
        // The same folder without a worktree starts as ever.
        let r = Redact::for_chat(id).with(plain.clone(), "<plain>");
        assert_eq!(ClaudeAsk { folder: plain, ..ClaudeAsk::chat() }.start(id, &r).cwd, "<plain>");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A model that takes no effort (Haiku 4.5 by its id) is asked for with
    /// no effort, and starts with no `--effort`.
    #[test]
    fn golden_claude_chat_on_a_model_without_effort_has_no_effort_flag() {
        const ID: &str = "golden-claude-no-effort";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk { model: "claude-haiku-4-5-20251001", effort: "", ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&["--model", "claude-haiku-4-5-20251001", "--permission-mode", "default", "--settings", "<settings-file>",])
        );
    }

    /// The variables every start goes without, as applied.
    fn removed() -> String {
        format!("unset {}", super::super::spawn::removed_env().join(", "))
    }

    fn claude_envs(id: &str, account: &str) -> Vec<(String, String)> {
        pairs(&[("CLAUDE_CONFIG_DIR", account), ("DISABLE_AUTOUPDATER", "1"), ("CLAUDE_CODE_NO_FLICKER", "1"), ("LUNA_SESSION_ID", id)])
    }

    #[test]
    fn golden_claude_chat_starts_with_its_flags_and_hooks() {
        const ID: &str = "golden-claude-chat";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk::chat().start(ID, &r);
        assert_eq!(
            s.args,
            strings(&["--model", "opus", "--effort", "high", "--permission-mode", "default", "--settings", "<settings-file>",])
        );
        assert_eq!(s.envs, claude_envs(ID, "/golden/acc-claude"));
        assert_eq!(
            s.settings,
            Some(golden::claude_chat_settings()),
            "the fullscreen renderer in the file, nothing written into the account"
        );
        assert_eq!(s.mcp, None);
        assert_eq!((s.cwd.as_str(), s.resume), (FOLDER, None));
        assert_eq!(s.describe, "model opus perm default worktree false");
        use crate::applied::Class::*;
        let want = [
            ("account", Runtime, "CLAUDE_CONFIG_DIR=/golden/acc-claude"),
            ("no-self-update", Runtime, "DISABLE_AUTOUPDATER=1"),
            ("session-id", Runtime, "LUNA_SESSION_ID=golden-claude-chat"),
            ("model", UserAction, "--model opus"),
            ("effort", UserAction, "--effort high"),
            ("permission-mode", UserAction, "--permission-mode default"),
            ("activity-hooks", Runtime, "hooks in --settings <settings-file>"),
            ("core-read-deny", Boundary, "permissions.deny Read(//<core-dir>/**) in --settings <settings-file>"),
            ("claude.fullscreen", Runtime, "tui: fullscreen in --settings <settings-file>, CLAUDE_CODE_NO_FLICKER=1"),
            // Settings' default; the golden account has no `.claude.json`,
            // and none is made (claude/config.rs has the writes).
            ("copy-on-select", UserAction, "off: not written, the account has no .claude.json yet"),
            ("terminal", Runtime, "TERM=xterm-256color"),
            ("cwd", Runtime, "cwd /golden/project"),
        ];
        let mut want: Vec<_> = want.into_iter().map(|(k, c, v)| (k.to_string(), c, v.to_string())).collect();
        want.push(("inherited-env-cleared".into(), Runtime, removed()));
        assert_eq!(s.applied, want);
    }

    #[test]
    fn golden_plain_claude_chat_uses_the_cli_snapshot_default() {
        const ID: &str = "golden-claude-plain";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk { tools: Some(false), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&["--model", "opus", "--effort", "high", "--permission-mode", "default", "--settings", "<settings-file>"])
        );
        assert_eq!(s.instructions, None);
        assert_eq!(s.mcp, None);
        assert_eq!(s.value("claude.fresh-prompt"), None);
    }

    #[test]
    fn golden_claude_chat_with_tools_has_no_wait_guard() {
        const ID: &str = "golden-claude-no-guard";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk { tools: Some(true), ..ClaudeAsk::chat() }.start(ID, &r);
        let mut settings = golden::claude_chat_settings();
        settings["permissions"]["allow"] = serde_json::json!(["Read(//<notes-dir>/**)"]);
        assert_eq!(s.settings, Some(settings), "no PreToolUse hook but the question's");
        assert_eq!(s.instructions, None);
        assert_eq!(s.value("claude.fresh-prompt"), Some("--system-prompt-snapshot off"));
        assert!(!s.keys().iter().any(|k| k.ends_with("guard")), "{:?}", s.keys());
        assert_eq!(
            s.value("luna-tools"),
            Some(
                format!(
                    "--mcp-config <mcp-file>, --allowedTools {}; its token in LUNA_MCP_TOKEN, not shown",
                    golden::CLAUDE_ALLOWED_LUNA_TOOLS
                )
                .as_str()
            )
        );
    }

    /// A chat's tools are recorded as given (what an orphan adopted later
    /// keeps, and a restart gives again) only where what gives them could be
    /// had: Claude Code's MCP config written, Codex's hub address known.
    #[test]
    fn tools_are_recorded_only_where_they_were_given() {
        assert_eq!(delivered(true, || Some(1)), Some(1));
        assert_eq!(delivered(true, || None::<u8>), None, "asked, with nothing to give them by");
        assert_eq!(delivered(false, || -> Option<u8> { unreachable!("not asked: nothing written") }), None);
        // Given (the hub is up in these tests), and recorded so, by either
        // CLI's start: the registry read while the session is registered.
        const ID: &str = "golden-tools-recorded";
        let starts: [std::pin::Pin<Box<dyn std::future::Future<Output = Result<Attached, String>>>>; 2] = [
            Box::pin(ClaudeAsk { tools: Some(true), ..ClaudeAsk::chat() }.ensure(ID)),
            Box::pin(CodexAsk { tools: Some(true), ..CodexAsk::chat() }.ensure(ID)),
        ];
        for start in starts {
            capture::expect(ID);
            assert!(crate::runtime::block_on(start).is_ok());
            let launch = capture::take(ID).expect("the start was caught");
            assert!(launch.applied.iter().any(|a| a.key == "luna-tools") && crate::agents::has_tools(ID), "{:?}", launch.provider);
            crate::agents::forget(ID);
        }
    }

    #[test]
    fn golden_claude_permission_modes_pass_through_as_given() {
        for mode in ["acceptEdits", "plan", "bypassPermissions", "auto"] {
            let id = format!("golden-claude-mode-{mode}");
            let r = Redact::for_chat(&id);
            let s = ClaudeAsk { mode, ..ClaudeAsk::chat() }.start(&id, &r);
            assert_eq!(
                s.args,
                strings(&["--model", "opus", "--effort", "high", "--permission-mode", mode, "--settings", "<settings-file>",])
            );
        }
    }

    #[test]
    fn golden_claude_chat_defaults_to_auto_unless_settings_choose() {
        use luna_window::dto::PermissionFlags;

        if crate::claude::defaults::managed_mode_for_test() {
            return;
        }
        let account = std::env::temp_dir().join(format!("luna-golden-claude-mode-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let acc = account.to_string_lossy().into_owned();
        let id = "golden-claude-luna-mode";
        let r = Redact::for_chat(id).with(acc.clone(), "<acc>");
        let s = ClaudeAsk { account: acc.clone(), mode: "", ..ClaudeAsk::chat() }.start(id, &r);
        assert!(s.args.windows(2).any(|a| a == ["--permission-mode", "auto"]));
        assert_eq!(s.value("permission-mode"), Some("--permission-mode auto"));
        assert_eq!(s.registered["modeFrom"], "luna");
        assert_eq!(s.registered["permissionMode"], "auto");
        let first = s.permission.expect("the start returns its resolved mode");
        let PermissionFlags::Claude { permission_mode } = &first.flags else { panic!("expected Claude's flags") };

        std::fs::write(account.join("settings.json"), r#"{ "permissions": { "defaultMode": "plan" } }"#).unwrap();
        let sid = "d447ff66-759a-4b99-b2d2-66a188831ff0";
        std::fs::create_dir_all(account.join("projects").join("test")).unwrap();
        std::fs::write(account.join("projects").join("test").join(format!("{sid}.jsonl")), "{}\n").unwrap();
        let id = "golden-claude-saved-mode";
        let r = Redact::for_chat(id).with(acc.clone(), "<acc>");
        let s = ClaudeAsk {
            account: acc.clone(),
            mode: permission_mode.as_str(),
            mode_from: Some(first.mode_from),
            resume: Some(sid.into()),
            ..ClaudeAsk::chat()
        }
        .start(id, &r);
        assert_eq!(s.value("permission-mode"), Some("--permission-mode auto"));
        assert_eq!(s.resume.as_deref(), Some(sid));
        assert_eq!(s.permission.as_ref(), Some(&first));

        std::fs::write(account.join("settings.json"), r#"{ "permissions": { "defaultMode": "dontAsk" } }"#).unwrap();
        let id = "golden-claude-cli-mode";
        let r = Redact::for_chat(id).with(acc.clone(), "<acc>");
        let s = ClaudeAsk { account: acc.clone(), mode: "", ..ClaudeAsk::chat() }.start(id, &r);
        assert!(!s.args.iter().any(|a| a == "--permission-mode"));
        assert_eq!(s.value("permission-mode"), None);
        assert_eq!(s.registered["modeFrom"], "cli");
        let first = s.permission.expect("the start returns the CLI-owned mode");
        let PermissionFlags::Claude { permission_mode } = &first.flags else { panic!("expected Claude's flags") };
        assert_eq!(permission_mode, crate::claude::defaults::OWN_MODE);

        std::fs::write(account.join("settings.json"), r#"{ "permissions": { "defaultMode": "default" } }"#).unwrap();
        let id = "golden-claude-cli-resume";
        let r = Redact::for_chat(id).with(acc.clone(), "<acc>");
        let resumed = ClaudeAsk {
            account: acc.clone(),
            mode: permission_mode.as_str(),
            mode_from: Some(first.mode_from),
            resume: Some(sid.into()),
            ..ClaudeAsk::chat()
        }
        .start(id, &r);
        assert!(!resumed.args.iter().any(|a| a == "--permission-mode"));
        assert_eq!(resumed.resume.as_deref(), Some(sid));
        assert_eq!(resumed.permission, Some(first));

        for (i, body) in ["{broken", r#"{ "permissions": { "defaultMode": 0 } }"#].into_iter().enumerate() {
            std::fs::write(account.join("settings.json"), body).unwrap();
            let id = format!("golden-claude-unreadable-{i}");
            let r = Redact::for_chat(&id).with(acc.clone(), "<acc>");
            let s = ClaudeAsk { account: acc.clone(), mode: "", ..ClaudeAsk::chat() }.start(&id, &r);
            assert_eq!(s.value("permission-mode"), None);
            assert_eq!(s.registered["modeFrom"], "cli");
            assert_eq!(s.registered["permissionMode"], "claude");
        }
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn golden_claude_resumes_a_conversation_that_has_a_transcript_and_only_then() {
        const ID: &str = "golden-claude-resume";
        const SID: &str = "0a1b2c3d-0000-4000-8000-00000000c1a0";
        let account = std::env::temp_dir().join(format!("luna-golden-resume-{}", std::process::id()));
        let projects = account.join("projects").join("golden");
        std::fs::create_dir_all(&projects).unwrap();
        let transcript = projects.join(format!("{SID}.jsonl"));
        std::fs::write(&transcript, "{}\n").unwrap();
        let acc = account.to_string_lossy().into_owned();
        let r = Redact::for_chat(ID).with(acc.clone(), "<acc>");
        let s = ClaudeAsk { account: acc.clone(), mode: "acceptEdits", resume: Some(SID.into()), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&[
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission-mode",
                "acceptEdits",
                "--resume",
                SID,
                "--settings",
                "<settings-file>",
            ])
        );
        assert_eq!(s.envs, claude_envs(ID, "<acc>"));
        assert_eq!(s.resume.as_deref(), Some(SID));

        for (tools, agent) in [(Some(true), None), (None, Some("reviewer"))] {
            let s = ClaudeAsk { account: acc.clone(), resume: Some(SID.into()), tools, agent, ..ClaudeAsk::chat() }.start(ID, &r);
            assert!(s.args.windows(2).any(|pair| pair == ["--resume", SID]));
            assert!(s.args.windows(2).any(|pair| pair == claude_launch::FRESH_PROMPT));
        }

        std::fs::write(
            &transcript,
            r#"{"type":"attachment","attachment":{"type":"prompt_snapshot","tools":[{"name":"mcp__luna__luna_session_list","server":"luna"}]}}"#,
        )
        .unwrap();
        let s = ClaudeAsk { account: acc.clone(), resume: Some(SID.into()), tools: Some(false), ..ClaudeAsk::chat() }.start(ID, &r);
        assert!(s.args.windows(2).any(|pair| pair == ["--resume", SID]));
        assert!(s.args.windows(2).any(|pair| pair == claude_launch::FRESH_PROMPT));
        assert_eq!(s.mcp, None, "the fresh prompt removes the recorded Luna tools");

        // No transcript: `--resume` would fail, and it starts afresh.
        let s = ClaudeAsk { account: acc, resume: Some("0a1b2c3d-0000-4000-8000-00000000dead".into()), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&["--model", "opus", "--effort", "high", "--permission-mode", "default", "--settings", "<settings-file>",])
        );
        assert_eq!(s.resume, None);
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn golden_claude_resume_uses_fresh_prompt_until_its_layers_are_removed() {
        const ID: &str = "golden-claude-resume-layers";
        const SID: &str = "0a1b2c3d-0000-4000-8000-00000000c1a1";
        let lib = crate::library::GoldenLibrary::new("pane-claude-resume-layers");
        let r = library_redact(ID, &lib, &lib.claude());
        let projects = std::path::Path::new(&lib.claude()).join("projects").join("golden");
        std::fs::create_dir_all(&projects).unwrap();
        // A Luna start with layers renders fresh and writes no snapshot.
        std::fs::write(projects.join(format!("{SID}.jsonl")), "{}\n").unwrap();
        let s =
            ClaudeAsk { folder: lib.folder.clone(), account: lib.claude(), resume: Some(SID.into()), ..ClaudeAsk::chat() }.start(ID, &r);
        assert!(s.args.windows(2).any(|pair| pair == ["--resume", SID]));
        assert!(s.args.windows(2).any(|pair| pair == claude_launch::FRESH_PROMPT));
        assert!(s.instructions.is_some());

        // Resuming outside the library folder gives neither its layers nor skills.
        let s = ClaudeAsk { account: lib.claude(), resume: Some(SID.into()), ..ClaudeAsk::chat() }.start(ID, &r);
        assert!(s.args.windows(2).any(|pair| pair == ["--resume", SID]));
        assert!(!s.args.iter().any(|arg| arg == "--system-prompt-snapshot"));
        assert_eq!(s.instructions, None);
    }

    #[test]
    fn golden_claude_worktree_goes_by_its_name_or_the_clis() {
        const ID: &str = "golden-claude-worktree";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk { worktree: true, worktree_name: Some("feature-x"), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&[
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission-mode",
                "default",
                "--worktree=feature-x",
                "--settings",
                "<settings-file>",
            ])
        );
        assert_eq!(s.describe, "model opus perm default worktree true");
        assert_eq!(s.cwd, FOLDER, "it starts in the project and moves itself");
        // No name: the CLI names it.
        let s = ClaudeAsk { worktree: true, ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&[
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission-mode",
                "default",
                "--worktree",
                "--settings",
                "<settings-file>",
            ])
        );
    }

    #[test]
    fn golden_claude_login_runs_in_its_account_with_nothing_of_a_chat() {
        const ID: &str = "golden-claude-login";
        const ACC: &str = "/golden/acc-claude-login";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk { folder: ACC.into(), account: ACC.into(), tools: Some(true), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&[
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission-mode",
                "default",
                "--settings",
                r#"{"permissions":{"deny":["Read(//<core-dir>/**)"]},"tui":"fullscreen"}"#,
            ])
        );
        assert_eq!(s.envs, claude_envs(ID, ACC), "no token: not a chat agents may reach");
        assert!(s.settings.is_none() && s.mcp.is_none() && s.instructions.is_none(), "no file of Luna's: the core's deny goes inline");
        assert_eq!(s.cwd, ACC);
        assert_eq!(s.value("core-read-deny").map(|v| v.starts_with("permissions.deny Read(//<core-dir>/**) in --settings {")), Some(true));
        assert!(!s.keys().iter().any(|k| ["activity-hooks", "luna-tools", "notes-dir", "copy-on-select"].contains(k)), "{:?}", s.keys());
    }

    /// The login pane's own id, `login:<provider>:<name>` (LoginModal.tsx):
    /// a start under it writes nothing under the data dir, and so no name with
    /// a `:`, which Windows refuses (os error 123) and which once kept every
    /// Claude Code login from starting.
    #[test]
    fn a_login_start_writes_no_file_of_its_own() {
        const ID: &str = "login:claude:golden";
        let account = std::env::temp_dir().join(format!("luna-golden-login-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let acc = account.to_string_lossy().into_owned();
        let data = crate::paths::data_dir();
        let names = |sub: &str| -> Vec<String> {
            std::fs::read_dir(data.join(sub))
                .map_or_else(|_| Vec::new(), |d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        };
        let r = Redact::for_chat(ID).with(acc.clone(), "<acc>");
        let s = ClaudeAsk { folder: acc.clone(), account: acc, tools: Some(true), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(s.cwd, "<acc>");
        assert!(s.args.iter().all(|a| !a.contains("login")), "{:?}", s.args);
        for sub in ["hooks", "mcp", "media"] {
            let named: Vec<String> = names(sub).into_iter().filter(|n| n.contains(':') || n.starts_with("login")).collect();
            assert!(named.is_empty(), "{sub}/ holds {named:?}");
        }
        let _ = std::fs::remove_dir_all(&account);
    }

    /// A Claude Code chat's start sets copy on select in its account's
    /// `.claude.json` as Settings has it (claude/config.rs), and says so in
    /// what it applied; left to the account, the file stays byte for byte.
    #[test]
    fn a_claude_chat_start_sets_copy_on_select_in_its_account() {
        use crate::settings::CopyOnSelect;
        let account = std::env::temp_dir().join(format!("luna-golden-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&account);
        std::fs::create_dir_all(&account).unwrap();
        let acc = account.to_string_lossy().into_owned();
        let file = account.join(".claude.json");
        let text = "{\n  \"copyOnSelect\": true,\n  \"numStartups\": 2\n}";
        std::fs::write(&file, text).unwrap();
        let start = |id: &str, setting| {
            crate::claude::config::setting_for_test(&acc, Some(setting));
            let r = Redact::for_chat(id).with(acc.clone(), "<acc>");
            ClaudeAsk { account: acc.clone(), ..ClaudeAsk::chat() }.start(id, &r)
        };
        let s = start("golden-claude-copy-account", CopyOnSelect::Account);
        assert_eq!(s.value("copy-on-select"), None);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
        let s = start("golden-claude-copy-off", CopyOnSelect::Off);
        assert_eq!(s.value("copy-on-select"), Some("off: written into .claude.json"));
        let cfg = crate::claude::config::read_config(&file);
        assert_eq!((cfg["copyOnSelect"].as_bool(), cfg["numStartups"].as_u64()), (Some(false), Some(2)));
        crate::claude::config::setting_for_test(&acc, None);
        let _ = std::fs::remove_dir_all(&account);
    }

    /// A chat the window runs is the person's, never a helper: with tools, a role
    /// and an agent it keeps its account's servers and memory, and no helper's
    /// note, opening prompt or parent goes in, nor any helper's item (helpers
    /// start in background/command.rs).
    #[test]
    fn golden_claude_chat_with_tools_and_a_role_gets_luna_mcp_as_the_persons() {
        const ID: &str = "golden-claude-tooled";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk {
            model: "sonnet",
            effort: "medium",
            mode: "bypassPermissions",
            tools: Some(true),
            read_only: Some(true),
            agent: Some("reviewer"),
            ..ClaudeAsk::chat()
        }
        .start(ID, &r);
        assert_eq!(
            s.args,
            strings(&[
                "--model",
                "sonnet",
                "--effort",
                "medium",
                "--permission-mode",
                "bypassPermissions",
                "--settings",
                "<settings-file>",
                "--agent",
                "reviewer",
                "--disallowedTools",
                "Edit,Write,NotebookEdit",
                "--allowedTools",
                golden::CLAUDE_ALLOWED_LUNA_TOOLS,
                "--mcp-config",
                "<mcp-file>",
                "--system-prompt-snapshot",
                "off",
            ])
        );
        assert_eq!(
            s.envs,
            pairs(&[
                ("CLAUDE_CONFIG_DIR", "/golden/acc-claude"),
                ("DISABLE_AUTOUPDATER", "1"),
                ("CLAUDE_CODE_NO_FLICKER", "1"),
                ("LUNA_MCP_TOKEN", "<token>"),
                ("LUNA_SESSION_ID", ID),
            ])
        );
        let mut settings = golden::claude_chat_settings();
        settings["permissions"]["allow"] = serde_json::json!(["Read(//<notes-dir>/**)"]);
        assert_eq!(s.settings, Some(settings), "its notes readable without asking, by no --add-dir");
        assert_eq!(s.mcp, Some(golden::claude_mcp_config()), "the token stays in the environment");
        assert_eq!(s.describe, "model sonnet perm bypassPermissions worktree false");
        assert_eq!(
            s.keys(),
            [
                "account",
                "no-self-update",
                "session-id",
                "model",
                "effort",
                "permission-mode",
                "activity-hooks",
                "core-read-deny",
                "notes-dir",
                "agent",
                "read-only",
                "luna-tools",
                "claude.fresh-prompt",
                "claude.fullscreen",
                "copy-on-select",
                "terminal",
                "cwd",
                "inherited-env-cleared",
            ]
        );
        assert_eq!(s.value("session-id"), Some("LUNA_SESSION_ID=golden-claude-tooled"));
        assert_eq!(s.value("notes-dir"), Some("permissions.allow Read(//<notes-dir>/**) in --settings <settings-file>"));
    }

    /// The placeholders for a start in a library's folder on `account`.
    fn library_redact(id: &str, lib: &crate::library::GoldenLibrary, account: &str) -> Redact {
        Redact::for_chat(id)
            .with(lib.folder.clone(), "<project>")
            .with(account, "<acc>")
            // As `claudeMdExcludes` names a file, on every OS.
            .with(account.replace('\\', "/"), "<acc>")
    }

    /// Its folder's skills are a plugin of the chat's own, and nothing but the
    /// account's skill the folder keeps is hidden by name.
    #[test]
    fn golden_claude_chat_in_a_library_folder_gets_its_skills_and_instructions() {
        const ID: &str = "golden-claude-library";
        let lib = crate::library::GoldenLibrary::new("pane-claude");
        let r = library_redact(ID, &lib, &lib.claude());
        let s = ClaudeAsk { folder: lib.folder.clone(), account: lib.claude(), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&[
                "--model",
                "opus",
                "--effort",
                "high",
                "--permission-mode",
                "default",
                "--settings",
                "<settings-file>",
                "--append-system-prompt-file",
                "<instructions-file>",
                "--system-prompt-snapshot",
                "off",
                "--plugin-dir",
                "<plugin-dir>",
            ])
        );
        let mut settings = golden::claude_chat_settings();
        settings["skillOverrides"] = serde_json::json!({ "review": "off" });
        settings["claudeMdExcludes"] = serde_json::json!(["<acc>/CLAUDE.md"]);
        assert_eq!(s.settings, Some(settings), "the folder keeps review and the account's instructions from it");
        assert_eq!(s.value("library.skills"), Some("--plugin-dir <plugin-dir>"));
        assert_eq!(s.value("library.hidden-skills"), Some("skillOverrides off: review"));
        assert_eq!(s.value("library.memory-narrowed"), None, "no folder of skills is added: no memory to leave out");
        // The plugin is the folder's `golden`, holding the one skill it gives.
        let dir = crate::paths::data_dir().join("session-skills").join(crate::paths::id_file_name(ID));
        assert!(dir.join("golden/.claude-plugin/plugin.json").is_file());
        assert!(dir.join("golden/skills/notes/SKILL.md").is_file());
        assert!(!dir.join("golden/skills/review").exists());
        crate::library::drop_plugins(ID);
        assert!(!dir.exists());
    }

    #[test]
    fn a_claude_chat_whose_role_layers_will_not_write_starts_without_them_and_says_so() {
        const ID: &str = "golden-claude-layers-unwritable";
        let lib = crate::library::GoldenLibrary::new("pane-claude-unwritable");
        let r = library_redact(ID, &lib, &lib.claude());
        // A file where its folder goes: the write fails as on a locked or full disk.
        let dir = crate::paths::data_dir().join("hooks").join(crate::paths::id_file_name(ID));
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, b"in the way").unwrap();
        let s = ClaudeAsk { folder: lib.folder.clone(), account: lib.claude(), ..ClaudeAsk::chat() }.start(ID, &r);
        let _ = std::fs::remove_file(&dir);
        crate::library::drop_plugins(ID);
        assert!(!s.args.iter().any(|a| a == "<instructions-file>" || a == "--append-system-prompt-file"), "{:?}", s.args);
        assert_eq!(s.instructions, None);
        assert_eq!(s.value("library.skills"), Some("--plugin-dir <plugin-dir>"));
        assert!(s.args.windows(2).any(|pair| pair == claude_launch::FRESH_PROMPT), "the plugin alone needs a fresh prompt");
        assert_eq!(s.value("role.layers"), None);
        let off = s.value("role.layers-off").expect("the chat says it starts without them");
        assert!(off.contains("could not write its role layers"), "{off}");
    }

    /// The window's ask for a Codex chat; each case changes what it is about.
    struct CodexAsk {
        folder: String,
        account: String,
        model: Option<&'static str>,
        effort: &'static str,
        approval: &'static str,
        sandbox: &'static str,
        mode_from: Option<luna_window::dto::ModeSource>,
        worktree: Option<bool>,
        resume: Option<&'static str>,
        login: bool,
        tools: Option<bool>,
        trust_folder: Option<luna_window::dto::ProjectTrust>,
    }

    const CODEX_ACC: &str = "/golden/acc-codex";

    impl CodexAsk {
        fn chat() -> Self {
            CodexAsk {
                folder: golden::project(),
                account: CODEX_ACC.into(),
                model: Some("gpt-5.5"),
                effort: "medium",
                approval: "on-request",
                sandbox: "workspace-write",
                mode_from: None,
                worktree: None,
                resume: None,
                login: false,
                tools: None,
                trust_folder: None,
            }
        }

        fn start(self, id: &str, r: &Redact) -> Started {
            started(id, r, self.ensure(id))
        }

        fn ensure(self, id: &str) -> impl std::future::Future<Output = Result<Attached, String>> {
            let s = |v: Option<&str>| v.map(str::to_owned);
            ensure_codex_session(
                id.into(),
                self.folder,
                self.account,
                s(self.model),
                self.effort.into(),
                self.approval.into(),
                self.sandbox.into(),
                self.worktree,
                s(self.resume),
                self.login,
                self.tools,
                None,
                None,
                self.mode_from,
                self.trust_folder,
            )
        }
    }

    /// A Codex chat's arguments in FOLDER: `head` before `-C`, its own flags,
    /// the title, Luna's status hooks, then `tail`.
    fn codex_args(head: &[&str], flags: Vec<String>, tail: Vec<String>) -> Vec<String> {
        let mut args = strings(head);
        args.extend(strings(&["-C", FOLDER, "--no-daemon"]));
        args.extend(flags);
        args.extend(strings(&["-c", "check_for_update_on_startup=false"]));
        args.extend(golden::codex_tui_settings());
        args.extend(strings(&["-c", "tui.terminal_title=[\"thread-id\"]"]));
        args.extend(golden::codex_status_hooks());
        args.extend(tail);
        args
    }

    /// The model and effort a chat asks for, then `--ask-for-approval`, and
    /// the sandbox as a flag.
    fn codex_flags(model: Option<&str>, effort: &str, approval: &str, sandbox: &str) -> Vec<String> {
        let mut args = model.map_or_else(Vec::new, |m| strings(&["--model", m]));
        args.extend(strings(&["-c", &format!("model_reasoning_effort=\"{effort}\"")]));
        args.extend(codex_asks(approval, sandbox));
        args
    }

    /// `--ask-for-approval` and the sandbox: whatever reviewer the account
    /// or the project names stands. Codex's own from its config (`codex`)
    /// gets no flag.
    fn codex_asks(approval: &str, sandbox: &str) -> Vec<String> {
        let mut args = if approval == "codex" { Vec::new() } else { strings(&["--ask-for-approval", approval]) };
        if sandbox == "read-only" {
            args.extend(golden::codex_read_only(&["--sandbox", sandbox]));
        } else if sandbox != "codex" {
            args.extend(strings(&["--sandbox", sandbox]));
        }
        if sandbox != "danger-full-access" {
            args.extend(golden::codex_windows_sandbox());
        }
        args
    }

    fn codex_envs(id: &str, sandboxed: bool) -> Vec<(String, String)> {
        let mut envs = pairs(&[("CODEX_HOME", CODEX_ACC)]);
        if sandboxed {
            envs.extend(golden::codex_sandbox_path());
        }
        envs.extend(pairs(&[("LUNA_SESSION_ID", id)]));
        envs.extend(golden::codex_hook_env());
        envs
    }

    #[test]
    fn golden_codex_chat_starts_in_its_folder_with_a_person_answering() {
        const ID: &str = "golden-codex-chat";
        let r = Redact::for_chat(ID);
        let s = CodexAsk::chat().start(ID, &r);
        assert_eq!(s.args, codex_args(&[], codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write"), Vec::new()));
        assert!(!s.args.iter().any(|a| a.starts_with("approvals_reviewer")), "the account's reviewer, if it names one, stands");
        assert!(!s.args.iter().any(|a| a.starts_with("projects=")), "no trust Luna was not asked for");
        assert_eq!(s.envs, codex_envs(ID, true));
        assert_eq!((s.cwd.as_str(), s.resume), (FOLDER, None));
        assert_eq!(s.describe, "model gpt-5.5 effort medium approval on-request sandbox workspace-write");
        use crate::applied::Class::*;
        let mut want = vec![
            ("account", Runtime, "CODEX_HOME=/golden/acc-codex".to_string()),
            ("session-id", Runtime, "LUNA_SESSION_ID=golden-codex-chat".into()),
            ("folder", Runtime, "-C /golden/project".into()),
            ("codex.no-daemon", Runtime, "--no-daemon".into()),
            ("model", UserAction, "--model gpt-5.5".into()),
            ("effort", UserAction, "-c model_reasoning_effort=\"medium\"".into()),
            ("approval", UserAction, "--ask-for-approval on-request".into()),
            ("sandbox", UserAction, "--sandbox workspace-write".into()),
        ];
        if cfg!(windows) {
            want.push(("codex.windows-sandbox", Workaround, "-c windows.sandbox=\"unelevated\"".into()));
            want.push(("codex.sandbox-path", Workaround, "PATH without …\\Microsoft\\WindowsApps".into()));
        }
        want.push(("no-self-update", Runtime, "-c check_for_update_on_startup=false".into()));
        want.push(("codex.tui-settings", Runtime, "-c tui.notifications=false".into()));
        // Settings' default, off (codex/launch.rs has the three values).
        want.push(("copy-on-select", UserAction, "-c tui.copy_on_select=\"never\"".into()));
        want.push(("codex.thread-title", Runtime, "-c tui.terminal_title=[\"thread-id\"]".into()));
        if codex_hook_curl().is_some() {
            want.push(("activity-hooks", Runtime, crate::codex::launch::HOOKS_VALUE.into()));
        }
        want.push(("terminal", Runtime, "TERM=xterm-256color".into()));
        want.push(("cwd", Runtime, "cwd /golden/project".into()));
        want.push(("inherited-env-cleared", Runtime, removed()));
        assert_eq!(s.applied, want.into_iter().map(|(k, c, v)| (k.to_string(), c, v)).collect::<Vec<_>>());
    }

    /// No chat of either CLI is given anything of a handoff, with Luna's
    /// tools or without: Luna has none.
    #[test]
    fn a_chat_starts_with_nothing_of_a_handoff() {
        const ID: &str = "golden-plain-tooled-chat";
        let r = Redact::for_chat(ID);
        for s in [
            ClaudeAsk { tools: Some(true), ..ClaudeAsk::chat() }.start(ID, &r),
            CodexAsk { tools: Some(true), ..CodexAsk::chat() }.start(ID, &r),
            ClaudeAsk::chat().start(ID, &r),
            CodexAsk::chat().start(ID, &r),
        ] {
            let said = format!("{:?}{:?}{:?}", s.args, s.envs, s.applied).to_lowercase();
            assert!(!said.contains("handoff"), "{said}");
        }
    }

    #[test]
    fn golden_codex_chat_keeps_a_windows_sandbox_its_user_chose() {
        const ID: &str = "golden-codex-own-sandbox";
        let account = std::env::temp_dir().join(format!("luna-golden-winsandbox-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("config.toml"), "[windows]\nsandbox = \"elevated\"\n").unwrap();
        let acc = account.to_string_lossy().into_owned();
        let r = Redact::for_chat(ID).with(acc.clone(), "<acc>");
        let s = CodexAsk { account: acc, ..CodexAsk::chat() }.start(ID, &r);
        assert!(!s.args.iter().any(|a| a.starts_with("windows.sandbox")), "{:?}", s.args);
        assert!(!s.keys().contains(&"codex.windows-sandbox"));
        let _ = std::fs::remove_dir_all(&account);
        // None set: Luna's, where Windows needs one.
        let s = CodexAsk::chat().start(ID, &r);
        assert_eq!(s.args.iter().any(|a| a == "windows.sandbox=\"unelevated\""), cfg!(windows));
    }

    /// Neither the wait guard nor the trust bypass is in any Codex start, with
    /// Luna's tools or without: no `--dangerously-bypass-hook-trust`, no hook.
    #[test]
    fn golden_codex_chat_has_its_status_hooks_alone_and_no_hook_trust_bypass() {
        const ID: &str = "golden-codex-no-guard";
        let r = Redact::for_chat(ID);
        let ours = golden::codex_status_hooks();
        let ours: Vec<&String> = ours.iter().filter(|a| *a != "-c").collect();
        for tools in [None, Some(true)] {
            let s = CodexAsk { tools, ..CodexAsk::chat() }.start(ID, &r);
            assert!(!s.args.iter().any(|a| a == "--dangerously-bypass-hook-trust"), "{:?}", s.args);
            let hooks: Vec<&String> = s.args.iter().filter(|a| a.starts_with("hooks.")).collect();
            assert_eq!(hooks, ours, "Luna's status hooks alone, each trusted by its own hash: no wait guard, no other hook trusted");
            assert!(!s.keys().iter().any(|k| k.ends_with("guard") || *k == "hooks-unasked"), "{:?}", s.keys());
        }
    }

    #[test]
    fn golden_codex_resume_names_its_thread_and_never_a_worktree() {
        const ID: &str = "golden-codex-resume";
        const THREAD: &str = "019a0000-0000-7000-8000-000000000c0d";
        let r = Redact::for_chat(ID);
        let s = CodexAsk { resume: Some(THREAD), worktree: Some(true), ..CodexAsk::chat() }.start(ID, &r);
        let flags = codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write");
        assert_eq!(s.args, codex_args(&["resume", THREAD], flags, Vec::new()));
        assert_eq!(s.envs, codex_envs(ID, true));
        assert_eq!(s.resume.as_deref(), Some(THREAD));
    }

    #[test]
    fn golden_codex_trust_in_luna_goes_to_start_and_resume_without_writing_the_account() {
        const ID: &str = "golden-codex-trust";
        const THREAD: &str = "019a0000-0000-7000-8000-000000000c0d";
        let account = std::env::temp_dir().join(format!("luna-golden-codex-trust-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let config = account.join("config.toml");
        let old = "model = \"gpt-5.5\"\n[projects.'/other']\ntrust_level = \"trusted\"\n";
        std::fs::write(&config, old).unwrap();
        let acc = account.to_string_lossy().into_owned();
        let r = Redact::for_chat(ID).with(acc.clone(), "<acc>");
        for (resume, head) in [(None, &[][..]), (Some(THREAD), &["resume", THREAD][..])] {
            let s = CodexAsk {
                account: acc.clone(),
                trust_folder: crate::codex::trust::project_trust(&golden::project()),
                resume,
                ..CodexAsk::chat()
            }
            .start(ID, &r);
            let mut args = s.args.clone();
            let trust = args.remove(head.len() + 4);
            assert_eq!(args.remove(head.len() + 3), "-c");
            let doc: toml_edit::DocumentMut = trust.parse().unwrap();
            let projects = doc["projects"].as_inline_table().unwrap();
            let (key, decision) = projects.iter().next().unwrap();
            assert_eq!(projects.len(), 1);
            assert!(key == FOLDER || crate::paths::path_key(key) == crate::paths::path_key(&golden::project()), "{key}");
            assert_eq!(decision.as_inline_table().unwrap().get("trust_level").and_then(toml_edit::Value::as_str), Some("trusted"));
            assert_eq!(args, codex_args(head, codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write"), Vec::new()));
            assert_eq!(s.value("session-trust"), Some(format!("-c {trust}").as_str()));
        }
        let without_grant = CodexAsk { account: acc.clone(), resume: Some(THREAD), ..CodexAsk::chat() }.start(ID, &r);
        assert!(!without_grant.args.iter().any(|a| a.starts_with("projects=")), "a window start with null ignores the saved grant");
        assert_eq!(without_grant.value("session-trust-dropped"), None);
        let unrelated = account.join("unrelated");
        std::fs::create_dir_all(&unrelated).unwrap();
        let resumed_elsewhere = CodexAsk {
            folder: unrelated.to_string_lossy().into_owned(),
            account: acc.clone(),
            trust_folder: crate::codex::trust::project_trust(&golden::project()),
            resume: Some(THREAD),
            ..CodexAsk::chat()
        }
        .start(ID, &r);
        assert!(!resumed_elsewhere.args.iter().any(|a| a.starts_with("projects=")), "a resume in another project needs its own trust");
        assert!(resumed_elsewhere.value("session-trust-dropped").is_some());
        let other = CodexAsk { account: acc, ..CodexAsk::chat() }.start("golden-codex-no-trust", &r);
        assert!(!other.args.iter().any(|a| a.starts_with("projects=")));
        assert_eq!(std::fs::read_to_string(config).unwrap(), old);
        let _ = std::fs::remove_dir_all(account);
    }

    #[test]
    fn golden_codex_worktree_is_codexs_own_to_make() {
        const ID: &str = "golden-codex-worktree";
        let r = Redact::for_chat(ID);
        let s = CodexAsk { worktree: Some(true), model: None, ..CodexAsk::chat() }.start(ID, &r);
        let mut flags = strings(&["--worktree"]);
        flags.extend(codex_flags(None, "medium", "on-request", "workspace-write"));
        assert_eq!(s.args, codex_args(&[], flags, Vec::new()));
        assert_eq!(s.cwd, FOLDER, "it starts in the project and moves itself");
        assert_eq!(s.describe, "model default effort medium approval on-request sandbox workspace-write");
    }

    #[test]
    fn golden_codex_login_is_the_bare_command() {
        const ID: &str = "golden-codex-login";
        let r = Redact::for_chat(ID);
        let s = CodexAsk {
            login: true,
            resume: Some("019a0000-0000-7000-8000-00000000dead"),
            tools: Some(true),
            trust_folder: crate::codex::trust::project_trust(&golden::project()),
            ..CodexAsk::chat()
        }
        .start(ID, &r);
        assert_eq!(s.args, ["login"]);
        assert_eq!(s.envs, pairs(&[("CODEX_HOME", CODEX_ACC)]));
        assert_eq!((s.resume, s.describe.as_str()), (None, "login"));
    }

    /// A chat the window runs is the person's, never a helper: with tools it
    /// keeps its account's apps, servers and reviewer, is trusted only as the
    /// person trusted it, and nothing of a helper's start is applied (helpers
    /// start in background/command.rs).
    #[test]
    fn golden_codex_chat_with_tools_gets_luna_mcp_as_the_persons() {
        crate::codex::config::use_empty_machine_layers_for_test();
        const ID: &str = "golden-codex-tooled";
        let r = Redact::for_chat(ID);
        for (approval, prompt) in [("never", cfg!(target_os = "macos")), ("on-request", true), (AUTO_REVIEW, true), ("codex", true)] {
            let s = CodexAsk { model: None, effort: "high", approval, tools: Some(true), ..CodexAsk::chat() }.start(ID, &r);
            let flags = if approval == AUTO_REVIEW {
                let mut args = strings(&["-c", "model_reasoning_effort=\"high\"", "--approve-for-me"]);
                args.extend(golden::codex_windows_sandbox());
                args
            } else {
                codex_flags(None, "high", approval, "workspace-write")
            };
            assert_eq!(s.args, codex_args(&[], flags, golden::codex_mcp(prompt)), "{approval}");
            let mut envs = codex_envs(ID, true);
            envs.extend(pairs(&[("LUNA_MCP_TOKEN", "<token>")]));
            assert_eq!(s.envs, envs);
            assert_eq!(s.value("luna-tools"), Some(crate::codex::launch::luna_tools_value(prompt).as_str()));
            assert_eq!(s.value("session-id"), Some("LUNA_SESSION_ID=golden-codex-tooled"));
            assert!(!s.keys().iter().any(|k| k.starts_with("helper.")), "{:?}", s.keys());
        }
    }

    /// A chat given Luna's tools is refused, before it is registered as given
    /// them, where its account names an MCP server `luna`: Codex would merge
    /// it into Luna's own. Without the tools the account's `luna` is the
    /// chat's, and it starts.
    #[test]
    fn a_codex_chat_with_tools_is_refused_where_its_account_has_a_server_named_luna() {
        const ID: &str = "golden-codex-luna-taken";
        let account = std::env::temp_dir().join(format!("luna-golden-luna-taken-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("config.toml"), "[mcp_servers.luna]\ncommand = \"node\"\n").unwrap();
        let acc = account.to_string_lossy().into_owned();
        let ask = || CodexAsk { account: acc.clone(), tools: Some(true), ..CodexAsk::chat() };
        capture::expect(ID);
        let got = crate::runtime::block_on(ask().ensure(ID));
        assert!(!crate::agents::is_known(ID), "refused before it was registered");
        assert!(capture::take(ID).is_none(), "no launch built");
        let why = got.expect_err("refused");
        let file = account.join("config.toml").display().to_string();
        assert!(why.starts_with(&format!("the chat is not started: {file} defines an MCP server named luna")), "{why}");
        assert!(why.ends_with("rename that server in that file, or start the chat without Luna tools"), "{why}");
        let r = Redact::for_chat(ID).with(acc.clone(), "<acc>");
        let s = CodexAsk { tools: None, ..ask() }.start(ID, &r);
        assert!(!s.args.iter().any(|a| a.starts_with("mcp_servers.luna.")), "{:?}", s.args);
        // A project saved as untrusted: Codex leaves its layer out, a `luna`
        // in it or one that will not parse, and the chat starts with its tools.
        let project = std::env::temp_dir().join(format!("luna-golden-untrusted-project-{}", std::process::id()));
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        let folder = project.to_string_lossy().into_owned();
        std::fs::write(account.join("config.toml"), format!("[projects.'{folder}']\ntrust_level = \"untrusted\"\n")).unwrap();
        let r = r.with(folder.clone(), "<project>");
        for layer in ["[mcp_servers.luna]\ncommand = \"node\"\n", "[mcp_servers.luna\n"] {
            std::fs::write(project.join(".codex").join("config.toml"), layer).unwrap();
            let s = CodexAsk { folder: folder.clone(), ..ask() }.start(ID, &r);
            assert!(s.args.iter().any(|a| a.starts_with("mcp_servers.luna.url=")), "{layer}: {:?}", s.args);
        }
        std::fs::write(project.join(".codex/config.toml"), "[mcp_servers.luna]\ncommand = \"node\"\n").unwrap();
        capture::expect(ID);
        let trusted = CodexAsk { trust_folder: crate::codex::trust::project_trust(&folder), folder, ..ask() };
        let why = crate::runtime::block_on(trusted.ensure(ID)).expect_err("per-run trust enables the conflicting project server");
        assert!(why.contains("defines an MCP server named luna"), "{why}");
        assert!(capture::take(ID).is_none());
        let _ = std::fs::remove_dir_all(&account);
        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn golden_codex_approval_and_sandbox_pairs() {
        let with_windows = |mut a: Vec<String>| {
            a.extend(golden::codex_windows_sandbox());
            a
        };
        let mut read_only_net = strings(&["--ask-for-approval", "on-request"]);
        read_only_net.extend(golden::codex_read_only_net());
        // (approval, sandbox, the flags for them, PATH cut for the sandbox, the approval and sandbox it runs in)
        let cases: [(&str, &str, Vec<String>, bool, (&str, &str)); 9] = [
            (
                "never",
                "danger-full-access",
                strings(&["--dangerously-bypass-approvals-and-sandbox"]),
                false,
                ("never", "danger-full-access"),
            ),
            (
                "on-request",
                "danger-full-access",
                codex_asks("on-request", "danger-full-access"),
                false,
                ("on-request", "danger-full-access"),
            ),
            ("on-request", "read-only-net", with_windows(read_only_net), true, ("on-request", "read-only-net")),
            // The reviewer model takes the sandbox it runs in, whatever was asked.
            ("auto-review", "read-only", with_windows(strings(&["--approve-for-me"])), true, ("auto-review", "workspace-write")),
            // `--ask-for-approval` refuses all but on-request and never
            // (0.169.0): Codex's alias as what it stands for, and what Luna
            // does not know, `untrusted` (which 0.169.0 no longer runs) and
            // a granular the files do not say included, closed.
            ("on-failure", "workspace-write", codex_asks("on-request", "workspace-write"), true, ("on-request", "workspace-write")),
            ("untrusted", "read-only", codex_asks("on-request", "read-only"), true, ("on-request", "read-only")),
            // Codex's own from its config, and what 0.20's first builds said
            // for a granular table: no flag.
            ("granular", "workspace-write", codex_asks("codex", "workspace-write"), true, ("codex", "workspace-write")),
            ("codex", "codex", codex_asks("codex", "codex"), true, ("codex", "codex")),
            ("Never", "everything", codex_asks("never", "read-only"), true, ("never", "read-only")),
        ];
        for (approval, sandbox, asks, sandboxed, (runs, runs_in)) in cases {
            let id = format!("golden-codex-{}-{sandbox}", approval.to_lowercase());
            let r = Redact::for_chat(&id);
            let s = CodexAsk { approval, sandbox, ..CodexAsk::chat() }.start(&id, &r);
            let mut flags = strings(&["--model", "gpt-5.5", "-c", "model_reasoning_effort=\"medium\""]);
            flags.extend(asks);
            assert_eq!(s.args, codex_args(&[], flags, Vec::new()), "{approval} {sandbox}");
            let mut envs = codex_envs(&id, sandboxed);
            if sandbox == READ_ONLY_NET {
                // git's https, which schannel cannot do there: after the
                // home and the sandbox's PATH.
                envs.splice(2..2, golden::codex_git_ssl());
            }
            assert_eq!(s.envs, envs, "{approval} {sandbox}");
            assert_eq!(s.describe, format!("model gpt-5.5 effort medium approval {runs} sandbox {runs_in}"));
        }
    }

    /// A chat on Codex's own approval and sandbox (`codex`) gets no flag for
    /// either, whatever its account's config says, a granular table among it:
    /// Codex reads its config itself.
    #[test]
    fn golden_codex_chat_on_codexs_own_gets_no_mode_flag() {
        crate::codex::config::use_empty_machine_layers_for_test();
        const ID: &str = "golden-codex-own";
        let account = std::env::temp_dir().join(format!("luna-golden-granular-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(
            account.join("config.toml"),
            "[approval_policy.granular]\nsandbox_approval = true\nrules = false\nmcp_elicitations = true\n",
        )
        .unwrap();
        let acc = account.to_string_lossy().into_owned();
        let r = Redact::for_chat(ID).with(acc.clone(), "<acc>");
        let s = CodexAsk { account: acc, approval: "", sandbox: "", ..CodexAsk::chat() }.start(ID, &r);
        let mut flags = strings(&["--model", "gpt-5.5", "-c", "model_reasoning_effort=\"medium\""]);
        flags.extend(codex_asks("codex", "codex"));
        assert_eq!(s.args, codex_args(&[], flags, Vec::new()));
        assert!(!s.args.iter().any(|a| a == "--ask-for-approval" || a == "--sandbox" || a.starts_with("approval_policy")), "{:?}", s.args);
        assert_eq!((s.value("approval"), s.value("sandbox")), (None, None));
        assert_eq!(s.describe, "model gpt-5.5 effort medium approval codex sandbox codex");
        assert_eq!(s.registered["modeFrom"], "cli");
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn golden_codex_chat_defaults_to_auto_review() {
        crate::codex::config::use_empty_machine_layers_for_test();
        const ID: &str = "golden-codex-luna-mode";
        let r = Redact::for_chat(ID);
        let s = CodexAsk { approval: "", sandbox: "", ..CodexAsk::chat() }.start(ID, &r);
        let mut flags = strings(&["--model", "gpt-5.5", "-c", "model_reasoning_effort=\"medium\"", "--approve-for-me"]);
        flags.extend(golden::codex_windows_sandbox());
        assert_eq!(s.args, codex_args(&[], flags, Vec::new()));
        assert_eq!(s.registered["modeFrom"], "luna");
        assert_eq!(s.registered["approval"], "auto-review");
        assert_eq!(s.registered["sandbox"], "workspace-write");
    }

    #[test]
    fn golden_codex_chat_resumes_with_its_saved_closed_mode() {
        crate::codex::config::use_empty_machine_layers_for_test();
        let account = std::env::temp_dir().join(format!("luna-golden-codex-closed-mode-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let file = account.join("config.toml");
        std::fs::write(&file, "approval_policy = \"sometimes\"\n").unwrap();
        let acc = account.to_string_lossy().into_owned();
        let id = "golden-codex-closed-first";
        let r = Redact::for_chat(id).with(acc.clone(), "<acc>");
        let first = CodexAsk { account: acc.clone(), approval: "", sandbox: "", ..CodexAsk::chat() }.start(id, &r);
        let flags = codex_flags(Some("gpt-5.5"), "medium", "on-request", "read-only");
        assert_eq!(first.args, codex_args(&[], flags.clone(), Vec::new()));
        let permission = first.permission.expect("the start returns its resolved mode");
        assert_eq!(permission.mode_from, luna_window::dto::ModeSource::Closed);

        std::fs::write(file, "approval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n").unwrap();
        let id = "golden-codex-closed-resume";
        let r = Redact::for_chat(id).with(acc.clone(), "<acc>");
        const THREAD: &str = "019a0000-0000-7000-8000-00000000aced";
        let resumed = CodexAsk {
            account: acc,
            approval: "on-request",
            sandbox: "read-only",
            mode_from: Some(permission.mode_from),
            resume: Some(THREAD),
            ..CodexAsk::chat()
        }
        .start(id, &r);
        assert_eq!(resumed.args, codex_args(&["resume", THREAD], flags, Vec::new()));
        assert_eq!(resumed.permission, Some(permission));
        let _ = std::fs::remove_dir_all(account);
    }
    #[test]
    fn golden_codex_chat_in_a_library_folder_gets_its_plugin_and_keeps_its_skills_off() {
        const ID: &str = "golden-codex-library";
        let lib = crate::library::GoldenLibrary::new("pane-codex");
        let r = library_redact(ID, &lib, &lib.codex());
        let s = CodexAsk { folder: lib.folder.clone(), account: lib.codex(), ..CodexAsk::chat() }.start(ID, &r);
        let mut want = strings(&["-C", "<project>", "--no-daemon"]);
        want.extend(codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write"));
        want.extend(strings(&["-c", "check_for_update_on_startup=false"]));
        want.extend(golden::codex_tui_settings());
        want.extend(strings(&["-c", "tui.terminal_title=[\"thread-id\"]"]));
        want.extend(golden::codex_status_hooks());
        want.extend(strings(&[
            "-c",
            &format!("plugins.{}@rm-luna.enabled=true", crate::library::GoldenLibrary::SLUG),
            "-c",
            // By the path of its `SKILL.md`, not by name.
            "skills.config=[{path=\"<acc>/skills/review/SKILL.md\",enabled=false}]",
        ]));
        assert_eq!(s.args, want);
        let mut envs = pairs(&[("CODEX_HOME", "<acc>")]);
        envs.extend(golden::codex_sandbox_path());
        envs.extend(pairs(&[("LUNA_SESSION_ID", ID)]));
        envs.extend(golden::codex_hook_env());
        assert_eq!(s.envs, envs);
        assert_eq!(s.instructions, None, "Codex has no way to take other instructions");
        assert_eq!(s.cwd, "<project>");
        let plugin = format!("-c plugins.{}@rm-luna.enabled=true", crate::library::GoldenLibrary::SLUG);
        assert_eq!(s.value("library.skills"), Some(plugin.as_str()));
        assert_eq!(s.value("library.hidden-skills"), Some("-c skills.config=[{path=\"<acc>/skills/review/SKILL.md\",enabled=false}]"));
        assert_eq!(s.skills_unhidden, 0);
    }

    #[test]
    fn golden_codex_chat_caps_skill_selectors_and_returns_the_overflow_count() {
        const ID: &str = "golden-codex-library-overflow";
        let names: Vec<_> = (0..80).map(|i| format!("a-skill-with-a-longish-name-{i:02}")).collect();
        let lib =
            crate::library::GoldenLibrary::with_folder_exclusions("pane-overflow", &names.iter().map(String::as_str).collect::<Vec<_>>());
        let account = lib.codex();
        // Keep the account's full path in argv to measure what Windows gets.
        let r = Redact::for_chat(ID);
        let s = CodexAsk { folder: lib.folder.clone(), account, ..CodexAsk::chat() }.start(ID, &r);
        let value = s.args.iter().find(|a| a.starts_with("skills.config=")).unwrap();
        assert!(crate::library::command_line_units(value) <= crate::library::CODEX_HIDE_MAX);
        assert!(s.skills_unhidden > 0);
        assert_eq!(value.matches("{path=").count() + s.skills_unhidden, names.len());
        assert!(s.value("library.hidden-trimmed").unwrap().starts_with(&format!("{} stay visible", s.skills_unhidden)));
    }

    // The person's own chat where every entry of the library is theirs
    // (library/audience.rs): started as with no audience set at all. Helpers'
    // turns on the same library: background/command.rs.

    #[test]
    fn golden_claude_chat_gets_what_the_library_means_for_chats_as_ever() {
        const ID: &str = "golden-claude-chats-only";
        let lib = crate::library::GoldenLibrary::chats_only("pane-claude-audience");
        let r = library_redact(ID, &lib, &lib.claude());
        let s = ClaudeAsk { folder: lib.folder.clone(), account: lib.claude(), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&["--model", "opus", "--effort", "high", "--permission-mode", "default", "--settings", "<settings-file>",])
        );
        assert_eq!(s.envs, claude_envs(ID, "<acc>"));
        assert_eq!(s.settings, Some(golden::claude_chat_settings()), "nothing hidden, nothing left out");
        assert_eq!(s.instructions, None);
        for key in ["library.audience", "library.hidden-skills", "library.account-instructions-off"] {
            assert_eq!(s.value(key), None, "{key}");
        }
    }

    #[test]
    fn golden_codex_chat_gets_what_the_library_means_for_chats_as_ever() {
        const ID: &str = "golden-codex-chats-only";
        let lib = crate::library::GoldenLibrary::chats_only("pane-codex-audience");
        let r = library_redact(ID, &lib, &lib.codex());
        let s = CodexAsk { folder: lib.folder.clone(), account: lib.codex(), ..CodexAsk::chat() }.start(ID, &r);
        let mut want = strings(&["-C", "<project>", "--no-daemon"]);
        want.extend(codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write"));
        want.extend(strings(&["-c", "check_for_update_on_startup=false"]));
        want.extend(golden::codex_tui_settings());
        want.extend(strings(&["-c", "tui.terminal_title=[\"thread-id\"]"]));
        want.extend(golden::codex_status_hooks());
        assert_eq!(s.args, want, "no skill turned off");
        for key in ["library.audience", "library.hidden-skills"] {
            assert_eq!(s.value(key), None, "{key}");
        }
    }

    /// A chat's layers are the library's instructions for chats alone, under
    /// their heading, with nothing of Luna's before them.
    fn chat_layers() -> String {
        format!("# Instructions: main\n\n{}", crate::library::GoldenLibrary::PER_SESSION)
    }

    #[test]
    fn golden_claude_chat_gets_the_library_s_instructions_for_chats_and_nothing_else_of_a_role() {
        use crate::settings::{Audience, Role};
        for (tag, audience, given) in [("chats", Audience::CHATS, Some(chat_layers())), ("helpers", Audience::of(&[Role::Helper]), None)] {
            let id = format!("golden-claude-per-session-{tag}");
            let lib = crate::library::GoldenLibrary::per_session(&format!("pane-claude-per-session-{tag}"), audience);
            let r = library_redact(&id, &lib, &lib.claude());
            let s = ClaudeAsk { folder: lib.folder.clone(), account: lib.claude(), ..ClaudeAsk::chat() }.start(&id, &r);
            let file = s.args.iter().position(|a| a == "--append-system-prompt-file").map(|i| s.args[i + 1].as_str());
            assert_eq!(file, given.as_ref().map(|_| "<instructions-file>"), "{tag}: {:?}", s.args);
            assert_eq!(s.instructions, given, "{tag}");
            let applied = s.value("role.layers");
            assert_eq!(applied.is_some(), given.is_some(), "{tag}: {applied:?}");
            assert!(applied.is_none_or(|a| a.ends_with("; the library's instructions for its role: main")), "{applied:?}");
        }
    }

    #[test]
    fn golden_codex_chat_gets_the_library_s_instructions_for_chats_and_nothing_else_of_a_role() {
        use crate::settings::{Audience, Role};
        for (tag, audience, given) in [("chats", Audience::CHATS, Some(chat_layers())), ("helpers", Audience::of(&[Role::Helper]), None)] {
            let id = format!("golden-codex-per-session-{tag}");
            let lib = crate::library::GoldenLibrary::per_session(&format!("pane-codex-per-session-{tag}"), audience);
            let r = library_redact(&id, &lib, &lib.codex());
            let s = CodexAsk { folder: lib.folder.clone(), account: lib.codex(), ..CodexAsk::chat() }.start(&id, &r);
            assert_eq!(s.role, given, "{tag}");
            assert_eq!(s.args.iter().any(|a| a == "developer_instructions=<role-layers>"), given.is_some(), "{tag}: {:?}", s.args);
            assert_eq!(
                s.value("role.layers"),
                given.as_ref().map(|_| "-c developer_instructions; the library's instructions for its role: main")
            );
        }
    }

    #[test]
    fn memory_excludes_leave_out_only_folders_beside_the_one_worked_in() {
        use crate::claude::launch::{memory_excludes, AddedDir};
        let at = |d: &str| AddedDir { spellings: vec![d.to_string()], resolved: d.to_string() };
        let added = [at("/data/notes"), at("/work"), at("/work/app/sub"), at("/other/")];
        assert_eq!(
            memory_excludes(&added, "/work/app"),
            [
                "/data/notes/CLAUDE.md",
                "/data/notes/CLAUDE.local.md",
                "/data/notes/.claude/CLAUDE.md",
                "/data/notes/.claude/rules/**",
                "/other/CLAUDE.md",
                "/other/CLAUDE.local.md",
                "/other/.claude/CLAUDE.md",
                "/other/.claude/rules/**",
            ],
            "a folder above the one worked in, or below it, is read as ever"
        );
        assert!(memory_excludes(&[at("/work/application")], "/work/app").len() == 4, "a sibling sharing a prefix is beside it");
        assert_eq!(memory_excludes(&[at(r"C:\Data\notes")], r"C:\Work")[0], "C:/Data/notes/CLAUDE.md", "as Claude Code matches a path");
        if cfg!(windows) {
            assert!(memory_excludes(&[at(r"E:\Work")], r"e:\work\app").is_empty(), "Windows paths in any case");
        }
        // Each spelling of a folder outside gets its patterns; inside, none.
        let linked = AddedDir { spellings: vec!["/link/shared".into(), "/real/shared".into()], resolved: "/real/shared".into() };
        assert_eq!(memory_excludes(&[linked], "/work/app").len(), 8);
        let into_project = AddedDir { spellings: vec!["/link/app".into()], resolved: "/work/app/sub".into() };
        assert!(memory_excludes(&[into_project], "/work/app").is_empty(), "judged where it resolves to");
    }

    #[test]
    fn a_folder_name_goes_into_claude_md_excludes_literally() {
        use crate::claude::launch::{glob_literal, memory_excludes, AddedDir};
        assert_eq!(glob_literal("E:/work/[ab]"), "E:/work/[[]ab[]]");
        assert_eq!(glob_literal("C:/Program Files (x86)/a*b?{c,d}|\"q\""), "C:/Program Files [(]x86[)]/a[*]b[?][{]c,d[}][|][\"]q[\"]");
        // Literal where nothing next to them makes them more: an escape
        // there would match a folder spelled like it as well.
        assert_eq!(glob_literal("E:/w/a!b/@c/d+e,f/!(x)"), "E:/w/a!b/@c/d+e,f/![(]x[)]");
        assert_eq!(
            glob_literal("/home/u/.notes dir/é \u{65e5} 😀#$%&'~^`=;:<>"),
            "/home/u/.notes dir/é \u{65e5} 😀#$%&'~^`=;:<>",
            "nothing else changes"
        );
        let dir = AddedDir { spellings: vec!["E:/work/[ab]".into()], resolved: "E:/work/[ab]".into() };
        let patterns = memory_excludes(&[dir], "E:/work/a");
        assert_eq!(patterns[0], "E:/work/[[]ab[]]/CLAUDE.md", "not a class that matches E:/work/a/CLAUDE.md");
        assert!(patterns.iter().all(|p| p.starts_with("E:/work/[[]ab[]]/")), "{patterns:?}");
        // The account's instructions file, a path too, goes in literally.
        let library = crate::library::SessionSkills { excluded: vec!["E:/acc/[x]/CLAUDE.md".into()], ..Default::default() };
        let settings = claude_settings(None, &library, "/d/core", None);
        assert_eq!(settings["claudeMdExcludes"], serde_json::json!(["E:/acc/[[]x[]]/CLAUDE.md"]));
    }

    /// The patterns `memory_excludes` makes for folders named with every
    /// printable ASCII character, a space and non-ASCII ones, in the places
    /// picomatch reads differently, written as JSON to `LUNA_GLOB_TABLE`: for
    /// checking them with Claude Code's own matcher, which only its binary
    /// has (docs/cli-compatibility.md, `claudeMdExcludes`). Run by hand on a
    /// Claude Code upgrade.
    #[test]
    #[ignore = "writes a table for an outside check; run with LUNA_GLOB_TABLE set"]
    fn claude_md_exclude_patterns_for_the_matcher_check() {
        use crate::claude::launch::{memory_excludes, AddedDir};
        let Ok(out) = std::env::var("LUNA_GLOB_TABLE") else { return };
        let mut chars: Vec<String> = "!\"#$%&'()*+,-.:;<=>?@[]^_`{|}~ ".chars().map(String::from).collect();
        chars.extend(["é", "\u{65e5}", "😀"].map(String::from));
        let mut rows = Vec::new();
        for c in &chars {
            for name in [
                format!("x{c}y"),
                format!("{c}y"),
                format!("x{c}"),
                c.clone(),
                format!("x{c}(y)"),
                format!("x{c}(y|z)"),
                format!("{c}{c}y"),
                format!("x{c}{{a,b}}"),
                format!("x{c}[ab]"),
                format!("x{c}*"),
            ] {
                for base in ["E:/w", "/home/u"] {
                    let dir = format!("{base}/{name}");
                    let added = AddedDir { spellings: vec![dir.clone()], resolved: dir.clone() };
                    let patterns = memory_excludes(&[added], "/elsewhere");
                    rows.push(serde_json::json!({ "dir": dir, "char": c, "name": name, "base": base, "patterns": patterns }));
                }
            }
        }
        std::fs::write(out, serde_json::to_string_pretty(&rows).unwrap()).unwrap();
    }

    #[test]
    fn an_exclude_that_could_match_the_projects_own_instructions_is_not_used() {
        use crate::claude::launch::{keep_own_memory, memory_excludes, AddedDir, OwnMemory};
        let at = |d: &str| AddedDir { spellings: vec![d.to_string()], resolved: d.to_string() };
        let own = |roots: &[(&str, bool)]| roots.iter().map(|(r, n)| OwnMemory { root: r.to_string(), nested: *n }).collect::<Vec<_>>();
        // An added folder [ab]: its pattern E:/w/[[]ab[]]/CLAUDE.md is also,
        // in picomatch's shortcut, the CLAUDE.md of a folder named [[]ab[]].
        let patterns = memory_excludes(&[at("E:/w/[ab]")], "E:/w/[[]ab[]]");
        assert_eq!(patterns.len(), 4);
        let (kept, notes) = keep_own_memory(patterns.clone(), &own(&[("E:/w/[[]ab[]]", true), ("E:/w", false), ("E:", false)]));
        assert!(kept.is_empty(), "{kept:?}");
        assert_eq!(notes, ["could not exclude E:/w/[ab]: its name collides with this project's instruction path E:/w/[[]ab[]]"]);
        // Nor below the project, where Claude Code loads memory on demand.
        let (kept, _) = keep_own_memory(patterns.clone(), &own(&[("E:/w", true)]));
        assert!(kept.is_empty(), "a folder [[]ab[]] under E:/w is the project's");
        // A normal case: nothing of the project's is touched, all are used.
        let normal = memory_excludes(&[at("/data/notes"), at("E:/w/[ab]")], "/work/app");
        let (kept, notes) = keep_own_memory(normal.clone(), &own(&[("/work/app", true), ("/work", false), ("/", false)]));
        assert_eq!((kept, notes.len()), (normal, 0));
        // Through the start: the Workaround is what Luna applied.
        let library = crate::library::SessionSkills {
            memory_kept: vec!["could not exclude E:/w/[ab]: its name collides with this project's instruction path E:/w/[[]ab[]]".into()],
            ..Default::default()
        };
        let applied = crate::claude::launch::settings_applied("/s.json", false, &library, "/d/core", None);
        let item = applied.iter().find(|a| a.key == "library.memory-not-narrowed").expect("said");
        assert_eq!(item.class, crate::applied::Class::Workaround);
        assert!(item.value.starts_with("could not exclude E:/w/[ab]"), "{}", item.value);
    }

    #[test]
    fn a_real_project_named_like_an_escape_keeps_its_instructions() {
        let root = std::env::temp_dir().join(format!("luna-collide-{}", std::process::id()));
        let (project, added) = (root.join("[[]ab[]]"), root.join("[ab]"));
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::create_dir_all(&added).unwrap();
        let account = root.join("acc");
        std::fs::create_dir_all(&account).unwrap();
        let settings = serde_json::json!({ "permissions": { "additionalDirectories": [added.to_string_lossy()] } });
        std::fs::write(project.join(".claude").join("settings.json"), settings.to_string()).unwrap();
        let (off, kept) = claude_memory_off(&[], &account.to_string_lossy(), &project.to_string_lossy(), None);
        assert!(off.is_empty(), "no pattern that is also this project's CLAUDE.md: {off:?}");
        assert!(!kept.is_empty() && kept.iter().all(|k| k.contains("collides with this project's instruction path")), "{kept:?}");
        // Another project beside them: [ab] is left out as usual.
        let other = root.join("plain");
        std::fs::create_dir_all(other.join(".claude")).unwrap();
        std::fs::write(other.join(".claude").join("settings.json"), settings.to_string()).unwrap();
        let (off, kept) = claude_memory_off(&[], &account.to_string_lossy(), &other.to_string_lossy(), None);
        // Four memory files for each spelling of [ab].
        assert!(!off.is_empty() && off.len() % 4 == 0 && kept.is_empty(), "{off:?} {kept:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_account_file_whose_pattern_reads_as_the_projects_is_read_after_all() {
        let root = std::env::temp_dir().join(format!("luna-acc-collide-{}", std::process::id()));
        // The account [ab]: its file's pattern <root>/[[]ab[]]/CLAUDE.md is
        // also, in picomatch's shortcut, the CLAUDE.md of a project [[]ab[]].
        let (project, account, plain) = (root.join("[[]ab[]]"), root.join("[ab]"), root.join("plain"));
        for d in [&project, &account, &plain] {
            std::fs::create_dir_all(d).unwrap();
        }
        let file = account.join("CLAUDE.md").to_string_lossy().replace('\\', "/");
        let acc = account.to_string_lossy().into_owned();
        let (kept, notes) = claude_account_excludes(vec![file.clone()], &[], &acc, &project.to_string_lossy(), None);
        assert!(kept.is_empty(), "the project's own instructions are not to be taken with it: {kept:?}");
        assert!(notes.len() == 1 && notes[0].contains("collides with this project's instruction path"), "{notes:?}");
        let (kept, notes) = claude_account_excludes(vec![file.clone()], &[], &acc, &plain.to_string_lossy(), None);
        assert_eq!((kept, notes.len()), (vec![file], 0), "elsewhere it is left out as asked");
        // Said as a Workaround of what Luna applied.
        let library = crate::library::SessionSkills { excluded_kept: notes_of(&project, &account), ..Default::default() };
        let applied = crate::claude::launch::settings_applied("/s.json", false, &library, "/d/core", None);
        let item = applied.iter().find(|a| a.key == "library.account-instructions-kept").expect("said");
        assert_eq!(item.class, crate::applied::Class::Workaround);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What `claude_account_excludes` says of the account's file in `project`.
    fn notes_of(project: &std::path::Path, account: &std::path::Path) -> Vec<String> {
        let file = account.join("CLAUDE.md").to_string_lossy().replace('\\', "/");
        claude_account_excludes(vec![file], &[], &account.to_string_lossy(), &project.to_string_lossy(), None).1
    }

    #[test]
    fn a_folder_above_the_project_by_dot_dot_is_beside_it_and_left_out() {
        let root = std::env::temp_dir().join(format!("luna-dotdot-{}", std::process::id()));
        let (project, shared) = (root.join("proj"), root.join("shared"));
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        let account = root.join("acc");
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(project.join(".claude").join("settings.json"), r#"{"permissions":{"additionalDirectories":["../shared"]}}"#)
            .unwrap();
        let (off, _) = claude_memory_off(&[], &account.to_string_lossy(), &project.to_string_lossy(), None);
        let shared = resolved_path(&shared.to_string_lossy()).replace('\\', "/");
        assert!(off.contains(&format!("{}/CLAUDE.md", crate::claude::launch::glob_literal(&shared))), "{off:?}");
        assert!(off.iter().all(|p| !p.contains("..")), "resolved, not `<project>/../shared`: {off:?}");
        // Nested in the project instead: read as ever.
        std::fs::write(project.join(".claude").join("settings.json"), r#"{"permissions":{"additionalDirectories":["./docs/../sub"]}}"#)
            .unwrap();
        assert!(claude_memory_off(&[], &account.to_string_lossy(), &project.to_string_lossy(), None).0.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_users_own_added_folders_are_read_from_their_settings() {
        let root = std::env::temp_dir().join(format!("luna-added-dirs-{}", std::process::id()));
        let (account, folder) = (root.join("acc"), root.join("proj"));
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("settings.json"), r#"{"permissions":{"additionalDirectories":["/shared/docs"]}}"#).unwrap();
        std::fs::write(folder.join(".claude").join("settings.local.json"), r#"{"permissions":{"additionalDirectories":["../sibling"]}}"#)
            .unwrap();
        let dirs = claude_settings_dirs(&account.to_string_lossy(), &folder.to_string_lossy());
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert!(dirs[0].ends_with("docs") && dirs[1].ends_with("sibling"), "{dirs:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project's settings file past the cap adds no folder, and on Unix a
    /// FIFO in the local file's place, with no writer, does not hold a start
    /// up: the account's file still answers.
    #[test]
    fn a_settings_file_too_large_or_no_file_adds_nothing_and_holds_no_start() {
        use crate::claude::defaults::tests::oversized;
        let root = std::env::temp_dir().join(format!("luna-added-dirs-bounded-{}", std::process::id()));
        let (account, folder) = (root.join("acc"), root.join("proj"));
        std::fs::create_dir_all(folder.join(".claude")).unwrap();
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("settings.json"), r#"{"permissions":{"additionalDirectories":["shared"]}}"#).unwrap();
        let big = oversized(r#"{"permissions":{"additionalDirectories":["big"]},"pad":"#);
        std::fs::write(folder.join(".claude").join("settings.json"), big).unwrap();
        #[cfg(unix)]
        crate::claude::defaults::tests::mkfifo(&folder.join(".claude").join("settings.local.json"));
        let (a, f) = (account.to_string_lossy().into_owned(), folder.to_string_lossy().into_owned());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(claude_settings_dirs(&a, &f));
        });
        let dirs = rx.recv_timeout(std::time::Duration::from_secs(10)).expect("answered without waiting on a FIFO");
        assert_eq!(dirs, vec![folder.join("shared").to_string_lossy().into_owned()]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
