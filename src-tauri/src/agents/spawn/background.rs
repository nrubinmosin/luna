//! A spawn as a background helper: the `Spec` background.rs runs it from, and
//! the call that waits for its first turn.

use crate::agents::owed::{owe_asked, shown, Asked};
use crate::agents::registry::{chat_name, reg, register, set_name};
use crate::agents::spawn::folder::neighbors_in;
use crate::agents::spawn::{spawn_is, SpawnParams, Spawned};
use crate::agents::teardown::{forget, Auto};
use crate::provider::Provider;

/// The levels a Claude model the account's list does not know may be asked
/// at. `ultracode` is Claude Code's standing orchestration, which it runs at
/// any level, so every model that takes effort takes it too.
const CLAUDE_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];
const ULTRACODE: &str = "ultracode";

/// The permission modes a Claude Code helper starts in: every one `claude -p
/// --permission-mode` takes but `manual`, Luna's `default` by another name
/// (2.1.322's choices; docs/cli-compatibility.md). `auto` has a model review
/// what would go further and refuses what it cannot review; `dontAsk` runs what
/// the allow rules let through and refuses the rest. Neither waits on a person.
pub(in crate::agents) const CLAUDE_MODES: [&str; 6] = ["default", "acceptEdits", "plan", "bypassPermissions", "auto", "dontAsk"];

