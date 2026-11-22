//! What a new chat opens on, as plain data and the rules that make it: each
//! CLI's models, levels and modes, the settings the CLI's own files resolve
//! for an account and folder (the core does that, `claude_defaults` and
//! `codex_defaults`), and the effort a model takes. Ported from
//! src/features/providers/ and new-chat/draft.ts, nothing of it drawn here.

use serde::Deserialize;
use std::collections::HashMap;

/// Claude Code's models the window knows by name, until the account's own
/// list has come.
pub const MODELS: [&str; 4] = ["Fable", "Opus", "Sonnet", "Haiku"];
/// Claude Code's standing orchestration, offered beside every model's levels.
pub const ULTRACODE: &str = "ultracode";
/// The levels of a model the list does not know.
pub const EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", ULTRACODE];
/// Claude Code's permission modes, by Luna's label, and what each means.
pub const PERMS: [(&str, &str); 5] = [
    ("Ask", "confirm every action"),
    ("Edits", "write files, ask for commands"),
    ("Plan only", "no disk writes"),
    ("Auto", "Claude Code reviews risky actions itself"),
    ("Bypass", "full access, no prompts"),
];
/// The levels shown for a Codex model the list does not know.
pub const CODEX_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];
/// The core's word for an approval or a sandbox left to Codex, "Codex's own,
/// from its config" (codex/defaults.rs `OWN`): the chat is started with no
/// flag for it.
pub const CODEX_OWN: &str = "codex";
pub const CLAUDE_OWN_LABEL: &str = "Claude Code's own";
pub const CODEX_APPROVALS: [&str; 4] = [CODEX_OWN, "on-request", "never", "auto-review"];
pub const CODEX_SANDBOXES: [&str; 5] = [CODEX_OWN, "read-only", "read-only-net", "workspace-write", "danger-full-access"];
/// The one sandbox `auto-review` runs in.
pub const AUTO_REVIEW_SANDBOX: &str = "workspace-write";

/// An approval or a sandbox as the dialog's radio names it.
pub fn codex_mode_label(v: &str) -> &str {
    if v == CODEX_OWN { "Codex's own" } else { v }
}

pub fn codex_approval_hint(a: &str) -> &'static str {
    match a {
        CODEX_OWN => "approvals as Codex's config.toml says",
        "on-request" => "Codex asks you before anything the sandbox would block",
        "never" => "never asks, blocked actions just fail",
        _ => "a reviewer model answers what Codex would ask you, and the turn goes on",
    }
}

pub fn codex_sandbox_hint(s: &str) -> &'static str {
    match s {
        CODEX_OWN => "the sandbox as Codex's config.toml says",
        "read-only" => "commands can only read the disk",
        "read-only-net" => "commands can only read the disk, with the network on",
        "workspace-write" => "writes inside the folder and temp, nowhere else",
        _ => "no sandbox at all",
    }
}

/// What an approval and a sandbox come to together, as one line.
pub fn codex_mode_hint(approval: &str, sandbox: &str) -> String {
    if approval == "auto-review" {
        return format!("Runs with --approve-for-me: {}; {}.", codex_approval_hint(approval), codex_sandbox_hint(AUTO_REVIEW_SANDBOX));
    }
    if approval == CODEX_OWN && sandbox == CODEX_OWN {
        return "Luna passes no flag: Codex runs as its own config.toml says, or on its own defaults.".into();
    }
    if sandbox == "danger-full-access" {
        return match approval {
            "never" => "Runs with --dangerously-bypass-approvals-and-sandbox.".into(),
            "on-request" => "No sandbox; Codex still asks you before commands it finds dangerous.".into(),
            _ => format!("No sandbox; {}.", codex_approval_hint(approval)),
        };
    }
    format!("{}; {}.", codex_approval_hint(approval), codex_sandbox_hint(sandbox))
}

/// One setting as the core resolved it, and where it came from.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Setting {
    pub value: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub key: Option<String>,
}

/// `claude_defaults`' answer.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeDefaults {
    pub model: Option<Setting>,
    pub effort: Option<Setting>,
    pub permission_mode: Option<Setting>,
    pub effort_by_model: HashMap<String, Setting>,
}

/// `codex_defaults`' answer.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexDefaults {
    pub model: Option<Setting>,
    pub effort: Option<Setting>,
    pub approval: Option<Setting>,
    pub sandbox: Option<Setting>,
}

