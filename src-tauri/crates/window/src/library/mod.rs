//! The library window: the instructions and skills Luna lays into the
//! accounts, whose sessions each is for, the folders that give their
//! sessions other ones, and Luna's own system texts, with an editor for any
//! of their files. What the page's library dialog does, less what ADR-0030
//! cut: no "Needs a look", no diffs, nothing kept or left out by hand. The
//! library is the source of truth; the core lays it out after every change
//! and on Sync, which writes the library's over Luna's own copies, puts back
//! those deleted and takes out those no longer wanted, leaves a file not
//! Luna's alone, and says so.
//!
//! `Library` holds the core's overview (library/overview.rs `Overview`), as
//! every command answers it; `LibraryPanel` (view.rs) shows it, and
//! `EditorDoc` (editor.rs) is one file open in the editor.

mod editor;
mod view;

pub use editor::EditorDoc;
#[cfg(test)]
pub use editor::StatusKind;
pub use view::{LibraryPanel, Next, Sel};

use crate::host::Host;
use crate::panels::{Provider, ask, cmd};
use gpui::{Context, Task};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Every account, in a reach.
pub const ALL: &str = "all";

/// Where everything in the library stands (`LibraryOverviewDto`), what the
/// window shows of it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Overview {
    pub root: String,
    pub is_default: bool,
    /// False while the library's folder is not there.
    pub found: bool,
    pub instructions: Vec<Instructions>,
    /// Accounts that get no instructions however others reach them.
    pub no_instructions: Vec<String>,
    /// The instructions each account gets, by `provider/name`.
    pub instructions_for: BTreeMap<String, String>,
    /// The instructions.md of before there could be several.
    pub old_instructions: Option<String>,
    pub skills: Vec<Skill>,
    /// Every account, to pick a reach from.
    pub accounts: Vec<AccountRef>,
    pub folders: Vec<Folder>,
    /// Codex accounts whose AGENTS.override.md hides the AGENTS.md Luna writes.
    pub overridden: Vec<String>,
    pub system: Vec<SystemText>,
    /// Folders set aside in `system/`, read by nothing.
    pub system_aside: Vec<String>,
    /// Every copy the library wants, and where it stands.
    pub places: Vec<Place>,
    /// What the pass behind this answer did (library/pass.rs `Pass`):
    /// the copies it wrote the library's into, those it took out, and those
    /// it could not take out (`dest: why`). Empty on a look.
    pub written: Vec<String>,
    pub removed: Vec<String>,
    pub not_removed: Vec<String>,
}

/// One copy the library wants (`LibraryPlaceDto`): where, and how it stands
/// (`current`, `behind`, `missing`, `taken`: a file Luna did not write,
/// `skipped`, `failed`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Place {
    pub dest: String,
    pub status: String,
    pub problem: Option<String>,
}

