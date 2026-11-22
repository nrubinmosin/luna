//! The one import of 0.19's chat list, on the first start of the native
//! window: the WebView window kept its chats, boards and choices in its Local
//! Storage, which the host reads from a copy (luna_lib's native/import.rs)
//! and hands over as each key's text. Here they become the window's own
//! state, by the shapes the page's stores wrote (`luna.chats` and
//! `luna.panes` in zustand's `{ state, version }`, `luna.sidebar` as `on` or
//! `off`).
//!
//! What 0.20 cut is let go of as it is read: chat colors, handoff marks,
//! account moves, Codex's hooks without asking, the splits. Groups past the
//! fifth become board V's, whose sidebar then lists their chats; their panes
//! are not kept. A chat that cannot say which CLI and account it runs on,
//! or a folder with no path, is left out and counted (`Found::lost`): the
//! host keeps a copy of the list as it was and the window says so, as for
//! its own file (state.rs `Lost`). A list of a version before the one this
//! reads (`OLDEST`) is not imported, and the window says that.
//!
//! The choices 0.19 kept beside the list that the host keeps for Settings,
//! and its theme (`OldPrefs`), are not the window's state: they go to the host, which
//! takes each only where no choice was made here yet.

use crate::dto::ThemePref;
use crate::state::{AccountRef, BOARDS, Board, Chat, Folder, FolderStart, Lost, NameSource, Slots, Status, WindowState};
use crate::term::paste::Provider;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// The oldest version of `luna.chats` this reads: the one chats gained a
/// provider in (0.3.0), which every 0.19 wrote. The page let go of an older
/// list (zustand's `migrate`), but one a Luna before 0.3.0 left and no later
/// page read is still the person's list, not an empty one.
pub const OLDEST: u64 = 2;

/// What the import found, beside the state it made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub chats: usize,
    pub folders: usize,
    /// The groups 0.19 had, before they were held to five boards.
    pub groups: usize,
    /// What was left out: rows that are no chat, or have no CLI or account
    /// to start them on, and folders with no path, their chats with them.
    pub lost: Lost,
}

/// Why a chat list that is there was not imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotRead {
    /// It does not read, or holds no folder list: an empty list would say
    /// there were no chats when there were. Trying again may help (the
    /// store was caught as it changed).
    Broken(String),
    /// It is of this version, older than `OLDEST`: this build reads nothing
    /// of it, and trying again cannot help.
    Older(u64),
}