/// One of the models Claude Code lists for an account.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ClaudeModel {
    pub slug: String,
    pub display_name: String,
    pub default_effort: Option<String>,
    /// Empty for a model that takes no effort.
    pub efforts: Vec<String>,
    pub effort_hints: HashMap<String, String>,
    /// Listed under "More…" (`MORE`): not a family's main model.
    pub more: bool,
}

/// One of the models Codex lists for an account.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct CodexModel {
    pub slug: String,
    pub display_name: String,
    pub default_effort: Option<String>,
    pub efforts: Vec<String>,
    pub effort_hints: HashMap<String, String>,
}

/// The heading the older Claude models are listed under, after the main ones.
pub const MORE: &str = "More…";

/// One line of a model drop-down: what picking it sets (a Claude model's
/// label, a Codex model's slug), what it reads, and the heading it is listed
/// under, if any.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelEntry {
    pub value: String,
    pub label: String,
    pub group: Option<&'static str>,
}

/// A model's line as it reads: the model a chat gets with nothing changed
/// carries "· default", so no "default" line hides which model that is.
fn marked(name: &str, default: bool) -> String {
    if default { format!("{name} · default") } else { name.to_owned() }
}

/// The models a list does not know that still get a line of their own,
/// after the listed ones: the one the settings files give, kept while
/// another is picked so it can be picked back, then the one chosen.
fn unlisted(known: impl Fn(&str) -> bool, current: Option<&str>, resolved: Option<&str>) -> Vec<ModelEntry> {
    let mut out: Vec<ModelEntry> = Vec::new();
    for m in [resolved, current].into_iter().flatten() {
        if !known(m) && !out.iter().any(|e| e.value == m) {
            out.push(ModelEntry { value: m.to_owned(), label: marked(m, Some(m) == resolved), group: None });
        }
    }
    out
}

/// The Claude models' drop-down: the main models first, the models the
/// settings name that the list does not know after them (`unlisted`), and
/// the rest under "More…", each in the list's order. `resolved` is the model
/// the files give, marked as the default; None while they have not answered
/// for this account and folder, when nothing is.
pub fn claude_model_entries(list: &[ClaudeModel], current: &str, resolved: Option<&str>) -> Vec<ModelEntry> {
    let entry = |m: &ClaudeModel| {
        let value = model_label_of(&m.slug);
        ModelEntry { label: marked(&m.display_name, Some(value.as_str()) == resolved), group: m.more.then_some(MORE), value }
    };
    let mut out: Vec<ModelEntry> = list.iter().filter(|m| !m.more).map(entry).collect();
    out.extend(unlisted(|v| list.iter().any(|m| model_label_of(&m.slug) == v), Some(current), resolved));
    out.extend(list.iter().filter(|m| m.more).map(entry));
    out
}

/// Codex's models in Codex's order, and those the settings name that the
/// list does not know after them (`unlisted`). `resolved` is the model a
/// chat gets with nothing changed: the account's config.toml model, or the
/// first Codex lists (codex/defaults.rs), marked as the default; None while
/// the files have not answered.
pub fn codex_model_entries(list: &[CodexModel], current: Option<&str>, resolved: Option<&str>) -> Vec<ModelEntry> {
    let mut out: Vec<ModelEntry> = list
        .iter()
        .map(|m| ModelEntry { value: m.slug.clone(), label: marked(&m.display_name, Some(m.slug.as_str()) == resolved), group: None })
        .collect();
    out.extend(unlisted(|v| list.iter().any(|m| m.slug == v), current, resolved));
    out
}

/// The four aliases at every level, before the account's list has come.
pub fn stock_models() -> Vec<ClaudeModel> {
    MODELS
        .iter()
        .map(|m| ClaudeModel {
            slug: m.to_lowercase(),
            display_name: (*m).to_owned(),
            efforts: EFFORTS.iter().filter(|e| **e != ULTRACODE).map(|e| (*e).to_owned()).collect(),
            ..ClaudeModel::default()
        })
        .collect()
}

