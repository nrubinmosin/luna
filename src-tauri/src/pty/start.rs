//! Starting a chat's CLI in a pane: what each provider's start settles under
//! the chat's spawn gate (see `spawn`) — its MCP token, the settings, MCP
//! and instructions files it reads at start, its worktree, its place in the
//! agents' registry, what its sandbox and guard take from this machine —
//! before the arguments are laid out by `claude::launch` and `codex::launch`,
//! which only lay out what they are handed, and the process is run. A
//! helper's turn (background/command.rs) settles the same through the
//! functions here.

use super::same_dir;
use super::spawn::{spawn, Launch};
use crate::claude::launch::{self as claude_launch, claude_settings, with_role, HelperSettings, CLAUDE_MCP_TOKEN_ENV};
use crate::codex::launch::{self as codex_launch, AUTO_REVIEW, AUTO_REVIEW_SANDBOX, READ_ONLY_NET};
use crate::provider::Provider;
use std::sync::Mutex;

/// The settings a Claude Code session is started with, as its `--settings`
/// (one only: a second takes the first's place). `hooks`: each event posts
/// its stdin to Luna's loopback listener (hub.rs) and prints nothing — a
/// hook's stdout is added to the model's context, silence is free — while
/// the listener is on. `guard`: a PreToolUse hook on its shell tools that
/// refuses a command waiting forever (guard.rs), for a session with Luna's
/// tools or a helper. `library`: what the library's folders keep from the
/// session (library/folders.rs `SessionSkills`) — skills off in `skillOverrides`,
/// which settings of this source beat whatever the account's or the
/// project's say, and the account's instructions in `claudeMdExcludes`.
/// Always: Luna's `core/` denied to its file tools (`claude_read_deny`), so
/// there is always a file to write. Written to a file under the data dir
/// (`hooks/`, named for what it first held) rather than passed inline, so the
/// command line stays readable and quoting is not a concern. Err if it cannot
/// be written: a session carries the core-folder deny or does not start
/// (threat model F3), so the caller refuses the launch rather than run without
/// it.
pub(crate) fn claude_settings_file(
    chat_id: &str,
    hooks: bool,
    guard: bool,
    library: &crate::library::SessionSkills,
    helper: Option<HelperSettings>,
) -> Result<std::path::PathBuf, String> {
    let settings = claude_settings(
        crate::hub::hook_url(chat_id).filter(|_| hooks).as_deref(),
        crate::hub::guard_url(chat_id).filter(|_| guard).as_deref(),
        library,
        &crate::ipc::core_dir().to_string_lossy(),
        helper,
    );
    // Owner-only: the hook URLs in it carry the hub's secret.
    let dir = crate::paths::data_dir().join("hooks");
    crate::secret::private_dir(&dir).map_err(|e| format!("cannot make {}: {e}", dir.display()))?;
    let path = dir.join(format!("{chat_id}.json"));
    let json = serde_json::to_string_pretty(&settings).map_err(|e| format!("the settings do not serialize: {e}"))?;
    crate::secret::write_private(&path, json.as_bytes()).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

/// The instructions a folder of the library gives a Claude Code session in
/// place of its account's (library/folders.rs `SessionSkills::instructions`), as the
/// `CLAUDE.md` of a folder of its own (`hooks/<id>/`) for `--add-dir`, which
/// Claude Code reads with `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD` set
/// — those of every folder it is given then, Luna's own holding none.
/// Memory, not `--append-system-prompt-file`: a resume keeps the system
/// prompt its session started with, and reads memory again. Written at every
/// start; with nothing to give, gone, and None.
pub(crate) fn claude_instructions_dir(chat_id: &str, file: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    let dir = crate::paths::data_dir().join("hooks").join(chat_id);
    let Some(file) = file else {
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    };
    // `hooks/` made owner-only first, if this is what makes it: the settings
    // files beside this folder carry the hub's secret.
    let hooks = dir.parent().map_or(Ok(()), crate::secret::private_dir);
    let copied = hooks.and_then(|()| std::fs::create_dir_all(&dir)).and_then(|()| std::fs::copy(file, dir.join("CLAUDE.md")));
    match copied {
        Ok(_) => Some(dir),
        Err(e) => {
            crate::log::warn("library", &format!("could not give {chat_id} {}: {e}", file.display()));
            None
        }
    }
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
                // one up first. All eight are what the box was ticked for.
                "alwaysLoad": true
            }
        }
    });
    let dir = crate::paths::data_dir().join("mcp");
    crate::secret::private_dir(&dir).ok()?;
    let path = dir.join(format!("{chat_id}.json"));
    crate::secret::write_private(&path, serde_json::to_string_pretty(&config).ok()?.as_bytes()).ok()?;
    Some(path)
}

/// How long a Luna tool call may take, in ms, as both CLIs are told: `wait`
/// holds its request for up to 30 min, `spawn` for up to two.
pub(crate) const MCP_TOOL_TIMEOUT_MS: u64 = 1_900_000;

