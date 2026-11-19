//! The in-app windows over the main one: the library, settings and the
//! accounts (library/, settings/, accounts/). One is up at a time, over a
//! scrim that keeps the window below from being clicked, the way the page's
//! dialogs sit over it; Esc or its close button closes it. Each asks the host
//! for what the page's dialog asks for, by the same commands (`cmd`), and
//! shows long work by its state in words: nothing spins.
//!
//! Who opens them: the demo scenes and, in the test build, its driver
//! (drive.rs); the title bar and the status bar once the board has them.

pub(crate) mod ui;

#[cfg(any(test, feature = "shots"))]
pub mod demo;
#[cfg(test)]
mod tests;

use crate::accounts::AccountsPanel;
use crate::host::Host;
use crate::library::LibraryPanel;
use crate::models::Models;
use crate::settings::SettingsPanel;
use gpui::{
    AnyView, App, AppContext, Context, Entity, FocusHandle, InteractiveElement, IntoElement, KeyBinding, ParentElement, Render, Styled,
    Window, actions, div, px,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;

/// The commands the panels name, beside `cmd::ALL` and the desktop's (a test
/// in luna_lib holds its dispatch to these too). Their arguments and answers
/// are the page's (`src/ipc/library.ts`, `accounts.ts`, `cli.ts`, `trust.ts`,
/// `agents.ts`, `accountSettings.ts`), but for `window_prefs`,
/// `set_window_prefs`, `app_version`, `library_rename_skill` and `set_helpers_above_caller`'s
/// `confirmed`, which are the native window's own.
pub mod cmd {
    pub const LIBRARY_OVERVIEW: &str = "library_overview";
    pub const LIBRARY_SYNC: &str = "library_sync";
    pub const LIBRARY_SET_SOURCE: &str = "library_set_source";
    pub const LIBRARY_SET_INSTRUCTIONS: &str = "library_set_instructions";
    pub const LIBRARY_SET_ACCOUNTS: &str = "library_set_accounts";
    pub const LIBRARY_SET_AUDIENCE: &str = "library_set_audience";
    pub const LIBRARY_SET_FOLDER: &str = "library_set_folder";
    pub const LIBRARY_SET_FOLDER_SKILL: &str = "library_set_folder_skill";
    pub const LIBRARY_SET_FOLDER_INSTRUCTIONS: &str = "library_set_folder_instructions";
    pub const LIBRARY_READ: &str = "library_read";
    pub const LIBRARY_WRITE: &str = "library_write";
    pub const LIBRARY_CHECK_SKILL: &str = "library_check_skill";
    pub const LIBRARY_CHECK_SYSTEM: &str = "library_check_system";
    pub const LIBRARY_SYSTEM_COMPARE: &str = "library_system_compare";
    pub const LIBRARY_SYSTEM_RESET: &str = "library_system_reset";
    pub const LIBRARY_SYSTEM_ADOPT: &str = "library_system_adopt";
    pub const LIBRARY_NEW_SKILL: &str = "library_new_skill";
    pub const LIBRARY_RENAME_SKILL: &str = "library_rename_skill";
    pub const LIBRARY_DELETE_SKILL: &str = "library_delete_skill";
    pub const LIBRARY_NEW_INSTRUCTIONS: &str = "library_new_instructions";
    pub const LIBRARY_RENAME_INSTRUCTIONS: &str = "library_rename_instructions";
    pub const LIBRARY_DELETE_INSTRUCTIONS: &str = "library_delete_instructions";
    pub const LIBRARY_OPEN: &str = "library_open";

    /// `{}`: `Prefs`, the window's own preferences.
    pub const WINDOW_PREFS: &str = "window_prefs";
    /// `{ prefs }`: keeps them. Sent, so changes reach the host in order.
    pub const SET_WINDOW_PREFS: &str = "set_window_prefs";
    /// `{}`: Luna's version, the app's (this crate has its own).
    pub const APP_VERSION: &str = "app_version";
    pub const GET_ACCOUNTS_ROOT: &str = "get_accounts_root";
    pub const SET_ACCOUNTS_ROOT: &str = "set_accounts_root";
    pub const HELPERS_ABOVE_CALLER: &str = "helpers_above_caller";
    /// `{ on, confirmed }`: on only with the window's own yes.
    pub const SET_HELPERS_ABOVE_CALLER: &str = "set_helpers_above_caller";
    pub const HELPER_SETTINGS: &str = "helper_settings";
    pub const SET_MAX_HELPERS: &str = "set_max_helpers";
    pub const SET_HELPER_MODE: &str = "set_helper_mode";
    pub const SET_HELPER_CONTEXT_PCT: &str = "set_helper_context_pct";
    pub const CODEX_TRUSTED_ROOTS: &str = "codex_trusted_roots";
    pub const SET_CODEX_TRUSTED_ROOTS: &str = "set_codex_trusted_roots";
    pub const CLI_STATUS: &str = "cli_status";
    pub const CLI_VERSIONS: &str = "cli_versions";
    pub const CLI_UPDATE_NOW: &str = "cli_update_now";

    pub const LIST_ACCOUNTS: &str = "list_accounts";
    pub const CREATE_ACCOUNT: &str = "create_account";
    pub const RENAME_ACCOUNT: &str = "rename_account";
    pub const DELETE_ACCOUNT: &str = "delete_account";
    pub const ACCOUNT_LIMITS: &str = "account_limits";
    pub const CODEX_LIMITS: &str = "codex_limits";
    pub const SETTINGS_SYNC_STATE: &str = "settings_sync_state";
    pub const SET_SETTINGS_PARENT: &str = "set_settings_parent";
    pub const SET_SETTINGS_SYNC_ON_START: &str = "set_settings_sync_on_start";
    pub const SYNC_SETTINGS_NOW: &str = "sync_settings_now";

    pub const CALLED: &[&str] = &[
        LIBRARY_OVERVIEW,
        LIBRARY_SYNC,
        LIBRARY_SET_SOURCE,
        LIBRARY_SET_INSTRUCTIONS,
        LIBRARY_SET_ACCOUNTS,
        LIBRARY_SET_AUDIENCE,
        LIBRARY_SET_FOLDER,
        LIBRARY_SET_FOLDER_SKILL,
        LIBRARY_SET_FOLDER_INSTRUCTIONS,
        LIBRARY_READ,
        LIBRARY_WRITE,
        LIBRARY_CHECK_SKILL,
        LIBRARY_CHECK_SYSTEM,
        LIBRARY_SYSTEM_COMPARE,
        LIBRARY_SYSTEM_RESET,
        LIBRARY_SYSTEM_ADOPT,
        LIBRARY_NEW_SKILL,
        LIBRARY_RENAME_SKILL,
        LIBRARY_DELETE_SKILL,
        LIBRARY_NEW_INSTRUCTIONS,
        LIBRARY_RENAME_INSTRUCTIONS,
        LIBRARY_DELETE_INSTRUCTIONS,
        LIBRARY_OPEN,
        WINDOW_PREFS,
        APP_VERSION,
        GET_ACCOUNTS_ROOT,
        SET_ACCOUNTS_ROOT,
        HELPERS_ABOVE_CALLER,
        SET_HELPERS_ABOVE_CALLER,
        HELPER_SETTINGS,
        SET_MAX_HELPERS,
        SET_HELPER_MODE,
        SET_HELPER_CONTEXT_PCT,
        CODEX_TRUSTED_ROOTS,
        SET_CODEX_TRUSTED_ROOTS,
        CLI_STATUS,
        CLI_VERSIONS,
        CLI_UPDATE_NOW,
        LIST_ACCOUNTS,
        CREATE_ACCOUNT,
        RENAME_ACCOUNT,
        DELETE_ACCOUNT,
        ACCOUNT_LIMITS,
        CODEX_LIMITS,
        SETTINGS_SYNC_STATE,
        SET_SETTINGS_PARENT,
        SET_SETTINGS_SYNC_ON_START,
        SYNC_SETTINGS_NOW,
    ];
    pub const SENT: &[&str] = &[SET_WINDOW_PREFS];
}

/// The panels' events the inbox takes, beside `event::ALL`.
pub mod event {
    /// A CLI's update pass moved: its `CliStatus` (cli.rs).
    pub const CLI_STATUS: &str = "cli://status";

    pub const ALL: &[&str] = &[CLI_STATUS];
}

/// One of the panels' events, to the open panel it is about.
pub(crate) fn on_event(name: &str, payload: &Value, cx: &mut App) {
    if name == event::CLI_STATUS
        && let Some(layer) = cx.try_global::<Layer>().and_then(|l| l.0.upgrade())
    {
        let payload = payload.clone();
        layer.update(cx, |layer, cx| {
            if let Some(Open::Settings(s)) = &layer.open {
                s.update(cx, |s, cx| s.cli_status(payload, cx));
            }
        });
    }
}

/// What the native window keeps of its own beside its theme (native/panels.rs
/// `WindowPrefs`): what the page keeps in its local storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    /// Desktop notifications play the system's sound.
    pub sound: bool,
    pub new_chat: NewChatStart,
}

