//! What a chat with auto-handoff on is served to write its handoff by
//! (docs/adr/0022-content-luna-serves-at-its-own-trigger.md).
//! `luna_session_handoff` without `file` answers with the procedure and the
//! instructions library/system.rs resolves, Luna's own or the library's
//! override, and keeps on the chat's record which text that was, for the
//! handoff written by it. The chat's What Luna applied panel shows the same,
//! and warns of a skill of the same name its CLI may list to the model: Luna
//! serves only its own or the override, never that one.

use super::{facts, store};
use crate::agents::registry::{label, reg};
use crate::library::{Resolved, Source, SystemSkillState, HANDOFF};
use crate::provider::Provider;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The instructions a chat was served, as its handoffs' record keeps them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) struct Served {
    pub source: Source,
    pub sha256: String,
    /// Luna's version, for its own text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The override's path, for the library's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl Served {
    pub(super) fn of(r: &Resolved) -> Served {
        let (version, path) = match r.source {
            Source::Luna => (Some(env!("CARGO_PKG_VERSION").to_string()), None),
            Source::Library => (None, Some(r.path.to_string_lossy().into_owned())),
        };
        Served { source: r.source, sha256: r.sha256.clone(), version, path }
    }
}

/// Whose instructions they are, as the answer says it: an override with where
/// it is, for what it names beside it; Luna's own with why an override there
/// is not used.
fn source_words(r: &Resolved) -> String {
    let luna = format!("Luna's own, {}", env!("CARGO_PKG_VERSION"));
    match (r.source, &r.problem) {
        (Source::Library, _) => {
            format!("the person's, from Luna's library at {}; what they name beside them is in that folder", r.path.display())
        }
        (Source::Luna, Some(why)) => format!("{luna}; the library's override at {} is not used: {why}", r.path.display()),
        (Source::Luna, None) => luna,
    }
}

/// `luna_session_handoff` without `file`, for `caller`'s own chat: how to hand
/// off, where the file goes, what Luna adds itself, and the instructions now
/// in effect. Which text it was is kept for the handoff that follows; a record
/// that will not save costs that note, not the answer.
pub(super) fn instructions(caller: &str) -> Result<String, String> {
    if !super::offers_tool(caller) {
        return Err(super::NOT_OFFERED.into());
    }
    if let Some(n) = store::records().get(caller).and_then(|r| r.switch.as_ref().map(|s| s.n)) {
        return Err(format!("handoff {n} is under way already; end your turn"));
    }
    let r = crate::library::handoff_instructions();
    let served = Served::of(&r);
    if let Err(e) = store::change(caller, |rec| rec.served = Some(served)) {
        crate::log::warn("handoff", &format!("{}: which instructions it was served could not be saved: {e}", label(caller)));
    }
    let said = match (&r.source, &r.problem) {
        (Source::Library, _) => format!("the library's override {}", r.path.display()),
        (Source::Luna, Some(why)) => format!("Luna's own (the override will not do: {why})"),
        (Source::Luna, None) => "Luna's own".to_string(),
    };
    crate::log::info("handoff", &format!("{} was served the handoff's instructions: {said}, sha256 {}", label(caller), r.short_sha()));
    // A name nobody can have put there first: on a machine shared with others
    // the system's temp folder is theirs too. Luna writes nothing there itself.
    let suggested = std::env::temp_dir().join(format!("luna-handoff-{}.md", crate::secret::random_hex(8)));
    let data = crate::paths::data_dir();
    Ok(facts::how_to(&source_words(&r), &r.sha256, r.body(), &data.to_string_lossy(), &suggested.to_string_lossy()))
}

/// Whether a handoff file is looked at for the sections of Luna's checklist
/// (facts.rs `missing_sections`): where the chat was served Luna's own
/// instructions, or none and Luna's own are in effect, so that the firm notice
/// carried their checklist. An override's sections are its author's.
pub(super) fn checks_sections(served: Option<&Served>) -> bool {
    match served {
        Some(s) => s.source == Source::Luna,
        None => crate::library::handoff_instructions().source == Source::Luna,
    }
}

/// What the chat's What Luna applied panel shows of its handoff's instructions.
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffApplied {
    /// What `luna_session_handoff` would serve it now.
    pub instructions: SystemSkillState,
    /// Skills of the same name its CLI may list to the model, by their
    /// SKILL.md: in its folder or its account. Never served for the handoff.
    pub same_named: Vec<String>,
}

/// The panel's word on `chat`'s handoff instructions: none for a chat that is
/// not offered the tool.
pub fn applied_of(chat: &str) -> Option<HandoffApplied> {
    if !super::offers_tool(chat) {
        return None;
    }
    let (provider, folder, account) = {
        let r = reg().lock().unwrap_or_else(|e| e.into_inner());
        let k = r.known.get(chat)?;
        (k.provider, k.folder.clone(), k.account_path.clone())
    };
    let instructions = crate::library::handoff_instructions().state();
    Some(HandoffApplied { instructions, same_named: same_named(provider, Path::new(&folder), Path::new(&account)) })
}

/// The skills named as the handoff's instructions that a chat of `provider` in
/// `folder` on the account at `account` may be listed: the account's own
/// (`skills/`, both CLIs) and the folder's, where its CLI reads them (Claude
/// Code `.claude/skills`, Codex `.agents/skills`).
pub(super) fn same_named(provider: Provider, folder: &Path, account: &Path) -> Vec<String> {
    let project = match provider {
        Provider::Claude => folder.join(".claude").join("skills"),
        Provider::Codex => folder.join(".agents").join("skills"),
    };
    [project, account.join("skills")]
        .into_iter()
        .map(|skills| skills.join(HANDOFF).join("SKILL.md"))
        .filter(|p| p.is_file())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(source: Source, problem: Option<&str>) -> Resolved {
        Resolved {
            source,
            text: "---\nname: session-handoff\ndescription: x\n---\nBody\n".into(),
            sha256: "0123456789abcdef".into(),
            path: "/lib/system/session-handoff/SKILL.md".into(),
            problem: problem.map(str::to_owned),
        }
    }

    #[test]
    fn what_was_served_is_kept_by_whose_and_which() {
        let own = Served::of(&resolved(Source::Luna, None));
        assert_eq!((own.source, own.version.as_deref(), own.path.as_deref()), (Source::Luna, Some(env!("CARGO_PKG_VERSION")), None));
        let theirs = Served::of(&resolved(Source::Library, None));
        assert_eq!((theirs.version.as_deref(), theirs.path.as_deref()), (None, Some("/lib/system/session-handoff/SKILL.md")));
        assert_eq!(theirs.sha256, "0123456789abcdef");
        // Kept on disk: it reads back as it was.
        let back: Served = serde_json::from_value(serde_json::to_value(&theirs).unwrap()).unwrap();
        assert_eq!(back, theirs);
        // Where an override was there and would not do, the answer says so.
        assert!(source_words(&resolved(Source::Luna, Some("The front matter has no description")))
            .contains("override at /lib/system/session-handoff/SKILL.md is not used: The front matter has no description"));
        assert!(source_words(&resolved(Source::Library, None)).starts_with("the person's, from Luna's library at /lib/"));
        assert!(!checks_sections(Some(&theirs)) && checks_sections(Some(&own)));
    }
}
