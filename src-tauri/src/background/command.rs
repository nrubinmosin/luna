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
/// F3) — cannot be written, or a Codex turn's role layers do not fit on its
/// command line: the turn refuses rather than run without them.
pub(super) fn command_with(e: &Entry, id: &str, texts: &crate::library::RoleTexts) -> Result<Command, String> {
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
    // A worktree the first turn made, while it is there; Claude Code may
    // drop a clean one on exit, and is then asked for it again by name.
    let made = e.cwd.clone().filter(|c| !crate::pty::same_dir(c, &s.folder));
    let mut dir = s.folder.clone();
    // The guard against endless waits (guard.rs), a helper's as any.
    let guard = s.tools || s.parent.is_some();
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
            args.extend(["--permission-mode".into(), s.mode.clone()]);
            applied.push(Item::PermissionMode.with(format!("--permission-mode {}", s.mode)));
            match (&e.session, &made) {
                (Some(sid), Some(wt)) if std::path::Path::new(wt).is_dir() => {
                    dir = wt.clone();
                    args.extend(["--resume".into(), sid.clone()]);
                }
                (Some(sid), Some(wt)) => {
                    let name = std::path::Path::new(wt).file_name().map(|n| n.to_string_lossy().into_owned());
                    // By a name that will do, or it stays where it is: no worktree of
                    // any other name is the one this conversation was in.
                    if let Some(name) = name.filter(|n| crate::worktree::name_ok(n)) {
                        args.push(crate::worktree::worktree_arg(Some(&name)));
                    }
                    args.extend(["--resume".into(), sid.clone()]);
                }
                (Some(sid), None) => args.extend(["--resume".into(), sid.clone()]),
                (None, _) if s.worktree => args.push(crate::worktree::worktree_arg(s.worktree_name.as_deref())),
                (None, _) => {}
            }
            if let Some(sid) = &e.session {
                applied.push(Item::Resume.with(format!("--resume {sid}")));
            }
            if let Some(wt) = args.iter().find(|a| a.starts_with("--worktree")) {
                applied.push(Item::Worktree.with(wt));
            }
            if let Some(agent) = &s.agent {
                args.extend(["--agent".into(), agent.clone()]);
                applied.push(Item::Agent.with(format!("--agent {agent}")));
            }
            let mut denied: Vec<&str> = if s.read_only { crate::claude::launch::CLAUDE_EDIT_TOOLS.to_vec() } else { Vec::new() };
            if s.read_only {
                applied.push(Item::ReadOnly.with(format!("--disallowedTools {}", denied.join(","))));
            }
            // Luna's server alone, unless spawned with the account's own; what
            // that keeps it to is applied with the settings file that holds it.
            let helper = crate::claude::launch::HelperSettings { connectors: s.connectors };
            if !s.connectors {
                args.push(crate::claude::launch::STRICT_MCP.into());
            }
            if let Some((mcp, token)) = s.token.clone().and_then(|t| Some((crate::pty::claude_mcp_file(id)?, t))) {
                let mcp = mcp.to_string_lossy().into_owned();
                args.extend(["--mcp-config".into(), mcp.clone()]);
                envs.push((crate::claude::launch::CLAUDE_MCP_TOKEN_ENV, token));
                args.extend(["--permission-prompt-tool".into(), "mcp__luna__luna_permission".into()]);
                applied.push(
                    Item::HelperPermissionPrompt.with(format!("--mcp-config {mcp}, --permission-prompt-tool mcp__luna__luna_permission")),
                );
                // The CLI's to call, not the model's: hidden from its tools,
                // and the prompts still come through.
                denied.push("mcp__luna__luna_permission");
                if s.tools {
                    args.extend(["--allowedTools".into(), "mcp__luna".into()]);
                    applied.push(Item::LunaTools.with(crate::claude::launch::luna_tools_value(&mcp)));
                }
            }
            if !denied.is_empty() {
                args.extend(["--disallowedTools".into(), denied.join(",")]);
                if s.token.is_some() {
                    applied
                        .push(Item::HelperPermissionPrompt.with(format!("Luna's own tool hidden: --disallowedTools {}", denied.join(","))));
                }
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
            // (library/folders.rs `session_args`). No hooks: its stream says it all; but
            // the guard against endless waits, a helper's as any (guard.rs).
            let mut skills = crate::library::session_args(Provider::Claude, &s.account_path, &dir, role);
            // Its role layers, written afresh every turn: an edit reaches it
            // at its next (library/role.rs).
            let layers = crate::library::role_layers(texts, &skills, &constraints);
            let instructions = crate::pty::claude_instructions_dir(id, layers.as_deref());
            let mut luna_dirs: Vec<String> = skills.args.windows(2).filter(|w| w[0] == "--add-dir").map(|w| w[1].clone()).collect();
            luna_dirs.extend(notes);
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
            let (settings, holds) = crate::pty::claude_settings_file(id, false, guard.then_some(Item::HelperGuard), &skills, Some(helper))?;
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
            args.extend(skills.args);
            envs.push(("CLAUDE_CONFIG_DIR", s.account_path.clone()));
            envs.push(("DISABLE_AUTOUPDATER", "1".into()));
            applied.push(Item::NoAutoUpdate.with("DISABLE_AUTOUPDATER=1"));
            // Never inherited: a `0` there turns auto-memory on (`HelperSettings`).
            envs.push((crate::claude::launch::NO_AUTO_MEMORY_ENV, "1".into()));
            // Its stdin closes with each turn's result (events.rs), and from then
            // `claude -p` waits only so long for what the turn left in the
            // background before it stops it, subagents and their uncommitted
            // work included (2.1.319+, 10 min by default). The turn is the
            // helper's until then: one that waits that long is told to the
            // parent as a turn gone quiet, with what it waits on
            // (agents/notifier `QUIET_TURN_MS`), and ends with
            // `luna_session_stop`. Set always, over anything inherited.
            envs.push((BG_WAIT_CEILING_ENV, BG_WAIT_CEILING_MS.into()));
            applied.push(Item::HelperBackgroundWait.with(format!("{BG_WAIT_CEILING_ENV}={BG_WAIT_CEILING_MS}")));
        }
        Provider::Codex => {
            args.push("exec".into());
            applied.push(Item::HelperHeadless.with("exec --json, the prompt on stdin (-)"));
            match &e.session {
                Some(thread) => {
                    args.extend(["resume".into(), thread.clone()]);
                    applied.push(Item::Resume.with(format!("resume {thread}")));
                    if let Some(wt) = made.filter(|w| std::path::Path::new(w).is_dir()) {
                        dir = wt;
                    }
                }
                None => {
                    args.extend(["-C".into(), s.folder.clone()]);
                    applied.push(Item::Folder.with(format!("-C {}", s.folder)));
                    if s.worktree {
                        args.push("--worktree".into());
                        applied.push(Item::Worktree.with("--worktree"));
                    }
                }
            }
            args.push("--json".into());
            // `exec` refuses a folder outside git without it; in a repository
            // it is not needed, and the check is Codex's to make.
            if !in_git(&dir) {
                args.push("--skip-git-repo-check".into());
                applied.push(Item::SkipGitRepoCheck.with("--skip-git-repo-check"));
            }
            // `exec` cannot ask for the folder's trust and goes without the
            // project's config and hooks if it lacks it; a helper is where it
            // was sent, trusted for the run (codex/trust.rs).
            if let Some((trust, item)) = crate::pty::codex_trust(&s.account_path, &s.folder, true) {
                applied.push(item.with(format!("-c {}", trust[1])));
                args.extend(trust);
            }
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
                applied.push(Item::Model.with(format!("--model {m}")));
            }
            args.extend(["-c".into(), format!("model_reasoning_effort=\"{}\"", s.effort)]);
            applied.push(Item::Effort.with(format!("-c model_reasoning_effort=\"{}\"", s.effort)));
            if s.hooks_unasked {
                args.push(crate::codex::launch::HOOKS_UNASKED.into());
                applied.push(Item::HooksUnasked.with(crate::codex::launch::HOOKS_UNASKED));
            }
            let guard_args = crate::pty::codex_guard_args(id, guard, s.hooks_unasked);
            if !guard_args.is_empty() {
                applied.push(crate::codex::launch::guard_applied(true));
            }
            args.extend(guard_args);
            if s.auto_review && e.session.is_some() {
                // `exec resume` refuses `--approve-for-me` (0.167.0: "unexpected
                // argument") but takes `-c`: the overrides the flag stands for, given
                // each turn rather than left to what the thread may have kept.
                for o in crate::codex::launch::AUTO_REVIEW_OVERRIDES {
                    args.extend(["-c".into(), o.to_string()]);
                }
                let [reviewer, policy, sandbox] = crate::codex::launch::AUTO_REVIEW_OVERRIDES;
                applied.push(Item::Approval.with(format!("-c {reviewer} -c {policy}")));
                applied.push(Item::Sandbox.with(format!("-c {sandbox}")));
            } else if s.auto_review {
                args.push("--approve-for-me".into());
                applied.push(Item::Approval.with("--approve-for-me"));
                let sandbox = crate::codex::launch::AUTO_REVIEW_SANDBOX;
                applied.push(Item::Sandbox.with(format!("{sandbox}, Codex's own under --approve-for-me")));
            } else {
                // `exec` asks nobody: the sandbox is what holds it.
                args.extend(["-c".into(), "approval_policy=\"never\"".into()]);
                applied.push(Item::HelperApprovalNever.with("-c approval_policy=\"never\""));
                args.extend(["-c".into(), crate::codex::launch::CODEX_PERSON_REVIEWS.into()]);
                applied.push(Item::HelperReviewer.with(format!("-c {}", crate::codex::launch::CODEX_PERSON_REVIEWS)));
                let sandbox = crate::pty::codex_sandbox_args(&s.mode);
                applied.push(crate::codex::launch::sandbox_applied(&s.mode, &sandbox));
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
            // Luna's own server, with its tools: none while the hub is not up.
            let luna = s.token.clone().filter(|_| s.tools).map(|token| (crate::pty::codex_mcp_args(), token));
            if !s.connectors {
                let supplied = luna.as_ref().is_some_and(|(mcp, _)| !mcp.is_empty());
                let servers = crate::codex::config::helper_mcp_servers(&s.account_path)?;
                let off = crate::codex::launch::connectors_off(&servers, supplied)?;
                applied.push(crate::codex::launch::connectors_off_applied(&off));
                args.extend(off);
                let left = crate::codex::config::servers_left_on(&s.account_path, &dir);
                applied.extend(crate::codex::launch::left_on_applied(&left));
            }
            if let Some((mcp, token)) = luna {
                args.extend(mcp);
                envs.push(("LUNA_MCP_TOKEN", token));
                applied.push(Item::LunaTools.with(crate::codex::launch::LUNA_TOOLS_VALUE));
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
            // Its role layers, given every turn: Codex takes them at the
            // thread's start and after a compaction (library/role.rs). The
            // folder is trusted for the run, so its project's config counts.
            let (role_args, role_applied) = crate::pty::codex_role_args(&s.account_path, &dir, true, texts, &library, &constraints)
                .map_err(|why| format!("the helper's turn is not started: {why}; its role and constraints would not reach it"))?;
            args.extend(role_args);
            applied.extend(role_applied);
            if let (Some(_), Some(note)) = (&s.parent, &texts.note) {
                applied.push(Item::HelperIdentity.with(format!("a note after every prompt: {note}")));
            }
            // The prompt comes on stdin; Claude Code reads it there unasked.
            args.push("-".into());
        }
    }
    envs.extend(crate::pty::session_envs(id, s.parent.as_deref()));
    // No terminal: a helper's CLI writes to pipes (`run.rs`, `clean_env`).
    crate::pty::finish_spec(&mut envs, &mut applied, &dir, false);
    Ok((args, dir, envs, applied))
}

