//! A host made of fixtures, for the tests and the screenshot harness
//! (src/bin/shots.rs): it answers from a table, keeps every call and send it
//! got, and its mailbox is the test's or the scene's to fill. Compiled only
//! for those two, never into the app.

use crate::dto::{Core, CoreState, LinkState, ThemePref};
use crate::host::{Answer, Host, Inbox, Mailbox, OutputSink, cmd, mailbox};
use crate::term::SessionSpec;
use crate::term::paste::Provider;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type Responder = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

#[derive(Default)]
pub struct FakeHost {
    answers: Mutex<HashMap<String, Value>>,
    failures: Mutex<HashMap<String, String>>,
    calls: Mutex<Vec<(String, Value)>>,
    sends: Mutex<Vec<(String, Value)>>,
    outputs: Mutex<HashMap<String, Arc<dyn OutputSink>>>,
    /// Sends of a command it refuses yet, by name (`refuse`).
    refusals: Mutex<HashMap<String, usize>>,
    /// Answers worked out from a call's arguments (`respond`), asked before
    /// the fixed ones.
    responders: Mutex<HashMap<String, Responder>>,
    /// Calls of a command left unanswered until the test answers them
    /// (`defer`, `resolve`), by name, oldest first.
    deferred: Mutex<HashMap<String, Vec<futures::channel::oneshot::Sender<Result<Value, String>>>>>,
    /// Its desktop (desktop.rs `FakeDesktop`).
    pub desktop: crate::desktop::FakeDesktop,
}

impl FakeHost {
    pub fn new() -> Arc<FakeHost> {
        Arc::new(FakeHost::default())
    }

    /// What `cmd` answers from now on; null when nothing was set.
    pub fn answer(&self, cmd: &str, value: Value) {
        self.answers.lock().unwrap().insert(cmd.to_owned(), value);
    }

    /// What `cmd` answers from now on, worked out from the arguments of each
    /// call.
    pub fn respond(&self, cmd: &str, f: impl Fn(&Value) -> Value + Send + Sync + 'static) {
        self.responders.lock().unwrap().insert(cmd.to_owned(), Arc::new(f));
    }