/// Given and not blank.
pub(in crate::agents) fn asked(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// A Claude helper's model and effort: what the spawn asks for over the
/// account's and project's defaults (`default_model`, and `default_effort` of
/// the model it runs on: Claude Code keeps effort per model), held to the
/// account's model list. The effort is checked against the model the helper
/// will run on, asked for or not; a model that takes no effort gets none (an
/// empty effort: no `--effort`), and a default level it does not take moves
/// to its own default.
fn claude_model_and_effort(
    models: &[crate::claude::models::ClaudeModel],
    asked_model: Option<&str>,
    asked_effort: Option<&str>,
    default_model: Option<&str>,
    default_effort: impl Fn(Option<&str>) -> String,
) -> Result<(Option<String>, String), String> {
    let find = |m: &str| {
        models.iter().find(|x| x.slug.eq_ignore_ascii_case(m) || x.model_id.as_deref().is_some_and(|id| id.eq_ignore_ascii_case(m)))
    };
    let model = match asked_model {
        Some(m) if find(m).is_some() => Some(m.to_string()),
        // A spelling the list does not carry that names a family plainly
        // (`opusplan`, `claude-opus-5[1m]`): the CLI's to read.
        Some(m) if crate::claude::models::ALIASES.iter().any(|a| m.to_lowercase().contains(a)) => Some(m.to_string()),
        Some(m) => {
            let all: Vec<&str> = models.iter().map(|x| x.slug.as_str()).collect();
            return Err(format!("unknown Claude model \"{m}\"; this account lists {}, or a full id such as claude-opus-5", all.join(", ")));
        }
        None => default_model.map(str::to_owned),
    };
    let listed = model.as_deref().and_then(find);
    // As in the dialog: the model brings its own levels, and its own default
    // level where it does not take the default one.
    let efforts: Vec<&str> = match listed {
        Some(l) if !l.efforts.is_empty() => l.efforts.iter().map(String::as_str).chain([ULTRACODE]).collect(),
        _ => CLAUDE_EFFORTS.to_vec(),
    };
    let effort = match (asked_effort, listed) {
        (Some(_), Some(l)) if l.efforts.is_empty() => return Err(format!("{} takes no effort level; leave effort out", l.slug)),
        (None, Some(l)) if l.efforts.is_empty() => String::new(),
        (Some(e), _) if efforts.contains(&e) => e.to_string(),
        (Some(e), Some(l)) => return Err(format!("{} takes effort {}, not \"{e}\"", l.slug, efforts.join(", "))),
        (Some(e), None) => return Err(format!("unknown Claude effort \"{e}\"; use {}", efforts.join(", "))),
        (None, _) => {
            let default_effort = default_effort(model.as_deref());
            match listed {
                Some(l) if !efforts.contains(&default_effort.as_str()) => {
                    l.default_effort.clone().or_else(|| l.efforts.first().cloned()).unwrap_or_default()
                }
                _ => default_effort,
            }
        }
    };
    Ok((model, effort))
}

/// The settings a helper runs with: what the request asks for over the
/// account's defaults, refused in the words the new-chat dialog would use.
fn background_spec(params: &SpawnParams, provider: Provider, account_path: &str, folder: &str) -> Result<crate::background::Spec, String> {
    let tools = params.tools.unwrap_or(false);
    match provider {
        Provider::Claude => {
            if asked(&params.approval).is_some() || asked(&params.sandbox).is_some() {
                return Err("approval and sandbox are Codex settings; Claude takes permissionMode".into());
            }
            let d = crate::claude::defaults::flags(account_path, folder);
            // The account's own list (claude/models.rs), or the four aliases
            // where Luna knows none yet.
            let models = crate::claude::models::models_known(account_path);
            let (model, effort) =
                claude_model_and_effort(&models, asked(&params.model), asked(&params.effort), d["model"].as_str(), |m| match m {
                    Some(m) => crate::claude::defaults::effort_of(account_path, folder, m),
                    None => d["effort"].as_str().unwrap_or("high").to_owned(),
                })?;
            // Leave a configured mode to the CLI, and pass Luna's Auto when
            // no settings file chose a mode.
            let (mode, own_mode) = match asked(&params.permission_mode) {
                Some(p) if CLAUDE_MODES.contains(&p) => (p.to_string(), false),
                Some(p) => return Err(format!("unknown permission mode \"{p}\"; use {}", CLAUDE_MODES.join(", "))),
                None => (d["permissionMode"].as_str().unwrap_or(crate::claude::defaults::LUNA_MODE).to_string(), d["modeFrom"] == "cli"),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                model,
                effort,
                mode,
                own_mode,
                approval: None,
                auto_review: false,
                tools,
                read_only: params.read_only.unwrap_or(false),
                agent: params.agent.clone(),
                parent: None,
                connectors: params.connectors.unwrap_or(false),
                constraints: Vec::new(),
                token: None,
            })
        }
        Provider::Codex => {
            use crate::codex::defaults::Mode;
            use crate::codex::launch::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX};
            if asked(&params.permission_mode).is_some() {
                return Err("permissionMode is a Claude setting; Codex takes approval and sandbox".into());
            }
            let (d, mode) = crate::codex::defaults::helper_defaults(account_path, folder);
            let (approval_asked, sandbox_asked) = (asked(&params.approval), asked(&params.sandbox));
            // Luna defaults the pair only when the spawn leaves both unset.
            let (approval, auto_review) = match approval_asked {
                Some(AUTO_REVIEW) => (None, true),
                Some("never") => (Some("never".to_string()), false),
                Some("on-request" | "on-failure") => (Some("on-request".to_string()), false),
                Some(a) => return Err(format!("unknown approval \"{a}\"; use never, on-request or {AUTO_REVIEW}")),
                None => match mode {
                    Mode::AutoReview if sandbox_asked.is_none() => (None, true),
                    Mode::Closed => (Some(crate::codex::defaults::CLOSED_APPROVAL.to_string()), false),
                    _ => (None, false),
                },
            };
            let (sandbox, own_mode) = match sandbox_asked {
                Some(s) if auto_review && s != AUTO_REVIEW_SANDBOX => {
                    return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX} only; leave sandbox out"))
                }
                Some(s) if ["read-only", crate::codex::launch::READ_ONLY_NET, "workspace-write", "danger-full-access"].contains(&s) => {
                    (s.to_string(), false)
                }
                Some(s) => {
                    return Err(format!("unknown sandbox \"{s}\"; use read-only, read-only-net, workspace-write, danger-full-access"))
                }
                None if auto_review => (AUTO_REVIEW_SANDBOX.to_string(), false),
                None => match mode {
                    Mode::Closed => (crate::codex::defaults::CLOSED_SANDBOX.to_string(), false),
                    _ => (crate::codex::defaults::OWN.to_string(), true),
                },
            };
            let models = crate::codex::models::models_known(account_path);
            let model = asked(&params.model).map(str::to_owned).or_else(|| d["model"].as_str().map(str::to_owned));
            let listed = model.as_deref().and_then(|m| models.iter().find(|x| x.slug == m));
            if let (Some(m), None, false) = (asked(&params.model), listed, models.is_empty()) {
                let all: Vec<&str> = models.iter().map(|x| x.slug.as_str()).collect();
                return Err(format!("this Codex account does not list model \"{m}\"; it has {}", all.join(", ")));
            }
            // As in the dialog: a model asked for brings its own default level.
            let effort = match (asked(&params.effort), listed) {
                (Some(e), Some(l)) if !l.efforts.is_empty() && !l.efforts.iter().any(|x| x == e) => {
                    return Err(format!("{} takes effort {}, not \"{e}\"", l.slug, l.efforts.join(", ")))
                }
                // No list of the account's own to hold it to (the account has
                // not run, or its model is not in the list): as asked, whatever
                // its case, unless Codex's built-in catalog was read this run
                // and does not have it. A word, never text for a `-c`
                // (it is given as a TOML string all the same).
                (Some(e), _) if e.is_empty() || e.len() > 32 || !e.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') => {
                    return Err(format!("\"{e}\" is not an effort level; use a word such as low, medium or high"))
                }
                (Some(e), _) => match crate::codex::models::known_efforts() {
                    Some(known) if !known.iter().any(|k| k == e) => {
                        return Err(format!("Codex takes effort {}, not \"{e}\"", known.join(", ")))
                    }
                    _ => e.to_string(),
                },
                (None, Some(l)) if asked(&params.model).is_some() && l.default_effort.is_some() => {
                    l.default_effort.clone().unwrap_or_default()
                }
                (None, _) => d["effort"].as_str().unwrap_or("medium").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                model,
                effort,
                mode: sandbox,
                own_mode,
                approval,
                auto_review,
                tools,
                read_only: false,
                agent: None,
                parent: None,
                connectors: params.connectors.unwrap_or(false),
                constraints: Vec::new(),
                token: None,
            })
        }
    }
}