impl Overview {
    fn places(&self, status: &str) -> impl Iterator<Item = &Place> {
        self.places.iter().filter(move |p| p.status == status)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Instructions {
    pub name: String,
    /// Its first heading.
    pub description: String,
    pub problem: Option<String>,
    pub accounts: Vec<String>,
    pub audience: Audience,
    /// For some roles only: given to each of their sessions as Luna starts
    /// it, never put into an account.
    pub per_session: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Why it is laid out nowhere.
    pub problem: Option<String>,
    pub accounts: Vec<String>,
    pub audience: Audience,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AccountRef {
    pub provider: Provider,
    pub name: String,
}

impl AccountRef {
    /// `provider/name`, as a reach names it.
    pub fn key(&self) -> String {
        format!("{}/{}", self.provider.key(), self.name)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    pub path: String,
    /// The accounts whose sessions here get a skill, by the skills it says
    /// anything of.
    pub skills: BTreeMap<String, Vec<String>>,
    /// The instructions its sessions get in place of their account's, by the
    /// reach entry they are for; None in a value: none at all.
    pub instructions: BTreeMap<String, Option<String>>,
    pub found: bool,
    /// The accounts whose sessions here get each skill were the folder to
    /// say nothing of it.
    pub inherited: BTreeMap<String, Vec<String>>,
}

impl Folder {
    /// What the folder says for `a`'s sessions here, the way the core picks
    /// it (folders.rs `folder_instructions`): the account's own entry, its
    /// CLI's, every account's; None where none does and it goes by what
    /// holds outside it. Some(None): none at all.
    pub fn instructions_for(&self, a: &AccountRef) -> Option<(String, Option<String>)> {
        [a.key(), a.provider.key().to_owned(), ALL.to_owned()]
            .into_iter()
            .find_map(|k| self.instructions.get(&k).map(|choice| (k, choice.clone())))
    }
}

/// One of Luna's system texts as it is served now (`SystemSkillState`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemText {
    pub name: String,
    /// `skill`, `role` or `note`.
    pub kind: String,
    /// `luna`, `library` (yours) or `broken` (yours, not used).
    pub state: String,
    pub version: String,
    pub sha256: String,
    pub path: String,
    pub problem: Option<String>,
}

impl SystemText {
    /// An override of yours is there, used or not.
    pub fn yours(&self) -> bool {
        self.state != "luna"
    }

    /// Whose text is served, in words.
    pub fn state_words(&self) -> String {
        let sha = &self.sha256[..self.sha256.len().min(12)];
        match self.state.as_str() {
            "library" => format!("Yours: {}, sha {sha}", self.path),
            "broken" => format!(
                "Yours is broken: {}. Luna's own is used ({}, sha {sha})",
                self.problem.as_deref().unwrap_or("it will not do"),
                self.version
            ),
            _ => format!("Luna's own: {}, sha {sha}", self.version),
        }
    }

    /// What it is, as the page says it.
    pub fn what(&self) -> &'static str {
        match self.name.as_str() {
            "role-chat" => {
                "What every chat of yours Luna starts is told of its role, read as an instructions file of its own. Empty: Luna tells a chat nothing."
            }
            "role-lead" => {
                "What a lead (a helper started with Luna's tools, able to start helpers of its own) is told of its role, read as an instructions file of its own at every turn."
            }
            "role-helper" => {
                "What a helper started without Luna's tools is told of its role, read as an instructions file of its own at every turn."
            }
            "note-lead" | "note-helper" => {
                "The note after a task another agent gives this role: one line of plain text, no brackets. Every note Luna has served stays known, so it never shows in what a session was asked."
            }
            _ => "Served by Luna at its own trigger; never laid out into an account or a folder.",
        }
    }
}

/// The system texts ADR-0030 took the feature of away (the handoff's skill),
/// which the window does not show even where an older core lists them.
pub fn shown_system(o: &Overview) -> impl Iterator<Item = &SystemText> {
    o.system.iter().filter(|s| s.name != "session-handoff")
}

/// The kinds of session: the person's own chat, a lead, a helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Chat,
    Lead,
    Helper,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Chat, Role::Lead, Role::Helper];

    pub fn key(self) -> &'static str {
        match self {
            Role::Chat => "chat",
            Role::Lead => "lead",
            Role::Helper => "helper",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Chat => "Chats",
            Role::Lead => "Leads",
            Role::Helper => "Helpers",
        }
    }
}

/// Whose sessions an entry is for: `"every"` or the roles listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Audience {
    pub chat: bool,
    pub lead: bool,
    pub helper: bool,
}

impl Default for Audience {
    fn default() -> Self {
        Audience { chat: true, lead: true, helper: true }
    }
}

impl Audience {
    pub fn has(self, role: Role) -> bool {
        match role {
            Role::Chat => self.chat,
            Role::Lead => self.lead,
            Role::Helper => self.helper,
        }
    }

    /// With `role` flipped; None where that leaves nobody's, an entry no
    /// session would get.
    pub fn flipped(self, role: Role) -> Option<Audience> {
        let mut a = self;
        match role {
            Role::Chat => a.chat = !a.chat,
            Role::Lead => a.lead = !a.lead,
            Role::Helper => a.helper = !a.helper,
        }
        (a.chat || a.lead || a.helper).then_some(a)
    }

    pub fn every(self) -> bool {
        self.chat && self.lead && self.helper
    }
}