/// How the new-chat dialog's boxes start in a folder with no start of its own.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NewChatStart {
    pub worktree: bool,
    pub tools: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { sound: true, new_chat: NewChatStart::default() }
    }
}

impl Default for NewChatStart {
    fn default() -> Self {
        NewChatStart { worktree: true, tools: false }
    }
}

/// The CLIs, by the names the host knows them by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Claude,
    Codex,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Claude, Provider::Codex];

    pub fn key(self) -> &'static str {
        match self {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude Code",
            Provider::Codex => "Codex",
        }
    }
}

/// A command's answer, read as `T`, off the window's thread.
pub(crate) fn ask<T: DeserializeOwned + 'static>(
    host: &Arc<dyn Host>,
    cmd: &str,
    args: Value,
) -> impl Future<Output = Result<T, String>> + 'static {
    let answer = host.call(cmd, args);
    let cmd = cmd.to_owned();
    async move {
        let value = answer.await.inspect_err(|e| log::warn!("{cmd}: {e}"))?;
        serde_json::from_value(value).map_err(|e| {
            log::warn!("{cmd}: {e}");
            format!("{cmd} answered what the window cannot read: {e}")
        })
    }
}

/// Now, in milliseconds since the epoch.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A moment as the clock on the wall says it: the time alone today, the day
/// with it on another. Said once, as the panel draws, it stays true while
/// the panel is up: nothing counts down.
fn wall(t: chrono::DateTime<chrono::Local>) -> String {
    if t.date_naive() == chrono::Local::now().date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%a %-d %b %H:%M").to_string()
    }
}