/// The label a chat keeps for what `--model` is given: an alias as its
/// family's name, a full id as it is.
pub fn model_label_of(slug: &str) -> String {
    if !slug.is_empty() && slug.chars().all(|c| c.is_ascii_lowercase()) {
        let mut chars = slug.chars();
        chars.next().map(|f| f.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default()
    } else {
        slug.to_owned()
    }
}

fn is_word(v: &str) -> bool {
    !v.is_empty() && v.chars().all(|c| c.is_ascii_lowercase())
}

/// `model` as the core settled it: an alias (or a family Luna has not heard
/// of) is its family, a full id is that model, a compound alias names its
/// family plainly enough to match on, and anything else means Luna keeps
/// its own.
pub fn model_from_setting(raw: &str) -> Option<String> {
    let v = raw.trim().to_lowercase();
    let family = MODELS.iter().find(|m| v.contains(&m.to_lowercase())).copied();
    let full_id =
        v.strip_prefix("claude-").is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    if full_id {
        return Some(v);
    }
    if is_word(&v) {
        return Some(match family {
            Some(f) if v != f.to_lowercase() => f.to_owned(),
            _ => model_label_of(&v),
        });
    }
    family.map(str::to_owned)
}

/// A level as the CLI spells it: one lower-case word.
pub fn effort_from_setting(raw: &str) -> Option<String> {
    let v = raw.trim().to_lowercase();
    is_word(&v).then_some(v)
}

/// `permissions.defaultMode` as the CLI spells it, back onto Luna's labels.
pub fn perm_from_setting(raw: &str) -> Option<&'static str> {
    match raw {
        "default" => Some("Ask"),
        "acceptEdits" => Some("Edits"),
        "plan" => Some("Plan only"),
        "auto" => Some("Auto"),
        "bypassPermissions" => Some("Bypass"),
        _ => None,
    }
}

/// Which file a Claude Code setting came from, as a person reads it.
pub fn claude_source(source: &str) -> String {
    match source {
        "account" => "the account's settings.json",
        "project" => "the project's .claude/settings.json",
        "project-local" => "the project's .claude/settings.local.json",
        "managed" => "the machine-wide managed settings",
        "model" => "the model's own default in Claude Code's catalog",
        other => other,
    }
    .to_owned()
}

pub fn codex_source(source: &str) -> String {
    match source {
        "account" => "the account's config.toml",
        "project" => "the project's .codex/config.toml",
        "system" => "the machine-wide config.toml",
        "codex" => "Codex's own default",
        // What the core picks where it cannot read the files
        // (codex/defaults.rs `CLOSED_APPROVAL`, `CLOSED_SANDBOX`).
        "closed" => "a config.toml Luna cannot read: Luna's safe choice",
        other => other,
    }
    .to_owned()
}

/// Where an approval or a sandbox came from: Codex's own is not a value Luna
/// read anywhere but one it leaves to Codex.
fn codex_mode_source(source: &str) -> String {
    match source {
        "codex" => "Codex itself: Luna passes no flag".to_owned(),
        other => codex_source(other),
    }
}

/// A chat's settings, the Claude Code way.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeSettings {
    pub model: String,
    pub effort: String,
    pub perm: String,
}

/// Where a chat opens when no settings file says otherwise (the core's stock,
/// claude/defaults.rs), for the moment before it answers or if it cannot.
pub fn claude_stock() -> ClaudeSettings {
    ClaudeSettings { model: "Opus".into(), effort: "high".into(), perm: "Auto".into() }
}

/// And the Codex way: no model means Codex's own default.
#[derive(Clone, Debug, PartialEq)]
pub struct CodexSettings {
    pub model: Option<String>,
    pub effort: String,
    pub approval: String,
    pub sandbox: String,
}

/// Before the core answers or where it cannot: the approval and the sandbox
/// the core itself takes when it cannot read the files (codex/defaults.rs
/// `CLOSED_APPROVAL`, `CLOSED_SANDBOX`), since nothing heard is not nothing
/// said, and never a mode that goes further than the files might.
pub fn codex_stock() -> CodexSettings {
    CodexSettings { model: None, effort: "high".into(), approval: "on-request".into(), sandbox: "read-only".into() }
}

/// The file each value came from, by field; none for a stock value.
pub type Origins = HashMap<&'static str, String>;

/// The levels a Claude model takes: its own and ultracode, none for a model
/// that takes no effort, every level for one the list does not know.
pub fn efforts_for(listed: Option<&ClaudeModel>) -> Vec<String> {
    match listed {
        None => EFFORTS.iter().map(|e| (*e).to_owned()).collect(),
        Some(m) if m.efforts.is_empty() => Vec::new(),
        Some(m) => m.efforts.iter().cloned().chain(std::iter::once(ULTRACODE.to_owned())).collect(),
    }
}