impl<'de> Deserialize<'de> for Audience {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Said {
            /// `"every"`.
            Word(#[allow(dead_code)] String),
            Roles(Vec<String>),
        }
        Ok(match Said::deserialize(d)? {
            Said::Word(_) => Audience::default(),
            Said::Roles(r) => {
                let has = |k: &str| r.iter().any(|x| x == k);
                Audience { chat: has("chat"), lead: has("lead"), helper: has("helper") }
            }
        })
    }
}

impl Serialize for Audience {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.every() {
            return s.serialize_str("every");
        }
        Role::ALL.iter().filter(|r| self.has(**r)).map(|r| r.key()).collect::<Vec<_>>().serialize(s)
    }
}

/// A file of the library's the editor opens: a skill's SKILL.md, an
/// instructions file, or a system text's override.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Entry {
    Skill(String),
    Instructions(String),
    System(String),
}

impl Entry {
    pub fn name(&self) -> &str {
        match self {
            Entry::Skill(n) | Entry::Instructions(n) | Entry::System(n) => n,
        }
    }
}

/// A file as the library has it (`LibraryFileDto`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LibraryFile {
    pub path: String,
    pub text: String,
    /// False: not there yet, and `text` what to start from.
    pub exists: bool,
    /// The library's folder it was read from, by its path and its identity
    /// on its drive (empty: no folder there yet), and the sha256 of what was
    /// read (None: no file there): a save is bound to all three.
    pub root: String,
    pub root_id: String,
    pub revision: Option<String>,
}

/// What a save bound to its source came to (`library_write_checked`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Written {
    /// Why nothing was saved: the file changed on disk, or the library moved.
    pub conflict: Option<String>,
    /// The file's revision on disk now.
    pub revision: Option<String>,
    pub overview: Option<Overview>,
    /// Saved, but laying the library out after failed: why.
    pub layout_error: Option<String>,
}

/// The editor's text not saved when the window last went
/// (`library_draft`), with what it was read from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    /// The editor's that keeps it: a forget names it, so one editor's never
    /// takes another's (`set_library_draft`).
    #[serde(default)]
    pub id: String,
    pub entry: Entry,
    pub root: String,
    #[serde(default)]
    pub root_id: String,
    pub revision: Option<String>,
    pub text: String,
}

/// How a skill's rename went (`library_rename_skill`'s `Renamed`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    #[default]
    Committed,
    RolledBack,
    RollbackFailed,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Renamed {
    pub outcome: Outcome,
    pub why: Option<String>,
    pub overview: Option<Overview>,
}

/// Luna's own text of a system text beside your override of it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SystemCompare {
    pub luna: String,
    pub yours: Option<String>,
}

/// How a reach takes in an account, the closest first (library/reach.rs
/// `closeness`): the closest reach wins it.
pub fn how(reach: &[String], a: &AccountRef) -> Option<String> {
    if reach.contains(&a.key()) {
        Some("named for it".into())
    } else if reach.iter().any(|r| r == a.provider.key()) {
        Some(format!("as every {} account", a.provider.label()))
    } else if reach.iter().any(|r| r == ALL) {
        Some("as every account".into())
    } else {
        None
    }
}

/// A reach as the core keeps it (reach.rs `tidy_reach`): `all` alone, a CLI
/// without its own accounts, nothing twice.
pub fn tidy(reach: Vec<String>) -> Vec<String> {
    if reach.iter().any(|r| r == ALL) {
        return vec![ALL.into()];
    }
    let mut out: Vec<String> = Vec::new();
    for r in &reach {
        let under_cli = r.split_once('/').is_some_and(|(p, _)| reach.iter().any(|x| x == p));
        if !under_cli && !out.contains(r) {
            out.push(r.clone());
        }
    }
    out
}

