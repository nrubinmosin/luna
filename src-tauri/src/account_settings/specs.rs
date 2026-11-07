//! The settings Luna recommends for anyone running a CLI inside it, each with
//! why, and what an account's files say of each now.

use super::*;

/// One recommendation as the dialog lists it.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Rec {
    /// Dotted path, e.g. `tui` or `env.CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY`.
    pub key: String,
    /// What the file says now, as written there; None when it says nothing.
    pub current: Option<String>,
    pub recommended: String,
    pub why: String,
    /// Part of the set "Tick recommended" picks. Off for what is a matter of
    /// taste.
    pub default_on: bool,
    /// The file already says exactly this.
    pub applied: bool,
    /// The file it is kept in, when that is not the account's settings file.
    pub file: Option<String>,
}

/// A recommendation in the making: the path, the value, and who it is for.
pub(super) struct Spec {
    pub(super) path: &'static [&'static str],
    pub(super) value: Value,
    why: &'static str,
    default_on: bool,
    /// Windows only, e.g. a PowerShell default.
    windows_only: bool,
    /// In Claude Code's `.claude.json` rather than its `settings.json`: a few
    /// of its settings are read from there only.
    pub(super) global: bool,
}

fn spec(path: &'static [&'static str], value: Value, why: &'static str, default_on: bool) -> Spec {
    Spec { path, value, why, default_on, windows_only: false, global: false }
}

const COPY_ON_SELECT_WHY: &str = "Selecting in a pane copies nothing until Ctrl+C, so the clipboard keeps what you put there.";

/// Claude Code 2.1, every key checked against the CLI's own settings schema.
fn claude_specs() -> Vec<Spec> {
    vec![
        spec(
            &["tui"],
            json!("fullscreen"),
            "Draws on the alternate screen with its own scrollback: no flicker, and a pane resize redraws cleanly.",
            true,
        ),
        spec(&["spinnerTipsEnabled"], json!(false), "No tips under the spinner.", true),
        spec(&["feedbackDrafts"], json!("off"), "No feedback drafts queued by the model.", true),
        spec(&["env", "CLAUDE_CODE_DISABLE_FEEDBACK_SURVEY"], json!("1"), "No feedback survey mid-session.", true),
        spec(
            &["idleCompaction"],
            json!(false),
            "No compacting a long session while it sits idle: its context stays whole until it fills the window.",
            true,
        ),
        spec(
            &["skipDangerousModePermissionPrompt"],
            json!(true),
            "Bypass is a choice made in Luna's new-chat dialog; the CLI need not ask again.",
            true,
        ),
        Spec {
            windows_only: true,
            ..spec(&["defaultShell"], json!("powershell"), "`!` commands in the input box run in PowerShell instead of bash.", true)
        },
        Spec { global: true, ..spec(&["copyOnSelect"], json!(false), COPY_ON_SELECT_WHY, true) },
        spec(&["cleanupPeriodDays"], json!(14), "Transcripts older than two weeks are cleaned up (default 30).", false),
        spec(
            &["attribution"],
            json!({ "commit": "", "pr": "", "sessionUrl": false }),
            "No Co-Authored-By or session links in commits and PRs.",
            false,
        ),
        spec(&["disableRemoteControl"], json!(true), "No Remote Control for sessions on this account.", false),
        spec(&["permissions", "defaultMode"], json!("bypassPermissions"), "New sessions start in Bypass.", false),
    ]
}

/// Codex 0.165, the same comforts in Codex's words; every key checked against
/// the CLI's config structs.
fn codex_specs() -> Vec<Spec> {
    vec![
        spec(&["check_for_update_on_startup"], json!(false), "Luna keeps Codex up to date itself.", true),
        spec(&["tui", "show_tooltips"], json!(false), "No tips in the composer.", true),
        spec(
            &["tui", "notifications"],
            json!(false),
            "No desktop notifications from Codex: Luna says when a chat is waiting for you.",
            true,
        ),
        spec(&["tui", "copy_on_select"], json!("never"), COPY_ON_SELECT_WHY, true),
        spec(&["analytics", "enabled"], json!(false), "No usage analytics.", true),
        spec(&["feedback", "enabled"], json!(false), "No feedback prompts.", true),
        spec(&["web_search"], json!("live"), "Web search, as Claude Code has it.", true),
        spec(&["approval_policy"], json!("never"), "With the next one: new sessions start in Bypass.", false),
        spec(&["sandbox_mode"], json!("danger-full-access"), "With the one above: no sandbox.", false),
    ]
}

pub(super) fn specs(provider: Provider) -> Vec<Spec> {
    let all = match provider {
        Provider::Claude => claude_specs(),
        Provider::Codex => codex_specs(),
    };
    all.into_iter().filter(|s| !s.windows_only || cfg!(windows)).collect()
}

pub(super) fn recommend(provider: Provider, account_path: &str) -> Result<Vec<Rec>, String> {
    let mut out = vec![];
    match provider {
        Provider::Claude => {
            let docs = [read_claude(&claude_spec_file(account_path, false)?)?, read_claude(&claude_spec_file(account_path, true)?)?];
            for s in specs(provider) {
                let current = json_at(&docs[s.global as usize], s.path);
                out.push(Rec {
                    key: s.path.join("."),
                    current: current.map(Value::to_string),
                    recommended: s.value.to_string(),
                    why: s.why.into(),
                    default_on: s.default_on,
                    applied: current == Some(&s.value),
                    file: s.global.then(|| CLAUDE_GLOBAL.into()),
                });
            }
        }
        Provider::Codex => {
            let doc = crate::codex::config::read(&crate::codex::config::account_file(account_path)?)?;
            for s in specs(provider) {
                let current = toml_at(&doc, s.path).and_then(Item::as_value).map(plain);
                let recommended = toml_value(&s.value).as_ref().map(plain).unwrap_or_default();
                out.push(Rec {
                    key: s.path.join("."),
                    applied: current.as_deref() == Some(recommended.as_str()),
                    current,
                    recommended,
                    why: s.why.into(),
                    default_on: s.default_on,
                    file: None,
                });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_powershell_default_is_only_offered_on_windows() {
        let offered = specs(Provider::Claude).iter().any(|s| s.path == ["defaultShell"]);
        assert_eq!(offered, cfg!(windows));
    }
}
