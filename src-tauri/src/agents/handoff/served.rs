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
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

/// What a chat was given to write its handoff by.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) enum Given {
    /// Luna's own instructions, whole, by the tool.
    Luna,
    /// The library's override, whole, by the tool.
    Library,
    /// Only the checklist the firm notice carries (facts.rs `checklist`).
    Checklist,
}

/// The instructions a chat was given, as its handoffs' record keeps them.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(in crate::agents) struct Served {
    pub source: Given,
    /// Of the text given.
    pub sha256: String,
    /// Luna's version, for its own text and its checklist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The override's path, for the library's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl Served {
    pub(super) fn of(r: &Resolved) -> Served {
        let (source, version, path) = match r.source {
            Source::Luna => (Given::Luna, Some(env!("CARGO_PKG_VERSION").to_string()), None),
            Source::Library => (Given::Library, None, Some(r.path.to_string_lossy().into_owned())),
        };
        Served { source, sha256: r.sha256.clone(), version, path }
    }

    /// The firm notice's checklist, as a chat that wrote by it alone was given it.
    pub(super) fn checklist() -> Served {
        let sha256 = Sha256::digest(facts::checklist().as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        Served { source: Given::Checklist, sha256, version: Some(env!("CARGO_PKG_VERSION").to_string()), path: None }
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
    // Its drafts folder, private and its own; the folder made the chat's
    // start makes it too, this call again where it went meanwhile.
    let drafts = super::drafts_of(caller)?;
    let settings = reg().lock().unwrap_or_else(|e| e.into_inner()).known.get(caller).map(|k| k.settings.clone()).unwrap_or_default();
    let suggested = new_draft(&drafts)?;
    Ok(facts::how_to(&source_words(&r), &r.sha256, r.body(), &suggested.to_string_lossy(), unwritable(&settings)))
}

/// A file for the chat to write its handoff in, in its `drafts` folder: a
/// name of its own, made here new, empty and private (`secret::create_private`,
/// which makes nothing where anything is), so the path named to the chat is a
/// plain file of this user's. A name used before could hold a link put there
/// while the folder was still others' to write, and a write through it would
/// land wherever it leads before the handoff's own check refuses the draft.
pub(super) fn new_draft(drafts: &Path) -> Result<std::path::PathBuf, String> {
    let path = drafts.join(format!("handoff-{}.md", crate::secret::random_hex(4)));
    crate::secret::create_private(&path).map_err(|e| format!("could not make {}: {e}", path.display()))?;
    Ok(path)
}

/// Why a chat run with `settings` can write no file, its handoff included:
/// Codex's read-only sandboxes write nowhere (an `--add-dir` is ignored under
/// them), and Claude Code's plan mode edits nothing.
pub(super) fn unwritable(settings: &Value) -> Option<&'static str> {
    match (settings["sandbox"].as_str(), settings["permissionMode"].as_str()) {
        (Some("read-only" | "read-only-net"), _) => Some(
            "This chat runs in Codex's read-only sandbox, which writes no file, this folder included: ask the person to \
             write it there, or to start the chat with a sandbox that writes (workspace-write).",
        ),
        (_, Some("plan")) => Some("In plan mode Claude Code writes no file: leave plan mode first, or ask the person to."),
        _ => None,
    }
}

/// Whether a handoff file is looked at for the sections of Luna's checklist
/// (facts.rs `missing_sections`): where the chat was given Luna's own
/// instructions or their checklist, or nothing and Luna's own are in effect.
/// An override's sections are its author's.
pub(super) fn checks_sections(served: Option<&Served>) -> bool {
    match served {
        Some(s) => s.source != Given::Library,
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
        assert_eq!((own.source, own.version.as_deref(), own.path.as_deref()), (Given::Luna, Some(env!("CARGO_PKG_VERSION")), None));
        let theirs = Served::of(&resolved(Source::Library, None));
        assert_eq!((theirs.source, theirs.version.as_deref()), (Given::Library, None));
        assert_eq!(theirs.path.as_deref(), Some("/lib/system/session-handoff/SKILL.md"));
        assert_eq!(theirs.sha256, "0123456789abcdef");
        // The firm notice's checklist alone: Luna's, named by its own hash.
        let list = Served::checklist();
        let sha: String = Sha256::digest(facts::checklist().as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            (list.source, list.sha256.as_str(), list.version.as_deref()),
            (Given::Checklist, sha.as_str(), Some(env!("CARGO_PKG_VERSION")))
        );
        // Kept on disk: it reads back as it was, and as 0.12.0's records name it.
        for s in [&own, &theirs, &list] {
            let back: Served = serde_json::from_value(serde_json::to_value(s).unwrap()).unwrap();
            assert_eq!(&back, s);
        }
        assert_eq!(serde_json::to_value(&list).unwrap()["source"], "checklist");
        // Where an override was there and would not do, the answer says so.
        assert!(source_words(&resolved(Source::Luna, Some("The front matter has no description")))
            .contains("override at /lib/system/session-handoff/SKILL.md is not used: The front matter has no description"));
        assert!(source_words(&resolved(Source::Library, None)).starts_with("the person's, from Luna's library at /lib/"));
        assert!(!checks_sections(Some(&theirs)) && checks_sections(Some(&own)) && checks_sections(Some(&list)));
    }

    #[test]
    fn a_chat_that_can_write_no_file_is_told_so() {
        let said = |s: Value| unwritable(&s);
        for sandbox in ["read-only", "read-only-net"] {
            assert!(said(serde_json::json!({ "sandbox": sandbox })).is_some_and(|w| w.contains("read-only sandbox")), "{sandbox}");
        }
        assert!(said(serde_json::json!({ "permissionMode": "plan" })).is_some_and(|w| w.contains("plan mode")));
        for writes in [serde_json::json!({ "sandbox": "workspace-write" }), serde_json::json!({ "permissionMode": "acceptEdits" })] {
            assert_eq!(said(writes), None);
        }
    }
}
