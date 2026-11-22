//! What a helper's turn runs: the CLI's arguments, folder and environment, and the
//! lines its stdin is given.

use super::*;
use crate::applied::{Applied, Item};

/// What `command_for` gives a turn: its CLI's arguments, the folder it runs
/// in, its environment, and what Luna applied in them (applied.rs).
pub(super) type Command = (Vec<String>, String, Vec<(&'static str, String)>, Vec<Applied>);

/// How long `claude -p`, its stdin closed, waits for what its turn left in the
/// background before it stops it, in ms (`0`: as long as that takes).
const BG_WAIT_CEILING_ENV: &str = "CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS";

/// A helper's: 3 hours, not the CLI's 10 min nor no end. Subagents and test
/// gates a turn put in the background legitimately run for an hour and more,
/// but a watcher left behind (a dev server, a `tail -f`, a Monitor with no
/// end) keeps the CLI up, and with it the helper's turn: messages to it would
/// queue for good. Past this the CLI stops what is left and says so on stderr
/// (run.rs `stops_background`).
const BG_WAIT_CEILING_MS: &str = "10800000";

/// A helper's role: a lead where it was spawned with Luna's tools, which
/// make it able to start helpers of its own, else a helper.
pub(crate) fn role_of(s: &Spec) -> crate::library::Role {
    match s.tools {
        true => crate::library::Role::Lead,
        false => crate::library::Role::Helper,
    }
}

/// `command_with` on the texts its role is served now.
#[cfg(test)]
pub(super) fn command_for(e: &Entry, id: &str) -> Result<Command, String> {
    command_with(e, id, &crate::library::role_texts(role_of(&e.spec)))
}

/// `stdin_with` on the note its role is served now.
#[cfg(test)]
pub(super) fn stdin_text(e: &Entry, prompt: &str) -> String {
    stdin_with(e, prompt, crate::library::role_texts(role_of(&e.spec)).note.as_deref())
}

/// The arguments and working directory of the next turn, `texts` its role's
/// system texts, read before this module's lock (`spawn_turn`). Err when a
/// Claude Code turn's mandatory settings — its core-folder deny (threat model
/// F3) — cannot be written, or a Codex turn's saved thread is no thread id:
/// the turn refuses rather than run without them.
#[cfg(test)]
pub(super) fn command_with(e: &Entry, id: &str, texts: &crate::library::RoleTexts) -> Result<Command, String> {
    command_and_open(e, id, texts).map(|(command, _)| command)
}

/// `command_with`, and for Codex the request that opens the turn's thread
/// once its server is up (appserver.rs): `thread/start` with the folder, the
/// model and the role layers, or `thread/resume` of the thread an earlier
/// turn started, with the same.
pub(super) fn command_and_open(e: &Entry, id: &str, texts: &crate::library::RoleTexts) -> Result<(Command, Option<Value>), String> {
    let mut open = None;
    let s = &e.spec;
    let role = role_of(s);
    let constraints: Vec<String> = s.constraints.iter().map(|c| c.text.clone()).collect();
    let mut args: Vec<String> = Vec::new();
    let mut envs: Vec<(&'static str, String)> = Vec::new();
    let mut applied = vec![
        Item::Account.with(match s.provider {
            Provider::Claude => format!("CLAUDE_CONFIG_DIR={}", s.account_path),
            Provider::Codex => format!("CODEX_HOME={}", s.account_path),
        }),
        crate::claude::launch::session_applied(id, s.parent.as_deref()),
    ];
    // Where the CLI said it works when it is not the folder it was sent to, while
    // it is there: a conversation is found by its folder.
    let made = e.cwd.clone().filter(|c| !crate::pty::same_dir(c, &s.folder));
    let mut dir = s.folder.clone();
    match s.provider {
        Provider::Claude => {
            args.extend(
                ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--replay-user-messages"]
                    .map(String::from),
            );
            applied.push(
                Item::HelperHeadless.with("-p, --input-format stream-json, --output-format stream-json, --verbose, --replay-user-messages"),
            );
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
                applied.push(Item::Model.with(format!("--model {m}")));
            }
            // No effort for a model that takes none (claude/models.rs): no flag.
            if !s.effort.is_empty() {
                args.extend(["--effort".into(), s.effort.clone()]);
                applied.push(Item::Effort.with(format!("--effort {}", s.effort)));
            }
            // A spawn that named none runs in what Claude Code's own settings say.
            if !s.own_mode {
                args.extend(["--permission-mode".into(), s.mode.clone()]);
                applied.push(Item::PermissionMode.with(format!("--permission-mode {}", s.mode)));
            }
            match (&e.session, &made) {
                (Some(sid), Some(wt)) if std::path::Path::new(wt).is_dir() => {
                    dir = wt.clone();
                    args.extend(["--resume".into(), sid.clone()]);
                }
                (Some(sid), _) => args.extend(["--resume".into(), sid.clone()]),
                // No conversation to resume, in the worktree an earlier turn
                // was in.
                (None, Some(wt)) if std::path::Path::new(wt).is_dir() => dir = wt.clone(),
                (None, _) => {}
            }
            if let Some(sid) = &e.session {
                applied.push(Item::Resume.with(format!("--resume {sid}")));
            }
            if let Some(agent) = &s.agent {
                args.extend(["--agent".into(), agent.clone()]);
                applied.push(Item::Agent.with(format!("--agent {agent}")));
            }
            let denied: Vec<&str> = if s.read_only { crate::claude::launch::CLAUDE_EDIT_TOOLS.to_vec() } else { Vec::new() };
            if s.read_only {
                applied.push(Item::ReadOnly.with(format!("--disallowedTools {}", denied.join(","))));
            }
            // Luna's server alone, unless spawned with the account's own; what
            // that keeps it to is applied with the settings file that holds it.
            let helper = crate::claude::launch::HelperSettings { connectors: s.connectors };
            if !s.connectors {
                args.push(crate::claude::launch::STRICT_MCP.into());
            }
            // Its permission prompts and `AskUserQuestion` come as control
            // requests on its stdout and are answered on its stdin
            // (events.rs, ask.rs `take_ask`), tools or not.
            args.extend(["--permission-prompt-tool".into(), "stdio".into()]);
            applied.push(Item::HelperPermissionPrompt.with("--permission-prompt-tool stdio"));
            // Luna's server only with Luna's tools, as for Codex: nothing else
            // of it is a helper's.
            if let Some((mcp, token)) = s.token.clone().filter(|_| s.tools).and_then(|t| Some((crate::pty::claude_mcp_file(id)?, t))) {
                let mcp = mcp.to_string_lossy().into_owned();
                let allowed = crate::claude::launch::luna_allowed_tools();
                applied.push(Item::LunaTools.with(crate::claude::launch::luna_tools_value(&mcp, &allowed)));
                args.extend(["--mcp-config".into(), mcp]);
                envs.push((crate::claude::launch::CLAUDE_MCP_TOKEN_ENV, token));
                args.extend(["--allowedTools".into(), allowed]);
            }
            if !denied.is_empty() {
                args.extend(["--disallowedTools".into(), denied.join(",")]);
            }
            if let (Some(_), Some(note)) = (&s.parent, &texts.note) {
                applied.push(Item::HelperIdentity.with(format!("a note after its first prompt: {note}")));
            }
            // A helper with tools has helpers of its own to hear from: what a
            // notice names is a file there, to read without asking.
            let mut notes = None;
            if s.tools {
                let dir = crate::agents::notes_dir();
                if std::fs::create_dir_all(&dir).is_ok() {
                    let dir = dir.to_string_lossy().into_owned();
                    args.extend(["--add-dir".into(), dir.clone()]);
                    applied.push(Item::NotesDir.with(format!("--add-dir {dir}")));
                    notes = Some(dir);
                }
            }
            // The skills the library gives sessions where it works and keeps
            // from them, on every turn: each is a process of its own
            // (library/folders.rs `session_args`). No hooks: its stream says it all.
            let mut skills = crate::library::session_args(Provider::Claude, &s.account_path, &dir, role);
            // Its role layers, written afresh every turn: an edit reaches it
            // at its next (library/role.rs).
            // Never without them: a lead or helper started without its
            // role and constraints would run on what its caller asked alone.
            let layers = crate::library::role_layers(texts, &skills, &constraints);
            let instructions = crate::pty::claude_instructions_dir(id, layers.as_deref())
                .map_err(|why| format!("the helper's turn is not started: {why}; its role and constraints would not reach it"))?;
            // The folders Luna adds itself: the notes' alone, as the folders'
            // skills come as a plugin (`give_plugins`), no folder added.
            let luna_dirs: Vec<String> = notes.into_iter().collect();
            let own = instructions.as_ref().map(|d| d.to_string_lossy().into_owned());
            if own.is_some() {
                (skills.memory_off, skills.memory_kept) = crate::pty::claude_memory_off(&luna_dirs, &s.account_path, &dir, own.as_deref());
            }
            // The account's instructions left out only where that takes
            // nothing of the project's own.
            (skills.excluded, skills.excluded_kept) = crate::pty::claude_account_excludes(
                std::mem::take(&mut skills.excluded),
                &luna_dirs,
                &s.account_path,
                &dir,
                own.as_deref(),
            );
            let (settings, holds) = crate::pty::claude_settings_file(id, false, &skills, Some(helper), None)?;
            // The folders' skills as a plugin of this helper's own, made afresh
            // every turn (library/session_plugin.rs) once nothing that can
            // refuse the turn is left; a turn whose plugin will not write runs
            // without them, said in what it applied and to its parent
            // (`background/run.rs`).
            let skills_off = crate::library::give_plugins(id, &mut skills).err();
            args.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
            applied.extend(holds);
            if let Some(dir) = instructions {
                let file = dir.join("CLAUDE.md").to_string_lossy().into_owned();
                let dir = dir.to_string_lossy().into_owned();
                args.extend(["--add-dir".into(), dir.clone()]);
                envs.push((crate::claude::launch::CLAUDE_ADDED_DIR_MEMORY, "1".into()));
                applied.push(crate::claude::launch::instructions_applied(&dir));
                applied.extend(crate::library::layers_applied(texts, &skills, &constraints, &file));
            }
            applied.extend(crate::claude::launch::skills_applied(&skills.args));
            applied.extend(skills_off.map(|why| Item::LibrarySkillsOff.with(why)));
            args.extend(skills.args);
            envs.push(("CLAUDE_CONFIG_DIR", s.account_path.clone()));
            envs.push(("DISABLE_AUTOUPDATER", "1".into()));
            applied.push(Item::NoAutoUpdate.with("DISABLE_AUTOUPDATER=1"));
            // Never inherited: a `0` there turns auto-memory on (`HelperSettings`).
            envs.push((crate::claude::launch::NO_AUTO_MEMORY_ENV, "1".into()));
            // Its stdin closes with each turn's result (events.rs), and from then
            // `claude -p` waits only so long for what the turn left in the
            // background before it stops it, subagents and their uncommitted
            // work included (2.1.319+, 10 min by default). A lead's may stay
            // open past the result for its helpers' notices, for as long
            // again at most (events.rs `LISTEN_MS`): this counts from its
            // close, not before. The turn is the helper's until then: one that
            // waits that long is told to the parent as a turn gone quiet, with
            // what it waits on (agents/notifier `QUIET_TURN_MS`), and ends
            // with `luna_session_stop`. Set always, over anything inherited.
            envs.push((BG_WAIT_CEILING_ENV, BG_WAIT_CEILING_MS.into()));
            applied.push(Item::HelperBackgroundWait.with(format!("{BG_WAIT_CEILING_ENV}={BG_WAIT_CEILING_MS}")));
        }
        Provider::Codex => {
            // One `codex app-server` a turn, spoken to over its stdio
            // (appserver.rs): the thread is opened and the turn started by
            // requests, not flags, and the turn's text goes in `turn/start`.
            args.push("app-server".into());
            applied.push(Item::HelperHeadless.with("app-server: JSON-RPC on stdio, thread/start or thread/resume, then turn/start"));
            // A conversation resumes, and one not begun starts, where an
            // earlier turn worked if that is still there.
            if let Some(wt) = made.filter(|w| std::path::Path::new(w).is_dir()) {
                dir = wt;
            }
            let mut resume = None;
            match &e.session {
                // By its id only, any form of a UUID as the plain one: never
                // a name or anything the server would pick by itself
                // (codex/launch.rs `thread_to_resume`).
                Some(thread) => {
                    let thread =
                        crate::codex::launch::thread_to_resume(thread).map_err(|why| format!("the helper's turn is not started: {why}"))?;
                    applied.push(Item::Resume.with(format!("thread/resume {thread}")));
                    resume = Some(thread);
                }
                None => applied.push(Item::Folder.with(format!("thread/start cwd {dir}"))),
            }
            // The server cannot ask for the folder's trust and goes without
            // the project's config and hooks if it lacks it; a helper is where
            // it was sent, trusted for the run (codex/trust.rs).
            if let Some((trust, item)) = crate::pty::codex_trust(&s.account_path, &s.folder, true) {
                applied.push(item.with(format!("-c {}", trust[1])));
                args.extend(trust);
            }
            if let Some(m) = &s.model {
                applied.push(Item::Model.with(format!("model {m} of the thread")));
            }
            if !s.effort.is_empty() {
                applied.push(Item::Effort.with(format!("effort {} of turn/start", s.effort)));
            }
            // The approval and sandbox Luna passes, typed in the thread's
            // request too: a resumed thread's own saved settings win over `-c`
            // (0.169.0 restores them as typed overrides), not over these, and
            // the server's answer is checked against them (appserver.rs
            // `settings_differ`).
            let mut typed = serde_json::Map::new();
            if s.auto_review {
                typed.insert("approvalPolicy".into(), json!("on-request"));
                typed.insert("approvalsReviewer".into(), json!("auto_review"));
                typed.insert("sandbox".into(), json!(crate::codex::launch::AUTO_REVIEW_SANDBOX));
                // What `--approve-for-me` stands for, which the server has no
                // flag for: the same overrides on every turn.
                for o in crate::codex::launch::AUTO_REVIEW_OVERRIDES {
                    args.extend(["-c".into(), o.to_string()]);
                }
                let [reviewer, policy, sandbox] = crate::codex::launch::AUTO_REVIEW_OVERRIDES;
                applied.push(Item::Approval.with(format!("-c {reviewer} -c {policy}")));
                applied.push(Item::Sandbox.with(format!("-c {sandbox}")));
            } else {
                // What it asks comes to its caller (appserver.rs): the approval
                // its parent named, or none, and its config decides.
                if let Some(approval) = &s.approval {
                    typed.insert("approvalPolicy".into(), json!(approval));
                    let policy = format!("approval_policy=\"{approval}\"");
                    applied.push(Item::Approval.with(format!("-c {policy}")));
                    args.extend(["-c".into(), policy]);
                }
                // A spawn that named none runs in the sandbox Codex's own config
                // settles on, with no flag of Luna's; its read-only still takes
                // Luna's profile where that keeps it out of Luna's `core/`
                // (codex/launch.rs `denies_core`).
                let sandbox = if s.own_mode && !crate::codex::launch::takes_profile(&s.mode, cfg!(windows)) {
                    Vec::new()
                } else {
                    crate::pty::codex_sandbox_args(&s.mode)
                };
                if !sandbox.is_empty() {
                    applied.push(crate::codex::launch::sandbox_applied(&s.mode, &sandbox));
                }
                // A plain sandbox mode typed as `sandbox`; Luna's own profile
                // (read-only-net, a core deny), defined by `-c`, typed by its
                // name as `permissions` (experimental, never beside
                // `sandbox`), which the server's `activePermissionProfile`
                // answers (0.169.0, seen live).
                if sandbox.iter().any(|a| a.starts_with("sandbox_mode=")) {
                    typed.insert("sandbox".into(), json!(s.mode));
                }
                let profile = sandbox.iter().find_map(|a| a.strip_prefix("default_permissions=\"").and_then(|r| r.strip_suffix('"')));
                if let Some(profile) = profile {
                    typed.insert("permissions".into(), json!(profile));
                }
                if let Some((profile, dir)) = profile_deny(&sandbox) {
                    applied.push(crate::codex::launch::core_deny_applied(&profile, &dir));
                }
                args.extend(sandbox);
            }
            let own = crate::codex::config::windows_sandbox_set(&s.account_path);
            if let Some(sb) = crate::codex::launch::codex_windows_sandbox(&s.mode).filter(|_| !own) {
                applied.push(Item::WindowsSandbox.with(format!("-c {sb}")));
                args.extend(["-c".into(), sb]);
            }
            if let Some(path) = crate::pty::codex_sandbox_path(&s.mode) {
                envs.push(("PATH", path));
                applied.push(Item::SandboxPath.with("PATH without …\\Microsoft\\WindowsApps"));
            }
            args.extend(crate::codex::launch::no_update_check());
            applied.push(Item::NoAutoUpdate.with(format!("-c {}", crate::codex::launch::NO_UPDATE_CHECK)));
            args.extend(crate::codex::launch::no_memories());
            applied.push(crate::codex::launch::no_memories_applied());
            // Luna's own server, with its tools: none while the hub is not up.
            let luna = s.token.clone().filter(|_| s.tools).zip(crate::hub::mcp_url()).map(|(token, url)| {
                let approval = if s.auto_review { Some(crate::codex::launch::AUTO_REVIEW) } else { s.approval.as_deref() };
                let prompt = crate::codex::defaults::prompt_for_spawn(approval, &s.account_path, &dir, true, resume.is_some());
                (crate::codex::launch::mcp_args(&url, prompt), token, prompt)
            });
            if luna.is_some() {
                let found =
                    crate::codex::config::servers_named(&s.account_path, &dir, crate::codex::launch::LUNA_SERVER, true).map_err(|e| {
                        format!("the helper is not started: Luna could not read whether its run has an MCP server named luna ({e})")
                    })?;
                crate::codex::launch::luna_server_free(&found, !s.connectors, "the helper")?;
            }
            // Before Luna's own server and the library's plugins: the tables
            // these give whole would replace what an earlier `-c` set in them.
            if !s.connectors {
                let mcp = crate::codex::config::helper_mcp(&s.account_path, &dir)?;
                let off = crate::codex::launch::connectors_off(&mcp, luna.is_some());
                applied.push(crate::codex::launch::connectors_off_applied(&off));
                args.extend(off);
            }
            if let Some((mcp, token, prompt)) = luna {
                args.extend(mcp);
                envs.push(("LUNA_MCP_TOKEN", token));
                applied.push(Item::LunaTools.with(crate::codex::launch::luna_tools_value(prompt)));
            }
            let git = crate::pty::codex_git_envs(&s.mode);
            if !git.is_empty() {
                applied.push(crate::codex::launch::git_ssl_applied(&git));
            }
            envs.extend(git);
            envs.push(("CODEX_HOME", s.account_path.clone()));
            let mut library = crate::library::session_args(Provider::Codex, &s.account_path, &dir, role);
            applied.extend(crate::codex::launch::library_applied(&library.args));
            applied.extend(crate::library::audience_applied(&library));
            args.extend(std::mem::take(&mut library.args));
            // Its role layers, on every turn's thread request: Codex takes them
            // at the thread's start and after a compaction (library/role.rs),
            // after its config's own value, which a value given here replaces.
            // Whole: a JSON request has no command line to fit, so what a
            // chat's `-c` spills to a file (pty/start.rs `codex_role_args`)
            // goes here as it is. The folder is trusted for the run, so its
            // project's config counts.
            let instructions = crate::library::role_layers(texts, &library, &constraints).map(|layers| {
                let stack = crate::codex::config::developer_instructions_in(&s.account_path, &dir, true);
                let value = match stack.value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
                    Some(own) => format!("{own}\n\n{layers}"),
                    None => layers,
                };
                let how = match stack.value {
                    Some(_) => "developerInstructions of the thread, after the config's own",
                    None => "developerInstructions of the thread",
                };
                applied.extend(crate::library::layers_applied(texts, &library, &constraints, how));
                if !stack.unsure.is_empty() {
                    applied.push(Item::RoleLayersUnsure.with(stack.unsure.join("; ")));
                }
                value
            });
            if let (Some(_), Some(note)) = (&s.parent, &texts.note) {
                applied.push(Item::HelperIdentity.with(format!("a note after every prompt: {note}")));
            }
            let mut params = match &resume {
                Some(thread) => json!({ "threadId": thread, "cwd": dir }),
                None => json!({ "cwd": dir }),
            };
            if let Some(m) = &s.model {
                params["model"] = json!(m);
            }
            if let Some(value) = instructions {
                params["developerInstructions"] = json!(value);
            }
            for (k, v) in typed {
                params[k.as_str()] = v;
            }
            let method = if resume.is_some() { "thread/resume" } else { "thread/start" };
            open = Some(json!({ "method": method, "params": params }));
        }
    }
    envs.extend(crate::pty::session_envs(id, s.parent.as_deref()));
    // No terminal: a helper's CLI writes to pipes (`run.rs`, `clean_env`).
    crate::pty::finish_spec(&mut envs, &mut applied, &dir, false);
    Ok(((args, dir, envs, applied), open))
}

/// The profile and the folder Luna's sandbox `-c`s deny (codex/launch.rs
/// `profile_args`), if they deny one.
fn profile_deny(args: &[String]) -> Option<(String, String)> {
    let value = args.iter().find_map(|a| a.strip_prefix("permissions.").and_then(|r| r.split_once(".filesystem=")))?;
    let (profile, table) = value;
    let doc: toml_edit::DocumentMut = format!("f = {table}").parse().ok()?;
    let (dir, _) = doc["f"].as_inline_table()?.iter().next()?;
    Some((profile.to_string(), dir.to_string()))
}

/// What a turn's CLI is given: `prompt`, with the note for its role
/// (`pty::helper_prompt`, `note` as `spawn_turn` read it) after the first of
/// a Claude Code helper's conversation and after every one of a Codex
/// helper's. Both have their role layers at memory level every turn too;
/// the note stands beside the task, which a Codex thread may have compacted
/// away from where its developer message was. Not in `Entry::opening`, which
/// goes back to the caller as what it asked, nor in what `luna_message_list`
/// gives back (`message_of` in codex/session/read.rs and
/// claude/session/transcript.rs). A helper whose parent is gone is told none.
pub(super) fn stdin_with(e: &Entry, prompt: &str, note: Option<&str>) -> String {
    match (&e.spec.parent, note, e.spec.provider) {
        (Some(_), Some(note), Provider::Codex) => crate::pty::helper_prompt(note, prompt),
        (Some(_), Some(note), Provider::Claude) if e.session.is_none() => crate::pty::helper_prompt(note, prompt),
        _ => prompt.to_string(),
    }
}

/// A message as a line of Claude Code's stream-json stdin. The uuid is what its
/// replay of the line comes back under, and what its transcript keeps.
pub(super) fn user_line(uuid: &str, text: &str) -> String {
    let line = json!({
        "type": "user",
        "message": { "role": "user", "content": text },
        "parent_tool_use_id": null,
        "session_id": "",
        "uuid": uuid,
    });
    format!("{line}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn with no conversation to resume starts where the CLI said an earlier
    /// turn worked, with no `--worktree`, which would make another (Claude Code
    /// 2.1.325 `-w, --worktree [name]`; Codex 0.169.0 `--worktree`, "Run the
    /// session in a new managed Git worktree").
    #[test]
    fn a_fresh_conversation_starts_where_an_earlier_turn_worked() {
        let wt = std::env::temp_dir().join(format!("luna-fresh-wt-{}", std::process::id()));
        std::fs::create_dir_all(&wt).unwrap();
        let wt = wt.to_string_lossy().into_owned();
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.cwd = Some(wt.clone());
            e.session = None;
            let (args, dir, _, _) = command_for(&e, "x").unwrap();
            assert_eq!(dir, wt, "{provider:?}");
            assert!(!args.iter().any(|a| a.starts_with("--worktree")), "{provider:?}: {args:?}");
            assert!(!args.iter().any(|a| a == "--resume" || a == "resume"), "{provider:?}: {args:?}");
            if provider == Provider::Codex {
                let open = open_of(&e, "x");
                assert_eq!((open["method"].as_str(), open["params"]["cwd"].as_str()), (Some("thread/start"), Some(wt.as_str())));
            }
        }
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn notes_are_open_to_a_helper_with_tools() {
        let mut e = entry(Provider::Claude);
        let notes = crate::agents::notes_dir().to_string_lossy().into_owned();
        let has_dir = |e: &Entry| command_for(e, "x").unwrap().0.windows(2).any(|w| w[0] == "--add-dir" && w[1] == notes);
        assert!(!has_dir(&e), "one with no tools gets no notices");
        e.spec.tools = true;
        assert!(has_dir(&e));
        let mut codex = entry(Provider::Codex);
        codex.spec.tools = true;
        assert!(!command_for(&codex, "x").unwrap().0.iter().any(|a| a == "--add-dir"), "Codex's sandboxes read anywhere");
    }

    #[test]
    fn a_claude_turn_refuses_when_the_core_folder_deny_cannot_be_written() {
        // A Claude settings file carries the core-folder deny (threat model
        // F3). A folder where the file of this id goes makes the write fail:
        // the turn must refuse, not run without the deny.
        let bad = "f3-blocked";
        let hooks = crate::paths::data_dir().join("hooks");
        let blocker = hooks.join(format!("{}.json", crate::paths::id_file_name(bad)));
        let _ = crate::secret::private_dir(&hooks);
        std::fs::create_dir_all(&blocker).unwrap();
        assert!(command_for(&entry(Provider::Claude), bad).is_err(), "no deny written, no Claude turn");
        // The helper itself refuses the same way.
        let none = crate::library::SessionSkills::default();
        assert!(crate::pty::claude_settings_file(bad, true, &none, None, None).is_err(), "and a pane start would too");
        // Codex carries the deny in its sandbox profile, not a settings file,
        // so the same id does not stop it.
        assert!(command_for(&entry(Provider::Codex), bad).is_ok(), "Codex writes no settings file");
        let _ = std::fs::remove_dir_all(&blocker);
    }

    #[test]
    fn a_turn_resumes_the_conversation_the_first_one_named() {
        let mut e = entry(Provider::Codex);
        let (args, dir, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[0], "app-server");
        assert!(
            args.iter().any(|a| a == "sandbox_mode=\"read-only\"" || a.starts_with("default_permissions=\"luna-")),
            "held by its sandbox"
        );
        assert_eq!(dir, "C:\\p");
        let open = open_of(&e, "x");
        assert_eq!(open["method"], "thread/start");
        assert_eq!((open["params"]["cwd"].as_str(), open["params"]["model"].as_str()), (Some("C:\\p"), Some("haiku")));
        e.session = Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
        let open = open_of(&e, "x");
        assert_eq!(
            (open["method"].as_str(), open["params"]["threadId"].as_str()),
            (Some("thread/resume"), Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"))
        );
        assert_eq!(command_for(&e, "x").unwrap().0[0], "app-server");
        // Only by the thread's id: nothing the server would read as a name.
        for bad in [
            "--last",
            "",
            "a b",
            "release-review",
            "t-1",
            "0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b --last",
            "0198a1b2c3d4-7e5f-8a9b-0c1d2e3f4a5b",
        ] {
            e.session = Some(bad.into());
            let why = command_for(&e, "x").unwrap_err();
            assert!(why.contains("not started") && why.contains("by its id only"), "{bad:?}: {why}");
        }
        // Any form of a UUID goes as the plain one.
        for form in [
            "0198A1B2-C3D4-7E5F-8A9B-0C1D2E3F4A5B",
            "0198a1b2c3d47e5f8a9b0c1d2e3f4a5b",
            "{0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b}",
            "urn:uuid:0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
        ] {
            e.session = Some(form.into());
            assert_eq!(open_of(&e, "x")["params"]["threadId"], "0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b", "{form}");
        }

        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "s-1", "cwd": "C:\\p" }));
        let (args, _, envs, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["-p", "--input-format", "stream-json"]);
        let at = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[at + 1], "s-1");
        assert!(envs.iter().any(|(k, v)| *k == "CLAUDE_CONFIG_DIR" && v == "C:\\acc"));
    }

    #[test]
    fn a_claude_helper_on_a_model_without_effort_gets_no_effort_flag() {
        let mut e = entry(Provider::Claude);
        e.spec.effort = "high".into();
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert!(args.windows(2).any(|w| w == ["--effort", "high"]), "{args:?}");
        e.spec.model = Some("claude-haiku-4-5-20251001".into());
        e.spec.effort = String::new();
        let (args, _, _, applied) = command_for(&e, "x").unwrap();
        assert!(!args.iter().any(|a| a == "--effort"), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--model", "claude-haiku-4-5-20251001"]), "{args:?}");
        assert!(!applied.iter().any(|a| format!("{a:?}").contains("--effort")));
    }

    #[test]
    fn a_claude_helpers_mcp_token_goes_by_its_environment_and_never_into_its_config_file() {
        crate::hub::start();
        let mut e = entry(Provider::Claude);
        e.spec.token = Some("the-helpers-token".into());
        e.spec.tools = true;
        let (args, _, envs, _) = command_for(&e, "bg-mcp-token").unwrap();
        let at = args.iter().position(|a| a == "--mcp-config").expect("Luna's server");
        let config = std::fs::read_to_string(&args[at + 1]).unwrap();
        assert!(!config.contains("the-helpers-token"), "{config}");
        assert!(config.contains("Bearer ${LUNA_MCP_TOKEN}"), "{config}");
        assert!(envs.iter().any(|(k, v)| *k == "LUNA_MCP_TOKEN" && v == "the-helpers-token"));
        let _ = std::fs::remove_file(&args[at + 1]);
    }

    #[test]
    fn a_helpers_turn_never_asks_its_cli_for_a_worktree() {
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            for session in [None, Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string())] {
                e.session = session;
                let (args, dir, _, _) = command_for(&e, "x").unwrap();
                assert!(!args.iter().any(|a| a.starts_with("--worktree")), "{provider:?}: {args:?}");
                assert_eq!(dir, "C:\\p", "{provider:?}");
            }
        }
    }

    #[test]
    fn a_codex_helpers_note_goes_with_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.parent = Some("p-1".into());
        let notes = |text: String| text.matches("[A note, not part of the task: ").count();
        assert_eq!(notes(stdin_text(&e, "the prompt")), 1, "the turn that starts its conversation");
        assert_eq!(stdin_text(&e, "/review"), "/review", "not into a command's arguments");
        e.session = Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
        assert_eq!(notes(stdin_text(&e, "Carry on where you left off.")), 1, "and each one after: a compaction loses the first");
        // Claude Code's first prompt has it too, beside the task its account's
        // instructions would otherwise outweigh; after that its role layers
        // say it on every turn.
        let mut e = entry(Provider::Claude);
        e.spec.parent = Some("p-1".into());
        let first = stdin_text(&e, "the prompt");
        assert!(first.starts_with("the prompt\n\n") && notes(first.clone()) == 1, "{first}");
        assert!(first.contains("comes from another agent, not a person"), "{first}");
        for word in ["operator", "Luna", "session p-1"] {
            assert!(!first.contains(word), "the note names nobody: {word}");
        }
        assert_eq!(stdin_text(&e, "/review"), "/review");
        e.session = Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
        assert_eq!(stdin_text(&e, "the next"), "the next", "its role layers say it there");
        e.session = None;
        e.spec.parent = None;
        assert_eq!(stdin_text(&e, "the prompt"), "the prompt", "a helper of nobody's is told nothing");
    }

    /// A Codex turn's request that opens its thread (`command_and_open`).
    fn open_of(e: &Entry, id: &str) -> Value {
        command_and_open(e, id, &crate::library::role_texts(role_of(&e.spec))).unwrap().1.expect("a Codex turn opens its thread")
    }

    /// The text of a turn's role layers: Claude Code's `CLAUDE.md` in the
    /// folder it is given, Codex's `developerInstructions` of its thread.
    fn layers_of(e: &Entry, id: &str) -> Option<String> {
        match e.spec.provider {
            Provider::Claude => {
                let (args, _, _, _) = command_for(e, id).unwrap();
                let hooks = crate::paths::data_dir().join("hooks");
                args.windows(2)
                    .filter(|w| w[0] == "--add-dir" && std::path::Path::new(&w[1]).starts_with(&hooks))
                    .find_map(|w| std::fs::read_to_string(std::path::Path::new(&w[1]).join("CLAUDE.md")).ok())
            }
            Provider::Codex => open_of(e, id)["params"]["developerInstructions"].as_str().map(str::to_owned),
        }
    }

    #[test]
    fn a_helper_passed_to_another_parent_keeps_its_role_and_is_told_of_nobody_without_one() {
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.spec.parent = Some("old".into());
            e.session = Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
            lock().parent_changes.remove("moved");
            lock().entries.insert("moved".into(), e);
            set_parent("moved", Some("new".into()), 1);
            let r = lock();
            let e = &r.entries["moved"];
            let (_, _, envs, _) = command_for(e, "moved").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "new"), "{provider:?}");
            assert!(layers_of(e, "moved").unwrap().contains("Your task comes from another agent"));
            let line = stdin_text(e, "go on");
            match provider {
                Provider::Claude => assert_eq!(line, "go on", "its role layers say it"),
                Provider::Codex => assert_eq!(line.matches("[A note, not part of the task: ").count(), 1, "{line}"),
            }
            drop(r);
            // With nobody to answer to, it hears no note; its role stays.
            set_parent("moved", None, 2);
            let e = lock().entries.remove("moved").unwrap();
            let (_, _, envs, _) = command_for(&e, "moved").unwrap();
            assert!(!envs.iter().any(|(k, _)| *k == "LUNA_PARENT_ID"));
            assert!(layers_of(&e, "moved").is_some());
            assert_eq!(stdin_text(&e, "go on"), "go on");
        }
    }

    #[test]
    fn read_only_net_goes_to_codex_as_a_profile_on_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = crate::codex::launch::READ_ONLY_NET.into();
        for session in [None, Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string())] {
            e.session = session;
            let (args, _, envs, _) = command_for(&e, "x").unwrap();
            assert!(args.iter().any(|a| a.starts_with("default_permissions=\"luna-")));
            assert!(args.iter().any(|a| a.ends_with(".network.enabled=true")));
            assert!(!args.iter().any(|a| a.starts_with("sandbox_mode=")), "Codex's docs say not to set both");
            assert_eq!(envs.iter().any(|(k, _)| *k == "GIT_CONFIG_COUNT"), cfg!(windows), "git https, which schannel cannot do there");
            assert!(!args.iter().any(|a| a.starts_with("approval_policy=") || a.starts_with("approvals_reviewer=")), "its config's own");
        }
    }

    /// What a Codex helper asks comes to its caller: the approval its parent
    /// named is passed, none where it named none, and no reviewer of Luna's.
    #[test]
    fn a_codex_helper_is_given_the_approval_its_parent_named_and_nothing_else() {
        let mut e = entry(Provider::Codex);
        for (approval, want) in
            [(None, None), (Some("on-request"), Some("approval_policy=\"on-request\"")), (Some("never"), Some("approval_policy=\"never\""))]
        {
            e.spec.approval = approval.map(str::to_owned);
            for session in [None, Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string())] {
                e.session = session;
                let (args, _, _, _) = command_for(&e, "x").unwrap();
                let given: Vec<&String> = args.iter().filter(|a| a.starts_with("approval_policy=")).collect();
                assert_eq!(given.first().map(|a| a.as_str()), want, "{args:?}");
                assert!(given.len() <= 1 && !args.iter().any(|a| a.starts_with("approvals_reviewer=")), "{args:?}");
            }
        }
    }

    #[test]
    fn read_only_takes_the_edit_tools_away_from_claude() {
        let mut e = entry(Provider::Claude);
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--disallowedTools"), None);
        e.spec.read_only = true;
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--disallowedTools"), Some("Edit,Write,NotebookEdit"));
        assert_eq!(args.iter().filter(|a| *a == "--disallowedTools").count(), 1);
    }

    #[test]
    fn an_agent_is_claude_codes_flag_on_every_turn() {
        let mut e = entry(Provider::Claude);
        e.spec.agent = Some("checker".into());
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--agent"), Some("checker"));
        e.session = Some("s-1".into());
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--agent"), Some("checker"));
        assert_eq!(value_of(&args, "--resume"), Some("s-1"));
    }

    #[test]
    fn a_helper_knows_it_is_one_on_both_clis() {
        for provider in [Provider::Claude, Provider::Codex] {
            // An id of its own: its role layers are a file of the id's, which
            // another test's turn of "x" may write meanwhile.
            let mut e = entry(provider);
            e.spec.parent = Some("p-1".into());
            let (args, _, envs, applied) = command_for(&e, "bg-knows").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_SESSION_ID" && v == "bg-knows"));
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));
            assert!(!args.iter().any(|a| a == "--append-system-prompt"), "memory outweighs it: the role goes there");
            let layers = layers_of(&e, "bg-knows").unwrap_or_else(|| panic!("{provider:?}: {args:?}"));
            assert!(layers.starts_with("# Your role\n\nYour task comes from another agent"), "{layers}");
            assert!(!layers.contains("helpers of your own"), "a helper is no lead");
            assert!(applied.iter().any(|a| a.key == "role.layers"), "{provider:?}");
            if provider == Provider::Claude {
                assert!(envs.iter().any(|(k, v)| *k == crate::claude::launch::CLAUDE_ADDED_DIR_MEMORY && v == "1"));
            }
            // A lead is told of its helpers.
            e.spec.tools = true;
            let _ = command_for(&e, "bg-knows").unwrap();
            assert!(layers_of(&e, "bg-knows").unwrap().contains("You may start helpers of your own"), "{provider:?}");
        }
        for provider in [Provider::Claude, Provider::Codex] {
            let (_, _, envs, _) = command_for(&entry(provider), "x").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_SESSION_ID" && v == "x"), "whoever started it");
            assert!(!envs.iter().any(|(k, _)| *k == "LUNA_PARENT_ID"));
        }
    }

    #[test]
    fn constraints_are_the_last_layer_on_every_turn_of_both_clis() {
        let chain = vec![
            crate::agents::Constraint { from: "top".into(), text: "Touch nothing in src/.".into() },
            crate::agents::Constraint { from: "lead".into(), text: "No network.".into() },
        ];
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.spec.parent = Some("lead".into());
            e.spec.constraints = chain.clone();
            for session in [None, Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string())] {
                e.session = session.clone();
                let (_, _, _, applied) = command_for(&e, "constrained").unwrap();
                let layers = layers_of(&e, "constrained").unwrap();
                let (role, rules) = layers.split_once("# Constraints").expect("a heading of their own");
                assert!(role.contains("# Your role"), "{provider:?} {session:?}");
                assert!(rules.find("Touch nothing in src/.") < rules.find("No network."), "the caller's first: {rules}");
                assert!(rules.trim_end().ends_with("No network."), "nothing after them");
                let said = applied.iter().find(|a| a.key == "constraints").expect("said");
                assert_eq!(said.value, "Touch nothing in src/.\n\nNo network.");
            }
        }
    }

    #[test]
    fn a_codex_turns_role_layers_go_whole_however_long() {
        let mut e = entry(Provider::Codex);
        let text = format!("Keep to src/.{}End.", "x".repeat(crate::library::CODEX_ROLE_MAX));
        e.spec.constraints = vec![crate::agents::Constraint { from: "top".into(), text: text.clone() }];
        // A thread's request has no command line to fit: no pointer to a file.
        let layers = layers_of(&e, "too-long").unwrap();
        assert!(layers.starts_with("# Your role") && layers.contains(&text), "the layers whole, the constraints at the end of them");
        assert!(golden::developer_instructions(&command_for(&e, "too-long").unwrap().0).is_none());
        // Claude Code reads them from a file.
        let mut e = entry(Provider::Claude);
        e.spec.constraints = vec![crate::agents::Constraint { from: "top".into(), text }];
        assert!(command_for(&e, "too-long").is_ok());
    }

    #[test]
    fn a_claude_turn_whose_role_layers_will_not_write_is_refused() {
        const ID: &str = "bg-layers-unwritable";
        // A file where its folder goes: the write fails as on a locked or full disk.
        let dir = crate::paths::data_dir().join("hooks").join(crate::paths::id_file_name(ID));
        std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
        std::fs::write(&dir, b"in the way").unwrap();
        for tools in [false, true] {
            let mut e = entry(Provider::Claude);
            e.spec.parent = Some("p-1".into());
            e.spec.tools = tools;
            e.spec.constraints = vec![crate::agents::Constraint { from: "p-1".into(), text: "No network.".into() }];
            let why = command_for(&e, ID).unwrap_err();
            assert!(why.contains("not started") && why.contains("could not write its role layers"), "{why}");
        }
        std::fs::remove_file(&dir).unwrap();
        let e = entry(Provider::Claude);
        assert!(layers_of(&e, ID).is_some_and(|l| l.contains("# Your role")), "written once it can be");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_review_is_the_overrides_codex_own_flag_stands_for_on_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.auto_review = true;
        let overrides = strings(&[
            "-c",
            "approvals_reviewer=\"auto_review\"",
            "-c",
            "approval_policy=\"on-request\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
        ]);
        for session in [None, Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string())] {
            e.session = session;
            let (args, _, _, applied) = command_for(&e, "x").unwrap();
            assert!(!args.contains(&"--approve-for-me".to_string()), "the server has no such flag: {args:?}");
            assert!(args.windows(6).any(|w| w == overrides.as_slice()), "{args:?}");
            assert_eq!(args.iter().filter(|a| a.starts_with("approval_policy=") || a.starts_with("sandbox_mode=")).count(), 2, "{args:?}");
            assert!(applied.iter().any(|a| a.value.contains("approvals_reviewer")), "the Applied list says what went");
        }
    }

    /// A Codex helper's turn, resumes too, is given no hook of Luna's and no
    /// bypass of Codex's trust of hooks.
    #[test]
    fn a_codex_helper_is_given_no_hook_and_no_hook_trust_bypass() {
        let mut e = entry(Provider::Codex);
        e.session = Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(open_of(&e, "x")["method"], "thread/resume");
        assert!(!args.iter().any(|a| a == "--dangerously-bypass-hook-trust" || a.starts_with("hooks.")), "{args:?}");
        assert!(args.iter().any(|a| a.starts_with("projects={") && a.contains("trust_level")), "trusted for the run");
    }

    /// A lead's stdin kept open for notices is bounded as long as the CLI's
    /// own wait, which counts only once it is closed.
    #[test]
    fn a_lead_listens_past_its_result_no_longer_than_the_cli_waits() {
        assert_eq!(BG_WAIT_CEILING_MS.parse::<u64>().unwrap(), LISTEN_MS);
    }

    #[test]
    fn claude_helpers_keep_stdin_for_stream_json_and_codex_ones_for_json_rpc() {
        let has = |args: &[String], flag: &str| args.iter().any(|a| a == flag);
        let (args, _, _, _) = command_for(&entry(Provider::Claude), "x").unwrap();
        assert!(args.windows(2).any(|w| w == ["--input-format", "stream-json"]), "{args:?}");
        assert!(has(&args, "--replay-user-messages"), "its word that a line was taken in");
        let (args, _, _, _) = command_for(&entry(Provider::Codex), "x").unwrap();
        assert!(!has(&args, "--input-format") && !has(&args, "--replay-user-messages") && !has(&args, "-"), "{args:?}");
        assert!(!has(&args, "--listen") && !has(&args, "--stdio"), "stdio is the server's own default");
    }

    #[test]
    fn a_message_is_one_line_of_json_whatever_it_says() {
        let text = "two\nlines, \"quotes\", a back\\slash and 🌧";
        let line = user_line("the-uuid", text);
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1, "{line}");
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!((v["type"].as_str(), v["uuid"].as_str(), v["message"]["role"].as_str()), (Some("user"), Some("the-uuid"), Some("user")));
        assert_eq!(v["message"]["content"].as_str(), Some(text));
        assert!(v["parent_tool_use_id"].is_null());
    }

    // Golden tests of what a background helper's turn is started with: the
    // arguments, folder and environment of each kind of turn, word for word,
    // as they stand.

    use crate::golden::{self, pairs, strings, Redact};

    /// A turn's command for `e`, placeholders in, with the settings, MCP and
    /// instructions files it was given read back (and removed).
    struct Turn {
        args: Vec<String>,
        dir: String,
        envs: Vec<(String, String)>,
        settings: Option<Value>,
        mcp: Option<Value>,
        instructions: Option<String>,
        /// Codex: its thread's `developerInstructions`, the role layers.
        role: Option<String>,
        /// Codex: the request that opens its thread, the role layers as
        /// `<role-layers>` and its folder put as the others are.
        open: Option<Value>,
        /// What Luna applied, as (key, class, value), placeholders in.
        applied: Vec<(String, crate::applied::Class, String)>,
    }

    impl Turn {
        fn keys(&self) -> Vec<&str> {
            self.applied.iter().map(|(k, _, _)| k.as_str()).collect()
        }
        fn value(&self, key: &str) -> Option<&str> {
            self.applied.iter().find(|(k, _, _)| k == key).map(|(_, _, v)| v.as_str())
        }
    }

    fn turn(e: &Entry, id: &str, r: &Redact) -> Turn {
        let ((args, dir, envs, applied), open) = command_and_open(e, id, &crate::library::role_texts(role_of(&e.spec))).unwrap();
        Redact::assert_shown(&args, &envs, &dir, &applied);
        Redact::assert_probed(e.spec.provider, &args);
        let applied = r.applied(&applied, &envs);
        assert!(golden::developer_instructions(&args).is_none(), "the role goes with the thread, not on the command line");
        // A Codex turn's role layers, every turn, in its thread's request:
        // compared apart (`role`).
        let role = open.as_ref().and_then(|o| o["params"]["developerInstructions"].as_str().map(str::to_owned));
        let open = open.map(|mut o| {
            if role.is_some() {
                o["params"]["developerInstructions"] = json!("<role-layers>");
            }
            let cwd = o["params"]["cwd"].as_str().map(|c| r.text(c));
            o["params"]["cwd"] = json!(cwd);
            // Luna's profile, named afresh at every start, as the arguments put it.
            if o["params"].get("permissions").is_some() {
                o["params"]["permissions"] = json!("<profile>");
            }
            o
        });
        let args = r.args(&args);
        let data = crate::paths::data_dir();
        let read = |sub: &str, placeholder: &str| {
            let path = data.join(sub).join(format!("{}.json", crate::paths::id_file_name(id)));
            let v = args.iter().any(|a| a == placeholder).then(|| r.file(&path.to_string_lossy()));
            let _ = std::fs::remove_file(&path);
            v
        };
        let (settings, mcp) = (read("hooks", "<settings-file>"), read("mcp", "<mcp-file>"));
        let instructions = golden::instructions(id, &args);
        Turn { dir: r.text(&dir), envs: r.envs(&envs), settings, mcp, instructions, role, open, args, applied }
    }

    /// A Claude Code turn's arguments up to where a conversation is named.
    fn claude_turn(mode: &str) -> Vec<String> {
        strings(&[
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--replay-user-messages",
            "--model",
            "haiku",
            "--effort",
            "low",
            "--permission-mode",
            mode,
        ])
    }

    fn claude_turn_envs(id: &str) -> Vec<(String, String)> {
        pairs(&[
            ("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1"),
            ("CLAUDE_CONFIG_DIR", "C:\\acc"),
            ("DISABLE_AUTOUPDATER", "1"),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
            ("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS", "10800000"),
            ("LUNA_SESSION_ID", id),
        ])
    }

    /// A helper's token as its spawn has it (agents/spawn/background.rs):
    /// the one Luna knows it by, until the test forgets it.
    fn helper_token(id: &str, e: &Entry) -> Option<String> {
        let s = &e.spec;
        crate::agents::register(id, s.provider, &s.folder, &s.account_path, s.parent.as_deref(), true, json!({}))
    }

    /// A helper of `provider` sent to a library's folder, on its account
    /// there, and the placeholders for its turns.
    fn library_entry(id: &str, lib: &crate::library::GoldenLibrary, provider: Provider) -> (Entry, Redact) {
        let account = if provider == Provider::Claude { lib.claude() } else { lib.codex() };
        let mut e = entry(provider);
        e.spec.account_path = account.clone();
        e.spec.folder = lib.folder.clone();
        let r = Redact::for_chat(id)
            .with(lib.folder.clone(), "<project>")
            .with(account.clone(), "<acc>")
            // As `claudeMdExcludes` names a file, on every OS.
            .with(account.replace('\\', "/"), "<acc>");
        (e, r)
    }

    #[test]
    fn golden_headless_claude_turn_streams_json_in_its_folder() {
        const ID: &str = "golden-bg-claude";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Claude);
        e.spec.mode = "acceptEdits".into();
        let t = turn(&e, ID, &r);
        assert_eq!(t.args, with_settings(strict(claude_turn("acceptEdits"))));
        assert_eq!(t.dir, "C:\\p");
        assert_eq!(t.envs, claude_turn_envs(ID));
        assert_eq!(t.value("helper.background-wait"), Some("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS=10800000"), "said where it is set");
        assert_eq!(t.instructions.as_deref(), Some(golden::role_layers(crate::library::Role::Helper).as_str()));
        let settings = golden::claude_helper_settings(golden::claude_settings_alone());
        assert_eq!((t.settings, t.mcp), (Some(settings), None), "no hooks: its stream says it all");
    }

    /// `args`, then no MCP server but those it is given: none of its account's;
    /// and its asks over its stdin and stdout.
    fn strict(mut args: Vec<String>) -> Vec<String> {
        args.push("--strict-mcp-config".into());
        args.extend(asks());
        args
    }

    /// Every Claude Code turn's permission prompts, tools or not.
    fn asks() -> Vec<String> {
        strings(&["--permission-prompt-tool", "stdio"])
    }

    /// `args`, then the settings file every Claude Code turn is given, and
    /// the folder of its role layers.
    fn with_settings(mut args: Vec<String>) -> Vec<String> {
        args.extend(strings(&["--settings", "<settings-file>", "--add-dir", "<instructions-dir>"]));
        args
    }

    #[test]
    fn golden_headless_claude_resume() {
        const ID: &str = "golden-bg-claude-resume";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Claude);
        e.spec.mode = "default".into();
        // Afresh, in the folder it was sent to.
        assert_eq!(turn(&e, ID, &r).args, with_settings(strict(claude_turn("default"))));

        // A conversation, in the folder it was sent to.
        e.session = Some("s-1".into());
        let mut want = claude_turn("default");
        want.extend(strings(&["--resume", "s-1"]));
        let want = with_settings(strict(want));
        let t = turn(&e, ID, &r);
        assert_eq!((t.args, t.dir.as_str()), (want.clone(), "C:\\p"));

        // Where its first turn said it worked, while that is there.
        let wt = std::env::temp_dir().join(format!("luna-golden-bg-wt-{}", std::process::id())).join("luna-x");
        std::fs::create_dir_all(&wt).unwrap();
        e.cwd = Some(wt.to_string_lossy().into_owned());
        let t = turn(&e, ID, &r);
        assert_eq!((t.args, t.dir), (want, wt.to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(wt.parent().unwrap());

        // Gone: resumed from the folder it was sent to.
        let t = turn(&e, ID, &r);
        let mut want = claude_turn("default");
        want.extend(strings(&["--resume", "s-1"]));
        assert_eq!((t.args, t.dir.as_str()), (with_settings(strict(want)), "C:\\p"));
    }

    #[test]
    fn golden_headless_claude_helper_with_tools_gets_luna_mcp_and_asks_on_stdio() {
        const ID: &str = "golden-bg-claude-helper";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Claude);
        e.spec.mode = "default".into();
        e.spec.tools = true;
        e.spec.token = helper_token(ID, &e);
        e.spec.parent = Some("golden-parent".into());
        e.spec.read_only = true;
        e.spec.agent = Some("reviewer".into());
        let t = turn(&e, ID, &r);
        let mut want = claude_turn("default");
        want.extend(strings(&[
            "--agent",
            "reviewer",
            "--strict-mcp-config",
            "--permission-prompt-tool",
            "stdio",
            "--mcp-config",
            "<mcp-file>",
            "--allowedTools",
            golden::CLAUDE_ALLOWED_LUNA_TOOLS,
            "--disallowedTools",
            "Edit,Write,NotebookEdit",
            "--add-dir",
            "<notes-dir>",
            "--settings",
            "<settings-file>",
            "--add-dir",
            "<instructions-dir>",
        ]));
        assert_eq!(t.args, want);
        assert_eq!(t.instructions.as_deref(), Some(golden::role_layers(crate::library::Role::Lead).as_str()));
        let mut envs = pairs(&[("LUNA_MCP_TOKEN", "<token>")]);
        envs.extend(claude_turn_envs(ID));
        envs.extend(pairs(&[("LUNA_PARENT_ID", "golden-parent")]));
        assert_eq!(t.envs, envs);
        let mut settings = golden::claude_helper_settings(golden::claude_settings_alone());
        // Its role layers read as memory of a folder added: the other added
        // folders' memory is not.
        settings["claudeMdExcludes"] = json!([
            "<notes-dir>/CLAUDE.md",
            "<notes-dir>/CLAUDE.local.md",
            "<notes-dir>/.claude/CLAUDE.md",
            "<notes-dir>/.claude/rules/**"
        ]);
        assert_eq!(t.settings, Some(settings), "no hooks, no renderer: a helper has no screen");
        assert_eq!(t.mcp, Some(golden::claude_mcp_config()));

        // No tools of its own: no server of Luna's either, a token or not,
        // as for Codex; its asks still come over stdio.
        e.spec.tools = false;
        e.spec.parent = None;
        e.spec.read_only = false;
        e.spec.agent = None;
        let t = turn(&e, ID, &r);
        assert_eq!(t.args, with_settings(strict(claude_turn("default"))));
        assert!(!t.envs.iter().any(|(k, _)| k == "LUNA_MCP_TOKEN"), "{:?}", t.envs);
        let alone = golden::claude_helper_settings(golden::claude_settings_alone());
        assert_eq!(t.settings, Some(alone), "neither tools nor a parent: the core's deny and its memory alone");

        // Spawned with `connectors`: the account's servers and connectors too, its
        // auto-memory still off.
        e.spec.connectors = true;
        let t = turn(&e, ID, &r);
        assert!(!t.args.iter().any(|a| a == "--strict-mcp-config"), "{:?}", t.args);
        let mut kept = golden::claude_settings_alone();
        kept["autoMemoryEnabled"] = json!(false);
        kept["env"] = json!({ "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1" });
        assert!(t.envs.contains(&("CLAUDE_CODE_DISABLE_AUTO_MEMORY".to_string(), "1".to_string())), "{:?}", t.envs);
        assert_eq!(t.settings, Some(kept));
        crate::agents::forget(ID);
    }

    /// A Codex turn's arguments up to its sandbox: the server, and its folder
    /// trusted for the run.
    fn codex_turn(folder: &str, r: &Redact) -> Vec<String> {
        let mut args = strings(&["app-server"]);
        args.extend(crate::codex::trust::trust_override(folder).iter().map(|a| r.text(a)));
        args
    }

    /// Held by the sandbox `mode`, its approval its config's own; then
    /// Codex's own update check kept off, and its memories, connectors or not.
    fn codex_held(sandbox: Vec<String>, windows: bool) -> Vec<String> {
        let mut args = sandbox;
        if windows {
            args.extend(golden::codex_windows_sandbox());
        }
        args.extend(strings(&["-c", "check_for_update_on_startup=false"]));
        args.extend(strings(&[
            "-c",
            "features.memories=false",
            "-c",
            "memories.use_memories=false",
            "-c",
            "memories.generate_memories=false",
        ]));
        args
    }

    /// The account's apps off for the run, each server its layers name (the
    /// golden account has none), and plugins no layer names.
    fn apps_off() -> Vec<String> {
        strings(&["-c", "features.apps=false", "-c", "plugins={ _default = { enabled = false } }"])
    }

    /// A thread request's typed read-only: the plain mode, or where Luna
    /// passes it as its own profile, that profile by its name.
    fn typed_read_only(open: &mut Value) {
        match golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]).iter().any(|a| a.starts_with("sandbox_mode=")) {
            true => open["params"]["sandbox"] = json!("read-only"),
            false => open["params"]["permissions"] = json!("<profile>"),
        }
    }

    fn codex_turn_envs(id: &str, sandboxed: bool) -> Vec<(String, String)> {
        let mut envs = if sandboxed { golden::codex_sandbox_path() } else { Vec::new() };
        envs.extend(pairs(&[("CODEX_HOME", "C:\\acc"), ("LUNA_SESSION_ID", id)]));
        envs
    }

    #[test]
    fn golden_headless_codex_turn_is_an_app_server_opening_its_thread() {
        const ID: &str = "golden-bg-codex";
        let r = Redact::for_chat(ID);
        let e = entry(Provider::Codex);
        let t = turn(&e, ID, &r);
        let mut want = codex_turn("C:\\p", &r);
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        want.extend(apps_off());
        assert_eq!(t.args, want);
        assert_eq!(t.dir, "C:\\p");
        assert_eq!(t.envs, codex_turn_envs(ID, true));
        let mut open =
            json!({ "method": "thread/start", "params": { "cwd": "C:\\p", "model": "haiku", "developerInstructions": "<role-layers>" } });
        typed_read_only(&mut open);
        assert_eq!(t.open, Some(open));
        assert_eq!(t.role.as_deref(), Some(golden::role_layers(crate::library::Role::Helper).as_str()), "its config sets none");
        assert_eq!(t.value("effort"), Some("effort low of turn/start"));
    }

    #[test]
    fn golden_headless_codex_resume() {
        const ID: &str = "golden-bg-codex-resume";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Codex);
        e.spec.model = None;
        let mut want = codex_turn("C:\\p", &r);
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        want.extend(apps_off());
        let t = turn(&e, ID, &r);
        assert_eq!(t.args, want);
        assert_eq!(t.open.unwrap()["method"], "thread/start");
        e.session = Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".into());
        let t = turn(&e, ID, &r);
        assert_eq!((&t.args, t.dir.as_str()), (&want, "C:\\p"), "the same server, whatever the turn");
        let mut open = json!({ "method": "thread/resume", "params": {
            "threadId": "0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b", "cwd": "C:\\p", "developerInstructions": "<role-layers>"
        } });
        typed_read_only(&mut open);
        assert_eq!(t.open, Some(open));
        // Where its first turn said it worked, while that is there.
        let wt = std::env::temp_dir().join(format!("luna-golden-bg-codex-wt-{}", std::process::id()));
        std::fs::create_dir_all(&wt).unwrap();
        let wt = wt.to_string_lossy().into_owned();
        e.cwd = Some(wt.clone());
        let t = turn(&e, ID, &r);
        assert_eq!((t.args, t.dir.as_str()), (want, wt.as_str()));
        assert_eq!(t.open.unwrap()["params"]["cwd"], wt.as_str());
        let _ = std::fs::remove_dir_all(&wt);
    }

    /// A folder outside git needs no word of Luna's: the server opens a
    /// thread there as in a repository (0.169.0, seen live).
    #[test]
    fn a_codex_turn_is_the_same_in_and_outside_a_repository() {
        const ID: &str = "golden-bg-codex-git";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Codex);
        let outside = turn(&e, ID, &r);
        assert!(!outside.args.iter().any(|a| a.contains("git")), "{:?}", outside.args);
        let repo = std::env::temp_dir().join(format!("luna-bg-git-{}", std::process::id()));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        e.spec.folder = repo.join("sub").to_string_lossy().into_owned();
        let r = r.with(e.spec.folder.clone(), "<sub>");
        let t = turn(&e, ID, &r);
        assert_eq!(t.args, [codex_turn(&e.spec.folder, &r), outside.args[codex_turn("C:\\p", &r).len()..].to_vec()].concat());
        assert_eq!(t.value("helper.trust").map(|v| v.starts_with("-c projects=")), Some(true), "trusted for the run, nothing written");
        assert_eq!(t.value("no-self-update"), Some("-c check_for_update_on_startup=false"));
        assert_eq!(
            t.value("helper.no-auto-memory"),
            Some("-c features.memories=false, -c memories.use_memories=false, -c memories.generate_memories=false")
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn a_codex_turn_keeps_a_windows_sandbox_its_user_chose() {
        const ID: &str = "golden-bg-codex-own-sandbox";
        let account = std::env::temp_dir().join(format!("luna-bg-winsandbox-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::write(account.join("config.toml"), "[windows]\nsandbox = \"elevated\"\n").unwrap();
        let mut e = entry(Provider::Codex);
        e.spec.account_path = account.to_string_lossy().into_owned();
        let r = Redact::for_chat(ID).with(e.spec.account_path.clone(), "<acc>");
        let t = turn(&e, ID, &r);
        assert!(!t.args.iter().any(|a| a.starts_with("windows.sandbox")), "{:?}", t.args);
        let _ = std::fs::remove_dir_all(&account);
    }

    /// A helper whose spawn names no mode, as the spawn settles it
    /// (agents/spawn `background_spec`): Codex's own from its config gets no
    /// sandbox or approval flag; an account Luna cannot read, the closed
    /// read-only and on-request, passed. With no configured mode, both CLIs
    /// get their review mode.
    #[test]
    fn golden_headless_turns_whose_spawn_named_no_mode() {
        crate::codex::config::use_empty_machine_layers_for_test();
        let account = std::env::temp_dir().join(format!("luna-golden-bg-own-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let acc = account.to_string_lossy().into_owned();
        let settled = |config: &str, provider: Provider| {
            std::fs::write(account.join("config.toml"), config).unwrap();
            crate::agents::spawn::background_spec_for_test(provider, &acc, "C:\\p")
        };
        let closed =
            [strings(&["-c", "approval_policy=\"on-request\""]), golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""])].concat();
        for (config, held) in [
            (
                "",
                codex_held(
                    strings(&[
                        "-c",
                        "approvals_reviewer=\"auto_review\"",
                        "-c",
                        "approval_policy=\"on-request\"",
                        "-c",
                        "sandbox_mode=\"workspace-write\"",
                    ]),
                    true,
                ),
            ),
            ("sandbox_mode = \"danger-full-access\"\n", codex_held(Vec::new(), true)),
            ("approval_policy = \"sometimes\"\n", codex_held(closed, true)),
        ] {
            let id = "golden-bg-codex-own";
            let r = Redact::for_chat(id);
            let mut e = entry(Provider::Codex);
            let spec = settled(config, Provider::Codex);
            (e.spec.mode, e.spec.own_mode, e.spec.approval, e.spec.auto_review) =
                (spec.mode, spec.own_mode, spec.approval, spec.auto_review);
            let t = turn(&e, id, &r);
            let mut want = codex_turn("C:\\p", &r);
            want.extend(held);
            want.extend(apps_off());
            assert_eq!(t.args, want, "{config}");
            assert_eq!(t.envs, codex_turn_envs(id, true), "{config}");
        }
        if crate::claude::defaults::managed_mode_for_test() {
            let _ = std::fs::remove_dir_all(&account);
            return;
        }
        let id = "golden-bg-claude-own";
        let r = Redact::for_chat(id);
        let mut e = entry(Provider::Claude);
        let spec = settled("", Provider::Claude);
        (e.spec.mode, e.spec.own_mode) = (spec.mode, spec.own_mode);
        let t = turn(&e, id, &r);
        assert!(t.args.windows(2).any(|a| a == ["--permission-mode", "auto"]), "{:?}", t.args);
        assert_eq!(t.value("permission-mode"), Some("--permission-mode auto"));

        std::fs::write(account.join("settings.json"), r#"{ "permissions": { "defaultMode": "plan" } }"#).unwrap();
        let spec = settled("", Provider::Claude);
        (e.spec.mode, e.spec.own_mode) = (spec.mode, spec.own_mode);
        let t = turn(&e, id, &r);
        assert!(!t.args.iter().any(|a| a == "--permission-mode"), "{:?}", t.args);
        assert!(!t.keys().contains(&"permission-mode"), "{:?}", t.keys());
        let _ = std::fs::remove_dir_all(&account);
    }

    #[test]
    fn golden_headless_codex_sandboxes_and_the_reviewer() {
        // (sandbox, auto review, what holds it, whether Windows sandboxes it)
        let read_only_net = codex_held(golden::codex_read_only_net(), true);
        let reviewer = strings(&[
            "-c",
            "approvals_reviewer=\"auto_review\"",
            "-c",
            "approval_policy=\"on-request\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
        ]);
        let cases: [(&str, bool, Vec<String>, bool); 4] = [
            ("workspace-write", false, codex_held(strings(&["-c", "sandbox_mode=\"workspace-write\""]), true), true),
            ("danger-full-access", false, codex_held(strings(&["-c", "sandbox_mode=\"danger-full-access\""]), false), false),
            ("read-only-net", false, read_only_net, true),
            ("workspace-write", true, codex_held(reviewer, true), true),
        ];
        for (mode, auto_review, held, sandboxed) in cases {
            let id = format!("golden-bg-codex-{mode}-{auto_review}");
            let r = Redact::for_chat(&id);
            let mut e = entry(Provider::Codex);
            e.spec.mode = mode.into();
            e.spec.auto_review = auto_review;
            for session in [None, Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b".to_string())] {
                e.session = session;
                let t = turn(&e, &id, &r);
                let mut want = codex_turn("C:\\p", &r);
                want.extend(held.clone());
                want.extend(apps_off());
                assert_eq!(t.args, want, "{mode} {auto_review}");
                let mut envs = codex_turn_envs(&id, sandboxed);
                if mode == crate::codex::launch::READ_ONLY_NET {
                    // git's https, which schannel cannot do there: after the
                    // sandbox's PATH.
                    envs.splice(1..1, golden::codex_git_ssl());
                }
                assert_eq!(t.envs, envs, "{mode} {auto_review}");
                if auto_review {
                    assert_eq!(t.value("approval"), Some("-c approvals_reviewer=\"auto_review\" -c approval_policy=\"on-request\""));
                    assert_eq!(t.value("sandbox"), Some("-c sandbox_mode=\"workspace-write\""));
                }
            }
        }
    }

    #[test]
    fn golden_headless_codex_helper_with_tools_gets_luna_mcp() {
        const ID: &str = "golden-bg-codex-helper";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.approval = Some("on-request".into());
        e.spec.tools = true;
        e.spec.token = helper_token(ID, &e);
        e.spec.parent = Some("golden-parent".into());
        let t = turn(&e, ID, &r);
        let mut want = codex_turn("C:\\p", &r);
        want.extend(codex_held(strings(&["-c", "approval_policy=\"on-request\"", "-c", "sandbox_mode=\"workspace-write\""]), true));
        want.extend(apps_off());
        want.extend(golden::codex_mcp(true));
        assert_eq!(t.args, want);
        let mut envs = golden::codex_sandbox_path();
        envs.extend(pairs(&[
            ("LUNA_MCP_TOKEN", "<token>"),
            ("CODEX_HOME", "C:\\acc"),
            ("LUNA_SESSION_ID", ID),
            ("LUNA_PARENT_ID", "golden-parent"),
        ]));
        assert_eq!(t.envs, envs);
        assert_eq!(t.value("approval"), Some("-c approval_policy=\"on-request\""));
        assert!(t.role.as_deref().is_some_and(|l| l.contains("You may start helpers of your own")), "a lead's role");

        for (approval, auto_review, prompt) in [(Some("never"), false, false), (None, false, true), (None, true, true)] {
            e.spec.approval = approval.map(str::to_owned);
            e.spec.auto_review = auto_review;
            let t = turn(&e, ID, &r);
            assert_eq!(
                t.args.iter().filter(|a| a.starts_with("mcp_servers.luna.")).collect::<Vec<_>>(),
                golden::codex_mcp(prompt).iter().filter(|a| *a != "-c").collect::<Vec<_>>(),
                "{approval:?} {auto_review}",
            );
        }

        e.spec.approval = Some("on-request".into());
        e.spec.auto_review = false;
        // A token without tools gives no server: what Codex asks comes as the
        // server's requests, not through Luna's tools.
        e.spec.tools = false;
        let t = turn(&e, ID, &r);
        assert!(!t.args.iter().any(|a| a.starts_with("mcp_servers.")), "{:?}", t.args);
        assert!(!t.envs.iter().any(|(k, _)| k == "LUNA_MCP_TOKEN"));
        // Spawned with `connectors`: its account's apps and servers stay on,
        // its memories still off.
        e.spec.connectors = true;
        let t = turn(&e, ID, &r);
        let mut want = codex_turn("C:\\p", &r);
        want.extend(codex_held(strings(&["-c", "approval_policy=\"on-request\"", "-c", "sandbox_mode=\"workspace-write\""]), true));
        assert_eq!(t.args, want);
        e.spec.tools = true;
        let t = turn(&e, ID, &r);
        want.extend(golden::codex_mcp(true));
        assert_eq!(t.args, want);
        crate::agents::forget(ID);
    }

    #[test]
    fn golden_headless_codex_helper_turns_off_the_servers_and_plugins_of_its_run() {
        const ID: &str = "golden-bg-codex-servers";
        let account = std::env::temp_dir().join(format!("luna-golden-codex-servers-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let config = "[mcp_servers.gmail]\ncommand = \"node\"\n\n\
                      [mcp_servers.luna]\nurl = \"https://y\"\n\n[mcp_servers.\"team.slack\"]\nurl = \"https://z\"\n\n\
                      [plugins.\"crm@market\"]\nenabled = true\n";
        std::fs::write(account.join("config.toml"), config).unwrap();
        let account = account.to_string_lossy().into_owned();
        let r = Redact::for_chat(ID).with(account.clone(), "<acc>");
        let mut e = entry(Provider::Codex);
        e.spec.account_path = account.clone();
        let t = turn(&e, ID, &r);
        let mut want = codex_turn("C:\\p", &r);
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        // One the account calls `luna` is its own, not Luna's, which this helper
        // without tools is not given: off as well. A name with a dot is a
        // quoted key in the table, not split.
        want.extend(strings(&[
            "-c",
            "features.apps=false",
            "-c",
            "mcp_servers={ gmail = { enabled = false }, luna = { enabled = false }, \"team.slack\" = { enabled = false } }",
            "-c",
            "plugins={ \"crm@market\" = { enabled = false }, _default = { enabled = false } }",
        ]));
        assert_eq!(t.args, want);
        assert!(t.value("helper.luna-mcp-alone").is_some_and(|v| v.contains("team.slack")));
        // A project's servers go off as the account's do.
        let project = std::path::Path::new(&account).join("project");
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        std::fs::write(project.join(".codex").join("config.toml"), "[mcp_servers.docs]\ncommand = \"x\"\n").unwrap();
        let mut in_project = entry(Provider::Codex);
        in_project.spec.account_path = account.clone();
        in_project.spec.folder = project.to_string_lossy().into_owned();
        let (args, _, _, _) = command_for(&in_project, ID).unwrap();
        assert!(args.iter().any(|a| a.starts_with("mcp_servers=") && a.contains("docs = { enabled = false }")), "{args:?}");
        // With tools, Luna's own is `luna`: the account's, a url alone, is replaced
        // whole and turned on, and Luna's keys come after the table, which
        // would otherwise replace them; one with more of its own stops the start.
        e.spec.tools = true;
        e.spec.token = helper_token(ID, &e);
        let t = turn(&e, ID, &r);
        let table = t.args.iter().position(|a| a.starts_with("mcp_servers={")).unwrap();
        assert!(t.args[table].contains("luna = { enabled = true }"), "{:?}", t.args);
        assert!(t.args.iter().skip(table + 1).any(|a| a.starts_with("mcp_servers.luna.url=")), "{:?}", t.args);
        assert!(t.args.ends_with(&golden::codex_mcp(true)), "{:?}", t.args);
        std::fs::write(std::path::Path::new(&account).join("config.toml"), format!("approval_policy = \"never\"\n{config}")).unwrap();
        for (approval, thread, prompt) in [
            (None, None, false),
            (None, Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"), true),
            (Some("never"), Some("0198a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"), false),
        ] {
            e.spec.approval = approval.map(str::to_owned);
            e.session = thread.map(str::to_owned);
            let t = turn(&e, ID, &r);
            assert!(t.args.ends_with(&golden::codex_mcp(prompt)), "{approval:?} {thread:?}: {:?}", t.args);
            assert_eq!(t.open.unwrap()["params"]["approvalPolicy"].as_str(), approval);
        }
        std::fs::write(std::path::Path::new(&account).join("config.toml"), "[mcp_servers.luna]\ncommand = \"node\"\n").unwrap();
        let refused = command_for(&e, ID).err().unwrap();
        assert!(refused.contains("named luna") && refused.contains("rename that server"), "{refused}");
        // With its connectors it is given Luna's server all the same, and the
        // account's `luna` would merge into it just as well: refused too, as is
        // one turning it off, which nothing turns on for it there.
        e.spec.connectors = true;
        assert!(command_for(&e, ID).err().unwrap().contains("named luna"));
        std::fs::write(std::path::Path::new(&account).join("config.toml"), "[mcp_servers.luna]\nenabled = false\n").unwrap();
        assert!(command_for(&e, ID).err().unwrap().contains("its enabled would be merged"));
        e.spec.connectors = false;
        assert!(command_for(&e, ID).is_ok(), "without connectors it is turned on");
        e.spec.tools = false;
        e.spec.token = None;
        crate::agents::forget(ID);
        // A config that will not parse: no turn, rather than one with its servers on.
        std::fs::write(std::path::Path::new(&account).join("config.toml"), "[mcp_servers.gmail\n").unwrap();
        let e = command_for(&e, ID).err().unwrap();
        assert!(e.contains("not started"), "{e}");
        // Spawned with them, it never needed the list.
        let mut kept = entry(Provider::Codex);
        kept.spec.account_path = account.clone();
        kept.spec.connectors = true;
        assert!(command_for(&kept, ID).is_ok());
        let _ = std::fs::remove_dir_all(&account);
    }

    /// Its folder's skills are a plugin of the helper's own, made afresh for
    /// the turn, and gone with the helper.
    #[test]
    fn golden_headless_claude_turn_in_a_library_folder_gets_its_skills_and_instructions() {
        const ID: &str = "golden-bg-claude-library";
        let lib = crate::library::GoldenLibrary::new("bg-claude");
        let (mut e, r) = library_entry(ID, &lib, Provider::Claude);
        e.spec.mode = "default".into();
        let t = turn(&e, ID, &r);
        let mut want = strict(claude_turn("default"));
        want.extend(strings(&["--settings", "<settings-file>", "--add-dir", "<instructions-dir>", "--plugin-dir", "<plugin-dir>"]));
        assert_eq!(t.args, want);
        assert_eq!(t.dir, "<project>");
        let mut settings = golden::claude_helper_settings(golden::claude_settings_alone());
        settings["skillOverrides"] = json!({ "review": "off" });
        settings["claudeMdExcludes"] = json!(["<acc>/CLAUDE.md"]);
        assert_eq!(t.settings, Some(settings), "no hooks: what the folder keeps from it alone, and no folder of skills to leave out");
        assert_eq!(t.value("library.skills"), Some("--plugin-dir <plugin-dir>"));
        let plugin = crate::paths::data_dir().join("session-skills").join(crate::paths::id_file_name(ID));
        assert!(plugin.join("golden/skills/notes/SKILL.md").is_file());
        crate::library::drop_plugins(ID);
        assert!(!plugin.exists());
    }

    #[test]
    fn golden_headless_codex_turn_in_a_library_folder_gets_its_plugin_and_keeps_its_skills_off() {
        const ID: &str = "golden-bg-codex-library";
        let lib = crate::library::GoldenLibrary::new("bg-codex");
        let (e, r) = library_entry(ID, &lib, Provider::Codex);
        let t = turn(&e, ID, &r);
        let mut want = codex_turn(&lib.folder, &r);
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        want.extend(apps_off());
        want.extend(strings(&[
            "-c",
            &format!("plugins.{}@rm-luna.enabled=true", crate::library::GoldenLibrary::SLUG),
            "-c",
            // By the path of its `SKILL.md`, not by name.
            "skills.config=[{path=\"<acc>/skills/review/SKILL.md\",enabled=false}]",
        ]));
        assert_eq!(t.args, want);
        assert_eq!(t.dir, "<project>");
        let mut envs = golden::codex_sandbox_path();
        envs.extend(pairs(&[("CODEX_HOME", "<acc>"), ("LUNA_SESSION_ID", ID)]));
        assert_eq!(t.envs, envs);
        assert_eq!((t.settings, t.instructions), (None, None), "Codex has no way to take other instructions");
        assert_eq!(t.open.unwrap()["params"]["cwd"], "<project>");
    }

    // A helper's turn on a library whose every entry is the person's chats'
    // (library/audience.rs): kept from all of it that its CLI can keep. The
    // person's chats on the same library: pty/start.rs.

    #[test]
    fn golden_headless_claude_turn_is_kept_from_what_the_library_means_for_chats() {
        const ID: &str = "golden-bg-claude-chats-only";
        let lib = crate::library::GoldenLibrary::chats_only("bg-claude-audience");
        let (mut e, r) = library_entry(ID, &lib, Provider::Claude);
        e.spec.mode = "default".into();
        let t = turn(&e, ID, &r);
        assert_eq!(t.args, with_settings(strict(claude_turn("default"))), "nothing added in their place");
        assert_eq!(t.dir, "<project>");
        let mut settings = golden::claude_helper_settings(golden::claude_settings_alone());
        settings["skillOverrides"] = json!({ "report": "off" });
        settings["claudeMdExcludes"] = json!(["<acc>/CLAUDE.md"]);
        assert_eq!(t.settings, Some(settings));
        assert_eq!(t.instructions.as_deref(), Some(golden::role_layers(crate::library::Role::Helper).as_str()), "its role alone");
        assert_eq!(t.value("library.audience"), Some("instructions main (chats only); skill report (chats only)"));
        assert_eq!(t.value("library.hidden-skills"), Some("skillOverrides off: report"));
        assert_eq!(t.value("library.account-instructions-off"), Some("claudeMdExcludes <acc>/CLAUDE.md"));
    }

    #[test]
    fn golden_headless_codex_turn_is_kept_from_skills_meant_for_chats_and_told_whose_its_instructions_are() {
        const ID: &str = "golden-bg-codex-chats-only";
        let lib = crate::library::GoldenLibrary::chats_only("bg-codex-audience");
        let (e, r) = library_entry(ID, &lib, Provider::Codex);
        let t = turn(&e, ID, &r);
        let mut want = codex_turn(&lib.folder, &r);
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        want.extend(apps_off());
        want.extend(strings(&["-c", "skills.config=[{path=\"<acc>/skills/report/SKILL.md\",enabled=false}]"]));
        assert_eq!(t.args, want);
        assert_eq!(
            t.value("library.audience"),
            Some("instructions main (chats only): AGENTS.md says so, as Codex reads it in every session; skill report (chats only)")
        );
        assert_eq!(t.value("library.hidden-skills"), Some("-c skills.config=[{path=\"<acc>/skills/report/SKILL.md\",enabled=false}]"));
    }
}