/// The effort a chat on this model must move to from `current`, or None where
/// `current` will do: none (`""`, no `--effort`) for a model that takes none,
/// the model's own default (else `high`, else its first level) for one that
/// does not take `current`.
pub fn effort_moved_to(listed: Option<&ClaudeModel>, current: &str) -> Option<String> {
    let takes = efforts_for(listed);
    if takes.is_empty() {
        return (!current.is_empty()).then(String::new);
    }
    if takes.iter().any(|e| e == current) {
        return None;
    }
    let own: Vec<String> = listed.map(|m| m.efforts.clone()).unwrap_or_else(|| EFFORTS.iter().map(|e| (*e).to_owned()).collect());
    Some(listed.and_then(|m| m.default_effort.clone()).unwrap_or_else(|| {
        if own.iter().any(|e| e == "high") { "high".into() } else { own.first().cloned().unwrap_or_else(|| "high".into()) }
    }))
}

/// The effort a chat on `listed` is to run at, from `current`: the level the
/// settings files keep for the model (`own`) unless the effort was picked by
/// hand, then held to what the model takes.
pub fn next_effort(listed: Option<&ClaudeModel>, current: &str, own: Option<&str>, touched: bool) -> String {
    let start = match own {
        Some(o) if !touched => o,
        _ => current,
    };
    effort_moved_to(listed, start).unwrap_or_else(|| start.to_owned())
}

/// Model, effort and permission mode as Claude Code itself would resolve them
/// for the account in the folder. A value Luna has no equivalent for comes
/// back as the stock one; an unrecognized permission mode opens on Ask.
pub fn claude_settings(dto: Option<&ClaudeDefaults>, models: &[ClaudeModel]) -> (ClaudeSettings, Origins) {
    let mut values = claude_stock();
    let mut from = Origins::new();
    if let Some(d) = dto {
        if let (Some(s), Some(model)) = (&d.model, d.model.as_ref().and_then(|m| model_from_setting(&m.value))) {
            values.model = model;
            if s.source != "stock" {
                from.insert("model", claude_source(&s.source));
            }
        }
        if let (Some(s), Some(effort)) = (&d.effort, d.effort.as_ref().and_then(|e| effort_from_setting(&e.value))) {
            values.effort = effort;
            if s.source != "stock" {
                let said = claude_source(&s.source);
                from.insert(
                    "effort",
                    match &s.key {
                        Some(k) => format!("{said}, {k}"),
                        None => said,
                    },
                );
            }
        }
        if let Some(mode) = &d.permission_mode {
            if let Some(perm) = perm_from_setting(&mode.value) {
                values.perm = perm.to_owned();
                if !matches!(mode.source.as_str(), "stock" | "luna") {
                    from.insert("perm", claude_source(&mode.source));
                }
            } else {
                values.perm.clear();
                from.insert("perm", CLAUDE_OWN_LABEL.to_owned());
            }
        }
    }
    let listed = models.iter().find(|m| model_label_of(&m.slug) == values.model);
    if let Some(moved) = effort_moved_to(listed, &values.effort) {
        values.effort = moved;
        from.remove("effort");
    }
    (values, from)
}

/// The same out of Codex's `config.toml`.
pub fn codex_settings(dto: Option<&CodexDefaults>) -> (CodexSettings, Origins) {
    let mut values = codex_stock();
    let mut from = Origins::new();
    let Some(d) = dto else { return (values, from) };
    if let Some(s) = d.model.as_ref().filter(|s| !s.value.is_empty()) {
        values.model = Some(s.value.clone());
        if s.source != "stock" {
            from.insert("model", codex_source(&s.source));
        }
    }
    if let Some((s, e)) = d.effort.as_ref().and_then(|s| Some((s, effort_from_setting(&s.value)?))) {
        values.effort = e;
        if s.source != "stock" {
            from.insert("effort", codex_source(&s.source));
        }
    }
    if let Some((s, a)) = d.approval.as_ref().and_then(|s| Some((s, CODEX_APPROVALS.iter().find(|a| **a == s.value.to_lowercase())?))) {
        values.approval = (*a).to_owned();
        if !matches!(s.source.as_str(), "stock" | "luna") {
            from.insert("approval", codex_mode_source(&s.source));
        }
    }
    if let Some((s, a)) = d.sandbox.as_ref().and_then(|s| Some((s, CODEX_SANDBOXES.iter().find(|a| **a == s.value.to_lowercase())?))) {
        values.sandbox = (*a).to_owned();
        if !matches!(s.source.as_str(), "stock" | "luna") {
            from.insert("sandbox", codex_mode_source(&s.source));
        }
    }
    (values, from)
}