/// `ms` since the epoch, on the wall clock.
pub(crate) fn local_time(ms: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_millis_opt(ms).earliest().map(wall).unwrap_or_else(|| "-".into())
}

/// An RFC 3339 instant (a limit's reset), on the wall clock.
pub(crate) fn local_reset(at: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(at).ok().map(|t| wall(t.with_timezone(&chrono::Local)))
}

/// Which panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Library,
    Settings,
    Accounts,
}

impl Panel {
    pub fn name(self) -> &'static str {
        match self {
            Panel::Library => "library",
            Panel::Settings => "settings",
            Panel::Accounts => "accounts",
        }
    }

    pub fn from_name(name: &str) -> Option<Panel> {
        [Panel::Library, Panel::Settings, Panel::Accounts].into_iter().find(|p| p.name() == name)
    }
}

/// The panel up, its view.
#[derive(Clone)]
pub(crate) enum Open {
    Library(Entity<LibraryPanel>),
    Settings(Entity<SettingsPanel>),
    Accounts(Entity<AccountsPanel>),
}

impl Open {
    fn view(&self) -> AnyView {
        match self {
            Open::Library(v) => v.clone().into(),
            Open::Settings(v) => v.clone().into(),
            Open::Accounts(v) => v.clone().into(),
        }
    }

    pub(crate) fn panel(&self) -> Panel {
        match self {
            Open::Library(_) => Panel::Library,
            Open::Settings(_) => Panel::Settings,
            Open::Accounts(_) => Panel::Accounts,
        }
    }
}