/// The guard (guard.rs) for a Codex session, as `-c` overrides: a PreToolUse
/// hook on its shell, which its hooks call `Bash`. Codex runs a hook given so
/// only with its trust bypassed (`HOOKS_UNASKED`): untrusted, `exec` drops it
/// and the TUI stops on a review screen. Without that, none, and a line in the
/// log says why.
pub(crate) fn codex_guard_args(chat_id: &str, guard: bool, hooks_unasked: bool) -> Vec<String> {
    if !guard {
        return Vec::new();
    }
    if !hooks_unasked {
        // Once per session: a helper's `codex exec` starts at every turn.
        static TOLD: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
        if TOLD.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()).insert(chat_id.to_string()) {
            crate::log::info(
                "guard",
                &format!(
                    "{}: no guard against endless waits, as Codex runs a hook Luna gives it only with hooks' trust bypassed (Settings, Codex hooks)",
                    crate::agents::label(chat_id)
                ),
            );
        }
        return Vec::new();
    }
    crate::hub::guard_url(chat_id).map_or_else(Vec::new, |url| vec!["-c".into(), codex_launch::guard_override(&url, &curl_program())])
}

/// curl by its full path where Windows keeps it, else by name: Codex runs a
/// hook with an environment of its own (codex/launch.rs `hook_command`).
pub(crate) fn curl_program() -> String {
    #[cfg(windows)]
    if let Some(root) = std::env::var("SystemRoot").ok().filter(|r| !r.is_empty() && !r.contains([' ', '\'', '"'])) {
        let full = format!(r"{root}\System32\curl.exe");
        if std::path::Path::new(&full).is_file() {
            return full;
        }
    }
    if cfg!(windows) { "curl.exe" } else { "curl" }.to_string()
}

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

/// PATH for a Codex session in that sandbox, where it has a Windows one: this
/// process's, without the Store's app aliases (codex/launch.rs
/// `without_windows_apps`).
pub(crate) fn codex_sandbox_path(sandbox: &str) -> Option<String> {
    codex_launch::codex_windows_sandbox(sandbox)?;
    let path = std::env::var("PATH").ok()?;
    Some(codex_launch::without_windows_apps(&path))
}