/// What a reach picker's face says.
pub fn describe(reach: &[String]) -> String {
    if reach.is_empty() {
        return "Nowhere".into();
    }
    if reach.iter().any(|r| r == ALL) {
        return "All accounts".into();
    }
    reach
        .iter()
        .map(|r| match r.split_once('/') {
            Some((_, name)) => name.to_owned(),
            None => match r.as_str() {
                "claude" => "All Claude Code".to_owned(),
                "codex" => "All Codex".to_owned(),
                other => other.to_owned(),
            },
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The accounts by CLI, Claude Code's first, each CLI's in the order listed.
pub fn by_cli(accounts: &[AccountRef]) -> Vec<AccountRef> {
    Provider::ALL.iter().flat_map(|p| accounts.iter().filter(move |a| a.provider == *p).cloned()).collect()
}

/// Where a sync of the library stands: shown in words beside its button.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SyncState {
    #[default]
    Idle,
    Syncing,
    Synced(SyncReport),
    Failed(String),
}

/// What a Sync's pass did and left, from the overview it answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncReport {
    /// Copies the library's was written into: put there, or over Luna's
    /// own copy that was behind or changed there (backed up first).
    pub written: usize,
    /// Luna's copies no longer wanted, taken out.
    pub removed: usize,
    /// Places holding a file Luna did not write, left alone.
    pub foreign: usize,
    /// What it could not write or take out, a line each with why.
    pub failed: Vec<String>,
}

impl SyncReport {
    pub fn of(o: &Overview) -> SyncReport {
        let mut failed: Vec<String> =
            o.places("failed").map(|p| format!("{}: {}", p.dest, p.problem.as_deref().unwrap_or("no reason given"))).collect();
        // Not written: the copy changed there while the pass was at it.
        for p in o.places("behind").chain(o.places("missing")) {
            failed.push(format!("{}: changed there while Sync was at it; left for the next", p.dest));
        }
        failed.extend(o.not_removed.iter().cloned());
        SyncReport { written: o.written.len(), removed: o.removed.len(), foreign: o.places("taken").count(), failed }
    }

    /// Everything Luna manages holds what the library says.
    pub fn clean(&self) -> bool {
        self.failed.is_empty()
    }

    /// In words.
    pub fn said(&self) -> String {
        let count = |n: usize, one: &str, many: &str| if n == 1 { one.to_owned() } else { format!("{n} {many}") };
        let mut out = if self.clean() {
            "Synced: every copy Luna manages holds what the library says.".to_owned()
        } else {
            "Synced, but not everything Luna manages holds what the library says.".to_owned()
        };
        let mut done = Vec::new();
        if self.written > 0 {
            done.push(format!("{} written", count(self.written, "a copy", "copies")));
        }
        if self.removed > 0 {
            done.push(format!("{} no longer wanted taken out", count(self.removed, "a copy", "copies")));
        }
        if !done.is_empty() {
            let line = done.join(", ");
            out.push_str(&format!(" {}{} (a changed one backed up first).", line[..1].to_uppercase(), &line[1..]));
        }
        if self.foreign > 0 {
            out.push_str(&format!(" {} left alone.", count(self.foreign, "A file Luna did not write is", "files Luna did not write are")));
        }
        for line in &self.failed {
            out.push_str(&format!(" Not done: {line}."));
        }
        out
    }
}

/// The library as the core last answered it, and the commands that change
/// it: each answers the overview after the change, which is kept unless an
/// answer to a later command came first.
pub struct Library {
    host: Arc<dyn Host>,
    pub overview: Option<Overview>,
    /// Why the last command failed.
    pub error: Option<String>,
    /// Commands still out.
    pub pending: usize,
    pub sync: SyncState,
    asked: u64,
    shown: u64,
}

impl Library {
    /// Asks for the overview at once.
    pub fn new(host: Arc<dyn Host>, cx: &mut Context<Self>) -> Library {
        let mut library = Library { host, overview: None, error: None, pending: 0, sync: SyncState::Idle, asked: 0, shown: 0 };
        library.run(cmd::LIBRARY_OVERVIEW, json!({}), cx).detach();
        library
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    /// Runs a command that answers with the overview and keeps it; the task
    /// says whether it went through.
    pub fn run(&mut self, command: &str, args: Value, cx: &mut Context<Self>) -> Task<bool> {
        let task = self.run_answer::<Option<Overview>>(command, args, Option::clone, cx);
        cx.spawn(async move |_, _| task.await.is_ok())
    }

    /// Runs a command whose answer `overview_of` takes the overview from
    /// (where it has one), keeps that, and hands the whole answer back.
    pub fn run_answer<T: DeserializeOwned + 'static>(
        &mut self,
        command: &str,
        args: Value,
        overview_of: fn(&T) -> Option<Overview>,
        cx: &mut Context<Self>,
    ) -> Task<Result<T, String>> {
        self.asked += 1;
        let mine = self.asked;
        self.pending += 1;
        cx.notify();
        let answer = ask::<T>(&self.host, command, args);
        cx.spawn(async move |this, cx| {
            let got = answer.await;
            this.update(cx, |l, cx| {
                l.pending -= 1;
                match &got {
                    // An answer older than the one shown is dropped: a look
                    // asked before a click must not undo what it changed.
                    Ok(t) if mine > l.shown => {
                        l.shown = mine;
                        if let Some(o) = overview_of(t) {
                            l.overview = Some(o);
                        }
                        l.error = None;
                    }
                    Ok(_) => {}
                    Err(e) => l.error = Some(e.clone()),
                }
                cx.notify();
                got
            })
            .unwrap_or_else(|_| Err("the library window went".into()))
        })
    }

    /// Lays the library out now (ADR-0030): Luna's own copies written over
    /// with the library's or put back, those no longer wanted taken out, a
    /// changed one backed up first; a file Luna did not write left alone.
    /// Says what it wrote, took out and left alone, and what it could not do.
    pub fn sync_now(&mut self, cx: &mut Context<Self>) {
        self.sync = SyncState::Syncing;
        let pass = self.run_answer::<Option<Overview>>(cmd::LIBRARY_SYNC, json!({}), Option::clone, cx);
        cx.spawn(async move |this, cx| {
            let state = match pass.await {
                Ok(Some(o)) => SyncState::Synced(SyncReport::of(&o)),
                Ok(None) => SyncState::Failed("the core answered no overview".into()),
                Err(e) => SyncState::Failed(e),
            };
            let _ = this.update(cx, |l, cx| {
                l.sync = state;
                cx.notify();
            });
        })
        .detach();
    }

    /// A skill renamed; the task has how it went, or why it was refused with
    /// nothing changed (also kept as `error`). Rolled back or not, a rename
    /// that failed partway is said in `error` too.
    pub fn rename_skill(&mut self, from: &str, to: &str, cx: &mut Context<Self>) -> Task<Option<Outcome>> {
        let task = self.run_answer::<Renamed>(cmd::LIBRARY_RENAME_SKILL, json!({ "from": from, "to": to }), |r| r.overview.clone(), cx);
        let (from, to) = (from.to_owned(), to.to_owned());
        cx.spawn(async move |this, cx| {
            let r = task.await.ok()?;
            let why = r.why.clone().unwrap_or_default();
            let said = match r.outcome {
                Outcome::Committed => None,
                Outcome::RolledBack => Some(format!("{from} was not renamed: {why}. Everything is as it was.")),
                Outcome::RollbackFailed => Some(format!("{from} was renamed {to} only partly, and could not be put back: {why}")),
            };
            if said.is_some() {
                let _ = this.update(cx, |l, cx| {
                    l.error = said;
                    cx.notify();
                });
            }
            Some(r.outcome)
        })
    }

    /// Another folder for the library; empty, back to Luna's own.
    pub fn set_source(&mut self, path: &str, cx: &mut Context<Self>) -> Task<bool> {
        self.run(cmd::LIBRARY_SET_SOURCE, json!({ "path": path }), cx)
    }

    pub fn create(&mut self, entry: &Entry, cx: &mut Context<Self>) -> Task<bool> {
        match entry {
            Entry::Skill(n) => self.run(cmd::LIBRARY_NEW_SKILL, json!({ "name": n }), cx),
            Entry::Instructions(n) => self.run(cmd::LIBRARY_NEW_INSTRUCTIONS, json!({ "name": n }), cx),
            Entry::System(_) => Task::ready(false),
        }
    }

    pub fn rename(&mut self, entry: &Entry, to: &str, cx: &mut Context<Self>) -> Task<bool> {
        let to = to.trim();
        match entry {
            _ if to.is_empty() || to == entry.name() => Task::ready(false),
            Entry::Skill(n) => {
                let task = self.rename_skill(&n.clone(), to, cx);
                cx.spawn(async move |_, _| task.await == Some(Outcome::Committed))
            }
            Entry::Instructions(n) => self.run(cmd::LIBRARY_RENAME_INSTRUCTIONS, json!({ "from": n, "to": to }), cx),
            Entry::System(_) => Task::ready(false),
        }
    }

    /// Takes it out of the library, into Luna's backups.
    pub fn delete(&mut self, entry: &Entry, cx: &mut Context<Self>) -> Task<bool> {
        match entry {
            Entry::Skill(n) => self.run(cmd::LIBRARY_DELETE_SKILL, json!({ "name": n }), cx),
            Entry::Instructions(n) => self.run(cmd::LIBRARY_DELETE_INSTRUCTIONS, json!({ "name": n }), cx),
            Entry::System(_) => Task::ready(false),
        }
    }

    /// The accounts an entry is meant for.
    pub fn set_reach(&mut self, entry: &Entry, reach: Vec<String>, cx: &mut Context<Self>) -> Task<bool> {
        let reach = tidy(reach);
        match entry {
            Entry::Skill(n) => self.run(cmd::LIBRARY_SET_ACCOUNTS, json!({ "skill": n, "accounts": reach }), cx),
            Entry::Instructions(n) => self.run(cmd::LIBRARY_SET_INSTRUCTIONS, json!({ "name": n, "accounts": reach }), cx),
            Entry::System(_) => Task::ready(false),
        }
    }

    /// The radio in an account's column: it gets these instructions (None:
    /// none at all), taken from whichever named it before.
    pub fn give(&mut self, instructions: Option<&str>, account: &AccountRef, cx: &mut Context<Self>) -> Task<bool> {
        let Some(o) = &self.overview else { return Task::ready(false) };
        let mut reach = match instructions {
            Some(n) => o.instructions.iter().find(|i| i.name == n).map(|i| i.accounts.clone()).unwrap_or_default(),
            None => o.no_instructions.clone(),
        };
        // Not tidied: the account named is what the core takes from the
        // instructions that named it before, even where `all` covers it.
        reach.push(account.key());
        self.run(cmd::LIBRARY_SET_INSTRUCTIONS, json!({ "name": instructions, "accounts": reach }), cx)
    }

    pub fn set_audience(&mut self, entry: &Entry, audience: Audience, cx: &mut Context<Self>) -> Task<bool> {
        self.run(cmd::LIBRARY_SET_AUDIENCE, json!({ "entry": entry, "audience": audience }), cx)
    }

    /// A folder listed, saying nothing yet; or taken off with all it says.
    pub fn set_folder(&mut self, path: &str, listed: bool, cx: &mut Context<Self>) -> Task<bool> {
        self.run(cmd::LIBRARY_SET_FOLDER, json!({ "path": path, "listed": listed }), cx)
    }

    /// The accounts whose sessions in a folder get a skill; None: the folder
    /// says nothing of it.
    pub fn set_folder_skill(&mut self, path: &str, skill: &str, reach: Option<Vec<String>>, cx: &mut Context<Self>) -> Task<bool> {
        let reach = reach.map(tidy);
        self.run(cmd::LIBRARY_SET_FOLDER_SKILL, json!({ "path": path, "skill": skill, "accounts": reach }), cx)
    }

    /// What sessions on `account` in a folder get in place of its
    /// instructions: as outside it (`None`), none (`Some(None)`) or others.
    pub fn set_folder_instructions(
        &mut self,
        path: &str,
        account: &AccountRef,
        choice: Option<Option<String>>,
        cx: &mut Context<Self>,
    ) -> Task<bool> {
        let (inherit, name) = match choice {
            None => (true, None),
            Some(name) => (false, name),
        };
        self.run(
            cmd::LIBRARY_SET_FOLDER_INSTRUCTIONS,
            json!({ "path": path, "account": account.key(), "inherit": inherit, "name": name }),
            cx,
        )
    }

    /// Back to Luna's own text: the override goes to the backups.
    pub fn system_reset(&mut self, name: &str, cx: &mut Context<Self>) -> Task<bool> {
        self.run(cmd::LIBRARY_SYSTEM_RESET, json!({ "name": name }), cx)
    }

    /// The library's folder, or a skill's, in the file manager.
    pub fn open_folder(&self, skill: Option<&str>, cx: &mut Context<Self>) {
        let answer = self.host.call(cmd::LIBRARY_OPEN, json!({ "skill": skill }));
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = answer.await {
                    log::warn!("library_open: {e}");
                }
            })
            .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(p: Provider, n: &str) -> AccountRef {
        AccountRef { provider: p, name: n.into() }
    }

    #[test]
    fn a_reach_takes_an_account_in_by_its_closest_entry_and_is_kept_tidy() {
        let work = acc(Provider::Claude, "work");
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(how(&r(&["claude/work", "all"]), &work).as_deref(), Some("named for it"));
        assert_eq!(how(&r(&["claude"]), &work).as_deref(), Some("as every Claude Code account"));
        assert_eq!(how(&r(&["all"]), &work).as_deref(), Some("as every account"));
        assert_eq!(how(&r(&["codex"]), &work), None);
        assert_eq!(tidy(r(&["claude/work", "all", "codex"])), r(&["all"]));
        assert_eq!(tidy(r(&["claude/work", "claude", "codex/x", "codex/x"])), r(&["claude", "codex/x"]));
        assert_eq!(describe(&[]), "Nowhere");
        assert_eq!(describe(&r(&["claude", "codex/home"])), "All Claude Code, home");
    }

    #[test]
    fn an_audience_reads_and_writes_as_the_page_says_it() {
        let every: Audience = serde_json::from_value(json!("every")).unwrap();
        assert!(every.every());
        let some: Audience = serde_json::from_value(json!(["chat", "helper"])).unwrap();
        assert_eq!(some, Audience { chat: true, lead: false, helper: true });
        assert_eq!(serde_json::to_value(some).unwrap(), json!(["chat", "helper"]));
        assert_eq!(serde_json::to_value(every).unwrap(), json!("every"));
        // Nobody's is no audience at all.
        let chats = Audience { chat: true, lead: false, helper: false };
        assert_eq!(chats.flipped(Role::Chat), None);
        assert_eq!(chats.flipped(Role::Lead), Some(Audience { chat: true, lead: true, helper: false }));
        assert_eq!(serde_json::to_value(Entry::Instructions("main".into())).unwrap(), json!({ "instructions": "main" }));
    }

    #[test]
    fn a_sync_says_what_it_wrote_took_out_left_and_could_not_do() {
        let clean = SyncReport::default();
        assert_eq!(clean.said(), "Synced: every copy Luna manages holds what the library says.");
        let r = SyncReport { written: 2, removed: 1, foreign: 1, ..SyncReport::default() };
        assert_eq!(
            r.said(),
            "Synced: every copy Luna manages holds what the library says. 2 copies written, a copy no longer wanted taken out (a changed one backed up first). A file Luna did not write is left alone."
        );
        let r = SyncReport { removed: 3, ..SyncReport::default() };
        assert!(r.said().contains(" 3 copies no longer wanted taken out (a changed"), "{}", r.said());
        let r = SyncReport { failed: vec!["/a/skills/x: its backup would not write".into()], ..SyncReport::default() };
        assert!(!r.clean());
        assert!(r.said().starts_with("Synced, but not everything"), "{}", r.said());
        assert!(r.said().contains("Not done: /a/skills/x: its backup would not write."));
    }

    #[test]
    fn a_folder_decides_for_an_account_by_its_closest_entry() {
        let mut f = Folder::default();
        let work = acc(Provider::Claude, "work");
        assert_eq!(f.instructions_for(&work), None);
        f.instructions.insert("all".into(), Some("main".into()));
        assert_eq!(f.instructions_for(&work), Some(("all".into(), Some("main".into()))));
        f.instructions.insert("claude/work".into(), None);
        assert_eq!(f.instructions_for(&work), Some(("claude/work".into(), None)));
    }
}
