//! A spawn as a background helper: the `Spec` background.rs runs it from, and
//! the call that waits for its first turn.

use crate::agents::delivery::POLL;
use crate::agents::owed::{owe_from, shown};
use crate::agents::registry::{chat_name, issue_token, reg, register, set_name};
use crate::agents::spawn::worktree::{helper_worktree_name, neighbors_in, worktree_of, worktree_queue, Worktree};
use crate::agents::spawn::{spawn_is, SpawnParams, Spawned};
use crate::agents::teardown::{forget, Auto};
use crate::provider::Provider;

/// How long a background spawn in a worktree of its own waits for the CLI
/// to say where it made it.
const WORKTREE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

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
    let worktree = params.worktree.unwrap_or(false);
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
            let mode = match asked(&params.permission_mode) {
                Some(p) if CLAUDE_MODES.contains(&p) => p.to_string(),
                Some(p) => return Err(format!("unknown permission mode \"{p}\"; use {}", CLAUDE_MODES.join(", "))),
                None => d["permissionMode"].as_str().unwrap_or("default").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                worktree,
                worktree_name: None,
                model,
                effort,
                mode,
                auto_review: false,
                tools,
                hooks_unasked: false,
                read_only: params.read_only.unwrap_or(false),
                agent: params.agent.clone(),
                parent: None,
                connectors: params.connectors.unwrap_or(false),
                constraints: Vec::new(),
                token: None,
            })
        }
        Provider::Codex => {
            use crate::codex::launch::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX};
            if asked(&params.permission_mode).is_some() {
                return Err("permissionMode is a Claude setting; Codex takes approval and sandbox".into());
            }
            // Nobody is there to ask, but a reviewer model can answer.
            let auto_review = match asked(&params.approval) {
                None | Some("never") => false,
                Some(AUTO_REVIEW) => true,
                Some(_) => {
                    return Err(format!(
                        "a Codex helper runs in the background with nobody to ask for approval; leave approval out, \
                         or use {AUTO_REVIEW}"
                    ))
                }
            };
            let d = crate::codex::defaults::flags(account_path, folder);
            let sandbox = match asked(&params.sandbox) {
                Some(s) if auto_review && s != AUTO_REVIEW_SANDBOX => {
                    return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX} only; leave sandbox out"))
                }
                Some(s) if ["read-only", crate::codex::launch::READ_ONLY_NET, "workspace-write", "danger-full-access"].contains(&s) => {
                    s.to_string()
                }
                Some(s) => {
                    return Err(format!("unknown sandbox \"{s}\"; use read-only, read-only-net, workspace-write, danger-full-access"))
                }
                None if auto_review => AUTO_REVIEW_SANDBOX.to_string(),
                None => d["sandbox"].as_str().unwrap_or("read-only").to_string(),
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
                (Some(e), _) => e.to_string(),
                (None, Some(l)) if asked(&params.model).is_some() && l.default_effort.is_some() => {
                    l.default_effort.clone().unwrap_or_default()
                }
                (None, _) => d["effort"].as_str().unwrap_or("medium").to_string(),
            };
            Ok(crate::background::Spec {
                provider,
                account_path: account_path.to_string(),
                folder: folder.to_string(),
                worktree,
                worktree_name: None,
                model,
                effort,
                mode: sandbox,
                auto_review,
                tools,
                hooks_unasked: false,
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
    let own_worktree = params.worktree.unwrap_or(false);
    // A helper handed its caller's worktree works in it and never owns it.
    let run_in = in_worktree.clone().unwrap_or_else(|| project.clone());
    let mut spec = background_spec(&params, provider, &account_path, &run_in)?;
    // As its project folder would start a chat of the user's: a helper does
    // not choose this for itself.
    spec.hooks_unasked = provider == Provider::Codex && crate::codex::hooks::unasked_in(&project);
    spec.parent = Some(caller.to_string());
    let id = crate::background::mint_id();
    spec.worktree_name = (own_worktree && provider == Provider::Claude).then(|| helper_worktree_name(&id));
    let mut settings = match provider {
        Provider::Claude => crate::claude::launch::with_role(
            serde_json::json!({
                "model": spec.model, "effort": spec.effort, "permissionMode": spec.mode, "worktree": own_worktree, "background": true,
            }),
            spec.read_only,
            spec.agent.as_deref(),
        ),
        Provider::Codex => serde_json::json!({
            "model": spec.model, "effort": spec.effort,
            "approval": if spec.auto_review { crate::codex::launch::AUTO_REVIEW } else { "never" }, "sandbox": spec.mode,
            "hooksUnasked": spec.hooks_unasked, "background": true,
        }),
    };
    if params.resume_on_restart == Some(true) {
        settings["resumeOnRestart"] = serde_json::json!(true);
    }
    if let Some(budget) = params.budget_tokens {
        settings["budgetTokens"] = serde_json::json!(budget);
    }
    if spec.connectors {
        settings["connectors"] = serde_json::json!(true);
    }
    // Where its mode came from when the spawn named none (privilege.rs `settle_mode`).
    if let Some(from) = params.mode_from {
        settings["modeFrom"] = serde_json::json!(from);
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
    // Claude Code asks its permissions of Luna, tools or not.
    spec.token = token.or_else(|| (provider == Provider::Claude).then(|| issue_token(&id)));
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
        if in_worktree.is_some() {
            r.borrowed.insert(id.clone());
        }
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
    // Its CLI makes its worktree as its first turn starts: one at a time per
    // repository (`worktree_queue`), held until it has said where, its turn
    // has ended without, or WORKTREE_WAIT is up.
    let queue = own_worktree.then(|| worktree_queue(&project));
    let making = queue.as_ref().map(|q| q.lock().unwrap_or_else(|e| e.into_inner()));
    if let Err(e) = crate::background::start(&id, spec, &params.prompt) {
        forget(&id);
        return Err(e);
    }
    crate::agentlog::spawned(&crate::agentlog::Spawn {
        id: &id,
        parent: caller,
        parent_name: chat_name(caller),
        folder: &project,
        worktree: own_worktree,
        provider: provider.as_str(),
        account: params.account_name().unwrap_or(""),
        settings: &settings,
        name: Some(name),
        prompt: &params.prompt,
        auto_delete,
        background: true,
    });
    let same_folder = if own_worktree { Vec::new() } else { neighbors_in(&run_in, caller, &[caller, &id]) };
    shown(caller, &id, 0);
    owe_from(caller, &id, 0, 0);
    // The CLI makes its worktree as the first turn starts: worth a few
    // seconds, so the caller need not go looking for it.
    let pty = crate::pty::manager();
    let mut worktree = Worktree::default();
    if own_worktree {
        let deadline = std::time::Instant::now() + WORKTREE_WAIT;
        loop {
            worktree = worktree_of(pty, &id);
            let ended = crate::activity::turn_state(&id).is_some_and(|t| t.turns_ended > 0);
            if worktree.path.is_some() || ended || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL);
        }
    }
    drop(making);
    let worktree_pending = own_worktree && worktree.path.is_none();
    let mcp_left_on = mcp_left_on(&id);
    Ok(Spawned {
        id,
        turns_ended: 0,
        settings,
        same_folder,
        worktree,
        worktree_pending,
        mcp_left_on,
        account_choice: None,
        places: Default::default(),
    })
}

/// What its first turn's "What Luna applied" lists as kept despite
/// `connectors: false` (codex/launch.rs `left_on_applied`), one per line.
fn mcp_left_on(id: &str) -> Vec<String> {
    let launch = crate::agents::applied_of(id).map(|(a, _, _)| a).unwrap_or_default();
    let key = crate::applied::Item::HelperMcpLeftOn.key();
    launch.iter().filter(|a| a.key == key).flat_map(|a| a.value.lines().map(str::to_owned)).collect()
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
        assert!(spec("auto-review").err().unwrap().contains("workspace-write only"), "the reviewer's sandbox is fixed");
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
