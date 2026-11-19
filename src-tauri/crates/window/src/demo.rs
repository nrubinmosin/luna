//! A host made of fixtures, for the tests and the screenshot harness
//! (src/bin/shots.rs): it answers from a table, keeps every call and send it
//! got, and its mailbox is the test's or the scene's to fill. Compiled only
//! for those two, never into the app.

use crate::dto::{Core, CoreState, LinkState, ThemePref};
use crate::host::{Answer, Host, Inbox, Mailbox, OutputSink, cmd, mailbox};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct FakeHost {
    answers: Mutex<HashMap<String, Value>>,
    failures: Mutex<HashMap<String, String>>,
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
        if let Some(why) = self.failures.lock().unwrap().get(cmd).cloned() {
            return Box::pin(async move { Err(why) });
        }
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
    /// Then the driver's lines, one after another (panels/mod.rs `drive`):
    /// the panel opened and what is done in it.
    pub drive: Vec<&'static str>,
}

impl Scene {
    /// The host, and the inbox it feeds.
    pub fn host(&self) -> (Arc<FakeHost>, Mailbox, Inbox) {
        let host = FakeHost::new();
        crate::panels::demo::answer_all(&host);
        host.answer(cmd::CORE_STATE, serde_json::to_value(&self.core).unwrap());
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

/// Every scene, in both themes: the title bar over an empty board, the
/// core banner in each state, and the panels.
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
            drive: Vec::new(),
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
        ]);
        for (name, drive) in crate::panels::demo::scenes() {
            out.push(Scene { name, theme, core: CoreState { revision: 1, ..up.clone() }, then: Vec::new(), drive });
        }
    }
    out
}

/// The arguments a theme's `set_window_theme` carries.
pub fn theme_args(pref: ThemePref) -> Value {
    json!({ "theme": pref.name() })
}