/// An account's settings of one CLI or the other.
#[derive(Clone, Debug, PartialEq)]
pub enum Settings {
    Claude(ClaudeSettings),
    Codex(CodexSettings),
}

pub use crate::state::same_path;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn model(slug: &str, efforts: &[&str], default: Option<&str>) -> ClaudeModel {
        ClaudeModel {
            slug: slug.into(),
            display_name: model_label_of(slug),
            default_effort: default.map(str::to_owned),
            efforts: efforts.iter().map(|e| (*e).to_owned()).collect(),
            ..ClaudeModel::default()
        }
    }

    #[test]
    fn a_model_setting_is_a_family_a_full_id_or_nothing() {
        assert_eq!(model_from_setting("opus").as_deref(), Some("Opus"));
        assert_eq!(model_from_setting("Sonnet ").as_deref(), Some("Sonnet"));
        // A compound alias names its family.
        assert_eq!(model_from_setting("opusplan").as_deref(), Some("Opus"));
        // A family Luna has not heard of is its own label.
        assert_eq!(model_from_setting("mythos").as_deref(), Some("Mythos"));
        assert_eq!(model_from_setting("claude-opus-4-7").as_deref(), Some("claude-opus-4-7"));
        // A variant names its family.
        assert_eq!(model_from_setting("claude-sonnet-5[1m]").as_deref(), Some("Sonnet"));
        assert_eq!(model_from_setting("???"), None);
        assert_eq!(model_label_of("opus"), "Opus");
        assert_eq!(model_label_of("claude-opus-4-7"), "claude-opus-4-7");
    }

    #[test]
    fn older_claude_models_are_listed_only_under_more_and_the_default_is_marked() {
        let older = |slug: &str, name: &str| ClaudeModel { display_name: name.into(), more: true, ..model(slug, &[], None) };
        let list = [
            model("opus", &["high"], None),
            older("claude-opus-4-7", "Opus 4.7"),
            model("sonnet", &[], None),
            older("claude-haiku-4-5", "Haiku 4.5"),
        ];
        let e = claude_model_entries(&list, "Opus", Some("Opus"));
        fn line(e: &ModelEntry) -> (&str, Option<&str>) {
            (e.label.as_str(), e.group)
        }
        assert_eq!(
            e.iter().map(line).collect::<Vec<_>>(),
            [("Opus · default", None), ("Sonnet", None), ("Opus 4.7", Some(MORE)), ("Haiku 4.5", Some(MORE))]
        );
        // An older one runs by its full id.
        assert_eq!(e[2].value, "claude-opus-4-7");
        // A model the settings name that the list does not know comes after
        // the main ones, outside "More…", marked when the files give it.
        let e = claude_model_entries(&list, "claude-opus-4-1", Some("claude-opus-4-1"));
        assert_eq!(line(&e[2]), ("claude-opus-4-1 · default", None));
        assert!(e[3..].iter().all(|e| e.group == Some(MORE)));
        // Another picked, it keeps its line, to be picked back.
        let e = claude_model_entries(&list, "Sonnet", Some("claude-opus-4-1"));
        assert_eq!(line(&e[2]), ("claude-opus-4-1 · default", None));
        assert_eq!(e.len(), 5);
        // Picked here, the default is still the files' model.
        let e = claude_model_entries(&list, "Sonnet", Some("claude-haiku-4-5"));
        assert_eq!(e.iter().filter(|e| e.label.ends_with("· default")).map(|e| e.value.as_str()).collect::<Vec<_>>(), ["claude-haiku-4-5"]);
        // The files not heard from: nothing is said to be the default.
        assert!(claude_model_entries(&list, "Opus", None).iter().all(|e| !e.label.contains("default")));
    }

    #[test]
    fn codexs_default_model_is_marked_in_codexs_order() {
        let m = |slug: &str| CodexModel { slug: slug.into(), display_name: slug.to_uppercase(), ..CodexModel::default() };
        let list = [m("gpt-6"), m("gpt-6-mini")];
        let labels = |e: Vec<ModelEntry>| e.into_iter().map(|e| e.label).collect::<Vec<_>>();
        assert_eq!(labels(codex_model_entries(&list, Some("gpt-6-mini"), Some("gpt-6"))), ["GPT-6 · default", "GPT-6-MINI"]);
        assert_eq!(labels(codex_model_entries(&list, Some("gpt-6"), Some("gpt-6-mini"))), ["GPT-6", "GPT-6-MINI · default"]);
        // One config.toml names that the list does not know comes last, and
        // stays while another is picked.
        assert_eq!(labels(codex_model_entries(&list, Some("gpt-x"), Some("gpt-x"))), ["GPT-6", "GPT-6-MINI", "gpt-x · default"]);
        assert_eq!(labels(codex_model_entries(&list, Some("gpt-6"), Some("gpt-x"))), ["GPT-6", "GPT-6-MINI", "gpt-x · default"]);
        // One picked that is neither listed nor the default: after it.
        assert_eq!(labels(codex_model_entries(&list, Some("gpt-y"), Some("gpt-x"))), ["GPT-6", "GPT-6-MINI", "gpt-x · default", "gpt-y"]);
        // Before the core answers, nothing is said to be the default.
        assert!(codex_model_entries(&list, None, None).iter().all(|e| !e.label.contains("default")));
    }

    #[test]
    fn a_model_that_does_not_take_the_effort_moves_it() {
        let opus = model("opus", &["low", "high"], Some("high"));
        assert_eq!(efforts_for(Some(&opus)), ["low", "high", "ultracode"]);
        assert_eq!(effort_moved_to(Some(&opus), "high"), None);
        assert_eq!(effort_moved_to(Some(&opus), "max").as_deref(), Some("high"));
        // No default of its own: high, else its first level.
        assert_eq!(effort_moved_to(Some(&model("m", &["low", "medium"], None)), "max").as_deref(), Some("low"));
        // One that takes none runs with none.
        let none = model("haiku", &[], None);
        assert!(efforts_for(Some(&none)).is_empty());
        assert_eq!(effort_moved_to(Some(&none), "high").as_deref(), Some(""));
        assert_eq!(effort_moved_to(Some(&none), ""), None);
        // One the list does not know takes every level.
        assert_eq!(effort_moved_to(None, "xhigh"), None);
    }

    #[test]
    fn the_models_own_level_leads_unless_the_effort_was_picked() {
        let opus = model("opus", &["low", "medium", "high"], Some("high"));
        assert_eq!(next_effort(Some(&opus), "high", Some("low"), false), "low");
        assert_eq!(next_effort(Some(&opus), "medium", Some("low"), true), "medium");
        assert_eq!(next_effort(Some(&opus), "max", None, true), "high");
    }

    #[test]
    fn claude_defaults_resolve_as_the_page_resolved_them() {
        let dto: ClaudeDefaults = serde_json::from_value(json!({
            "model": { "value": "sonnet", "source": "account" },
            "effort": { "value": "medium", "source": "account", "key": "effortLevel" },
            "permissionMode": { "value": "dontAsk", "source": "project" },
        }))
        .unwrap();
        let models = [model("sonnet", &["low", "medium", "high"], None)];
        let (s, from) = claude_settings(Some(&dto), &models);
        assert_eq!((s.model.as_str(), s.effort.as_str(), s.perm.as_str()), ("Sonnet", "medium", ""));
        assert_eq!(from["model"], "the account's settings.json");
        assert_eq!(from["effort"], "the account's settings.json, effortLevel");
        assert_eq!(from["perm"], CLAUDE_OWN_LABEL);
        let unreadable = serde_json::from_value(json!({ "permissionMode": { "value": "claude", "source": "cli" } })).unwrap();
        let (s, from) = claude_settings(Some(&unreadable), &[]);
        assert!(s.perm.is_empty());
        assert_eq!(from["perm"], CLAUDE_OWN_LABEL);
        // Nothing said: the stock values, naming no file.
        let (s, from) = claude_settings(None, &[]);
        assert_eq!(s, claude_stock());
        assert!(from.is_empty());
        // A stock mode that is a known one is taken, with no file.
        let dto: ClaudeDefaults = serde_json::from_value(json!({ "permissionMode": { "value": "plan", "source": "stock" } })).unwrap();
        assert_eq!(claude_settings(Some(&dto), &[]).0.perm, "Plan only");
        // An effort the settled model does not take is moved, its origin dropped.
        let dto: ClaudeDefaults = serde_json::from_value(json!({
            "model": { "value": "haiku", "source": "account" },
            "effort": { "value": "high", "source": "account" },
        }))
        .unwrap();
        let (s, from) = claude_settings(Some(&dto), &[model("haiku", &[], None)]);
        assert_eq!(s.effort, "");
        assert!(!from.contains_key("effort"));
    }

    #[test]
    fn codex_defaults_take_only_what_codex_knows() {
        let dto: CodexDefaults = serde_json::from_value(json!({
            "model": { "value": "gpt-6", "source": "codex" },
            "effort": { "value": "low", "source": "account" },
            "approval": { "value": "on-request", "source": "account" },
            "sandbox": { "value": "bogus", "source": "account" },
        }))
        .unwrap();
        let (s, from) = codex_settings(Some(&dto));
        assert_eq!(s.model.as_deref(), Some("gpt-6"));
        assert_eq!((s.effort.as_str(), s.approval.as_str()), ("low", "on-request"));
        // A sandbox Luna cannot put in a title bar reads as nothing said.
        assert_eq!(s.sandbox, "read-only");
        assert_eq!(from["model"], "Codex's own default");
        assert!(!from.contains_key("sandbox"));
        assert_eq!(codex_settings(None).0, codex_stock());
    }

    /// What the core answers for files it can read is Codex's own, kept as
    /// the core's word so no flag goes for it; files it cannot read are its
    /// safe choice, said as such. Neither, nor nothing heard at all, is ever
    /// the full bypass.
    #[test]
    fn codexs_own_and_the_closed_choice_are_taken_as_the_core_says() {
        let own: CodexDefaults = serde_json::from_value(json!({
            "approval": { "value": "codex", "source": "codex" },
            "sandbox": { "value": "codex", "source": "codex" },
        }))
        .unwrap();
        let (s, from) = codex_settings(Some(&own));
        assert_eq!((s.approval.as_str(), s.sandbox.as_str()), (CODEX_OWN, CODEX_OWN));
        assert_eq!(from["approval"], "Codex itself: Luna passes no flag");
        assert!(codex_mode_hint(&s.approval, &s.sandbox).contains("no flag"));
        let closed: CodexDefaults = serde_json::from_value(json!({
            "approval": { "value": "on-request", "source": "closed" },
            "sandbox": { "value": "read-only", "source": "closed" },
        }))
        .unwrap();
        let (s, from) = codex_settings(Some(&closed));
        assert_eq!((s.approval.as_str(), s.sandbox.as_str()), ("on-request", "read-only"));
        assert_eq!(from["approval"], "a config.toml Luna cannot read: Luna's safe choice");
        for (s, _) in [codex_settings(Some(&own)), codex_settings(Some(&closed)), codex_settings(None)] {
            assert!(!(s.approval == "never" && s.sandbox == "danger-full-access"), "{s:?}");
        }
    }

    #[test]
    fn modes_read_as_one_line() {
        assert!(codex_mode_hint("never", "danger-full-access").contains("--dangerously-bypass"));
        assert!(codex_mode_hint("auto-review", "read-only").contains("--approve-for-me"));
        assert!(codex_mode_hint("on-request", "read-only").contains("asks you"));
        assert!(codex_mode_hint(CODEX_OWN, "danger-full-access").contains("as Codex's config.toml says"));
        assert!(codex_mode_hint("never", CODEX_OWN).contains("the sandbox as Codex's config.toml says"));
        assert_eq!(codex_mode_label(CODEX_OWN), "Codex's own");
    }

    #[test]
    fn paths_compare_by_either_slash_and_case() {
        assert!(!same_path("C:\\repo", "C:\\repo2"));
        if cfg!(windows) {
            assert!(same_path("C:\\Repo\\", "c:/repo"));
        } else {
            assert!(same_path("/home/a/Work/", "/home/a/Work"));
            assert!(!same_path("/home/a/Work", "/home/a/work"), "two folders off Windows");
        }
    }
}