/// Whether `dir` lies in a Git checkout, as `codex exec` looks for one: a
/// `.git` (a folder, or a worktree's file) there or above.
fn in_git(dir: &str) -> bool {
    std::path::Path::new(dir).ancestors().any(|d| d.join(".git").exists())
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

    /// Auto-handoff is a person's chat's alone: a helper, tools or none, is
    /// started with no word of it, nor with its instructions as a skill.
    #[test]
    fn a_helper_is_started_with_nothing_of_the_handoff() {
        for provider in [Provider::Claude, Provider::Codex] {
            for tools in [false, true] {
                let mut e = entry(provider);
                e.spec.tools = tools;
                e.spec.parent = Some("a-chat".into());
                let (args, dir, envs, applied) = command_for(&e, "bg-no-handoff").unwrap();
                let said = format!("{args:?}{dir:?}{envs:?}{applied:?}").to_lowercase();
                for word in ["luna_session_handoff", "session-handoff", "handoffs", "auto-handoff"] {
                    assert!(!said.contains(word), "{provider:?} tools {tools}: {word} in {said}");
                }
            }
        }
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
        assert!(crate::pty::claude_settings_file(bad, true, None, &none, None).is_err(), "and a pane start would too");
        // Codex carries the deny in its sandbox profile, not a settings file,
        // so the same id does not stop it.
        assert!(command_for(&entry(Provider::Codex), bad).is_ok(), "Codex writes no settings file");
        let _ = std::fs::remove_dir_all(&blocker);
    }

    #[test]
    fn a_turn_resumes_the_conversation_the_first_one_named() {
        let mut e = entry(Provider::Codex);
        let (args, dir, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["exec", "-C", "C:\\p"]);
        assert_eq!(args.last().map(String::as_str), Some("-"), "the prompt comes on stdin");
        assert!(
            args.iter().any(|a| a == "sandbox_mode=\"read-only\"" || a.starts_with("default_permissions=\"luna-")),
            "held by its sandbox"
        );
        assert_eq!(dir, "C:\\p");
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);

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
        let (args, _, envs, _) = command_for(&e, "bg-mcp-token").unwrap();
        let at = args.iter().position(|a| a == "--mcp-config").expect("Luna's server");
        let config = std::fs::read_to_string(&args[at + 1]).unwrap();
        assert!(!config.contains("the-helpers-token"), "{config}");
        assert!(config.contains("Bearer ${LUNA_MCP_TOKEN}"), "{config}");
        assert!(envs.iter().any(|(k, v)| *k == "LUNA_MCP_TOKEN" && v == "the-helpers-token"));
        let _ = std::fs::remove_file(&args[at + 1]);
    }

    #[test]
    fn a_helpers_worktree_goes_by_the_name_luna_gave_it_never_as_an_argument_of_its_own() {
        const FLAG: &str = "--dangerously-skip-permissions";
        // Named, with `=`, from the project.
        let mut e = entry(Provider::Claude);
        e.spec.worktree = true;
        e.spec.worktree_name = Some("luna-x".into());
        let (args, dir, _, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--worktree=luna-x".to_string()) && !args.contains(&"--worktree".to_string()));
        assert_eq!(dir, "C:\\p");
        assert!(!args.iter().any(|a| a == "--resume"), "there is no conversation to resume");

        // A name that is a flag turns permissions off, as Claude Code reads it, if it
        // ever gets to be an argument: not from the spec, and not from where it worked.
        e.spec.worktree_name = Some(FLAG.into());
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--worktree".to_string()) && !args.iter().any(|a| a.contains(FLAG)), "{args:?}");
        e.session = Some("s-1".into());
        e.cwd = Some(std::path::Path::new("C:\\no\\such").join(FLAG).to_string_lossy().into_owned());
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--resume".to_string()) && !args.iter().any(|a| a.contains(FLAG)), "{args:?}");
        e.cwd = Some(std::path::Path::new("C:\\no\\such").join("luna-x").to_string_lossy().into_owned());
        assert!(
            command_for(&e, "x").unwrap().0.contains(&"--worktree=luna-x".to_string()),
            "gone from disk, Claude Code makes it again by name"
        );

        // Codex names none, and always makes its own.
        let mut e = entry(Provider::Codex);
        e.spec.worktree = true;
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["exec", "-C", "C:\\p"]);
        assert!(args.contains(&"--worktree".to_string()));

        // No core has cut off a first turn that is only just starting.
        let mut e = entry(Provider::Claude);
        e.spec.worktree = true;
        e.spec.worktree_name = Some("luna-first".into());
        lock().entries.insert("first-turn".into(), e);
        settle_worktree("first-turn");
        assert_eq!(lock().entries.remove("first-turn").unwrap().spec.worktree_name.as_deref(), Some("luna-first"));
    }

    #[test]
    fn a_codex_helpers_note_goes_with_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.parent = Some("p-1".into());
        let notes = |text: String| text.matches("[A note, not part of the task: ").count();
        assert_eq!(notes(stdin_text(&e, "the prompt")), 1, "the turn that starts its conversation");
        assert_eq!(stdin_text(&e, "/review"), "/review", "not into a command's arguments");
        e.session = Some("t-1".into());
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
        e.session = Some("s-1".into());
        assert_eq!(stdin_text(&e, "the next"), "the next", "its role layers say it there");
        e.session = None;
        e.spec.parent = None;
        assert_eq!(stdin_text(&e, "the prompt"), "the prompt", "a helper of nobody's is told nothing");
    }

    /// The text of a turn's role layers: Claude Code's `CLAUDE.md` in the
    /// folder it is given, Codex's `developer_instructions`.
    fn layers_of(provider: Provider, args: &[String]) -> Option<String> {
        match provider {
            Provider::Claude => {
                let hooks = crate::paths::data_dir().join("hooks");
                args.windows(2)
                    .filter(|w| w[0] == "--add-dir" && std::path::Path::new(&w[1]).starts_with(&hooks))
                    .find_map(|w| std::fs::read_to_string(std::path::Path::new(&w[1]).join("CLAUDE.md")).ok())
            }
            Provider::Codex => args.iter().find_map(|a| a.strip_prefix("developer_instructions=")).map(|v| {
                let doc: toml_edit::DocumentMut = format!("v = {v}").parse().unwrap();
                doc["v"].as_str().unwrap().to_string()
            }),
        }
    }

    #[test]
    fn a_helper_passed_to_another_parent_keeps_its_role_and_is_told_of_nobody_without_one() {
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.spec.parent = Some("old".into());
            e.session = Some("s-1".into());
            lock().entries.insert("moved".into(), e);
            set_parent("moved", Some("new".into()));
            let r = lock();
            let e = &r.entries["moved"];
            let (args, _, envs, _) = command_for(e, "moved").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "new"), "{provider:?}");
            assert!(layers_of(provider, &args).unwrap().contains("Your task comes from another agent"));
            let line = stdin_text(e, "go on");
            match provider {
                Provider::Claude => assert_eq!(line, "go on", "its role layers say it"),
                Provider::Codex => assert_eq!(line.matches("[A note, not part of the task: ").count(), 1, "{line}"),
            }
            drop(r);
            // With nobody to answer to, it hears no note; its role stays.
            set_parent("moved", None);
            let e = lock().entries.remove("moved").unwrap();
            let (args, _, envs, _) = command_for(&e, "moved").unwrap();
            assert!(!envs.iter().any(|(k, _)| *k == "LUNA_PARENT_ID"));
            assert!(layers_of(provider, &args).is_some());
            assert_eq!(stdin_text(&e, "go on"), "go on");
        }
    }

    #[test]
    fn read_only_net_goes_to_codex_as_a_profile_on_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = crate::codex::launch::READ_ONLY_NET.into();
        for session in [None, Some("t-1".to_string())] {
            e.session = session;
            let (args, _, envs, _) = command_for(&e, "x").unwrap();
            assert!(args.iter().any(|a| a.starts_with("default_permissions=\"luna-")));
            assert!(args.iter().any(|a| a.ends_with(".network.enabled=true")));
            assert!(!args.iter().any(|a| a.starts_with("sandbox_mode=")), "Codex's docs say not to set both");
            assert_eq!(envs.iter().any(|(k, _)| *k == "GIT_CONFIG_COUNT"), cfg!(windows), "git https, which schannel cannot do there");
            assert!(args.contains(&"approval_policy=\"never\"".to_string()));
            assert!(args.contains(&crate::codex::launch::CODEX_PERSON_REVIEWS.to_string()), "no reviewer model from a config");
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
            let mut e = entry(provider);
            e.spec.parent = Some("p-1".into());
            let (args, _, envs, applied) = command_for(&e, "x").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_SESSION_ID" && v == "x"));
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));
            assert!(!args.iter().any(|a| a == "--append-system-prompt"), "memory outweighs it: the role goes there");
            let layers = layers_of(provider, &args).unwrap_or_else(|| panic!("{provider:?}: {args:?}"));
            assert!(layers.starts_with("# Your role\n\nYour task comes from another agent"), "{layers}");
            assert!(!layers.contains("helpers of your own"), "a helper is no lead");
            assert!(applied.iter().any(|a| a.key == "role.layers"), "{provider:?}");
            if provider == Provider::Claude {
                assert!(envs.iter().any(|(k, v)| *k == crate::claude::launch::CLAUDE_ADDED_DIR_MEMORY && v == "1"));
            }
            // A lead is told of its helpers.
            e.spec.tools = true;
            let (args, _, _, _) = command_for(&e, "x").unwrap();
            assert!(layers_of(provider, &args).unwrap().contains("You may start helpers of your own"), "{provider:?}");
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
            for session in [None, Some("s-1".to_string())] {
                e.session = session.clone();
                let (args, _, _, applied) = command_for(&e, "constrained").unwrap();
                let layers = layers_of(provider, &args).unwrap();
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
    fn a_codex_turn_whose_role_layers_do_not_fit_is_refused() {
        let mut e = entry(Provider::Codex);
        e.spec.constraints = vec![crate::agents::Constraint { from: "top".into(), text: "x".repeat(crate::library::CODEX_ROLE_MAX) }];
        let why = command_for(&e, "too-long").unwrap_err();
        assert!(why.contains("not started") && why.contains("24 KB"), "{why}");
        // Claude Code reads them from a file: no command line to fit.
        let mut e = entry(Provider::Claude);
        e.spec.constraints = vec![crate::agents::Constraint { from: "top".into(), text: "x".repeat(crate::library::CODEX_ROLE_MAX) }];
        assert!(command_for(&e, "too-long").is_ok());
    }

    #[test]
    fn auto_review_is_codex_own_flag_without_a_policy_or_sandbox() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.auto_review = true;
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--approve-for-me".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("approvals_reviewer=")), "its own flag says who reviews");
        assert!(!args.iter().any(|a| a.starts_with("approval_policy=") || a.starts_with("sandbox_mode=")), "Codex refuses them beside it");
        // A later turn resumes, and `exec resume` refuses the flag: the overrides it
        // stands for, in its place.
        e.session = Some("t-1".into());
        let (args, _, _, applied) = command_for(&e, "x").unwrap();
        assert!(args.starts_with(&strings(&["exec", "resume", "t-1"])), "{args:?}");
        assert!(!args.contains(&"--approve-for-me".to_string()), "{args:?}");
        let overrides = strings(&[
            "-c",
            "approvals_reviewer=\"auto_review\"",
            "-c",
            "approval_policy=\"on-request\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
        ]);
        assert!(args.windows(6).any(|w| w == overrides.as_slice()), "{args:?}");
        assert!(serde_json::to_string(&applied).unwrap().contains("approvals_reviewer"), "the Applied list says what went");
    }

    #[test]
    fn hooks_run_unasked_only_when_the_folder_says_so() {
        let mut e = entry(Provider::Codex);
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert!(!args.contains(&crate::codex::launch::HOOKS_UNASKED.to_string()));
        e.spec.hooks_unasked = true;
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);
        assert!(args.contains(&crate::codex::launch::HOOKS_UNASKED.to_string()), "every turn, resumes too");
        assert!(args.iter().any(|a| a.starts_with("projects={") && a.contains("trust_level")), "trusted for the run");
    }

    #[test]
    fn claude_helpers_keep_stdin_for_stream_json_and_codex_ones_do_not() {
        let has = |args: &[String], flag: &str| args.iter().any(|a| a == flag);
        let (args, _, _, _) = command_for(&entry(Provider::Claude), "x").unwrap();
        assert!(args.windows(2).any(|w| w == ["--input-format", "stream-json"]), "{args:?}");
        assert!(has(&args, "--replay-user-messages"), "its word that a line was taken in");
        let (args, _, _, _) = command_for(&entry(Provider::Codex), "x").unwrap();
        assert!(!has(&args, "--input-format") && !has(&args, "--replay-user-messages"));
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
        /// Codex: its `developer_instructions`, the role layers.
        role: Option<String>,
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
        let (args, dir, envs, applied) = command_for(e, id).unwrap();
        Redact::assert_shown(&args, &envs, &dir, &applied);
        Redact::assert_probed(e.spec.provider, &args);
        let applied = r.applied(&applied, &envs);
        let role = golden::developer_instructions(&args);
        let mut args = r.args(&args);
        // A Codex turn's role layers, every turn, just before its prompt on
        // stdin: compared apart (`role`), the rest of its arguments as ever.
        if e.spec.provider == Provider::Codex {
            let n = args.len();
            assert!(n >= 3 && args[n - 3..] == strings(&["-c", "developer_instructions=<role-layers>", "-"]), "{args:?}");
            args.drain(n - 3..n - 1);
        }
        let data = crate::paths::data_dir();
        let read = |sub: &str, placeholder: &str| {
            let path = data.join(sub).join(format!("{}.json", crate::paths::id_file_name(id)));
            let v = args.iter().any(|a| a == placeholder).then(|| r.file(&path.to_string_lossy()));
            let _ = std::fs::remove_file(&path);
            v
        };
        let (settings, mcp) = (read("hooks", "<settings-file>"), read("mcp", "<mcp-file>"));
        let instructions = golden::instructions(id, &args);
        Turn { dir: r.text(&dir), envs: r.envs(&envs), settings, mcp, instructions, role, args, applied }
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
            .with(lib.claude_skills(), "<library-dir>")
            // As `claudeMdExcludes` names it, on every OS.
            .with(lib.claude_skills().replace('\\', "/"), "<library-dir>")
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

    /// `args`, then no MCP server but those it is given: none of its account's.
    fn strict(mut args: Vec<String>) -> Vec<String> {
        args.push("--strict-mcp-config".into());
        args
    }

    /// `args`, then the settings file every Claude Code turn is given, and
    /// the folder of its role layers.
    fn with_settings(mut args: Vec<String>) -> Vec<String> {
        args.extend(strings(&["--settings", "<settings-file>", "--add-dir", "<instructions-dir>"]));
        args
    }

    #[test]
    fn golden_headless_claude_resume_and_worktree() {
        const ID: &str = "golden-bg-claude-resume";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Claude);
        e.spec.mode = "default".into();
        // Afresh in a worktree of Luna's naming.
        e.spec.worktree = true;
        e.spec.worktree_name = Some("luna-x".into());
        let mut want = claude_turn("default");
        want.push("--worktree=luna-x".into());
        assert_eq!(turn(&e, ID, &r).args, with_settings(strict(want)));

        // A conversation, in the folder it was sent to.
        e.session = Some("s-1".into());
        let mut want = claude_turn("default");
        want.extend(strings(&["--resume", "s-1"]));
        let want = with_settings(strict(want));
        let t = turn(&e, ID, &r);
        assert_eq!((t.args, t.dir.as_str()), (want.clone(), "C:\\p"));

        // In the worktree its first turn made, while that is there.
        let wt = std::env::temp_dir().join(format!("luna-golden-bg-wt-{}", std::process::id())).join("luna-x");
        std::fs::create_dir_all(&wt).unwrap();
        e.cwd = Some(wt.to_string_lossy().into_owned());
        let t = turn(&e, ID, &r);
        assert_eq!((t.args, t.dir), (want, wt.to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(wt.parent().unwrap());

        // Gone: asked for again by its name, from the folder.
        let t = turn(&e, ID, &r);
        let mut want = claude_turn("default");
        want.extend(strings(&["--worktree=luna-x", "--resume", "s-1"]));
        assert_eq!((t.args, t.dir.as_str()), (with_settings(strict(want)), "C:\\p"));
    }

    #[test]
    fn golden_headless_claude_helper_with_tools_answers_permissions_through_luna() {
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
            "--mcp-config",
            "<mcp-file>",
            "--permission-prompt-tool",
            "mcp__luna__luna_permission",
            "--allowedTools",
            "mcp__luna",
            "--disallowedTools",
            "Edit,Write,NotebookEdit,mcp__luna__luna_permission",
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
        let mut guard = golden::claude_helper_settings(golden::claude_settings_alone());
        guard["hooks"] = json!({ "PreToolUse": [golden::claude_guard_entry()] });
        // Its role layers read as memory of a folder added: the other added
        // folders' memory is not.
        guard["claudeMdExcludes"] = json!([
            "<notes-dir>/CLAUDE.md",
            "<notes-dir>/CLAUDE.local.md",
            "<notes-dir>/.claude/CLAUDE.md",
            "<notes-dir>/.claude/rules/**"
        ]);
        assert_eq!(t.settings, Some(guard), "the guard alone");
        assert_eq!(t.mcp, Some(golden::claude_mcp_config()));

        // No tools of its own: Luna's server is there for the permission prompts only.
        e.spec.tools = false;
        e.spec.parent = None;
        e.spec.read_only = false;
        e.spec.agent = None;
        let t = turn(&e, ID, &r);
        let mut want = strict(claude_turn("default"));
        want.extend(strings(&[
            "--mcp-config",
            "<mcp-file>",
            "--permission-prompt-tool",
            "mcp__luna__luna_permission",
            "--disallowedTools",
            "mcp__luna__luna_permission",
        ]));
        assert_eq!(t.args, with_settings(want));
        let alone = golden::claude_helper_settings(golden::claude_settings_alone());
        assert_eq!(t.settings, Some(alone), "neither tools nor a parent: no guard");

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

    /// A Codex turn's arguments from the folder's trust to the effort.
    fn codex_turn(model: bool) -> Vec<String> {
        let mut args = strings(&["--json", "--skip-git-repo-check"]);
        args.extend(crate::codex::trust::trust_override("C:\\p"));
        if model {
            args.extend(strings(&["--model", "haiku"]));
        }
        args.extend(strings(&["-c", "model_reasoning_effort=\"low\""]));
        args
    }

    /// `exec` asking nobody, held by the sandbox `mode`; then Codex's own
    /// update check kept off.
    fn codex_held(sandbox: Vec<String>, windows: bool) -> Vec<String> {
        let mut args = strings(&["-c", "approval_policy=\"never\"", "-c", "approvals_reviewer=\"user\""]);
        args.extend(sandbox);
        if windows {
            args.extend(golden::codex_windows_sandbox());
        }
        args.extend(strings(&["-c", "check_for_update_on_startup=false"]));
        args
    }

    /// The account's apps off for the run, and each server its `config.toml`
    /// names: the golden account has none.
    fn apps_off() -> Vec<String> {
        strings(&["-c", "features.apps=false"])
    }

    fn codex_turn_envs(id: &str, sandboxed: bool) -> Vec<(String, String)> {
        let mut envs = if sandboxed { golden::codex_sandbox_path() } else { Vec::new() };
        envs.extend(pairs(&[("CODEX_HOME", "C:\\acc"), ("LUNA_SESSION_ID", id)]));
        envs
    }

    #[test]
    fn golden_headless_codex_turn_is_exec_on_json_with_the_prompt_on_stdin() {
        const ID: &str = "golden-bg-codex";
        let r = Redact::for_chat(ID);
        let e = entry(Provider::Codex);
        let t = turn(&e, ID, &r);
        let mut want = strings(&["exec", "-C", "C:\\p"]);
        want.extend(codex_turn(true));
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        want.extend(apps_off());
        want.push("-".into());
        assert_eq!(t.args, want);
        assert_eq!(t.dir, "C:\\p");
        assert_eq!(t.envs, codex_turn_envs(ID, true));
        assert_eq!(t.role.as_deref(), Some(golden::role_layers(crate::library::Role::Helper).as_str()), "its config sets none");
    }

    #[test]
    fn golden_headless_codex_resume_and_worktree() {
        const ID: &str = "golden-bg-codex-resume";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Codex);
        e.spec.model = None;
        e.spec.worktree = true;
        let tail = |mut a: Vec<String>| {
            a.extend(codex_turn(false));
            a.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
            a.extend(apps_off());
            a.push("-".into());
            a
        };
        assert_eq!(turn(&e, ID, &r).args, tail(strings(&["exec", "-C", "C:\\p", "--worktree"])));
        e.session = Some("t-1".into());
        let t = turn(&e, ID, &r);
        assert_eq!((t.args, t.dir.as_str()), (tail(strings(&["exec", "resume", "t-1"])), "C:\\p"));
        // In the worktree its first turn made, while that is there.
        let wt = std::env::temp_dir().join(format!("luna-golden-bg-codex-wt-{}", std::process::id()));
        std::fs::create_dir_all(&wt).unwrap();
        e.cwd = Some(wt.to_string_lossy().into_owned());
        let t = turn(&e, ID, &r);
        assert_eq!((t.args, t.dir), (tail(strings(&["exec", "resume", "t-1"])), wt.to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(&wt);
    }

    #[test]
    fn a_codex_turn_is_told_to_skip_the_git_check_only_outside_a_repository() {
        const ID: &str = "golden-bg-codex-git";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Codex);
        let t = turn(&e, ID, &r);
        assert!(t.args.contains(&"--skip-git-repo-check".to_string()), "C:\\p is no repository");
        assert!(t.keys().contains(&"helper.skip-git-repo-check"));
        let repo = std::env::temp_dir().join(format!("luna-bg-git-{}", std::process::id()));
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        e.spec.folder = repo.join("sub").to_string_lossy().into_owned();
        let r = r.with(e.spec.folder.clone(), "<sub>");
        let t = turn(&e, ID, &r);
        assert!(!t.args.contains(&"--skip-git-repo-check".to_string()), "in a repository Codex needs no word: {:?}", t.args);
        assert!(!t.keys().contains(&"helper.skip-git-repo-check"));
        assert_eq!(t.value("helper.trust").map(|v| v.starts_with("-c projects=")), Some(true), "trusted for the run, nothing written");
        assert_eq!(t.value("helper.reviewer"), Some("-c approvals_reviewer=\"user\""));
        assert_eq!(t.value("no-self-update"), Some("-c check_for_update_on_startup=false"));
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

    #[test]
    fn golden_headless_codex_sandboxes_and_the_reviewer() {
        // (sandbox, auto review, what holds it, whether Windows sandboxes it)
        let read_only_net = codex_held(golden::codex_read_only_net(), true);
        let cases: [(&str, bool, Vec<String>, bool); 4] = [
            ("workspace-write", false, codex_held(strings(&["-c", "sandbox_mode=\"workspace-write\""]), true), true),
            ("danger-full-access", false, codex_held(strings(&["-c", "sandbox_mode=\"danger-full-access\""]), false), false),
            ("read-only-net", false, read_only_net, true),
            (
                "workspace-write",
                true,
                [strings(&["--approve-for-me"]), golden::codex_windows_sandbox(), strings(&["-c", "check_for_update_on_startup=false"])]
                    .concat(),
                true,
            ),
        ];
        for (mode, auto_review, held, sandboxed) in cases {
            let id = format!("golden-bg-codex-{mode}-{auto_review}");
            let r = Redact::for_chat(&id);
            let mut e = entry(Provider::Codex);
            e.spec.mode = mode.into();
            e.spec.auto_review = auto_review;
            let t = turn(&e, &id, &r);
            let mut want = strings(&["exec", "-C", "C:\\p"]);
            want.extend(codex_turn(true));
            want.extend(held);
            want.extend(apps_off());
            want.push("-".into());
            assert_eq!(t.args, want, "{mode} {auto_review}");
            let mut envs = codex_turn_envs(&id, sandboxed);
            if mode == crate::codex::launch::READ_ONLY_NET {
                // git's https, which schannel cannot do there: after the
                // sandbox's PATH.
                envs.splice(1..1, golden::codex_git_ssl());
            }
            assert_eq!(t.envs, envs, "{mode} {auto_review}");
        }
        // The reviewer's later turns resume, and `exec resume` has no
        // `--approve-for-me`: the overrides it stands for, in its place.
        let id = "golden-bg-codex-auto-review-resume";
        let r = Redact::for_chat(id);
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.auto_review = true;
        e.session = Some("t-1".into());
        let t = turn(&e, id, &r);
        let mut want = strings(&["exec", "resume", "t-1"]);
        want.extend(codex_turn(true));
        want.extend(strings(&[
            "-c",
            "approvals_reviewer=\"auto_review\"",
            "-c",
            "approval_policy=\"on-request\"",
            "-c",
            "sandbox_mode=\"workspace-write\"",
        ]));
        want.extend(golden::codex_windows_sandbox());
        want.extend(strings(&["-c", "check_for_update_on_startup=false"]));
        want.extend(apps_off());
        want.push("-".into());
        assert_eq!(t.args, want);
        assert_eq!(t.value("approval"), Some("-c approvals_reviewer=\"auto_review\" -c approval_policy=\"on-request\""));
        assert_eq!(t.value("sandbox"), Some("-c sandbox_mode=\"workspace-write\""));
    }

    #[test]
    fn golden_headless_codex_helper_with_tools_gets_the_guard_and_luna_mcp() {
        const ID: &str = "golden-bg-codex-helper";
        let r = Redact::for_chat(ID);
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.tools = true;
        e.spec.token = helper_token(ID, &e);
        e.spec.hooks_unasked = true;
        e.spec.parent = Some("golden-parent".into());
        let t = turn(&e, ID, &r);
        let mut want = strings(&["exec", "-C", "C:\\p"]);
        want.extend(codex_turn(true));
        want.extend(strings(&["--dangerously-bypass-hook-trust", "-c", &golden::codex_guard()]));
        want.extend(codex_held(strings(&["-c", "sandbox_mode=\"workspace-write\""]), true));
        want.extend(apps_off());
        want.extend(golden::codex_mcp());
        want.push("-".into());
        assert_eq!(t.args, want);
        let mut envs = golden::codex_sandbox_path();
        envs.extend(pairs(&[
            ("LUNA_MCP_TOKEN", "<token>"),
            ("CODEX_HOME", "C:\\acc"),
            ("LUNA_SESSION_ID", ID),
            ("LUNA_PARENT_ID", "golden-parent"),
        ]));
        assert_eq!(t.envs, envs);

        // A token without tools gives no server: Codex has no permission prompt to route.
        e.spec.tools = false;
        let t = turn(&e, ID, &r);
        assert!(!t.args.iter().any(|a| a.starts_with("mcp_servers.")), "{:?}", t.args);
        assert!(!t.envs.iter().any(|(k, _)| k == "LUNA_MCP_TOKEN"));
        // Spawned with `connectors`: its account's apps and servers stay on.
        e.spec.connectors = true;
        let t = turn(&e, ID, &r);
        assert!(!t.args.iter().any(|a| a == "features.apps=false"), "{:?}", t.args);
        crate::agents::forget(ID);
    }

    #[test]
    fn golden_headless_codex_helper_turns_off_the_servers_its_account_names() {
        const ID: &str = "golden-bg-codex-servers";
        let account = std::env::temp_dir().join(format!("luna-golden-codex-servers-{}", std::process::id()));
        std::fs::create_dir_all(&account).unwrap();
        let config = "[mcp_servers.gmail]\ncommand = \"node\"\n\n\
                      [mcp_servers.luna]\nurl = \"https://y\"\n\n[mcp_servers.Zoho_CRM-2]\nurl = \"https://z\"\n";
        std::fs::write(account.join("config.toml"), config).unwrap();
        let account = account.to_string_lossy().into_owned();
        let r = Redact::for_chat(ID).with(account.clone(), "<acc>");
        let mut e = entry(Provider::Codex);
        e.spec.account_path = account.clone();
        let t = turn(&e, ID, &r);
        let mut want = strings(&["exec", "-C", "C:\\p"]);
        want.extend(codex_turn(true));
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        // One the account calls `luna` is its own, not Luna's, which this helper
        // without tools is not given: off as well.
        want.extend(apps_off());
        want.extend(strings(&[
            "-c",
            "mcp_servers.gmail.enabled=false",
            "-c",
            "mcp_servers.luna.enabled=false",
            "-c",
            "mcp_servers.Zoho_CRM-2.enabled=false",
            "-",
        ]));
        assert_eq!(t.args, want);
        assert_eq!(t.value("helper.mcp-left-on"), None, "nothing else to keep");
        // A name a `-c` key cannot spell cannot be turned off: no turn, rather
        // than one that keeps it.
        let file = std::path::Path::new(&account).join("config.toml");
        std::fs::write(&file, format!("{config}\n[mcp_servers.\"team.slack\"]\nurl = \"https://x\"\n")).unwrap();
        let err = command_for(&e, ID).err().unwrap();
        assert!(err.contains("not started") && err.contains("\"team.slack\""), "{err}");
        std::fs::write(&file, config).unwrap();
        // What Codex cannot turn off for a run is listed: a project's servers.
        let project = std::path::Path::new(&account).join("project");
        std::fs::create_dir_all(project.join(".codex")).unwrap();
        std::fs::write(project.join(".codex").join("config.toml"), "[mcp_servers.docs]\ncommand = \"x\"\n").unwrap();
        let mut in_project = entry(Provider::Codex);
        in_project.spec.account_path = account.clone();
        in_project.spec.folder = project.to_string_lossy().into_owned();
        let (_, _, _, applied) = command_for(&in_project, ID).unwrap();
        let left = applied.iter().find(|a| a.key == "helper.mcp-left-on").map(|a| a.value.clone());
        assert!(left.as_deref().is_some_and(|l| l.starts_with("server \"docs\" of ")), "{left:?}");
        // With tools, Luna's own is `luna`: the account's, a url alone, is replaced
        // whole and turned on; one with more of its own stops the start.
        e.spec.tools = true;
        e.spec.token = helper_token(ID, &e);
        let t = turn(&e, ID, &r);
        assert!(t.args.windows(2).any(|w| w == ["-c", "mcp_servers.luna.enabled=true"]), "{:?}", t.args);
        assert!(!t.args.iter().any(|a| a == "mcp_servers.luna.enabled=false"), "{:?}", t.args);
        assert!(t.args.ends_with(&[golden::codex_mcp(), strings(&["-"])].concat()), "{:?}", t.args);
        std::fs::write(std::path::Path::new(&account).join("config.toml"), "[mcp_servers.luna]\ncommand = \"node\"\n").unwrap();
        assert!(command_for(&e, ID).err().unwrap().contains("named luna"));
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

    #[test]
    fn golden_headless_claude_turn_in_a_library_folder_gets_its_skills_and_instructions() {
        const ID: &str = "golden-bg-claude-library";
        let lib = crate::library::GoldenLibrary::new("bg-claude");
        let (mut e, r) = library_entry(ID, &lib, Provider::Claude);
        e.spec.mode = "default".into();
        let t = turn(&e, ID, &r);
        let mut want = strict(claude_turn("default"));
        want.extend(strings(&["--settings", "<settings-file>", "--add-dir", "<instructions-dir>", "--add-dir", "<library-dir>"]));
        assert_eq!(t.args, want);
        assert_eq!(t.dir, "<project>");
        assert_eq!(
            t.envs,
            pairs(&[
                ("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1"),
                ("CLAUDE_CONFIG_DIR", "<acc>"),
                ("DISABLE_AUTOUPDATER", "1"),
                ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
                ("CLAUDE_CODE_PRINT_BG_WAIT_CEILING_MS", "10800000"),
                ("LUNA_SESSION_ID", ID),
            ])
        );
        let mut settings = golden::claude_helper_settings(golden::claude_settings_alone());
        settings["skillOverrides"] = json!({ "review": "off" });
        settings["claudeMdExcludes"] = json!([
            "<acc>/CLAUDE.md",
            "<library-dir>/CLAUDE.md",
            "<library-dir>/CLAUDE.local.md",
            "<library-dir>/.claude/CLAUDE.md",
            "<library-dir>/.claude/rules/**"
        ]);
        assert_eq!(t.settings, Some(settings), "no hooks, no guard: what the folder keeps from it alone");
        let want =
            format!("{}\n\n{}", crate::library::GoldenLibrary::INSTRUCTIONS.trim(), golden::role_layers(crate::library::Role::Helper));
        assert_eq!(t.instructions, Some(want), "the library's, word for word, in the account's place, then its role");
    }

    #[test]
    fn golden_headless_codex_turn_in_a_library_folder_gets_its_plugin_and_keeps_its_skills_off() {
        const ID: &str = "golden-bg-codex-library";
        let lib = crate::library::GoldenLibrary::new("bg-codex");
        let (e, r) = library_entry(ID, &lib, Provider::Codex);
        let t = turn(&e, ID, &r);
        // A repository: Codex finds it, and needs no word to run there.
        let mut want = strings(&["exec", "-C", "<project>", "--json"]);
        want.extend(crate::codex::trust::trust_override(&lib.folder).iter().map(|a| r.text(a)));
        want.extend(strings(&["--model", "haiku", "-c", "model_reasoning_effort=\"low\""]));
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        want.extend(apps_off());
        want.extend(strings(&[
            "-c",
            &format!("plugins.{}@luna.enabled=true", crate::library::GoldenLibrary::SLUG),
            "-c",
            "skills.config=[{name=\"review\",enabled=false}]",
            "-",
        ]));
        assert_eq!(t.args, want);
        assert_eq!(t.dir, "<project>");
        let mut envs = golden::codex_sandbox_path();
        envs.extend(pairs(&[("CODEX_HOME", "<acc>"), ("LUNA_SESSION_ID", ID)]));
        assert_eq!(t.envs, envs);
        assert_eq!((t.settings, t.instructions), (None, None), "Codex has no way to take other instructions");
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
        let mut want = strings(&["exec", "-C", "<project>", "--json"]);
        want.extend(crate::codex::trust::trust_override(&lib.folder).iter().map(|a| r.text(a)));
        want.extend(strings(&["--model", "haiku", "-c", "model_reasoning_effort=\"low\""]));
        want.extend(codex_held(golden::codex_read_only(&["-c", "sandbox_mode=\"read-only\""]), true));
        want.extend(apps_off());
        want.extend(strings(&["-c", "skills.config=[{name=\"report\",enabled=false}]", "-"]));
        assert_eq!(t.args, want);
        assert_eq!(
            t.value("library.audience"),
            Some("instructions main (chats only): AGENTS.md says so, as Codex reads it in every session; skill report (chats only)")
        );
        assert_eq!(t.value("library.hidden-skills"), Some("-c skills.config=[{name=\"report\",enabled=false}]"));
    }
}