    /// `cmd` fails with `why` from now on; None answers it again.
    pub fn fail(&self, cmd: &str, why: Option<&str>) {
        let mut failures = self.failures.lock().unwrap();
        match why {
            Some(why) => failures.insert(cmd.to_owned(), why.to_owned()),
            None => failures.remove(cmd),
        };
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<(String, Value)> {
        self.calls.lock().unwrap().clone()
    }

    /// The calls of `cmd` so far, their arguments.
    pub fn calls_of(&self, cmd: &str) -> Vec<Value> {
        self.calls().into_iter().filter(|(c, _)| c == cmd).map(|(_, a)| a).collect()
    }

    pub fn sends(&self) -> Vec<(String, Value)> {
        self.sends.lock().unwrap().clone()
    }

    /// The sends of `cmd` so far, their arguments.
    pub fn sends_of(&self, cmd: &str) -> Vec<Value> {
        self.sends().into_iter().filter(|(c, _)| c == cmd).map(|(_, a)| a).collect()
    }

    /// Refuses the next `times` sends of `cmd`, as a host whose line to the
    /// core is full for a moment; none of them is kept.
    pub fn refuse(&self, cmd: &str, times: usize) {
        self.refusals.lock().unwrap().insert(cmd.to_owned(), times);
    }

    /// Leaves the calls of `cmd` from now on unanswered until `resolve`.
    pub fn defer(&self, cmd: &str) {
        self.deferred.lock().unwrap().insert(cmd.to_owned(), Vec::new());
    }

    /// Answers the oldest call of `cmd` still unanswered: false when there
    /// is none, or its caller has stopped waiting.
    pub fn resolve(&self, cmd: &str, answer: Result<Value, String>) -> bool {
        let waiting = self.deferred.lock().unwrap().get_mut(cmd).filter(|w| !w.is_empty()).map(|w| w.remove(0));
        waiting.is_some_and(|tx| tx.send(answer).is_ok())
    }

    /// Answers the newest call of `cmd` still unanswered, ahead of older
    /// ones: false when there is none, or its caller has stopped waiting.
    pub fn resolve_newest(&self, cmd: &str, answer: Result<Value, String>) -> bool {
        let waiting = self.deferred.lock().unwrap().get_mut(cmd).and_then(Vec::pop);
        waiting.is_some_and(|tx| tx.send(answer).is_ok())
    }

    /// Output for `session`, as a host's reader hands it over: false when no
    /// sink is routed or it is gone.
    pub fn output(&self, session: &str, data: &[u8]) -> bool {
        self.output_at(session, data, None)
    }

    /// `output`, as a core that numbers its output sends it: the text starts
    /// at `offset`.
    pub fn output_at(&self, session: &str, data: &[u8], offset: Option<u64>) -> bool {
        let sink = self.outputs.lock().unwrap().get(session).cloned();
        sink.is_some_and(|s| s.bytes(data, offset))
    }
}

impl Host for FakeHost {
    fn call(&self, cmd: &str, args: Value) -> Answer {
        self.calls.lock().unwrap().push((cmd.to_owned(), args.clone()));
        if let Some(why) = self.failures.lock().unwrap().get(cmd).cloned() {
            return Box::pin(async move { Err(why) });
        }
        if let Some(waiting) = self.deferred.lock().unwrap().get_mut(cmd) {
            let (tx, rx) = futures::channel::oneshot::channel();
            waiting.push(tx);
            return Box::pin(async move { rx.await.unwrap_or_else(|_| Err("no answer".into())) });
        }
        let responder = self.responders.lock().unwrap().get(cmd).cloned();
        let mut answer = match responder {
            Some(f) => f(&args),
            None => self.answers.lock().unwrap().get(cmd).cloned().unwrap_or(Value::Null),
        };
        if cmd == cmd::FLUSH_WINDOW_STATE && answer.is_null() {
            // As the real host: it wrote every state it was sent.
            let seq = self.sends_of(cmd::SAVE_WINDOW_STATE).iter().filter_map(|a| a["seq"].as_u64()).max().unwrap_or(0);
            answer = json!({ "seq": seq });
        }
        Box::pin(async move { Ok(answer) })
    }

    fn send(&self, cmd: &str, args: Value) -> Result<(), String> {
        if let Some(left) = self.refusals.lock().unwrap().get_mut(cmd).filter(|n| **n > 0) {
            *left -= 1;
            return Err(format!("{cmd} refused"));
        }
        self.sends.lock().unwrap().push((cmd.to_owned(), args));
        Ok(())
    }