/// The state 0.19's stores make, from each key's stored text. None when
/// there is no chat list at all (a profile that kept only its theme, say):
/// nothing to import.
pub fn from_storage(old: &HashMap<String, String>) -> Result<Option<(WindowState, Found)>, NotRead> {
    let Some(chats_text) = old.get("luna.chats") else { return Ok(None) };
    let broken = |why: &str| NotRead::Broken(why.to_owned());
    let chats: Value = serde_json::from_str(chats_text).map_err(|e| broken(&format!("luna.chats does not read: {e}")))?;
    let chats_state = chats.get("state").filter(|s| s.is_object()).ok_or_else(|| broken("luna.chats holds no state"))?;
    // zustand writes 0 for a store that gives no version, as v0.1.0's did.
    let version = chats.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version < OLDEST {
        return Err(NotRead::Older(version));
    }
    let folders = chats_state.get("folders").and_then(Value::as_array).ok_or_else(|| broken("luna.chats holds no folder list"))?;
    let mut state = WindowState::default();
    let mut found = Found::default();

    let new_chat = stored(old, "luna.newchat");
    let last_folder = new_chat.as_ref().and_then(|s| str_of(s, "lastFolder"));
    for f in folders {
        let raw: Vec<&Value> = f.get("chats").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
        let Some(path) = str_of(f, "path") else {
            found.lost.folders += 1;
            found.lost.chats += raw.len();
            continue;
        };
        let mut folder_chats = Vec::new();
        for c in &raw {
            match chat(c) {
                Some(c) => folder_chats.push(c),
                None => found.lost.chats += 1,
            }
        }
        let start =
            f.get("start").and_then(|s| Some(FolderStart { worktree: s.get("worktree")?.as_bool()?, tools: s.get("tools")?.as_bool()? }));
        // Stored before folders said who launched there: as the page's
        // merge decides it.
        let picked = f.get("picked").and_then(Value::as_bool).unwrap_or_else(|| {
            raw.iter().any(|c| c.get("parentId").is_none_or(Value::is_null)) && !raw.is_empty()
                || start.is_some()
                || last_folder.as_deref() == Some(path.as_str())
        });
        state.folders.push(Folder {
            id: str_of(f, "id").unwrap_or_else(|| format!("f{}", state.folders.len())),
            path,
            open: f.get("open").and_then(Value::as_bool).unwrap_or(true),
            chats: folder_chats,
            picked,
            start,
        });
    }
    state.active = chats_state.get("active").and_then(Value::as_str).map(str::to_owned);

    if let Some(panes) = stored(old, "luna.panes") {
        let groups = panes.get("groups").and_then(Value::as_array).cloned().unwrap_or_default();
        found.groups = groups.len();
        for (i, g) in groups.iter().take(BOARDS).enumerate() {
            state.boards[i] = board(g);
        }
        let group = panes.get("group").and_then(Value::as_u64).unwrap_or(0) as usize;
        state.board = group.min(BOARDS - 1);
        state.auto_tile = panes.get("autoTile").and_then(Value::as_bool).unwrap_or(true);
    }
    if let Some(sidebar) = old.get("luna.sidebar") {
        state.sidebar.pinned = sidebar.trim() != "off";
    }
    if let Some(s) = new_chat {
        let p = &mut state.new_chat;
        p.last_folder = last_folder;
        p.last_account = s
            .get("lastAccount")
            .and_then(|a| Some(AccountRef { provider: provider(a.get("provider")?)?, name: a.get("name")?.as_str()?.to_owned() }));
    }
    let listed = state.chats().count();
    state.tidy();
    found.chats = state.chats().count();
    found.folders = state.folders.len();
    // A chat listed twice is kept once (`tidy`), as the window's own file.
    found.lost.twice = listed - found.chats;
    state.imported =
        Some(crate::state::Imported { ok: true, chats: found.chats, folders: found.folders, why: None, seen: true, older: false });
    state.prefs_to_import = Some(prefs_of(old)).filter(OldPrefs::any);
    Ok(Some((state, found)))
}

/// What 0.19's window kept of the choices the host keeps in `window.json`
/// (panels.rs `Prefs`, Settings' own), handed to it as the old store is
/// read (`IMPORT_WINDOW_PREFS`): it takes each only where no choice was
/// made in this window yet. None where 0.19 kept none. Kept in the list
/// until the host has them (`WindowState::prefs_to_import`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OldPrefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound: Option<bool>,
    /// How the new-chat dialog's boxes start in a folder with no start of
    /// its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_chat: Option<OldStart>,
    /// The theme, kept beside them in `window.json`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<ThemePref>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct OldStart {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<bool>,
}

impl OldPrefs {
    pub fn any(&self) -> bool {
        self.sound.is_some() || self.new_chat.is_some() || self.theme.is_some()
    }

    /// These, and `other`'s where these say nothing.
    pub fn or(self, other: OldPrefs) -> OldPrefs {
        OldPrefs { sound: self.sound.or(other.sound), new_chat: self.new_chat.or(other.new_chat), theme: self.theme.or(other.theme) }
    }
}

impl OldStart {
    /// `worktree` and `tools` of `v`, None where it holds neither.
    fn of(v: &Value) -> Option<OldStart> {
        let start = OldStart { worktree: v.get("worktree").and_then(Value::as_bool), tools: v.get("tools").and_then(Value::as_bool) };
        (start != OldStart::default()).then_some(start)
    }
}

