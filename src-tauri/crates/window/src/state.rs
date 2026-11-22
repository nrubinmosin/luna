//! The window's own state: the chats and the folders they were launched
//! from, the five boards with what sits in their panes, the sidebar, and the
//! choices the dialogs keep. The host keeps it in a file of the window's
//! (`cmd::SAVE_WINDOW_STATE`); the window never writes a file itself.
//!
//! Plain data and the rules that change it, ported from the page's stores
//! (src/features/chats/chats.store.ts, src/features/panes/panes.store.ts)
//! with what 0.20 cut left out: groups are five fixed boards, chats have no
//! color, no chat or pane is dragged. What is only about this run
//! (the peek, the pane last worked in, each chat's context and draft) is not
//! kept: `#[serde(skip)]` here, or in the workspace (workspace.rs).

use crate::term::paste::Provider;
use serde::{Deserialize, Serialize};

/// The boards, fixed: I to V.
pub const BOARDS: usize = 5;
/// The version of the file this writes. One of another version is read for
/// what this one knows of it (`WindowState::read`), and the host keeps the
/// file aside as it was.
pub const VERSION: u32 = 1;
/// The sidebar's width, and the bounds a kept one is held to.
pub const SIDEBAR_WIDTH: f32 = 300.;
pub const SIDEBAR_MIN: f32 = 220.;
pub const SIDEBAR_MAX: f32 = 480.;

/// A board's name on the title bar.
pub fn board_label(board: usize) -> &'static str {
    ["I", "II", "III", "IV", "V"].get(board).copied().unwrap_or("?")
}

/// What a chat's CLI is doing, as the sidebar and its pane show it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Working,
    /// Waiting for the person: a prompt, a question, a permission.
    Waiting,
    /// Its turn is over and something it started still runs.
    Background,
    /// Nothing runs: its CLI is not started, or exited.
    #[default]
    Resting,
    /// Shown, never kept: the core is not up, so what the chat did last says
    /// nothing of what it does now (`Workspace::shown_status`).
    #[serde(skip)]
    NoCore,
}

impl Status {
    /// The core's word for a session (`SessionMetaDto.status`), as the page's
    /// `mapStatus` folds it.
    pub fn of(raw: Option<&str>) -> Status {
        let Some(s) = raw else { return Status::Resting };
        match s {
            "background" => Status::Background,
            "busy" | "working" => Status::Working,
            _ if ["wait", "input", "block", "attention", "permission"].iter().any(|w| s.contains(w)) => Status::Waiting,
            _ => Status::Resting,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Status::Working => "working",
            Status::Waiting => "waiting",
            Status::Background => "background",
            Status::Resting => "resting",
            Status::NoCore => "no core",
        }
    }
}

/// Where a chat's name came from, which decides what may still replace it
/// (`may_rename`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum NameSource {
    /// Given in Luna: final.
    User,
    /// A `/rename` in the CLI: only another one replaces it.
    Rename,
    /// The title the CLI made up: taken once.
    Cli,
    /// The opening prompt, until the CLI's title lands.
    Prompt,
}

/// Whether a name from `next` may replace one from `current` (none: the
/// placeholder a new chat starts with). A chat is named once and then left
/// alone; only the person's own word moves it on.
pub fn may_rename(current: Option<NameSource>, next: NameSource) -> bool {
    match next {
        NameSource::User => true,
        NameSource::Rename => current != Some(NameSource::User),
        NameSource::Cli | NameSource::Prompt => matches!(current, None | Some(NameSource::Prompt)),
    }
}

/// One chat: a CLI session Luna starts, resumes and shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_source: Option<NameSource>,
    /// Kept, so a window started again does not notify of a chat that was
    /// already waiting.
    #[serde(default)]
    pub status: Status,
    pub provider: Provider,
    /// The account's name; the provider says whose.
    pub account: String,
    /// The board whose sidebar lists it, 0 to 4.
    #[serde(default)]
    pub board: usize,
    /// Started with the CLI's own worktree flag.
    #[serde(default)]
    pub worktree: bool,
    /// Where that worktree turned out to be, to resume in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// The CLI's session, to resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Luna's MCP tools attached.
    #[serde(default)]
    pub tools: bool,
    /// Claude Code: a family's label (`Opus`) or a full id. Codex: its model,
    /// none for Codex's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: String,
    /// Claude Code's permission mode, by Luna's label (`Ask`, `Edits`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perm: Option<String>,
    /// Codex's approval policy and sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
    /// A helper's asks of a Claude chat: no edit tools, an agent to run as.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The model Codex reports running, for a chat that left it to Codex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_seen: Option<String>,
    /// An orphan adopted into this chat whose pane has had no start answered
    /// yet: the start of the session it was adopted for, which that first
    /// start attaches to alone or starts nothing (`Workspace::spec_of`, the
    /// core's `attach`). Saved with the chat, so a window started again
    /// before that answer still holds the start to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adopt: Option<SessionStart>,
}

/// One start of a session: its generation, by the mark of the core that gave
/// it (pty `Start`), which a later core never repeats.
#[derive(Deserialize, Serialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionStart {
    pub generation: u64,
    pub core: String,
}

impl Chat {
    /// Model and effort as a pane's title bar says them: `Opus · high`.
    pub fn model_effort(&self) -> String {
        let model = match self.provider {
            Provider::Claude => self.model.clone().unwrap_or_default(),
            Provider::Codex => self.model.clone().or_else(|| self.model_seen.clone()).unwrap_or_else(|| "Codex default".into()),
        };
        match (model.is_empty(), self.effort.is_empty()) {
            (false, false) => format!("{model} · {}", self.effort),
            (false, true) => model,
            (true, false) => self.effort.clone(),
            (true, true) => String::new(),
        }
    }
}

/// How the new-chat dialog's boxes start in a folder, when kept for it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FolderStart {
    pub worktree: bool,
    pub tools: bool,
}

/// Two spellings of one path: a trailing separator aside, and on Windows
/// either slash and case. Folded only where the file system folds: on
/// Windows `Work` and `work` are one folder; on Linux they are two, and on
/// macOS they may be (APFS can be case-sensitive), where folding could match
/// one account's path to another's and run a chat on that one's
/// credentials, or a chat in another folder. A miss only finds nothing. A
/// folder's row is found by it, so another spelling of a saved folder finds
/// that row and its start, and the saved spelling stays.
pub fn same_path(a: &str, b: &str) -> bool {
    same_path_on(a, b, cfg!(windows))
}

/// `same_path` by the rule of Windows or of the other systems.
fn same_path_on(a: &str, b: &str, windows: bool) -> bool {
    let key = |p: &str| match windows {
        true => p.replace('\\', "/").trim_end_matches('/').to_lowercase(),
        false => p.trim_end_matches('/').to_owned(),
    };
    key(a) == key(b)
}

/// A folder chats were launched from, kept whether anything is in it or not:
/// the new-chat dialog offers the `picked` ones.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub path: String,
    /// Its chats are listed, not folded away.
    pub open: bool,
    pub chats: Vec<Chat>,
    /// A person launched a chat here or browsed to it.
    #[serde(default)]
    pub picked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<FolderStart>,
}

impl Folder {
    /// The folder's last part, as a sidebar header names it.
    pub fn leaf(&self) -> &str {
        self.path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).unwrap_or(&self.path)
    }
}

/// The panes of each layout of a board, in pane order; a layout uses its
/// first `n`.
pub type Slots = [Option<String>; 4];

/// The share of its row or column a divider leaves the pane before it, held
/// to this much either way (0.19's `SPLIT_MIN`, `SPLIT_MAX`).
pub const SPLIT_MIN: f32 = 0.15;
pub const SPLIT_MAX: f32 = 0.85;

/// A layout's dividers (src/features/panes/panes.store.ts `Splits`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Split {
    /// Between the left and the right.
    Col,
    /// In the left column of four panes.
    RowL,
    /// In the right column of three or four.
    RowR,
}

impl Split {
    pub fn key(self) -> &'static str {
        match self {
            Split::Col => "col",
            Split::RowL => "row-l",
            Split::RowR => "row-r",
        }
    }
}

/// Where a layout's dividers sit: the share of the row the left column
/// takes, and of each column its top pane.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Splits {
    pub col: f32,
    pub row_l: f32,
    pub row_r: f32,
}

impl Default for Splits {
    fn default() -> Splits {
        Splits { col: 0.5, row_l: 0.5, row_r: 0.5 }
    }
}

impl Splits {
    pub fn get(&self, split: Split) -> f32 {
        match split {
            Split::Col => self.col,
            Split::RowL => self.row_l,
            Split::RowR => self.row_r,
        }
    }

    /// `split` at `share`, held to the bounds.
    pub fn with(mut self, split: Split, share: f32) -> Splits {
        let share = held_share(share);
        match split {
            Split::Col => self.col = share,
            Split::RowL => self.row_l = share,
            Split::RowR => self.row_r = share,
        }
        self
    }