    fn route_output(&self, session: &str, sink: Option<Arc<dyn OutputSink>>) {
        let mut outputs = self.outputs.lock().unwrap();
        match sink {
            Some(sink) => outputs.insert(session.to_owned(), sink),
            None => outputs.remove(session),
        };
    }
}

/// A screenshot's state of things: what the fake host answers, and what it
/// tells the window after.
/// Something done to the window's views by a scene (`Scene::window_act`).
pub type WindowAct = fn(&mut crate::root::LunaWindow, &mut gpui::Window, &mut gpui::Context<crate::root::LunaWindow>);

pub struct Scene {
    /// The file's name, without the theme or the extension.
    pub name: &'static str,
    pub theme: ThemePref,
    /// The core's state as the window first asks for it.
    pub core: CoreState,
    /// Told after that, in order, each newer than the first answer, which
    /// they come before and so drop (models.rs `CoreLink::take`): a core being
    /// replaced is told `up`, then `starting`.
    pub then: Vec<CoreState>,
    /// A terminal on the board, and what it shows.
    pub terminal: Option<TermDemo>,
    /// The host's `window_state` answer: what it kept of the window, or
    /// what the import found.
    pub kept: Value,
    /// A pane's menu open.
    pub menu: Option<crate::board::MenuAt>,
    /// A chat's row clicked once the window is up (the peek is not kept).
    pub click: Option<&'static str>,
    /// What the host answers beyond the defaults, by command.
    pub answers: Vec<(&'static str, Value)>,
    /// Commands the host answers with an error.
    pub failures: Vec<(&'static str, &'static str)>,
    /// Events told to the window once it is up, in order.
    pub events: Vec<(&'static str, Value)>,
    /// Something done to the status bar's model once the window is up.
    pub act: Option<crate::status::Act>,
    /// Something done to the window's own views once it is up: a menu of the
    /// title bar opened, a row's name being edited.
    pub window_act: Option<WindowAct>,
    /// Then the driver's lines, one after another (panels/mod.rs `drive`):
    /// the panel opened and what is done in it.
    pub drive: Vec<&'static str>,
    /// What a session prints once the driver's lines are done: its id, and
    /// the text (the sign-in terminal's CLI).
    pub output: Option<(&'static str, String)>,
}

/// What a scene's terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TermDemo {
    /// Truecolor, the 256 colors, the 16, every attribute, wide characters.
    Colors,
    /// Box drawing and block elements, as the CLIs draw their frames.
    Boxes,
    /// A session's text with a selection over it.
    Selection,
    /// An IME composition at the cursor.
    Ime,
    /// A session that exited.
    Exited,
    /// The core being found: the pane waits and says so.
    Waiting,
}

impl Scene {
    /// The host, and the inbox it feeds.
    pub fn host(&self) -> (Arc<FakeHost>, Mailbox, Inbox) {
        let host = FakeHost::new();
        crate::panels::demo::answer_all(&host);
        crate::dialogs::demo::answer_all(&host);
        if self.name == "dialog-new-chat" {
            host.answer(crate::dialogs::cmd::CLAUDE_DEFAULTS, crate::dialogs::demo::claude_luna_defaults());
        }
        if self.name == "dialog-new-chat-codex" {
            host.answer(crate::dialogs::cmd::CODEX_DEFAULTS, crate::dialogs::demo::codex_luna_defaults());
        }
        if self.name == "dialog-new-chat-untrusted" {
            host.answer(crate::dialogs::cmd::CLAUDE_FOLDER_TRUSTED, json!(false));
        }
        if self.name == "dialog-helpers-many" {
            host.answer(crate::dialogs::cmd::BACKGROUND_HELPERS, crate::dialogs::demo::many_helpers());
        }
        // A folder not in git: no worktree box.
        if self.name == "dialog-new-chat-no-git" {
            host.answer(crate::dialogs::cmd::CHAT_FOLDER_CHECK, json!({ "refusal": null, "inGit": false }));
        }
        host.answer(cmd::CORE_STATE, serde_json::to_value(&self.core).unwrap());
        host.answer(cmd::ENSURE_CLAUDE_SESSION, attached("", 1));
        host.answer(cmd::ENSURE_CODEX_SESSION, attached("", 1));
        host.answer(cmd::WINDOW_STATE, self.kept.clone());
        host.answer(cmd::LIST_ACCOUNTS, accounts());
        if !self.drive.is_empty() {
            // The panels' scenes list the panels' accounts.
            host.answer(cmd::LIST_ACCOUNTS, crate::panels::demo::accounts());
        }
        host.answer(cmd::SESSIONS_META, looks());
        // The core's power rule, idle, as a core that is up says it.
        host.answer(crate::status::cmd::POWER_STATE, serde_json::to_value(crate::status::dto::PowerState::default()).unwrap());
        for (command, answer) in &self.answers {
            host.answer(command, answer.clone());
        }
        for (command, why) in &self.failures {
            host.fail(command, Some(why));
        }
        let (mailbox, inbox) = mailbox();
        (host, mailbox, inbox)
    }

    /// Tells the models the scene's core states, one by one: through the
    /// inbox, the state events would come down to the last (`event::LATEST`).
    pub(crate) fn tell(&self, models: &crate::models::Models, cx: &mut gpui::App) {
        for link in &self.then {
            crate::models::on_event(models, crate::event::CORE_STATE, serde_json::to_value(link).unwrap(), cx);
        }
    }

    /// Then the scene's other events, once the window is up: told before the
    /// core's `up` was taken, a power state would be dropped, and the status
    /// bar's own first `cli_status` answers would replace a CLI's.
    pub(crate) fn tell_events(&self, models: &crate::models::Models, cx: &mut gpui::App) {
        for (name, payload) in &self.events {
            crate::models::on_event(models, name, payload.clone(), cx);
        }
    }
}

/// What a session's start answers: its backlog and its spawn's number.
pub fn attached(backlog: &str, generation: u64) -> Value {
    json!({ "backlog": backlog, "generation": generation })
}

/// The demo terminal's chat, a Claude Code one, and its session.
pub const DEMO_CHAT: &str = "demo-1";

/// `attached` from a core that numbers its output: the text offset the
/// backlog ends at, and the modes as of there.
pub fn attached_at(backlog: &str, generation: u64, end: u64, modes: &str) -> Value {
    json!({ "backlog": backlog, "generation": generation, "backlogEnd": end, "modes": modes })
}

/// The demo chat's session.
pub fn session() -> SessionSpec {
    SessionSpec { id: DEMO_CHAT.into(), provider: Provider::Claude, start: cmd::ENSURE_CLAUDE_SESSION, args: json!({ "id": DEMO_CHAT }) }
}

/// The demo's accounts, one of each CLI, as `list_accounts` answers.
pub fn accounts() -> Value {
    json!([
        { "provider": "claude", "name": "work", "path": "C:\\accounts\\anthropic\\work" },
        { "provider": "codex", "name": "personal", "path": "C:\\accounts\\openai\\personal" },
    ])
}

/// What `sessions_meta` answers for the fixture's chats: one waiting with a
/// draft and its tokens to auto-compaction known, one working with messages
/// held, one in the background, the rest not running.
pub fn looks() -> Value {
    let meta = |status: &str, context: f64, draft: bool, held: u64, title: &str| json!({ "status": status, "context": context, "draft": draft, "held": held, "title": title, "titleRenamed": false });
    let mut parser = meta("waiting", 0.62, true, 0, "Parser fix");
    parser["contextToCompact"] = 347_400.into();
    json!([
        { "id": "c-parser", "meta": parser },
        { "id": "c-codex", "meta": meta("busy", 0.31, false, 2, "Lexer tests") },
        { "id": "c-notes", "meta": meta("background", 0.12, false, 0, "Notes") },
        { "id": "c-site", "meta": meta("busy", 0.48, false, 0, "Landing page") },
        { "id": DEMO_CHAT, "meta": meta("busy", 0.05, false, 0, "What the parser thread does") },
    ])
}

/// The window's state from the synthetic 0.19 store, on board I in
/// `layout`, its panes filled in tile order, the sidebar pinned or not.
pub fn chats_state(layout: usize, pinned: bool) -> crate::state::WindowState {
    let (mut state, _) = crate::import::from_storage(&crate::import::fixture()).ok().flatten().expect("the fixture imports");
    if let Some(c) = state.find_mut("c-old") {
        c.board = 0;
    }
    state.board = 0;
    state.auto_tile = true;
    state.clear_board(0);
    state.set_layout(layout);
    for (i, id) in ["c-parser", "c-codex", "c-notes", "c-old"].iter().take(layout).enumerate() {
        state.put(i, id);
    }
    state.active = Some("c-parser".into());
    state.active_pane = 0;
    state.sidebar.pinned = pinned;
    // As a later run keeps it: the host took 0.19's choices then.
    state.prefs_to_import = None;
    state
}

/// The state a terminal scene keeps: the demo chat alone, in one pane.
pub fn term_state() -> crate::state::WindowState {
    let mut state = crate::state::WindowState::default();
    let chat = crate::state::Chat {
        id: DEMO_CHAT.into(),
        name: "What the parser thread does".into(),
        name_source: Some(crate::state::NameSource::Cli),
        status: crate::state::Status::Working,
        provider: Provider::Claude,
        account: "work".into(),
        board: 0,
        worktree: false,
        worktree_path: None,
        session_id: Some("5f0c9a7e-0000-4000-8000-00000000d3e0".into()),
        tools: false,
        model: Some("Opus".into()),
        effort: "high".into(),
        perm: Some("Ask".into()),
        approval: None,
        sandbox: None,
        default_mode: false,
        resolved_permission: None,
        read_only: false,
        agent: None,
        model_seen: None,
        adopt: None,
    };
    state.add_chat("C:\\work\\parser", chat, true, || "f-demo".into());
    state.put(0, DEMO_CHAT);
    state
}

/// A `window_state` answer holding `state`.
pub fn kept(state: &crate::state::WindowState) -> Value {
    json!({ "saved": serde_json::from_str::<Value>(&state.to_text()).expect("the state reads back") })
}

/// What Codex's login prints in the sign-in window.
pub fn login_output() -> String {
    let lines = [
        "\x1b[1mWelcome to Codex\x1b[0m",
        "",
        "  Sign in with ChatGPT to use it as part of your paid plan",
        "  or connect an API key for usage-based billing",
        "",
        "\x1b[36m> 1. Sign in with ChatGPT\x1b[0m",
        "  2. Provide your own API key",
        "",
        "  Press Enter to continue",
    ];
    lines.iter().map(|l| format!("{l}\r\n")).collect()
}

/// What the demo session prints for `demo`.
pub fn term_output(demo: TermDemo) -> String {
    const R: &str = "\x1b[0m";
    let mut s = String::new();
    match demo {
        TermDemo::Colors => {
            s += "\x1b[1mTruecolor\x1b[0m  ";
            for i in 0..48 {
                let t = i as f32 / 47.;
                let (r, g, b) = ((255. * (1. - t)) as u8, (255. * (1. - (2. * t - 1.).abs())) as u8, (255. * t) as u8);
                s += &format!("\x1b[48;2;{r};{g};{b}m ");
            }
            s += R;
            s += "\r\n\x1b[1m256 colors\x1b[0m ";
            for i in 16..232u32 {
                if (i - 16) % 36 == 0 && i > 16 {
                    s += "\r\n           ";
                }
                s += &format!("\x1b[48;5;{i}m ");
            }
            s += R;
            s += "\r\n           ";
            for i in 232..256u32 {
                s += &format!("\x1b[48;5;{i}m  ");
            }
            s += R;
            s += "\r\n\x1b[1m16 colors\x1b[0m  ";
            for i in 0..8 {
                s += &format!("\x1b[3{i}m fg{i} ");
            }
            s += "\r\n           ";
            for i in 0..8 {
                s += &format!("\x1b[9{i}m br{i} ");
            }
            s += R;
            s += "\r\n           ";
            for i in 0..8 {
                s += &format!("\x1b[4{i}m\x1b[3{}m bg{i} ", if i == 0 { 7 } else { 0 });
            }
            s += R;
            s += "\r\n\r\n\x1b[1mbold\x1b[0m \x1b[2mdim\x1b[0m \x1b[3mitalic\x1b[0m \x1b[1;3mbold italic\x1b[0m \x1b[4munderline\x1b[0m \x1b[4:2mdouble\x1b[0m ";
            s += "\x1b[4:3;58;2;255;64;64mcurly\x1b[0m \x1b[9mstrike\x1b[0m \x1b[7minverse\x1b[0m [\x1b[8mhidden\x1b[0m] \x1b[2;31mdim red\x1b[0m";
            s += "\r\n\r\nWide: 日本語のテキスト 한국어 中文字符  Emoji: ✅ 🚀 📦  Combining: e\u{301} a\u{308}"; // lang-check: allow
            s += "\r\n\x1b[38;2;217;119;87m✻\x1b[0m Thinking... \x1b[2m(esc to interrupt)\x1b[0m\r\n\r\n> ";
        }
        TermDemo::Boxes => {
            s += "\x1b[38;2;215;119;87m╭──────────────────────────────────────────────╮\x1b[0m\r\n";
            s += "\x1b[38;2;215;119;87m│\x1b[0m \x1b[1m✻ Welcome to the demo\x1b[0m                        \x1b[38;2;215;119;87m│\x1b[0m\r\n";
            s += "\x1b[38;2;215;119;87m│\x1b[0m   /help for help, /status for your setup     \x1b[38;2;215;119;87m│\x1b[0m\r\n";
            s += "\x1b[38;2;215;119;87m╰──────────────────────────────────────────────╯\x1b[0m\r\n\r\n";
            s += "┌─┬─┐ ┏━┳━┓ ╔═╦═╗ ╭─╮ ┄┄┄ ┅┅┅ ╱╲╳\r\n";
            s += "├─┼─┤ ┣━╋━┫ ╠═╬═╣ │ │ ┆ ┇  ╌╍ ╎╏\r\n";
            s += "└─┴─┘ ┗━┻━┛ ╚═╩═╝ ╰─╯ ╒═╕ ╓─╖ ┍━┑\r\n\r\n";
            s += "Blocks: █▉▊▋▌▍▎▏ ▁▂▃▄▅▆▇█ ░▒▓ ▖▗▘▙▚▛▜▝▞▟ ▀▐▔▕\r\n";
            s += "Meter:  \x1b[32m████████████\x1b[2m░░░░░░░░\x1b[0m 60%\r\n\r\n";
            s += "\x1b[2m─────────────────────────────────────────────────\x1b[0m\r\n";
            s += "> \r\n";
            s += "\x1b[2m─────────────────────────────────────────────────\x1b[0m\r\n";
            s += "  \x1b[2m? for shortcuts\x1b[0m\x1b[14;3H";
        }
        TermDemo::Selection | TermDemo::Ime | TermDemo::Exited | TermDemo::Waiting => {
            s += "\x1b[1m> \x1b[0mwhat does the parser thread do?\r\n\r\n";
            s += "\x1b[38;2;217;119;87m⏺\x1b[0m It parses the session's output into the pane's grid, off the\r\n";
            s += "  window's thread, and wakes the pane once it has: the window\r\n";
            s += "  draws only when there is something new to show.\r\n\r\n";
            s += "\x1b[2m─────────────────────────────────────────────────\x1b[0m\r\n> ";
        }
    }
    s
}

/// The demo core: this build, two sessions running.
pub fn core() -> Core {
    Core {
        pid: 4242,
        version: "0.20.0".into(),
        build: "demo".into(),
        fingerprint: "0123456789abcdef".into(),
        started: 1_763_500_000_000,
        sessions: 2,
        refused: None,
    }
}

fn link(state: LinkState, core: Option<Core>, error: Option<&str>) -> CoreState {
    CoreState { state, core, error: error.map(String::from), revision: 0 }
}

/// Every scene, in both themes: the title bar over an empty board, the core
/// banner in each state, a terminal in each of its looks, the board with
/// the fixture's chats (each layout, the sidebar collapsed, the peek, a
/// pane's menu and the peek's, and the notice of an import that failed),
/// and the panels.
pub fn scenes() -> Vec<Scene> {
    let up = link(LinkState::Up, Some(core()), None);
    let older = Core { version: "0.19.0".into(), build: "a1b2c3d".into(), sessions: 3, ..core() };
    let elevated = "The sessions are running in a Luna core started as administrator (2 running). Restart it to run them normally: \
                    they quit and resume in the new one.";
    let mut out = Vec::new();
    for theme in [ThemePref::Light, ThemePref::Dark] {
        // The answer first, then each event a revision newer.
        let one = |name, core: CoreState, then: Vec<CoreState>| Scene {
            name,
            theme,
            core: CoreState { revision: 1, ..core },
            then: then.into_iter().zip(2..).map(|(link, revision)| CoreState { revision, ..link }).collect(),
            terminal: None,
            kept: kept(&crate::state::WindowState::default()),
            menu: None,
            click: None,
            answers: Vec::new(),
            failures: Vec::new(),
            events: Vec::new(),
            act: None,
            window_act: None,
            drive: Vec::new(),
            output: None,
        };
        let term = |name, demo: TermDemo| Scene {
            terminal: Some(demo),
            kept: kept(&term_state()),
            ..one(name, if demo == TermDemo::Waiting { link(LinkState::Starting, None, None) } else { up.clone() }, vec![])
        };
        let chats = |name, state: crate::state::WindowState| Scene { kept: kept(&state), ..one(name, up.clone(), vec![]) };
        let mut overflow = chats("skills-overflow", chats_state(2, true));
        overflow.answers.push((cmd::ENSURE_CODEX_SESSION, json!({ "generation": 1, "skillsUnhidden": 12 })));
        let mut peek = chats("peek", chats_state(2, true));
        peek.click = Some("c-notes");
        let mut menu = chats("pane-menu", chats_state(2, true));
        menu.menu = Some(crate::board::MenuAt::Pane(0));
        // A bottom pane's, which opens over its bar where it does not fit
        // under it.
        let mut menu_bottom = chats("pane-menu-bottom", chats_state(4, true));
        menu_bottom.menu = Some(crate::board::MenuAt::Pane(2));
        let mut peek_menu = chats("peek-menu", chats_state(2, true));
        peek_menu.click = Some("c-notes");
        peek_menu.menu = Some(crate::board::MenuAt::Peek);
        let mut title_menu = chats("title-menu", chats_state(2, true));
        title_menu.window_act = Some(|w, _, cx| w.title_bar.update(cx, |t, cx| t.open_menu(cx)));
        let mut renaming = chats("sidebar-rename", chats_state(2, true));
        renaming.window_act = Some(|w, window, cx| w.sidebar.update(cx, |s, cx| s.rename("c-notes", window, cx)));
        let mut notice = one("import-notice", up.clone(), vec![]);
        notice.kept = json!({ "saved": null, "old": null, "oldError": "the old window's store could not be copied: access denied" });
        // The chat list's own file: one that could not be read, one that did
        // not all read (another version, a chat of a CLI this build does not
        // know), one that was no JSON.
        let mut unread = one("notice-read", up.clone(), vec![]);
        unread.kept =
            json!({ "readError": "The process cannot access the file because it is being used by another process. (os error 32)" });
        let mut part = chats("notice-kept", chats_state(2, true));
        let mut newer: Value = serde_json::from_str(&chats_state(2, true).to_text()).expect("the state reads back");
        newer["version"] = 2.into();
        newer["folders"][0]["chats"][0]["provider"] = "gemini".into();
        part.kept = json!({ "saved": newer, "kept": r"C:\Luna\window\chats.kept-20261119-101500.json" });
        let mut aside = one("notice-aside", up.clone(), vec![]);
        aside.kept = json!({ "saved": null, "aside": r"C:\Luna\window\chats.unreadable-20261119-101500.json" });
        out.extend([
            one("board", up.clone(), vec![]),
            one("core-starting", link(LinkState::Starting, None, None), vec![]),
            one("core-replacing", up.clone(), vec![up.clone(), link(LinkState::Starting, None, None)]),
            one("core-down", link(LinkState::Down, None, Some("the core is not reachable: pipe closed")), vec![]),
            one("core-mismatch", link(LinkState::Mismatch, Some(older.clone()), None), vec![]),
            one(
                "core-elevated",
                link(LinkState::Elevated, Some(Core { refused: Some("elevated".into()), ..core() }), Some(elevated)),
                vec![],
            ),
            term("term-colors", TermDemo::Colors),
            term("term-boxes", TermDemo::Boxes),
            term("term-selection", TermDemo::Selection),
            term("term-ime", TermDemo::Ime),
            term("term-exited", TermDemo::Exited),
            term("term-waiting", TermDemo::Waiting),
            chats("chats-1", chats_state(1, true)),
            chats("chats-2", chats_state(2, true)),
            overflow,
            chats("chats-3", chats_state(3, true)),
            chats("chats-4", chats_state(4, true)),
            chats("sidebar-collapsed", chats_state(2, false)),
            peek,
            menu,
            menu_bottom,
            peek_menu,
            title_menu,
            renaming,
            notice,
            unread,
            part,
            aside,
        ]);
        out.extend(crate::status::demo::scenes(theme));
        for (name, drive) in crate::panels::demo::scenes().into_iter().chain(crate::dialogs::demo::scenes()) {
            let mut scene = Scene { drive, ..one(name, up.clone(), vec![]) };
            match name {
                // Chats on the account the question names.
                "accounts-delete" => scene.kept = kept(&chats_state(2, true)),
                n if n.starts_with("dialog-") => scene.kept = kept(&chats_state(2, true)),
                "accounts-signin" => scene.output = Some(("login:codex:main", login_output())),
                _ => {}
            }
            out.push(scene);
        }
    }
    out
}

/// The arguments a theme's `set_window_theme` carries.
pub fn theme_args(pref: ThemePref) -> Value {
    json!({ "theme": pref.name() })
}