/// Luna's MCP server as Codex's `-c` overrides (codex/launch.rs `mcp_args`),
/// while it is up.
pub(crate) fn codex_mcp_args() -> Vec<String> {
    crate::hub::mcp_url().map_or_else(Vec::new, |url| codex_launch::mcp_args(&url))
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

/// What a helper is told it is: instructions written for the chat a person
/// talks to (say, to agree a plan with them first) would otherwise hold it up
/// with nobody there to answer. Its language too: told to speak the user's
/// language to them, a helper went on in it to the agent that asked in English,
/// progress notes and all.
/// Instructions files name the person in their own words — the operator, the
/// user, the person at this machine — and a helper told only that it is "not
/// a chat with a person" went on following the rules written for that person's
/// chat: a helper on 2026-11-08 wrote its progress notes in the language its
/// account's instructions set for the operator. So the note names those words.
pub(crate) fn helper_line(parent: &str) -> String {
    format!(
        "You are a helper session started through Luna by session {parent}, not a chat with a person: your answers go to \
         that session. You are not the chat of whoever your instructions call the operator, the user or the person at \
         this machine, and what they set for that chat (its language, its reply format, questions to that person) does \
         not apply to you. Answer your caller in the language and form its task asks for, progress notes included; \
         where it says none, in the language the task is written in."
    )
}

/// A helper's opening prompt, with `helper_line` as a note after it: every turn
/// of a Codex helper, and the first of a Claude Code one, whose system prompt
/// says it too. Codex can only replace `developer_instructions`, not add to them,
/// and what they come to depends on its whole stack of config files: working that
/// out here to pass it on would be Codex's resolution done twice, and a long one
/// would not fit on a command line. Claude Code reads its instructions files as
/// memory that outweighs an appended system prompt; in the task itself the note
/// stands beside what it is about. After the prompt, not before: the opening
/// words are what names the chat, and a slash command has to come first. One
/// that does goes without the note, which would land in its arguments.
pub(crate) fn helper_prompt(parent: &str, prompt: &str) -> String {
    if prompt.trim_start().starts_with('/') {
        return prompt.to_string();
    }
    format!("{prompt}{HELPER_NOTE}{}]", helper_line(parent))
}

/// Where the note starts: what comes before it is the prompt as it was asked.
const HELPER_NOTE: &str = "\n\n[A note from Luna, not part of the task: ";

/// A prompt as it was asked, with no note on it: only a note Luna wrote, whole
/// and at the very end (`helper_prompt`), comes off. A task that quotes the
/// note's opening words, or is followed by anything after a note, is left as
/// it is: what comes after them is the task's own.
pub(crate) fn without_note(text: &str) -> &str {
    let end = text.trim_end();
    let Some(at) = end.rfind(HELPER_NOTE) else { return text };
    let note = end[at + HELPER_NOTE.len()..].strip_suffix(']').unwrap_or_default();
    // The note as this Luna writes it, or as the one before it did: a helper's
    // conversation goes on across an update.
    let lines = [helper_line("\u{0}"), PREVIOUS_HELPER_LINE.replace("{parent}", "\u{0}")];
    let ours = lines.iter().filter_map(|line| line.split_once('\u{0}')).any(|(head, tail)| {
        // A session id: no spaces, no brackets, nothing that is prose.
        note.strip_prefix(head)
            .and_then(|rest| rest.strip_suffix(tail))
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')))
    });
    if ours {
        &text[..at]
    } else {
        text
    }
}

/// `helper_line` as Luna 0.10 wrote it, still in conversations it began.
const PREVIOUS_HELPER_LINE: &str = "You are a helper session started through Luna by session {parent}, not a chat with a person: \
your answers go to that session. Instructions meant only for the session a person talks to do not apply to you. Write in the \
language of the task you were given, progress notes included.";

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn ensure_claude_session(
    id: String,
    folder: String,
    account_path: String,
    model: String,
    effort: String,
    permission_mode: String,
    worktree: bool,
    // With `worktree`: an existing one to go into by name, which Claude Code
    // makes again, same name and branch, if it has been removed meanwhile.
    worktree_name: Option<String>,
    resume: Option<String>,
    // Luna's MCP tools (agents/): off unless the chat asked.
    tools: Option<bool>,
    // The chat that spawned this one, if an agent did.
    parent: Option<String>,
    // An opening prompt, for a chat an agent starts.
    prompt: Option<String>,
    // Without the edit tools (`CLAUDE_EDIT_TOOLS`).
    read_only: Option<bool>,
    // The agent it runs as: `--agent`.
    agent: Option<String>,
    // The spawn this is the session of, if an agent's (agents/spawn/mod.rs `spawn_is`).
    spawn_request: Option<u64>,
) -> Result<String, String> {
    // Built only if no process is running (see `spawn`).
    spawn(id.clone(), move || {
        let parent = crate::agents::parent_on_start(&id, parent);
        // `--resume` of a session with no transcript fails with "No
        // conversation found" and leaves the pane at a dead CLI: a chat that
        // was opened and never typed into starts afresh instead.
        let resume = resume.filter(|sid| crate::claude::session::find_transcript(&account_path, sid).is_some());
        let asked_nothing = spawn_request.is_none() && prompt.is_none();
        // A conversation a core went in the middle of and the window never heard of.
        let resume = resume
            .or_else(|| asked_nothing.then(|| crate::agents::conversation_of(&id, Provider::Claude, &folder, &account_path)).flatten());
        // A helper's is named by Luna (`agents::helper_worktree_name`), so that this
        // start knows which is its own: in a conversation that one, afresh that one if
        // still as made, else another.
        let worktree_name = match worktree_name {
            Some(name) => Some(name),
            None if !worktree => None,
            None if spawn_request.is_some() => Some(crate::agents::helper_worktree_name(&id)),
            None => crate::agents::worktree_for_start(&id, &folder, &account_path, resume.is_some()),
        };
        // A new worktree is about to branch off origin; a resumed session
        // already has its worktree, and this one its name. Here, under the spawn gate,
        // so a pane asking for the session meanwhile waits for this spawn — the one
        // with the opening prompt — instead of starting its own without it.
        let there = worktree_name.as_deref().is_some_and(|name| std::path::Path::new(&crate::worktree::named_path(&folder, name)).exists());
        if worktree && resume.is_none() && !there {
            crate::worktree::fetch_origin(&folder);
        }
        // The skills the library gives sessions in this folder and keeps from
        // them, at every start (library/folders.rs `session_args`); not to a login
        // session.
        let login = same_dir(&folder, &account_path);
        let skills = if login {
            crate::library::SessionSkills::default()
        } else {
            crate::library::lay_out("before a session starts");
            crate::library::session_args(Provider::Claude, &account_path, &folder)
        };
        // Rewritten at every spawn: the listener's port is new with every Luna run.
        // The guard against endless waits (guard.rs): a chat with Luna's tools
        // and a helper, which only such a chat starts.
        let guard = tools.unwrap_or(false) || parent.is_some();
        // A helper, the window's or not, starts with none of its account's
        // reach out of Luna but what it was spawned with (`HelperSettings`).
        let helper = parent.is_some().then(|| HelperSettings { connectors: crate::agents::keeps_connectors(&id, spawn_request) });
        // Mandatory: it carries the core-folder deny (threat model F3). If it
        // cannot be written, the session does not start rather than run
        // without it.
        let settings_file = claude_settings_file(&id, true, guard, &skills, helper)?;
        let tools = tools.unwrap_or(false);
        let read_only = read_only.unwrap_or(false);
        let agent = agent.filter(|a| !a.is_empty());
        let settings = with_role(
            serde_json::json!({
                "model": model, "effort": effort, "permissionMode": permission_mode, "worktree": worktree,
            }),
            read_only,
            agent.as_deref(),
        );
        // A login session runs in the account folder itself (the providers'
        // `loginSession`): not a chat, so not one agents may see or write to.
        let token = if login {
            None
        } else {
            crate::agents::register(&id, Provider::Claude, &folder, &account_path, parent.as_deref(), tools, settings)
        };
        match spawn_request {
            Some(request) => crate::agents::spawn_is(request, &id)?,
            None if prompt.is_none() => crate::agents::chat_starting(&id, resume.is_some()),
            None => {}
        }
        let mcp = token.and_then(|t| Some((claude_mcp_file(&id)?, t)));
        let notes = (parent.is_some() || tools).then(crate::agents::notes_dir).filter(|notes| std::fs::create_dir_all(notes).is_ok());
        let instructions = if login { None } else { claude_instructions_dir(&id, skills.instructions.as_deref()) };
        let (args, envs) = claude_launch::chat_args(&claude_launch::ChatStart {
            id: &id,
            account_path: &account_path,
            model: &model,
            effort: &effort,
            permission_mode: &permission_mode,
            resume: resume.as_deref(),
            worktree: worktree.then_some(worktree_name.as_deref()),
            settings_file: Some(&settings_file),
            agent: agent.as_deref(),
            read_only,
            parent: parent.as_deref(),
            strict_mcp: helper.is_some_and(|h| !h.connectors),
            mcp: mcp.as_ref().map(|(file, token)| (file.as_path(), token.as_str())),
            notes_dir: notes.as_deref(),
            instructions_dir: instructions.as_deref(),
            skill_args: &skills.args,
            prompt: prompt.as_deref(),
        });
        Ok(Launch {
            provider: Provider::Claude,
            cwd: folder,
            account_path,
            args,
            envs,
            resume,
            describe: format!("model {model} perm {permission_mode} worktree {worktree}"),
        })
    })
    .await
}

/// A Codex session. `folder` is where it runs: the project, which `worktree`
/// has Codex leave for a checkout of its own (`--worktree`), or a worktree it
/// made earlier, to resume in. `login` runs `codex login` instead of a chat.
/// `hooks_unasked` runs the project's hooks without their trust prompt
/// (codex/hooks.rs).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
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
    hooks_unasked: Option<bool>,
    parent: Option<String>,
    prompt: Option<String>,
    spawn_request: Option<u64>,
) -> Result<String, String> {
    // Built only if no process is running (see `spawn`).
    spawn(id.clone(), move || {
        let parent = crate::agents::parent_on_start(&id, parent);
        let asked_nothing = !login && spawn_request.is_none() && prompt.is_none();
        // A conversation a core went in the middle of and the window never heard of,
        // resumed where it works, as Codex refuses `--worktree` on a resume.
        let recovered = if asked_nothing && resume.is_none() {
            crate::agents::conversation_of(&id, Provider::Codex, &folder, &account_path)
        } else {
            None
        };
        let folder = recovered.as_deref().and_then(|thread| crate::codex::session::thread_cwd(&account_path, thread)).unwrap_or(folder);
        let resume = resume.or(recovered);
        // Afresh, it makes a new worktree: Codex takes no name to go back into one
        // by. What it may have begun is told of, ahead of what this one makes.
        if asked_nothing && resume.is_none() && worktree.unwrap_or(false) {
            crate::agents::report_codex_worktrees(&id, &folder, &account_path);
        }
        if login {
            let envs = vec![("CODEX_HOME", account_path.clone())];
            let args = vec!["login".to_string()];
            return Ok(Launch { provider: Provider::Codex, cwd: folder, account_path, args, envs, resume: None, describe: "login".into() });
        }
        // A project under a folder trusted on this machine starts trusted
        // without an entry in the account's config, and so does one
        // trusted while the account was in use (codex/trust.rs).
        let trust = crate::codex::trust::root_override(&folder).or_else(|| crate::codex::trust::session_override(&account_path, &folder));
        let chosen_model = model.as_deref().map(str::trim).filter(|m| !m.is_empty());
        let sandbox: &str = if approval == AUTO_REVIEW { AUTO_REVIEW_SANDBOX } else { &sandbox };
        let hooks_unasked = hooks_unasked.unwrap_or(false);
        // What the sandbox and the guard take from this machine, ahead of the
        // registration: the profile's new name and what it denies, PATH and
        // git's entries (each nothing in a sandbox that takes none), and the
        // guard's hook — or the log's word that Codex would drop it.
        let profile = codex_profile(sandbox);
        let deny = codex_core_deny(sandbox);
        let sandbox_path = codex_sandbox_path(sandbox);
        let git_envs = codex_git_envs(sandbox);
        let guard = codex_guard_args(&id, tools.unwrap_or(false) || parent.is_some(), hooks_unasked);
        let tools = tools.unwrap_or(false);
        let settings = serde_json::json!({
            "model": chosen_model,
            "effort": effort, "approval": approval, "sandbox": sandbox, "hooksUnasked": hooks_unasked,
        });
        let token = crate::agents::register(&id, Provider::Codex, &folder, &account_path, parent.as_deref(), tools, settings);
        match spawn_request {
            Some(request) => crate::agents::spawn_is(request, &id)?,
            None if prompt.is_none() => crate::agents::chat_starting(&id, resume.is_some()),
            None => {}
        }
        let mcp = token.and_then(|t| Some((crate::hub::mcp_url()?, t)));
        // A helper starts without its account's MCP servers and apps, as a
        // Claude Code one does, unless spawned with them (`connectors_off`).
        let connectors_off = if parent.is_some() && !crate::agents::keeps_connectors(&id, spawn_request) {
            codex_launch::connectors_off(&crate::codex::config::helper_mcp_servers(&account_path)?, mcp.is_some())?
        } else {
            Vec::new()
        };
        // The skills the library gives sessions in this folder and keeps
        // from them, at every start (library/folders.rs `session_args`).
        crate::library::lay_out("before a session starts");
        let skills = crate::library::session_args(Provider::Codex, &account_path, &folder);
        let (args, envs) = codex_launch::chat_args(&codex_launch::ChatStart {
            id: &id,
            folder: &folder,
            account_path: &account_path,
            resume: resume.as_deref(),
            trust,
            worktree: worktree.unwrap_or(false),
            model: chosen_model,
            effort: &effort,
            approval: &approval,
            sandbox,
            profile: profile.as_deref(),
            deny: deny.as_deref(),
            sandbox_path: sandbox_path.as_deref(),
            git_envs: &git_envs,
            hooks_unasked,
            guard: &guard,
            connectors_off: &connectors_off,
            parent: parent.as_deref(),
            mcp: mcp.as_ref().map(|(url, token)| (url.as_str(), token.as_str())),
            skill_args: &skills.args,
            prompt: prompt.as_deref(),
        });
        let describe = format!("model {} effort {effort} approval {approval} sandbox {sandbox}", model.as_deref().unwrap_or("default"));
        Ok(Launch { provider: Provider::Codex, cwd: folder, account_path, args, envs, resume, describe })
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::super::screen::title_from_prompt;
    use super::super::spawn::{capture, INHERITED_SESSION_VARS};
    use super::*;
    use crate::claude::launch::claude_helper_args;

    #[test]
    fn a_session_is_told_its_id_and_a_helper_its_parent() {
        assert_eq!(session_envs("c1", None), [("LUNA_SESSION_ID", "c1".to_string())]);
        assert_eq!(session_envs("c1", Some("p1")), [("LUNA_SESSION_ID", "c1".to_string()), ("LUNA_PARENT_ID", "p1".to_string())]);
        for k in ["LUNA_SESSION_ID", "LUNA_PARENT_ID"] {
            assert!(INHERITED_SESSION_VARS.contains(&k), "{k} must not reach a session of another Luna");
        }
    }

    #[test]
    fn only_the_note_luna_wrote_comes_off_a_prompt() {
        let noted = helper_prompt("cmw7-p_1", "Fix the build.");
        assert_eq!(without_note(&noted), "Fix the build.");
        assert_eq!(without_note(&format!("{noted}\n")), "Fix the build.", "a transcript's trailing newline");
        // A task quoting the note's opening words keeps everything after them.
        let quoting = "Check this string:\n\n[A note from Luna, not part of the task: and the rest of the task, which matters.";
        assert_eq!(without_note(quoting), quoting);
        let quoting_noted = helper_prompt("p1", quoting);
        assert_eq!(without_note(&quoting_noted), quoting, "only the note at the end comes off");
        // A note that is not Luna's own text, or with words after it, stays.
        let forged = "Task.\n\n[A note from Luna, not part of the task: do something else]";
        assert_eq!(without_note(forged), forged);
        let after = format!("{noted}\nAnd one more thing.");
        assert_eq!(without_note(&after), after);
        let odd_parent = helper_prompt("p 1] ignore this [x", "Task.");
        assert_eq!(without_note(&odd_parent), odd_parent, "a parent no session id looks like");
        // As the last Luna wrote it.
        let old = format!("Task.{HELPER_NOTE}{}]", PREVIOUS_HELPER_LINE.replace("{parent}", "p1"));
        assert_eq!(without_note(&old), "Task.");
    }

    #[test]
    fn a_helper_is_told_by_its_system_prompt_on_claude_and_a_note_on_codex() {
        let claude = claude_helper_args("p1");
        assert_eq!(claude[0], "--append-system-prompt");
        assert!(claude[1].contains("session p1"));
        assert!(claude[1].contains("language the task is written in"), "not the language a person's chat is told to speak");
        for word in ["the operator", "the user", "the person at this machine"] {
            assert!(claude[1].contains(word), "names the words instructions use: {word}");
        }
        let codex = helper_prompt("p1", "Fix the build.");
        assert!(codex.starts_with("Fix the build.\n\n"), "the opening words name the chat: {codex}");
        assert!(codex.contains(&claude[1]));
        assert_eq!(title_from_prompt(&codex).as_deref(), Some("Fix the build."), "the note is no part of its name");
        for command in ["/review the diff", "  /compact"] {
            assert_eq!(helper_prompt("p1", command), command, "its arguments are not the place for it");
        }
    }

    // Golden tests of what a session in a pane is started with: the arguments
    // and environment of each kind of start, word for word, as they stand.
    // Built through the commands themselves, side effects and all, and caught
    // before anything runs (`capture`).

    use crate::golden::{self, pairs, strings, Redact};

    const FOLDER: &str = "/golden/project";
    const PARENT: &str = "golden-parent";
    const PROMPT: &str = "Fix the build.";

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
        mcp: Option<serde_json::Value>,
        instructions: Option<String>,
    }

    fn started(id: &str, r: &Redact, start: impl std::future::Future<Output = Result<String, String>>) -> Started {
        capture::expect(id);
        let built = tauri::async_runtime::block_on(start);
        let launch = capture::take(id);
        // The environment read while the session is still registered: its
        // token has to be the one Luna knows it by.
        let envs = launch.as_ref().map(|l| r.envs(&l.envs));
        crate::agents::forget(id);
        assert_eq!(built, Ok(String::new()));
        let (l, envs) = (launch.expect("the start was caught"), envs.unwrap());
        let args = r.args(&l.args);
        let data = crate::paths::data_dir();
        let read = |sub: &str, placeholder: &str| {
            let path = data.join(sub).join(format!("{id}.json"));
            let v = args.iter().any(|a| a == placeholder).then(|| r.file(&path.to_string_lossy()));
            let _ = std::fs::remove_file(&path);
            v
        };
        let (settings, mcp) = (read("hooks", "<settings-file>"), read("mcp", "<mcp-file>"));
        let instructions = golden::instructions(id, &args);
        Started { envs, cwd: r.text(&l.cwd), resume: l.resume, describe: l.describe, settings, mcp, instructions, args }
    }

    /// The window's ask for a Claude Code chat; each case changes what it is about.
    struct ClaudeAsk {
        folder: String,
        account: String,
        model: &'static str,
        effort: &'static str,
        mode: &'static str,
        worktree: bool,
        worktree_name: Option<&'static str>,
        resume: Option<String>,
        tools: Option<bool>,
        parent: Option<&'static str>,
        prompt: Option<&'static str>,
        read_only: Option<bool>,
        agent: Option<&'static str>,
    }

    impl ClaudeAsk {
        fn chat() -> Self {
            ClaudeAsk {
                folder: FOLDER.into(),
                account: "/golden/acc-claude".into(),
                model: "opus",
                effort: "high",
                mode: "default",
                worktree: false,
                worktree_name: None,
                resume: None,
                tools: None,
                parent: None,
                prompt: None,
                read_only: None,
                agent: None,
            }
        }

        fn start(self, id: &str, r: &Redact) -> Started {
            let s = |v: Option<&str>| v.map(str::to_owned);
            started(
                id,
                r,
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
                    s(self.parent),
                    s(self.prompt),
                    self.read_only,
                    s(self.agent),
                    None,
                ),
            )
        }
    }

    fn claude_envs(id: &str, account: &str) -> Vec<(String, String)> {
        pairs(&[("CLAUDE_CONFIG_DIR", account), ("DISABLE_AUTOUPDATER", "1"), ("LUNA_SESSION_ID", id)])
    }

    #[test]
    fn golden_claude_chat_starts_with_its_flags_and_hooks() {
        const ID: &str = "golden-claude-chat";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk::chat().start(ID, &r);
        assert_eq!(
            s.args,
            strings(&["--model", "opus", "--effort", "high", "--permission-mode", "default", "--settings", "<settings-file>"])
        );
        assert_eq!(s.envs, claude_envs(ID, "/golden/acc-claude"));
        assert_eq!(s.settings, Some(golden::claude_chat_settings(false)));
        assert_eq!(s.mcp, None);
        assert_eq!((s.cwd.as_str(), s.resume), (FOLDER, None));
        assert_eq!(s.describe, "model opus perm default worktree false");
    }

    #[test]
    fn golden_claude_permission_modes_pass_through_as_given() {
        for mode in ["acceptEdits", "plan", "bypassPermissions", "auto"] {
            let id = format!("golden-claude-mode-{mode}");
            let r = Redact::for_chat(&id);
            let s = ClaudeAsk { mode, ..ClaudeAsk::chat() }.start(&id, &r);
            assert_eq!(
                s.args,
                strings(&["--model", "opus", "--effort", "high", "--permission-mode", mode, "--settings", "<settings-file>"])
            );
        }
    }

    #[test]
    fn golden_claude_resumes_a_conversation_that_has_a_transcript_and_only_then() {
        const ID: &str = "golden-claude-resume";
        const SID: &str = "0a1b2c3d-0000-4000-8000-00000000c1a0";
        let account = std::env::temp_dir().join(format!("luna-golden-resume-{}", std::process::id()));
        let projects = account.join("projects").join("golden");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::write(projects.join(format!("{SID}.jsonl")), "{}\n").unwrap();
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
                "<settings-file>"
            ])
        );
        assert_eq!(s.envs, claude_envs(ID, "<acc>"));
        assert_eq!(s.resume.as_deref(), Some(SID));

        // No transcript: `--resume` would fail, and it starts afresh.
        let s = ClaudeAsk { account: acc, resume: Some("0a1b2c3d-0000-4000-8000-00000000dead".into()), ..ClaudeAsk::chat() }.start(ID, &r);
        assert_eq!(
            s.args,
            strings(&["--model", "opus", "--effort", "high", "--permission-mode", "default", "--settings", "<settings-file>"])
        );
        assert_eq!(s.resume, None);
        let _ = std::fs::remove_dir_all(&account);
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
                "<settings-file>"
            ])
        );
        assert_eq!(s.describe, "model opus perm default worktree true");
        assert_eq!(s.cwd, FOLDER, "it starts in the project and moves itself");
        // No name of Luna's: the CLI names it.
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
                "<settings-file>"
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
                "<settings-file>",
                "--add-dir",
                "<notes-dir>",
            ])
        );
        assert_eq!(s.envs, claude_envs(ID, ACC), "no token: not a chat agents may reach");
        assert_eq!(s.settings, Some(golden::claude_chat_settings(true)));
        assert_eq!(s.mcp, None);
        assert_eq!(s.cwd, ACC);
    }

    #[test]
    fn golden_claude_helper_with_tools_gets_the_guard_luna_mcp_and_its_role() {
        const ID: &str = "golden-claude-helper";
        let r = Redact::for_chat(ID);
        let s = ClaudeAsk {
            model: "sonnet",
            effort: "medium",
            mode: "bypassPermissions",
            tools: Some(true),
            parent: Some(PARENT),
            prompt: Some(PROMPT),
            read_only: Some(true),
            agent: Some("reviewer"),
            ..ClaudeAsk::chat()
        }
        .start(ID, &r);
        let line = golden::helper_line(PARENT);
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
                "--append-system-prompt",
                &line,
                "--strict-mcp-config",
                "--allowedTools",
                "mcp__luna",
                "--mcp-config",
                "<mcp-file>",
                "--add-dir",
                "<notes-dir>",
                "--",
                PROMPT,
            ])
        );
        assert_eq!(
            s.envs,
            pairs(&[
                ("CLAUDE_CONFIG_DIR", "/golden/acc-claude"),
                ("DISABLE_AUTOUPDATER", "1"),
                ("LUNA_MCP_TOKEN", "<token>"),
                ("LUNA_SESSION_ID", ID),
                ("LUNA_PARENT_ID", PARENT),
            ])
        );
        assert_eq!(s.settings, Some(golden::claude_helper_settings(golden::claude_chat_settings(true))));
        assert_eq!(s.mcp, Some(golden::claude_mcp_config()), "the token stays in the environment");
        assert_eq!(s.describe, "model sonnet perm bypassPermissions worktree false");
    }

    /// The placeholders for a start in a library's folder on `account`.
    fn library_redact(id: &str, lib: &crate::library::GoldenLibrary, account: &str) -> Redact {
        Redact::for_chat(id)
            .with(lib.claude_skills(), "<library-dir>")
            .with(lib.folder.clone(), "<project>")
            .with(account, "<acc>")
            // As `claudeMdExcludes` names a file, on every OS.
            .with(account.replace('\\', "/"), "<acc>")
    }

    #[test]
    fn golden_claude_chat_in_a_library_folder_gets_its_skills_and_instructions() {
        const ID: &str = "golden-claude-library";
        let lib = crate::library::GoldenLibrary::new("pane-claude");
        let r = library_redact(ID, &lib, &lib.claude());
        let s = ClaudeAsk { folder: lib.folder.clone(), account: lib.claude(), prompt: Some(PROMPT), ..ClaudeAsk::chat() }.start(ID, &r);
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
                "--add-dir",
                "<instructions-dir>",
                "--add-dir",
                "<library-dir>",
                "--",
                PROMPT,
            ])
        );
        assert_eq!(
            s.envs,
            pairs(&[
                ("CLAUDE_CONFIG_DIR", "<acc>"),
                ("DISABLE_AUTOUPDATER", "1"),
                ("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1"),
                ("LUNA_SESSION_ID", ID),
            ])
        );
        let mut settings = golden::claude_chat_settings(false);
        settings["skillOverrides"] = serde_json::json!({ "review": "off" });
        settings["claudeMdExcludes"] = serde_json::json!(["<acc>/CLAUDE.md"]);
        assert_eq!(s.settings, Some(settings), "the folder keeps review and the account's instructions from it");
        assert_eq!(s.instructions.as_deref(), Some(crate::library::GoldenLibrary::INSTRUCTIONS), "the library's, word for word");
        assert_eq!(s.cwd, "<project>");
    }

    /// The window's ask for a Codex chat; each case changes what it is about.
    struct CodexAsk {
        folder: String,
        account: String,
        model: Option<&'static str>,
        effort: &'static str,
        approval: &'static str,
        sandbox: &'static str,
        worktree: Option<bool>,
        resume: Option<&'static str>,
        login: bool,
        tools: Option<bool>,
        hooks_unasked: Option<bool>,
        parent: Option<&'static str>,
        prompt: Option<&'static str>,
    }

    const CODEX_ACC: &str = "/golden/acc-codex";

    impl CodexAsk {
        fn chat() -> Self {
            CodexAsk {
                folder: FOLDER.into(),
                account: CODEX_ACC.into(),
                model: Some("gpt-5.5"),
                effort: "medium",
                approval: "on-request",
                sandbox: "workspace-write",
                worktree: None,
                resume: None,
                login: false,
                tools: None,
                hooks_unasked: None,
                parent: None,
                prompt: None,
            }
        }

        fn start(self, id: &str, r: &Redact) -> Started {
            let s = |v: Option<&str>| v.map(str::to_owned);
            started(
                id,
                r,
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
                    self.hooks_unasked,
                    s(self.parent),
                    s(self.prompt),
                    None,
                ),
            )
        }
    }

    /// A Codex chat's arguments in FOLDER: `head` before `-C`, its own flags,
    /// the title, then `tail`.
    fn codex_args(head: &[&str], flags: Vec<String>, tail: Vec<String>) -> Vec<String> {
        let mut args = strings(head);
        args.extend(strings(&["-C", FOLDER, "--no-daemon"]));
        args.extend(flags);
        args.extend(strings(&["-c", "tui.terminal_title=[\"thread-id\"]"]));
        args.extend(tail);
        args
    }

    /// The model and effort a chat asks for, then `--ask-for-approval` with a
    /// person answering, and the sandbox as a flag.
    fn codex_flags(model: Option<&str>, effort: &str, approval: &str, sandbox: &str) -> Vec<String> {
        let mut args = model.map_or_else(Vec::new, |m| strings(&["--model", m]));
        args.extend(strings(&["-c", &format!("model_reasoning_effort=\"{effort}\"")]));
        args.extend(codex_asks(approval, sandbox));
        args
    }

    fn codex_asks(approval: &str, sandbox: &str) -> Vec<String> {
        let mut args = strings(&["--ask-for-approval", approval, "-c", "approvals_reviewer=\"user\""]);
        if sandbox == "read-only" {
            args.extend(golden::codex_read_only(&["--sandbox", sandbox]));
        } else {
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
        envs
    }

    #[test]
    fn golden_codex_chat_starts_in_its_folder_with_a_person_answering() {
        const ID: &str = "golden-codex-chat";
        let r = Redact::for_chat(ID);
        let s = CodexAsk::chat().start(ID, &r);
        assert_eq!(s.args, codex_args(&[], codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write"), Vec::new()));
        assert_eq!(s.envs, codex_envs(ID, true));
        assert_eq!((s.cwd.as_str(), s.resume), (FOLDER, None));
        assert_eq!(s.describe, "model gpt-5.5 effort medium approval on-request sandbox workspace-write");
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
        let s = CodexAsk { login: true, resume: Some("019a0000-0000-7000-8000-00000000dead"), tools: Some(true), ..CodexAsk::chat() }
            .start(ID, &r);
        assert_eq!(s.args, ["login"]);
        assert_eq!(s.envs, pairs(&[("CODEX_HOME", CODEX_ACC)]));
        assert_eq!((s.resume, s.describe.as_str()), (None, "login"));
    }

    #[test]
    fn golden_codex_helper_with_tools_gets_the_guard_and_luna_mcp() {
        const ID: &str = "golden-codex-helper";
        let r = Redact::for_chat(ID);
        let s = CodexAsk {
            model: None,
            effort: "high",
            approval: "never",
            tools: Some(true),
            hooks_unasked: Some(true),
            parent: Some(PARENT),
            prompt: Some(PROMPT),
            ..CodexAsk::chat()
        }
        .start(ID, &r);
        let mut flags = codex_flags(None, "high", "never", "workspace-write");
        flags.extend(strings(&["--dangerously-bypass-hook-trust", "-c", &golden::codex_guard()]));
        // Its account's apps off (the golden account names no server).
        flags.extend(strings(&["-c", "features.apps=false"]));
        let mut tail = golden::codex_mcp();
        tail.extend(strings(&["--", PROMPT]));
        assert_eq!(s.args, codex_args(&[], flags, tail));
        let mut envs = codex_envs(ID, true);
        envs.extend(pairs(&[("LUNA_PARENT_ID", PARENT), ("LUNA_MCP_TOKEN", "<token>")]));
        assert_eq!(s.envs, envs);
        assert_eq!(s.describe, "model default effort high approval never sandbox workspace-write");

        // Hooks' trust asked for: Codex would drop the guard, and it is given none.
        let s = CodexAsk { tools: Some(true), ..CodexAsk::chat() }.start(ID, &r);
        assert_eq!(s.args, codex_args(&[], codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write"), golden::codex_mcp()));
    }

    #[test]
    fn golden_codex_approval_and_sandbox_pairs() {
        let with_windows = |mut a: Vec<String>| {
            a.extend(golden::codex_windows_sandbox());
            a
        };
        let mut read_only_net = strings(&["--ask-for-approval", "on-request", "-c", "approvals_reviewer=\"user\""]);
        read_only_net.extend(golden::codex_read_only_net());
        // (approval, sandbox, the flags for them, PATH cut for the sandbox, the sandbox it runs in)
        let cases: [(&str, &str, Vec<String>, bool, &str); 5] = [
            ("never", "danger-full-access", strings(&["--dangerously-bypass-approvals-and-sandbox"]), false, "danger-full-access"),
            ("on-request", "danger-full-access", codex_asks("on-request", "danger-full-access"), false, "danger-full-access"),
            ("untrusted", "read-only", codex_asks("untrusted", "read-only"), true, "read-only"),
            ("on-request", "read-only-net", with_windows(read_only_net), true, "read-only-net"),
            // The reviewer model takes the sandbox it runs in, whatever was asked.
            ("auto-review", "read-only", with_windows(strings(&["--approve-for-me"])), true, "workspace-write"),
        ];
        for (approval, sandbox, asks, sandboxed, runs_in) in cases {
            let id = format!("golden-codex-{approval}-{sandbox}");
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
            assert_eq!(s.describe, format!("model gpt-5.5 effort medium approval {approval} sandbox {runs_in}"));
        }
    }

    #[test]
    fn golden_codex_chat_in_a_library_folder_gets_its_plugin_and_keeps_its_skills_off() {
        const ID: &str = "golden-codex-library";
        let lib = crate::library::GoldenLibrary::new("pane-codex");
        let r = library_redact(ID, &lib, &lib.codex());
        let s = CodexAsk { folder: lib.folder.clone(), account: lib.codex(), prompt: Some(PROMPT), ..CodexAsk::chat() }.start(ID, &r);
        let mut want = strings(&["-C", "<project>", "--no-daemon"]);
        want.extend(codex_flags(Some("gpt-5.5"), "medium", "on-request", "workspace-write"));
        want.extend(strings(&[
            "-c",
            "tui.terminal_title=[\"thread-id\"]",
            "-c",
            &format!("plugins.{}@luna.enabled=true", crate::library::GoldenLibrary::SLUG),
            "-c",
            "skills.config=[{name=\"review\",enabled=false}]",
            "--",
            PROMPT,
        ]));
        assert_eq!(s.args, want);
        let mut envs = pairs(&[("CODEX_HOME", "<acc>")]);
        envs.extend(golden::codex_sandbox_path());
        envs.extend(pairs(&[("LUNA_SESSION_ID", ID)]));
        assert_eq!(s.envs, envs);
        assert_eq!(s.instructions, None, "Codex has no way to take other instructions");
        assert_eq!(s.cwd, "<project>");
    }
}