actions!(luna_panel, [Close, Save]);

/// The key context every panel's frame sets.
pub(crate) const CONTEXT: &str = "LunaPanel";

/// Esc closes the panel up (a panel with a question open, or an editor with
/// unsaved text, takes it first); Ctrl+S (Cmd+S on macOS) saves the text
/// in the library's editor.
pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Close, Some(CONTEXT)),
        KeyBinding::new(if cfg!(target_os = "macos") { "cmd-s" } else { "ctrl-s" }, Save, Some(CONTEXT)),
    ]);
}

/// The layer the panels sit on, over the whole window (root.rs): nothing at
/// all while none is up.
pub struct PanelLayer {
    models: Models,
    pub(crate) open: Option<Open>,
    focus: FocusHandle,
}

/// The window's layer, for whoever opens a panel from outside its views.
struct Layer(gpui::WeakEntity<PanelLayer>);

impl gpui::Global for Layer {}

impl PanelLayer {
    pub fn new(models: Models, cx: &mut Context<Self>) -> PanelLayer {
        cx.observe(&models.shell, |_, _, cx| cx.notify()).detach();
        let me = cx.weak_entity();
        cx.set_global(Layer(me));
        PanelLayer { models, open: None, focus: cx.focus_handle() }
    }

    pub fn shown(&self) -> Option<Panel> {
        self.open.as_ref().map(Open::panel)
    }

    /// Brings `panel` up in place of whichever was; the same one stays as it
    /// is.
    pub fn open(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        if self.shown() == Some(panel) {
            return;
        }
        let models = self.models.clone();
        let layer = cx.weak_entity();
        self.open = Some(match panel {
            Panel::Library => Open::Library(cx.new(|cx| LibraryPanel::new(models, layer, window, cx))),
            Panel::Settings => Open::Settings(cx.new(|cx| SettingsPanel::new(models, layer, window, cx))),
            Panel::Accounts => Open::Accounts(cx.new(|cx| AccountsPanel::new(models, layer, window, cx))),
        });
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Takes the panel down, at once: what asks first has asked already.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.notify();
        }
    }

    /// Esc, or a click on the scrim: the panel says whether it may go
    /// (a question open, text not saved).
    fn ask_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let go = match &self.open {
            Some(Open::Library(v)) => v.update(cx, |v, cx| v.may_close(window, cx)),
            Some(Open::Settings(v)) => v.update(cx, |v, cx| v.may_close(cx)),
            Some(Open::Accounts(v)) => v.update(cx, |v, cx| v.may_close(cx)),
            None => false,
        };
        if go {
            self.close(cx);
        }
    }
}

/// Opens `panel` in the window whose layer this is: for the driver and the
/// shots, and the title bar and status bar later.
pub fn open(panel: Panel, window: &mut Window, cx: &mut App) {
    if let Some(layer) = cx.try_global::<Layer>().and_then(|l| l.0.upgrade()) {
        layer.update(cx, |l, cx| l.open(panel, window, cx));
    }
}

impl Render for PanelLayer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = &self.open else { return div().id("panels") };
        let p = self.models.shell.read(cx).palette();
        div()
            .id("panels")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Close, window, cx| this.ask_close(window, cx)))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.))
            // A dither would be the period's; a flat tint is what GPUI draws
            // without a texture, and it stays still.
            .bg(p.frame.opacity(0.45))
            // Nothing under it takes a click or a hover while a panel is up.
            .occlude()
            .child(open.view())
    }
}