    fn tidy(&mut self) {
        *self = Splits { col: held_share(self.col), row_l: held_share(self.row_l), row_r: held_share(self.row_r) };
    }
}

/// A share held to `SPLIT_MIN`..`SPLIT_MAX`; one that is no number is even.
fn held_share(share: f32) -> f32 {
    if share.is_finite() { share.clamp(SPLIT_MIN, SPLIT_MAX) } else { 0.5 }
}

/// One board: the layout it is on, and what sits in the panes of each of its
/// four layouts and where their dividers are, each remembered on its own, so
/// 4 -> 1 -> 4 brings back what was there.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    /// 1 to 4 panes.
    pub layout: u8,
    pub slots: [Slots; 4],
    /// The dividers of layouts 2, 3 and 4; one pane has none.
    #[serde(default)]
    pub splits: [Splits; 3],
}

impl Board {
    fn blank(layout: u8) -> Board {
        Board { layout, slots: Default::default(), splits: Default::default() }
    }

    /// The live layout's dividers.
    pub fn splits(&self) -> Splits {
        self.layout().checked_sub(2).map_or_else(Splits::default, |n| self.splits[n])
    }

    pub fn layout(&self) -> usize {
        usize::from(self.layout.clamp(1, 4))
    }

    /// The live layout's panes, all four of them.
    pub fn live(&self) -> &Slots {
        &self.slots[self.layout() - 1]
    }

    fn live_mut(&mut self) -> &mut Slots {
        let n = self.layout();
        &mut self.slots[n - 1]
    }

    /// The pane showing `chat` on the live layout.
    pub fn pane_of(&self, chat: &str) -> Option<usize> {
        self.live()[..self.layout()].iter().position(|s| s.as_deref() == Some(chat))
    }

    /// The chats on the live layout, in the order auto-tile fills its panes.
    fn seated(&self) -> Vec<String> {
        tile_order(self.layout()).iter().filter_map(|&i| self.live()[i].clone()).collect()
    }

    /// Laid out again as exactly as many panes as `chats` (one at least).
    fn tiled(&mut self, chats: &[String]) {
        let n = chats.len().clamp(1, 4);
        let mut slots: Slots = Default::default();
        for (slot, chat) in tile_order(n).iter().zip(chats) {
            slots[*slot] = Some(chat.clone());
        }
        self.layout = n as u8;
        self.slots[n - 1] = slots;
    }
}

/// Each layout's panes in the order auto-tile fills them. Growing a board by
/// one keeps every chat where it was and puts the newcomer in the one new
/// place: 1 -> 2 opens the right half, 2 -> 3 splits the right column, 3 -> 4
/// the left one (layout 4's right column is panes 1 and 3, so the old bottom
/// right moves to 3 and the newcomer takes 2, bottom left).
pub fn tile_order(layout: usize) -> &'static [usize] {
    match layout {
        1 => &[0],
        2 => &[0, 1],
        3 => &[0, 1, 2],
        _ => &[0, 1, 3, 2],
    }
}

/// The sidebar: pinned beside the board, or collapsed to a strip.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sidebar {
    pub pinned: bool,
    pub width: f32,
}

impl Default for Sidebar {
    fn default() -> Sidebar {
        Sidebar { pinned: true, width: SIDEBAR_WIDTH }
    }
}

/// An account by provider and name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AccountRef {
    pub provider: Provider,
    pub name: String,
}

/// What the new-chat dialog keeps: where the last chat was made. How its
/// boxes start is Settings' (panels.rs `NewChatStart`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NewChatPrefs {
    pub last_folder: Option<String>,
    pub last_account: Option<AccountRef>,
}

/// What the one import of 0.19's chat list did (import.rs).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    /// The old list was read; false: it could not be, and the window started
    /// empty.
    pub ok: bool,
    pub chats: usize,
    pub folders: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// The notice that it could not be read was dismissed.
    #[serde(default)]
    pub seen: bool,
    /// It could not be read because it is of a version older than the
    /// import reads (import.rs `OLDEST`): never tried again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub older: bool,
}

/// What is up over the board: a pane of it held up, or a chat with no pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Peek {
    Pane(usize),
    Chat(String),
}

/// Where `auto_place` put a chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placed {
    Pane(usize),
    Peek,
}

/// Everything the window keeps, and what it holds only for this run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub version: u32,
    pub folders: Vec<Folder>,
    /// The chat last worked in.
    #[serde(default)]
    pub active: Option<String>,
    /// The board on screen.
    #[serde(default)]
    pub board: usize,
    pub boards: [Board; BOARDS],
    /// A new chat with no free pane grows the board by one; a pane emptied
    /// shrinks it.
    #[serde(default = "yes")]
    pub auto_tile: bool,
    #[serde(default)]
    pub sidebar: Sidebar,
    #[serde(default)]
    pub new_chat: NewChatPrefs,
    /// The window's scale in percent, one of `scale::STEPS`; the system's
    /// text size is on top of it (scale.rs).
    #[serde(default = "default_ui_scale")]
    pub ui_scale: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported: Option<Imported>,
    /// 0.19's choices the host is yet to take into `window.json` (import.rs
    /// `OldPrefs`): kept here until it has, so the list's write (the
    /// import's own, at once) never ends the import before they are kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefs_to_import: Option<crate::import::OldPrefs>,
    /// The list's Codex chats saved as never + danger-full-access were moved
    /// to Codex's own, once (`move_codex_bypass`). True for a list begun by
    /// this build or imported from 0.19 now; false for one a build before it
    /// wrote.
    #[serde(default)]
    pub codex_bypass_moved: bool,
    /// Up over the board, this run only.
    #[serde(skip)]
    pub peek: Option<Peek>,
    /// The pane last worked in, this run only.
    #[serde(skip)]
    pub active_pane: usize,
}

fn yes() -> bool {
    true
}

fn default_ui_scale() -> u16 {
    crate::scale::DEFAULT
}

/// What `WindowState::read` could not take from a kept file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lost {
    /// The file's version, where it is not `VERSION` (0: it says none).
    pub version: Option<u64>,
    /// Chats left out: one that does not read (no CLI this build knows, no
    /// account), or one in a folder left out.
    pub chats: usize,
    /// Folders left out: one with no id or path.
    pub folders: usize,
    /// The boards were not five, or one of them did not read.
    pub boards: bool,
    /// Settings that did not read and took their default.
    pub settings: usize,
    /// A chat's status or name's source this build does not know, read as
    /// none: what the file said is gone once the list is written.
    pub unknown: usize,
    /// Chats listed more than once: the first is kept, the others are left
    /// out, and whatever differed in them with it.
    pub twice: usize,
}

impl Lost {
    /// Anything was left out, or the file is of another version.
    pub fn any(&self) -> bool {
        *self != Lost::default()
    }

    /// What was left out, in a few words, for a notice.
    pub fn words(&self) -> String {
        let n = |count: usize, one: &str, many: &str| format!("{count} {}", if count == 1 { one } else { many });
        let mut parts = Vec::new();
        if let Some(v) = self.version {
            parts.push(format!("it is of version {v}, this Luna's is {VERSION}"));
        }
        if self.chats > 0 {
            parts.push(format!("{} could not be read", n(self.chats, "chat", "chats")));
        }
        if self.folders > 0 {
            parts.push(format!("{} could not be read", n(self.folders, "folder", "folders")));
        }
        if self.boards {
            parts.push("the boards were not as this Luna keeps them".into());
        }
        if self.settings > 0 {
            parts.push(format!("{} took the default", n(self.settings, "setting", "settings")));
        }
        if self.unknown > 0 {
            parts.push(format!("{} this Luna does not know read as none", n(self.unknown, "value", "values")));
        }
        if self.twice > 0 {
            parts.push(format!("{} listed more than once kept once", n(self.twice, "chat", "chats")));
        }
        parts.join("; ")
    }
}

/// A setting of a kept file: none where it is missing; where it does not
/// read, none and counted.
fn setting<T: serde::de::DeserializeOwned>(obj: &serde_json::Map<String, serde_json::Value>, key: &str, lost: &mut Lost) -> Option<T> {
    let v = obj.get(key).filter(|v| !v.is_null())?;
    let read = serde_json::from_value(v.clone()).ok();
    if read.is_none() {
        lost.settings += 1;
    }
    read
}

/// The statuses and names' sources this build knows: another reads as none,
/// and the next look at the session says what it is.
const KNOWN_STATUS: &[&str] = &["working", "waiting", "background", "resting"];
const KNOWN_NAME_SOURCE: &[&str] = &["user", "rename", "cli", "prompt"];

