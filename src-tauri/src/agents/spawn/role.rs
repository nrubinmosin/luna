//! A spawn's `readOnly` and `agent`, settled before anything is made.

use crate::agents::spawn::background::asked;
use crate::agents::spawn::SpawnParams;
use crate::provider::Provider;

/// An agent's name as Claude Code takes one, a plugin's `plugin:name` too, and
/// a guard against one being read as a flag.
fn agent_name_ok(name: &str) -> bool {
    !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
}

/// `readOnly` and `agent` checked, for a helper however it starts: what cannot
/// hold together is refused, and a read-only Codex gets its sandbox here.
pub(super) fn settle_role(params: &mut SpawnParams, provider: Provider) -> Result<(), String> {
    use crate::codex::launch::{AUTO_REVIEW, AUTO_REVIEW_SANDBOX, READ_ONLY_NET};
    params.agent = asked(&params.agent).map(str::to_owned);
    if let Some(agent) = params.agent.as_deref() {
        if provider == Provider::Codex {
            return Err("agent is a Claude Code option: Codex cannot start a session as a named agent; its roles \
                        are for the subagents a session spawns itself"
                .into());
        }
        if !agent_name_ok(agent) {
            return Err(format!("\"{agent}\" is not an agent's name"));
        }
    }
    if !params.read_only.unwrap_or(false) {
        return Ok(());
    }
    match provider {
        Provider::Claude => {
            if asked(&params.permission_mode) == Some("acceptEdits") {
                return Err("readOnly takes the edit tools away, so acceptEdits has nothing to accept; leave one out".into());
            }
        }
        Provider::Codex => {
            if asked(&params.approval) == Some(AUTO_REVIEW) {
                return Err(format!("{AUTO_REVIEW} runs in {AUTO_REVIEW_SANDBOX}, which is not read-only; leave one out"));
            }
            match asked(&params.sandbox) {
                None => params.sandbox = Some("read-only".into()),
                Some("read-only") | Some(READ_ONLY_NET) => {}
                Some(s) => {
                    return Err(format!("readOnly contradicts sandbox \"{s}\"; leave sandbox out, or use read-only or {READ_ONLY_NET}"))
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(read_only: bool, f: impl FnOnce(&mut SpawnParams)) -> SpawnParams {
        let mut p = SpawnParams { read_only: Some(read_only), ..Default::default() };
        f(&mut p);
        p
    }

    #[test]
    fn read_only_codex_goes_to_the_read_only_sandbox_unless_told_which() {
        let mut p = role(true, |_| {});
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox.as_deref(), Some("read-only"), "no network unless asked for");
        let mut p = role(true, |p| p.sandbox = Some("read-only-net".into()));
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox.as_deref(), Some("read-only-net"));
        let mut p = role(false, |_| {});
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.sandbox, None, "Codex's own");
    }

    #[test]
    fn what_contradicts_read_only_is_refused() {
        for sandbox in ["workspace-write", "danger-full-access"] {
            let mut p = role(true, |p| p.sandbox = Some(sandbox.into()));
            assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("contradicts"), "{sandbox}");
        }
        let mut p = role(true, |p| p.approval = Some("auto-review".into()));
        assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("auto-review"));
        let mut p = role(true, |p| p.permission_mode = Some("acceptEdits".into()));
        assert!(settle_role(&mut p, Provider::Claude).unwrap_err().contains("acceptEdits"));
        // A checker with Bash and nobody to ask is the point of it.
        let mut p = role(true, |p| p.permission_mode = Some("bypassPermissions".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
    }

    #[test]
    fn an_agent_is_for_claude_code_and_a_name_only() {
        let mut p = role(false, |p| p.agent = Some("checker".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
        assert_eq!(p.agent.as_deref(), Some("checker"));
        assert!(settle_role(&mut p, Provider::Codex).unwrap_err().contains("Claude Code option"));
        let mut p = role(false, |p| p.agent = Some("  ".into()));
        settle_role(&mut p, Provider::Codex).unwrap();
        assert_eq!(p.agent, None, "blank is not asked");
        for bad in ["--dangerously-skip-permissions", "a b", "a/b"] {
            let mut p = role(false, |p| p.agent = Some(bad.into()));
            assert!(settle_role(&mut p, Provider::Claude).is_err(), "{bad}");
        }
        let mut p = role(false, |p| p.agent = Some("plugin:review-2.0_x".into()));
        settle_role(&mut p, Provider::Claude).unwrap();
    }
}