/// The driver's lines for the panels (drive.rs, and the shots' scenes):
/// `panel library|settings|accounts|none`, `select instructions|skill|
/// system|folder <name>` (the editor focused on it), `new instructions|skill
/// <name>`, `save`, `menu <id>`,
/// `compare <system text>`, `page <key>`, `setting sound on|off`,
/// `cli-check claude|codex`, `update-check`, `update-confirm`, `add-account
/// claude|codex <name>`, `delete <n>` (the nth account), each doing what its
/// click does. True when the line
/// was one of these and its panel is up.
#[cfg(any(test, luna_test, feature = "shots"))]
pub(crate) fn drive(verb: &str, rest: &str, window: &mut Window, cx: &mut App) -> bool {
    use crate::library::Sel;
    let Some(layer) = cx.try_global::<Layer>().and_then(|l| l.0.upgrade()) else { return false };
    if verb == "panel" {
        match Panel::from_name(rest) {
            Some(panel) => layer.update(cx, |l, cx| l.open(panel, window, cx)),
            None => layer.update(cx, |l, cx| l.close(cx)),
        }
        return true;
    }
    let open = layer.read(cx).open.clone();
    match (verb, &open) {
        ("select", Some(Open::Library(v))) => {
            let (kind, name) = rest.split_once(' ').unwrap_or((rest, ""));
            let sel = match kind {
                "instructions" => Sel::Instructions(name.to_owned()),
                "skill" => Sel::Skill(name.to_owned()),
                "system" => Sel::System(name.to_owned()),
                "folder" => Sel::Folder(name.to_owned()),
                _ => return false,
            };
            v.clone().update(cx, |v, cx| {
                v.select(sel, window, cx);
                if let Some(e) = v.editor.clone() {
                    let state = e.read(cx).state.clone();
                    state.update(cx, |s, cx| s.focus(window, cx));
                }
            });
            true
        }
        ("new", Some(Open::Library(v))) => {
            let (kind, name) = rest.split_once(' ').unwrap_or((rest, ""));
            let skill = match kind {
                "skill" => true,
                "instructions" => false,
                _ => return false,
            };
            v.clone().update(cx, |v, cx| {
                let field = if skill { v.new_skill.clone() } else { v.new_instructions.clone() };
                field.update(cx, |f, cx| f.set_value(name.to_owned(), window, cx));
                v.create(skill, window, cx);
            });
            true
        }
        ("save", Some(Open::Library(v))) => {
            if let Some(e) = v.read(cx).editor.clone() {
                e.update(cx, |e, cx| e.save(cx));
            }
            true
        }
        ("page", Some(Open::Settings(v))) => match crate::settings::Page::ALL.into_iter().find(|p| p.key() == rest) {
            Some(page) => {
                v.clone().update(cx, |v, cx| v.show_page(page, cx));
                true
            }
            None => false,
        },
        ("setting", Some(Open::Settings(v))) => match rest.split_once(' ') {
            Some(("sound", on)) => {
                let on = on == "on";
                v.clone().update(cx, |v, cx| v.set_prefs(|p| p.sound = on, cx));
                true
            }
            _ => false,
        },
        ("menu", Some(Open::Library(v))) => {
            v.clone().update(cx, |v, cx| v.toggle_menu(rest, cx));
            true
        }
        ("compare", Some(Open::Library(v))) => {
            v.clone().update(cx, |v, cx| v.compare_with_luna(rest.to_owned(), cx));
            true
        }
        ("update-check", Some(Open::Settings(v))) => {
            v.clone().update(cx, |v, cx| v.check_update(cx));
            true
        }
        ("update-confirm", Some(Open::Settings(v))) => {
            v.clone().update(cx, |v, cx| v.ask_install(cx));
            true
        }
        ("add-account", Some(Open::Accounts(v))) => {
            let (provider, name) = rest.split_once(' ').unwrap_or((rest, ""));
            let Some(provider) = Provider::ALL.into_iter().find(|p| p.key() == provider) else { return false };
            v.clone().update(cx, |v, cx| {
                v.adding = true;
                v.add_provider = provider;
                v.add_field_value(name, window, cx);
                v.create(window, cx);
            });
            true
        }
        ("delete", Some(Open::Accounts(v))) => match rest.parse::<usize>() {
            Ok(i) => {
                v.clone().update(cx, |v, cx| v.ask_delete(i, cx));
                true
            }
            Err(_) => false,
        },
        ("cli-check", Some(Open::Settings(v))) => match [Provider::Claude, Provider::Codex].into_iter().find(|p| p.key() == rest) {
            Some(provider) => {
                v.clone().update(cx, |v, cx| v.check_cli(provider, cx));
                true
            }
            None => false,
        },
        _ => false,
    }
}