/// 0.19's choices, from each key's stored text: `luna.notify`'s sound,
/// `luna.newchat`'s boxes (the fixed defaults of its version 3; an older
/// store, which 0.19 migrated as it read it, names neither) and
/// `luna.theme`, the page's `ThemePref` as plain text, whose three are
/// this window's three.
pub fn prefs_of(old: &HashMap<String, String>) -> OldPrefs {
    let theme = old.get("luna.theme").and_then(|t| match t.trim() {
        "system" => Some(ThemePref::System),
        "light" => Some(ThemePref::Light),
        "dark" => Some(ThemePref::Dark),
        _ => None,
    });
    OldPrefs {
        sound: stored(old, "luna.notify").and_then(|s| s.get("sound").and_then(Value::as_bool)),
        new_chat: stored(old, "luna.newchat").as_ref().and_then(OldStart::of),
        theme,
    }
}

/// 0.19's choices as 0.20.0 imported them, into the window's own file
/// (`notify.sound`, `newChat`'s `worktree` and `tools`) where Settings did
/// not look: taken from a list that imported 0.19's (`imported.ok`), so
/// they are 0.19's and no default. This build keeps them in `window.json`
/// alone and writes them here no more: `notify` there says 0.20.0 wrote
/// the list (`reads_old_prefs_again`).
pub fn prefs_kept_by_0_20_0(saved: &Value) -> OldPrefs {
    if saved.pointer("/imported/ok") != Some(&Value::Bool(true)) {
        return OldPrefs::default();
    }
    // 0.20.0 did not keep 0.19's theme.
    OldPrefs {
        sound: saved.pointer("/notify/sound").and_then(Value::as_bool),
        new_chat: saved.get("newChat").and_then(OldStart::of),
        theme: None,
    }
}

/// Whether the host reads 0.19's store again with the list `saved`, for
/// the choices of it 0.20.0 did not keep (its theme): a list 0.20.0 wrote
/// (`notify`, which this build no longer writes) after importing 0.19's.
/// Once this build writes the list, the store is not read again.
pub fn reads_old_prefs_again(saved: &Value) -> bool {
    saved.get("notify").is_some() && saved.pointer("/imported/ok") == Some(&Value::Bool(true))
}

/// The size of the chat list `luna.chats`'s text `text` holds, as the
/// import would take it (`from_storage`): its chats and folders. None where
/// it is no list that imports. The host asks, to log the lists of the
/// origins it does not take (luna_lib's native/import.rs).
pub fn chat_list_size(text: &str) -> Option<(usize, usize)> {
    let old = HashMap::from([("luna.chats".to_owned(), text.to_owned())]);
    match from_storage(&old) {
        Ok(Some((_, found))) => Some((found.chats, found.folders)),
        _ => None,
    }
}

/// What the import of `old` would leave out, where it imports and leaves
/// anything out: the host keeps a copy of the list as it was before the
/// window takes it (luna_lib's native/state_file.rs).
pub fn import_lost(old: &HashMap<String, String>) -> Option<Lost> {
    from_storage(old).ok().flatten().map(|(_, found)| found.lost).filter(Lost::any)
}