/// One kept chat, or None where it does not read even so; a value it holds
/// that this build does not know is counted in `lost` as it is let go of.
fn read_chat(c: &serde_json::Value, lost: &mut Lost) -> Option<Chat> {
    let mut c = c.as_object()?.clone();
    let mut unknown = 0;
    for (key, known) in [("status", KNOWN_STATUS), ("nameSource", KNOWN_NAME_SOURCE)] {
        // Null is none, as it was written; anything else unknown is counted.
        if let Some(v) = c.get(key).filter(|v| !v.as_str().is_some_and(|s| known.contains(&s))) {
            unknown += usize::from(!v.is_null());
            c.remove(key);
        }
    }
    let chat = serde_json::from_value(serde_json::Value::Object(c)).ok()?;
    lost.unknown += unknown;
    Some(chat)
}

/// One kept folder, its chats read one by one (those left out counted in
/// `lost`); None where the folder itself does not read.
fn read_folder(f: &serde_json::Value, lost: &mut Lost) -> Option<Folder> {
    let mut obj = f.as_object()?.clone();
    let chats = obj.insert("chats".into(), serde_json::Value::Array(Vec::new()));
    if obj.get("start").is_some_and(|s| !s.is_null() && serde_json::from_value::<FolderStart>(s.clone()).is_err()) {
        obj.remove("start");
        lost.settings += 1;
    }
    let mut folder: Folder = serde_json::from_value(serde_json::Value::Object(obj)).ok()?;
    match chats {
        Some(serde_json::Value::Array(chats)) => {
            for c in &chats {
                match read_chat(c, lost) {
                    Some(c) => folder.chats.push(c),
                    None => lost.chats += 1,
                }
            }
        }
        None | Some(serde_json::Value::Null) => {}
        Some(_) => lost.chats += 1,
    }
    Some(folder)
}

/// One kept board, each layout's panes held to four; None where it says no
/// layout. A divider that is no number sits even.
fn read_board(b: &serde_json::Value) -> Option<Board> {
    use serde_json::Value;
    let layout = b.get("layout")?.as_u64()?.clamp(1, 4) as u8;
    let mut slots: [Slots; 4] = Default::default();
    for (n, stored) in b.get("slots").and_then(Value::as_array).into_iter().flatten().take(4).enumerate() {
        for (i, s) in stored.as_array().into_iter().flatten().take(4).enumerate() {
            slots[n][i] = s.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
        }
    }
    let mut splits: [Splits; 3] = Default::default();
    for (n, stored) in b.get("splits").and_then(Value::as_array).into_iter().flatten().take(3).enumerate() {
        let share = |key: &str| stored.get(key).and_then(Value::as_f64).map_or(0.5, |v| v as f32);
        splits[n] = Splits { col: share("col"), row_l: share("rowL"), row_r: share("rowR") };
    }
    Some(Board { layout, slots, splits })
}

impl Default for WindowState {
    fn default() -> WindowState {
        WindowState {
            version: VERSION,
            folders: Vec::new(),
            active: None,
            board: 0,
            boards: std::array::from_fn(|_| Board::blank(1)),
            auto_tile: true,
            sidebar: Sidebar::default(),
            new_chat: NewChatPrefs::default(),
            ui_scale: crate::scale::DEFAULT,
            imported: None,
            prefs_to_import: None,
            codex_bypass_moved: true,
            peek: None,
            active_pane: 0,
        }
    }
}

impl WindowState {
    /// What a kept file holds, held to what the window can show: boards and
    /// layouts in range, a chat listed once, seated only where it is listed
    /// and once per layout. Read piece by piece, so what does not read costs
    /// only itself: a chat or a folder that does not read is left out and
    /// counted, a status or a name's source this build does not know reads
    /// as none, the boards are held to five, a setting that does not read
    /// takes its default. A file of another version is read the same way.
    /// None only for one that is not an object at all. What was left out is
    /// in `Lost`: the host keeps the file aside as it was when anything was
    /// (luna_lib's native/state_file.rs).
    pub fn read(json: &serde_json::Value) -> Option<(WindowState, Lost)> {
        use serde_json::Value;
        let obj = json.as_object()?;
        let mut lost = Lost::default();
        let version = obj.get("version").and_then(Value::as_u64);
        if version != Some(u64::from(VERSION)) {
            lost.version = Some(version.unwrap_or(0));
        }
        let mut state = WindowState::default();
        match obj.get("folders") {
            Some(Value::Array(folders)) => {
                for f in folders {
                    match read_folder(f, &mut lost) {
                        Some(f) => state.folders.push(f),
                        None => {
                            lost.folders += 1;
                            lost.chats += f.get("chats").and_then(Value::as_array).map_or(0, Vec::len);
                        }
                    }
                }
            }
            None | Some(Value::Null) => {}
            Some(_) => lost.folders += 1,
        }
        match obj.get("boards") {
            Some(Value::Array(boards)) => {
                lost.boards = boards.len() != BOARDS;
                for (i, b) in boards.iter().take(BOARDS).enumerate() {
                    match read_board(b) {
                        Some(b) => state.boards[i] = b,
                        None => lost.boards = true,
                    }
                }
            }
            None | Some(Value::Null) => {}
            Some(_) => lost.boards = true,
        }
        state.active = setting(obj, "active", &mut lost).unwrap_or_default();
        state.board = setting(obj, "board", &mut lost).unwrap_or_default();
        state.auto_tile = setting(obj, "autoTile", &mut lost).unwrap_or(true);
        state.sidebar = setting(obj, "sidebar", &mut lost).unwrap_or_default();
        state.new_chat = setting(obj, "newChat", &mut lost).unwrap_or_default();
        state.ui_scale = setting(obj, "uiScale", &mut lost).unwrap_or(crate::scale::DEFAULT);
        state.imported = setting(obj, "imported", &mut lost);
        // A list 0.20.0 imported 0.19's into kept its choices where nothing
        // read them.
        state.prefs_to_import = setting(obj, "prefsToImport", &mut lost)
            .or_else(|| Some(crate::import::prefs_kept_by_0_20_0(json)))
            .filter(crate::import::OldPrefs::any);
        // Missing: a list written before the move, which is still to make.
        state.codex_bypass_moved = setting(obj, "codexBypassMoved", &mut lost).unwrap_or(false);
        // `tidy` keeps a chat listed twice once: the others are counted.
        let mut ids = std::collections::HashSet::new();
        lost.twice = state.chats().filter(|c| !ids.insert(c.id.as_str())).count();
        state.tidy();
        Some((state, lost))
    }

    /// 0.20.0 started every new Codex chat as never + danger-full-access (its
    /// dialog had no word for Codex's own), whatever config.toml said: once,
    /// the Codex chats saved as that pair move to Codex's own (`codex` for
    /// both, no flag), so they run as the config says. A row keeps no date
    /// or version to tell 0.20.0's from an older import's or a Bypass chosen
    /// on purpose, so every such chat moves; a list imported from 0.19 by
    /// this build starts moved, untouched. Done once: a Bypass chosen again
    /// afterwards stays. The chats moved, by id and name; None where it was
    /// done already.
    pub fn move_codex_bypass(&mut self) -> Option<Vec<(String, String)>> {
        if self.codex_bypass_moved {
            return None;
        }
        self.codex_bypass_moved = true;
        let mut moved = Vec::new();
        for c in self.folders.iter_mut().flat_map(|f| f.chats.iter_mut()) {
            if c.provider == Provider::Codex && c.approval.as_deref() == Some("never") && c.sandbox.as_deref() == Some("danger-full-access")
            {
                c.approval = Some(crate::dialogs::CODEX_OWN.to_owned());
                c.sandbox = Some(crate::dialogs::CODEX_OWN.to_owned());
                moved.push((c.id.clone(), c.name.clone()));
            }
        }
        Some(moved)
    }

    /// The log's line for one chat `move_codex_bypass` moved.
    pub fn codex_bypass_moved_line(id: &str, name: &str) -> String {
        format!(
            "Codex chat {id} \"{name}\": saved as never + danger-full-access, now on Codex's own approval and sandbox \
             (no flag) from its next start; a session of it still running keeps the full bypass until it is started again"
        )
    }

    /// An import of 0.19's list that failed and left the list as it was,
    /// empty: the import is tried again (workspace.rs `loaded`), but for
    /// one of a list too old to read, which another try cannot change.
    pub fn import_retryable(&self) -> bool {
        self.imported.as_ref().is_some_and(|i| !i.ok && !i.older) && self.folders.iter().all(|f| f.chats.is_empty())
    }

    /// Holds the state to what the window can show (`read`).
    pub fn tidy(&mut self) {
        let mut seen = std::collections::HashSet::new();
        for f in &mut self.folders {
            f.chats.retain(|c| seen.insert(c.id.clone()));
            for c in &mut f.chats {
                c.board = c.board.min(BOARDS - 1);
            }
        }
        self.board = self.board.min(BOARDS - 1);
        for b in &mut self.boards {
            b.layout = b.layout.clamp(1, 4);
            b.splits.iter_mut().for_each(Splits::tidy);
            for slots in &mut b.slots {
                let mut here = std::collections::HashSet::new();
                for s in slots.iter_mut() {
                    if s.as_ref().is_some_and(|id| !seen.contains(id) || !here.insert(id.clone())) {
                        *s = None;
                    }
                }
            }
        }
        if self.active.as_ref().is_some_and(|a| !seen.contains(a)) {
            self.active = None;
        }
        self.sidebar.width = self.sidebar.width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        self.ui_scale = crate::scale::snap(self.ui_scale);
    }

