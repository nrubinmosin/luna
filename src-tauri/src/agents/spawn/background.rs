//! A spawn as a background helper: the `Spec` background.rs runs it from, and
//! the call that waits for its first turn.

use crate::agents::delivery::POLL;
use crate::agents::owed::{owe_from, shown};
use crate::agents::registry::{chat_name, issue_token, reg, register, set_name};
use crate::agents::spawn::worktree::{helper_worktree_name, neighbors_in, worktree_of, Worktree};
use crate::agents::spawn::{spawn_is, SpawnParams, Spawned};
use crate::agents::teardown::{forget, Auto};
use crate::provider::Provider;

/// How long a background spawn in a worktree of its own waits for the CLI
/// to say where it made it.
const WORKTREE_WAIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Claude Code's model aliases, as the new-chat dialog lists them.
pub const CLAUDE_MODELS: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

const CLAUDE_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultracode"];

pub(in crate::agents) const CLAUDE_MODES: [&str; 4] = ["default", "acceptEdits", "plan", "bypassPermissions"];

/// Given and not blank.
pub(in crate::agents) fn asked(v: &Option<String>) -> Option<&str> {
    v.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

/// The settings a background helper runs with: what the request asks for
/// over the account's defaults, refused in the same words the window uses
/// for a visible one (agents/bridge.ts).
fn background_spec(params: &SpawnParams, provider: Provider, account_path: &str, folder: &str) -> Result<crate::background::Spec, String> {
    let worktree = params.worktree.unwrap_or(false);
    let tools = params.tools.unwrap_or(false);
    match provider {
        Provider::Claude => {
            if asked(&params.approval).is_some() || asked(&params.sandbox).is_some() {
                return Err("approval and sandbox are Codex settings; Claude takes permissionMode".into());
            }
            let d = crate::claude::defaults::flags(account_path, folder);
            let model = match asked(&params.model) {
                Some(m) if CLAUDE_MODELS.iter().any(|a| m.to_lowercase().contains(a)) => Some(m.to_string()),
                Some(m) => {
                    return Err(format!(
                        "unknown Claude model \"{m}\"; use {} or a full id such as claude-opus-5",
                        CLAUDE_MODELS.join(", ")
                    ))
                }
                None => d["model"].as_str().map(str::to_owned),
            };
            let effort = match asked(&params.effort) {
                Some(e) if CLAUDE_EFFORTS.contains(&e) => e.to_string(),
                Some(e) => return Err(format!("unknown Claude effort \"{e}\"; use {}", CLAUDE_EFFORTS.join(", "))),
                None => d["effort"].as_str().unwrap_or("high").to_string(),
            };
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
                token: None,
            })
        }
        Provider::Codex => {
            use crate::pty::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX};
            if asked(&params.permission_mode).is_some() {
                return Err("permissionMode is a Claude setting; Codex takes approval and sandbox".into());
            }
            // Nobody is there to ask, but a reviewer model can answer.
            let auto_review = match asked(&params.approval) {
                None | Some("never") => false,
                Some(AUTO_REVIEW) => true,
                Some(_) => {
                    return Err(format!(
                        "a background Codex helper has nobody to ask for approval; leave approval out, \
                         use {AUTO_REVIEW}, or spawn it with visible: true"
                    ))
                }
            };
            let d = crate::codex::defaults::flags(account_path, folder);
            let sandbox = match asked(&params.sandbox) {
                Some(s) if auto_review && s != AUTO_REVIEW_SANDBOX => {
                    return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX} only; leave sandbox out"))
                }
                Some(s) if ["read-only", crate::pty::READ_ONLY_NET, "workspace-write", "danger-full-access"].contains(&s) => s.to_string(),
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
        Provider::Claude => crate::pty::with_role(
            serde_json::json!({
                "model": spec.model, "effort": spec.effort, "permissionMode": spec.mode, "worktree": own_worktree, "background": true,
            }),
            spec.read_only,
            spec.agent.as_deref(),
        ),
        Provider::Codex => serde_json::json!({
            "model": spec.model, "effort": spec.effort,
            "approval": if spec.auto_review { crate::pty::AUTO_REVIEW } else { "never" }, "sandbox": spec.mode,
            "hooksUnasked": spec.hooks_unasked, "background": true,
        }),
    };
    if params.resume_on_restart == Some(true) {
        settings["resumeOnRestart"] = serde_json::json!(true);
    }
    if let Some(budget) = params.budget_tokens {
        settings["budgetTokens"] = serde_json::json!(budget);
    }
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
    let same_folder = if own_worktree { Vec::new() } else { neighbors_in(&run_in, &[caller, &id]) };
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
            if worktree.path.is_some() || std::time::Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL);
        }
    }
    let worktree_pending = own_worktree && worktree.path.is_none();
    Ok(Spawned {
        id,
        turns_ended: 0,
        settings,
        started: true,
        screen: None,
        ask: None,
        starting: false,
        warnings: Vec::new(),
        same_folder,
        worktree,
        worktree_pending,
        account_choice: None,
    })
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
}