/// A zustand store's `state`, from its key's text.
fn stored(old: &HashMap<String, String>, key: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(old.get(key)?).ok()?;
    v.get("state").filter(|s| s.is_object()).cloned()
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn provider(v: &Value) -> Option<Provider> {
    match v.as_str()? {
        "claude" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        _ => None,
    }
}

/// One stored chat; None for one with no id, CLI or account.
fn chat(c: &Value) -> Option<Chat> {
    let id = str_of(c, "id").filter(|s| !s.is_empty())?;
    let provider = provider(c.get("provider")?)?;
    let account = str_of(c, "account").filter(|s| !s.is_empty())?;
    let session_id = str_of(c, "sessionId");
    let name_source = match c.get("nameSource").and_then(Value::as_str) {
        Some("user") => Some(NameSource::User),
        Some("rename") => Some(NameSource::Rename),
        Some("cli") => Some(NameSource::Cli),
        Some("prompt") => Some(NameSource::Prompt),
        // From before there was one: a flag said whether the person named
        // it; one with a session has been called something by now.
        _ if c.get("nameCustom").and_then(Value::as_bool) == Some(true) => Some(NameSource::User),
        _ if session_id.is_some() => Some(NameSource::Prompt),
        _ => None,
    };
    let status = match c.get("status").and_then(Value::as_str) {
        Some("working") => Status::Working,
        Some("waiting") => Status::Waiting,
        Some("background") => Status::Background,
        _ => Status::Resting,
    };
    let flag = |key: &str| c.get(key).and_then(Value::as_bool).unwrap_or(false);
    Some(Chat {
        name: str_of(c, "name").unwrap_or_else(|| id.clone()),
        name_source,
        status,
        provider,
        account,
        board: (c.get("group").and_then(Value::as_u64).unwrap_or(0) as usize).min(BOARDS - 1),
        worktree: flag("worktree"),
        worktree_path: str_of(c, "worktreePath").filter(|s| !s.is_empty()),
        session_id,
        tools: flag("tools"),
        model: str_of(c, "model"),
        effort: str_of(c, "effort").unwrap_or_default(),
        perm: (provider == Provider::Claude).then(|| str_of(c, "perm")).flatten(),
        approval: (provider == Provider::Codex).then(|| str_of(c, "approval")).flatten(),
        sandbox: (provider == Provider::Codex).then(|| str_of(c, "sandbox")).flatten(),
        read_only: flag("readOnly"),
        agent: str_of(c, "agent"),
        model_seen: str_of(c, "modelSeen"),
        id,
    })
}

/// One stored group: its layout and each layout's panes.
fn board(g: &Value) -> Board {
    let layout = g.get("layout").and_then(Value::as_u64).unwrap_or(1).clamp(1, 4) as u8;
    let mut slots: [Slots; 4] = Default::default();
    for (n, slots) in slots.iter_mut().enumerate() {
        let stored = g.get("boards").and_then(|b| b.get((n + 1).to_string())).and_then(Value::as_array);
        for (i, s) in stored.into_iter().flatten().take(4).enumerate() {
            slots[i] = s.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        }
    }
    Board { layout, slots }
}

/// The state a failed import starts with: empty, the reason kept for the
/// notice that says so.
pub fn failed(why: String) -> WindowState {
    WindowState {
        imported: Some(crate::state::Imported { ok: false, chats: 0, folders: 0, why: Some(why), seen: false, older: false }),
        ..WindowState::default()
    }
}

/// The state an import of a list older than `OLDEST` starts with: empty,
/// and never tried again (`WindowState::import_retryable`).
pub fn older(version: u64) -> WindowState {
    let why = format!("it is of version {version}, this Luna reads {OLDEST} and later");
    WindowState {
        imported: Some(crate::state::Imported { ok: false, chats: 0, folders: 0, why: Some(why), seen: false, older: true }),
        ..WindowState::default()
    }
}

/// A synthetic 0.19 store, for the tests and the shots: three folders, the
/// chats of both CLIs on three groups (one past board V), a group's panes,
/// and the stores' choices. Made up for the purpose; no real data.
#[cfg(any(test, feature = "shots"))]
pub fn fixture() -> HashMap<String, String> {
    serde_json::from_str(include_str!("../fixtures/storage-0.19.json")).expect("the fixture reads")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Peek;

    #[test]
    fn the_fixture_becomes_folders_chats_and_boards() {
        let (state, found) = from_storage(&fixture()).unwrap().unwrap();
        assert_eq!(found, Found { chats: 7, folders: 3, groups: 7, lost: Lost { chats: 1, ..Lost::default() } });
        assert_eq!(
            state.folders.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["C:\\work\\parser", "C:\\work\\site", "/home/demo/notes"]
        );
        let codex = state.find("c-codex").unwrap();
        assert_eq!(codex.provider, Provider::Codex);
        assert_eq!(codex.model, None);
        assert_eq!(codex.approval.as_deref(), Some("on-request"));
        assert_eq!(codex.sandbox.as_deref(), Some("workspace-write"));
        assert_eq!(codex.perm, None);
        let claude = state.find("c-parser").unwrap();
        assert_eq!(claude.name_source, Some(NameSource::Cli));
        assert_eq!(claude.status, Status::Waiting);
        assert_eq!(claude.session_id.as_deref(), Some("0d6f1c2e-0000-4000-8000-000000000001"));
        // Named before there was a name source, by the person.
        assert_eq!(state.find("c-old").unwrap().name_source, Some(NameSource::User));
        // Group VII is board V's now.
        assert_eq!(state.find("c-far").unwrap().board, 4);
        // Group I's panes, layout 2; a pane naming a chat not listed is empty.
        assert_eq!(state.boards[0].layout, 2);
        assert_eq!(state.boards[0].live()[..2], [Some("c-parser".to_string()), Some("c-codex".to_string())]);
        assert_eq!(state.boards[1].live()[0], None, "c-gone is not listed");
        assert_eq!(state.board, 0);
        assert!(!state.auto_tile);
        assert!(!state.sidebar.pinned);
        assert_eq!(state.new_chat.last_folder.as_deref(), Some("C:\\work\\parser"));
        assert_eq!(state.new_chat.last_account, Some(AccountRef { provider: Provider::Claude, name: "work".into() }));
        // A folder stored before `picked`, with a person's chat in it.
        assert!(state.folders[2].picked);
        assert!(!state.folders[1].open);
        // What 0.20 cut is not carried: no color or handoff to keep, and
        // the helper's own asks stay.
        let helper = state.find("c-helper").unwrap();
        assert!(helper.read_only);
        assert_eq!(helper.agent.as_deref(), Some("reviewer"));
        assert_eq!(state.active.as_deref(), Some("c-parser"));
        assert_eq!(state.peek, None::<Peek>);
        let imported = state.imported.as_ref().unwrap();
        assert!(imported.ok && imported.seen);
        // 0.19's choices, kept with the list until the host has them.
        assert_eq!(state.prefs_to_import, Some(prefs_of(&fixture())));
    }

    /// 0.19's choices kept beside its list are for the host's `window.json`,
    /// not the window's state; a list 0.20.0 imported them into gives them
    /// up the same way, one that imported nothing gives up none.
    #[test]
    fn the_choices_0_19_kept_are_handed_to_the_host() {
        let start = OldStart { worktree: Some(false), tools: Some(true) };
        assert_eq!(prefs_of(&fixture()), OldPrefs { sound: Some(false), new_chat: Some(start), theme: Some(ThemePref::Dark) });
        assert_eq!(
            serde_json::to_value(prefs_of(&fixture())).unwrap(),
            serde_json::json!({ "sound": false, "newChat": { "worktree": false, "tools": true }, "theme": "dark" })
        );
        assert!(!prefs_of(&HashMap::new()).any());
        for (text, theme) in [("system", Some(ThemePref::System)), ("light", Some(ThemePref::Light)), ("sepia", None)] {
            let old = HashMap::from([("luna.theme".to_string(), text.to_string())]);
            assert_eq!(prefs_of(&old).theme, theme, "{text}");
        }
        // A version 2 store said how each box started otherwise: nothing of
        // it is taken for a choice.
        let v2 = HashMap::from([("luna.newchat".to_string(), r#"{"state":{"worktreeStart":"off"},"version":2}"#.to_string())]);
        assert!(!prefs_of(&v2).any());
        let imported = serde_json::json!({
            "imported": { "ok": true, "chats": 1, "folders": 1 },
            "notify": { "sound": false },
            "newChat": { "lastFolder": "/a", "worktree": false, "tools": true },
        });
        assert_eq!(prefs_kept_by_0_20_0(&imported), OldPrefs { sound: Some(false), new_chat: Some(start), theme: None });
        let failed = serde_json::json!({
            "imported": { "ok": false, "chats": 0, "folders": 0 },
            "notify": { "sound": true },
            "newChat": { "worktree": true, "tools": false },
        });
        assert!(!prefs_kept_by_0_20_0(&failed).any(), "a failed import's defaults are no choice");
        assert!(!prefs_kept_by_0_20_0(&serde_json::json!({ "notify": { "sound": false } })).any());
        // Read with the list, from 0.20.0's file; its `notify` has the host
        // read the store again for the rest.
        let mut file = serde_json::to_value(WindowState::default()).unwrap();
        file.as_object_mut().unwrap().extend(imported.as_object().unwrap().clone());
        let (state, _) = WindowState::read(&file).unwrap();
        assert_eq!(state.prefs_to_import, Some(OldPrefs { sound: Some(false), new_chat: Some(start), theme: None }));
        assert!(reads_old_prefs_again(&file) && !reads_old_prefs_again(&serde_json::to_value(&state).unwrap()));
    }

    #[test]
    fn no_chat_list_is_nothing_to_import_and_one_that_does_not_read_an_error() {
        assert_eq!(from_storage(&HashMap::new()), Ok(None));
        let theme_only = HashMap::from([("luna.theme".to_string(), "dark".to_string())]);
        assert_eq!(from_storage(&theme_only), Ok(None), "a profile that kept only its theme");
        let bad = HashMap::from([("luna.chats".to_string(), "{not json".to_string())]);
        let broken = |e: NotRead| match e {
            NotRead::Broken(why) => why,
            NotRead::Older(v) => panic!("read as of version {v}"),
        };
        assert!(broken(from_storage(&bad).unwrap_err()).contains("does not read"));
        // A list that says nothing of its folders is not an empty one.
        for text in [r#"{"state":{},"version":2}"#, r#"{"state":{"folders":{}},"version":2}"#] {
            let old = HashMap::from([("luna.chats".to_string(), text.to_string())]);
            assert!(broken(from_storage(&old).unwrap_err()).contains("no folder list"), "{text}");
        }
        let failed = failed("why".into());
        assert!(!failed.imported.unwrap().ok);
    }

    /// A list of a version before `OLDEST` (v0.1.0's said none: 0) is no
    /// empty success: it is not imported, and says its version.
    #[test]
    fn a_list_older_than_this_reads_is_not_an_empty_one() {
        let folders = r#"{"folders":[{"path":"/x","chats":[{"id":"a","provider":"claude","account":"w"}]}]}"#;
        for (text, version) in [
            (format!(r#"{{"state":{folders}}}"#), 0),
            (format!(r#"{{"state":{folders},"version":0}}"#), 0),
            (format!(r#"{{"state":{folders},"version":1}}"#), 1),
        ] {
            let old = HashMap::from([("luna.chats".to_string(), text.clone())]);
            assert_eq!(from_storage(&old), Err(NotRead::Older(version)), "{text}");
            assert_eq!(import_lost(&old), None);
            assert_eq!(chat_list_size(&text), None);
        }
        let older = older(0).imported.unwrap();
        assert!(!older.ok && older.older && older.why.unwrap().contains("version 0"));
    }

    /// A folder with no path, a row that is no chat, a chat with no CLI or
    /// account and one listed twice are left out and counted, a folder's
    /// chats with it.
    #[test]
    fn rows_and_folders_that_do_not_read_are_counted_as_lost() {
        let text = serde_json::json!({ "state": { "folders": [
            { "id": "f1", "path": "/a", "chats": [
                { "id": "c1", "provider": "claude", "account": "work" },
                "not a chat",
                { "id": "c2", "provider": "gemini", "account": "work" },
                { "id": "c3", "provider": "codex" },
                { "id": "c1", "provider": "claude", "account": "work" },
            ] },
            { "id": "f2", "chats": [{ "id": "c4", "provider": "claude", "account": "work" }] },
            "not a folder",
        ] }, "version": 2 })
        .to_string();
        let old = HashMap::from([("luna.chats".to_string(), text)]);
        let (state, found) = from_storage(&old).unwrap().unwrap();
        assert_eq!((found.chats, found.folders), (1, 1));
        assert_eq!(found.lost, Lost { chats: 4, folders: 2, twice: 1, ..Lost::default() });
        assert_eq!(state.folders[0].chats[0].id, "c1");
        assert_eq!(import_lost(&old), Some(found.lost));
        let whole = HashMap::from([("luna.chats".to_string(), r#"{"state":{"folders":[]},"version":2}"#.to_string())]);
        assert_eq!(import_lost(&whole), None, "a list that reads whole loses nothing");
    }
}