    /// The text kept: the state without what is only this run's.
    pub fn to_text(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    // ---------------------------------------------------------------- chats

    pub fn chats(&self) -> impl Iterator<Item = &Chat> {
        self.folders.iter().flat_map(|f| f.chats.iter())
    }

    pub fn find(&self, id: &str) -> Option<&Chat> {
        self.chats().find(|c| c.id == id)
    }

    /// The chats set to an account.
    pub fn chats_on<'a>(&'a self, provider: Provider, account: &'a str) -> impl Iterator<Item = &'a Chat> + 'a {
        self.chats().filter(move |c| c.provider == provider && c.account == account)
    }

    /// An account was renamed: every chat on it follows, and so does the
    /// new-chat dialog's last account (the page's `renameAccount`). Whether
    /// anything did.
    pub fn rename_account(&mut self, provider: Provider, from: &str, to: &str) -> bool {
        let mut any = false;
        if let Some(last) = self.new_chat.last_account.as_mut().filter(|a| a.provider == provider && a.name == from) {
            last.name = to.to_owned();
            any = true;
        }
        for c in self.folders.iter_mut().flat_map(|f| f.chats.iter_mut()) {
            if c.provider == provider && c.account == from {
                c.account = to.to_owned();
                any = true;
            }
        }
        any
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Chat> {
        self.folders.iter_mut().flat_map(|f| f.chats.iter_mut()).find(|c| c.id == id)
    }

    pub fn folder_of(&self, id: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| f.chats.iter().any(|c| c.id == id))
    }

    /// The folders with chats on `board`, each with only those: the
    /// sidebar's sections. A folder with none there is not listed.
    pub fn folders_of_board(&self, board: usize) -> Vec<(&Folder, Vec<&Chat>)> {
        self.folders
            .iter()
            .map(|f| (f, f.chats.iter().filter(|c| c.board == board).collect::<Vec<_>>()))
            .filter(|(_, chats)| !chats.is_empty())
            .collect()
    }

    /// The sidebar's edge dragged to `width` (at 100%), whole and held to
    /// its bounds. False when that is the width it has.
    pub fn set_sidebar_width(&mut self, width: f32) -> bool {
        if !width.is_finite() {
            return false;
        }
        let width = width.round().clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let changed = width != self.sidebar.width;
        self.sidebar.width = width;
        changed
    }

    /// `board`'s chats in the order the sidebar lists them, folded folders
    /// skipped: what the rows' numbers count and Ctrl+digit goes by in the
    /// one-pane layout.
    pub fn numbered(&self, board: usize) -> Vec<String> {
        self.folders_of_board(board)
            .into_iter()
            .filter(|(f, _)| f.open)
            .flat_map(|(_, chats)| chats.into_iter().map(|c| c.id.clone()))
            .collect()
    }

    /// The folder row for `path`, by any spelling of it (`same_path`).
    pub fn folder_at(&self, path: &str) -> Option<&Folder> {
        self.folders.iter().find(|f| same_path(&f.path, path))
    }

    pub fn folder_at_mut(&mut self, path: &str) -> Option<&mut Folder> {
        self.folders.iter_mut().find(|f| same_path(&f.path, path))
    }

    /// A chat into the folder it was launched from, made if new; `by_person`
    /// makes the folder one the new-chat dialog offers. The chat becomes the
    /// active one. A chat listed already is not listed twice.
    pub fn add_chat(&mut self, folder: &str, chat: Chat, by_person: bool, new_id: impl FnOnce() -> String) {
        let id = chat.id.clone();
        if self.find(&id).is_none() {
            match self.folder_at_mut(folder) {
                Some(f) => {
                    f.open = true;
                    f.picked |= by_person;
                    f.chats.push(chat);
                }
                None => self.folders.push(Folder {
                    id: new_id(),
                    path: folder.to_owned(),
                    open: true,
                    chats: vec![chat],
                    picked: by_person,
                    start: None,
                }),
            }
        }
        self.active = Some(id);
    }

    /// Takes a chat off every board and out of the list; its folder stays,
    /// a place to launch from. False when there was no such chat.
    pub fn delete_chat(&mut self, id: &str) -> bool {
        if self.find(id).is_none() {
            return false;
        }
        self.evict(id);
        for f in &mut self.folders {
            f.chats.retain(|c| c.id != id);
        }
        if self.active.as_deref() == Some(id) {
            self.active = None;
        }
        true
    }

    /// Names a chat from `source`, where its name may still be replaced from
    /// there; true when it changed.
    pub fn set_name(&mut self, id: &str, name: &str, source: NameSource) -> bool {
        let name = name.trim();
        let Some(c) = self.find_mut(id) else { return false };
        if name.is_empty() || !may_rename(c.name_source, source) || (c.name == name && c.name_source == Some(source)) {
            return false;
        }
        c.name = name.to_owned();
        c.name_source = Some(source);
        true
    }

    /// Folds a folder's chats away, or lists them again.
    pub fn toggle_folder(&mut self, folder_id: &str) {
        if let Some(f) = self.folders.iter_mut().find(|f| f.id == folder_id) {
            f.open = !f.open;
        }
    }

    // --------------------------------------------------------------- boards

    pub fn current(&self) -> &Board {
        &self.boards[self.board.min(BOARDS - 1)]
    }

    fn current_mut(&mut self) -> &mut Board {
        &mut self.boards[self.board.min(BOARDS - 1)]
    }

    pub fn layout(&self) -> usize {
        self.current().layout()
    }

    /// The chat in pane `i` of the board on screen.
    pub fn seat(&self, i: usize) -> Option<&str> {
        (i < self.layout()).then(|| self.current().live()[i].as_deref()).flatten()
    }

    /// The pane of the board on screen showing `chat`.
    pub fn pane_of(&self, chat: &str) -> Option<usize> {
        self.current().pane_of(chat)
    }

    /// Another board, or another layout, is another set of panes: what was
    /// up over the old one and the pane last worked in go.
    fn left_board(&mut self) {
        self.peek = None;
        self.active_pane = 0;
    }

    pub fn set_board(&mut self, board: usize) {
        if board < BOARDS && board != self.board {
            self.board = board;
            self.left_board();
        }
    }

    /// Ctrl+Tab: the next board, round.
    pub fn next_board(&mut self) {
        self.set_board((self.board + 1) % BOARDS);
    }

    pub fn set_layout(&mut self, n: usize) {
        if (1..=4).contains(&n) {
            self.current_mut().layout = n as u8;
            self.left_board();
        }
    }

    /// Clear board: every pane of every layout of `board` emptied and every
    /// divider even (0.19's `resetGroup`), its layout kept; the chats stay
    /// where they are listed.
    pub fn clear_board(&mut self, board: usize) {
        if let Some(b) = self.boards.get_mut(board) {
            *b = Board::blank(b.layout);
        }
        if board == self.board {
            self.left_board();
        }
    }

    /// A divider of layout `layout` of `board` dragged to `share`, held to
    /// the bounds; one pane has none.
    pub fn set_split(&mut self, board: usize, layout: usize, split: Split, share: f32) {
        if let (Some(b), Some(n @ 0..3)) = (self.boards.get_mut(board), layout.checked_sub(2)) {
            b.splits[n] = b.splits[n].with(split, share);
        }
    }

    /// Put `chat` in pane `pane`: it leaves the pane it was in, and whatever
    /// was in that one goes off the board.
    pub fn put(&mut self, pane: usize, chat: &str) {
        if pane >= self.layout() {
            return;
        }
        let slots = self.current_mut().live_mut();
        for s in slots.iter_mut() {
            if s.as_deref() == Some(chat) {
                *s = None;
            }
        }
        slots[pane] = Some(chat.to_owned());
        self.active_pane = pane;
        if self.peek == Some(Peek::Chat(chat.to_owned())) {
            self.peek = None;
        }
    }

    /// Two panes exchange what they show.
    pub fn swap(&mut self, a: usize, b: usize) {
        let n = self.layout();
        if a == b || a >= n || b >= n {
            return;
        }
        self.current_mut().live_mut().swap(a, b);
        self.active_pane = b;
        // What was held up stays the chat it was, wherever it went.
        if let Some(Peek::Pane(p)) = self.peek {
            self.peek = Some(Peek::Pane(if p == a {
                b
            } else if p == b {
                a
            } else {
                p
            }));
        }
    }

    /// The chat in pane `pane` off the board; it runs on and stays listed.
    /// Auto-tile closes the gap, one pane fewer.
    pub fn unseat(&mut self, pane: usize) {
        if pane >= self.layout() {
            return;
        }
        let was = self.current().live()[self.active_pane.min(3)].clone();
        self.current_mut().live_mut()[pane] = None;
        if self.auto_tile {
            let rest = self.current().seated();
            if rest.len() < self.layout() {
                self.current_mut().tiled(&rest);
                self.peek = None;
                self.active_pane = self.keep_active(was);
                return;
            }
        }
        // An emptied pane has nothing to hold up.
        if self.peek == Some(Peek::Pane(pane)) {
            self.peek = None;
        }
    }