/// `background_spec` for a spawn that names nothing.
#[cfg(test)]
pub(crate) fn background_spec_for_test(provider: Provider, account_path: &str, folder: &str) -> crate::background::Spec {
    background_spec(&SpawnParams::default(), provider, account_path, folder).unwrap()
}

fn mode_source(params: &SpawnParams, spec: &crate::background::Spec) -> Option<&'static str> {
    if asked(&params.permission_mode).or(asked(&params.approval)).or(asked(&params.sandbox)).is_some() {
        None
    } else if spec.own_mode {
        Some("cli")
    } else if spec.provider == Provider::Claude || spec.auto_review {
        Some("luna")
    } else {
        Some("closed")
    }
}

/// `spawn` for a helper with no row: started here, in the core, on the
/// settings `background_spec` works out.
pub(in crate::agents) fn spawn_background(
    caller: &str,
    params: SpawnParams,
    account_path: String,
    in_worktree: Option<String>,
    request: u64,
    constraints: Vec<crate::agents::Constraint>,
) -> Result<Spawned, String> {
    let provider = if params.provider.as_deref() == Some("codex") { Provider::Codex } else { Provider::Claude };
    let project = params.folder.clone().unwrap_or_default();
    // A helper handed its caller's worktree works in it.
    let run_in = in_worktree.clone().unwrap_or_else(|| project.clone());
    let mut spec = background_spec(&params, provider, &account_path, &run_in)?;
    spec.parent = Some(caller.to_string());
    let id = crate::background::mint_id();
    let mut settings = match provider {
        Provider::Claude => crate::claude::launch::with_role(
            serde_json::json!({
                "model": spec.model, "effort": spec.effort, "permissionMode": spec.mode, "background": true,
            }),
            spec.read_only,
            spec.agent.as_deref(),
        ),
        Provider::Codex => serde_json::json!({
            "model": spec.model, "effort": spec.effort,
            "approval": match (&spec.approval, spec.auto_review) {
                (_, true) => crate::codex::launch::AUTO_REVIEW,
                (Some(a), false) => a.as_str(),
                (None, false) => crate::codex::defaults::OWN,
            },
            "sandbox": spec.mode,
            "background": true,
        }),
    };
    if params.resume_on_restart == Some(true) {
        settings["resumeOnRestart"] = serde_json::json!(true);
    }
    if let Some(budget) = params.budget_tokens {
        settings["budgetTokens"] = serde_json::json!(budget);
    }
    if let Some(pct) = params.budget_pct {
        settings["budgetPct"] = serde_json::json!(pct);
    }
    if spec.connectors {
        settings["connectors"] = serde_json::json!(true);
    }
    if let Some(source) = mode_source(&params, &spec) {
        settings["modeFrom"] = serde_json::json!(source);
    }
    // Kept in the registry too, where `luna_session_get` shows them and every
    // spawn of its own reads them to pass on (`constraints_for`).
    if !constraints.is_empty() {
        settings["constraints"] = serde_json::json!(constraints);
    }
    spec.constraints = constraints;
    let tools = spec.tools;
    let auto_delete = params.auto_delete.unwrap_or(true);
    let token = register(&id, provider, &run_in, &account_path, Some(caller), tools, settings.clone());
    // A token only with Luna's tools: a Claude Code helper's asks come on its
    // own stdout, not through Luna's server.
    spec.token = token;
    let name = params
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .or_else(|| crate::pty::title_from_prompt(&params.prompt))
        .unwrap_or_else(|| "helper".into());
    set_name(&id, &name);
    {
        let mut r = reg().lock().unwrap_or_else(|e| e.into_inner());
        // Nobody sees it to delete it: it goes once its answer is out,
        // unless its caller means to delete it itself.
        if auto_delete {
            r.auto.insert(id.clone(), Auto::default());
        }
    }
    if let Err(e) = spawn_is(request, &id) {
        forget(&id);
        return Err(e);
    }
    if let Err(e) = crate::background::start_on(&id, spec, &params.prompt, Some(request)) {
        forget(&id);
        return Err(e);
    }
    crate::agentlog::spawned(&crate::agentlog::Spawn {
        id: &id,
        parent: caller,
        parent_name: chat_name(caller),
        folder: &project,
        provider: provider.as_str(),
        account: params.account_name().unwrap_or(""),
        settings: &settings,
        name: Some(name),
        prompt: &params.prompt,
        auto_delete,
        background: true,
    });
    let same_folder = neighbors_in(&run_in, caller, &[caller, &id]);
    shown(caller, &id, 0);
    owe_asked(caller, &id, 0, 0, &Asked { text: &params.prompt, key: params.request_key.as_deref(), reply: true });
    Ok(Spawned { id, turns_ended: 0, settings, same_folder, account_choice: None, places: Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::spawn::SpawnParams;

    #[test]
    fn read_only_net_goes_with_never_and_not_with_auto_review() {
        let spec = |approval: &str| {
            let p = SpawnParams { sandbox: Some("read-only-net".into()), approval: Some(approval.into()), ..Default::default() };
            background_spec(&p, Provider::Codex, "", "")
        };
        assert_eq!(spec("never").ok().unwrap().mode, "read-only-net");
        let effort = |e: &str| background_spec(&SpawnParams { effort: Some(e.into()), ..Default::default() }, Provider::Codex, "", "");
        assert!(effort("high\"\nx=1").err().unwrap().contains("not an effort level"), "no list: a plain word only");
        assert_eq!(effort("high").ok().unwrap().effort, "high");
        assert_eq!(effort("XHigh").ok().unwrap().effort, "XHigh", "no list: passed as asked, not lowercased or refused");
        // Codex's built-in catalog, once read, is the set to hold an effort to
        // where the account has no list of its own.
        crate::codex::models::set_known_efforts_for_test(Some(&["low", "medium", "high"]));
        let why = effort("ultra").err().unwrap();
        assert!(why.contains("Codex takes effort low, medium, high"), "{why}");
        assert_eq!(effort("medium").ok().unwrap().effort, "medium");
        crate::codex::models::set_known_efforts_for_test(None);
        assert!(spec("auto-review").err().unwrap().contains("workspace-write only"), "the reviewer's sandbox is fixed");
    }

    /// A Codex helper asks its caller (background/appserver.rs): the
    /// approval its parent names is passed, on-failure as on-request.
    #[test]
    fn a_codex_helper_runs_in_the_approval_its_parent_names() {
        let spec = |approval: &str| {
            let p = SpawnParams { sandbox: Some("workspace-write".into()), approval: Some(approval.into()), ..Default::default() };
            background_spec(&p, Provider::Codex, "", "")
        };
        let named = |a: &str| spec(a).map(|s| (s.approval, s.auto_review));
        assert_eq!(named("never"), Ok((Some("never".into()), false)));
        assert_eq!(named("on-request"), Ok((Some("on-request".into()), false)));
        assert_eq!(named("on-failure"), Ok((Some("on-request".into()), false)));
        assert_eq!(named("auto-review"), Ok((None, true)));
        assert!(spec("untrusted").err().unwrap().contains("unknown approval"), "0.169.0 refuses to start with it");
    }

    #[test]
    fn codex_helper_modes_follow_the_spawn_then_config_then_luna() {
        let root = std::env::temp_dir().join(format!("luna-spawn-own-mode-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (account, repo) = (root.join("account"), root.join("repo"));
        std::fs::create_dir_all(&account).unwrap();
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let (acc, dir) = (account.to_string_lossy().into_owned(), repo.to_string_lossy().into_owned());
        let omitted = SpawnParams::default();
        let spec = || background_spec(&omitted, Provider::Codex, &acc, &dir).unwrap();
        let s = spec();
        assert_eq!((s.mode.as_str(), s.own_mode, s.approval, s.auto_review), ("workspace-write", false, None, true));
        assert_eq!(mode_source(&omitted, &spec()), Some("luna"));
        for config in
            ["sandbox_mode = \"danger-full-access\"\n", "default_permissions = \"mine\"\n", "approvals_reviewer = \"auto_review\"\n"]
        {
            std::fs::write(account.join("config.toml"), config).unwrap();
            let s = spec();
            assert_eq!((s.mode.as_str(), s.own_mode, s.approval.as_deref(), s.auto_review), ("codex", true, None, false), "{config}");
            assert_eq!(mode_source(&omitted, &s), Some("cli"));
        }
        // A value Luna does not know, in the account's or the repository's layer.
        std::fs::write(account.join("config.toml"), "approval_policy = \"sometimes\"\n").unwrap();
        let s = spec();
        assert_eq!((s.mode.as_str(), s.own_mode, s.approval.as_deref()), ("read-only", false, Some("on-request")));
        assert_eq!(mode_source(&omitted, &s), Some("closed"));
        std::fs::write(account.join("config.toml"), "").unwrap();
        std::fs::create_dir_all(repo.join(".codex")).unwrap();
        std::fs::write(repo.join(".codex").join("config.toml"), "sandbox_mode = [broken\n").unwrap();
        let s = spec();
        assert_eq!((s.mode.as_str(), s.own_mode, s.approval.as_deref()), ("read-only", false, Some("on-request")));
        // Named, it is passed as named.
        let p = SpawnParams { sandbox: Some("workspace-write".into()), ..Default::default() };
        let s = background_spec(&p, Provider::Codex, &acc, &dir).unwrap();
        assert_eq!((s.mode.as_str(), s.own_mode), ("workspace-write", false));
        assert_eq!(mode_source(&p, &s), None);
        std::fs::remove_file(repo.join(".codex").join("config.toml")).unwrap();
        let s = background_spec(&p, Provider::Codex, &acc, &dir).unwrap();
        assert_eq!((s.mode.as_str(), s.approval, s.auto_review), ("workspace-write", None, false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claude_helper_modes_follow_the_spawn_then_config_then_luna() {
        let root = std::env::temp_dir().join(format!("luna-spawn-claude-mode-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let account = root.to_string_lossy();
        let spec = |p| background_spec(p, Provider::Claude, &account, "").unwrap();
        let omitted = SpawnParams::default();
        assert_eq!(mode_source(&omitted, &spec(&omitted)), Some("luna"));
        let s = spec(&omitted);
        assert_eq!((s.mode.as_str(), s.own_mode), ("auto", false));
        std::fs::write(root.join("settings.json"), r#"{ "permissions": { "defaultMode": "dontAsk" } }"#).unwrap();
        let s = spec(&omitted);
        assert_eq!((s.mode.as_str(), s.own_mode, mode_source(&omitted, &s)), ("dontAsk", true, Some("cli")));
        let explicit = SpawnParams { permission_mode: Some("plan".into()), ..Default::default() };
        let s = spec(&explicit);
        assert_eq!((s.mode.as_str(), s.own_mode, mode_source(&explicit, &s)), ("plan", false, None));
        let _ = std::fs::remove_dir_all(&root);
    }
    fn model(slug: &str, efforts: &[&str], default: Option<&str>) -> crate::claude::models::ClaudeModel {
        crate::claude::models::ClaudeModel {
            slug: slug.into(),
            model_id: None,
            display_name: slug.into(),
            default_effort: default.map(str::to_owned),
            efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
            effort_hints: Default::default(),
            more: false,
        }
    }

    #[test]
    fn a_claude_helpers_effort_is_held_to_the_model_it_will_run_on() {
        let list = [
            model("opus", &["low", "medium", "high", "xhigh", "max"], Some("medium")),
            model("claude-opus-4-7", &["medium", "xhigh"], Some("xhigh")),
            model("claude-haiku-4-5-20251001", &[], None),
        ];
        let pick = |m, e, dm, de: &str| claude_model_and_effort(&list, m, e, dm, |_| de.to_owned());
        // The model from the account's settings, not asked for: its levels count.
        assert!(pick(None, Some("low"), Some("claude-opus-4-7"), "high").unwrap_err().contains("takes effort medium, xhigh"));
        assert_eq!(pick(None, Some("xhigh"), Some("claude-opus-4-7"), "high").unwrap().1, "xhigh");
        // A default level the model does not take moves to its own default.
        assert_eq!(pick(None, None, Some("claude-opus-4-7"), "high").unwrap().1, "xhigh");
        assert_eq!(pick(Some("opus"), None, None, "high").unwrap().1, "high");
        // A model that takes no effort runs with none, asked for or by default.
        assert_eq!(
            pick(Some("claude-haiku-4-5-20251001"), None, None, "high").unwrap(),
            (Some("claude-haiku-4-5-20251001".into()), String::new())
        );
        assert_eq!(pick(None, None, Some("claude-haiku-4-5-20251001"), "high").unwrap().1, "");
        assert!(pick(None, Some("high"), Some("claude-haiku-4-5-20251001"), "high").unwrap_err().contains("takes no effort"));
        // Unknown models and levels are refused; a family spelling passes.
        assert!(pick(Some("gpt-6"), None, None, "high").is_err());
        assert!(pick(Some("opusplan"), Some("turbo"), None, "high").unwrap_err().contains("unknown Claude effort"));
        assert_eq!(pick(Some("opus"), Some("ultracode"), None, "high").unwrap().1, "ultracode");
        // The default level is the one of the model it runs on, asked for or not.
        let per_model = |m: Option<&str>| if m == Some("opus") { "low".to_owned() } else { "high".to_owned() };
        assert_eq!(claude_model_and_effort(&list, Some("opus"), None, Some("claude-opus-4-7"), per_model).unwrap().1, "low");
        assert_eq!(claude_model_and_effort(&list, None, None, Some("opus"), per_model).unwrap().1, "low");
    }
}
