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
//! are not kept. A chat that cannot say which CLI and account it runs on is
//! skipped, as the page's own migration skipped one.

use crate::state::{AccountRef, BOARDS, Board, Chat, Folder, FolderStart, NameSource, Slots, Status, WindowState};
use crate::term::paste::Provider;
use serde_json::Value;
use std::collections::HashMap;

/// What the import found, beside the state it made.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    pub chats: usize,
    pub folders: usize,
    /// The groups 0.19 had, before they were held to five boards.
    pub groups: usize,
    /// Chats left out: no provider or account to start them on.
    pub skipped: usize,
}

/// The state 0.19's stores make, from each key's stored text. None when
/// there is no chat list at all (a profile that kept only its theme, say):
/// nothing to import. An error when the list is there and does not read,
/// or holds no folders where its version has them: an empty list then would
/// say there were no chats when there were.
pub fn from_storage(old: &HashMap<String, String>) -> Result<Option<(WindowState, Found)>, String> {
    let Some(chats_text) = old.get("luna.chats") else { return Ok(None) };
    let chats: Value = serde_json::from_str(chats_text).map_err(|e| format!("luna.chats does not read: {e}"))?;
    let chats_state = chats.get("state").filter(|s| s.is_object()).ok_or("luna.chats holds no state")?;
    let version = chats.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version >= 2 && !chats_state.get("folders").is_some_and(Value::is_array) {
        return Err("luna.chats holds no folder list".into());
    }
    let mut state = WindowState::default();
    let mut found = Found::default();

    let new_chat = stored(old, "luna.newchat");
    let last_folder = new_chat.as_ref().and_then(|s| str_of(s, "lastFolder"));
    // The page's own migration let go of a list from before chats had a
    // provider: there is none to take then.
    if version >= 2 {
        for f in chats_state.get("folders").and_then(Value::as_array).into_iter().flatten() {
            let Some(path) = str_of(f, "path") else { continue };
            let raw: Vec<&Value> = f.get("chats").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
            let mut folder_chats = Vec::new();
            for c in &raw {
                match chat(c) {
                    Some(c) => folder_chats.push(c),
                    None => found.skipped += 1,
                }
            }
            let start = f
                .get("start")
                .and_then(|s| Some(FolderStart { worktree: s.get("worktree")?.as_bool()?, tools: s.get("tools")?.as_bool()? }));
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
    }

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
        p.worktree = s.get("worktree").and_then(Value::as_bool).unwrap_or(p.worktree);
        p.tools = s.get("tools").and_then(Value::as_bool).unwrap_or(p.tools);
    }
    if let Some(sound) = stored(old, "luna.notify").and_then(|s| s.get("sound").and_then(Value::as_bool)) {
        state.notify.sound = sound;
    }

    state.tidy();
    found.chats = state.chats().count();
    found.folders = state.folders.len();
    state.imported = Some(crate::state::Imported { ok: true, chats: found.chats, folders: found.folders, why: None, seen: true });
    Ok(Some((state, found)))
}

/// The size of the chat list `luna.chats`'s text `text` holds, as the
/// import would take it (`from_storage`): its chats and folders. None where
/// it is no list that imports. The host asks, to choose between origins
/// that each kept one (luna_lib's native/import.rs).
pub fn chat_list_size(text: &str) -> Option<(usize, usize)> {
    let old = HashMap::from([("luna.chats".to_owned(), text.to_owned())]);
    match from_storage(&old) {
        Ok(Some((_, found))) => Some((found.chats, found.folders)),
        _ => None,
    }
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
        imported: Some(crate::state::Imported { ok: false, chats: 0, folders: 0, why: Some(why), seen: false }),
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
        assert_eq!(found, Found { chats: 7, folders: 3, groups: 7, skipped: 1 });
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
        assert!(!state.notify.sound);
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
    }

    #[test]
    fn no_chat_list_is_nothing_to_import_and_one_that_does_not_read_an_error() {
        assert_eq!(from_storage(&HashMap::new()), Ok(None));
        let theme_only = HashMap::from([("luna.theme".to_string(), "dark".to_string())]);
        assert_eq!(from_storage(&theme_only), Ok(None), "a profile that kept only its theme");
        let bad = HashMap::from([("luna.chats".to_string(), "{not json".to_string())]);
        assert!(from_storage(&bad).unwrap_err().contains("does not read"));
        // A list that says nothing of its folders is not an empty one.
        for text in [r#"{"state":{},"version":2}"#, r#"{"state":{"folders":{}},"version":2}"#] {
            let old = HashMap::from([("luna.chats".to_string(), text.to_string())]);
            assert!(from_storage(&old).unwrap_err().contains("no folder list"), "{text}");
        }
        // A list from before chats had a provider: read, and empty.
        let old = HashMap::from([(
            "luna.chats".to_string(),
            r#"{"state":{"folders":[{"path":"/x","chats":[{"id":"a"}]}]},"version":1}"#.to_string(),
        )]);
        let (state, found) = from_storage(&old).unwrap().unwrap();
        assert_eq!((found.chats, state.folders.len()), (0, 0));
        let failed = failed("why".into());
        assert!(!failed.imported.unwrap().ok);
    }
}