    fn keep_active(&self, was: Option<String>) -> usize {
        was.and_then(|id| self.pane_of(&id)).unwrap_or(0)
    }

    /// A chat taken off every layout of every board (it was deleted).
    /// Auto-tile closes the gap on each board whose live layout had it.
    pub fn evict(&mut self, chat: &str) {
        let was = self.current().live()[self.active_pane.min(3)].clone();
        let layout_before = self.layout();
        let auto = self.auto_tile;
        for b in &mut self.boards {
            let had = b.pane_of(chat).is_some();
            for slots in &mut b.slots {
                for s in slots.iter_mut() {
                    if s.as_deref() == Some(chat) {
                        *s = None;
                    }
                }
            }
            if auto && had {
                let rest = b.seated();
                if rest.len() < b.layout() {
                    b.tiled(&rest);
                }
            }
        }
        if self.layout() != layout_before {
            self.peek = None;
            self.active_pane = self.keep_active(was);
            return;
        }
        match &self.peek {
            Some(Peek::Chat(id)) if id == chat => self.peek = None,
            Some(Peek::Pane(p)) if self.seat(*p).is_none() => self.peek = None,
            _ => {}
        }
    }

    /// Seats a chat where it will be seen: a free pane of the board on
    /// screen; with none, auto-tile grows the board by one pane; with no room
    /// to grow it comes up over the board, and the board stays as arranged.
    /// The board's other layouts take it where they have room.
    pub fn auto_place(&mut self, chat: &str) -> Placed {
        let live = self.layout();
        let on_board = self.pane_of(chat).is_some();
        if self.auto_tile && !on_board && live < 4 && self.current().seated().len() >= live {
            let mut chats = self.current().seated();
            chats.push(chat.to_owned());
            self.current_mut().tiled(&chats);
            let at = self.pane_of(chat).unwrap_or(0);
            self.active_pane = at;
            self.peek = None;
            return Placed::Pane(at);
        }
        let board = self.current_mut();
        for n in 1..=4 {
            let slots = &mut board.slots[n - 1];
            if slots.iter().any(|s| s.as_deref() == Some(chat)) {
                continue;
            }
            if let Some(free) = slots[..n].iter().position(Option::is_none) {
                slots[free] = Some(chat.to_owned());
            }
        }
        match self.pane_of(chat) {
            Some(at) => {
                self.active_pane = at;
                if self.peek.is_some() {
                    self.peek = Some(Peek::Pane(at));
                }
                Placed::Pane(at)
            }
            None => {
                self.peek = Some(Peek::Chat(chat.to_owned()));
                Placed::Peek
            }
        }
    }

    /// "To board" on a chat up over the board: seated as a new chat is
    /// (`auto_place`); where that finds no pane, it takes the pane last
    /// worked in.
    pub fn to_board(&mut self, chat: &str) -> usize {
        match self.auto_place(chat) {
            Placed::Pane(at) => {
                self.peek = None;
                at
            }
            Placed::Peek => {
                let at = self.active_pane.min(self.layout() - 1);
                self.put(at, chat);
                self.peek = None;
                at
            }
        }
    }

    /// A click on a chat's row. With one pane, the pane shows it. With more,
    /// the board is an arrangement the click leaves as it is: a chat on it
    /// is held up from its own pane, one that is not comes up over it; the
    /// row of the chat up already puts it back.
    pub fn show_chat(&mut self, chat: &str) {
        self.active = Some(chat.to_owned());
        if self.layout() == 1 {
            self.current_mut().live_mut()[0] = Some(chat.to_owned());
            self.active_pane = 0;
            self.peek = None;
            return;
        }
        match self.pane_of(chat) {
            None if self.peek == Some(Peek::Chat(chat.to_owned())) => self.peek = None,
            None => self.peek = Some(Peek::Chat(chat.to_owned())),
            Some(i) if self.peek == Some(Peek::Pane(i)) => self.peek = None,
            Some(i) => {
                self.active_pane = i;
                self.peek = Some(Peek::Pane(i));
            }
        }
    }

    /// A notification's click: the chat up over whatever is on screen,
    /// never put back, the board untouched. A chat on a pane of the board
    /// on screen is shown there.
    pub fn open_chat(&mut self, chat: &str) {
        self.active = Some(chat.to_owned());
        match self.pane_of(chat) {
            Some(i) => {
                self.active_pane = i;
                self.peek = (self.layout() > 1).then_some(Peek::Pane(i));
            }
            None => self.peek = Some(Peek::Chat(chat.to_owned())),
        }
    }

    /// Ctrl+Shift+W: the next chat waiting for the person, round the list;
    /// its board, its pane if it has one, else up over the board. `status`
    /// is the one each chat shows (`Workspace::shown_status`), which the
    /// Waiting count goes by too: the chat's own is a round behind it.
    pub fn next_waiting(&mut self, status: impl Fn(&Chat) -> Status) -> Option<String> {
        let waiting: Vec<(String, usize)> =
            self.chats().filter(|c| status(c) == Status::Waiting).map(|c| (c.id.clone(), c.board)).collect();
        if waiting.is_empty() {
            return None;
        }
        let at = self.active.as_ref().and_then(|a| waiting.iter().position(|(id, _)| id == a));
        let (id, board) = waiting[at.map_or(0, |i| (i + 1) % waiting.len())].clone();
        self.set_board(board);
        self.active = Some(id.clone());
        match self.pane_of(&id) {
            Some(i) => {
                self.peek = None;
                self.active_pane = i;
            }
            None => self.peek = Some(Peek::Chat(id.clone())),
        }
        Some(id)
    }

    /// Ctrl+digit: with panes to choose between, the keyboard into pane
    /// `digit`; with one, the chat of that number in the list, as its row's
    /// click. The chat whose terminal should have the keyboard, or None when
    /// the digit means nothing here.
    pub fn digit(&mut self, digit: usize) -> Option<String> {
        if digit == 0 {
            return None;
        }
        if self.layout() > 1 {
            if digit > self.layout() {
                return None;
            }
            self.peek = None;
            self.active_pane = digit - 1;
            return self.seat(digit - 1).map(str::to_owned);
        }
        let chat = self.numbered(self.board).get(digit - 1).cloned()?;
        self.show_chat(&chat);
        Some(chat)
    }

    /// Alt+digit: the chat seated in pane `digit` up over the board, or
    /// put back when it is up already. Some(the chat) when it went up,
    /// Some(None) when it was put back, None when the digit means nothing
    /// here: one pane, a pane past the layout, or none seated there.
    pub fn toggle_peek(&mut self, digit: usize) -> Option<Option<String>> {
        if digit == 0 || self.layout() < 2 || digit > self.layout() {
            return None;
        }
        let pane = digit - 1;
        let chat = self.seat(pane)?.to_owned();
        if self.peek == Some(Peek::Pane(pane)) {
            self.peek = None;
            return Some(None);
        }
        self.active_pane = pane;
        self.active = Some(chat.clone());
        self.peek = Some(Peek::Pane(pane));
        Some(Some(chat))
    }

    /// The chat up over the board, if any.
    pub fn peeked_chat(&self) -> Option<&str> {
        match &self.peek {
            Some(Peek::Chat(id)) => Some(id),
            Some(Peek::Pane(i)) => self.seat(*i),
            None => None,
        }
    }

    /// The chat whose terminal the keyboard belongs in: the one up over the
    /// board, or the pane last worked in.
    pub fn home_chat(&self) -> Option<&str> {
        self.peeked_chat().or_else(|| self.seat(self.active_pane))
    }

    /// Every chat a pane or the peek shows now.
    pub fn shown(&self) -> Vec<String> {
        let mut out: Vec<String> = (0..self.layout()).filter_map(|i| self.seat(i).map(str::to_owned)).collect();
        if let Some(Peek::Chat(id)) = &self.peek
            && !out.contains(id)
        {
            out.push(id.clone());
        }
        out
    }

