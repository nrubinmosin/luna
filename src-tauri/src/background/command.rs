//! What a helper's turn runs: the CLI's arguments, folder and environment, and the
//! lines its stdin is given.

use super::*;

/// The arguments and working directory of the next turn. Err when a Claude
/// Code turn's mandatory settings — its core-folder deny (threat model F3) —
/// cannot be written: the turn refuses rather than run without the deny.
pub(super) fn command_for(e: &Entry, id: &str) -> Result<(Vec<String>, String, Vec<(&'static str, String)>), String> {
    let s = &e.spec;
    let mut args: Vec<String> = Vec::new();
    let mut envs: Vec<(&'static str, String)> = Vec::new();
    // A worktree the first turn made, while it is there; Claude Code may
    // drop a clean one on exit, and is then asked for it again by name.
    let made = e.cwd.clone().filter(|c| !crate::pty::same_dir(c, &s.folder));
    let mut dir = s.folder.clone();
    match s.provider {
        Provider::Claude => {
            args.extend(
                ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--replay-user-messages"]
                    .map(String::from),
            );
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
            }
            args.extend(["--effort".into(), s.effort.clone(), "--permission-mode".into(), s.mode.clone()]);
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
            if let Some(agent) = &s.agent {
                args.extend(["--agent".into(), agent.clone()]);
            }
            let mut denied: Vec<&str> = if s.read_only { crate::claude::launch::CLAUDE_EDIT_TOOLS.to_vec() } else { Vec::new() };
            // Luna's server alone, unless spawned with the account's own.
            let helper = crate::claude::launch::HelperSettings { connectors: s.connectors };
            if !s.connectors {
                args.push(crate::claude::launch::STRICT_MCP.into());
            }
            if let Some((mcp, token)) = s.token.clone().and_then(|t| Some((crate::pty::claude_mcp_file(id)?, t))) {
                args.extend(["--mcp-config".into(), mcp.to_string_lossy().into_owned()]);
                envs.push((crate::claude::launch::CLAUDE_MCP_TOKEN_ENV, token));
                args.extend(["--permission-prompt-tool".into(), "mcp__luna__luna_permission".into()]);
                // The CLI's to call, not the model's: hidden from its tools,
                // and the prompts still come through.
                denied.push("mcp__luna__luna_permission");
                if s.tools {
                    args.extend(["--allowedTools".into(), "mcp__luna".into()]);
                }
            }
            if !denied.is_empty() {
                args.extend(["--disallowedTools".into(), denied.join(",")]);
            }
            if let Some(parent) = &s.parent {
                args.extend(crate::claude::launch::claude_helper_args(parent));
            }
            // A helper with tools has helpers of its own to hear from: what a
            // notice names is a file there, to read without asking.
            if s.tools {
                let notes = crate::agents::notes_dir();
                if std::fs::create_dir_all(&notes).is_ok() {
                    args.extend(["--add-dir".into(), notes.to_string_lossy().into_owned()]);
                }
            }
            // The skills the library gives sessions where it works and keeps
            // from them, on every turn: each is a process of its own
            // (library/folders.rs `session_args`). No hooks: its stream says it all; but
            // the guard against endless waits, a helper's as any (guard.rs).
            let skills = crate::library::session_args(Provider::Claude, &s.account_path, &dir);
            let settings = crate::pty::claude_settings_file(id, false, s.tools || s.parent.is_some(), &skills, Some(helper))?;
            args.extend(["--settings".into(), settings.to_string_lossy().into_owned()]);
            if let Some(dir) = crate::pty::claude_instructions_dir(id, skills.instructions.as_deref()) {
                args.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
                envs.push((crate::claude::launch::CLAUDE_ADDED_DIR_MEMORY, "1".into()));
            }
            args.extend(skills.args);
            envs.push(("CLAUDE_CONFIG_DIR", s.account_path.clone()));
            envs.push(("DISABLE_AUTOUPDATER", "1".into()));
            // Never inherited: a `0` there turns auto-memory on (`HelperSettings`).
            envs.push((crate::claude::launch::NO_AUTO_MEMORY_ENV, "1".into()));
        }
        Provider::Codex => {
            args.push("exec".into());
            match &e.session {
                Some(thread) => {
                    args.extend(["resume".into(), thread.clone()]);
                    if let Some(wt) = made.filter(|w| std::path::Path::new(w).is_dir()) {
                        dir = wt;
                    }
                }
                None => {
                    args.extend(["-C".into(), s.folder.clone()]);
                    if s.worktree {
                        args.push("--worktree".into());
                    }
                }
            }
            args.extend(["--json".into(), "--skip-git-repo-check".into()]);
            // `exec` cannot ask for the folder's trust and goes without the
            // project's config and hooks if it lacks it; a helper is where it
            // was sent, trusted for the run (codex/trust.rs).
            args.extend(crate::codex::trust::trust_override(&s.folder));
            if let Some(m) = &s.model {
                args.extend(["--model".into(), m.clone()]);
            }
            args.extend(["-c".into(), format!("model_reasoning_effort=\"{}\"", s.effort)]);
            if s.hooks_unasked {
                args.push(crate::codex::launch::HOOKS_UNASKED.into());
            }
            args.extend(crate::pty::codex_guard_args(id, s.tools || s.parent.is_some(), s.hooks_unasked));
            if s.auto_review {
                args.push("--approve-for-me".into());
            } else {
                // `exec` asks nobody: the sandbox is what holds it.
                args.extend(["-c".into(), "approval_policy=\"never\"".into()]);
                args.extend(["-c".into(), crate::codex::launch::CODEX_PERSON_REVIEWS.into()]);
                args.extend(crate::pty::codex_sandbox_args(&s.mode));
            }
            if let Some(sb) = crate::codex::launch::codex_windows_sandbox(&s.mode) {
                args.extend(["-c".into(), sb]);
            }
            if let Some(path) = crate::pty::codex_sandbox_path(&s.mode) {
                envs.push(("PATH", path));
            }
            // Luna's own server, with its tools: none while the hub is not up.
            let luna = s.token.clone().filter(|_| s.tools).map(|token| (crate::pty::codex_mcp_args(), token));
            if !s.connectors {
                let supplied = luna.as_ref().is_some_and(|(mcp, _)| !mcp.is_empty());
                let servers = crate::codex::config::helper_mcp_servers(&s.account_path)?;
                args.extend(crate::codex::launch::connectors_off(&servers, supplied)?);
            }
            if let Some((mcp, token)) = luna {
                args.extend(mcp);
                envs.push(("LUNA_MCP_TOKEN", token));
            }
            envs.extend(crate::pty::codex_git_envs(&s.mode));
            envs.push(("CODEX_HOME", s.account_path.clone()));
            args.extend(crate::library::session_args(Provider::Codex, &s.account_path, &dir).args);
            // The prompt comes on stdin; Claude Code reads it there unasked.
            args.push("-".into());
        }
    }
    envs.extend(crate::pty::session_envs(id, s.parent.as_deref()));
    Ok((args, dir, envs))
}

/// What a turn's CLI is given: `prompt`, which on every turn of a Codex helper
/// carries Luna's note (`pty::helper_prompt`), naming its parent as it is
/// now. Claude Code's system prompt says it every turn, and its first prompt
/// too, beside the task its account's instructions would otherwise outweigh;
/// Codex has nowhere but the conversation, where one note at its start goes
/// with a compaction, and a long helper would go on as the chat of a person.
/// Not in `Entry::opening`, which goes back to the caller as what it asked, nor
/// in what `luna_read` gives back (`message_of` in codex/session/read.rs and
/// claude/session/transcript.rs).
pub(super) fn stdin_text(e: &Entry, prompt: &str) -> String {
    match (&e.spec.parent, e.spec.provider) {
        (Some(parent), Provider::Codex) => crate::pty::helper_prompt(parent, prompt),
        (Some(parent), Provider::Claude) if e.session.is_none() => crate::pty::helper_prompt(parent, prompt),
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

    #[test]
    fn notes_are_open_to_a_helper_with_tools() {
        let mut e = entry(Provider::Claude);
        let has_dir = |e: &Entry| command_for(e, "x").unwrap().0.iter().any(|a| a == "--add-dir");
        assert!(!has_dir(&e), "one with no tools gets no notices");
        e.spec.tools = true;
        assert!(has_dir(&e));
        let (args, _, _) = command_for(&e, "x").unwrap();
        let at = args.iter().position(|a| a == "--add-dir").unwrap();
        assert_eq!(args[at + 1], crate::agents::notes_dir().to_string_lossy());
        let mut codex = entry(Provider::Codex);
        codex.spec.tools = true;
        assert!(!has_dir(&codex), "Codex's sandboxes read anywhere");
    }

    #[test]
    fn a_claude_turn_refuses_when_the_core_folder_deny_cannot_be_written() {
        // A Claude settings file carries the core-folder deny (threat model
        // F3). An id with a path separator names a file under a hooks subfolder
        // that is not there, so the write fails: the turn must refuse, not run
        // without the deny.
        let hooks = crate::paths::data_dir().join("hooks");
        let _ = crate::secret::private_dir(&hooks);
        let _ = std::fs::remove_dir_all(hooks.join("f3-no-such-subdir"));
        let bad = "f3-no-such-subdir/x";
        assert!(command_for(&entry(Provider::Claude), bad).is_err(), "no deny written, no Claude turn");
        // The helper itself refuses the same way.
        let none = crate::library::SessionSkills::default();
        assert!(crate::pty::claude_settings_file(bad, true, false, &none, None).is_err(), "and a pane start would too");
        // Codex carries the deny in its sandbox profile, not a settings file,
        // so the same id does not stop it.
        assert!(command_for(&entry(Provider::Codex), bad).is_ok(), "Codex writes no settings file");
    }

    #[test]
    fn a_turn_resumes_the_conversation_the_first_one_named() {
        let mut e = entry(Provider::Codex);
        let (args, dir, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["exec", "-C", "C:\\p"]);
        assert_eq!(args.last().map(String::as_str), Some("-"), "the prompt comes on stdin");
        assert!(
            args.iter().any(|a| a == "sandbox_mode=\"read-only\"" || a.starts_with("default_permissions=\"luna-")),
            "held by its sandbox"
        );
        assert_eq!(dir, "C:\\p");
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);

        let mut e = entry(Provider::Claude);
        on_claude_event(&mut e, &json!({ "type": "system", "subtype": "init", "session_id": "s-1", "cwd": "C:\\p" }));
        let (args, _, envs) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["-p", "--input-format", "stream-json"]);
        let at = args.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(args[at + 1], "s-1");
        assert!(envs.iter().any(|(k, v)| *k == "CLAUDE_CONFIG_DIR" && v == "C:\\acc"));
    }

    #[test]
    fn a_claude_helpers_mcp_token_goes_by_its_environment_and_never_into_its_config_file() {
        crate::hub::start();
        let mut e = entry(Provider::Claude);
        e.spec.token = Some("the-helpers-token".into());
        let (args, _, envs) = command_for(&e, "bg-mcp-token").unwrap();
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
        let (args, dir, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--worktree=luna-x".to_string()) && !args.contains(&"--worktree".to_string()));
        assert_eq!(dir, "C:\\p");
        assert!(!args.iter().any(|a| a == "--resume"), "there is no conversation to resume");

        // A name that is a flag turns permissions off, as Claude Code reads it, if it
        // ever gets to be an argument: not from the spec, and not from where it worked.
        e.spec.worktree_name = Some(FLAG.into());
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--worktree".to_string()) && !args.iter().any(|a| a.contains(FLAG)), "{args:?}");
        e.session = Some("s-1".into());
        e.cwd = Some(std::path::Path::new("C:\\no\\such").join(FLAG).to_string_lossy().into_owned());
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--resume".to_string()) && !args.iter().any(|a| a.contains(FLAG)), "{args:?}");
        e.cwd = Some(std::path::Path::new("C:\\no\\such").join("luna-x").to_string_lossy().into_owned());
        assert!(
            command_for(&e, "x").unwrap().0.contains(&"--worktree=luna-x".to_string()),
            "gone from disk, Claude Code makes it again by name"
        );

        // Codex names none, and always makes its own.
        let mut e = entry(Provider::Codex);
        e.spec.worktree = true;
        let (args, _, _) = command_for(&e, "x").unwrap();
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
        let notes = |text: String| text.matches("A note from Luna").count();
        assert_eq!(notes(stdin_text(&e, "the prompt")), 1, "the turn that starts its conversation");
        assert_eq!(stdin_text(&e, "/review"), "/review", "not into a command's arguments");
        e.session = Some("t-1".into());
        assert_eq!(notes(stdin_text(&e, "Carry on where you left off.")), 1, "and each one after: a compaction loses the first");
        // Claude Code's first prompt has it too, beside the task its account's
        // instructions would otherwise outweigh; after that its system prompt
        // says it on every turn.
        let mut e = entry(Provider::Claude);
        e.spec.parent = Some("p-1".into());
        let first = stdin_text(&e, "the prompt");
        assert!(first.starts_with("the prompt\n\n") && notes(first.clone()) == 1, "{first}");
        assert!(first.contains("the operator, the user or the person at this machine"), "{first}");
        assert_eq!(stdin_text(&e, "/review"), "/review");
        e.session = Some("s-1".into());
        assert_eq!(stdin_text(&e, "the next"), "the next", "its system prompt says it there");
        e.session = None;
        e.spec.parent = None;
        assert_eq!(stdin_text(&e, "the prompt"), "the prompt", "a helper of nobody's is told nothing");
    }

    #[test]
    fn a_helper_passed_to_another_parent_is_told_on_its_next_turn() {
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.spec.parent = Some("old".into());
            e.session = Some("s-1".into());
            lock().entries.insert("moved".into(), e);
            set_parent("moved", Some("new".into()));
            let r = lock();
            let e = &r.entries["moved"];
            let (args, _, envs) = command_for(e, "moved").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "new"), "{provider:?}");
            let line = stdin_text(e, "go on");
            match provider {
                Provider::Claude => {
                    assert!(value_of(&args, "--append-system-prompt").unwrap().contains("by session new"));
                    assert_eq!(line, "go on", "its system prompt says it");
                }
                Provider::Codex => {
                    assert!(line.starts_with("go on") && line.contains("by session new"), "{line}");
                    assert_eq!(line.matches("A note from Luna").count(), 1);
                }
            }
            drop(r);
            // With nobody to answer to, it is told of nobody.
            set_parent("moved", None);
            let e = lock().entries.remove("moved").unwrap();
            let (args, _, envs) = command_for(&e, "moved").unwrap();
            assert!(!envs.iter().any(|(k, _)| *k == "LUNA_PARENT_ID"));
            assert!(!args.iter().any(|a| a == "--append-system-prompt"));
            assert_eq!(stdin_text(&e, "go on"), "go on");
        }
    }

    #[test]
    fn read_only_net_goes_to_codex_as_a_profile_on_every_turn() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = crate::codex::launch::READ_ONLY_NET.into();
        for session in [None, Some("t-1".to_string())] {
            e.session = session;
            let (args, _, envs) = command_for(&e, "x").unwrap();
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
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--disallowedTools"), None);
        e.spec.read_only = true;
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--disallowedTools"), Some("Edit,Write,NotebookEdit"));
        assert_eq!(args.iter().filter(|a| *a == "--disallowedTools").count(), 1);
    }

    #[test]
    fn an_agent_is_claude_codes_flag_on_every_turn() {
        let mut e = entry(Provider::Claude);
        e.spec.agent = Some("checker".into());
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--agent"), Some("checker"));
        e.session = Some("s-1".into());
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(value_of(&args, "--agent"), Some("checker"));
        assert_eq!(value_of(&args, "--resume"), Some("s-1"));
    }

    #[test]
    fn a_helper_knows_it_is_one_on_both_clis() {
        for provider in [Provider::Claude, Provider::Codex] {
            let mut e = entry(provider);
            e.spec.parent = Some("p-1".into());
            let (args, _, envs) = command_for(&e, "x").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_SESSION_ID" && v == "x"));
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_PARENT_ID" && v == "p-1"));
            assert!(!args.iter().any(|a| a.contains("developer_instructions")), "that would replace what its config says");
            if provider == Provider::Claude {
                assert!(value_of(&args, "--append-system-prompt").unwrap().contains("started through Luna by session p-1"));
            }
        }
        for provider in [Provider::Claude, Provider::Codex] {
            let (args, _, envs) = command_for(&entry(provider), "x").unwrap();
            assert!(envs.iter().any(|(k, v)| *k == "LUNA_SESSION_ID" && v == "x"), "whoever started it");
            assert!(!envs.iter().any(|(k, _)| *k == "LUNA_PARENT_ID"));
            assert!(!args.iter().any(|a| a == "--append-system-prompt"));
        }
    }

    #[test]
    fn auto_review_is_codex_own_flag_without_a_policy_or_sandbox() {
        let mut e = entry(Provider::Codex);
        e.spec.mode = "workspace-write".into();
        e.spec.auto_review = true;
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert!(args.contains(&"--approve-for-me".to_string()));
        assert!(!args.iter().any(|a| a.starts_with("approvals_reviewer=")), "its own flag says who reviews");
        assert!(!args.iter().any(|a| a.starts_with("approval_policy=") || a.starts_with("sandbox_mode=")), "Codex refuses them beside it");
    }

    #[test]
    fn hooks_run_unasked_only_when_the_folder_says_so() {
        let mut e = entry(Provider::Codex);
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert!(!args.contains(&crate::codex::launch::HOOKS_UNASKED.to_string()));
        e.spec.hooks_unasked = true;
        on_codex_event(&mut e, &json!({ "type": "thread.started", "thread_id": "t-1" }));
        let (args, _, _) = command_for(&e, "x").unwrap();
        assert_eq!(args[..3], ["exec", "resume", "t-1"]);
        assert!(args.contains(&crate::codex::launch::HOOKS_UNASKED.to_string()), "every turn, resumes too");
        assert!(args.iter().any(|a| a.starts_with("projects={") && a.contains("trust_level")), "trusted for the run");
    }

    #[test]
    fn claude_helpers_keep_stdin_for_stream_json_and_codex_ones_do_not() {
        let has = |args: &[String], flag: &str| args.iter().any(|a| a == flag);
        let (args, _, _) = command_for(&entry(Provider::Claude), "x").unwrap();
        assert!(args.windows(2).any(|w| w == ["--input-format", "stream-json"]), "{args:?}");
        assert!(has(&args, "--replay-user-messages"), "its word that a line was taken in");
        let (args, _, _) = command_for(&entry(Provider::Codex), "x").unwrap();
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
    }

    fn turn(e: &Entry, id: &str, r: &Redact) -> Turn {
        let (args, dir, envs) = command_for(e, id).unwrap();
        let args = r.args(&args);
        let data = crate::paths::data_dir();
        let read = |sub: &str, placeholder: &str| {
            let path = data.join(sub).join(format!("{id}.json"));
            let v = args.iter().any(|a| a == placeholder).then(|| r.file(&path.to_string_lossy()));
            let _ = std::fs::remove_file(&path);
            v
        };
        let (settings, mcp) = (read("hooks", "<settings-file>"), read("mcp", "<mcp-file>"));
        let instructions = golden::instructions(id, &args);
        Turn { dir: r.text(&dir), envs: r.envs(&envs), settings, mcp, instructions, args }
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
            ("CLAUDE_CONFIG_DIR", "C:\\acc"),
            ("DISABLE_AUTOUPDATER", "1"),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
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
        let settings = golden::claude_helper_settings(golden::claude_settings_alone());
        assert_eq!((t.settings, t.mcp), (Some(settings), None), "no hooks: its stream says it all");
    }

    /// `args`, then no MCP server but those it is given: none of its account's.
    fn strict(mut args: Vec<String>) -> Vec<String> {
        args.push("--strict-mcp-config".into());
        args
    }

    /// `args`, then the settings file every Claude Code turn is given.
    fn with_settings(mut args: Vec<String>) -> Vec<String> {
        args.extend(strings(&["--settings", "<settings-file>"]));
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
            "--append-system-prompt",
            &golden::helper_line("golden-parent"),
            "--add-dir",
            "<notes-dir>",
            "--settings",
            "<settings-file>",
        ]));
        assert_eq!(t.args, want);
        let mut envs = pairs(&[("LUNA_MCP_TOKEN", "<token>")]);
        envs.extend(claude_turn_envs(ID));
        envs.extend(pairs(&[("LUNA_PARENT_ID", "golden-parent")]));
        assert_eq!(t.envs, envs);
        let mut guard = golden::claude_helper_settings(golden::claude_settings_alone());
        guard["hooks"] = json!({ "PreToolUse": [golden::claude_guard_entry()] });
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

    /// `exec` asking nobody, held by the sandbox `mode`.
    fn codex_held(sandbox: Vec<String>, windows: bool) -> Vec<String> {
        let mut args = strings(&["-c", "approval_policy=\"never\"", "-c", "approvals_reviewer=\"user\""]);
        args.extend(sandbox);
        if windows {
            args.extend(golden::codex_windows_sandbox());
        }
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
    fn golden_headless_codex_sandboxes_and_the_reviewer() {
        // (sandbox, auto review, what holds it, whether Windows sandboxes it)
        let read_only_net = codex_held(golden::codex_read_only_net(), true);
        let cases: [(&str, bool, Vec<String>, bool); 4] = [
            ("workspace-write", false, codex_held(strings(&["-c", "sandbox_mode=\"workspace-write\""]), true), true),
            ("danger-full-access", false, codex_held(strings(&["-c", "sandbox_mode=\"danger-full-access\""]), false), false),
            ("read-only-net", false, read_only_net, true),
            ("workspace-write", true, [strings(&["--approve-for-me"]), golden::codex_windows_sandbox()].concat(), true),
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
        let config = "[mcp_servers.gmail]\ncommand = \"node\"\n\n[mcp_servers.\"team.slack\"]\nurl = \"https://x\"\n\n\
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
        // A name a `-c` key cannot spell is left on: overriding it would fail the
        // run. One the account calls `luna` is its own, not Luna's, which this
        // helper without tools is not given: off as well.
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
                ("LUNA_SESSION_ID", ID),
            ])
        );
        let mut settings = golden::claude_helper_settings(golden::claude_settings_alone());
        settings["skillOverrides"] = json!({ "review": "off" });
        settings["claudeMdExcludes"] = json!(["<acc>/CLAUDE.md"]);
        assert_eq!(t.settings, Some(settings), "no hooks, no guard: what the folder keeps from it alone");
        assert_eq!(t.instructions.as_deref(), Some(crate::library::GoldenLibrary::INSTRUCTIONS), "the library's, word for word");
    }

    #[test]
    fn golden_headless_codex_turn_in_a_library_folder_gets_its_plugin_and_keeps_its_skills_off() {
        const ID: &str = "golden-bg-codex-library";
        let lib = crate::library::GoldenLibrary::new("bg-codex");
        let (e, r) = library_entry(ID, &lib, Provider::Codex);
        let t = turn(&e, ID, &r);
        let mut want = strings(&["exec", "-C", "<project>", "--json", "--skip-git-repo-check"]);
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
}
