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

#[derive(Default)]
pub struct FakeHost {
    answers: Mutex<HashMap<String, Value>>,
    calls: Mutex<Vec<(String, Value)>>,
    sends: Mutex<Vec<(String, Value)>>,
    outputs: Mutex<HashMap<String, Arc<dyn OutputSink>>>,
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

    /// Output for `session`, as a host's reader hands it over: false when no
    /// sink is routed or it is gone.
    pub fn output(&self, session: &str, data: &[u8]) -> bool {
        let sink = self.outputs.lock().unwrap().get(session).cloned();
        sink.is_some_and(|s| s.bytes(data))
    }
}

impl Host for FakeHost {
    fn call(&self, cmd: &str, args: Value) -> Answer {
        self.calls.lock().unwrap().push((cmd.to_owned(), args));
        let answer = self.answers.lock().unwrap().get(cmd).cloned().unwrap_or(Value::Null);
        Box::pin(async move { Ok(answer) })
    }

    fn send(&self, cmd: &str, args: Value) -> Result<(), String> {
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
        host.answer(cmd::CORE_STATE, serde_json::to_value(&self.core).unwrap());
        host.answer(cmd::ENSURE_CLAUDE_SESSION, attached("", 1));
        let (mailbox, inbox) = mailbox();
        (host, mailbox, inbox)
    }

    /// Tells the models the scene's events, one by one: through the inbox,
    /// the state events would come down to the last (`event::LATEST`).
    #[cfg(feature = "shots")]
    pub(crate) fn tell(&self, models: &crate::models::Models, cx: &mut gpui::App) {
        for link in &self.then {
            crate::models::on_event(models, crate::event::CORE_STATE, serde_json::to_value(link).unwrap(), cx);
        }
    }
}

/// What a session's start answers: its backlog and its spawn's number.
pub fn attached(backlog: &str, generation: u64) -> Value {
    json!({ "backlog": backlog, "generation": generation })
}

/// The demo terminal's session: a Claude Code chat.
pub fn session() -> SessionSpec {
    SessionSpec { id: "demo-1".into(), provider: Provider::Claude, start: cmd::ENSURE_CLAUDE_SESSION, args: json!({ "id": "demo-1" }) }
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
/// banner in each state, and a terminal in each of its looks.
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
        };
        let term = |name, demo: TermDemo| Scene {
            terminal: Some(demo),
            ..one(name, if demo == TermDemo::Waiting { link(LinkState::Starting, None, None) } else { up.clone() }, vec![])
        };
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
        ]);
    }
    out
}

/// The arguments a theme's `set_window_theme` carries.
pub fn theme_args(pref: ThemePref) -> Value {
    json!({ "theme": pref.name() })
}