    /// What a write must not wait for: the chats and where they live, their
    /// sessions and accounts. Two states with the same key differ only in
    /// what can wait for the next write (`Saver`, workspace.rs).
    pub fn critical_key(&self) -> Vec<(String, String, Option<String>, String, usize, Option<String>)> {
        self.folders
            .iter()
            .flat_map(|f| {
                f.chats
                    .iter()
                    .map(|c| (f.path.clone(), c.id.clone(), c.session_id.clone(), c.account.clone(), c.board, c.worktree_path.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn chat(id: &str, board: usize) -> Chat {
        Chat {
            id: id.into(),
            name: format!("chat {id}"),
            name_source: None,
            status: Status::Resting,
            provider: Provider::Claude,
            account: "work".into(),
            board,
            worktree: false,
            worktree_path: None,
            session_id: None,
            tools: false,
            model: Some("Opus".into()),
            effort: "high".into(),
            perm: Some("Ask".into()),
            approval: None,
            sandbox: None,
            read_only: false,
            agent: None,
            model_seen: None,
            adopt: None,
        }
    }

    fn with(chats: &[&str]) -> WindowState {
        let mut s = WindowState::default();
        let mut n = 0;
        for id in chats {
            s.add_chat("/repo", chat(id, 0), true, || {
                n += 1;
                format!("f{n}")
            });
        }
        s
    }

    fn live(s: &WindowState) -> Vec<Option<&str>> {
        s.current().live()[..s.layout()].iter().map(|x| x.as_deref()).collect()
    }

    #[test]
    fn a_name_is_replaced_only_from_where_it_may_be() {
        assert!(may_rename(None, NameSource::Prompt));
        assert!(may_rename(Some(NameSource::Prompt), NameSource::Cli));
        assert!(!may_rename(Some(NameSource::Cli), NameSource::Cli));
        assert!(!may_rename(Some(NameSource::Cli), NameSource::Prompt));
        assert!(may_rename(Some(NameSource::Cli), NameSource::Rename));
        assert!(!may_rename(Some(NameSource::User), NameSource::Rename));
        assert!(may_rename(Some(NameSource::User), NameSource::User));
        let mut s = with(&["a"]);
        assert!(s.set_name("a", "fix the parser", NameSource::Prompt));
        assert!(s.set_name("a", "Parser fix", NameSource::Cli));
        assert!(!s.set_name("a", "Another title", NameSource::Cli));
        assert!(s.set_name("a", "Parser rework", NameSource::Rename));
        assert!(s.set_name("a", "Mine", NameSource::User));
        assert!(!s.set_name("a", "theirs", NameSource::Rename));
        assert!(!s.set_name("a", "  ", NameSource::User), "a blank name is no name");
        assert_eq!(s.find("a").unwrap().name, "Mine");
    }

    #[test]
    fn the_cores_status_words_fold_into_four() {
        assert_eq!(Status::of(None), Status::Resting);
        assert_eq!(Status::of(Some("busy")), Status::Working);
        assert_eq!(Status::of(Some("waiting_for_input")), Status::Waiting);
        assert_eq!(Status::of(Some("permission")), Status::Waiting);
        assert_eq!(Status::of(Some("background")), Status::Background);
        assert_eq!(Status::of(Some("idle")), Status::Resting);
    }

    #[test]
    fn folders_list_their_boards_chats_and_numbers_skip_folded_ones() {
        let mut s = with(&["a", "b"]);
        s.add_chat("/other", chat("c", 0), true, || "f9".into());
        s.add_chat("/third", chat("d", 2), true, || "f10".into());
        assert_eq!(s.folders_of_board(0).len(), 2);
        assert_eq!(s.folders_of_board(2).len(), 1);
        assert!(s.folders_of_board(1).is_empty());
        assert_eq!(s.numbered(0), ["a", "b", "c"]);
        let first = s.folders[0].id.clone();
        s.toggle_folder(&first);
        assert_eq!(s.numbered(0), ["c"]);
        s.toggle_folder(&first);
        assert_eq!(s.numbered(0), ["a", "b", "c"]);
        // Listed once, whatever asks.
        s.add_chat("/repo", chat("a", 0), false, || unreachable!());
        assert_eq!(s.chats().count(), 4);
        assert_eq!(s.folders[0].leaf(), "repo");
    }

    #[test]
    fn auto_tile_grows_the_board_in_tile_order_and_shrinks_it_back() {
        let mut s = with(&["a", "b", "c", "d", "e"]);
        assert_eq!(s.auto_place("a"), Placed::Pane(0));
        assert_eq!(live(&s), [Some("a")]);
        assert_eq!(s.auto_place("b"), Placed::Pane(1));
        assert_eq!(s.auto_place("c"), Placed::Pane(2));
        assert_eq!(live(&s), [Some("a"), Some("b"), Some("c")]);
        // 3 -> 4: the old bottom right moves to 3, the new one takes 2.
        assert_eq!(s.auto_place("d"), Placed::Pane(2));
        assert_eq!(live(&s), [Some("a"), Some("b"), Some("d"), Some("c")]);
        // Full at four: up over the board, the board as it was.
        assert_eq!(s.auto_place("e"), Placed::Peek);
        assert_eq!(s.peek, Some(Peek::Chat("e".into())));
        assert_eq!(s.layout(), 4);
        // Shrinking walks the order back.
        s.unseat(2);
        assert_eq!(live(&s), [Some("a"), Some("b"), Some("c")]);
        s.unseat(0);
        assert_eq!(live(&s), [Some("b"), Some("c")]);
    }

    #[test]
    fn without_auto_tile_a_full_board_stays_and_an_emptied_pane_stays_empty() {
        let mut s = with(&["a", "b"]);
        s.auto_tile = false;
        assert_eq!(s.auto_place("a"), Placed::Pane(0));
        assert_eq!(s.auto_place("b"), Placed::Peek);
        s.set_layout(2);
        // Layout 2 took both where it had room, as each was placed.
        assert_eq!(live(&s), [Some("a"), Some("b")]);
        s.unseat(0);
        assert_eq!(live(&s), [None, Some("b")]);
        assert_eq!(s.layout(), 2);
    }

    #[test]
    fn put_swap_and_unseat_move_chats_between_panes() {
        let mut s = with(&["a", "b", "c"]);
        s.auto_tile = false;
        s.set_layout(3);
        s.put(0, "a");
        s.put(1, "b");
        assert_eq!(live(&s), [Some("a"), Some("b"), None]);
        // A chat put elsewhere leaves where it was.
        s.put(2, "a");
        assert_eq!(live(&s), [None, Some("b"), Some("a")]);
        // Into a full pane: what was there goes off the board.
        s.put(1, "c");
        assert_eq!(live(&s), [None, Some("c"), Some("a")]);
        s.swap(1, 2);
        assert_eq!(live(&s), [None, Some("a"), Some("c")]);
        assert_eq!(s.active_pane, 2);
        s.unseat(1);
        assert_eq!(live(&s), [None, None, Some("c")]);
        // Out of range: nothing.
        s.put(3, "b");
        s.swap(0, 3);
        assert_eq!(live(&s), [None, None, Some("c")]);
    }

    #[test]
    fn each_layout_and_board_keeps_its_own_panes() {
        let mut s = with(&["a", "b", "c"]);
        s.auto_tile = false;
        s.set_layout(2);
        s.put(0, "a");
        s.put(1, "b");
        s.set_layout(1);
        s.put(0, "c");
        s.set_layout(2);
        assert_eq!(live(&s), [Some("a"), Some("b")]);
        s.set_board(1);
        assert_eq!(s.layout(), 1);
        assert_eq!(live(&s), [None]);
        s.next_board();
        s.next_board();
        s.next_board();
        s.next_board();
        assert_eq!(s.board, 0, "round the five");
        assert_eq!(live(&s), [Some("a"), Some("b")]);
        s.clear_board(0);
        assert_eq!(live(&s), [None, None]);
        assert_eq!(s.layout(), 2, "Clear keeps the layout");
        s.set_layout(1);
        assert_eq!(live(&s), [None], "and empties every layout");
        assert_eq!(s.chats().count(), 3, "and no chat goes");
    }

    #[test]
    fn a_rows_click_fills_the_one_pane_or_peeks_and_puts_back() {
        let mut s = with(&["a", "b", "c"]);
        s.show_chat("a");
        assert_eq!(live(&s), [Some("a")]);
        s.show_chat("b");
        assert_eq!(live(&s), [Some("b")]);
        s.auto_tile = false;
        s.set_layout(2);
        s.put(0, "a");
        // Not on the board: up over it; again: back.
        s.show_chat("c");
        assert_eq!(s.peek, Some(Peek::Chat("c".into())));
        assert_eq!(s.peeked_chat(), Some("c"));
        s.show_chat("c");
        assert_eq!(s.peek, None);
        // On the board: its own pane held up.
        s.show_chat("a");
        assert_eq!(s.peek, Some(Peek::Pane(0)));
        assert_eq!(s.peeked_chat(), Some("a"));
        s.show_chat("a");
        assert_eq!(s.peek, None);
        assert_eq!(live(&s), [Some("a"), None], "the board as it was");
    }

    #[test]
    fn to_board_seats_a_peeked_chat_or_takes_the_pane_worked_in() {
        let mut s = with(&["a", "b", "c"]);
        s.auto_tile = false;
        s.set_layout(2);
        s.put(0, "a");
        s.show_chat("b");
        assert_eq!(s.to_board("b"), 1);
        assert_eq!(s.peek, None);
        assert_eq!(live(&s), [Some("a"), Some("b")]);
        s.active_pane = 0;
        s.show_chat("c");
        assert_eq!(s.to_board("c"), 0);
        assert_eq!(live(&s), [Some("c"), Some("b")]);
    }

    #[test]
    fn a_deleted_chat_leaves_every_board_and_the_peek() {
        let mut s = with(&["a", "b", "c"]);
        s.auto_place("a");
        s.auto_place("b");
        s.set_board(3);
        s.put(0, "b");
        s.set_board(0);
        s.show_chat("c");
        assert!(s.delete_chat("b"));
        assert_eq!(live(&s), [Some("a")], "auto-tile closed the gap");
        assert_eq!(s.boards[3].live()[0], None);
        assert!(!s.delete_chat("b"));
        assert!(s.delete_chat("c"));
        assert_eq!(s.peek, None);
        assert_eq!(s.folders.len(), 1, "the folder stays");
    }

    #[test]
    fn the_next_waiting_chat_goes_round_across_boards() {
        let mut s = with(&["a", "b", "c"]);
        s.find_mut("b").unwrap().status = Status::Waiting;
        s.find_mut("c").unwrap().status = Status::Waiting;
        s.find_mut("c").unwrap().board = 2;
        let own = |c: &Chat| c.status;
        assert_eq!(s.next_waiting(own).as_deref(), Some("b"));
        assert_eq!(s.board, 0);
        assert_eq!(s.peek, Some(Peek::Chat("b".into())));
        assert_eq!(s.next_waiting(own).as_deref(), Some("c"));
        assert_eq!(s.board, 2);
        assert_eq!(s.next_waiting(own).as_deref(), Some("b"));
        s.find_mut("b").unwrap().status = Status::Resting;
        s.find_mut("c").unwrap().status = Status::Resting;
        assert_eq!(s.next_waiting(own), None);
    }

    /// The status shown is the word, not the chat's own a round behind it:
    /// a chat the core's record has waiting is gone to, one it has back at
    /// work is passed over.
    #[test]
    fn the_next_waiting_chat_is_one_shown_waiting() {
        let mut s = with(&["a", "b", "c"]);
        s.find_mut("b").unwrap().status = Status::Waiting;
        let shown = |c: &Chat| match c.id.as_str() {
            "a" => Status::Waiting,
            "b" => Status::Working,
            _ => c.status,
        };
        assert_eq!(s.next_waiting(shown).as_deref(), Some("a"));
        assert_eq!(s.next_waiting(shown).as_deref(), Some("a"), "b is not waiting any more");
    }

    #[test]
    fn digits_pick_a_pane_or_count_through_the_list() {
        let mut s = with(&["a", "b", "c"]);
        assert_eq!(s.digit(2).as_deref(), Some("b"));
        assert_eq!(live(&s), [Some("b")]);
        assert_eq!(s.digit(9), None);
        s.auto_tile = false;
        s.set_layout(3);
        s.put(1, "c");
        assert_eq!(s.digit(2).as_deref(), Some("c"));
        assert_eq!(s.active_pane, 1);
        assert_eq!(s.digit(1), None, "an empty pane, focused all the same");
        assert_eq!(s.active_pane, 0);
        assert_eq!(s.digit(4), None);
    }

    /// Alt+digit puts the chat seated in that pane up over the board and
    /// the same key puts it back; another pane's key moves the peek there;
    /// one pane, an empty one or one past the layout means nothing.
    #[test]
    fn alt_digits_toggle_the_peek_of_a_seated_pane() {
        let mut s = with(&["a", "b", "c"]);
        assert_eq!(s.toggle_peek(1), None, "one pane: nothing to peek over");
        s.auto_tile = false;
        s.set_layout(3);
        s.put(0, "a");
        s.put(1, "b");
        assert_eq!(s.toggle_peek(2), Some(Some("b".into())));
        assert_eq!((s.peek.clone(), s.active_pane, s.active.as_deref()), (Some(Peek::Pane(1)), 1, Some("b")));
        assert_eq!(s.toggle_peek(1), Some(Some("a".into())), "another pane's key moves the peek");
        assert_eq!(s.peek, Some(Peek::Pane(0)));
        assert_eq!(s.toggle_peek(1), Some(None), "the same key puts it back");
        assert_eq!(s.peek, None);
        assert_eq!(s.toggle_peek(3), None, "an empty pane");
        assert_eq!(s.toggle_peek(4), None, "past the layout");
        assert_eq!(s.toggle_peek(0), None);
    }

    #[test]
    fn a_kept_state_round_trips_and_is_held_to_what_can_be_shown() {
        let mut s = with(&["a", "b"]);
        s.auto_place("a");
        s.peek = Some(Peek::Chat("b".into()));
        let text = s.to_text();
        let (back, lost) = WindowState::read(&serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(back.folders, s.folders);
        assert_eq!(back.boards, s.boards);
        assert_eq!(back.peek, None, "the peek is this run's");
        assert!(!lost.any(), "{lost:?}");
        // A pane naming a chat not listed, a board out of range.
        let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
        json["boards"][0]["slots"][0][1] = "ghost".into();
        json["boards"][0]["layout"] = 9.into();
        json["board"] = 7.into();
        let (held, lost) = WindowState::read(&json).unwrap();
        assert_eq!(held.boards[0].layout, 4);
        assert_eq!(held.boards[0].slots[0][1], None);
        assert_eq!(held.board, BOARDS - 1);
        assert!(!lost.any(), "held to range, nothing left out");
        assert!(WindowState::read(&serde_json::json!([1, 2])).is_none(), "not an object");
    }

    /// The window's scale is kept, held to the steps; a file from before it
    /// starts at 100%, and one of the wrong type costs that setting alone.
    #[test]
    fn the_ui_scale_is_kept_and_held_to_the_steps() {
        let mut s = with(&["a"]);
        s.ui_scale = 175;
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        assert_eq!(json["uiScale"], 175);
        assert_eq!(WindowState::read(&json).unwrap().0.ui_scale, 175);
        json["uiScale"] = 130.into();
        assert_eq!(WindowState::read(&json).unwrap().0.ui_scale, 125);
        json.as_object_mut().unwrap().remove("uiScale");
        let (got, lost) = WindowState::read(&json).unwrap();
        assert_eq!((got.ui_scale, lost.any()), (100, false));
        json["uiScale"] = "big".into();
        let (got, lost) = WindowState::read(&json).unwrap();
        assert_eq!((got.ui_scale, lost.settings), (100, 1));
    }

    /// The sidebar dragged: a whole width within its bounds, kept with the
    /// list; one past them is held at the bound, and nothing is not a width.
    #[test]
    fn a_dragged_sidebar_width_is_held_to_its_bounds_and_kept() {
        let mut s = with(&["a"]);
        assert!(s.set_sidebar_width(361.4));
        assert_eq!(s.sidebar.width, 361.);
        assert!(!s.set_sidebar_width(360.6), "the same whole width");
        assert!(s.set_sidebar_width(10.));
        assert_eq!(s.sidebar.width, SIDEBAR_MIN);
        assert!(s.set_sidebar_width(9000.));
        assert_eq!(s.sidebar.width, SIDEBAR_MAX);
        assert!(!s.set_sidebar_width(f32::NAN));
        assert_eq!(s.sidebar.width, SIDEBAR_MAX);
        s.set_sidebar_width(333.);
        let json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        assert_eq!(WindowState::read(&json).unwrap().0.sidebar, Sidebar { pinned: true, width: 333. });
    }

    /// A file that parses but does not fit costs only what does not fit:
    /// another version, a CLI or a status this build does not know, a folder
    /// with no path, boards not five, a setting of the wrong type.
    #[test]
    fn a_kept_file_that_does_not_fit_loses_only_what_does_not_read() {
        let s = with(&["a", "b", "c"]);
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        json["version"] = 7.into();
        let chats = &mut json["folders"][0]["chats"];
        chats[0]["status"] = "thinking".into();
        chats[0]["nameSource"] = "telepathy".into();
        chats[1]["provider"] = "gemini".into();
        json["folders"].as_array_mut().unwrap().push(serde_json::json!({ "id": "f9", "open": true, "chats": [{ "id": "x" }] }));
        json["boards"].as_array_mut().unwrap().truncate(3);
        json["sidebar"] = "wide".into();
        let (got, lost) = WindowState::read(&json).unwrap();
        assert_eq!(got.chats().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        let a = got.find("a").unwrap();
        assert_eq!((a.status, a.name_source), (Status::Resting, None), "unknown values read as none");
        assert_eq!(got.boards.len(), BOARDS);
        assert_eq!(got.sidebar, Sidebar::default());
        assert_eq!(lost, Lost { version: Some(7), chats: 2, folders: 1, boards: true, settings: 1, unknown: 2, twice: 0 });
        assert!(lost.any());
        let words = lost.words();
        assert!(words.contains("version 7") && words.contains("2 chats") && words.contains("1 folder "), "{words}");
        assert!(words.contains("2 values this Luna does not know"), "{words}");
        // Boards past five are let go of; a board with no layout is blank.
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        let first = json["boards"][0].clone();
        json["boards"].as_array_mut().unwrap().push(first);
        json["boards"][1] = serde_json::json!({ "slots": 3 });
        let (got, lost) = WindowState::read(&json).unwrap();
        assert!(lost.boards);
        assert_eq!(got.boards[1], Board::blank(1));
        assert_eq!(got.chats().count(), 3);
    }

    /// What reading holds to what the window shows and costs the file
    /// something counts as lost, for the host to keep the file as it was: a
    /// status or a name's source this build does not know, a chat listed
    /// twice. One of the current version that reads whole loses nothing.
    #[test]
    fn values_let_go_of_and_chats_listed_twice_count_as_lost() {
        let s = with(&["a", "b"]);
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        json["folders"][0]["chats"][1]["status"] = "thinking".into();
        let (_, lost) = WindowState::read(&json).unwrap();
        assert_eq!(lost, Lost { unknown: 1, ..Lost::default() });
        assert!(lost.any(), "the host keeps a copy");
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        let mut again = json["folders"][0]["chats"][0].clone();
        again["name"] = "the same id, another name".into();
        json["folders"][0]["chats"].as_array_mut().unwrap().push(again);
        let (got, lost) = WindowState::read(&json).unwrap();
        assert_eq!(got.chats().count(), 2);
        assert_eq!(lost, Lost { twice: 1, ..Lost::default() });
        assert!(lost.words().contains("1 chat listed more than once"), "{}", lost.words());
        // A value of null is none, as it was written.
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        json["folders"][0]["chats"][0]["nameSource"] = serde_json::Value::Null;
        assert!(!WindowState::read(&json).unwrap().1.any());
    }

    /// The dividers are kept per layout of each board and held to the bounds
    /// as they are set and as they are read; a file from before them, or one
    /// whose dividers are no numbers, reads them even and loses nothing.
    #[test]
    fn dividers_are_kept_per_layout_and_held_to_the_bounds() {
        let mut s = with(&["a"]);
        s.set_layout(3);
        s.set_split(0, 3, Split::RowR, 0.7);
        s.set_split(0, 3, Split::Col, 0.01);
        s.set_split(0, 1, Split::Col, 0.7);
        s.set_split(BOARDS, 3, Split::Col, 0.7);
        assert_eq!(s.current().splits(), Splits { col: SPLIT_MIN, row_l: 0.5, row_r: 0.7 });
        s.set_layout(2);
        assert_eq!(s.current().splits(), Splits::default(), "another layout, its own");
        s.set_layout(1);
        assert_eq!(s.current().splits(), Splits::default(), "one pane has none");
        s.set_layout(3);
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        let (got, lost) = WindowState::read(&json).unwrap();
        assert_eq!((got.boards.clone(), lost.any()), (s.boards.clone(), false));
        json["boards"][0]["splits"][1] = serde_json::json!({ "col": "wide", "rowR": 7 });
        let (got, lost) = WindowState::read(&json).unwrap();
        assert_eq!(got.boards[0].splits[1], Splits { col: 0.5, row_l: 0.5, row_r: SPLIT_MAX });
        assert!(!lost.any());
        json["boards"][0].as_object_mut().unwrap().remove("splits");
        assert_eq!(WindowState::read(&json).unwrap().0.boards[0].splits, [Splits::default(); 3]);
        s.clear_board(0);
        assert_eq!(s.current().splits(), Splits::default(), "Clear board evens them out");
    }

    /// An import that failed and left the list empty is tried again; one the
    /// person has since filled is not.
    /// A list a build before this one wrote, as it wrote it: no marker.
    fn as_written_before(s: &WindowState) -> WindowState {
        let mut json: serde_json::Value = serde_json::from_str(&s.to_text()).unwrap();
        json.as_object_mut().unwrap().remove("codexBypassMoved");
        let (read, lost) = WindowState::read(&json).unwrap();
        assert!(!lost.any(), "no marker is nothing lost");
        read
    }

    fn codex(id: &str, approval: &str, sandbox: &str) -> Chat {
        Chat { provider: Provider::Codex, approval: Some(approval.into()), sandbox: Some(sandbox.into()), ..chat(id, 0) }
    }

    fn modes(s: &WindowState, id: &str) -> (Option<String>, Option<String>) {
        let c = s.find(id).unwrap();
        (c.approval.clone(), c.sandbox.clone())
    }

    /// The Codex chats 0.20.0 saved as never + danger-full-access move to
    /// Codex's own once, each named for the log; any other pair, and a
    /// Claude Code chat in Bypass, stay as they are.
    #[test]
    fn codex_chats_saved_in_the_full_bypass_move_to_codexs_own() {
        let mut s = WindowState::default();
        for c in [
            codex("bypass", "never", "danger-full-access"),
            codex("asks", "on-request", "danger-full-access"),
            codex("never", "never", "workspace-write"),
            codex("closed", "on-request", "read-only"),
            codex("own", "codex", "codex"),
            Chat { perm: Some("Bypass".into()), ..chat("claude", 0) },
        ] {
            s.add_chat("/repo", c, true, || "f1".into());
        }
        let mut old = as_written_before(&s);
        assert!(!old.codex_bypass_moved);
        let before = old.clone();
        assert_eq!(old.move_codex_bypass(), Some(vec![("bypass".to_string(), "chat bypass".to_string())]));
        assert_eq!(modes(&old, "bypass"), (Some("codex".into()), Some("codex".into())));
        for id in ["asks", "never", "closed", "own", "claude"] {
            assert_eq!(old.find(id), before.find(id), "{id} is as it was");
        }
        assert!(old.codex_bypass_moved);
        let line = WindowState::codex_bypass_moved_line("bypass", "chat bypass");
        assert!(line.contains("bypass") && line.contains("\"chat bypass\"") && line.contains("Codex's own"), "{line}");
        assert!(line.contains("next start") && line.contains("still running keeps the full bypass"), "{line}");
    }

    /// Once: a second run moves nothing, and a Bypass chosen again after the
    /// move stays through a write and a read.
    #[test]
    fn the_move_is_made_once() {
        let mut s = WindowState::default();
        s.add_chat("/repo", codex("bypass", "never", "danger-full-access"), true, || "f1".into());
        let mut old = as_written_before(&s);
        assert!(old.move_codex_bypass().is_some());
        assert_eq!(old.move_codex_bypass(), None);
        let c = old.find_mut("bypass").unwrap();
        (c.approval, c.sandbox) = (Some("never".into()), Some("danger-full-access".into()));
        let (mut again, _) = WindowState::read(&serde_json::from_str(&old.to_text()).unwrap()).unwrap();
        assert!(again.codex_bypass_moved, "the marker is kept");
        assert_eq!(again.move_codex_bypass(), None);
        assert_eq!(modes(&again, "bypass"), (Some("never".into()), Some("danger-full-access".into())));
        // A list this build begins has nothing of 0.20.0's to move.
        let mut new = WindowState::default();
        new.add_chat("/repo", codex("picked", "never", "danger-full-access"), true, || "f1".into());
        assert_eq!(new.move_codex_bypass(), None);
    }

    /// Windows folds either slash and case; Linux and macOS fold only a
    /// trailing separator, so `/work/Repo` and `/work/repo` stay two folders.
    #[test]
    fn paths_fold_only_as_the_system_does() {
        assert!(same_path_on("C:\\Repo\\", "c:/repo", true));
        assert!(same_path_on("C:/Work/Repo/", "c:\\work\\repo", true));
        assert!(!same_path_on("C:\\repo", "C:\\repo2", true));
        assert!(same_path_on("/home/a/Work/", "/home/a/Work", false));
        assert!(!same_path_on("/work/Repo", "/work/repo", false), "two folders off Windows");
        assert!(!same_path_on("/work/a\\b", "/work/a/b", false), "a backslash is a name's off Windows");
        assert_eq!(same_path("/work/Repo", "/work/repo"), cfg!(windows));
    }

    #[test]
    fn a_failed_import_is_tried_again_only_while_the_list_is_empty() {
        let mut s = WindowState { imported: Some(Imported { ok: false, ..Imported::default() }), ..WindowState::default() };
        assert!(s.import_retryable());
        s.add_chat("/repo", chat("a", 0), true, || "f1".into());
        assert!(!s.import_retryable());
        assert!(!WindowState::default().import_retryable());
        let older = WindowState { imported: Some(Imported { ok: false, older: true, ..Imported::default() }), ..WindowState::default() };
        assert!(!older.import_retryable(), "a list too old to read is not tried again");
    }

    #[test]
    fn model_and_effort_read_as_the_title_bar_says_them() {
        let mut c = chat("a", 0);
        assert_eq!(c.model_effort(), "Opus · high");
        c.provider = Provider::Codex;
        c.model = None;
        assert_eq!(c.model_effort(), "Codex default · high");
        c.model_seen = Some("gpt-6".into());
        assert_eq!(c.model_effort(), "gpt-6 · high");
    }
}
